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
        client::crypto::public_key(&key)?;
        Ok(Self {
            socket,
            url,
            ca: flags.get("--ca").map(PathBuf::from),
            key: Arc::new(key),
            owner: required("--owner")?,
            client: flags.get("--client-id").cloned(),
            via: flags.get("--via-topic").cloned(),
            owner_ca: flags.get("--owner-ca").map(PathBuf::from),
            owner_name: flags.get("--owner-name").cloned(),
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
        self.socket = Some(socket);
        self.welcome = Some(welcome);
        Ok(())
    }
    fn envelope(&self, response: Value) -> Value {
        let Some(w) = &self.welcome else {
            return json!({"response":response});
        };
        json!({"owner_id":w.service_id,"client_id":w.client_id,"restart_counter":w.restart_counter,"response":response})
    }
    pub(super) async fn call(&mut self, mut request: Option<Value>) -> Value {
        if self.connect().await.is_err() {
            return tool_result(
                json!({"code":"connection_failed","message":"Could not authenticate with the configured owner.","outcome_unknown":false}),
                true,
            );
        }
        let Some(ref mut request) = request else {
            return tool_result(self.envelope(json!({"type":"connection_info"})), false);
        };
        let Some(next) = self.sequence.checked_add(1) else {
            return tool_result(json!({"code":"request_id_exhausted"}), true);
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
                tool_result(self.envelope(response), error)
            }
            Err(_) => {
                // A lost response must never cause the same mutation to be replayed.
                self.socket = None;
                self.welcome = None;
                tool_result(
                    json!({"code":"transport_error","message":"Owner connection lost. The operation was not retried. Inspect state before repeating a mutation.","outcome_unknown":unknown}),
                    true,
                )
            }
        }
    }
}
