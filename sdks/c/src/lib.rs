//! Native C ABI. All pointer access is confined to this crate; see include/mynofy.h.
#![deny(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)] // The common C memory contract is documented in the header.
mod abi;
mod response;
use abi::{Bytes, Str};
use mynofy_client::{
    self as client,
    protocol::{self, payload, pb},
};
use std::{
    ffi::c_char,
    path::Path,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{runtime::Runtime, sync::watch};
use zeroize::Zeroizing;
type Result<T> = std::result::Result<T, Failure>;

#[derive(Debug)]
pub struct Failure {
    code: i32,
    message: String,
    unknown: bool,
    broker: String,
    retry: Option<u64>,
}
impl Failure {
    fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            unknown: false,
            broker: String::new(),
            retry: None,
        }
    }
    fn invalid(message: impl Into<String>) -> Self {
        Self::new(1, message)
    }
    fn network(e: protocol::BoxError, unknown: bool) -> Self {
        let rotation = e.downcast_ref::<client::RotationError>();
        if let Some(error) = rotation.and_then(|r| r.broker.as_ref()) {
            return Self {
                code: 5,
                message: error.detail.clone(),
                broker: error.code.clone(),
                retry: error.retry_after_ms,
                unknown: false,
            };
        }
        let unknown = rotation.map_or(unknown, client::RotationError::outcome_unknown);
        if e.is::<tokio::time::error::Elapsed>()
            || rotation.is_some_and(|r| {
                std::error::Error::source(r).is_some_and(|s| s.is::<tokio::time::error::Elapsed>())
            })
        {
            Self {
                unknown,
                ..Self::new(8, "transport phase timed out; client closed")
            }
        } else {
            Self::transport(e, unknown)
        }
    }
    fn transport(e: impl std::fmt::Display, unknown: bool) -> Self {
        Self {
            unknown,
            ..Self::new(4, e.to_string())
        }
    }
}
pub struct Key {
    der: Zeroizing<Vec<u8>>,
    public: [u8; 32],
    token: String,
    id: String,
}
impl Key {
    fn parse(der: &[u8]) -> Result<Self> {
        let public = client::crypto::public_key(der).map_err(Failure::invalid_display)?;
        Ok(Self {
            der: Zeroizing::new(der.to_vec()),
            public,
            token: client::crypto::token_id(&public),
            id: client::crypto::initial_client_id(&public),
        })
    }
}
impl Failure {
    fn invalid_display(e: impl std::fmt::Display) -> Self {
        Self::invalid(e.to_string())
    }
}
struct Options {
    url: Option<String>,
    socket: Option<String>,
    ca: Option<String>,
    owner: String,
    expected: Option<String>,
    via: Option<String>,
    owner_ca: Option<String>,
    owner_name: Option<String>,
    timeout: Duration,
}
impl Options {
    unsafe fn parse(c: abi::Config) -> Result<Self> {
        if c.abi_version != 2 {
            return Err(Failure::invalid("config ABI version mismatch"));
        }
        // SAFETY: caller provides live NUL-terminated config strings.
        let (url, socket, ca, owner, expected, via, owner_ca, owner_name) = unsafe {
            (
                abi::text(c.url, 2048)?.map(str::to_owned),
                abi::text(c.socket_path, 4096)?.map(str::to_owned),
                abi::text(c.ca_file, 4096)?.map(str::to_owned),
                abi::required(c.owner, 256)?.to_owned(),
                abi::text(c.client_id, 512)?.map(str::to_owned),
                abi::text(c.via_topic, 256)?.map(str::to_owned),
                abi::text(c.owner_ca_file, 4096)?.map(str::to_owned),
                abi::text(c.owner_name, 256)?.map(str::to_owned),
            )
        };
        if (url.is_some() == socket.is_some())
            || url.as_ref().is_some_and(|u| !u.starts_with("wss://"))
            || (url.is_some() && ca.is_none())
            || (socket.is_some() && ca.is_some())
            || (via.is_some() != owner_ca.is_some())
            || (via.is_none() && owner_name.is_some())
        {
            return Err(Failure::invalid("invalid connection configuration"));
        }
        if [&url, &socket, &ca, &expected, &via, &owner_ca, &owner_name]
            .iter()
            .any(|v| v.as_ref().is_some_and(String::is_empty))
        {
            return Err(Failure::invalid(
                "optional strings must be NULL or nonempty",
            ));
        }
        if let Some(value) = &url {
            let uri: tokio_tungstenite::tungstenite::http::Uri =
                value.parse().map_err(Failure::invalid_display)?;
            if uri.scheme_str() != Some("wss")
                || uri.host().is_none()
                || value.contains('#')
                || uri.authority().is_some_and(|v| v.as_str().contains('@'))
            {
                return Err(Failure::invalid(
                    "expected WSS URL without credentials or fragment",
                ));
            }
        }
        Ok(Self {
            url,
            socket,
            ca,
            owner,
            expected,
            via,
            owner_ca,
            owner_name,
            timeout: Duration::from_millis(if c.timeout_ms == 0 {
                10000
            } else {
                u64::from(c.timeout_ms)
            }),
        })
    }
    fn via(&self) -> client::ViaOptions<'_> {
        client::ViaOptions {
            topic: self.via.as_deref().unwrap_or(""),
            owner: &self.owner,
            owner_ca: Path::new(self.owner_ca.as_deref().unwrap_or("")),
            owner_certificate_identity: self.owner_name.as_deref().unwrap_or(&self.owner),
        }
    }
}
enum Socket {
    #[cfg(test)]
    Memory(Box<tokio_tungstenite::WebSocketStream<tokio::io::DuplexStream>>),
    Tcp(Box<client::ClientSocket>),
    ViaTcp(Box<client::OwnerSocket<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>>),
    #[cfg(unix)]
    Unix(Box<tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>>),
    #[cfg(unix)]
    ViaUnix(Box<client::OwnerSocket<tokio::net::UnixStream>>),
}
impl Socket {
    async fn connect(
        o: &Options,
        key: &Key,
    ) -> std::result::Result<(Self, pb::AuthWelcome), protocol::BoxError> {
        let expected = o.expected.as_deref();
        if let Some(url) = &o.url {
            let ca = Path::new(o.ca.as_deref().unwrap_or(""));
            if o.via.is_some() {
                let (s, w) = client::connect_via_info(url, ca, o.via(), &key.der, expected).await?;
                return Ok((Self::ViaTcp(Box::new(s)), w));
            }
            let (s, w) = client::connect_client_info(url, ca, &key.der, &o.owner, expected).await?;
            return Ok((Self::Tcp(Box::new(s)), w));
        }
        #[cfg(unix)]
        {
            let path = Path::new(o.socket.as_deref().unwrap_or(""));
            if o.via.is_some() {
                let (s, w) =
                    client::connect_unix_via_info(path, o.via(), &key.der, expected).await?;
                return Ok((Self::ViaUnix(Box::new(s)), w));
            }
            let (s, w) =
                client::connect_unix_client_info(path, &key.der, &o.owner, expected).await?;
            Ok((Self::Unix(Box::new(s)), w))
        }
        #[cfg(not(unix))]
        {
            Err("Unix sockets are unsupported on this platform".into())
        }
    }
    async fn exchange(
        &mut self,
        request: &pb::Frame,
    ) -> std::result::Result<pb::Frame, protocol::BoxError> {
        match self {
            #[cfg(test)]
            Self::Memory(s) => client::exchange_frame(&mut **s, request).await,
            Self::Tcp(s) => client::exchange_frame(&mut **s, request).await,
            Self::ViaTcp(s) => client::exchange_frame(&mut **s, request).await,
            #[cfg(unix)]
            Self::Unix(s) => client::exchange_frame(&mut **s, request).await,
            #[cfg(unix)]
            Self::ViaUnix(s) => client::exchange_frame(&mut **s, request).await,
        }
    }
    async fn rotate(
        &mut self,
        old: &[u8],
        new: &[u8],
        owner: &str,
        id: &str,
    ) -> std::result::Result<pb::Frame, protocol::BoxError> {
        let value = match self {
            #[cfg(test)]
            Self::Memory(s) => client::rotate_client(&mut **s, old, new, owner, id).await?,
            Self::Tcp(s) => client::rotate_client(&mut **s, old, new, owner, id).await?,
            Self::ViaTcp(s) => client::rotate_client(&mut **s, old, new, owner, id).await?,
            #[cfg(unix)]
            Self::Unix(s) => client::rotate_client(&mut **s, old, new, owner, id).await?,
            #[cfg(unix)]
            Self::ViaUnix(s) => client::rotate_client(&mut **s, old, new, owner, id).await?,
        };
        Ok(pb::Frame::from_json(value)?)
    }
}
struct State {
    socket: Option<Socket>,
    sequence: u64,
}
pub struct Client {
    state: Mutex<State>,
    closed: AtomicBool,
    cancel: watch::Sender<bool>,
    key: Key,
    owner: String,
    welcome: pb::AuthWelcome,
    timeout: Duration,
    runtime: Arc<Runtime>,
}
fn runtime() -> Result<Arc<Runtime>> {
    static RUNTIME: OnceLock<Mutex<Weak<Runtime>>> = OnceLock::new();
    let mut slot = RUNTIME
        .get_or_init(|| Mutex::new(Weak::new()))
        .lock()
        .map_err(|_| Failure::new(6, "runtime registry poisoned"))?;
    if let Some(runtime) = slot.upgrade() {
        return Ok(runtime);
    }
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("mynofy-c-io")
            .enable_all()
            .build()
            .map_err(|e| Failure::new(6, e.to_string()))?,
    );
    *slot = Arc::downgrade(&runtime);
    Ok(runtime)
}
impl Client {
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.cancel.send_replace(true);
        match self.state.try_lock() {
            Ok(mut state) => {
                state.socket.take();
            }
            Err(std::sync::TryLockError::Poisoned(error)) => {
                error.into_inner().socket.take();
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
    }
    fn run(
        &self,
        mut request: Option<pb::Frame>,
        new: Option<&Key>,
        expected: u32,
        mutation: bool,
    ) -> Result<pb::Frame> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Failure::new(3, "client closed"));
        }
        let mut state = match self.state.try_lock() {
            Ok(v) => v,
            Err(std::sync::TryLockError::WouldBlock) => {
                return Err(Failure::new(2, "client busy; not queued"));
            }
            Err(_) => return Err(Failure::new(6, "client state poisoned")),
        };
        if self.closed.load(Ordering::Acquire) || state.socket.is_none() {
            return Err(Failure::new(3, "client closed"));
        }
        state.sequence = state
            .sequence
            .checked_add(1)
            .ok_or_else(|| Failure::new(6, "request ID space exhausted"))?;
        let id = if let Some(r) = &mut request {
            let id = format!("c-{}", state.sequence);
            set_id(r, &id);
            id
        } else {
            "rotation-commit".to_owned()
        };
        let mut cancel = self.cancel.subscribe();
        let socket = state
            .socket
            .as_mut()
            .ok_or_else(|| Failure::new(3, "client closed"))?;
        let work = async {
            if let Some(new) = new {
                socket
                    .rotate(
                        &self.key.der,
                        &new.der,
                        &self.owner,
                        &self.welcome.client_id,
                    )
                    .await
            } else {
                socket
                    .exchange(request.as_ref().expect("request or rotation key"))
                    .await
            }
        };
        let result = self.runtime.block_on(async {
            tokio::select! {
                biased;
                _ = async {
                    if !*cancel.borrow() { let _ = cancel.changed().await; }
                } => Err(Failure { unknown: mutation, ..Failure::new(7, "operation cancelled; client closed") }),
                value = tokio::time::timeout(self.timeout, work) => match value {
                    Ok(Ok(frame)) => Ok(frame),
                    Ok(Err(error)) => Err(Failure::network(error, mutation)),
                    Err(_) => Err(Failure { unknown: mutation, ..Failure::new(8, "operation timed out; client closed") }),
                }
            }
        });
        let result = result.and_then(|r| {
            response::validate(&r, &id, expected, request.as_ref())
                .map_err(|e| Failure::transport(e, mutation))?;
            if let Some(pb::Body::Error(e)) = &r.body {
                return Err(Failure {
                    code: 5,
                    message: e.detail.clone(),
                    broker: e.code.clone(),
                    retry: e.retry_after_ms,
                    unknown: false,
                });
            }
            Ok(r)
        });
        if (new.is_some() && result.is_ok()) || result.as_ref().is_err_and(|e| e.code != 5) {
            state.socket.take();
            self.closed.store(true, Ordering::Release);
        }
        result
    }
}
fn set_id(frame: &mut pb::Frame, id: &str) {
    match frame.body.as_mut().expect("request body") {
        pb::Body::Publish(v) => v.id = id.into(),
        pb::Body::Poll(v) => v.id = id.into(),
        pb::Body::ListMessages(v) => v.id = id.into(),
        pb::Body::GetMessage(v) => v.id = id.into(),
        pb::Body::GetPermissions(v) => v.id = id.into(),
        pb::Body::GetCatalog(v) => v.id = id.into(),
        _ => unreachable!("internal request kind"),
    }
}
pub struct Response {
    frame: pb::Frame,
}
fn boundary(error: *mut abi::Error, f: impl FnOnce() -> Result<()>) -> i32 {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| {
        Err(Failure {
            unknown: true,
            ..Failure::new(6, "Rust panic contained at C boundary")
        })
    });
    let (code, value) = match result {
        Ok(()) => (0, abi::Error::default()),
        Err(e) => (e.code, abi::Error::failure(&e)),
    };
    if !error.is_null() {
        // SAFETY: optional caller-owned writable error storage.
        unsafe { error.write(value) };
    }
    code
}
fn getter<T: Default>(f: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_default()
}
unsafe fn request(
    client: *const Client,
    body: pb::Body,
    kind: u32,
    mutation: bool,
    out: *mut *mut Response,
) -> Result<()> {
    // SAFETY: common C input/output lifetime contract.
    unsafe { abi::output(out, std::ptr::null_mut()) }?;
    let c = unsafe { abi::reference(client) }?;
    let frame = c.run(
        Some(pb::Frame {
            version: 1,
            body: Some(body),
        }),
        None,
        kind,
        mutation,
    )?;
    unsafe { abi::output(out, Box::into_raw(Box::new(Response { frame }))) }
}
fn limit(n: u32) -> Result<u32> {
    if (1..=100).contains(&n) {
        Ok(n)
    } else {
        Err(Failure::invalid("limit must be 1..100"))
    }
}
fn cursor(s: &str) -> bool {
    s.len() == 8 && s.bytes().all(|b| b.is_ascii_alphanumeric())
}
unsafe fn expires(p: *const c_char) -> Result<Option<pb::Expires>> {
    // SAFETY: C string lifetime contract.
    let value = unsafe { abi::text(p, 64) }?;
    if let Some(s) = value {
        protocol::parse_time(s).map_err(Failure::invalid_display)?;
    }
    Ok(value.map(|s| pb::Expires { time: s.to_owned() }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_key_generate(out: *mut *mut Key, error: *mut abi::Error) -> i32 {
    boundary(error, || {
        // SAFETY: output contract.
        unsafe { abi::output(out, std::ptr::null_mut()) }?;
        let der = Zeroizing::new(client::crypto::generate_key().map_err(Failure::invalid_display)?);
        let k = Key::parse(&der)?;
        unsafe { abi::output(out, Box::into_raw(Box::new(k))) }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_key_parse(
    data: *const u8,
    len: usize,
    out: *mut *mut Key,
    error: *mut abi::Error,
) -> i32 {
    boundary(error, || {
        // SAFETY: common bounded input/output contract.
        unsafe { abi::output(out, std::ptr::null_mut()) }?;
        let key = Key::parse(unsafe { abi::data(data, len, 4096) }?)?;
        unsafe { abi::output(out, Box::into_raw(Box::new(key))) }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_key_pkcs8(key: *const Key) -> Bytes {
    getter(|| {
        if key.is_null() {
            Bytes::default()
        } else {
            // SAFETY: live handle contract.
            Bytes::new(unsafe { &(*key).der })
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_key_public(key: *const Key) -> Bytes {
    getter(|| {
        if key.is_null() {
            Bytes::default()
        } else {
            // SAFETY: live handle contract.
            Bytes::new(unsafe { &(*key).public })
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_key_token_id(key: *const Key) -> Str {
    getter(|| {
        if key.is_null() {
            Str::default()
        } else {
            // SAFETY: live handle contract.
            Str::new(unsafe { &(*key).token })
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_key_client_id(key: *const Key) -> Str {
    getter(|| {
        if key.is_null() {
            Str::default()
        } else {
            // SAFETY: live handle contract.
            Str::new(unsafe { &(*key).id })
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_key_free(key: *mut Key) {
    getter(|| {
        if !key.is_null() {
            // SAFETY: exactly once, no concurrent use.
            drop(unsafe { Box::from_raw(key) });
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_connect(
    config: *const abi::Config,
    out: *mut *mut Client,
    error: *mut abi::Error,
) -> i32 {
    boundary(error, || {
        // SAFETY: config starts with readable size; reject ABI mismatch before full dereference.
        unsafe { abi::output(out, std::ptr::null_mut()) }?;
        let size = unsafe { abi::reference(config.cast::<usize>()) }?;
        if *size != std::mem::size_of::<abi::Config>() {
            return Err(Failure::invalid("config ABI size mismatch"));
        }
        let config = *unsafe { abi::reference(config) }?;
        let o = unsafe { Options::parse(config) }?;
        let source = unsafe { abi::reference(config.key) }?;
        let key = Key::parse(&source.der)?;
        let rt = runtime()?;
        let (socket, welcome) = rt
            .block_on(async { tokio::time::timeout(o.timeout, Socket::connect(&o, &key)).await })
            .map_err(|_| Failure::new(8, "connection/authentication timed out"))?
            .map_err(|e| Failure::network(e, false))?;
        let (cancellation, _) = watch::channel(false);
        let c = Client {
            state: Mutex::new(State {
                socket: Some(socket),
                sequence: 0,
            }),
            closed: AtomicBool::new(false),
            cancel: cancellation,
            key,
            owner: o.owner,
            welcome,
            timeout: o.timeout,
            runtime: rt,
        };
        unsafe { abi::output(out, Box::into_raw(Box::new(c))) }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_client_info(
    client: *const Client,
    out: *mut abi::Info,
    error: *mut abi::Error,
) -> i32 {
    boundary(error, || {
        // SAFETY: live immutable client and writable output.
        let c = unsafe { abi::reference(client) }?;
        unsafe {
            abi::output(
                out,
                abi::Info {
                    owner: Str::new(&c.owner),
                    client_id: Str::new(&c.welcome.client_id),
                    restart_counter: c.welcome.restart_counter.unwrap_or(0),
                },
            )
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_client_close(client: *const Client) {
    getter(|| {
        if !client.is_null() {
            // SAFETY: live shared handle; all mutations synchronized.
            unsafe { &*client }.close();
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_client_free(client: *mut Client) {
    getter(|| {
        if !client.is_null() {
            // SAFETY: exclusively owned, no concurrent calls remain.
            let c = unsafe { Box::from_raw(client) };
            c.close();
            drop(c);
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_publish_bytes(
    client: *const Client,
    topic: *const c_char,
    data: *const u8,
    len: usize,
    content: *const c_char,
    expiry: *const c_char,
    out: *mut *mut Response,
    error: *mut abi::Error,
) -> i32 {
    boundary(error, || {
        // SAFETY: forwarded common C input/output contract.
        unsafe { abi::output(out, std::ptr::null_mut()) }?;
        let topic = unsafe { abi::required(topic, 256) }?.to_owned();
        let data = unsafe { abi::data(data, len, 4096) }?;
        let content = unsafe { abi::text(content, 128) }?.map(str::to_owned);
        let payload = payload::Payload::bytes(data.to_vec(), content);
        payload.validate().map_err(Failure::invalid_display)?;
        let body = pb::Body::Publish(pb::Publish {
            id: String::new(),
            topic,
            payload,
            expires: unsafe { expires(expiry) }?,
        });
        unsafe { request(client, body, 1, true, out) }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_publish_event(
    client: *const Client,
    topic: *const c_char,
    event: *const abi::Event,
    expiry: *const c_char,
    out: *mut *mut Response,
    error: *mut abi::Error,
) -> i32 {
    boundary(error, || {
        // SAFETY: all event fields are initialized, selected union members valid.
        unsafe { abi::output(out, std::ptr::null_mut()) }?;
        let topic = unsafe { abi::required(topic, 256) }?.to_owned();
        let event = unsafe { abi::from_event(*abi::reference(event)?) }?;
        let payload = payload::Payload::event(event);
        payload.validate().map_err(Failure::invalid_display)?;
        unsafe {
            request(
                client,
                pb::Body::Publish(pb::Publish {
                    id: String::new(),
                    topic,
                    payload,
                    expires: expires(expiry)?,
                }),
                1,
                true,
                out,
            )
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_poll(
    client: *const Client,
    topic: *const c_char,
    after: *const c_char,
    n: u32,
    out: *mut *mut Response,
    error: *mut abi::Error,
) -> i32 {
    boundary(error, || {
        // SAFETY: common C contract.
        unsafe { abi::output(out, std::ptr::null_mut()) }?;
        let topic = unsafe { abi::required(topic, 256) }?.to_owned();
        let after = unsafe { abi::text(after, 8) }?.map(str::to_owned);
        if after.as_ref().is_some_and(|v| !cursor(v)) {
            return Err(Failure::invalid("invalid poll cursor"));
        }
        unsafe {
            request(
                client,
                pb::Body::Poll(pb::Poll {
                    id: String::new(),
                    topic,
                    after,
                    limit: Some(limit(n)?),
                }),
                2,
                false,
                out,
            )
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_list_messages(
    client: *const Client,
    topic: *const c_char,
    cursor: *const c_char,
    n: u32,
    out: *mut *mut Response,
    error: *mut abi::Error,
) -> i32 {
    boundary(error, || {
        // SAFETY: common C contract.
        unsafe { abi::output(out, std::ptr::null_mut()) }?;
        let topic = unsafe { abi::text(topic, 256) }?.map(str::to_owned);
        let cursor = unsafe { abi::text(cursor, 4096) }?.map(str::to_owned);
        unsafe {
            request(
                client,
                pb::Body::ListMessages(pb::ListMessages {
                    id: String::new(),
                    topic,
                    cursor,
                    limit: Some(limit(n)?),
                }),
                3,
                false,
                out,
            )
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_get_message(
    client: *const Client,
    id: *const c_char,
    out: *mut *mut Response,
    error: *mut abi::Error,
) -> i32 {
    boundary(error, || {
        // SAFETY: common C contract.
        unsafe { abi::output(out, std::ptr::null_mut()) }?;
        let message_id = unsafe { abi::required(id, 256) }?.to_owned();
        unsafe {
            request(
                client,
                pb::Body::GetMessage(pb::GetMessage {
                    id: String::new(),
                    message_id,
                }),
                4,
                false,
                out,
            )
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_get_permissions(
    client: *const Client,
    topic: *const c_char,
    out: *mut *mut Response,
    error: *mut abi::Error,
) -> i32 {
    boundary(error, || {
        // SAFETY: common C contract.
        unsafe { abi::output(out, std::ptr::null_mut()) }?;
        let topic = unsafe { abi::required(topic, 256) }?.to_owned();
        unsafe {
            request(
                client,
                pb::Body::GetPermissions(pb::GetPermissions {
                    id: String::new(),
                    topic,
                }),
                5,
                false,
                out,
            )
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_get_catalog(
    client: *const Client,
    name: *const c_char,
    version: u32,
    out: *mut *mut Response,
    error: *mut abi::Error,
) -> i32 {
    boundary(error, || {
        // SAFETY: common C contract.
        unsafe { abi::output(out, std::ptr::null_mut()) }?;
        let name = unsafe { abi::required(name, 256) }?.to_owned();
        if version == 0 {
            return Err(Failure::invalid("invalid catalog version"));
        }
        unsafe {
            request(
                client,
                pb::Body::GetCatalog(pb::GetCatalog {
                    id: String::new(),
                    name,
                    catalog_version: version,
                }),
                6,
                false,
                out,
            )
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mynofy_rotate(
    client: *const Client,
    new: *const Key,
    out: *mut *mut Response,
    error: *mut abi::Error,
) -> i32 {
    boundary(error, || {
        // SAFETY: live immutable keys/client and writable output.
        unsafe { abi::output(out, std::ptr::null_mut()) }?;
        let c = unsafe { abi::reference(client) }?;
        let key = unsafe { abi::reference(new) }?;
        let frame = c.run(None, Some(key), 7, true)?;
        unsafe { abi::output(out, Box::into_raw(Box::new(Response { frame }))) }
    })
}

#[cfg(test)]
mod tests;
