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
    let permits = Arc::new(Semaphore::new(4));
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
                tasks.spawn(async move {let _permit=permit;let _=timeout(Duration::from_secs(5), connection(stream,engine)).await;});
            },
            _=tasks.join_next(),if !tasks.is_empty()=>{},
        }
    }
}
fn error(code: &str, detail: &str) -> Value {
    serde_json::json!({"version":VERSION,"ok":false,"error":{"code":code,"detail":detail}})
}
async fn connection(mut stream: UnixStream, engine: Arc<Mutex<Engine>>) -> Result<(), BoxError> {
    let mut input = vec![0; MAX_REQUEST_BYTES + 1];
    let mut len = 0;
    let parsed = loop {
        let n = stream.read(&mut input[len..]).await?;
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
            let mutating = matches!(
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
            let (result, confirmation) = {
                let mut state = engine
                    .lock()
                    .map_err(|_| "Service state lock is poisoned")?;
                let result = state.admin(request.command, now_ms());
                let confirmation = (mutating && result.is_ok())
                    .then(|| state.security_confirmation())
                    .flatten();
                (result, confirmation)
            };
            let result = if let Some(confirmation) = confirmation
                && confirmation.confirm().await.is_err()
            {
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
        return Err("Admin request exceeds size limit".into());
    }
    timeout(Duration::from_secs(5), async {
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
}
