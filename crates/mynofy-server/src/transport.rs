//! WSS and local Unix sockets. A single connection processes request/response frames sequentially.
use crate::{BoxError, config::Config, engine::Engine, protocol};
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use std::{
    collections::HashMap,
    net::IpAddr,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    task::JoinSet,
    time::{Instant, timeout},
};
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::{
    TlsAcceptor,
    rustls::{self},
};
use tokio_tungstenite::tungstenite::{
    Message,
    protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
};

fn session_deadline(
    engine: &Arc<Mutex<Engine>>,
    id: u64,
    authed: bool,
    otherwise: Instant,
) -> Result<Instant, BoxError> {
    if !authed {
        return Ok(otherwise);
    }
    let until = locked(engine)?.credential_until(id, now_ms())?;
    Ok(Instant::now() + Duration::from_millis(until.saturating_sub(now_ms())))
}
async fn session_output<F>(
    future: F,
    invalidated: &mut tokio::sync::watch::Receiver<bool>,
    until: Instant,
    budget: Duration,
) -> Result<bool, BoxError>
where
    F: std::future::Future<Output = Result<(), tokio_tungstenite::tungstenite::Error>>,
{
    if *invalidated.borrow() {
        return Ok(false);
    }
    tokio::select! {
        biased;
        _ = invalidated.changed() => Ok(false),
        _ = tokio::time::sleep_until(until) => Ok(false),
        result = timeout(budget, future) => { result??; Ok(true) },
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
pub(crate) fn locked<T>(
    value: &Mutex<T>,
) -> std::result::Result<std::sync::MutexGuard<'_, T>, BoxError> {
    value
        .lock()
        .map_err(|_| "Service state lock is poisoned".into())
}
pub(crate) fn certs(
    path: &Path,
) -> std::result::Result<Vec<rustls::pki_types::CertificateDer<'static>>, BoxError> {
    let certs = rustls::pki_types::CertificateDer::pem_file_iter(path)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        return Err("No PEM certificates found".into());
    }
    Ok(certs)
}
pub fn server_tls(config: &Config) -> std::result::Result<Arc<rustls::ServerConfig>, BoxError> {
    let tls = config
        .service
        .tls
        .as_ref()
        .ok_or("TCP listener requires TLS configuration")?;
    let chain = certs(&tls.certificate_file)?;
    let key = rustls::pki_types::PrivateKeyDer::from_pem_file(&tls.private_key_file)?;
    let builder = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])?;
    let builder = if let Some(ca) = tls.peer_ca_file.as_ref() {
        let mut roots = rustls::RootCertStore::empty();
        for cert in certs(ca)? {
            roots.add(cert)?;
        }
        builder.with_client_cert_verifier(
            rustls::server::WebPkiClientVerifier::builder_with_provider(
                Arc::new(roots),
                Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
            )
            .allow_unauthenticated()
            .build()?,
        )
    } else {
        builder.with_no_client_auth()
    };
    let config = builder.with_single_cert(chain, key)?;
    Ok(Arc::new(config))
}
pub(crate) fn ws_config(max: usize) -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(8192.min(max))
        .write_buffer_size(16_384.min(max))
        .max_message_size(Some(max))
        .max_frame_size(Some(max))
        .max_write_buffer_size(max + 32_768)
}
struct Source {
    active: usize,
    tokens: u64,
    updated: Instant,
}
#[derive(Default)]
struct Sources {
    entries: HashMap<SourceId, Source>,
    expiry: std::collections::BTreeSet<(Instant, SourceId)>,
}
const SOURCE_TOKEN: u64 = 1_000_000_000;
const SOURCE_RETENTION: Duration = Duration::from_secs(60);
impl Sources {
    fn prune(&mut self, now: Instant) {
        for _ in 0..32 {
            let Some(&(until, key)) = self.expiry.first() else {
                break;
            };
            if until > now {
                break;
            }
            self.expiry.pop_first();
            self.entries.remove(&key);
        }
    }

    fn take(&mut self, key: SourceId, now: Instant, cap: usize, burst: u64, rate: u64) -> bool {
        let entry = self.entries.entry(key).or_insert(Source {
            active: 0,
            tokens: burst * SOURCE_TOKEN,
            updated: now,
        });
        if entry.active == 0 {
            self.expiry.remove(&(entry.updated + SOURCE_RETENTION, key));
        }
        entry.tokens = entry
            .tokens
            .saturating_add(
                u64::try_from(now.duration_since(entry.updated).as_nanos())
                    .unwrap_or(u64::MAX)
                    .saturating_mul(rate),
            )
            .min(burst * SOURCE_TOKEN);
        entry.updated = now;
        if entry.active >= cap || entry.tokens < SOURCE_TOKEN {
            if entry.active == 0 && !key.is_overflow() {
                self.expiry.insert((now + SOURCE_RETENTION, key));
            }
            return false;
        }
        entry.active += 1;
        entry.tokens -= SOURCE_TOKEN;
        true
    }

    fn release(&mut self, key: SourceId) {
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.active -= 1;
            if entry.active == 0 && !key.is_overflow() {
                self.expiry.insert((entry.updated + SOURCE_RETENTION, key));
            }
        }
    }
}
struct SourceGuard {
    primary: SourceId,
    prefix: Option<SourceId>,
    sources: Arc<Mutex<Sources>>,
}
impl Drop for SourceGuard {
    fn drop(&mut self) {
        if let Ok(mut sources) = self.sources.lock() {
            sources.release(self.primary);
            if let Some(prefix) = self.prefix {
                sources.release(prefix);
            }
        }
    }
}
struct SessionGuard {
    id: u64,
    engine: Arc<Mutex<Engine>>,
}
impl Drop for SessionGuard {
    fn drop(&mut self) {
        if let Ok(mut e) = self.engine.lock() {
            e.disconnect(self.id);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum SourceId {
    Overflow,
    PrefixOverflow,
    V6Prefix(u64),

    Ip(IpAddr),
    #[cfg(unix)]
    Uid(u32),
}
impl SourceId {
    fn is_overflow(self) -> bool {
        matches!(self, Self::Overflow | Self::PrefixOverflow)
    }
}
pub(crate) struct Egress {
    budget: Semaphore,
    rejection_slots: Semaphore,
    rotation_slots: Semaphore,
    pub rejected: std::sync::atomic::AtomicU64,
}
impl Egress {
    pub(crate) fn new(bytes: usize) -> Self {
        Self {
            budget: Semaphore::new(bytes),
            rejection_slots: Semaphore::new(4),
            rotation_slots: Semaphore::new(4),
            rejected: std::sync::atomic::AtomicU64::new(0),
        }
    }
}
struct ServerState {
    config: Config,
    engine: Arc<Mutex<Engine>>,
    slots: Arc<Semaphore>,
    sources: Arc<Mutex<Sources>>,
    network: Arc<crate::links::Network>,
}
impl ServerState {
    fn new(
        config: Config,
        secret: Option<crate::content::ContentSecret>,
    ) -> std::result::Result<Arc<Self>, BoxError> {
        config.require_local_owner()?;
        let engine = Engine::new_with_content_secret(config, now_ms(), secret)?;
        let config = engine.config().clone();
        let engine = Arc::new(Mutex::new(engine));
        let network = crate::links::Network::new(config.clone(), Arc::downgrade(&engine))?;
        Ok(Arc::new(Self {
            engine,
            network,
            slots: Arc::new(Semaphore::new(config.limits.max_connections)),
            sources: Arc::new(Mutex::new(Sources::default())),
            config,
        }))
    }
    fn admit(
        &self,
        source: SourceId,
    ) -> std::result::Result<Option<(tokio::sync::OwnedSemaphorePermit, SourceGuard)>, BoxError>
    {
        let Ok(permit) = self.slots.clone().try_acquire_owned() else {
            return Ok(None);
        };
        let source = match source {
            SourceId::Ip(IpAddr::V6(ip)) => ip
                .to_ipv4_mapped()
                .map_or(source, |v4| SourceId::Ip(IpAddr::V4(v4))),
            _ => source,
        };
        let now = Instant::now();
        let limits = &self.config.limits;
        let mut sources = locked(&self.sources)?;
        sources.prune(now);
        let capacity = limits.max_connections.saturating_mul(4);
        let primary = if sources.entries.len() >= capacity && !sources.entries.contains_key(&source)
        {
            SourceId::Overflow
        } else {
            source
        };
        let admitted = if primary.is_overflow() {
            sources.take(
                primary,
                now,
                (limits.max_connections / 32).clamp(1, 8),
                4,
                2,
            )
        } else {
            sources.take(
                primary,
                now,
                limits.max_connections_per_source,
                limits.source_auth_burst,
                limits.source_auth_requests_per_second,
            )
        };
        if !admitted {
            return Ok(None);
        }
        let mut prefix = None;
        if let SourceId::Ip(IpAddr::V6(ip)) = source
            && ip.to_ipv4_mapped().is_none()
            && let Some(cap) = limits.max_connections_per_ipv6_prefix
        {
            let key = SourceId::V6Prefix((u128::from(ip) >> 64) as u64);
            let key = if sources.entries.len() >= capacity && !sources.entries.contains_key(&key) {
                SourceId::PrefixOverflow
            } else {
                key
            };
            if !sources.take(
                key,
                now,
                cap,
                limits.source_auth_burst.saturating_mul(4),
                limits.source_auth_requests_per_second.saturating_mul(4),
            ) {
                sources.release(primary);
                return Ok(None);
            }
            prefix = Some(key);
        }
        Ok(Some((
            permit,
            SourceGuard {
                primary,
                prefix,
                sources: self.sources.clone(),
            },
        )))
    }
}
/// Fully loaded owner. Preparation happens once, before the CLI binds any listener.
pub struct PreparedService {
    state: Arc<ServerState>,
    tls: Option<TlsAcceptor>,
    admin: Option<crate::admin::AdminFuture>,
}
impl PreparedService {
    /// Bind the configured Admin socket before announcing listener readiness.
    pub fn bind_admin(&mut self) -> std::result::Result<(), BoxError> {
        if self.admin.is_none() {
            self.admin = Some(crate::admin::start(
                &self.state.config,
                self.state.engine.clone(),
            )?);
        }
        Ok(())
    }

    fn take_admin(&mut self) -> std::result::Result<crate::admin::AdminFuture, BoxError> {
        self.bind_admin()?;
        self.admin
            .take()
            .ok_or_else(|| "Admin listener was not prepared".into())
    }
}
pub fn prepare(
    mut config: Config,
    secret: Option<crate::content::ContentSecret>,
) -> std::result::Result<PreparedService, BoxError> {
    if config.security_storage.is_none() {
        return Err("Network owners require initialized security storage; use prepare_volatile for a RAM-only owner".into());
    }
    config.require_initialized_security = true;
    config.require_content_store_id()?;
    prepare_owner(config, secret)
}

/// Explicitly opt into a network owner whose content and security are both in RAM.
pub fn prepare_volatile(config: Config) -> std::result::Result<PreparedService, BoxError> {
    if config.security_storage.is_some() || config.content_storage.is_some() {
        return Err("A volatile network owner cannot have disk storage configured".into());
    }
    prepare_owner(config, None)
}

fn prepare_owner(
    config: Config,
    secret: Option<crate::content::ContentSecret>,
) -> std::result::Result<PreparedService, BoxError> {
    config.require_local_owner()?;
    let tls = if config.service.listen.is_some() {
        Some(TlsAcceptor::from(server_tls(&config)?))
    } else {
        None
    };
    Ok(PreparedService {
        state: ServerState::new(config, secret)?,
        tls,
        admin: None,
    })
}
pub async fn run(listener: TcpListener, config: Config) -> std::result::Result<(), BoxError> {
    run_prepared(listener, prepare(config, None)?).await
}
pub async fn run_prepared(
    listener: TcpListener,
    prepared: PreparedService,
) -> std::result::Result<(), BoxError> {
    run_prepared_until(listener, prepared, std::future::pending()).await
}

/// Stop accepting work when the shutdown future completes and report the durable drain.
pub async fn run_prepared_until(
    listener: TcpListener,
    prepared: PreparedService,
    shutdown: impl std::future::Future<Output = std::io::Result<()>>,
) -> std::result::Result<(), BoxError> {
    tokio::pin!(shutdown);
    let mut prepared = prepared;
    let mut admin = prepared.take_admin()?;
    let PreparedService { state, tls, .. } = prepared;
    let tls = tls.ok_or("Prepared service is not a TCP/TLS owner")?;
    let mut tasks = JoinSet::new();
    let mut maintenance = Box::pin(tokio::time::sleep(Duration::from_secs(1)));
    let result = async {
        loop {
        tokio::select! {
            stopped=&mut shutdown=>{return stopped.map_err(Into::into);},
            result=&mut admin=>{return result;},
            result=listener.accept()=>{
                let(stream,address)=result?;
                let Some(admission)=state.admit(SourceId::Ip(address.ip()))? else{continue};
                let state=state.clone();let tls=tls.clone();
                tasks.spawn(async move{
                    let _admission=admission;
                    let _=connection(stream,tls,state.engine.clone(),&state.config,&state.network).await;
                });
            },
            _=&mut maintenance=>{
                let progress = locked(&state.engine)?.maintenance(now_ms());
                let delay = if progress.more { Duration::from_millis(10) } else { Duration::from_secs(1) };
                maintenance.as_mut().reset(Instant::now() + delay);
            },
            _=tasks.join_next(),if !tasks.is_empty()=>{},
        }
        }
    }.await;
    drop(admin);
    drop(listener);
    tasks.shutdown().await;
    state.network.shutdown().await;
    let drained = finish_shutdown(state.engine.clone()).await;
    drained.and(result)
}
async fn finish_shutdown(engine: Arc<Mutex<Engine>>) -> std::result::Result<(), BoxError> {
    tokio::task::spawn_blocking(move || {
        let (content, security) = locked(&engine)?.begin_shutdown();
        // Content may hold a durability fence against the Security writer.
        let content = content.map_or(Ok(()), |mut writer| writer.drain());
        let security = security.map_or(Ok(()), |mut writer| writer.drain());
        match (content, security) {
            (Err(content), Err(security)) => Err(format!("{content}; {security}").into()),
            (content, security) => content.and(security),
        }
    })
    .await?
}
async fn connection(
    stream: TcpStream,
    tls: TlsAcceptor,
    engine: Arc<Mutex<Engine>>,
    config: &Config,
    network: &Arc<crate::links::Network>,
) -> std::result::Result<(), BoxError> {
    let source = stream.peer_addr()?.ip();
    stream.set_nodelay(true)?;
    let network_deadline = Duration::from_secs(config.limits.request_timeout_seconds);
    let tls = timeout(network_deadline, tls.accept(stream)).await??;
    let certificate = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certs| certs.first())
        .cloned();
    websocket_connection(
        tls,
        engine,
        config,
        Some(network),
        certificate,
        None,
        Some(source),
    )
    .await
}
pub(crate) async fn websocket_connection<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    stream: S,
    engine: Arc<Mutex<Engine>>,
    config: &Config,
    network: Option<&Arc<crate::links::Network>>,
    certificate: Option<rustls::pki_types::CertificateDer<'static>>,
    tunnel: Option<(
        &str,
        tokio::sync::OwnedSemaphorePermit,
        tokio::sync::oneshot::Sender<()>,
    )>,
    source: Option<std::net::IpAddr>,
) -> std::result::Result<(), BoxError> {
    let network_deadline = Duration::from_secs(config.limits.request_timeout_seconds);
    let mut ws = timeout(
        network_deadline,
        tokio_tungstenite::accept_async_with_config(
            stream,
            Some(
                ws_config(config.limits.max_response_bytes)
                    .max_message_size(Some(16 * 1024))
                    .max_frame_size(Some(16 * 1024)),
            ),
        ),
    )
    .await??;
    let auth_until = Instant::now() + Duration::from_secs(config.limits.auth_timeout_seconds);
    let mut first = if let Some(network) = network {
        let first = match timeout(
            Duration::from_secs(config.limits.auth_timeout_seconds),
            ws.next(),
        )
        .await
        {
            Ok(Some(Ok(first))) => first,
            _ => {
                let _ = timeout(network_deadline, ws.close(None)).await;
                return Ok(());
            }
        };
        if crate::links::Network::special(&first) {
            return Box::pin(network.dispatch(ws, first, certificate, source)).await;
        }
        Some(first)
    } else {
        None
    };
    let id = locked(&engine)?.connect(now_ms())?;
    let mut invalidated = locked(&engine)?.session_changes(id)?;
    let _session = SessionGuard {
        id,
        engine: engine.clone(),
    };
    let mut idle_until = Instant::now() + Duration::from_secs(config.limits.idle_timeout_seconds);
    let (tunnel_topic, mut pending_tunnel) = tunnel
        .map_or((None, None), |(topic, permit, auth)| {
            (Some(topic), Some((permit, auth)))
        });
    let mut is_authed = false;
    let egress = locked(&engine)?.egress.clone();
    let mut reusable_output = crate::output::ResponseBuffer::new(config.limits.max_response_bytes);
    let mut check = tokio::time::interval(Duration::from_secs(1));
    loop {
        let message = tokio::select! {
            biased;
            _=invalidated.changed()=>{return Ok(())},
            _=tokio::time::sleep_until(auth_until),if !is_authed=>{break},
            _=tokio::time::sleep_until(idle_until)=>{break},
            _=check.tick()=>{if is_authed && !locked(&engine)?.authenticated(id,now_ms()){break;}continue},
            msg=async {if first.is_some(){first.take().map(Ok)}else{ws.next().await}}=>{match msg{Some(Ok(msg))=>msg,_=>break}},
        };
        idle_until = Instant::now() + Duration::from_secs(config.limits.idle_timeout_seconds);
        match message {
            Message::Binary(raw) => {
                let decode = || {
                    if !is_authed && raw.len() > 1024 {
                        None
                    } else {
                        protocol::pb::Frame::decode(raw.as_ref()).ok()
                    }
                };
                let small_rotation = egress.is_some()
                    && is_authed
                    && config
                        .security_storage
                        .as_ref()
                        .is_some_and(|storage| storage.sync)
                    && protocol::correlation::is_rotation_commit(raw.clone());
                let admission = if let Some(egress) = &egress {
                    let deadline = session_deadline(&engine, id, is_authed, auth_until)?;
                    let waiting_until = deadline.min(Instant::now() + Duration::from_millis(100));
                    let (pool, allowance) = if small_rotation {
                        (&egress.rotation_slots, 1)
                    } else {
                        (&egress.budget, config.limits.max_response_bytes as u32)
                    };
                    let permit = tokio::select! {
                        biased;
                        _ = invalidated.changed() => return Ok(()),
                        permit = tokio::time::timeout_at(waiting_until,
                            pool.acquire_many(allowance)) => permit,
                    };
                    if let Ok(Ok(permit)) = permit {
                        Some(permit)
                    } else {
                        use std::sync::atomic::Ordering;
                        egress.rejected.fetch_add(1, Ordering::Relaxed);
                        if !is_authed || deadline <= Instant::now() {
                            return Ok(());
                        }
                        let Ok(_reserve) = egress.rejection_slots.try_acquire() else {
                            return Ok(());
                        };
                        let error = locked(&engine)?
                            .charge(id, now_ms())
                            .err()
                            .unwrap_or_else(protocol::ApiError::exhausted);
                        let request_id = match error.code {
                            "rate_limited" => None,
                            "resource_exhausted" => {
                                match protocol::correlation::request_id(raw, 16 * 1024) {
                                    Ok(id) => Some(id),
                                    Err(_) => return Ok(()),
                                }
                            }
                            _ => return Ok(()),
                        };
                        let response =
                            protocol::pb::Frame::error(error, request_id).encode_to_vec();
                        if response.len() > 1024
                            || !session_output(
                                ws.send(Message::Binary(response.into())),
                                &mut invalidated,
                                deadline,
                                network_deadline.min(Duration::from_millis(100)),
                            )
                            .await?
                        {
                            return Ok(());
                        }
                        continue;
                    }
                } else {
                    None
                };
                let mut limited_output = crate::output::ResponseBuffer::without_retention();
                let output = if admission.is_some() {
                    &mut limited_output
                } else {
                    &mut reusable_output
                };
                let slot = if is_authed {
                    let permit = {
                        let mut state = locked(&engine)?;
                        let now = now_ms();
                        let slots = state.request_semaphore(id, now)?;
                        state.charge(id, now).and_then(|()| {
                            slots
                                .try_acquire_owned()
                                .map_err(|_| protocol::ApiError::exhausted())
                        })
                    };
                    match permit {
                        Ok(permit) => Some(permit),
                        Err(error) => {
                            // Preserve the pre-decode rate-limit path. Capacity
                            // rejections need only schema-derived ID metadata.
                            let request_id = match error.code {
                                "rate_limited" => None,
                                "resource_exhausted" => match protocol::correlation::request_id(
                                    raw,
                                    config.limits.max_response_bytes,
                                ) {
                                    Ok(id) => Some(id),
                                    Err(_) => return Ok(()), // No execution and no guessed correlation.
                                },
                                _ => return Ok(()), // The identity became invalid before charging.
                            };
                            if !session_output(
                                ws.send({
                                    let frame = protocol::pb::Frame::error(error, request_id);
                                    let bytes = output.prepare();
                                    bytes.reserve(frame.encoded_len());
                                    frame.encode_raw(bytes);
                                    Message::Binary(output.take())
                                }),
                                &mut invalidated,
                                session_deadline(&engine, id, true, auth_until)?,
                                network_deadline,
                            )
                            .await?
                            {
                                return Ok(());
                            }
                            output.sent();
                            continue;
                        }
                    }
                } else {
                    None
                };
                let decoded = decode();
                let (kind, confirmation) = {
                    let mut state = locked(&engine)?;
                    let kind = state.write_protobuf_response(
                        id,
                        || decoded,
                        now_ms(),
                        output.prepare(),
                        tunnel_topic,
                        is_authed,
                    );
                    let confirmation = (kind == "rotation_complete")
                        .then(|| state.security_confirmation())
                        .flatten();
                    (kind, confirmation)
                };
                if let Ok(mut owned) = raw.try_into_mut() {
                    zeroize::Zeroize::zeroize(owned.as_mut());
                }
                let response = output.take();
                if small_rotation && response.len() > 1024 {
                    return Err("Rotation response exceeded its bounded allowance".into());
                }
                if let Some(confirmation) = confirmation {
                    // A failed commit confirmation is an unknown outcome, never a
                    // broker rejection promising that the rotation was not applied.
                    if !session_output(
                        async move {
                            confirmation.confirm().await.map_err(|error| {
                                tokio_tungstenite::tungstenite::Error::Io(std::io::Error::other(
                                    error,
                                ))
                            })
                        },
                        &mut invalidated,
                        session_deadline(&engine, id, true, auth_until)?,
                        network_deadline,
                    )
                    .await?
                    {
                        return Ok(());
                    }
                }
                let response = Message::Binary(response);
                let authenticated = locked(&engine)?.authenticated(id, now_ms());

                let close =
                    (!authenticated && kind != "auth_challenge") || (is_authed && !authenticated);
                // Never submit an already invalidated sensitive response to the sink.
                if is_authed && !authenticated {
                    return Ok(());
                }
                if !session_output(
                    ws.send(response),
                    &mut invalidated,
                    session_deadline(&engine, id, authenticated, auth_until)?,
                    network_deadline,
                )
                .await?
                {
                    return Ok(());
                }
                output.sent();
                if authenticated && let Some((_permit, notification)) = pending_tunnel.take() {
                    let _ = notification.send(());
                }
                is_authed = authenticated;
                drop(slot);
                if close {
                    break;
                }
            }
            Message::Ping(_) => {
                if !is_authed || locked(&engine)?.charge(id, now_ms()).is_err() {
                    break;
                }
                if !session_output(
                    ws.flush(),
                    &mut invalidated,
                    session_deadline(&engine, id, true, auth_until)?,
                    network_deadline,
                )
                .await?
                {
                    return Ok(());
                }
            }
            Message::Pong(_) => {
                if !is_authed || locked(&engine)?.charge(id, now_ms()).is_err() {
                    break;
                }
            }
            Message::Close(_) => break,
            Message::Text(_) | Message::Frame(_) => {
                let _ = session_output(
                    ws.close(Some(CloseFrame {
                        code: CloseCode::Unsupported,
                        reason: "Protobuf binary frames required".into(),
                    })),
                    &mut invalidated,
                    session_deadline(&engine, id, is_authed, auth_until)?,
                    network_deadline,
                )
                .await?;
                return Ok(());
            }
        }
    }
    if is_authed && !locked(&engine)?.authenticated(id, now_ms()) {
        return Ok(());
    }
    let _ = session_output(
        ws.close(None),
        &mut invalidated,
        session_deadline(&engine, id, is_authed, auth_until)?,
        network_deadline,
    )
    .await;
    Ok(())
}

#[cfg(unix)]
#[path = "unix_transport.rs"]
mod unix;
#[cfg(unix)]
pub use unix::{BoundUnixSocket, bind_unix, run_unix, run_unix_prepared, run_unix_prepared_until};

#[cfg(test)]
mod session_output_regressions {
    use super::*;
    use crate::config::Config;
    use protocol::{ProtobufMessage, crypto};
    use serde_json::{Value, json};

    async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
        ws: &mut tokio_tungstenite::WebSocketStream<S>,
        request: Value,
    ) -> Value {
        let frame = protocol::pb::Frame::from_json(request).unwrap();
        ws.send(Message::Binary(frame.encode_to_vec().into()))
            .await
            .unwrap();
        let response = ws.next().await.unwrap().unwrap().into_data();
        protocol::pb::Frame::decode(response)
            .unwrap()
            .to_json()
            .unwrap()
    }

    async fn case(hold_permit: bool, expire: bool) -> bool {
        let key = crypto::generate_key().unwrap();
        let public = crypto::public_key(&key).unwrap();
        let id = crypto::initial_client_id(&public);
        let mut config: Config =
            serde_json::from_str(include_str!("../../../config/scenarios/unix.json")).unwrap();
        config.security_storage = None;
        config.content_storage = None;
        config.clients=serde_json::from_value(json!([{"client_id":id,"keys":[{"algorithm":"ed25519","public_key":crypto::encode(&public),"valid_until":"2035-01-01T00:00:00Z"}]}])).unwrap();
        config.topics[0].acl = Some(
            serde_json::from_value(json!([{"client_id":id,"allow":["publish","subscribe"]}]))
                .unwrap(),
        );
        config.limits.max_parallel_requests_per_client = 1;
        config.limits.request_timeout_seconds = 30;
        if expire {
            config.clients[0].keys[0].valid_until = protocol::timestamp(now_ms() + 2000).unwrap();
        }
        config.validate().unwrap();
        let engine = Arc::new(Mutex::new(Engine::new(config.clone(), now_ms()).unwrap()));
        let (server, client) = tokio::io::duplex(128);
        let server_engine = engine.clone();
        let service_id = config.service.id.clone();
        let mut job = tokio::spawn(async move {
            websocket_connection(server, server_engine, &config, None, None, None, None).await
        });
        let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost/", client)
            .await
            .unwrap();
        let challenge = exchange(
            &mut ws,
            json!({"version":1,"type":"auth_hello","id":"h","token_id":crypto::token_id(&public)}),
        )
        .await;
        let nonce = crypto::decode(challenge["nonce"].as_str().unwrap()).unwrap();
        let signature =
            crypto::sign(&key, &crypto::auth_bytes(&nonce, &public, &service_id)).unwrap();
        let welcome = exchange(
            &mut ws,
            json!({"version":1,"type":"auth_proof","id":"p","auth_id":"h","signature":signature}),
        )
        .await;
        assert_eq!(welcome["type"], "auth_welcome");
        let permit = hold_permit.then(|| {
            engine
                .lock()
                .unwrap()
                .request_semaphore(1, now_ms())
                .unwrap()
                .try_acquire_owned()
                .unwrap()
        });
        let request=protocol::pb::Frame::from_json(json!({"version":1,"type":"get_permissions","id":"x".repeat(128),"topic":"local/events"})).unwrap();
        ws.send(Message::Binary(request.encode_to_vec().into()))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !job.is_finished(),
            "tiny duplex must block the pending response"
        );
        if !expire {
            engine.lock().unwrap().disconnect(1);
        }
        let closed = tokio::time::timeout(
            Duration::from_millis(if expire { 3000 } else { 250 }),
            &mut job,
        )
        .await
        .is_ok();
        if !closed {
            job.abort();
            let _ = job.await;
        }
        drop(permit);
        drop(ws);
        closed
    }

    #[tokio::test]
    async fn capacity_and_regular_sends_cancel_on_revocation_and_expiry() {
        for hold in [false, true] {
            assert!(
                case(hold, false).await,
                "blocked send ignored revocation, capacity={hold}"
            );
            assert!(
                case(hold, true).await,
                "blocked send ignored credential expiry, capacity={hold}"
            );
        }
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;

    #[test]
    fn source_refill_accumulates_submillisecond_intervals() {
        let mut fine = Sources::default();
        let mut coarse = Sources::default();
        let now = Instant::now();
        let key = SourceId::Overflow;
        assert!(fine.take(key, now, 1, 1, 1000));
        assert!(coarse.take(key, now, 1, 1, 1000));
        fine.release(key);
        coarse.release(key);
        for microseconds in [250, 500, 750] {
            assert!(!fine.take(key, now + Duration::from_micros(microseconds), 1, 1, 1000));
        }
        assert!(fine.take(key, now + Duration::from_millis(1), 1, 1, 1000));
        assert!(coarse.take(key, now + Duration::from_millis(1), 1, 1, 1000));
    }

    #[tokio::test]
    async fn egress_rejection_is_correlated_and_connection_recovers() {
        use protocol::pb::{self, Body};
        let mut config: Config =
            serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
        let key = crate::crypto::generate_key().unwrap();
        let public = crate::crypto::public_key(&key).unwrap();
        config.clients[0].client_id = crate::crypto::initial_client_id(&public);
        config.clients[0].keys[0].public_key = crate::crypto::encode(&public);
        config.topics[0].acl.as_mut().unwrap()[0].client_id = config.clients[0].client_id.clone();
        let capacity = config.limits.max_response_bytes;
        config.limits.max_egress_bytes = Some(capacity);
        let engine = Arc::new(Mutex::new(Engine::new(config.clone(), now_ms()).unwrap()));
        let egress = locked(&engine).unwrap().egress.clone().unwrap();
        let (client, server) = tokio::io::duplex(32_768);
        let state = engine.clone();
        let task = tokio::spawn(async move {
            websocket_connection(server, state, &config, None, None, None, None)
                .await
                .unwrap();
        });
        let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost", client)
            .await
            .unwrap();
        async fn call(
            ws: &mut tokio_tungstenite::WebSocketStream<tokio::io::DuplexStream>,
            body: Body,
        ) -> Body {
            ws.send(Message::Binary(
                pb::Frame {
                    version: 1,
                    body: Some(body),
                }
                .encode_to_vec()
                .into(),
            ))
            .await
            .unwrap();
            let raw = ws.next().await.unwrap().unwrap().into_data();
            pb::Frame::decode(raw).unwrap().body.unwrap()
        }
        let Body::AuthChallenge(challenge) = call(
            &mut ws,
            Body::AuthHello(pb::AuthHello {
                id: "hello".into(),
                token_id: crate::crypto::token_id(&public),
            }),
        )
        .await
        else {
            panic!("expected auth challenge")
        };
        let nonce = crate::crypto::decode::<32>(&challenge.nonce).unwrap();
        let signature =
            crate::crypto::sign(&key, &crate::crypto::auth_bytes(&nonce, &public, "local"))
                .unwrap();
        assert!(matches!(
            call(
                &mut ws,
                Body::AuthProof(pb::AuthProof {
                    id: "proof".into(),
                    auth_id: "hello".into(),
                    signature,
                })
            )
            .await,
            Body::AuthWelcome(_)
        ));

        let held = egress.budget.acquire_many(capacity as u32).await.unwrap();
        let publish = |id: &str| {
            Body::Publish(pb::Publish {
                id: id.into(),
                topic: "local/events".into(),
                payload: protocol::payload::Payload::bytes(vec![7; 4096], None),
                expires: None,
            })
        };
        let Body::Error(error) = call(&mut ws, publish("rejected")).await else {
            panic!("expected rejection")
        };
        assert_eq!(error.request_id.as_deref(), Some("rejected"));
        assert_eq!(error.code, "resource_exhausted");
        assert_eq!(
            egress.rejected.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        assert_eq!(locked(&engine).unwrap().storage_stats().active_records, 0);
        drop(held);
        assert!(matches!(
            call(&mut ws, publish("retry")).await,
            Body::Result(_)
        ));
        drop(ws);
        task.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn publish_uses_normal_egress_while_rotation_waits_for_security() {
        use std::os::unix::fs::DirBuilderExt;
        let directory = std::env::temp_dir().join(format!(
            "mne-{:016x}",
            u64::from_le_bytes(crate::crypto::random().unwrap())
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let path = directory.join("data.sock");
        let mut config: Config =
            serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
        config.service.listen = None;
        config.service.tls = None;
        config.service.unix_socket = Some(path.clone());
        config.security_storage = Some(crate::config::SecurityStorage {
            sync: true,
            directory: directory.join("security"),
            minimum_sequence: None,
            queue_capacity: 8,
            compact_after_records: 128,
        });
        config.limits.max_egress_bytes = Some(config.limits.max_response_bytes);
        let old_key = crate::crypto::generate_key().unwrap();
        let publish_key = crate::crypto::generate_key().unwrap();
        let mut clients = Vec::new();
        for key in [&old_key, &publish_key] {
            let public = crate::crypto::public_key(key).unwrap();
            let mut client = config.clients[0].clone();
            client.client_id = crate::crypto::initial_client_id(&public);
            client.keys[0].public_key = crate::crypto::encode(&public);
            clients.push(client);
        }
        let rotating_id = clients[0].client_id.clone();
        config.topics[0].acl.as_mut().unwrap()[0].client_id = clients[1].client_id.clone();
        config.clients = clients;
        crate::initialize_security(&config).unwrap();
        let prepared = prepare(config, None).unwrap();
        let engine = prepared.state.engine.clone();
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        locked(&engine)
            .unwrap()
            .set_security_write_gate(gate.clone());
        let bound = bind_unix(&path).unwrap();
        let (stop, ending) = tokio::sync::oneshot::channel();
        let service = tokio::spawn(run_unix_prepared_until(bound, prepared, async move {
            ending.await.map_err(std::io::Error::other)
        }));
        let mut rotating = mynofy_client::connect_unix_client(&path, &old_key, "local", None)
            .await
            .unwrap();
        let mut publisher = mynofy_client::connect_unix_client(&path, &publish_key, "local", None)
            .await
            .unwrap();
        let change = tokio::spawn(async move {
            let new_key = crate::crypto::generate_key().unwrap();
            mynofy_client::rotate_client(&mut rotating, &old_key, &new_key, "local", &rotating_id)
                .await
        });
        timeout(Duration::from_secs(1), async {
            loop {
                let status = locked(&engine).unwrap().security_status();
                if status.submitted_sequence > status.durable_sequence {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let published = mynofy_client::exchange(
            &mut publisher,
            &serde_json::json!({
                "version":1,"type":"publish","id":"concurrent","topic":"local/events","payload":"ok"
            }),
        )
        .await
        .unwrap();
        assert_eq!(published["type"], "result", "{published}");
        assert!(!change.is_finished());
        let (released, ready) = gate.as_ref();
        *released.lock().unwrap() = true;
        ready.notify_all();
        assert_eq!(change.await.unwrap().unwrap()["type"], "rotation_complete");
        stop.send(()).unwrap();
        service.await.unwrap().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn status_and_poll_continue_while_four_admin_commands_wait() {
        use crate::admin::{Command, request};
        use std::os::unix::fs::DirBuilderExt;

        let directory = std::env::temp_dir().join(format!(
            "mna-wait-{:016x}",
            u64::from_le_bytes(crate::crypto::random().unwrap())
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let data = directory.join("d.sock");
        let admin = directory.join("a.sock");
        let mut config: Config =
            serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
        config.service.listen = None;
        config.service.tls = None;
        config.service.unix_socket = Some(data.clone());
        config.service.admin_socket = Some(admin.clone());
        config.security_storage = Some(crate::config::SecurityStorage {
            minimum_sequence: None,
            sync: true,
            directory: directory.join("security"),
            queue_capacity: 8,
            compact_after_records: 128,
        });
        let key = crate::crypto::generate_key().unwrap();
        let public = crate::crypto::public_key(&key).unwrap();
        let template = config.clients[0].clone();
        config.clients[0].client_id = crate::crypto::initial_client_id(&public);
        config.clients[0].keys[0].public_key = crate::crypto::encode(&public);
        config.topics[0].acl.as_mut().unwrap()[0].client_id = config.clients[0].client_id.clone();
        for _ in 0..4 {
            let public =
                crate::crypto::public_key(&crate::crypto::generate_key().unwrap()).unwrap();
            let mut client = template.clone();
            client.client_id = crate::crypto::initial_client_id(&public);
            client.keys[0].public_key = crate::crypto::encode(&public);
            config.clients.push(client);
        }
        let targets: Vec<_> = config.clients[1..]
            .iter()
            .map(|client| client.client_id.clone())
            .collect();
        crate::initialize_security(&config).unwrap();
        let mut prepared = prepare(config, None).unwrap();
        let engine = prepared.state.engine.clone();
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        locked(&engine)
            .unwrap()
            .set_security_write_gate(gate.clone());
        prepared.bind_admin().unwrap();
        let bound = bind_unix(&data).unwrap();
        let (stop, ending) = tokio::sync::oneshot::channel();
        let service = tokio::spawn(run_unix_prepared_until(bound, prepared, async move {
            ending.await.map_err(std::io::Error::other)
        }));
        let mut client = mynofy_client::connect_unix_client(&data, &key, "local", None)
            .await
            .unwrap();
        let mut changes = JoinSet::new();
        for target in targets {
            let path = admin.clone();
            changes.spawn(async move {
                request(
                    &path,
                    Command::ClientDisable {
                        client: target,
                        disabled: true,
                    },
                )
                .await
            });
        }
        timeout(Duration::from_secs(1), async {
            while locked(&engine)
                .unwrap()
                .security_status()
                .submitted_sequence
                < 4
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let mut partial = tokio::net::UnixStream::connect(&admin).await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut partial, b"{")
            .await
            .unwrap();
        let status = request(&admin, Command::Status {}).await.unwrap();
        assert_eq!(
            status["result"]["security_storage"]["submitted_sequence"],
            4
        );
        assert_eq!(status["result"]["security_storage"]["durable_sequence"], 0);
        let page = mynofy_client::exchange(
            &mut client,
            &serde_json::json!({
                "version":1,"type":"poll","id":"concurrent","topic":"local/events"
            }),
        )
        .await
        .unwrap();
        assert_eq!(page["type"], "messages");
        drop(partial);
        let (released, ready) = gate.as_ref();
        *released.lock().unwrap() = true;
        ready.notify_all();
        while let Some(result) = changes.join_next().await {
            assert_eq!(result.unwrap().unwrap()["ok"], true);
        }
        stop.send(()).unwrap();
        service.await.unwrap().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn source_history_expires_in_bounded_batches_and_overflow_keeps_its_rate_limit() {
        let mut table = Sources::default();
        let now = Instant::now();
        for i in 0..64 {
            let key = SourceId::Ip(IpAddr::from([127, 0, 0, i]));
            assert!(table.take(key, now, 1, 1, 1));
            table.release(key);
        }
        assert!(table.take(SourceId::Overflow, now, 1, 1, 1));
        table.release(SourceId::Overflow);
        assert!(!table.take(SourceId::Overflow, now, 1, 1, 1));
        table.prune(now + SOURCE_RETENTION);
        assert_eq!(table.entries.len(), 33);
        table.prune(now + SOURCE_RETENTION);
        assert_eq!(table.entries.len(), 1);
        assert!(table.entries.contains_key(&SourceId::Overflow));
    }
}
