use super::*;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use zeroize::Zeroizing;
#[derive(Clone)]
pub(super) struct Config {
    socket: Option<PathBuf>,
    url: Option<String>,
    ca: Option<PathBuf>,
    key: Arc<Zeroizing<Vec<u8>>>,
    owner: String,
    client: Option<String>,
    via: Option<String>,
    owner_ca: Option<PathBuf>,
    owner_name: Option<String>,
    #[cfg(unix)]
    checkpoint: Option<checkpoint::Shared>,
}
impl Config {
    pub(super) fn parse(flags: &BTreeMap<String, String>) -> Result<Self, BoxError> {
        let required = |name: &str| {
            flags
                .get(name)
                .cloned()
                .ok_or_else(|| BoxError::from(format!("Missing {name}")))
        };
        let socket = flags.get("--socket").map(PathBuf::from);
        let url = flags.get("--url").cloned();
        if socket.is_some() == url.is_some() || (socket.is_some() && flags.contains_key("--ca")) {
            return Err("Choose --socket or --url with --ca".into());
        }
        if url.is_some() {
            required("--ca")?;
        }
        if flags.contains_key("--via-topic") {
            required("--owner-ca")?;
        } else if flags.contains_key("--owner-ca") || flags.contains_key("--owner-name") {
            return Err("Owner TLS overrides require --via-topic".into());
        }
        let key = Zeroizing::new(std::fs::read(required("--key")?)?);
        let _public = client::crypto::public_key(&key)?;
        let owner = required("--owner")?;
        let client_id = flags.get("--client-id").cloned();
        #[cfg(unix)]
        let checkpoint = flags
            .get("--checkpoint")
            .map(|path| {
                let expected = client_id
                    .clone()
                    .unwrap_or_else(|| client::crypto::initial_client_id(&_public));
                checkpoint::Checkpoint::open(std::path::Path::new(path), &owner, &expected)
            })
            .transpose()?;
        #[cfg(not(unix))]
        if flags.contains_key("--checkpoint") {
            return Err("MCP checkpoints require Unix".into());
        }
        Ok(Self {
            socket,
            url,
            ca: flags.get("--ca").map(PathBuf::from),
            key: Arc::new(key),
            owner,
            client: client_id,
            via: flags.get("--via-topic").cloned(),
            owner_ca: flags.get("--owner-ca").map(PathBuf::from),
            owner_name: flags.get("--owner-name").cloned(),
            #[cfg(unix)]
            checkpoint,
        })
    }
}
enum Socket {
    Wss(Box<client::ClientSocket>),
    Remote(Box<client::OwnerSocket<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>>),
    #[cfg(unix)]
    Unix(Box<tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>>),
    #[cfg(unix)]
    RemoteUnix(Box<client::OwnerSocket<tokio::net::UnixStream>>),
}
impl Socket {
    async fn exchange(&mut self, request: &Value) -> Result<Value, BoxError> {
        let request = mynofy_protocol::pb::Frame::from_json(request.clone())?;
        let frame = match self {
            Self::Wss(ws) => client::exchange_frame(ws, &request).await,
            Self::Remote(ws) => client::exchange_frame(ws, &request).await,
            #[cfg(unix)]
            Self::Unix(ws) => client::exchange_frame(ws, &request).await,
            #[cfg(unix)]
            Self::RemoteUnix(ws) => client::exchange_frame(ws, &request).await,
        }?;
        super::tools::frame_output(&frame)
    }
}
pub(super) struct Backend {
    config: Config,
    socket: Option<Socket>,
    welcome: Option<protocol::pb::AuthWelcome>,
    sequence: u64,
}
impl Backend {
    pub(super) fn new(config: Config) -> Self {
        Self {
            config,
            socket: None,
            welcome: None,
            sequence: 0,
        }
    }
    async fn connect(&mut self) -> Result<(), BoxError> {
        if self.socket.is_some() {
            return Ok(());
        }
        let c = &self.config;
        let via = || -> Result<client::ViaOptions<'_>, BoxError> {
            Ok(client::ViaOptions {
                topic: c.via.as_deref().ok_or("Missing remote topic")?,
                owner: &c.owner,
                owner_certificate_identity: c.owner_name.as_deref().unwrap_or(&c.owner),
                owner_ca: c.owner_ca.as_deref().ok_or("Missing owner CA")?,
            })
        };
        let (socket, welcome) = if let Some(path) = &c.socket {
            #[cfg(not(unix))]
            {
                let _ = path;
                return Err("Unix sockets are unavailable on this platform".into());
            }
            #[cfg(unix)]
            {
                if c.via.is_some() {
                    let (ws, welcome) =
                        client::connect_unix_via_info(path, via()?, &c.key, c.client.as_deref())
                            .await?;
                    (Socket::RemoteUnix(Box::new(ws)), welcome)
                } else {
                    let (ws, welcome) = client::connect_unix_client_info(
                        path,
                        &c.key,
                        &c.owner,
                        c.client.as_deref(),
                    )
                    .await?;
                    (Socket::Unix(Box::new(ws)), welcome)
                }
            }
        } else {
            let url = c.url.as_deref().ok_or("Missing URL")?;
            let ca = c.ca.as_deref().ok_or("Missing CA")?;
            if c.via.is_some() {
                let (ws, welcome) =
                    client::connect_via_info(url, ca, via()?, &c.key, c.client.as_deref()).await?;
                (Socket::Remote(Box::new(ws)), welcome)
            } else {
                let (ws, welcome) =
                    client::connect_client_info(url, ca, &c.key, &c.owner, c.client.as_deref())
                        .await?;
                (Socket::Wss(Box::new(ws)), welcome)
            }
        };
        #[cfg(unix)]
        if let Some(state) = &self.config.checkpoint {
            state.lock().await.bind(&welcome)?;
        }
        self.socket = Some(socket);
        self.welcome = Some(welcome);
        Ok(())
    }
    fn envelope(&self, response: Value) -> Value {
        let Some(w) = &self.welcome else {
            return json!({"response":response});
        };
        let mut envelope = json!({"owner_id":w.service_id,"client_id":w.client_id,"restart_counter":w.restart_counter,"response":response});
        if let Some(boot_id) = &w.boot_id {
            envelope["boot_id"] = json!(boot_id);
        }
        envelope
    }
    pub(super) async fn call(&mut self, request: Option<Value>) -> Value {
        if let Some(request) = request.as_ref()
            && matches!(
                request["type"].as_str(),
                Some("poll_next" | "checkpoint_status" | "checkpoint_commit")
            )
        {
            #[cfg(unix)]
            if let Some(checkpoint) = self.config.checkpoint.clone() {
                return self.checkpoint_call(checkpoint, request).await;
            }
            return tool_result(
                json!({"code":"checkpoint_disabled","message":"Start MCP with --checkpoint to use local polling helpers."}),
                true,
            );
        }
        let (value, error) = self.owner_call(request).await;
        tool_result(value, error)
    }

    #[cfg(unix)]
    async fn checkpoint_call(&mut self, checkpoint: checkpoint::Shared, request: &Value) -> Value {
        let topic = request["topic"].as_str().expect("validated topic");
        match request["type"].as_str() {
            Some("checkpoint_status") => {
                let state = checkpoint.lock().await;
                return tool_result(state.status(topic, self.welcome.as_ref()), false);
            }
            Some("checkpoint_commit") => {
                let topic = topic.to_owned();
                let receipt = request["receipt"]
                    .as_str()
                    .expect("validated receipt")
                    .to_owned();
                if !checkpoint.lock().await.has_receipt(&topic, &receipt) {
                    return checkpoint::stale_receipt();
                }
                let (response, error) = self
                    .owner_call(Some(json!({
                        "type":"get_permissions","version":1,"topic":topic
                    })))
                    .await;
                if error {
                    return tool_result(response, true);
                }
                let welcome = self.welcome.clone();
                let state = request.get("state").cloned();
                return tokio::task::spawn_blocking(move || {
                    checkpoint.blocking_lock().commit(
                        &topic,
                        &receipt,
                        state.as_ref(),
                        welcome.as_ref(),
                    )
                })
                .await
                .unwrap_or_else(|_| tool_result(json!({"code":"checkpoint_write_failed"}), true));
            }
            _ => {}
        }
        if self.connect().await.is_err() {
            return tool_result(connection_error(), true);
        }
        let interval = Duration::from_millis(request["interval_ms"].as_u64().unwrap_or(30_000));
        let after = match checkpoint.lock().await.begin(topic, interval) {
            Ok(after) => after,
            Err(result) => return result,
        };
        let mut poll = json!({"type":"poll","version":1,"topic":topic});
        if request["replay"] != true
            && let Some(after) = after
        {
            poll["after"] = after.into();
        }
        if let Some(limit) = request.get("limit") {
            poll["limit"] = limit.clone();
        }
        let (mut envelope, error) = self.owner_call(Some(poll)).await;
        let mut checkpoint = checkpoint.lock().await;
        checkpoint.finish(topic, interval);
        let response = &envelope["response"];
        if error || response["type"] != "messages" {
            return tool_result(envelope, error);
        }
        let receipt = checkpoint.page(topic, response);
        match receipt {
            Ok(receipt) => {
                envelope["checkpoint_receipt"] = receipt.into();
                tool_result(envelope, false)
            }
            Err(_) => tool_result(json!({"code":"invalid_poll_page"}), true),
        }
    }

    async fn owner_call(&mut self, mut request: Option<Value>) -> (Value, bool) {
        let was_connected = self.socket.is_some();
        if self.connect().await.is_err() {
            return (connection_error(), true);
        }
        let Some(ref mut request) = request else {
            return (self.envelope(json!({"type":"connection_info","owner_context":if was_connected {"cached"} else {"authenticated_now"}})), false);
        };
        let Some(next) = self.sequence.checked_add(1) else {
            return (json!({"code":"request_id_exhausted"}), true);
        };
        self.sequence = next;
        request["id"] = format!("mcp-{next}").into();
        let unknown = matches!(request["type"].as_str(), Some("publish"));
        match self
            .socket
            .as_mut()
            .expect("connected socket")
            .exchange(request)
            .await
        {
            Ok(response) => {
                let error = response["type"] == "error";
                (self.envelope(response), error)
            }
            Err(_) => {
                // A lost response must never cause the same mutation to be replayed.
                self.socket = None;
                self.welcome = None;
                (
                    json!({"code":"transport_error","message":"Owner connection lost. The operation was not retried. Inspect state before repeating a mutation.","outcome_unknown":unknown}),
                    true,
                )
            }
        }
    }
}

fn connection_error() -> Value {
    json!({"code":"connection_failed","message":"Could not authenticate with the configured owner.","outcome_unknown":false})
}
