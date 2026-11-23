use crate::{
    BoxError, crypto,
    protocol::{self, Result},
};
use serde::{Deserialize, Serialize};
use std::io::Read;
#[path = "config_format.rs"]
mod format;
use std::{
    collections::{BTreeMap, HashSet},
    net::SocketAddr,
    path::{Path, PathBuf},
};

pub(crate) const MAX_MESSAGE_TTL_SECONDS: u64 = 31_536_000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Publish,
    Subscribe,
    InspectOwn,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_storage: Option<ContentStorage>,
    pub schema_version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// None permits a purely in-memory embedded core. Config::load fills a default path.
    pub security_storage: Option<SecurityStorage>,
    pub service: Service,
    pub clients: Vec<Client>,
    #[serde(default)]
    pub limits: Limits,
    pub rotation: Rotation,
    pub routing: Routing,
    pub topics: Vec<Topic>,
    pub upstream: Option<Upstream>,
    #[serde(default)]
    pub accepted_peers: Vec<Peer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_peers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revoked_peer_certificates: Vec<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContentStorage {
    pub directory: PathBuf,
    #[serde(default)]
    pub encryption: ContentEncryption,
    #[serde(default = "content_max_bytes")]
    pub max_snapshot_bytes: u64,
}
fn content_max_bytes() -> u64 {
    128 * 1024 * 1024
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentEncryption {
    #[default]
    None,
    PasswordStdin,
    KeyStdin,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityStorage {
    /// Confirm security mutations only after journal synchronization (default true).
    #[serde(default = "security_sync")]
    pub sync: bool,
    pub directory: PathBuf,
    #[serde(default = "security_queue")]
    pub queue_capacity: usize,
    #[serde(default = "security_compaction")]
    pub compact_after_records: u64,
}
fn security_sync() -> bool {
    true
}
fn security_queue() -> usize {
    128
}
fn security_compaction() -> u64 {
    4096
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub id: String,
    pub listen: Option<SocketAddr>,
    pub unix_socket: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_socket: Option<PathBuf>,
    pub tls: Option<Tls>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Tls {
    pub certificate_file: PathBuf,
    pub private_key_file: PathBuf,
    pub peer_ca_file: Option<PathBuf>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Client {
    pub client_id: String,
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,
    pub keys: Vec<PublicKey>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicKey {
    pub algorithm: String,
    pub public_key: String,
    pub valid_until: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Topic {
    pub name: String,
    pub owner_id: String,
    pub policy: Option<Policy>,
    #[serde(default)]
    pub storage: TopicStorage,
    pub acl: Option<Vec<Rule>>,
    pub via: Option<String>,
}
/// Owner-local content policy. Secrets remain service-wide and startup-only.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum TopicStorage {
    Ram {},
    Manual {
        #[serde(default)]
        encrypted: bool,
    },
    Interval {
        seconds: u64,
        #[serde(default)]
        encrypted: bool,
    },
}
impl Default for TopicStorage {
    fn default() -> Self {
        Self::Ram {}
    }
}
impl TopicStorage {
    pub fn persistent(&self) -> bool {
        !matches!(self, Self::Ram {})
    }
    pub fn encrypted(&self) -> bool {
        matches!(
            self,
            Self::Manual { encrypted: true }
                | Self::Interval {
                    encrypted: true,
                    ..
                }
        )
    }
    pub fn interval(&self) -> Option<u64> {
        match self {
            Self::Interval { seconds, .. } => Some(*seconds),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub default_ttl_seconds: u64,
    pub max_ttl_seconds: u64,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub client_id: String,
    pub allow: Vec<Action>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rotation {
    pub timeout_seconds: u64,
    pub attempts_per_client: u64,
    pub window_seconds: u64,
    #[serde(default = "key_validity")]
    pub key_validity_seconds: u64,
}
fn key_validity() -> u64 {
    30 * 86_400
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Routing {
    pub hop_limit: u8,
    pub learned_route_ttl_seconds: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Peer {
    pub service_id: String,
    #[serde(default)]
    pub certificate_identity: Option<String>,
    pub topics: Vec<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Upstream {
    pub service_id: String,
    pub url: String,
    pub trust: Trust,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Trust {
    pub mode: String,
    pub ca_file: PathBuf,
    pub expected_certificate_identity: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub client_requests_per_second: u64,
    pub client_burst: u64,
    pub source_auth_requests_per_second: u64,
    pub source_auth_burst: u64,
    pub max_connections: usize,
    pub max_service_channels: usize,
    pub max_parallel_requests_per_client: usize,
    pub max_active_message_bytes: usize,
    pub max_content_working_bytes: usize,
    pub max_active_messages: usize,
    pub poll_page_items: usize,
    pub poll_page_max_items: usize,
    pub max_payload_bytes: usize,
    pub max_response_bytes: usize,
    pub auth_timeout_seconds: u64,
    pub request_timeout_seconds: u64,
    pub idle_timeout_seconds: u64,
    pub max_connections_per_source: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            client_requests_per_second: 100,
            client_burst: 200,
            source_auth_requests_per_second: 10,
            source_auth_burst: 20,
            max_connections: 1000,
            max_service_channels: 64,
            max_parallel_requests_per_client: 8,
            max_active_message_bytes: 16 * 1024 * 1024,
            max_content_working_bytes: 256 * 1024 * 1024,
            max_active_messages: 65_536,
            poll_page_items: 20,
            poll_page_max_items: 100,
            max_payload_bytes: 4096,
            max_response_bytes: 131_072,
            auth_timeout_seconds: 5,
            request_timeout_seconds: 10,
            idle_timeout_seconds: 60,
            max_connections_per_source: 32,
        }
    }
}
fn is_false(value: &bool) -> bool {
    !*value
}
/// File extension selection is explicit; unknown/no extensions retain the existing JSON behavior.
pub fn is_yaml(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml")
        })
}
impl Config {
    /// Parse either YAML (.yaml/.yml) or strict JSON, without resolving paths or opening storage.
    pub fn parse(path: &Path, raw: &str) -> std::result::Result<Self, BoxError> {
        if raw.len() > format::MAX_BYTES {
            return Err("Configuration exceeds 32 MiB".into());
        }
        let value = if is_yaml(path) {
            format::parse_yaml(raw)?
        } else {
            protocol::strict_json(raw)?
        };
        let config: Self = serde_json::from_value(value)?;
        config.validate().map_err(std::io::Error::other)?;
        Ok(config)
    }
    /// Emit the configuration subset of YAML; strings are always quoted to retain their types.
    pub fn to_yaml(&self) -> std::result::Result<String, BoxError> {
        format::to_yaml(&serde_json::to_value(self)?)
    }
    pub fn load(path: &Path) -> std::result::Result<Self, BoxError> {
        let mut raw = String::new();
        std::fs::File::open(path)?
            .take((format::MAX_BYTES + 1) as u64)
            .read_to_string(&mut raw)?;
        let mut cfg = Self::parse(path, &raw)?;
        let parent = std::fs::canonicalize(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new(".")),
        )?;
        if cfg.security_storage.is_none() {
            let mut directory = path
                .file_name()
                .ok_or("Missing configuration filename")?
                .to_os_string();
            directory.push(".security");
            cfg.security_storage = Some(SecurityStorage {
                sync: security_sync(),
                directory: parent.join(directory),
                queue_capacity: security_queue(),
                compact_after_records: security_compaction(),
            });
        }
        if let Some(storage) = &mut cfg.content_storage
            && storage.directory.is_relative()
        {
            storage.directory = parent.join(&storage.directory);
        }
        if let Some(storage) = &mut cfg.security_storage
            && storage.directory.is_relative()
        {
            storage.directory = parent.join(&storage.directory);
        }
        if let Some(tls) = &mut cfg.service.tls {
            for file in [&mut tls.certificate_file, &mut tls.private_key_file] {
                if file.is_relative() {
                    *file = parent.join(&*file);
                }
            }
            if let Some(file) = &mut tls.peer_ca_file
                && file.is_relative()
            {
                *file = parent.join(&*file);
            }
        }
        if let Some(file) = &mut cfg.service.unix_socket
            && file.is_relative()
        {
            *file = parent.join(&*file);
        }
        if let Some(file) = &mut cfg.service.admin_socket
            && file.is_relative()
        {
            *file = parent.join(&*file);
        }
        if let Some(u) = &mut cfg.upstream
            && u.trust.ca_file.is_relative()
        {
            u.trust.ca_file = parent.join(&u.trust.ca_file);
        }
        cfg.validate().map_err(std::io::Error::other)?;
        Ok(cfg)
    }
    pub fn validate(&self) -> std::result::Result<(), String> {
        let fail = |s: &str| Err(s.to_owned());
        if let Some(storage) = &self.content_storage {
            #[cfg(not(unix))]
            return fail("Content storage currently requires Unix");
            if storage.directory.as_os_str().is_empty()
                || !(65536..=512 * 1024 * 1024).contains(&storage.max_snapshot_bytes)
                || self
                    .security_storage
                    .as_ref()
                    .is_some_and(|security| security.directory == storage.directory)
            {
                return fail(
                    "Invalid content storage directory, interval, maximum size or shared security directory",
                );
            }
        }

        if let Some(storage) = &self.security_storage {
            #[cfg(not(unix))]
            return fail("Security journal currently requires Unix");
            if storage.directory.as_os_str().is_empty()
                || !(1..=4096).contains(&storage.queue_capacity)
                || !(1..=1_000_000).contains(&storage.compact_after_records)
            {
                return fail(
                    "Invalid security storage path, queue capacity or compaction interval",
                );
            }
        }

        if self.schema_version != 1 || self.service.id.is_empty() || self.service.id.len() > 256 {
            return fail("Invalid configuration version or service ID");
        }
        match (
            &self.service.listen,
            &self.service.unix_socket,
            &self.service.tls,
        ) {
            (Some(_), None, Some(_)) => {}
            (None, Some(path), _) if !path.as_os_str().is_empty() && path.file_name().is_some() => {
                #[cfg(not(unix))]
                return fail("Unix sockets are only supported on Unix platforms");
            }
            _ => return fail("Choose either listen with tls, or unix_socket without tls"),
        }
        if let Some(path) = &self.service.admin_socket {
            #[cfg(not(unix))]
            {
                let _ = path;
                return fail("Admin sockets are only supported on Unix platforms");
            }
            #[cfg(unix)]
            if path.as_os_str().is_empty()
                || path.file_name().is_none()
                || self.service.unix_socket.as_ref() == Some(path)
            {
                return fail("Admin socket needs a distinct valid path");
            }
        }
        if self.clients.len() > 10_000 || self.topics.len() > 10_000 {
            return fail("Too many configured clients/topics");
        }
        let mut ids = HashSet::new();
        let mut keys = HashSet::new();
        for c in &self.clients {
            if !crypto::valid_client_id(&c.client_id)
                || !ids.insert(&c.client_id)
                || c.keys.len() > 8
                || c.display_name.as_ref().is_some_and(|name| name.len() > 256)
            {
                return fail("Invalid/duplicate client identity or key count");
            }
            for k in &c.keys {
                if k.algorithm != "ed25519"
                    || crypto::decode::<32>(&k.public_key).is_err()
                    || protocol::parse_time(&k.valid_until).is_err()
                    || !keys.insert(&k.public_key)
                {
                    return fail("Invalid or duplicate public key");
                }
            }
        }
        let mut topics = HashSet::new();
        for t in &self.topics {
            if t.name.is_empty() || t.name.len() > 256 || !topics.insert(&t.name) {
                return fail("Invalid/duplicate topic");
            }
            if t.storage
                .interval()
                .is_some_and(|value| !(1..=2_592_000).contains(&value))
                || t.storage.persistent()
                    && (self.content_storage.is_none() || t.owner_id != self.service.id)
                || t.storage.encrypted()
                    && self
                        .content_storage
                        .as_ref()
                        .is_none_or(|store| store.encryption == ContentEncryption::None)
            {
                return fail(
                    "Topic storage requires a local owner, content directory, valid interval and encryption secret mode",
                );
            }
            if t.owner_id == self.service.id {
                if t.via.is_some() {
                    return fail("Local topic cannot have a remote next hop");
                }
                let p = t.policy.as_ref().ok_or("Missing local topic policy")?;
                if p.default_ttl_seconds == 0
                    || p.default_ttl_seconds > p.max_ttl_seconds
                    || p.max_ttl_seconds > MAX_MESSAGE_TTL_SECONDS
                {
                    return fail("Invalid topic TTL");
                }
                let mut rules = HashSet::new();
                for a in t.acl.as_ref().ok_or("Missing owner ACL")? {
                    if !ids.contains(&a.client_id)
                        || !rules.insert(&a.client_id)
                        || a.allow.iter().collect::<HashSet<_>>().len() != a.allow.len()
                    {
                        return fail("Unknown client or duplicate ACL rule/action");
                    }
                }
            } else if t.acl.is_some()
                || t.policy.is_some()
                || self
                    .upstream
                    .as_ref()
                    .is_none_or(|u| Some(&u.service_id) != t.via.as_ref())
            {
                return fail(
                    "Remote topic requires an upstream and must not carry a local ACL/policy",
                );
            }
        }
        if self.upstream.as_ref().is_some_and(|u| {
            u.service_id.is_empty()
                || u.service_id.len() > 256
                || u.service_id == self.service.id
                || !u.url.starts_with("wss://")
                || u.trust.mode != "ca"
                || tokio_rustls::rustls::pki_types::ServerName::try_from(
                    u.trust.expected_certificate_identity.as_str(),
                )
                .is_err()
        }) {
            return fail("Invalid upstream");
        }
        if self.accepted_peers.len() > 10_000 {
            return fail("Too many accepted peers");
        }
        if self.disabled_peers.len() > 10_000
            || self
                .disabled_peers
                .iter()
                .any(|id| id.is_empty() || id.len() > 256 || id == &self.service.id)
            || self.disabled_peers.iter().collect::<HashSet<_>>().len() != self.disabled_peers.len()
            || self.revoked_peer_certificates.len() > 10_000
            || self
                .revoked_peer_certificates
                .iter()
                .any(|value| !crate::admin::valid_certificate_fingerprint(value))
            || self
                .revoked_peer_certificates
                .iter()
                .collect::<HashSet<_>>()
                .len()
                != self.revoked_peer_certificates.len()
        {
            return fail("Invalid peer restrictions");
        }
        let mut peers = HashSet::new();
        let mut identities = HashSet::new();
        for peer in &self.accepted_peers {
            if peer.service_id.is_empty()
                || peer.service_id.len() > 256
                || peer.service_id == self.service.id
                || !peers.insert(&peer.service_id)
                || !identities.insert(
                    peer.certificate_identity
                        .as_ref()
                        .unwrap_or(&peer.service_id),
                )
                || tokio_rustls::rustls::pki_types::ServerName::try_from(
                    peer.certificate_identity
                        .as_ref()
                        .unwrap_or(&peer.service_id)
                        .as_str(),
                )
                .is_err()
                || peer.topics.is_empty()
                || peer.topics.len() > 10000
                || peer
                    .certificate_identity
                    .as_ref()
                    .is_some_and(|s| s.is_empty())
                || peer
                    .topics
                    .iter()
                    .any(|name| !self.topics.iter().any(|t| t.name == *name))
                || peer.topics.iter().collect::<HashSet<_>>().len() != peer.topics.len()
            {
                return fail("Invalid accepted peer identity or topic allowlist");
            }
        }
        if (self.upstream.is_some() || !self.accepted_peers.is_empty())
            && self.service.tls.is_none()
        {
            return fail("Service links require local TLS credentials");
        }
        if !self.accepted_peers.is_empty()
            && (self.service.listen.is_none()
                || self
                    .service
                    .tls
                    .as_ref()
                    .is_none_or(|t| t.peer_ca_file.is_none()))
        {
            return fail("Accepted peers require TCP/TLS and peer_ca_file");
        }
        let l = &self.limits;
        if l.max_payload_bytes > 4096 {
            return Err(
                "limits.max_payload_bytes must be at most 4096; migrate older configurations"
                    .into(),
            );
        }
        if l.client_requests_per_second == 0
            || l.client_requests_per_second > 1_000_000
            || l.client_burst == 0
            || l.client_burst > 1_000_000
            || l.source_auth_requests_per_second == 0
            || l.source_auth_requests_per_second > 1_000_000
            || l.source_auth_burst == 0
            || l.source_auth_burst > 1_000_000
            || l.max_connections == 0
            || l.max_connections > 100_000
            || !(1..=4096).contains(&l.max_service_channels)
            || l.max_connections_per_source == 0
            || l.max_connections_per_source > 100_000
            || l.max_parallel_requests_per_client == 0
            || l.max_parallel_requests_per_client > 1000
            || l.max_active_messages == 0
            || l.max_payload_bytes == 0
            || l.max_payload_bytes > 4096
            || l.max_response_bytes < l.max_payload_bytes.saturating_add(8192)
            || l.max_response_bytes > 4 * 1024 * 1024
            || l.max_active_message_bytes < l.max_payload_bytes
            || l.poll_page_items == 0
            || l.poll_page_items > l.poll_page_max_items
            || l.poll_page_max_items > 100
        {
            return fail("Invalid capacity or rate limit");
        }
        for seconds in [
            l.auth_timeout_seconds,
            l.request_timeout_seconds,
            l.idle_timeout_seconds,
            self.rotation.timeout_seconds,
            self.rotation.window_seconds,
            self.routing.learned_route_ttl_seconds,
        ] {
            if seconds == 0 || seconds > 86_400 {
                return fail("Invalid timeout");
            }
        }
        if self.rotation.key_validity_seconds == 0
            || self.rotation.key_validity_seconds > 31_536_000
            || self.rotation.attempts_per_client == 0
            || self.rotation.attempts_per_client > 1000
            || !(1..=16).contains(&self.routing.hop_limit)
        {
            return fail("Invalid rotation/routing limit");
        }
        Ok(())
    }
    /// Validate that this configuration can provide local ownership and optional CA peer links.
    pub fn require_local_owner(&self) -> std::result::Result<(), BoxError> {
        self.validate().map_err(Into::into)
    }
    pub fn acl(&self) -> BTreeMap<(String, String), Vec<Action>> {
        let mut acl = BTreeMap::new();
        for t in &self.topics {
            for a in t.acl.iter().flatten() {
                acl.insert((a.client_id.clone(), t.name.clone()), a.allow.clone());
            }
        }
        acl
    }
}
pub fn read_public(key: &PublicKey) -> Result<([u8; 32], u64)> {
    Ok((
        crypto::decode(&key.public_key)?,
        protocol::parse_time(&key.valid_until)?,
    ))
}

#[cfg(test)]
mod security_tests {
    #[test]
    fn security_sync_defaults_to_durable_and_preserves_explicit_async() {
        let mut storage: super::SecurityStorage =
            serde_json::from_str(r#"{"directory":"security"}"#).unwrap();
        assert!(storage.sync);
        storage.sync = false;
        let encoded = serde_json::to_string(&storage).unwrap();
        assert!(
            !serde_json::from_str::<super::SecurityStorage>(&encoded)
                .unwrap()
                .sync
        );
        assert!(
            serde_json::from_str::<super::SecurityStorage>(
                r#"{"directory":"security","sync":"true"}"#
            )
            .is_err()
        );
    }
    use super::*;
    #[test]
    fn security_storage_limits_are_checked_without_touching_disk() {
        let baseline: Config =
            serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
        for (capacity, compact) in [(0, 1), (4097, 1), (1, 0), (1, 1_000_001)] {
            let mut config = baseline.clone();
            config.security_storage = Some(SecurityStorage {
                sync: false,
                directory: "security".into(),
                queue_capacity: capacity,
                compact_after_records: compact,
            });
            assert!(config.validate().is_err());
        }
    }
}
