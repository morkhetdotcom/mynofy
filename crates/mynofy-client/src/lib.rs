#![forbid(unsafe_code)]
//! Authenticated pull client for WSS and same-user Unix sockets. No server-core dependency.
use futures_util::{SinkExt, StreamExt};
pub use mynofy_protocol::{self as protocol, BoxError, crypto};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    time::timeout,
};
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::{
    TlsConnector,
    rustls::{self, pki_types::ServerName},
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::WebSocketConfig},
};
fn certs(
    path: &Path,
) -> std::result::Result<Vec<rustls::pki_types::CertificateDer<'static>>, BoxError> {
    let certs = rustls::pki_types::CertificateDer::pem_file_iter(path)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        return Err("No PEM certificates found".into());
    }
    Ok(certs)
}
pub fn client_tls(ca: &Path) -> std::result::Result<Arc<rustls::ClientConfig>, BoxError> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in certs(ca)? {
        roots.add(cert)?;
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Arc::new(config))
}
fn ws_config(max: usize) -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(8192.min(max))
        .write_buffer_size(16_384.min(max))
        .max_message_size(Some(max))
        .max_frame_size(Some(max))
        .max_write_buffer_size(max + 32_768)
}
pub type ClientSocket = WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>;
pub async fn connect_client(
    url: &str,
    ca: &Path,
    pkcs8: &[u8],
    owner: &str,
    expected_client: Option<&str>,
) -> std::result::Result<ClientSocket, BoxError> {
    Ok(connect_client_info(url, ca, pkcs8, owner, expected_client)
        .await?
        .0)
}
pub async fn connect_client_info(
    url: &str,
    ca: &Path,
    pkcs8: &[u8],
    owner: &str,
    expected_client: Option<&str>,
) -> std::result::Result<(ClientSocket, protocol::pb::AuthWelcome), BoxError> {
    authenticate(open_wss(url, ca).await?, pkcs8, owner, expected_client).await
}
async fn open_wss(url: &str, ca: &Path) -> Result<ClientSocket, BoxError> {
    let uri: tokio_tungstenite::tungstenite::http::Uri = url.parse()?;
    if uri.scheme_str() != Some("wss") {
        return Err("Only wss:// is supported".into());
    }
    let host = uri.host().ok_or("Missing WSS host")?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port = uri.port_u16().unwrap_or(443);
    let stream = timeout(Duration::from_secs(10), TcpStream::connect((host, port))).await??;
    stream.set_nodelay(true)?;
    let name = ServerName::try_from(host.to_owned())?;
    let tls = timeout(
        Duration::from_secs(10),
        TlsConnector::from(client_tls(ca)?).connect(name, stream),
    )
    .await??;
    let (ws, _) = timeout(
        Duration::from_secs(10),
        tokio_tungstenite::client_async_with_config(url, tls, Some(ws_config(4 * 1024 * 1024))),
    )
    .await??;
    Ok(ws)
}
async fn authenticate<S: AsyncRead + AsyncWrite + Unpin>(
    mut ws: WebSocketStream<S>,
    pkcs8: &[u8],
    owner: &str,
    expected_client: Option<&str>,
) -> std::result::Result<(WebSocketStream<S>, protocol::pb::AuthWelcome), BoxError> {
    let public = crypto::public_key(pkcs8)?;
    let client = expected_client
        .map(str::to_owned)
        .unwrap_or_else(|| crypto::initial_client_id(&public));
    let hello = json!({"version":1,"type":"auth_hello","id":"auth-hello","token_id":crypto::token_id(&public)});
    let challenge = exchange(&mut ws, &hello).await?;
    if challenge["type"] != "auth_challenge"
        || challenge["request_id"] != "auth-hello"
        || challenge["service_id"] != owner
    {
        return Err("Owner authentication challenge mismatch".into());
    }
    let nonce = crypto::decode(challenge["nonce"].as_str().ok_or("Missing challenge")?)?;
    let signature = crypto::sign(pkcs8, &crypto::auth_bytes(&nonce, &public, owner))?;
    let welcome=exchange(&mut ws,&json!({"version":1,"type":"auth_proof","id":"auth-proof","auth_id":"auth-hello","signature":signature})).await?;
    if welcome["type"] != "auth_welcome"
        || welcome["request_id"] != "auth-proof"
        || welcome["service_id"] != owner
        || welcome["client_id"] != client
        || welcome["restart_counter"]
            .as_u64()
            .is_none_or(|counter| counter > u32::MAX as u64)
    {
        return Err("Owner/client welcome mismatch".into());
    }
    let frame = protocol::pb::Frame::from_json(welcome)?;
    let Some(protocol::pb::Body::AuthWelcome(welcome)) = frame.body else {
        return Err("Missing welcome metadata".into());
    };
    Ok((ws, welcome))
}
// A cancelled exchange cannot leave a reusable socket with an outstanding reply.
// Starting Close synchronously transitions tungstenite out of its writable state;
// no async cleanup or runtime-spawned task is required during future cancellation.
struct ExchangeGuard<'a, S: AsyncRead + AsyncWrite + Unpin> {
    socket: &'a mut WebSocketStream<S>,
    complete: bool,
}
impl<S: AsyncRead + AsyncWrite + Unpin> Drop for ExchangeGuard<'_, S> {
    fn drop(&mut self) {
        if !self.complete {
            let _ = futures_util::Sink::start_send(
                std::pin::Pin::new(&mut *self.socket),
                Message::Close(None),
            );
        }
    }
}
fn expected_response(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "auth_hello" => "auth_challenge",
        "auth_proof" => "auth_welcome",
        "publish" => "result",
        "poll" => "messages",
        "get_message" => "message_status",
        "list_messages" => "messages_page",
        "get_permissions" => "permissions",
        "get_catalog" => "catalog",
        "rotation_start" => "rotation_offer",
        "rotation_confirm" => "rotation_ready",
        "rotation_commit" => "rotation_complete",
        _ => return None,
    })
}
fn validate_response(
    request: &protocol::pb::Frame,
    response: &protocol::pb::Frame,
) -> Result<(), BoxError> {
    let id = request
        .request_id()
        .filter(|id| !id.is_empty())
        .ok_or("Missing request ID")?;
    let expected = expected_response(request.kind()).ok_or("Not a client command")?;
    let early_rate_limit = matches!(&response.body, Some(protocol::pb::Body::Error(e))
        if e.code == "rate_limited" && e.request_id.is_none() && e.retry_after_ms.is_some());
    if response.version != 1
        || (response.kind() != expected && response.kind() != "error")
        || (!early_rate_limit && response.request_id() != Some(id))
    {
        return Err("Response type/request ID mismatch".into());
    }
    match (&request.body, &response.body) {
        (Some(protocol::pb::Body::Publish(_)), Some(protocol::pb::Body::Result(result)))
            if result.message_id.is_none() || result.expires.is_none() =>
        {
            return Err("Publish result is incomplete; outcome unknown".into());
        }
        (
            Some(protocol::pb::Body::GetMessage(request)),
            Some(protocol::pb::Body::MessageStatus(status)),
        ) if status.message.id != request.message_id => {
            return Err("Response message ID does not match requested message".into());
        }
        _ => {}
    }
    Ok(())
}
/// Typed protobuf exchange, including auth, ACL and rotation frames.
/// Replies must match the request ID and command type. Cancellation, timeout or
/// protocol/transport failure closes the socket for future commands. Reconnect
/// explicitly; an interrupted mutation may already have executed and is not retried.
pub async fn exchange_frame<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    request: &protocol::pb::Frame,
) -> std::result::Result<protocol::pb::Frame, BoxError> {
    use prost::Message as _;
    expected_response(request.kind()).ok_or("Not a client command")?;
    let mut guard = ExchangeGuard {
        socket: ws,
        complete: false,
    };
    let result = timeout(Duration::from_secs(10), async {
        guard
            .socket
            .send(Message::Binary(request.encode_to_vec().into()))
            .await?;
        while let Some(frame) = guard.socket.next().await {
            match frame? {
                Message::Binary(data) => {
                    let frame = protocol::pb::Frame::decode(data)?;
                    if frame.version != 1 || frame.body.is_none() {
                        return Err("Invalid protobuf response".into());
                    }
                    let missing_producer = match &frame.body {
                        Some(protocol::pb::Body::Messages(page)) => page
                            .items
                            .iter()
                            .any(|item| item.message.producer_id.is_empty()),
                        Some(protocol::pb::Body::MessageStatus(status)) => {
                            status.message.producer_id.is_empty()
                        }
                        _ => false,
                    };
                    if missing_producer {
                        return Err("Missing owner-authenticated producer identity".into());
                    }
                    validate_response(request, &frame)?;
                    return Ok(frame);
                }
                Message::Ping(_) => guard.socket.flush().await?,
                Message::Close(_) => break,
                _ => return Err("Expected a protobuf binary response".into()),
            }
        }
        Err::<protocol::pb::Frame, BoxError>("Connection closed before response".into())
    })
    .await?;
    guard.complete = result.is_ok();
    result
}
/// JSON-shaped convenience API for the CLI. Network encoding is protobuf.
pub async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    request: &Value,
) -> std::result::Result<Value, BoxError> {
    let request = protocol::pb::Frame::from_json(request.clone())?;
    exchange_frame(ws, &request)
        .await?
        .to_json()
        .map_err(Into::into)
}
/// Last rotation phase attempted. Only an unconfirmed commit can change credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotationPhase {
    Start,
    Confirm,
    Commit,
}

/// A validated broker rejection or a failure in a specific rotation phase.
#[derive(Debug)]
pub struct RotationError {
    pub phase: RotationPhase,
    pub broker: Option<protocol::pb::Error>,
    source: BoxError,
}
impl RotationError {
    pub fn outcome_unknown(&self) -> bool {
        self.phase == RotationPhase::Commit && self.broker.is_none()
    }
}
impl std::fmt::Display for RotationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rotation {:?}: {}", self.phase, self.source)
    }
}
impl std::error::Error for RotationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
async fn rotation_exchange<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    request: &Value,
    phase: RotationPhase,
) -> Result<Value, BoxError> {
    let frame = protocol::pb::Frame::from_json(request.clone())?;
    let response = exchange_frame(ws, &frame).await?;
    if let Some(protocol::pb::Body::Error(error)) = response.body.as_ref() {
        return Err(Box::new(RotationError {
            phase,
            source: format!("{}: {}", error.code, error.detail).into(),
            broker: Some(error.clone()),
        }));
    }
    response.to_json().map_err(Into::into)
}

/// Client-initiated key rotation. The caller keeps both old and new private-key files.
pub async fn rotate_client<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    old: &[u8],
    new: &[u8],
    owner: &str,
    client: &str,
) -> std::result::Result<Value, BoxError> {
    let mut phase = RotationPhase::Start;
    let result: Result<Value, BoxError> = async {
        let offer = rotation_exchange(
            ws,
            &json!({"version":1,"type":"rotation_start","id":"rotation-start"}), phase,
        )
        .await?;
        if offer["type"] != "rotation_offer" || offer["request_id"] != "rotation-start" {
            return Err("Owner did not offer rotation".into());
        }
        let rid = offer["rotation_id"].as_str().ok_or("Missing rotation ID")?;
        let secret = zeroize::Zeroizing::new(crypto::random::<32>()?);
        phase = RotationPhase::Confirm;
        let ready = rotation_exchange(
            ws,
            &json!({"version":1,"type":"rotation_confirm","id":"rotation-confirm","rotation_id":rid,"shared_secret":crypto::encode(secret.as_ref())}), phase,
        )
        .await?;
        if ready["type"] != "rotation_ready"
            || ready["request_id"] != "rotation-confirm"
            || ready["rotation_id"] != rid
            || ready["new_valid_until"] != offer["new_valid_until"]
            || ready["expires_at"] != offer["expires_at"]
        {
            return Err("Rotation confirmation mismatch".into());
        }
        let until = protocol::parse_time(
            ready["new_valid_until"]
                .as_str()
                .ok_or("Missing key expiry")?,
        )?;
        let old_public = crypto::public_key(old)?;
        let aad = crypto::rotation_aad(owner, client, rid, &crypto::token_id(&old_public), until);
        if ready["aad"] != crypto::encode(&aad) {
            return Err("Rotation binding mismatch".into());
        }
        let nonce = crypto::decode(ready["nonce"].as_str().ok_or("Missing nonce")?)?;
        let challenge = crypto::decode(
            ready["challenge"]
                .as_str()
                .ok_or("Missing rotation challenge")?,
        )?;
        let public = crypto::public_key(new)?;
        let encrypted = crypto::encrypt_rotation(&secret, nonce, &aad, &public)?;
        let signature = crypto::sign(
            new,
            &crypto::rotation_proof_bytes(&aad, &public, &challenge),
        )?;
        phase = RotationPhase::Commit;
        let complete=rotation_exchange(ws,&json!({"version":1,"type":"rotation_commit","id":"rotation-commit","rotation_id":rid,"encrypted_public_key":encrypted,"signature":signature}),phase).await?;
        if complete["type"] != "rotation_complete"
            || complete["request_id"] != "rotation-commit"
            || complete["rotation_id"] != rid
            || complete["client_id"] != client
            || complete["token_id"] != crypto::token_id(&public)
            || complete["valid_until"] != ready["new_valid_until"]
        {
            return Err(
                "Rotation commit not confirmed; retain both keys and check the new credential".into(),
            );
        }
        Ok(complete)
    }.await;
    result.map_err(|source| {
        if source.is::<RotationError>() {
            source
        } else {
            Box::new(RotationError {
                phase,
                broker: None,
                source,
            })
        }
    })
}

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::{connect_unix_client, connect_unix_client_info, connect_unix_via_info};

#[cfg(test)]
mod tests;

/// Retrieve catalog metadata from an already authenticated owner connection.
/// This does not negotiate or enable typed-event publishing.
pub async fn get_catalog<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    name: &str,
    catalog_version: u32,
) -> std::result::Result<Value, BoxError> {
    let mut response = exchange(ws, &json!({"version":1,"type":"get_catalog","id":"get-catalog","name":name,"catalog_version":catalog_version})).await?;
    if response["version"] != 1
        || response["type"] != "catalog"
        || response["request_id"] != "get-catalog"
        || response["catalog"]["name"] != name
        || response["catalog"]["version"] != catalog_version
    {
        return Err("Catalog retrieval failed or response does not match requested catalog".into());
    }
    Ok(response["catalog"].take())
}

pub mod adapter;

pub mod progress;

mod via;
pub use via::{OwnerSocket, ViaOptions, connect_owner_info, connect_via_info};
