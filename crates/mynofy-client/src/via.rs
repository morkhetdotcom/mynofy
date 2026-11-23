//! A proxy selects a route, while TLS and authentication terminate at the owner.
use super::*;
use protocol::tunnel::WebSocketIo;
pub struct ViaOptions<'a> {
    pub topic: &'a str,
    pub owner: &'a str,
    pub owner_certificate_identity: &'a str,
    pub owner_ca: &'a Path,
}
pub type OwnerSocket<S> = WebSocketStream<tokio_rustls::client::TlsStream<WebSocketIo<S>>>;
pub async fn connect_owner_info<S: AsyncRead + AsyncWrite + Unpin>(
    mut proxy: WebSocketStream<S>,
    options: ViaOptions<'_>,
    pkcs8: &[u8],
    expected_client: Option<&str>,
) -> Result<(OwnerSocket<S>, protocol::pb::AuthWelcome), BoxError> {
    if options.topic.is_empty() || options.topic.len() > 256 {
        return Err("Invalid remote topic".into());
    }
    if options.owner.is_empty() || options.owner.len() > 256 {
        return Err("Invalid owner ID".into());
    }
    let name = ServerName::try_from(options.owner_certificate_identity.to_owned())?;
    let owner_tls = client_tls(options.owner_ca)?;
    let start = json!({"version":1,"type":"client_tunnel","id":"owner","topic":options.topic,"owner_id":options.owner});
    timeout(
        Duration::from_secs(10),
        proxy.send(Message::Text(start.to_string().into())),
    )
    .await??;
    let response = timeout(Duration::from_secs(10), proxy.next())
        .await?
        .ok_or("Proxy closed before route selection")??;
    let Message::Text(text) = response else {
        return Err("Expected tunnel readiness".into());
    };
    if text.len() > 16384 {
        return Err("Tunnel control frame too large".into());
    }
    let value = protocol::strict_json(&text)?;
    if value["version"] != 1
        || value["type"] != "client_tunnel_ready"
        || value["request_id"] != "owner"
        || value["owner_id"] != options.owner
    {
        return Err("Owner tunnel unavailable or mismatched".into());
    }
    // The proxy's path is diagnostic; the owner is authenticated by its own TLS certificate.
    let tls = timeout(
        Duration::from_secs(10),
        TlsConnector::from(owner_tls).connect(name, WebSocketIo::new(proxy)),
    )
    .await??;
    let authority = if options
        .owner_certificate_identity
        .parse::<std::net::Ipv6Addr>()
        .is_ok()
    {
        format!("[{}]", options.owner_certificate_identity)
    } else {
        options.owner_certificate_identity.to_owned()
    };
    let url = format!("wss://{authority}/");
    let (ws, _) = timeout(
        Duration::from_secs(10),
        tokio_tungstenite::client_async_with_config(url, tls, Some(ws_config(4 * 1024 * 1024))),
    )
    .await??;
    authenticate(ws, pkcs8, options.owner, expected_client).await
}
pub async fn connect_via_info(
    proxy_url: &str,
    proxy_ca: &Path,
    options: ViaOptions<'_>,
    pkcs8: &[u8],
    expected_client: Option<&str>,
) -> Result<
    (
        OwnerSocket<tokio_rustls::client::TlsStream<TcpStream>>,
        protocol::pb::AuthWelcome,
    ),
    BoxError,
> {
    connect_owner_info(
        open_wss(proxy_url, proxy_ca).await?,
        options,
        pkcs8,
        expected_client,
    )
    .await
}
