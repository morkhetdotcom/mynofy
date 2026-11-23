//! Exact JSON collection sizes, without retaining another copy of security values.
use super::*;
use std::io::{self, Write};
#[derive(Default)]
struct Counter(usize);
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("Security size overflow"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn length(value: &impl Serialize) -> usize {
    let mut count = Counter::default();
    serde_json::to_writer(&mut count, value)
        .expect("Security values contain only JSON-compatible primitives");
    count.0
}
#[derive(Default)]
struct Sizes {
    entries: BTreeMap<String, usize>,
    payload: usize,
}
impl Sizes {
    fn size(&self) -> usize {
        2 + self.payload + self.entries.len().saturating_sub(1)
    }
    fn projected(&self, name: &str, entry: Option<usize>) -> usize {
        let old = self.entries.get(name).copied();
        let count = self.entries.len() - usize::from(old.is_some()) + usize::from(entry.is_some());
        2 + self.payload - old.unwrap_or(0) + entry.unwrap_or(0) + count.saturating_sub(1)
    }
    fn put(&mut self, name: &str, entry: Option<usize>) {
        if let Some(old) = self.entries.remove(name) {
            self.payload -= old;
        }
        if let Some(size) = entry {
            self.entries.insert(name.into(), size);
            self.payload += size;
        }
    }
    fn map_entry(name: &str, value_size: usize) -> usize {
        length(&name) + 1 + value_size
    }
    fn from_map<T: Serialize>(map: &BTreeMap<String, T>) -> Self {
        let mut sizes = Self::default();
        for (key, value) in map {
            sizes.put(key, Some(Self::map_entry(key, length(value))));
        }
        sizes
    }
    fn from_set(set: &BTreeSet<String>) -> Self {
        let mut sizes = Self::default();
        for key in set {
            sizes.put(key, Some(length(key)));
        }
        sizes
    }
}
fn optional(name: &str, value: Option<usize>) -> usize {
    value.map_or(0, |size| length(&name) + 2 + size) // field, colon and preceding comma
}
fn replace(total: usize, old: usize, new: usize) -> usize {
    total - old + new
}
pub(super) struct Budget {
    base: usize,
    envelope: usize,
    peers: Option<Sizes>,
    disabled: Option<Sizes>,
    revoked: Option<Sizes>,
    transient: Sizes,
    clients: Sizes,
    acl: Sizes,
    rules: BTreeMap<String, Sizes>,
}
impl Budget {
    pub(super) fn check_state(state: &State, owner: &str) -> Result<()> {
        if length(state) + length(&owner) + 128 > super::disk::MAX_SNAPSHOT {
            return Err(ApiError::new(
                "security_snapshot_too_large",
                "Security state exceeds the recoverable snapshot budget.",
            ));
        }
        Ok(())
    }
    pub(super) fn new(state: &State, owner: &str) -> Self {
        let mut budget = Self {
            base: 0,
            // Snapshot fields, sequence up to u64::MAX, and restart-counter growth.
            envelope: length(&owner) + 128,
            peers: state.peers.as_ref().map(Sizes::from_map),
            disabled: state.disabled_peers.as_ref().map(Sizes::from_set),
            revoked: state
                .revoked_peer_certificates
                .as_ref()
                .map(Sizes::from_set),
            transient: Sizes::from_set(&state.transient_topics),
            clients: Sizes::from_map(&state.clients),
            acl: Sizes::from_map(&state.acl),
            rules: state
                .acl
                .iter()
                .map(|(topic, rules)| (topic.clone(), Sizes::from_map(rules)))
                .collect(),
        };
        budget.base = length(state) - budget.dynamic();
        budget
    }
    fn transient_field(size: usize, empty: bool) -> usize {
        optional("transient_topics", (!empty).then_some(size))
    }
    fn dynamic(&self) -> usize {
        self.clients.size()
            + self.acl.size()
            + optional("peers", self.peers.as_ref().map(Sizes::size))
            + optional("disabled_peers", self.disabled.as_ref().map(Sizes::size))
            + optional(
                "revoked_peer_certificates",
                self.revoked.as_ref().map(Sizes::size),
            )
            + Self::transient_field(self.transient.size(), self.transient.entries.is_empty())
    }
    fn size(&self) -> usize {
        self.base + self.dynamic()
    }
    fn next_size(&self, change: &Change) -> usize {
        let size = self.size();
        let empty = Sizes::default();
        match change {
            Change::Peer { service_id, value } => {
                let map = self.peers.as_ref().unwrap_or(&empty);
                replace(
                    size,
                    optional("peers", self.peers.as_ref().map(Sizes::size)),
                    optional(
                        "peers",
                        Some(
                            map.projected(
                                service_id,
                                value
                                    .as_ref()
                                    .map(|value| Sizes::map_entry(service_id, length(value))),
                            ),
                        ),
                    ),
                )
            }
            Change::PeerDisabled {
                service_id,
                disabled,
            } => {
                let map = self.disabled.as_ref().unwrap_or(&empty);
                replace(
                    size,
                    optional("disabled_peers", self.disabled.as_ref().map(Sizes::size)),
                    optional(
                        "disabled_peers",
                        Some(map.projected(service_id, disabled.then(|| length(service_id)))),
                    ),
                )
            }
            Change::PeerCertificateRevoked { fingerprint } => {
                let map = self.revoked.as_ref().unwrap_or(&empty);
                replace(
                    size,
                    optional(
                        "revoked_peer_certificates",
                        self.revoked.as_ref().map(Sizes::size),
                    ),
                    optional(
                        "revoked_peer_certificates",
                        Some(map.projected(fingerprint, Some(length(fingerprint)))),
                    ),
                )
            }
            Change::Client { value } => replace(
                size,
                self.clients.size(),
                self.clients.projected(
                    &value.client_id,
                    Some(Sizes::map_entry(&value.client_id, length(value))),
                ),
            ),
            Change::TopicReset { topic, transient } => {
                let next_set = self
                    .transient
                    .projected(topic, transient.then(|| length(topic)));
                let count = self.transient.entries.len()
                    - usize::from(self.transient.entries.contains_key(topic))
                    + usize::from(*transient);
                replace(
                    replace(size, self.acl.size(), self.acl.projected(topic, None)),
                    Self::transient_field(self.transient.size(), self.transient.entries.is_empty()),
                    Self::transient_field(next_set, count == 0),
                )
            }
            Change::Acl {
                client,
                topic,
                allow,
            } => {
                let row = self.rules.get(topic).unwrap_or(&empty);
                let next = row.projected(
                    client,
                    (!allow.is_empty()).then(|| Sizes::map_entry(client, length(allow))),
                );
                replace(
                    size,
                    self.acl.size(),
                    self.acl
                        .projected(topic, Some(Sizes::map_entry(topic, next))),
                )
            }
        }
    }
    pub(super) fn check(&self, change: &Change) -> Result<()> {
        if self.next_size(change) + self.envelope > super::disk::MAX_SNAPSHOT {
            return Err(ApiError::new(
                "security_snapshot_too_large",
                "Security change exceeds the recoverable snapshot budget.",
            ));
        }
        Ok(())
    }
    pub(super) fn apply(&mut self, change: &Change) {
        match change {
            Change::Peer { service_id, value } => self.peers.get_or_insert_default().put(
                service_id,
                value
                    .as_ref()
                    .map(|value| Sizes::map_entry(service_id, length(value))),
            ),
            Change::PeerDisabled {
                service_id,
                disabled,
            } => self
                .disabled
                .get_or_insert_default()
                .put(service_id, disabled.then(|| length(service_id))),
            Change::PeerCertificateRevoked { fingerprint } => self
                .revoked
                .get_or_insert_default()
                .put(fingerprint, Some(length(fingerprint))),
            Change::Client { value } => self.clients.put(
                &value.client_id,
                Some(Sizes::map_entry(&value.client_id, length(value))),
            ),
            Change::TopicReset { topic, transient } => {
                self.acl.put(topic, None);
                self.rules.remove(topic);
                self.transient.put(topic, transient.then(|| length(topic)));
            }
            Change::Acl {
                client,
                topic,
                allow,
            } => {
                let row = self.rules.entry(topic.clone()).or_default();
                row.put(
                    client,
                    (!allow.is_empty()).then(|| Sizes::map_entry(client, length(allow))),
                );
                self.acl
                    .put(topic, Some(Sizes::map_entry(topic, row.size())));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collection_accounting_matches_serialization_after_mixed_updates() {
        let mut state = State::default();
        let mut budget = Budget::new(&state, "owner");
        let actions = [
            Change::Peer {
                service_id: "peer\"ü".into(),
                value: Some(crate::config::Peer {
                    service_id: "peer\"ü".into(),
                    certificate_identity: None,
                    topics: vec!["topic\nü".into()],
                }),
            },
            Change::PeerDisabled {
                service_id: "peer\"ü".into(),
                disabled: true,
            },
            Change::PeerDisabled {
                service_id: "peer\"ü".into(),
                disabled: false,
            },
            Change::PeerCertificateRevoked {
                fingerprint: "sha256:abc".into(),
            },
            Change::PeerCertificateRevoked {
                fingerprint: "sha256:abc".into(),
            },
            Change::Acl {
                client: "client\"1".into(),
                topic: "topic\nü".into(),
                allow: vec![Action::Publish],
            },
            Change::Acl {
                client: "client\"2".into(),
                topic: "topic\nü".into(),
                allow: vec![Action::Subscribe, Action::InspectOwn],
            },
            Change::Acl {
                client: "client\"1".into(),
                topic: "topic\nü".into(),
                allow: vec![],
            },
            Change::TopicReset {
                topic: "topic\nü".into(),
                transient: true,
            },
            Change::TopicReset {
                topic: "topic\nü".into(),
                transient: false,
            },
            Change::Client {
                value: Client {
                    client_id: "client:abc".into(),
                    display_name: Some("name\n\"ü".into()),
                    disabled: false,
                    keys: vec![],
                },
            },
            Change::Client {
                value: Client {
                    client_id: "client:abc".into(),
                    display_name: None,
                    disabled: true,
                    keys: vec![],
                },
            },
            Change::Peer {
                service_id: "peer\"ü".into(),
                value: None,
            },
        ];
        for change in actions.iter().cycle().take(130) {
            budget.check(change).unwrap();
            let predicted = budget.next_size(change);
            state.apply(change);
            budget.apply(change);
            assert_eq!(predicted, length(&state));
            assert_eq!(budget.size(), length(&state));
        }
    }
    #[test]
    fn growth_rejection_is_pure_and_shrinking_reclaims_capacity() {
        let state = State {
            disabled_peers: Some(["existing".into()].into_iter().collect()),
            ..State::default()
        };
        let mut budget = Budget::new(&state, "owner");
        budget.envelope = super::super::disk::MAX_SNAPSHOT - budget.size();
        let growth = Change::PeerCertificateRevoked {
            fingerprint: "new".into(),
        };
        let original = budget.size();
        assert_eq!(
            budget.check(&growth).unwrap_err().code,
            "security_snapshot_too_large"
        );
        assert_eq!(budget.size(), original);
        let shrink = Change::PeerDisabled {
            service_id: "existing".into(),
            disabled: false,
        };
        budget.check(&shrink).unwrap();
        budget.apply(&shrink);
        assert!(budget.size() < original);
    }
}
