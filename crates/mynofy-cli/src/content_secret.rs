//! Startup/offline stdin secrets with controlled signal cancellation and terminal cleanup.
use mynofy_protocol::{BoxError, crypto};
use mynofy_server::{
    config::{Config, ContentEncryption},
    content::ContentSecret,
};
use std::io::{self, IsTerminal, Write};
use tokio::io::AsyncReadExt;
use zeroize::Zeroizing;
#[cfg(unix)]
struct EchoGuard<'a> {
    fd: std::os::fd::BorrowedFd<'a>,
    original: rustix::termios::Termios,
    discard_input: bool,
}
#[cfg(unix)]
impl Drop for EchoGuard<'_> {
    fn drop(&mut self) {
        if self.discard_input {
            let _ = rustix::termios::tcflush(self.fd, rustix::termios::QueueSelector::IFlush);
        }
        let _ = rustix::termios::tcsetattr(
            self.fd,
            rustix::termios::OptionalActions::Now,
            &self.original,
        );
    }
}
pub(super) async fn read(config: &Config) -> Result<Option<ContentSecret>, BoxError> {
    read_label(config, "Content").await
}
pub(super) async fn read_label(
    config: &Config,
    label: &str,
) -> Result<Option<ContentSecret>, BoxError> {
    let mode = config
        .content_storage
        .as_ref()
        .map_or(ContentEncryption::None, |storage| storage.encryption);
    if mode == ContentEncryption::None {
        return Ok(None);
    }
    // Register before changing the terminal: SIGINT/SIGTERM must unwind the guard.
    #[cfg(unix)]
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let stdin = io::stdin();
    let terminal = stdin.is_terminal();
    #[cfg(unix)]
    let mut echo = if terminal {
        use std::os::fd::AsFd;
        let original = rustix::termios::tcgetattr(stdin.as_fd())?;
        let mut hidden = original.clone();
        hidden
            .local_modes
            .remove(rustix::termios::LocalModes::ECHO | rustix::termios::LocalModes::ECHONL);
        rustix::termios::tcsetattr(
            stdin.as_fd(),
            rustix::termios::OptionalActions::Now,
            &hidden,
        )?;
        Some(EchoGuard {
            fd: stdin.as_fd(),
            original,
            discard_input: true,
        })
    } else {
        None
    };
    if terminal {
        eprint!(
            "{label} {}: ",
            if mode == ContentEncryption::PasswordStdin {
                "password"
            } else {
                "256-bit key (Base64)"
            }
        );
        io::stderr().flush()?;
    }
    let mut secret = Zeroizing::new(Vec::new());
    let input = async {
        let mut input = tokio::io::stdin();
        // Do not read ahead: offline rekey/restore can consume several separate lines.
        let mut byte = Zeroizing::new([0u8; 1]);
        while secret.len() < 4099 && input.read(byte.as_mut()).await? != 0 {
            secret.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        Ok::<(), BoxError>(())
    };
    #[cfg(unix)]
    let result = tokio::select! {
        biased;
        _ = interrupt.recv() => Err(BoxError::from("Content secret input interrupted by SIGINT")),
        _ = terminate.recv() => Err(BoxError::from("Content secret input interrupted by SIGTERM")),
        result = input => result,
    };
    #[cfg(not(unix))]
    let result = tokio::select! {
        result = tokio::signal::ctrl_c() => { result?; Err(BoxError::from("Content secret input interrupted")) },
        result = input => result,
    };
    if terminal {
        eprintln!();
    }
    result?;
    if secret.last() == Some(&b'\n') {
        secret.pop();
        if secret.last() == Some(&b'\r') {
            secret.pop();
        }
    }
    if secret.is_empty() || secret.len() > 4096 {
        return Err("Content stdin secret must contain 1..4096 bytes".into());
    }
    let secret = match mode {
        ContentEncryption::PasswordStdin => ContentSecret::Password(secret),
        ContentEncryption::KeyStdin => {
            let raw =
                std::str::from_utf8(&secret).map_err(|_| "Content key must be canonical Base64")?;
            ContentSecret::Key(Zeroizing::new(crypto::decode::<32>(raw).map_err(
                |_| "Content key must be canonical Base64 for exactly 32 bytes",
            )?))
        }
        ContentEncryption::None => return Ok(None),
    };
    #[cfg(unix)]
    if let Some(guard) = &mut echo {
        guard.discard_input = false;
    }
    Ok(Some(secret))
}
