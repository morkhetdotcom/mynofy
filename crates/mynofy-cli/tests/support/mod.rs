//! Existing CLI integration scenarios must run under a real unprivileged identity.
//! A root test runner re-executes the selected test, rather than weakening the CLI policy.
#[allow(dead_code)]
pub fn under_unprivileged_identity(test: &str) -> bool {
    #[cfg(target_os = "linux")]
    let root = rustix::process::geteuid().is_root();
    #[cfg(not(target_os = "linux"))]
    let root = nix::unistd::geteuid().is_root();
    if !root {
        return false;
    }
    let output = std::process::Command::new("python3")
        .args(["-c", "import os,sys; os.setgroups([]); os.setgid(65534); os.setuid(65534); os.execv(sys.argv[1], sys.argv[1:])"])
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "unprivileged {test}:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

#[allow(dead_code)]
pub async fn run_unix_fixture(
    bound: mynofy_server::transport::BoundUnixSocket,
    config: mynofy_server::config::Config,
) -> Result<(), mynofy_server::BoxError> {
    use mynofy_server::transport;
    if let Some(storage) = &config.security_storage {
        if !storage.directory.exists() {
            mynofy_server::initialize_security(&config)?;
        }
        transport::run_unix(bound, config).await
    } else {
        transport::run_unix_prepared(bound, transport::prepare_volatile(config)?).await
    }
}

#[allow(dead_code)]
pub async fn run_wss_fixture(
    listener: tokio::net::TcpListener,
    config: mynofy_server::config::Config,
) -> Result<(), mynofy_server::BoxError> {
    use mynofy_server::transport;
    transport::run_prepared(listener, transport::prepare_volatile(config)?).await
}

/// Unix bind makes the path visible before the service sets its final mode.
#[cfg(unix)]
#[allow(dead_code)]
pub fn unix_socket_ready(path: &std::path::Path) -> bool {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    std::fs::symlink_metadata(path)
        .is_ok_and(|meta| meta.file_type().is_socket() && meta.mode() & 0o777 == 0o600)
}
