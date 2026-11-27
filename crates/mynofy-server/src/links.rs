//! Bounded mTLS service links. Client traffic remains opaque end-to-end TLS bytes.
use super::transport::{certs, locked, now_ms, ws_config};
use crate::{BoxError, config::Config, engine::Engine, protocol};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU32, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream},
    net::TcpStream,
    sync::{mpsc, oneshot},
    task::{JoinHandle, JoinSet},
    time::timeout,
};
use tokio_rustls::{
    TlsAcceptor, TlsConnector,
    rustls::{
        self,
        pki_types::{CertificateDer, ServerName, pem::PemObject},
    },
};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};
const QUEUE: usize = 32;
const PEER_LINK_VERSION: u32 = 2;
const CHUNK: usize = protocol::tunnel::CHUNK;
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Frame {
    ServiceHello {
        #[serde(default)]
        link_version: u32,
        id: String,
        service_id: String,
    },
    ServiceWelcome {
        #[serde(default)]
        link_version: u32,
        request_id: String,
        service_id: String,
    },
    RouteQuery {
        id: String,
        topic: String,
        visited: Vec<String>,
        hop_limit: u8,
    },
    RouteInfo {
        request_id: String,
        topic: String,
        owner_id: String,
        path: Vec<String>,
        expires_at: String,
    },
    TunnelOpen {
        channel: u32,
        topic: String,
        owner_id: String,
        visited: Vec<String>,
        hop_limit: u8,
    },
    TunnelReady {
        channel: u32,
        path: Vec<String>,
        expires_at: String,
    },
    TunnelAuthenticated {
        channel: u32,
    },
    TunnelClose {
        channel: u32,
    },
    TunnelCredit {
        channel: u32,
    },
    ClientTunnel {
        id: String,
        topic: String,
        owner_id: String,
    },
    ClientTunnelReady {
        request_id: String,
        owner_id: String,
        path: Vec<String>,
        expires_at: String,
    },
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<u32>,
        code: String,
        detail: String,
    },
}
fn encode(frame: Frame) -> Message {
    let mut value = serde_json::to_value(frame).expect("serializable service frame");
    value["version"] = 1.into();
    Message::Text(value.to_string().into())
}
fn decode(message: &Message) -> Result<Frame, BoxError> {
    let Message::Text(text) = message else {
        return Err("Expected service control text frame".into());
    };
    if text.len() > 16384 {
        return Err("Service control frame too large".into());
    }
    let mut value = protocol::strict_json(text)?;
    if value.as_object_mut().and_then(|v| v.remove("version")) != Some(1.into()) {
        return Err("Unsupported service frame version".into());
    }
    Ok(serde_json::from_value(value)?)
}
#[derive(Clone, Serialize)]
struct Route {
    #[serde(rename = "owner_id")]
    owner: String,
    path: Vec<String>,
    #[serde(rename = "expires_at_ms")]
    until: u64,
}
struct Outgoing {
    handle: Handle,
    worker: JoinHandle<()>,
}
impl Drop for Outgoing {
    fn drop(&mut self) {
        self.worker.abort();
    }
}
#[derive(Clone, Default, Serialize)]
pub(crate) struct Status {
    registered_peers: BTreeSet<String>,
    registered_certificates: BTreeMap<String, String>,
    active_channels: usize,
    routes: BTreeMap<String, LearnedRoute>,
    upstream_last_error: Option<UpstreamFailure>,
    upstream_connecting: bool,
    upstream_connected: bool,
    upstream_service_id: Option<String>,
}
#[derive(Clone, Serialize)]
struct LearnedRoute {
    #[serde(flatten)]
    route: Route,
    next_hop: String,
    learned_from: String,
}
#[derive(Clone, Copy, Serialize)]
struct UpstreamFailure {
    phase: &'static str,
    at_ms: u64,
}

type LinkAttempt = tokio::sync::watch::Receiver<Option<Result<Handle, String>>>;

#[derive(Default)]
struct Connecting {
    stopped: bool,
    retry_after: Option<std::time::Instant>,
    attempt: Option<(LinkAttempt, tokio::task::JoinHandle<()>)>,
}

pub(crate) struct Network {
    config: Config,
    engine: Weak<Mutex<Engine>>,
    inner: Option<TlsAcceptor>,
    client: Option<Arc<rustls::ClientConfig>>,
    outgoing: tokio::sync::Mutex<Option<Outgoing>>,
    connecting: Mutex<Connecting>,
    status: Arc<Mutex<Status>>,
    channel_slots: Arc<tokio::sync::Semaphore>,
    pending_owner_tunnels: Arc<tokio::sync::Semaphore>,
    pending_sources: Mutex<HashMap<std::net::IpAddr, Arc<tokio::sync::Semaphore>>>,
}
impl Network {
    pub(crate) fn new(config: Config, engine: Weak<Mutex<Engine>>) -> Result<Arc<Self>, BoxError> {
        let inner = config
            .service
            .tls
            .as_ref()
            .map(|_| super::transport::server_tls(&config).map(TlsAcceptor::from))
            .transpose()?;
        let client = if let Some(up) = &config.upstream {
            let mut roots = rustls::RootCertStore::empty();
            for cert in certs(&up.trust.ca_file)? {
                roots.add(cert)?;
            }
            let tls = config
                .service
                .tls
                .as_ref()
                .ok_or("Peer needs local TLS credentials")?;
            Some(Arc::new(
                rustls::ClientConfig::builder_with_provider(Arc::new(
                    rustls::crypto::aws_lc_rs::default_provider(),
                ))
                .with_protocol_versions(&[&rustls::version::TLS13])?
                .with_root_certificates(roots)
                .with_client_auth_cert(
                    certs(&tls.certificate_file)?,
                    rustls::pki_types::PrivateKeyDer::from_pem_file(&tls.private_key_file)?,
                )?,
            ))
        } else {
            None
        };
        let owner = engine.upgrade().ok_or("Owner stopped")?;
        let status = locked(owner.as_ref())?.link_status.clone();
        locked(&status)?.upstream_service_id =
            config.upstream.as_ref().map(|up| up.service_id.clone());
        let capacity = config
            .limits
            .max_service_channels
            .min(config.limits.max_connections);
        Ok(Arc::new(Self {
            config,
            engine,
            inner,
            client,
            outgoing: tokio::sync::Mutex::new(None),
            connecting: Mutex::new(Connecting::default()),
            status,
            channel_slots: Arc::new(tokio::sync::Semaphore::new(capacity)),
            pending_sources: Mutex::new(HashMap::new()),
            pending_owner_tunnels: Arc::new(tokio::sync::Semaphore::new((capacity / 4).max(1))),
        }))
    }
    fn peer_config(&self, id: &str) -> Result<crate::config::Peer, BoxError> {
        let owner = self.engine.upgrade().ok_or("Owner stopped")?;
        let engine = locked(&owner)?;
        if engine.config.disabled_peers.iter().any(|peer| peer == id) {
            return Err("Peer disabled".into());
        }
        engine
            .config
            .accepted_peers
            .iter()
            .find(|p| p.service_id == id)
            .cloned()
            .ok_or_else(|| "Peer not allowed".into())
    }
    fn certificate_expires(certificate: &CertificateDer<'_>) -> Result<u64, BoxError> {
        use x509_cert::der::Decode;
        let certificate = x509_cert::Certificate::from_der(certificate.as_ref())?;
        Ok(u64::try_from(
            certificate
                .tbs_certificate()
                .validity()
                .not_after
                .to_unix_duration()
                .as_millis(),
        )?)
    }
    fn lease(
        &self,
        peer: &str,
        certificate: String,
        expires_at: u64,
        incoming: Option<&crate::config::Peer>,
    ) -> Result<Lease, BoxError> {
        let owner = self.engine.upgrade().ok_or("Owner stopped")?;
        let engine = locked(&owner)?;
        if incoming.is_some_and(|expected| !engine.config.accepted_peers.contains(expected)) {
            return Err("Peer binding changed".into());
        }
        let lease = Lease {
            peer: peer.into(),
            certificate,
            expires_at,
            revision: engine.peer_revisions.get(peer).copied().unwrap_or(0),
            incoming: incoming.is_some(),
        };
        if !lease.permitted(&engine) {
            return Err("Peer or certificate denied".into());
        }
        Ok(lease)
    }
    fn deadline(&self) -> Duration {
        Duration::from_secs(self.config.limits.request_timeout_seconds)
    }
    fn cap(&self) -> usize {
        self.config
            .limits
            .max_service_channels
            .min(self.config.limits.max_connections)
    }
    async fn upstream(self: &Arc<Self>) -> Result<Handle, BoxError> {
        let mut attempt = {
            let mut current = locked(&self.connecting)?;
            if current.stopped {
                return Err("Upstream connection stopped".into());
            }
            if let Ok(slot) = self.outgoing.try_lock()
                && let Some(link) = slot.as_ref().filter(|link| !link.handle.tx.is_closed())
            {
                return Ok(link.handle.clone());
            }
            if let Some((receiver, _)) = current
                .attempt
                .as_ref()
                .filter(|(_, worker)| !worker.is_finished())
            {
                receiver.clone()
            } else {
                if current
                    .retry_after
                    .is_some_and(|deadline| deadline > std::time::Instant::now())
                {
                    return Err("Upstream reconnect cooldown".into());
                }
                locked(&self.status)?.upstream_connecting = true;
                let (sender, receiver) = tokio::sync::watch::channel(None);
                let network = self.clone();
                let worker = tokio::spawn(async move {
                    let mut phase = "connect";
                    let result =
                        match timeout(network.deadline(), network.connect_upstream(&mut phase))
                            .await
                        {
                            Ok(result) => result.map_err(|error| error.to_string()),
                            Err(_) => Err("Upstream connection budget expired".into()),
                        };
                    if let Ok(mut connecting) = network.connecting.lock() {
                        if result.is_err() {
                            connecting.retry_after = Some(
                                std::time::Instant::now() + std::time::Duration::from_millis(250),
                            );
                        }
                        if let Ok(mut status) = network.status.lock() {
                            status.upstream_connecting = false;
                            if result.is_err() {
                                status.upstream_last_error = Some(UpstreamFailure {
                                    phase,
                                    at_ms: now_ms(),
                                });
                            }
                        }
                    }
                    sender.send_replace(Some(result));
                });
                current.attempt = Some((receiver.clone(), worker));
                receiver
            }
        };
        loop {
            if let Some(result) = attempt.borrow_and_update().clone() {
                return result.map_err(Into::into);
            }
            attempt
                .changed()
                .await
                .map_err(|_| "Upstream connection stopped")?;
        }
    }
    pub(crate) async fn shutdown(&self) {
        let attempt = {
            let mut current = self
                .connecting
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            current.stopped = true;
            current.attempt.take()
        };
        if let Some((_, worker)) = attempt {
            worker.abort();
            let _ = worker.await;
        }
        if let Some(mut outgoing) = self.outgoing.lock().await.take() {
            outgoing.worker.abort();
            let _ = (&mut outgoing.worker).await;
        }
    }
    async fn connect_upstream(
        self: &Arc<Self>,
        phase: &mut &'static str,
    ) -> Result<Handle, BoxError> {
        let mut slot = self.outgoing.lock().await;
        if let Some(link) = slot.as_ref().filter(|v| !v.handle.tx.is_closed()) {
            return Ok(link.handle.clone());
        }
        *slot = None;
        let up = self.config.upstream.as_ref().ok_or("No upstream")?;
        let (uri, port) = up.endpoint()?;
        let host = uri
            .host()
            .ok_or("Missing upstream host")?
            .trim_start_matches('[')
            .trim_end_matches(']');
        let stream = timeout(self.deadline(), TcpStream::connect((host, port))).await??;
        stream.set_nodelay(true)?;
        *phase = "tls";
        let tls = timeout(
            self.deadline(),
            TlsConnector::from(self.client.clone().ok_or("No peer TLS credentials")?).connect(
                ServerName::try_from(up.trust.expected_certificate_identity.clone())?,
                stream,
            ),
        )
        .await??;
        let certificate = tls
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|chain| chain.first())
            .ok_or("Missing upstream certificate")?;
        let lease = self.lease(
            &up.service_id,
            crate::admin::certificate_fingerprint(certificate.as_ref()),
            Self::certificate_expires(certificate)?,
            None,
        )?;
        *phase = "registration";
        let (mut ws, _) = timeout(
            self.deadline(),
            tokio_tungstenite::client_async_with_config(uri, tls, Some(ws_config(CHUNK + 16384))),
        )
        .await??;
        timeout(
            self.deadline(),
            ws.send(encode(Frame::ServiceHello {
                link_version: PEER_LINK_VERSION,
                id: "link".into(),
                service_id: self.config.service.id.clone(),
            })),
        )
        .await??;
        let reply = timeout(self.deadline(), ws.next())
            .await?
            .ok_or("Peer closed before welcome")??;
        match decode(&reply)? {
            Frame::ServiceWelcome {
                request_id,
                service_id,
                link_version,
            } if link_version != PEER_LINK_VERSION
                && request_id == "link"
                && service_id == up.service_id =>
            {
                return Err(format!(
                    "Peer link version mismatch: expected {PEER_LINK_VERSION}, got {link_version}"
                )
                .into());
            }
            Frame::Error { code, detail, .. } if code == "unsupported_version" => {
                return Err(detail.into());
            }
            Frame::ServiceWelcome {
                request_id,
                service_id,
                ..
            } if request_id == "link" && service_id == up.service_id => {}
            _ => return Err("Peer identity or welcome mismatch".into()),
        }
        let (handle, rx) = Handle::new(self.deadline());
        let network = Arc::downgrade(self);
        let peer = up.service_id.clone();
        let worker_handle = handle.clone();
        if !locked(&self.status)?.registered_peers.insert(peer.clone()) {
            return Err("Duplicate service registration".into());
        }
        locked(&self.status)?
            .registered_certificates
            .insert(peer.clone(), lease.certificate.clone());
        locked(&self.status)?.upstream_connected = true;
        let registration = Registration {
            outgoing: true,
            network: network.clone(),
            peer: peer.clone(),
        };
        let worker = tokio::spawn(async move {
            let _registration = registration;
            let _ = manager(ws, network, peer, true, worker_handle, rx, lease).await;
        });
        *slot = Some(Outgoing {
            handle: handle.clone(),
            worker,
        });
        Ok(handle)
    }
    fn target(&self, topic: &str) -> Result<String, BoxError> {
        let owner = self.engine.upgrade().ok_or("Owner stopped")?;
        locked(&owner)?
            .topic_owner(topic)
            .map(str::to_owned)
            .ok_or_else(|| "Unknown topic route".into())
    }
    fn check_hop(
        &self,
        peer: &str,
        topic: &str,
        visited: &[String],
        hops: u8,
    ) -> Result<(), BoxError> {
        if hops == 0
            || hops > self.config.routing.hop_limit
            || visited.is_empty()
            || visited.len() > 64
            || visited.last().is_none_or(|v| v != peer)
            || visited
                .iter()
                .any(|v| v == &self.config.service.id || v.is_empty() || v.len() > 256)
            || visited.iter().collect::<HashSet<_>>().len() != visited.len()
            || !self.peer_config(peer)?.topics.iter().any(|t| t == topic)
        {
            return Err("Peer topic or route denied".into());
        }
        self.target(topic)?;
        Ok(())
    }
    fn validate_route(
        &self,
        topic: &str,
        route: &Route,
        visited: &[String],
    ) -> Result<(), BoxError> {
        let up = self.config.upstream.as_ref().ok_or("No upstream")?;
        if route.owner != self.target(topic)?
            || route.path.first() != Some(&up.service_id)
            || route.path.last() != Some(&route.owner)
            || route.path.is_empty()
            || route.path.len() > 64
            || route.path.iter().collect::<HashSet<_>>().len() != route.path.len()
            || route
                .path
                .iter()
                .any(|s| visited.contains(s) || s.is_empty() || s.len() > 256)
            || route.until <= now_ms()
        {
            return Err("Conflicting or cyclic owner route".into());
        }
        Ok(())
    }
    fn record_route_failure(&self) -> Result<(), BoxError> {
        locked(&self.status)?.upstream_last_error = Some(UpstreamFailure {
            phase: "route",
            at_ms: now_ms(),
        });
        Ok(())
    }

    async fn route(
        self: &Arc<Self>,
        topic: String,
        mut visited: Vec<String>,
        hops: u8,
    ) -> Result<Route, BoxError> {
        let owner = self.target(&topic)?;
        if owner == self.config.service.id {
            return Ok(Route {
                owner,
                path: vec![self.config.service.id.clone()],
                until: now_ms()
                    .saturating_add(self.config.routing.learned_route_ttl_seconds * 1000),
            });
        }
        if hops <= 1 || visited.contains(&self.config.service.id) {
            return Err("Route hop limit or loop".into());
        }
        visited.push(self.config.service.id.clone());
        let link = self.upstream().await?;
        let result = async {
            let route = link.query(topic.clone(), visited.clone(), hops - 1).await?;
            if route.path.len() >= usize::from(hops) {
                return Err("Owner route exceeds hop limit".into());
            }
            self.validate_route(&topic, &route, &visited)?;
            Ok::<_, BoxError>(route)
        }
        .await;
        if result.is_err() {
            self.record_route_failure()?;
        }
        let mut route = result?;
        route.until = route
            .until
            .min(now_ms().saturating_add(self.config.routing.learned_route_ttl_seconds * 1000));
        let mut status = locked(&self.status)?;
        status.routes.retain(|_, r| r.route.until > now_ms());
        let next = self
            .config
            .upstream
            .as_ref()
            .ok_or("No upstream")?
            .service_id
            .clone();
        status.routes.insert(
            topic,
            LearnedRoute {
                route: route.clone(),
                next_hop: next.clone(),
                learned_from: next,
            },
        );
        route.path.insert(0, self.config.service.id.clone());
        Ok(route)
    }
    async fn open(
        self: &Arc<Self>,
        topic: String,
        owner: String,
        mut visited: Vec<String>,
        hops: u8,
    ) -> Result<(DuplexStream, Route, oneshot::Receiver<()>), BoxError> {
        if self.target(&topic)? != owner {
            return Err("Owner conflict".into());
        }
        if owner == self.config.service.id {
            return Err("Local owner channel must terminate here".into());
        }
        if hops <= 1 || visited.contains(&self.config.service.id) {
            return Err("Route hop limit or loop".into());
        }
        visited.push(self.config.service.id.clone());
        let link = self.upstream().await?;
        let result = async {
            let (stream, route, authenticated) = link
                .open(topic.clone(), owner, visited.clone(), hops - 1)
                .await?;
            if route.path.len() >= usize::from(hops) {
                return Err("Owner route exceeds hop limit".into());
            }
            self.validate_route(&topic, &route, &visited)?;
            Ok::<_, BoxError>((stream, route, authenticated))
        }
        .await;
        if result.is_err() {
            self.record_route_failure()?;
        }
        let (stream, mut route, authenticated) = result?;
        route.until = route
            .until
            .min(now_ms().saturating_add(self.config.routing.learned_route_ttl_seconds * 1000));
        route.path.insert(0, self.config.service.id.clone());
        Ok((stream, route, authenticated))
    }
    pub(crate) fn special(first: &Message) -> bool {
        if let Message::Text(text) = first
            && text.len() <= 16 * 1024
            && let Ok(value) = protocol::strict_json(text)
        {
            return matches!(
                value["type"].as_str(),
                Some("service_hello" | "client_tunnel")
            );
        }
        false
    }
    pub(crate) async fn dispatch<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
        self: &Arc<Self>,
        mut ws: WebSocketStream<S>,
        first: Message,
        certificate: Option<CertificateDer<'static>>,
        source: Option<std::net::IpAddr>,
    ) -> Result<(), BoxError> {
        match decode(&first)? {
            Frame::ServiceHello {
                id,
                service_id,
                link_version,
            } => {
                if link_version != PEER_LINK_VERSION {
                    let detail = format!(
                        "Peer link version mismatch: expected {PEER_LINK_VERSION}, got {link_version}"
                    );
                    timeout(
                        self.deadline(),
                        ws.send(encode(Frame::Error {
                            request_id: Some(id),
                            channel: None,
                            code: "unsupported_version".into(),
                            detail: detail.clone(),
                        })),
                    )
                    .await??;
                    return Err(detail.into());
                }
                if id.is_empty() || id.len() > 128 {
                    return Err("Invalid hello ID".into());
                }
                let peer = self.peer_config(&service_id)?;
                let certificate = certificate.ok_or("Peer mTLS required")?;
                let parsed = rustls::server::ParsedCertificate::try_from(&certificate)?;
                rustls::client::verify_server_name(&parsed, &peer.server_name()?)?;
                let lease = self.lease(
                    &service_id,
                    crate::admin::certificate_fingerprint(certificate.as_ref()),
                    Self::certificate_expires(&certificate)?,
                    Some(&peer),
                )?;
                if !locked(&self.status)?
                    .registered_peers
                    .insert(service_id.clone())
                {
                    return Err("Duplicate service registration".into());
                }
                locked(&self.status)?
                    .registered_certificates
                    .insert(service_id.clone(), lease.certificate.clone());
                let _registration = Registration {
                    outgoing: false,
                    network: Arc::downgrade(self),
                    peer: service_id.clone(),
                };
                timeout(
                    self.deadline(),
                    ws.send(encode(Frame::ServiceWelcome {
                        link_version: PEER_LINK_VERSION,
                        request_id: id,
                        service_id: self.config.service.id.clone(),
                    })),
                )
                .await??;
                let (handle, rx) = Handle::new(self.deadline());
                manager(
                    ws,
                    Arc::downgrade(self),
                    service_id,
                    false,
                    handle,
                    rx,
                    lease,
                )
                .await
            }
            Frame::ClientTunnel {
                id,
                topic,
                owner_id,
            } => {
                if id.is_empty() || id.len() > 128 {
                    return Err("Invalid tunnel ID".into());
                }
                if self.target(&topic)? != owner_id || owner_id == self.config.service.id {
                    return Err("Client tunnel needs a configured remote owner".into());
                }
                let pending = if let Some(source) = source {
                    let mut sources = locked(&self.pending_sources)?;
                    sources.retain(|_, quota| Arc::strong_count(quota) > 1);
                    let limit = (self.cap() / 16).max(1);
                    let quota = sources
                        .entry(source)
                        .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(limit)));
                    Some(
                        quota
                            .clone()
                            .try_acquire_owned()
                            .map_err(|_| "Pending source tunnel capacity exhausted")?,
                    )
                } else {
                    None
                };
                let until = tokio::time::Instant::now() + self.deadline();
                let setup = async {
                    let cached = locked(&self.status)?.routes.get(&topic).cloned();
                    let connected = self.outgoing.try_lock().is_ok_and(|slot| {
                        slot.as_ref()
                            .is_some_and(|link| !link.handle.tx.is_closed())
                    });
                    let valid = connected
                        && cached.is_some_and(|entry| {
                            entry.route.path.len() < usize::from(self.config.routing.hop_limit)
                                && self
                                    .validate_route(
                                        &topic,
                                        &entry.route,
                                        std::slice::from_ref(&self.config.service.id),
                                    )
                                    .is_ok()
                        });
                    if !valid {
                        self.route(topic.clone(), Vec::new(), self.config.routing.hop_limit)
                            .await?;
                    }
                    let open = || {
                        self.open(
                            topic.clone(),
                            owner_id.clone(),
                            Vec::new(),
                            self.config.routing.hop_limit,
                        )
                    };
                    let mut opened = open().await;
                    if opened.is_err() {
                        locked(&self.status)?.routes.remove(&topic);
                        if valid {
                            // No application stream has been exposed yet.
                            self.route(topic.clone(), Vec::new(), self.config.routing.hop_limit)
                                .await?;
                            opened = open().await;
                            if opened.is_err() {
                                locked(&self.status)?.routes.remove(&topic);
                            }
                        }
                    }
                    opened
                };
                tokio::pin!(setup);
                let setup_result = loop {
                    tokio::select! {
                        result = &mut setup => break result,
                        _ = tokio::time::sleep_until(until) => return Err("Tunnel setup budget expired".into()),
                        message = ws.next() => match message {
                            Some(Ok(Message::Ping(_))) => tokio::time::timeout_at(until, ws.flush()).await??,
                            Some(Ok(Message::Pong(_))) => {},
                            Some(Ok(Message::Close(_))) | None => return Ok(()),
                            _ => return Err("Unexpected message during tunnel setup".into()),
                        }
                    }
                };
                let (mut stream, route, authenticated) = setup_result?;
                tokio::time::timeout_at(
                    until,
                    ws.send(encode(Frame::ClientTunnelReady {
                        request_id: id,
                        owner_id: owner_id.clone(),
                        path: route.path,
                        expires_at: protocol::timestamp(route.until)?,
                    })),
                )
                .await??;
                let mut outer = protocol::tunnel::WebSocketIo::new(ws);
                // Per-transfer timeout, no command retransmission or connection resurrection.
                let relay = relay(
                    &mut outer,
                    &mut stream,
                    self.deadline(),
                    Duration::from_secs(self.config.limits.idle_timeout_seconds),
                );
                tokio::pin!(relay);
                tokio::select! {
                    result = &mut relay => result,
                    result = authenticated => {
                        result.map_err(|_| "Owner closed before tunnel authentication")?;
                        drop(pending);
                        relay.await
                    }
                }
            }
            _ => Err("Handshake required".into()),
        }
    }
}
struct Registration {
    outgoing: bool,
    network: Weak<Network>,
    peer: String,
}
impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(n) = self.network.upgrade()
            && let Ok(mut status) = n.status.lock()
        {
            if self.outgoing {
                status.upstream_connected = false;
            }
            status.registered_peers.remove(&self.peer);
            status.registered_certificates.remove(&self.peer);
            status.routes.retain(|_, r| r.next_hop != self.peer);
        }
    }
}
#[derive(Clone)]
struct Handle {
    tx: mpsc::Sender<Event>,
    next: Arc<AtomicU32>,
    deadline: Duration,
}
type Reply = oneshot::Sender<Result<Route, String>>;
enum Event {
    Open {
        topic: String,
        owner: String,
        visited: Vec<String>,
        hops: u8,
        pipe: DuplexStream,
        reply: Reply,
        authenticated: oneshot::Sender<()>,
    },
    Query {
        id: String,
        topic: String,
        visited: Vec<String>,
        hops: u8,
        reply: Reply,
    },
    Send(Message),
    Closed(u32),
}
impl Handle {
    fn new(deadline: Duration) -> (Self, mpsc::Receiver<Event>) {
        let (tx, rx) = mpsc::channel(QUEUE);
        (
            Self {
                tx,
                next: Arc::new(AtomicU32::new(1)),
                deadline,
            },
            rx,
        )
    }
    fn id(&self) -> Result<u32, BoxError> {
        let mut current = self.next.load(Ordering::Relaxed);
        loop {
            let next = current.checked_add(1).ok_or("Channel ID exhausted")?;
            match self.next.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(current),
                Err(value) => current = value,
            }
        }
    }
    async fn query(
        &self,
        topic: String,
        visited: Vec<String>,
        hops: u8,
    ) -> Result<Route, BoxError> {
        let id = format!("q{}", self.id()?);
        let (tx, rx) = oneshot::channel();
        self.tx
            .try_send(Event::Query {
                id,
                topic,
                visited,
                hops,
                reply: tx,
            })
            .map_err(|_| "Peer request capacity exhausted")?;
        timeout(self.deadline, rx).await??.map_err(Into::into)
    }
    async fn open(
        &self,
        topic: String,
        owner: String,
        visited: Vec<String>,
        hops: u8,
    ) -> Result<(DuplexStream, Route, oneshot::Receiver<()>), BoxError> {
        let (user, pipe) = tokio::io::duplex(CHUNK);
        let (tx, rx) = oneshot::channel();
        let (authenticated, auth_rx) = oneshot::channel();
        self.tx
            .try_send(Event::Open {
                topic,
                owner,
                visited,
                hops,
                pipe,
                reply: tx,
                authenticated,
            })
            .map_err(|_| "Peer channel capacity exhausted")?;
        let route = timeout(self.deadline, rx).await??.map_err(BoxError::from)?;
        Ok((user, route, auth_rx))
    }
}
struct Flow {
    _permit: Arc<ChannelPermit>,
    credits: Arc<tokio::sync::Semaphore>,
    outstanding: AtomicUsize,
}
struct ChannelPermit {
    _permit: tokio::sync::OwnedSemaphorePermit,
    status: Arc<Mutex<Status>>,
}
impl Drop for ChannelPermit {
    fn drop(&mut self) {
        if let Ok(mut status) = self.status.lock() {
            status.active_channels = status.active_channels.saturating_sub(1);
        }
    }
}
struct Channel {
    flow: Arc<Flow>,
    input: mpsc::Sender<Bytes>,
    reply: Option<Reply>,
    authenticated: Option<oneshot::Sender<()>>,
}
async fn pump(
    mut pipe: DuplexStream,
    mut input: mpsc::Receiver<Bytes>,
    tx: mpsc::Sender<Event>,
    channel: u32,
    deadline: Duration,
    idle: Duration,
    flow: Arc<Flow>,
) {
    let mut buffer = [0; CHUNK];
    loop {
        let event=timeout(idle,async {
            tokio::select! {
                read=async {let permit=flow.credits.clone().acquire_owned().await.map_err(std::io::Error::other)?;let n=pipe.read(&mut buffer).await?;Ok::<_,std::io::Error>((n,permit))}=>{
                    let(n,permit)=read?;if n==0{return Ok(false)};
                    flow.outstanding.fetch_add(1,Ordering::Relaxed);permit.forget();
                    let mut bytes=Vec::with_capacity(n+4);bytes.extend_from_slice(&channel.to_le_bytes());bytes.extend_from_slice(&buffer[..n]);
                    timeout(deadline,tx.send(Event::Send(Message::Binary(bytes.into())))).await.map_err(std::io::Error::other)?.map_err(std::io::Error::other)?;Ok(true)
                },
                chunk=input.recv()=>{
                    let Some(bytes)=chunk else{return Ok(false)};
                    timeout(deadline,pipe.write_all(&bytes)).await.map_err(std::io::Error::other)??;
                    timeout(deadline,tx.send(Event::Send(encode(Frame::TunnelCredit {channel})))).await.map_err(std::io::Error::other)?.map_err(std::io::Error::other)?;
                    Ok::<_,std::io::Error>(true)
                }
            }
        }).await;
        if !matches!(event, Ok(Ok(true))) {
            break;
        }
    }
    let _ = timeout(deadline, tx.send(Event::Closed(channel))).await;
}
fn channel(
    pipe: DuplexStream,
    reply: Option<Reply>,
    handle: &Handle,
    id: u32,
    tasks: &mut JoinSet<()>,
    idle: Duration,
    permit: Arc<ChannelPermit>,
) -> Channel {
    let (tx, rx) = mpsc::channel(4);
    let flow = Arc::new(Flow {
        _permit: permit,
        credits: Arc::new(tokio::sync::Semaphore::new(4)),
        outstanding: AtomicUsize::new(0),
    });
    tasks.spawn(pump(
        pipe,
        rx,
        handle.tx.clone(),
        id,
        handle.deadline,
        idle,
        flow.clone(),
    ));
    Channel {
        flow,
        input: tx,
        reply,
        authenticated: None,
    }
}
async fn relay<A: AsyncRead + AsyncWrite + Unpin, B: AsyncRead + AsyncWrite + Unpin>(
    a: &mut A,
    b: &mut B,
    deadline: Duration,
    idle: Duration,
) -> Result<(), BoxError> {
    let (mut ar, mut aw) = tokio::io::split(a);
    let (mut br, mut bw) = tokio::io::split(b);
    async fn copy<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
        r: &mut R,
        w: &mut W,
        d: Duration,
        i: Duration,
    ) -> Result<(), BoxError> {
        let mut bytes = [0; CHUNK];
        loop {
            let n = timeout(i, r.read(&mut bytes)).await??;
            if n == 0 {
                return Ok(());
            }
            timeout(d, w.write_all(&bytes[..n])).await??;
            timeout(d, w.flush()).await??;
        }
    }
    tokio::select! {r=copy(&mut ar,&mut bw,deadline,idle)=>r,r=copy(&mut br,&mut aw,deadline,idle)=>r}
}
fn unavailable(request_id: Option<String>, channel: Option<u32>) -> Message {
    encode(Frame::Error {
        request_id,
        channel,
        code: "owner_unavailable".into(),
        detail: "Service route or channel unavailable.".into(),
    })
}
struct Lease {
    peer: String,
    expires_at: u64,
    certificate: String,
    revision: u64,
    incoming: bool,
}
impl Lease {
    fn permitted(&self, engine: &Engine) -> bool {
        now_ms() < self.expires_at
            && !engine.config.disabled_peers.contains(&self.peer)
            && !engine
                .config
                .revoked_peer_certificates
                .contains(&self.certificate)
            && engine.peer_revisions.get(&self.peer).copied().unwrap_or(0) == self.revision
            && if self.incoming {
                engine
                    .config
                    .accepted_peers
                    .iter()
                    .any(|p| p.service_id == self.peer)
            } else {
                engine
                    .config
                    .upstream
                    .as_ref()
                    .is_some_and(|u| u.service_id == self.peer)
            }
    }
}
fn manager<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    ws: WebSocketStream<S>,
    network: Weak<Network>,
    peer: String,
    initiator: bool,
    handle: Handle,
    events: mpsc::Receiver<Event>,
    lease: Lease,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), BoxError>> + Send>> {
    Box::pin(async move {
        let engine = network
            .upgrade()
            .and_then(|net| net.engine.upgrade())
            .ok_or("Owner stopped")?;
        let mut changes = locked(&engine)?.link_changes.subscribe();
        let revoked = async {
            loop {
                if !lease.permitted(&*locked(&engine)?) {
                    return Err::<(), BoxError>("Peer policy changed".into());
                }
                changes.changed().await.map_err(|_| "Peer policy stopped")?;
            }
        };
        // Cancels even an in-flight write or route query; dropping JoinSet aborts all channel workers.
        let expired = async {
            loop {
                let remaining = lease.expires_at.saturating_sub(now_ms());
                if remaining == 0 {
                    return Err::<(), BoxError>("Peer certificate expired".into());
                }
                tokio::time::sleep(Duration::from_millis(remaining.min(1000))).await;
            }
        };
        tokio::select! {biased;
            result=expired=>result,
            result=revoked=>result,
            result=manager_work(ws,network,peer,initiator,handle,events)=>result,
        }
    })
}
// Keep link-local correlation for recently cancelled operations. Each table
// has a fixed capacity and discards its oldest entry under pressure.
struct LateReplies<K, V>(std::collections::VecDeque<(K, V, tokio::time::Instant)>);

impl<K: PartialEq, V> LateReplies<K, V> {
    fn new() -> Self {
        Self(std::collections::VecDeque::new())
    }

    fn remember(&mut self, key: K, value: V, deadline: Duration, capacity: usize) {
        self.expire();
        if self.0.len() >= capacity {
            self.0.pop_front();
        }
        self.0
            .push_back((key, value, tokio::time::Instant::now() + deadline));
    }

    fn expire(&mut self) {
        let now = tokio::time::Instant::now();
        while self.0.front().is_some_and(|entry| entry.2 <= now) {
            self.0.pop_front();
        }
    }

    fn take(&mut self, key: &K) -> Option<V> {
        self.expire();
        let index = self.0.iter().position(|entry| &entry.0 == key)?;
        self.0.remove(index).map(|(_, value, _)| value)
    }
}

fn manager_work<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    mut ws: WebSocketStream<S>,
    network: Weak<Network>,
    peer: String,
    initiator: bool,
    handle: Handle,
    mut events: mpsc::Receiver<Event>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), BoxError>> + Send>> {
    Box::pin(async move {
        let net = network.upgrade().ok_or("Service stopped")?;
        let cap = net.cap();
        let idle = Duration::from_secs(net.config.limits.idle_timeout_seconds);
        let deadline = net.deadline();
        drop(net);
        let mut channels: HashMap<u32, Channel> = HashMap::new();
        let mut last_channel = 0u32;
        let mut outgoing_channel = 0u32;
        let mut queries: HashMap<String, (String, Reply)> = HashMap::new();
        let mut late_queries = LateReplies::new();
        let mut late_channels = LateReplies::new();
        let mut tasks = JoinSet::new();
        let peer_pending = Arc::new(tokio::sync::Semaphore::new((cap / 8).max(1)));
        let work = Arc::new(tokio::sync::Semaphore::new(QUEUE));
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut active = tokio::time::Instant::now();
        loop {
            tokio::select! {
                _=tick.tick()=>{
                    if active.elapsed()>=idle {break;}
                    if let Some(net)=network.upgrade(){locked(&net.status)?.routes.retain(|_,r|r.route.until>now_ms());}
                    late_queries.expire();
                    late_channels.expire();
                    queries.retain(|id,(topic,reply)| {
                        if !reply.is_closed() { return true; }
                        late_queries.remember(id.clone(),topic.clone(),deadline,QUEUE);
                        false
                    });
                    let closed:Vec<_>=channels.iter().filter(|(_,c)|c.reply.as_ref().is_some_and(|r|r.is_closed())).map(|(id,_)|*id).collect();
                    for channel in closed {channels.remove(&channel);late_channels.remember(channel,(),deadline,cap);timeout(deadline,ws.send(encode(Frame::TunnelClose {channel}))).await??;}
                },
                _=tasks.join_next(),if !tasks.is_empty()=>{},
                event=events.recv()=>{
                    let Some(event)=event else{break};
                    match event {
                        Event::Send(message)=>{timeout(deadline,ws.send(message)).await??;},
                        Event::Closed(channel)=>{if let Some(closed)=channels.remove(&channel){if closed.reply.is_some(){late_channels.remember(channel,(),deadline,cap);}timeout(deadline,ws.send(encode(Frame::TunnelClose {channel}))).await??;}},
                        Event::Query {id,topic,visited,hops,reply}=>{
                            if !initiator || queries.len()>=QUEUE {let _=reply.send(Err("Peer query capacity exhausted".into()));continue;}
                            queries.insert(id.clone(),(topic.clone(),reply));timeout(deadline,ws.send(encode(Frame::RouteQuery {id,topic,visited,hop_limit:hops}))).await??;
                        },
                        Event::Open {topic,owner,visited,hops,pipe,reply,authenticated}=>{
                            if reply.is_closed() { continue; }
                            let Some(net)=network.upgrade() else{break};
                            let Ok(permit)=net.channel_slots.clone().try_acquire_owned() else{let _=reply.send(Err("Channel capacity exhausted".into()));continue};
                            if !initiator || channels.len()>=cap {let _=reply.send(Err("Channel capacity exhausted".into()));continue;}
                            let Some(id) = outgoing_channel.checked_add(1) else {
                                let _ = reply.send(Err("Channel ID exhausted".into()));
                                continue;
                            };
                            outgoing_channel = id;
                            locked(&net.status)?.active_channels+=1;
                        let permit=Arc::new(ChannelPermit {_permit:permit,status:net.status.clone()});
                        let mut opened=channel(pipe,Some(reply),&handle,id,&mut tasks,idle,permit);
                        opened.authenticated=Some(authenticated);
                        channels.insert(id,opened);
                            timeout(deadline,ws.send(encode(Frame::TunnelOpen {channel:id,topic,owner_id:owner,visited,hop_limit:hops}))).await??;
                        },
                    }
                },
                message=ws.next()=>{
                    let Some(message)=message else{break};let message=message?;active=tokio::time::Instant::now();
                    if let Message::Binary(bytes)=message {
                        if bytes.len()<=4 || bytes.len()>CHUNK+4 {return Err("Invalid channel frame size".into())}
                        let id=u32::from_le_bytes(bytes[..4].try_into()?);
                        let Some(channel)=channels.get(&id) else{continue};
                        if channel.reply.is_some(){return Err("Channel data before readiness".into())}
                        if channel.input.try_send(bytes.slice(4..)).is_err(){channels.remove(&id);timeout(deadline,ws.send(encode(Frame::TunnelClose {channel:id}))).await??;}
                        continue;
                    }
                    if matches!(message,Message::Ping(_)){timeout(deadline,ws.flush()).await??;continue;}
                    if matches!(message,Message::Pong(_)){continue;}
                    if matches!(message,Message::Close(_)){break;}
                    match decode(&message)? {
                        Frame::RouteQuery {id,topic,visited,hop_limit} if !initiator=>{
                            let net=network.upgrade().ok_or("Service stopped")?;
                            let permit=work.clone().try_acquire_owned();
                            if id.is_empty()||id.len()>128 || net.check_hop(&peer,&topic,&visited,hop_limit).is_err() || permit.is_err(){timeout(deadline,ws.send(unavailable(Some(id),None))).await??;continue;}
                            let permit=permit?;let tx=handle.tx.clone();
                            tasks.spawn(async move {let _permit=permit;let response=match timeout(deadline,net.route(topic.clone(),visited,hop_limit)).await {
                                Ok(Ok(route))=>match protocol::timestamp(route.until){Ok(expires_at)=>encode(Frame::RouteInfo {request_id:id,topic,owner_id:route.owner,path:route.path,expires_at}),Err(_)=>unavailable(Some(id),None)},_=>unavailable(Some(id),None)};
                                let _=timeout(deadline,tx.send(Event::Send(response))).await;
                            });
                        },
                        Frame::RouteInfo {request_id,topic,owner_id,path,expires_at} if initiator=>{
                            let Some((expected,reply))=queries.remove(&request_id) else {
                                if late_queries.take(&request_id).is_some_and(|expected|expected==topic){continue;}
                                return Err("Unexpected route response".into());
                            };
                            if expected!=topic{return Err("Route topic mismatch".into())}
                            let until=protocol::parse_time(&expires_at)?;let _=reply.send(Ok(Route {owner:owner_id,path,until}));
                        },
                        Frame::TunnelOpen {channel:id,topic,owner_id,visited,hop_limit} if !initiator=>{
                            let net=network.upgrade().ok_or("Service stopped")?;
                            if id<=last_channel||channels.contains_key(&id){return Err("Reused channel ID".into())}last_channel=id;
                            let permit=net.channel_slots.clone().try_acquire_owned();
                            if channels.len()>=cap || permit.is_err() || net.check_hop(&peer,&topic,&visited,hop_limit).is_err() || net.target(&topic)?!=owner_id {
                                timeout(deadline,ws.send(unavailable(None,Some(id)))).await??;continue;
                            }
                            let peer_admission = if owner_id == net.config.service.id {
                                let Ok(permit) = peer_pending.clone().try_acquire_owned() else {
                                    timeout(deadline,ws.send(unavailable(None,Some(id)))).await??;
                                    continue;
                                };
                                Some(permit)
                            } else {
                                None
                            };
                            let(user,pipe)=tokio::io::duplex(CHUNK);
                            let permit=permit?;locked(&net.status)?.active_channels+=1;
                        let permit=Arc::new(ChannelPermit {_permit:permit,status:net.status.clone()});
                        channels.insert(id,channel(pipe,None,&handle,id,&mut tasks,idle,permit.clone()));
                            let tx=handle.tx.clone();
                            tasks.spawn(async move {
                                let outcome=serve_channel(net,user,ChannelRequest {topic,owner:owner_id,visited,hops:hop_limit,_permit:permit,peer_admission},id,tx.clone()).await;
                                if outcome.is_err(){let _=timeout(deadline,tx.send(Event::Send(unavailable(None,Some(id))))).await;}
                                let _=timeout(deadline,tx.send(Event::Closed(id))).await;
                            });
                        },
                        Frame::TunnelReady {channel,path,expires_at} if initiator=>{
                            let Some(reply)=channels.get_mut(&channel).and_then(|c|c.reply.take()) else {
                                if late_channels.take(&channel).is_some() {
                                    timeout(deadline,ws.send(encode(Frame::TunnelClose {channel}))).await??;
                                    continue;
                                }
                                return Err("Unexpected channel readiness".into());
                            };
                            let until=protocol::parse_time(&expires_at)?;
                            let owner=path.last().cloned().ok_or("Empty owner route")?;let _=reply.send(Ok(Route {owner,path,until}));
                        },
                        Frame::TunnelAuthenticated {channel} if initiator=>{
                            if let Some(channel)=channels.get_mut(&channel) {
                                if channel.reply.is_some() { return Err("Authentication before tunnel readiness".into()); }
                                if let Some(authenticated)=channel.authenticated.take() {let _=authenticated.send(());}
                            }
                        },
                        Frame::TunnelCredit {channel}=>{
                        if let Some(c)=channels.get(&channel) {
                            let mut value=c.flow.outstanding.load(Ordering::Relaxed);
                            loop {if value==0{return Err("Unexpected channel credit".into())}
                                match c.flow.outstanding.compare_exchange_weak(value,value-1,Ordering::Relaxed,Ordering::Relaxed) {Ok(_)=>break,Err(v)=>value=v}
                            }
                            c.flow.credits.add_permits(1);
                        }
                    },
                    Frame::TunnelClose {channel}=>{channels.remove(&channel);},
                        Frame::Error {request_id,channel,..}=>{
                            if let Some(id)=request_id && let Some((_,reply))=queries.remove(&id){let _=reply.send(Err("Owner unavailable".into()));}
                            if let Some(id)=channel && let Some(mut channel)=channels.remove(&id) && let Some(reply)=channel.reply.take(){let _=reply.send(Err("Owner unavailable".into()));}
                        },_=>return Err("Unexpected service frame".into())
                    }
                },
            }
        }
        if initiator && let Some(net) = network.upgrade() {
            locked(&net.status)?.routes.clear();
        }
        Ok(())
    })
}
struct ChannelRequest {
    peer_admission: Option<tokio::sync::OwnedSemaphorePermit>,
    _permit: Arc<ChannelPermit>,
    topic: String,
    owner: String,
    visited: Vec<String>,
    hops: u8,
}
fn serve_channel(
    net: Arc<Network>,
    mut stream: DuplexStream,
    request: ChannelRequest,
    id: u32,
    tx: mpsc::Sender<Event>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), BoxError>> + Send>> {
    Box::pin(async move {
        let ChannelRequest {
            topic,
            owner,
            visited,
            hops,
            _permit,
            peer_admission,
        } = request;
        let deadline = net.deadline();
        if owner == net.config.service.id {
            let pending = net
                .pending_owner_tunnels
                .clone()
                .try_acquire_owned()
                .map_err(|_| "Pending owner tunnel capacity exhausted")?;
            let expires_at = protocol::timestamp(
                now_ms().saturating_add(net.config.routing.learned_route_ttl_seconds * 1000),
            )?;
            timeout(
                deadline,
                tx.send(Event::Send(encode(Frame::TunnelReady {
                    channel: id,
                    path: vec![owner],
                    expires_at,
                }))),
            )
            .await??;
            let tls = timeout(
                deadline,
                net.inner
                    .as_ref()
                    .ok_or("Owner TLS required")?
                    .accept(stream),
            )
            .await??;
            let engine = net.engine.upgrade().ok_or("Owner stopped")?;
            let (auth_tx, auth_rx) = oneshot::channel();
            let connection = super::transport::websocket_connection(
                tls,
                engine,
                &net.config,
                None,
                None,
                Some((&topic, pending, auth_tx)),
                None,
            );
            tokio::pin!(connection);
            return tokio::select! {
                result = &mut connection => result,
                auth = auth_rx => {
                    auth.map_err(|_| "Owner tunnel authentication ended")?;
                    drop(peer_admission);
                    timeout(deadline, tx.send(Event::Send(encode(Frame::TunnelAuthenticated {channel:id})))).await??;
                    connection.await
                }
            };
        }
        let (mut upstream, route, authenticated) =
            timeout(deadline, net.open(topic, owner, visited, hops)).await??;
        timeout(
            deadline,
            tx.send(Event::Send(encode(Frame::TunnelReady {
                channel: id,
                path: route.path,
                expires_at: protocol::timestamp(route.until)?,
            }))),
        )
        .await??;
        let forwarding = relay(
            &mut stream,
            &mut upstream,
            deadline,
            Duration::from_secs(net.config.limits.idle_timeout_seconds),
        );
        tokio::pin!(forwarding);
        tokio::select! {
            result = &mut forwarding => result,
            result = authenticated => {
                result.map_err(|_| "Upstream closed before tunnel authentication")?;
                timeout(deadline,tx.send(Event::Send(encode(Frame::TunnelAuthenticated {channel:id})))).await??;
                forwarding.await
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_control_rejects_ambiguous_unknown_oversized_and_wrong_version_frames() {
        for text in [
            r#"{"version":2,"type":"service_hello","id":"h","service_id":"edge"}"#,
            r#"{"version":1,"version":1,"type":"service_hello","id":"h","service_id":"edge"}"#,
            r#"{"version":1,"type":"service_hello","id":"h","service_id":"edge","client_id":"forged"}"#,
            r#"{"version":1,"type":"route_query","id":"q","topic":"x","visited":[],"hop_limit":256}"#,
        ] {
            assert!(decode(&Message::Text(text.into())).is_err());
        }
        assert!(decode(&Message::Binary(vec![0; 4].into())).is_err());
        assert!(decode(&Message::Text(" ".repeat(16385).into())).is_err());
        for examples in [
            include_str!("../../../protocol/v1/service-examples.json"),
            include_str!("../../../protocol/v1/route-examples.json"),
            include_str!("../../../protocol/v1/tunnel-examples.json"),
        ] {
            for example in serde_json::from_str::<Vec<serde_json::Value>>(examples).unwrap() {
                decode(&Message::Text(example["frame"].to_string().into())).unwrap();
            }
        }
    }
}

#[cfg(test)]
mod routing_tests {
    use super::*;
    #[tokio::test]
    async fn late_open_and_route_replies_preserve_an_established_channel() {
        use tokio_tungstenite::tungstenite::protocol::Role;
        let mut config: Config =
            serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
        let engine = Arc::new(Mutex::new(Engine::new(config.clone(), now_ms()).unwrap()));
        config.service.tls = None;
        let net = Network::new(config, Arc::downgrade(&engine)).unwrap();
        let (left, right) = tokio::io::duplex(65536);
        let ws = WebSocketStream::from_raw_socket(left, Role::Client, None).await;
        let mut peer = WebSocketStream::from_raw_socket(right, Role::Server, None).await;
        let (handle, events) = Handle::new(net.deadline());
        let manager = tokio::spawn(manager_work(
            ws,
            Arc::downgrade(&net),
            "owner".into(),
            true,
            handle.clone(),
            events,
        ));
        let stamp = || protocol::timestamp(now_ms() + 10000).unwrap();
        let ready = |channel| {
            encode(Frame::TunnelReady {
                channel,
                path: vec!["owner".into()],
                expires_at: stamp(),
            })
        };
        timeout(Duration::from_secs(4), async {
            let open = |handle: Handle| {
                tokio::spawn(async move {
                    handle
                        .open("local/events".into(), "owner".into(), Vec::new(), 2)
                        .await
                })
            };
            let established = open(handle.clone());
            let Frame::TunnelOpen { channel: live, .. } =
                decode(&peer.next().await.unwrap().unwrap()).unwrap()
            else {
                panic!("expected live open")
            };
            peer.send(ready(live)).await.unwrap();
            let (mut stream, _, _) = established.await.unwrap().unwrap();

            let abandoned = open(handle.clone());
            let Frame::TunnelOpen {
                channel: expired, ..
            } = decode(&peer.next().await.unwrap().unwrap()).unwrap()
            else {
                panic!("expected second open")
            };
            abandoned.abort();
            let _ = abandoned.await;

            let query =
                tokio::spawn(
                    async move { handle.query("local/events".into(), Vec::new(), 2).await },
                );
            let (id, topic) = loop {
                let message = peer.next().await.unwrap().unwrap();
                match decode(&message).unwrap() {
                    Frame::RouteQuery { id, topic, .. } => break (id, topic),
                    Frame::TunnelClose { channel } if channel == expired => continue,
                    _ => panic!("unexpected frame before route query"),
                }
            };
            query.abort();
            let _ = query.await;
            // Let the manager's cleanup tick remove the cancelled query.
            tokio::time::sleep(Duration::from_millis(1100)).await;
            peer.send(encode(Frame::RouteInfo {
                request_id: id,
                topic,
                owner_id: "owner".into(),
                path: vec!["owner".into()],
                expires_at: stamp(),
            }))
            .await
            .unwrap();
            peer.send(ready(expired)).await.unwrap();
            let mut data = live.to_le_bytes().to_vec();
            data.push(42);
            peer.send(Message::Binary(data.into())).await.unwrap();
            assert_eq!(stream.read_u8().await.unwrap(), 42);
            assert!(!manager.is_finished());
        })
        .await
        .unwrap();
        manager.abort();
        let _ = manager.await;
    }

    #[tokio::test]
    async fn network_shutdown_joins_pending_upstream_handshake() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config: Config =
            serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
        let engine = Arc::new(Mutex::new(Engine::new(config.clone(), now_ms()).unwrap()));
        config.service.tls = None;
        let mut net = Network::new(config, Arc::downgrade(&engine)).unwrap();
        let state = Arc::get_mut(&mut net).unwrap();
        state.config.upstream = Some(crate::config::Upstream {
            service_id: "upstream".into(),
            url: format!("wss://{}/", listener.local_addr().unwrap()),
            trust: crate::config::Trust {
                mode: "ca".into(),
                ca_file: "unused".into(),
                expected_certificate_identity: "localhost".into(),
            },
        });
        state.client = Some(Arc::new(
            rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth(),
        ));
        let waiting = tokio::spawn({
            let net = net.clone();
            async move { net.upstream().await }
        });
        timeout(Duration::from_secs(2), async {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut input = [0; 1];
            socket.read_exact(&mut input).await.unwrap();
            net.shutdown().await;
            assert!(waiting.await.unwrap().is_err());
            assert!(net.upstream().await.is_err());
            let mut buffered = Vec::new();
            socket.read_to_end(&mut buffered).await.unwrap();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn active_manager_stops_at_lease_expiry() {
        use tokio_tungstenite::tungstenite::protocol::Role;
        let mut config: Config =
            serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
        config.accepted_peers.push(crate::config::Peer {
            service_id: "child".into(),
            certificate_identity: None,
            topics: vec!["local/events".into()],
        });
        let engine = Arc::new(Mutex::new(Engine::new(config.clone(), now_ms()).unwrap()));
        config.service.tls = None;
        let net = Network::new(config, Arc::downgrade(&engine)).unwrap();
        let lease = net
            .lease(
                "child",
                "test fingerprint".into(),
                now_ms() + 300,
                Some(&net.peer_config("child").unwrap()),
            )
            .unwrap();
        let (left, right) = tokio::io::duplex(65536);
        let service = WebSocketStream::from_raw_socket(left, Role::Server, None).await;
        let mut peer = WebSocketStream::from_raw_socket(right, Role::Client, None).await;
        let (handle, events) = Handle::new(net.deadline());
        let task = tokio::spawn(manager(
            service,
            Arc::downgrade(&net),
            "child".into(),
            false,
            handle,
            events,
            lease,
        ));
        peer.send(Message::Ping(vec![7].into())).await.unwrap();
        assert!(matches!(peer.next().await, Some(Ok(Message::Pong(_)))));
        let result = timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err().to_string(), "Peer certificate expired");
    }

    #[test]
    fn hop_checks_and_static_owner_binding_reject_forged_routes() {
        let mut config: Config =
            serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
        config.service.id = "edge".into();
        config.topics[0].owner_id = "root".into();
        config.topics[0].via = Some("hub".into());
        config.topics[0].policy = None;
        config.topics[0].acl = None;
        config.upstream = Some(crate::config::Upstream {
            service_id: "hub".into(),
            url: "wss://localhost/".into(),
            trust: crate::config::Trust {
                mode: "ca".into(),
                ca_file: "ca.crt".into(),
                expected_certificate_identity: "hub".into(),
            },
        });
        config.accepted_peers = vec![crate::config::Peer {
            service_id: "child".into(),
            certificate_identity: None,
            topics: vec!["local/events".into()],
        }];
        let engine = Arc::new(Mutex::new(Engine::new(config.clone(), now_ms()).unwrap()));
        let net = Network {
            config,
            engine: Arc::downgrade(&engine),
            inner: None,
            client: None,
            outgoing: tokio::sync::Mutex::new(None),
            connecting: Mutex::new(Connecting::default()),
            status: Default::default(),
            channel_slots: Arc::new(tokio::sync::Semaphore::new(1)),
            pending_sources: Mutex::new(HashMap::new()),
            pending_owner_tunnels: Arc::new(tokio::sync::Semaphore::new(1)),
        };
        assert!(
            net.check_hop("child", "local/events", &["child".into()], 1)
                .is_ok()
        );
        for (peer, topic, visited, hops) in [
            ("unknown", "local/events", vec!["unknown".into()], 1),
            ("child", "forbidden", vec!["child".into()], 1),
            ("child", "local/events", vec!["child".into()], 0),
            ("child", "local/events", vec!["wrong".into()], 1),
            (
                "child",
                "local/events",
                vec!["edge".into(), "child".into()],
                1,
            ),
            (
                "child",
                "local/events",
                vec!["child".into(), "child".into()],
                1,
            ),
        ] {
            assert!(net.check_hop(peer, topic, &visited, hops).is_err());
        }
        let fingerprint = crate::admin::certificate_fingerprint(b"leaf");
        let peer = net.peer_config("child").unwrap();
        let lease = net
            .lease("child", fingerprint, now_ms() + 10_000, Some(&peer))
            .unwrap();
        for disabled in [true, false] {
            engine
                .lock()
                .unwrap()
                .admin(
                    crate::admin::Command::PeerDisable {
                        service_id: "child".into(),
                        disabled,
                    },
                    now_ms(),
                )
                .unwrap();
        }
        assert!(
            !lease.permitted(&engine.lock().unwrap()),
            "fast disable/enable must invalidate the previous registration"
        );
        let route = Route {
            owner: "root".into(),
            path: vec!["hub".into(), "root".into()],
            until: now_ms() + 1000,
        };
        assert!(
            net.validate_route("local/events", &route, &["edge".into()])
                .is_ok()
        );
        for path in [
            vec!["root".into()],
            vec!["hub".into(), "edge".into(), "root".into()],
            vec!["hub".into(), "hub".into(), "root".into()],
        ] {
            assert!(
                net.validate_route(
                    "local/events",
                    &Route {
                        path,
                        ..route.clone()
                    },
                    &["edge".into()]
                )
                .is_err()
            );
        }
        assert!(
            net.validate_route(
                "local/events",
                &Route {
                    owner: "other".into(),
                    ..route
                },
                &["edge".into()]
            )
            .is_err()
        );
    }
}
