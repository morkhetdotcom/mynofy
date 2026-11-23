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
struct SourceGuard {
    source: SourceId,
    sources: Arc<Mutex<HashMap<SourceId, Source>>>,
}
impl Drop for SourceGuard {
    fn drop(&mut self) {
        if let Ok(mut s) = self.sources.lock()
            && let Some(v) = s.get_mut(&self.source)
        {
            v.active = v.active.saturating_sub(1);
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

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum SourceId {
    Ip(IpAddr),
    #[cfg(unix)]
    Uid(u32),
}
struct ServerState {
    config: Config,
    engine: Arc<Mutex<Engine>>,
    slots: Arc<Semaphore>,
    sources: Arc<Mutex<HashMap<SourceId, Source>>>,
    network: Arc<crate::links::Network>,
}
impl ServerState {
    fn new(
        config: Config,
        secret: Option<crate::content::ContentSecret>,
    ) -> std::result::Result<Arc<Self>, BoxError> {
        config.require_local_owner()?;
        let mut engine = Engine::new_with_content_secret(config, now_ms(), secret)?;
        let config = engine.config.clone();
        engine.start_id_allocator();
        let engine = Arc::new(Mutex::new(engine));
        let network = crate::links::Network::new(config.clone(), Arc::downgrade(&engine))?;
        Ok(Arc::new(Self {
            engine,
            network,
            slots: Arc::new(Semaphore::new(config.limits.max_connections)),
            sources: Arc::new(Mutex::new(HashMap::new())),
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
        let now = Instant::now();
        let limits = &self.config.limits;
        let mut map = locked(&self.sources)?;
        map.retain(|_, v| v.active > 0 || now.duration_since(v.updated) < Duration::from_secs(60));
        if !map.contains_key(&source) && map.len() >= limits.max_connections.saturating_mul(4) {
            return Ok(None);
        }
        let v = map.entry(source).or_insert(Source {
            active: 0,
            tokens: limits.source_auth_burst * 1000,
            updated: now,
        });
        v.tokens = v
            .tokens
            .saturating_add(
                (now.duration_since(v.updated)
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64)
                    .saturating_mul(limits.source_auth_requests_per_second),
            )
            .min(limits.source_auth_burst * 1000);
        v.updated = now;
        if v.active >= limits.max_connections_per_source || v.tokens < 1000 {
            return Ok(None);
        }
        v.active += 1;
        v.tokens -= 1000;
        Ok(Some((
            permit,
            SourceGuard {
                source,
                sources: self.sources.clone(),
            },
        )))
    }
}
/// Fully loaded owner. Preparation happens once, before the CLI binds any listener.
pub struct PreparedService {
    state: Arc<ServerState>,
    tls: Option<TlsAcceptor>,
}
pub fn prepare(
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
    })
}
pub async fn run(listener: TcpListener, config: Config) -> std::result::Result<(), BoxError> {
    run_prepared(listener, prepare(config, None)?).await
}
pub async fn run_prepared(
    listener: TcpListener,
    prepared: PreparedService,
) -> std::result::Result<(), BoxError> {
    let PreparedService { state, tls } = prepared;
    let tls = tls.ok_or("Prepared service is not a TCP/TLS owner")?;
    let mut admin = crate::admin::start(&state.config, state.engine.clone())?;
    let mut tasks = JoinSet::new();
    let mut maintenance = Box::pin(tokio::time::sleep(Duration::from_secs(1)));
    loop {
        tokio::select! {
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
}
async fn connection(
    stream: TcpStream,
    tls: TlsAcceptor,
    engine: Arc<Mutex<Engine>>,
    config: &Config,
    network: &Arc<crate::links::Network>,
) -> std::result::Result<(), BoxError> {
    stream.set_nodelay(true)?;
    let network_deadline = Duration::from_secs(config.limits.request_timeout_seconds);
    let tls = timeout(network_deadline, tls.accept(stream)).await??;
    let certificate = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certs| certs.first())
        .cloned();
    websocket_connection(tls, engine, config, Some(network), certificate).await
}
pub(crate) async fn websocket_connection<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    stream: S,
    engine: Arc<Mutex<Engine>>,
    config: &Config,
    network: Option<&Arc<crate::links::Network>>,
    certificate: Option<rustls::pki_types::CertificateDer<'static>>,
) -> std::result::Result<(), BoxError> {
    let network_deadline = Duration::from_secs(config.limits.request_timeout_seconds);
    let mut ws = timeout(
        network_deadline,
        tokio_tungstenite::accept_async_with_config(
            stream,
            Some(ws_config(config.limits.max_response_bytes)),
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
            return Box::pin(network.dispatch(ws, first, certificate)).await;
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
    let mut is_authed = false;
    let mut output = crate::output::ResponseBuffer::new(config.limits.max_response_bytes);
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
                let slot = if is_authed {
                    let request_slots = locked(&engine)?.request_semaphore(id, now_ms())?;
                    match request_slots.try_acquire_owned() {
                        Ok(permit) => Some(permit),
                        Err(_) => {
                            let error = locked(&engine)?
                                .charge(id, now_ms())
                                .err()
                                .unwrap_or_else(protocol::ApiError::exhausted);
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
                let kind =
                    locked(&engine)?.handle_protobuf_into(id, &raw, now_ms(), output.prepare());
                let response = Message::Binary(output.take());
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
pub use unix::{BoundUnixSocket, bind_unix, run_unix, run_unix_prepared};

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
            websocket_connection(server, server_engine, &config, None, None).await
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
