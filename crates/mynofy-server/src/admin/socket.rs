use super::*;
use crate::{
    protocol,
    transport::{BoundUnixSocket, now_ms},
};
use std::{
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::Path,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::Semaphore,
    task::JoinSet,
    time::{Instant, timeout},
};

pub(super) async fn serve(
    bound: BoundUnixSocket,
    engine: Arc<Mutex<Engine>>,
) -> Result<(), BoxError> {
    // Four commands may wait for durability. Two bounded socket slots remain for status.
    let permits = Arc::new(Semaphore::new(6));
    let commands = Arc::new(Semaphore::new(4));
    let mut tasks = JoinSet::new();
    let mut tokens = 20.;
    let mut updated = Instant::now();
    loop {
        tokio::select! {
            accepted=bound.listener.accept()=>{
                let (stream,_)=accepted?;
                let Ok(credentials)=stream.peer_cred() else{continue;};
                if credentials.uid()!=bound.uid {continue;}
                let now=Instant::now();tokens=(tokens+now.duration_since(updated).as_secs_f64()*10.).min(20.);updated=now;
                if tokens<1. {continue;}tokens-=1.;
                let Ok(permit)=permits.clone().try_acquire_owned() else{continue};
                let engine=engine.clone();
                let commands=commands.clone();
                tasks.spawn(async move {let _permit=permit;let _=timeout(Duration::from_secs(9), connection(stream,engine,commands,Duration::from_secs(7))).await;});
            },
            _=tasks.join_next(),if !tasks.is_empty()=>{},
        }
    }
}
fn error(code: &str, detail: &str) -> Value {
    serde_json::json!({"version":VERSION,"ok":false,"error":{"code":code,"detail":detail}})
}
async fn connection(
    mut stream: UnixStream,
    engine: Arc<Mutex<Engine>>,
    commands: Arc<Semaphore>,
    start_budget: Duration,
) -> Result<(), BoxError> {
    let mut input = vec![0; MAX_REQUEST_BYTES + 1];
    let mut len = 0;
    let start = Instant::now();
    let read_deadline = start + Duration::from_secs(2);
    let confirmation_deadline = start + start_budget;
    let parsed = loop {
        let n = tokio::time::timeout_at(read_deadline, stream.read(&mut input[len..])).await??;
        if n == 0 {
            return Ok(());
        }
        let end = len + n;
        if let Some(newline) = input[len..end].iter().position(|b| *b == b'\n') {
            let end = len + newline;
            break if end > MAX_REQUEST_BYTES {
                Err(())
            } else {
                std::str::from_utf8(&input[..end])
                    .map_err(|_| ())
                    .and_then(|raw| protocol::strict_json(raw).map_err(|_| ()))
                    .and_then(|v| serde_json::from_value::<Request>(v).map_err(|_| ()))
            };
        }
        len = end;
        if len == input.len() {
            break Err(());
        }
    };
    #[derive(Serialize)]
    struct Reply {
        version: u8,
        ok: bool,
        result: ResultBody,
    }
    let response = match parsed {
        Ok(request) if request.version == VERSION => {
            let _command_permit =
                if matches!(request.command, Command::Status {} | Command::Health {}) {
                    None
                } else if let Ok(permit) = commands.try_acquire() {
                    Some(permit)
                } else {
                    stream
                        .write_all(&encode(&error(
                            "admin_busy",
                            "Admin command slots are occupied.",
                        ))?)
                        .await?;
                    stream.write_all(b"\n").await?;
                    stream.shutdown().await?;
                    return Ok(());
                };
            let confirms_security = matches!(
                request.command,
                Command::ClientAdd { .. }
                    | Command::ClientDisable { .. }
                    | Command::KeyAdd { .. }
                    | Command::KeyRevoke { .. }
                    | Command::AclSet { .. }
                    | Command::PeerSet { .. }
                    | Command::PeerDelete { .. }
                    | Command::PeerDisable { .. }
                    | Command::PeerCertificateRevoke { .. }
                    | Command::TopicCreate { .. }
                    | Command::TopicDelete { .. }
            );
            let needs_start_budget = confirms_security
                || matches!(
                    request.command,
                    Command::TopicPolicy { .. }
                        | Command::TopicStorage { .. }
                        | Command::ContentSave { .. }
                );
            let (result, confirmation) = {
                let mut state = engine
                    .lock()
                    .map_err(|_| "Service state lock is poisoned")?;
                let result = if needs_start_budget && Instant::now() >= confirmation_deadline {
                    Err(protocol::ApiError::new(
                        "admin_busy",
                        "Request could not start within its time budget.",
                    ))
                } else {
                    state.admin(request.command, now_ms())
                };
                let confirmation = (confirms_security && result.is_ok())
                    .then(|| state.security_confirmation())
                    .flatten();
                (result, confirmation)
            };
            let result = if let Some(confirmation) = confirmation
                && !matches!(
                    tokio::time::timeout_at(confirmation_deadline, confirmation.confirm()).await,
                    Ok(Ok(()))
                ) {
                Err(protocol::ApiError::new(
                    "security_storage_unsynced",
                    "RAM change applied; durable confirmation failed. Inspect security status before retrying.",
                ))
            } else {
                result
            };
            match result {
                Ok(result) => {
                    if let ResultBody::Configuration(config) = &result {
                        config
                            .validate()
                            .map_err(|_| "Invalid administrative snapshot")?;
                    }
                    encode(&Reply {
                        version: VERSION,
                        ok: true,
                        result,
                    })?
                }
                Err(e) => encode(&error(e.code, e.detail))?,
            }
        }
        _ => encode(&error(
            "invalid_request",
            "Expected a strict admin v1 JSON request.",
        ))?,
    };
    stream.write_all(&response).await?;
    stream.write_all(b"\n").await?;
    stream.shutdown().await?;
    Ok(())
}
/// One bounded JSON request per connection. No automatic mutation retry.
pub async fn request(path: &Path, command: Command) -> Result<Value, BoxError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket() || metadata.mode() & 0o777 != 0o600 {
        return Err("Admin endpoint must be a Unix socket with mode 0600".into());
    }
    let raw = serde_json::to_vec(&Request {
        version: VERSION,
        command,
    })?;
    if raw.len() > MAX_REQUEST_BYTES {
        return Err(
            format!("Encoded admin request exceeds the {MAX_REQUEST_BYTES}-byte limit").into(),
        );
    }
    timeout(Duration::from_secs(10), async {
        let mut stream = UnixStream::connect(path).await?;
        if stream.peer_cred()?.uid() != metadata.uid() {
            return Err::<Value, BoxError>("Admin socket owner changed".into());
        }
        stream.write_all(&raw).await?;
        stream.write_all(b"\n").await?;
        let mut response = Vec::new();
        stream
            .take((MAX_RESPONSE_BYTES + 2) as u64)
            .read_to_end(&mut response)
            .await?;
        if response.len() > MAX_RESPONSE_BYTES + 1 {
            return Err("Admin response exceeds size limit".into());
        }
        let value = protocol::strict_json(std::str::from_utf8(&response)?)?;
        if value["version"] != VERSION || !value["ok"].is_boolean() {
            return Err("Invalid admin response".into());
        }
        Ok(value)
    })
    .await?
}

struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}
impl std::io::Write for LimitedWriter {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(input.len()) > self.limit {
            self.exceeded = true;
            return Err(std::io::Error::other("Admin response exceeds size limit"));
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encode(value: &impl Serialize) -> Result<Vec<u8>, BoxError> {
    let mut writer = LimitedWriter {
        bytes: Vec::new(),
        limit: MAX_RESPONSE_BYTES,
        exceeded: false,
    };
    if let Err(error) = serde_json::to_writer(&mut writer, value) {
        if writer.exceeded {
            return Ok(serde_json::to_vec(&self::error(
                "resource_exhausted",
                "Admin response exceeds size limit.",
            ))?);
        }
        return Err(error.into());
    }
    Ok(writer.bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_serialization_stops_at_the_size_limit() {
        let mut writer = LimitedWriter {
            bytes: Vec::new(),
            limit: 32,
            exceeded: false,
        };
        assert!(
            serde_json::to_writer(&mut writer, &serde_json::json!({"data":"x".repeat(1024)}))
                .is_err()
        );
        assert!(writer.exceeded);
        assert!(writer.bytes.len() <= 32);
    }

    #[tokio::test]
    async fn live_policy_storage_and_save_cannot_start_after_deadline() {
        let config =
            serde_json::from_str(include_str!("../../../../config/scenarios/local.json")).unwrap();
        let engine = Arc::new(Mutex::new(Engine::new(config, now_ms()).unwrap()));
        for command in [
            serde_json::json!({"op":"topic_policy","topic":"local/events","policy":{
                "default_ttl_seconds":1,"max_ttl_seconds":2
            }}),
            serde_json::json!({"op":"topic_storage","topic":"local/events","storage":{
                "mode":"manual"
            }}),
            serde_json::json!({"op":"content_save"}),
        ] {
            let (locked, guard_acquired) = tokio::sync::oneshot::channel();
            let (release, wait_for_release) = std::sync::mpsc::channel();
            let state = engine.clone();
            let holder = std::thread::spawn(move || {
                let _guard = state.lock().unwrap();
                locked.send(()).unwrap();
                let _ = wait_for_release.recv();
            });
            guard_acquired.await.unwrap();

            let (mut client, server) = UnixStream::pair().unwrap();
            let slots = Arc::new(Semaphore::new(1));
            let budget = Duration::from_millis(40);
            let server = server.into_std().unwrap();
            let command_engine = engine.clone();
            let command_slots = slots.clone();
            let task = std::thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async {
                        let server = UnixStream::from_std(server).unwrap();
                        connection(server, command_engine, command_slots, budget).await
                    })
            });
            client
                .write_all(
                    format!(
                        "{}\n",
                        serde_json::json!({
                            "version":VERSION, "command":command,
                        })
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            timeout(Duration::from_secs(1), async {
                while slots.available_permits() != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            tokio::time::sleep(budget).await;
            release.send(()).unwrap();
            holder.join().unwrap();
            let mut reply = Vec::new();
            client.read_to_end(&mut reply).await.unwrap();
            task.join().unwrap().unwrap();
            let reply: Value = serde_json::from_slice(&reply).unwrap();
            assert_eq!(reply["error"]["code"], "admin_busy", "{reply}");
        }
    }
}
