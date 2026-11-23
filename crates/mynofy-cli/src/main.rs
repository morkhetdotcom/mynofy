#![forbid(unsafe_code)]
use mynofy_client as client;
use mynofy_protocol::{BoxError, crypto};
use mynofy_server::{config::Config, transport as server};
use std::{collections::BTreeMap, path::Path};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
#[cfg(unix)]
mod admin_cli;
#[cfg(unix)]
mod progress_file;
#[cfg(unix)]
mod storage_cli;

mod content_secret;
mod mcp;
mod privilege;

fn main() {
    if let Err(err) = bootstrap() {
        eprintln!("mynofy: {err}");
        std::process::exit(1);
    }
}
fn bootstrap() -> Result<(), BoxError> {
    let args = privilege::bootstrap(std::env::args_os().skip(1).collect())?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(run_with_startup_signals(args));
    // Tokio stdin uses a blocking OS read that cannot be aborted by dropping its
    // async task. All command cleanup runs above; bound leftover runtime shutdown.
    runtime.shutdown_timeout(std::time::Duration::from_millis(250));
    result
}
// Tokio signal handlers remain installed after a prompt receiver is dropped.
// Keep receivers for the whole startup/offline command so later signals cannot
// disappear between secret prompts, store preparation and the serving loop.
async fn run_with_startup_signals(args: Vec<String>) -> Result<(), BoxError> {
    #[cfg(unix)]
    if matches!(args.first().map(String::as_str), Some("serve" | "storage")) {
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        return tokio::select! {
            biased;
            // Existing prompt/service handlers get their normal cleanup/exit first.
            result = run(args) => result,
            _ = interrupt.recv() => Err("Command interrupted by SIGINT".into()),
            _ = terminate.recv() => Err("Command interrupted by SIGTERM".into()),
        };
    }
    run(args).await
}
async fn run(args: Vec<String>) -> Result<(), BoxError> {
    let mut args = args.into_iter();
    let cmd = args.next().unwrap_or_else(|| "help".into());
    if cmd == "storage" {
        #[cfg(unix)]
        return storage_cli::run(args.collect()).await;
        #[cfg(not(unix))]
        return Err("Offline storage requires Unix".into());
    }
    if cmd == "admin" {
        #[cfg(unix)]
        return admin_cli::run(args.collect()).await;
        #[cfg(not(unix))]
        return Err("Admin Unix sockets are unavailable on this platform".into());
    }
    let mut flags = BTreeMap::new();
    while let Some(flag) = args.next() {
        if !flag.starts_with("--") || flags.contains_key(&flag) {
            return Err("Expected unique --name value arguments".into());
        }
        let value = args.next().ok_or("Missing argument value")?;
        flags.insert(flag, value);
    }
    let allowed: &[&str] = match cmd.as_str() {
        "serve" | "check-config" => &["--config"],
        "config-convert" => &["--config", "--out"],
        "keygen" => &["--out"],
        "adapter" => &[
            "--url",
            "--ca",
            "--socket",
            "--key",
            "--owner",
            "--via-topic",
            "--owner-ca",
            "--owner-name",
            "--client-id",
            "--rules",
        ],
        "mcp" => &[
            "--url",
            "--ca",
            "--socket",
            "--key",
            "--owner",
            "--client-id",
            "--via-topic",
            "--owner-ca",
            "--owner-name",
        ],
        "client" => &[
            "--progress",
            "--url",
            "--ca",
            "--socket",
            "--key",
            "--owner",
            "--via-topic",
            "--owner-ca",
            "--owner-name",
            "--client-id",
        ],
        "rotate" => &[
            "--url",
            "--ca",
            "--socket",
            "--key",
            "--owner",
            "--via-topic",
            "--owner-ca",
            "--owner-name",
            "--client-id",
            "--new-key",
        ],
        _ => &[],
    };
    if flags.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("Unknown command argument".into());
    }
    let required = |key: &str| {
        flags
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| BoxError::from(format!("Missing {key}")))
    };
    match cmd.as_str() {
        "mcp" => mcp::run(&flags).await?,
        "serve" => {
            let config = Config::load(Path::new(required("--config")?))?;
            config.require_local_owner()?;
            let secret = content_secret::read(&config).await?;
            let prepared = server::prepare(config.clone(), secret)?;
            if let Some(address) = config.service.listen {
                let listener = tokio::net::TcpListener::bind(address).await?;
                eprintln!(
                    "mynofy {} listening on wss://{}",
                    config.service.id,
                    listener.local_addr()?
                );
                tokio::select! {result=server::run_prepared(listener,prepared)=>result?,result=shutdown()=>result?,}
            } else {
                #[cfg(unix)]
                {
                    let path = config
                        .service
                        .unix_socket
                        .as_ref()
                        .ok_or("Missing Unix socket path")?;
                    let bound = server::bind_unix(path)?;
                    eprintln!(
                        "mynofy {} listening on Unix socket {}",
                        config.service.id,
                        path.display()
                    );
                    tokio::select! {result=server::run_unix_prepared(bound,prepared)=>result?,result=shutdown()=>result?,}
                }
                #[cfg(not(unix))]
                return Err("Unix sockets are unavailable on this platform".into());
            }
        }
        "check-config" => {
            let config = Config::load(Path::new(required("--config")?))?;
            println!(
                "Valid configuration: {} ({} topics, {} clients)",
                config.service.id,
                config.topics.len(),
                config.clients.len()
            );
        }
        "config-convert" => {
            let config = Config::load(Path::new(required("--config")?))?;
            let path = Path::new(required("--out")?);
            let data = if mynofy_server::config::is_yaml(path) {
                config.to_yaml()?
            } else {
                format!("{}\n", serde_json::to_string_pretty(&config)?)
            };
            write_private_file(path, data.as_bytes())?;
        }
        "keygen" => {
            let path = Path::new(required("--out")?);
            let key = zeroize::Zeroizing::new(crypto::generate_key()?);
            let public = crypto::public_key(&key)?;
            write_private_file(path, &key)?;
            println!(
                "{}",
                serde_json::json!({"client_id":crypto::initial_client_id(&public),"token_id":crypto::token_id(&public),"algorithm":"ed25519","public_key":crypto::encode(&public)})
            );
        }
        "client" | "rotate" | "adapter" => {
            let key = zeroize::Zeroizing::new(std::fs::read(required("--key")?)?);
            let via = || -> Result<client::ViaOptions<'_>, BoxError> {
                Ok(client::ViaOptions {
                    topic: required("--via-topic")?,
                    owner: required("--owner")?,
                    owner_certificate_identity: flags
                        .get("--owner-name")
                        .map(String::as_str)
                        .unwrap_or(required("--owner")?),
                    owner_ca: Path::new(required("--owner-ca")?),
                })
            };
            if !flags.contains_key("--via-topic")
                && (flags.contains_key("--owner-ca") || flags.contains_key("--owner-name"))
            {
                return Err("--owner-ca/--owner-name require --via-topic".into());
            }
            if let Some(path) = flags.get("--socket") {
                if flags.contains_key("--url") || flags.contains_key("--ca") {
                    return Err("Use either --socket or --url with --ca".into());
                }
                #[cfg(unix)]
                {
                    if flags.contains_key("--via-topic") {
                        let (ws, welcome) = client::connect_unix_via_info(
                            Path::new(path),
                            via()?,
                            &key,
                            flags.get("--client-id").map(String::as_str),
                        )
                        .await?;
                        client_command(ws, welcome, &cmd, &flags, &key).await?;
                    } else {
                        let (ws, welcome) = client::connect_unix_client_info(
                            Path::new(path),
                            &key,
                            required("--owner")?,
                            flags.get("--client-id").map(String::as_str),
                        )
                        .await?;
                        client_command(ws, welcome, &cmd, &flags, &key).await?;
                    }
                }
                #[cfg(not(unix))]
                return Err("Unix sockets are unavailable on this platform".into());
            } else {
                if flags.contains_key("--via-topic") {
                    let (ws, welcome) = client::connect_via_info(
                        required("--url")?,
                        Path::new(required("--ca")?),
                        via()?,
                        &key,
                        flags.get("--client-id").map(String::as_str),
                    )
                    .await?;
                    client_command(ws, welcome, &cmd, &flags, &key).await?;
                } else {
                    let (ws, welcome) = client::connect_client_info(
                        required("--url")?,
                        Path::new(required("--ca")?),
                        &key,
                        required("--owner")?,
                        flags.get("--client-id").map(String::as_str),
                    )
                    .await?;
                    client_command(ws, welcome, &cmd, &flags, &key).await?;
                }
            }
        }
        "help" | "--help" => println!(
            "Global Unix options: --run-as-uid UID --run-as-gid GID (required when privileged)\nmynofy admin help\nmynofy storage help\nmynofy serve --config FILE\nmynofy check-config --config FILE\nmynofy config-convert --config FILE --out NEW.yaml\nmynofy keygen --out CLIENT.pk8\nmynofy mcp --socket PATH --key CLIENT.pk8 --owner ID\nmynofy mcp --url wss://HOST:PORT --ca CA.pem --key CLIENT.pk8 --owner ID\nmynofy client --url wss://HOST:PORT --ca CA.pem --key CLIENT.pk8 --owner ID [--client-id STABLE_ID] [--progress FILE]\nmynofy rotate --url wss://HOST:PORT --ca CA.pem --key OLD.pk8 --new-key NEW.pk8 --owner ID [--client-id STABLE_ID]\nmynofy client --socket PATH --key CLIENT.pk8 --owner ID [--client-id STABLE_ID] [--progress FILE]\nmynofy rotate --socket PATH --key OLD.pk8 --new-key NEW.pk8 --owner ID [--client-id STABLE_ID]\nFor remote owners add --via-topic TOPIC --owner-ca OWNER_CA.pem [--owner-name OWNER_DNS]; --ca verifies the proxy, --owner-ca the end-to-end owner.\nThe client reads JSONL requests from stdin and writes JSONL responses to stdout."
        ),
        _ => return Err("Unknown command; run mynofy help".into()),
    }
    Ok(())
}

async fn client_command<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    mut ws: tokio_tungstenite::WebSocketStream<S>,
    welcome: mynofy_protocol::pb::AuthWelcome,
    cmd: &str,
    flags: &BTreeMap<String, String>,
    key: &[u8],
) -> Result<(), BoxError> {
    if cmd == "adapter" {
        let rules = std::fs::read_to_string(flags.get("--rules").ok_or("Missing --rules")?)?;
        let rules: client::adapter::AdapterConfig =
            serde_json::from_value(mynofy_protocol::strict_json(&rules)?)?;
        let participant = flags
            .get("--client-id")
            .cloned()
            .unwrap_or(crypto::initial_client_id(&crypto::public_key(key)?));
        tokio::select! {result=client::adapter::run(ws,rules,&participant)=>result?,result=shutdown()=>result?,}
    } else if cmd == "rotate" {
        let new = zeroize::Zeroizing::new(crypto::generate_key()?);
        write_private_file(
            Path::new(flags.get("--new-key").ok_or("Missing --new-key")?),
            &new,
        )?;
        let client = flags
            .get("--client-id")
            .cloned()
            .unwrap_or(crypto::initial_client_id(&crypto::public_key(key)?));
        let done = client::rotate_client(
            &mut ws,
            key,
            &new,
            flags.get("--owner").ok_or("Missing --owner")?,
            &client,
        )
        .await?;
        println!("{done}");
    } else {
        #[cfg(unix)]
        if let Some(path) = flags.get("--progress") {
            let (file, progress) = progress_file::ProgressFile::open(Path::new(path))?;
            let mut tracked = client::progress::RestartClient::new(ws, welcome, progress)?;
            file.save(tracked.progress())?;
            let mut lines = tokio::io::BufReader::new(tokio::io::stdin());
            let mut out = tokio::io::stdout();
            while let Some(line) = client_line(&mut lines).await? {
                if line.trim().is_empty() {
                    continue;
                }
                let request = mynofy_protocol::strict_json(&line)?;
                let response = tracked.exchange(&request).await?;
                out.write_all(serde_json::to_string(&response)?.as_bytes())
                    .await?;
                out.write_all(b"\n").await?;
                out.flush().await?;
                file.save(tracked.progress())?;
            }
            let (mut socket, _) = tracked.into_parts();
            socket.close(None).await?;
            return Ok(());
        }
        client_stdio(ws).await?;
    }
    Ok(())
}

async fn shutdown() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {result=tokio::signal::ctrl_c()=>result,_=terminate.recv()=>Ok(())}
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}

const MAX_CLIENT_LINE: usize = 64 * 1024;
async fn client_line<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<Option<String>, BoxError> {
    let mut bytes = Vec::new();
    loop {
        let input = reader.fill_buf().await?;
        if input.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Ok(Some(String::from_utf8(bytes)?))
            };
        }
        let end = input.iter().position(|b| *b == b'\n');
        let count = end.map_or(input.len(), |i| i + 1);
        if count > MAX_CLIENT_LINE - bytes.len() {
            return Err("Client JSONL input exceeds 64 KiB".into());
        }
        bytes.extend_from_slice(&input[..count]);
        reader.consume(count);
        if end.is_some() {
            return Ok(Some(String::from_utf8(bytes)?));
        }
    }
}

async fn client_stdio<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    mut ws: tokio_tungstenite::WebSocketStream<S>,
) -> std::result::Result<(), BoxError> {
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin());
    let mut out = tokio::io::stdout();
    while let Some(line) = client_line(&mut lines).await? {
        if line.trim().is_empty() {
            continue;
        }
        let request = mynofy_protocol::strict_json(&line)?;
        let response = client::exchange(&mut ws, &request).await?;
        out.write_all(serde_json::to_string(&response)?.as_bytes())
            .await?;
        out.write_all(b"\n").await?;
        out.flush().await?;
    }
    ws.close(None).await?;
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> std::result::Result<(), BoxError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    std::io::Write::write_all(&mut file, bytes)?;
    file.sync_all()?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod client_input_tests {
    use super::*;
    #[tokio::test]
    async fn bounded_client_lines_reject_unterminated_input_without_waiting_for_eof() {
        let (mut writer, reader) = tokio::io::duplex(MAX_CLIENT_LINE * 2);
        writer
            .write_all(&vec![b'x'; MAX_CLIENT_LINE + 1])
            .await
            .unwrap();
        let mut reader = tokio::io::BufReader::new(reader);
        let error =
            tokio::time::timeout(std::time::Duration::from_secs(1), client_line(&mut reader))
                .await
                .unwrap()
                .unwrap_err();
        assert!(error.to_string().contains("64 KiB"));
        let mut reader = tokio::io::BufReader::new(&b"one\ntwo"[..]);
        assert_eq!(
            client_line(&mut reader).await.unwrap().as_deref(),
            Some("one\n")
        );
        assert_eq!(
            client_line(&mut reader).await.unwrap().as_deref(),
            Some("two")
        );
        assert!(client_line(&mut reader).await.unwrap().is_none());
    }
}
