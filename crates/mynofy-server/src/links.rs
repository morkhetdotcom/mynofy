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
const CHUNK: usize = protocol::tunnel::CHUNK;
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Frame {
    ServiceHello {
        id: String,
        service_id: String,
    },
    ServiceWelcome {
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
}
#[derive(Clone, Serialize)]
struct LearnedRoute {
    #[serde(flatten)]
    route: Route,
    next_hop: String,
    learned_from: String,
}
pub(crate) struct Network {
    config: Config,
    engine: Weak<Mutex<Engine>>,
    inner: Option<TlsAcceptor>,
    client: Option<Arc<rustls::ClientConfig>>,
    outgoing: tokio::sync::Mutex<Option<Outgoing>>,
    status: Arc<Mutex<Status>>,
    channel_slots: Arc<tokio::sync::Semaphore>,
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
            status,
            channel_slots: Arc::new(tokio::sync::Semaphore::new(capacity)),
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
    fn lease(
        &self,
        peer: &str,
        certificate: String,
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
        let mut slot = self.outgoing.lock().await;
        if let Some(link) = slot.as_ref().filter(|v| !v.handle.tx.is_closed()) {
            return Ok(link.handle.clone());
        }
        *slot = None;
        let up = self.config.upstream.as_ref().ok_or("No upstream")?;
        let uri: tokio_tungstenite::tungstenite::http::Uri = up.url.parse()?;
        let host = uri
            .host()
            .ok_or("Missing upstream host")?
            .trim_start_matches('[')
            .trim_end_matches(']');
        let stream = timeout(
            self.deadline(),
            TcpStream::connect((host, uri.port_u16().unwrap_or(443))),
        )
        .await??;
        stream.set_nodelay(true)?;
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
            None,
        )?;
        let (mut ws, _) = timeout(
            self.deadline(),
            tokio_tungstenite::client_async_with_config(
                &up.url,
                tls,
                Some(ws_config(CHUNK + 16384)),
            ),
        )
        .await??;
        timeout(
            self.deadline(),
            ws.send(encode(Frame::ServiceHello {
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
        let registration = Registration {
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
            .config
            .topics
            .iter()
            .find(|t| t.name == topic)
            .map(|t| t.owner_id.clone())
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
        let mut route = self
            .upstream()
            .await?
            .query(topic.clone(), visited.clone(), hops - 1)
            .await?;
        if route.path.len() >= usize::from(hops) {
            return Err("Owner route exceeds hop limit".into());
        }
        self.validate_route(&topic, &route, &visited)?;
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
    ) -> Result<(DuplexStream, Route), BoxError> {
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
        let (stream, mut route) = self
            .upstream()
            .await?
            .open(topic.clone(), owner, visited.clone(), hops - 1)
            .await?;
        if route.path.len() >= usize::from(hops) {
            return Err("Owner route exceeds hop limit".into());
        }
        self.validate_route(&topic, &route, &visited)?;
        route.until = route
            .until
            .min(now_ms().saturating_add(self.config.routing.learned_route_ttl_seconds * 1000));
        route.path.insert(0, self.config.service.id.clone());
        Ok((stream, route))
    }
    pub(crate) fn special(first: &Message) -> bool {
        if let Message::Text(text) = first
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
    ) -> Result<(), BoxError> {
        match decode(&first)? {
            Frame::ServiceHello { id, service_id } => {
                if id.is_empty() || id.len() > 128 {
                    return Err("Invalid hello ID".into());
                }
                let peer = self.peer_config(&service_id)?;
                let certificate = certificate.ok_or("Peer mTLS required")?;
                let parsed = rustls::server::ParsedCertificate::try_from(&certificate)?;
                rustls::client::verify_server_name(
                    &parsed,
                    &ServerName::try_from(
                        peer.certificate_identity
                            .clone()
                            .unwrap_or_else(|| peer.service_id.clone()),
                    )?,
                )?;
                let lease = self.lease(
                    &service_id,
                    crate::admin::certificate_fingerprint(certificate.as_ref()),
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
                    network: Arc::downgrade(self),
                    peer: service_id.clone(),
                };
                timeout(
                    self.deadline(),
                    ws.send(encode(Frame::ServiceWelcome {
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
                let route = self
                    .route(topic.clone(), Vec::new(), self.config.routing.hop_limit)
                    .await?;
                if route.owner != owner_id || owner_id == self.config.service.id {
                    return Err("Client tunnel needs a configured remote owner".into());
                }
                let (mut stream, route) = self
                    .open(
                        topic,
                        owner_id.clone(),
                        Vec::new(),
                        self.config.routing.hop_limit,
                    )
                    .await?;
                timeout(
                    self.deadline(),
                    ws.send(encode(Frame::ClientTunnelReady {
                        request_id: id,
                        owner_id,
                        path: route.path,
                        expires_at: protocol::timestamp(route.until)?,
                    })),
                )
                .await??;
                let mut outer = protocol::tunnel::WebSocketIo::new(ws);
                // Per-transfer timeout, no command retransmission or connection resurrection.
                relay(
                    &mut outer,
                    &mut stream,
                    self.deadline(),
                    Duration::from_secs(self.config.limits.idle_timeout_seconds),
                )
                .await
            }
            _ => Err("Handshake required".into()),
        }
    }
}
struct Registration {
    network: Weak<Network>,
    peer: String,
}
impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(n) = self.network.upgrade()
            && let Ok(mut status) = n.status.lock()
        {
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
        channel: u32,
        topic: String,
        owner: String,
        visited: Vec<String>,
        hops: u8,
        pipe: DuplexStream,
        reply: Reply,
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
    ) -> Result<(DuplexStream, Route), BoxError> {
        let channel = self.id()?;
        let (user, pipe) = tokio::io::duplex(CHUNK);
        let (tx, rx) = oneshot::channel();
        self.tx
            .try_send(Event::Open {
                channel,
                topic,
                owner,
                visited,
                hops,
                pipe,
                reply: tx,
            })
            .map_err(|_| "Peer channel capacity exhausted")?;
        let route = timeout(self.deadline, rx).await??.map_err(BoxError::from)?;
        Ok((user, route))
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
    certificate: String,
    revision: u64,
    incoming: bool,
}
impl Lease {
    fn permitted(&self, engine: &Engine) -> bool {
        !engine.config.disabled_peers.contains(&self.peer)
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
        tokio::select! {biased;
            result=revoked=>result,
            result=manager_work(ws,network,peer,initiator,handle,events)=>result,
        }
    })
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
        let mut queries: HashMap<String, (String, Reply)> = HashMap::new();
        let mut tasks = JoinSet::new();
        let work = Arc::new(tokio::sync::Semaphore::new(QUEUE));
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut active = tokio::time::Instant::now();
        loop {
            tokio::select! {
                _=tick.tick()=>{
                    if active.elapsed()>=idle {break;}
                    if let Some(net)=network.upgrade(){locked(&net.status)?.routes.retain(|_,r|r.route.until>now_ms());}
                    queries.retain(|_,(_,reply)|!reply.is_closed());
                    let closed:Vec<_>=channels.iter().filter(|(_,c)|c.reply.as_ref().is_some_and(|r|r.is_closed())).map(|(id,_)|*id).collect();
                    for channel in closed {channels.remove(&channel);timeout(deadline,ws.send(encode(Frame::TunnelClose {channel}))).await??;}
                },
                _=tasks.join_next(),if !tasks.is_empty()=>{},
                event=events.recv()=>{
                    let Some(event)=event else{break};
                    match event {
                        Event::Send(message)=>{timeout(deadline,ws.send(message)).await??;},
                        Event::Closed(channel)=>{if channels.remove(&channel).is_some(){timeout(deadline,ws.send(encode(Frame::TunnelClose {channel}))).await??;}},
                        Event::Query {id,topic,visited,hops,reply}=>{
                            if !initiator || queries.len()>=QUEUE {let _=reply.send(Err("Peer query capacity exhausted".into()));continue;}
                            queries.insert(id.clone(),(topic.clone(),reply));timeout(deadline,ws.send(encode(Frame::RouteQuery {id,topic,visited,hop_limit:hops}))).await??;
                        },
                        Event::Open {channel:id,topic,owner,visited,hops,pipe,reply}=>{
                            let Some(net)=network.upgrade() else{break};
                            let Ok(permit)=net.channel_slots.clone().try_acquire_owned() else{let _=reply.send(Err("Channel capacity exhausted".into()));continue};
                            if !initiator || channels.len()>=cap {let _=reply.send(Err("Channel capacity exhausted".into()));continue;}
                            locked(&net.status)?.active_channels+=1;
                        let permit=Arc::new(ChannelPermit {_permit:permit,status:net.status.clone()});
                        channels.insert(id,channel(pipe,Some(reply),&handle,id,&mut tasks,idle,permit));
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
                            let (expected,reply)=queries.remove(&request_id).ok_or("Unexpected route response")?;if expected!=topic{return Err("Route topic mismatch".into())}
                            let until=protocol::parse_time(&expires_at)?;let _=reply.send(Ok(Route {owner:owner_id,path,until}));
                        },
                        Frame::TunnelOpen {channel:id,topic,owner_id,visited,hop_limit} if !initiator=>{
                            let net=network.upgrade().ok_or("Service stopped")?;
                            if id<=last_channel||channels.contains_key(&id){return Err("Reused channel ID".into())}last_channel=id;
                            let permit=net.channel_slots.clone().try_acquire_owned();
                            if channels.len()>=cap || permit.is_err() || net.check_hop(&peer,&topic,&visited,hop_limit).is_err() || net.target(&topic)?!=owner_id {
                                timeout(deadline,ws.send(unavailable(None,Some(id)))).await??;continue;
                            }
                            let(user,pipe)=tokio::io::duplex(CHUNK);
                            let permit=permit?;locked(&net.status)?.active_channels+=1;
                        let permit=Arc::new(ChannelPermit {_permit:permit,status:net.status.clone()});
                        channels.insert(id,channel(pipe,None,&handle,id,&mut tasks,idle,permit.clone()));
                            let tx=handle.tx.clone();
                            tasks.spawn(async move {
                                let outcome=serve_channel(net,user,ChannelRequest {topic,owner:owner_id,visited,hops:hop_limit,_permit:permit},id,tx.clone()).await;
                                if outcome.is_err(){let _=timeout(deadline,tx.send(Event::Send(unavailable(None,Some(id))))).await;}
                                let _=timeout(deadline,tx.send(Event::Closed(id))).await;
                            });
                        },
                        Frame::TunnelReady {channel,path,expires_at} if initiator=>{
                            let reply=channels.get_mut(&channel).and_then(|c|c.reply.take()).ok_or("Unexpected channel readiness")?;
                            let until=protocol::parse_time(&expires_at)?;
                            let owner=path.last().cloned().ok_or("Empty owner route")?;let _=reply.send(Ok(Route {owner,path,until}));
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
        } = request;
        let deadline = net.deadline();
        if owner == net.config.service.id {
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
            return super::transport::websocket_connection(tls, engine, &net.config, None, None)
                .await;
        }
        let (mut upstream, route) =
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
        relay(
            &mut stream,
            &mut upstream,
            deadline,
            Duration::from_secs(net.config.limits.idle_timeout_seconds),
        )
        .await
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
            status: Default::default(),
            channel_slots: Arc::new(tokio::sync::Semaphore::new(1)),
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
        let lease = net.lease("child", fingerprint, Some(&peer)).unwrap();
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
