//! Local administration uses its own Unix socket, not the message protocol or topic ACL.
use crate::{
    BoxError,
    config::{Action, Config},
    engine::Engine,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

pub const VERSION: u8 = 1;
pub const MAX_REQUEST_BYTES: usize = 16 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u8,
    pub command: Command,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    PeersList {
        after: Option<String>,
        limit: Option<u16>,
    },
    PeerSet {
        peer: crate::config::Peer,
    },
    PeerDelete {
        service_id: String,
    },
    PeerDisable {
        service_id: String,
        disabled: bool,
    },
    PeerCertificateRevoke {
        fingerprint: String,
    },
    PeerCertificatesList {
        after: Option<String>,
        limit: Option<u16>,
    },
    ClientsList {
        after: Option<u16>,
        limit: Option<u16>,
    },
    ClientGet {
        client: String,
    },
    ClientAdd {
        public_key: String,
        valid_until: Option<String>,
        display_name: Option<String>,
    },
    ClientDisable {
        client: String,
        disabled: bool,
    },
    KeyAdd {
        client: String,
        public_key: String,
        valid_until: Option<String>,
    },
    KeyRevoke {
        token: String,
    },
    AclGet {
        client: String,
        topic: String,
    },
    AclSet {
        client: String,
        topic: String,
        allow: Vec<Action>,
    },
    TopicsList {
        after: Option<String>,
        limit: Option<u16>,
    },
    TopicGet {
        topic: String,
    },
    TopicCreate {
        topic: String,
        policy: crate::config::Policy,
    },
    TopicPolicy {
        topic: String,
        policy: crate::config::Policy,
    },
    TopicDelete {
        topic: String,
    },
    TopicStorage {
        topic: String,
        storage: crate::config::TopicStorage,
    },
    ContentSave {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topics: Option<Vec<String>>,
    },
    Status {},
    Health {},
    ContentTopicStatus {
        topic: String,
    },
    StorageStats {},
    Export {},
}
/// SHA-256 of the complete leaf certificate DER, not its public key/SPKI.
pub fn certificate_fingerprint(der: &[u8]) -> String {
    format!(
        "sha256:{}",
        crate::protocol::crypto::encode(&crate::protocol::crypto::sha256(der))
    )
}
pub(crate) fn valid_certificate_fingerprint(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|raw| {
        crate::protocol::crypto::decode::<32>(raw)
            .is_ok_and(|bytes| crate::protocol::crypto::encode(&bytes) == raw)
    })
}
#[derive(Serialize)]
#[serde(untagged)]
pub enum ResultBody {
    Data(Value),
    Configuration(Box<Config>),
}
pub(crate) type AdminFuture = Pin<Box<dyn Future<Output = Result<(), BoxError>> + Send>>;
pub(crate) fn start(config: &Config, engine: Arc<Mutex<Engine>>) -> Result<AdminFuture, BoxError> {
    #[cfg(unix)]
    if let Some(path) = &config.service.admin_socket {
        let bound = crate::transport::bind_unix(path)?;
        return Ok(Box::pin(socket::serve(bound, engine)));
    }
    let _ = (config, engine);
    Ok(Box::pin(std::future::pending()))
}
#[cfg(unix)]
mod socket;
#[cfg(unix)]
pub use socket::request;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn documented_requests_cover_every_operation_and_roundtrip() {
        let examples: Vec<Value> =
            serde_json::from_str(include_str!("../../../protocol/admin/examples.json")).unwrap();
        let schema: Value =
            serde_json::from_str(include_str!("../../../protocol/admin/v1.schema.json")).unwrap();
        let mut operations = std::collections::HashSet::new();
        for example in examples
            .into_iter()
            .filter(|value| value.get("command").is_some())
        {
            let request: Request = serde_json::from_value(example).unwrap();
            assert_eq!(request.version, VERSION);
            let encoded = serde_json::to_value(request).unwrap();
            let op = encoded["command"]["op"].as_str().unwrap();
            assert_eq!(schema["$defs"][op]["properties"]["op"]["const"], op);
            operations.insert(op.to_owned());
            let _: Request = serde_json::from_value(encoded).unwrap();
        }
        let schema_operations: std::collections::HashSet<_> =
            schema["$defs"]["request"]["properties"]["command"]["oneOf"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| {
                    entry["$ref"]
                        .as_str()
                        .unwrap()
                        .strip_prefix("#/$defs/")
                        .unwrap()
                        .to_owned()
                })
                .collect();
        assert_eq!(operations, schema_operations);
    }
}
