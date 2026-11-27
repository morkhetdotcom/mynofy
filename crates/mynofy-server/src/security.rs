//! Independent security state, ordered bounded asynchronous journal, and recovery.
use crate::{
    BoxError,
    config::{Action, Client, Config, Rule},
    protocol::{ApiError, Result},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread::JoinHandle,
};

#[cfg(unix)]
mod budget;
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Change {
    Peer {
        service_id: String,
        value: Option<crate::config::Peer>,
    },
    PeerDisabled {
        service_id: String,
        disabled: bool,
    },
    PeerCertificateRevoked {
        fingerprint: String,
    },
    TopicReset {
        topic: String,
        transient: bool,
    },
    Client {
        value: Client,
    },
    Acl {
        client: String,
        topic: String,
        allow: Vec<Action>,
    },
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    peers: Option<BTreeMap<String, crate::config::Peer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    disabled_peers: Option<BTreeSet<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revoked_peer_certificates: Option<BTreeSet<String>>,
    #[serde(default)]
    restart_counter: u32,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    transient_topics: BTreeSet<String>,
    clients: BTreeMap<String, Client>,
    acl: BTreeMap<String, BTreeMap<String, Vec<Action>>>,
}
impl State {
    fn from_config(config: &Config) -> Self {
        Self {
            peers: Some(
                config
                    .accepted_peers
                    .iter()
                    .map(|peer| (peer.service_id.clone(), peer.clone()))
                    .collect(),
            ),
            disabled_peers: Some(config.disabled_peers.iter().cloned().collect()),
            revoked_peer_certificates: Some(
                config.revoked_peer_certificates.iter().cloned().collect(),
            ),
            restart_counter: 0,
            transient_topics: BTreeSet::new(),
            clients: config
                .clients
                .iter()
                .map(|client| (client.client_id.clone(), client.clone()))
                .collect(),
            acl: config
                .topics
                .iter()
                .filter(|topic| topic.owner_id == config.service.id)
                .map(|topic| {
                    (
                        topic.name.clone(),
                        topic
                            .acl
                            .iter()
                            .flatten()
                            .map(|rule| (rule.client_id.clone(), rule.allow.clone()))
                            .collect(),
                    )
                })
                .collect(),
        }
    }
    fn apply(&mut self, change: &Change) {
        match change {
            Change::Peer { service_id, value } => {
                let peers = self.peers.get_or_insert_default();
                if let Some(value) = value {
                    peers.insert(service_id.clone(), value.clone());
                } else {
                    peers.remove(service_id);
                }
            }
            Change::PeerDisabled {
                service_id,
                disabled,
            } => {
                let peers = self.disabled_peers.get_or_insert_default();
                if *disabled {
                    peers.insert(service_id.clone());
                } else {
                    peers.remove(service_id);
                }
            }
            Change::PeerCertificateRevoked { fingerprint } => {
                self.revoked_peer_certificates
                    .get_or_insert_default()
                    .insert(fingerprint.clone());
            }
            Change::TopicReset { topic, transient } => {
                self.acl.remove(topic);
                if *transient {
                    self.transient_topics.insert(topic.clone());
                } else {
                    self.transient_topics.remove(topic);
                }
            }
            Change::Client { value } => {
                self.clients.insert(value.client_id.clone(), value.clone());
            }
            Change::Acl {
                client,
                topic,
                allow,
            } => {
                let rules = self.acl.entry(topic.clone()).or_default();
                if allow.is_empty() {
                    rules.remove(client);
                } else {
                    rules.insert(client.clone(), allow.clone());
                }
            }
        }
    }
    fn restore(&mut self, config: &mut Config) -> std::result::Result<(), BoxError> {
        if self
            .clients
            .iter()
            .any(|(id, client)| id != &client.client_id)
        {
            return Err("Security snapshot client identity mismatch".into());
        }
        let local: BTreeSet<_> = config
            .topics
            .iter()
            .filter(|t| t.owner_id == config.service.id)
            .map(|t| t.name.clone())
            .collect();
        for name in &self.transient_topics {
            if !local.contains(name) {
                self.acl.remove(name);
                if let Some(peers) = &mut self.peers {
                    for peer in peers.values_mut() {
                        peer.topics.retain(|topic| topic != name);
                    }
                    peers.retain(|_, peer| !peer.topics.is_empty());
                }
            }
        }
        self.transient_topics.retain(|name| local.contains(name));
        let peers = self.peers.get_or_insert_with(|| {
            config
                .accepted_peers
                .iter()
                .map(|peer| (peer.service_id.clone(), peer.clone()))
                .collect()
        });
        if peers.iter().any(|(id, peer)| id != &peer.service_id) {
            return Err("Security snapshot peer identity mismatch".into());
        }
        config.accepted_peers = peers.values().cloned().collect();
        config.disabled_peers = self
            .disabled_peers
            .get_or_insert_with(|| config.disabled_peers.iter().cloned().collect())
            .iter()
            .cloned()
            .collect();
        config.revoked_peer_certificates = self
            .revoked_peer_certificates
            .get_or_insert_with(|| config.revoked_peer_certificates.iter().cloned().collect())
            .iter()
            .cloned()
            .collect();
        config.clients = self.clients.values().cloned().collect();
        for (name, rules) in &self.acl {
            if !rules.is_empty()
                && !config
                    .topics
                    .iter()
                    .any(|topic| topic.name == *name && topic.owner_id == config.service.id)
            {
                return Err("Saved security ACL references an absent local topic".into());
            }
        }
        for topic in &mut config.topics {
            if topic.owner_id == config.service.id {
                topic.acl = Some(
                    self.acl
                        .get(&topic.name)
                        .into_iter()
                        .flat_map(|rules| rules.iter())
                        .map(|(client, allow)| Rule {
                            client_id: client.clone(),
                            allow: allow.clone(),
                        })
                        .collect(),
                );
            }
        }
        config.validate().map_err(Into::into)
    }
}
#[derive(Default, Serialize, Clone)]
pub(crate) struct Status {
    pub enabled: bool,
    pub sync: bool,
    pub submitted_sequence: u64,
    pub durable_sequence: u64,
    pub snapshot_sequence: u64,
    pub pending_records: usize,
    pub last_error: Option<String>,
}
#[cfg(all(test, unix))]
pub(crate) type WriteGate = (Mutex<bool>, Condvar);
struct Shared {
    progress: Condvar,
    status: Mutex<Status>,
    pending: AtomicUsize,
    stopping: AtomicBool,
    alive: AtomicBool,
    #[cfg(all(test, unix))]
    write_gate: Mutex<Option<Arc<WriteGate>>>,
}
impl Shared {
    fn status(&self) -> std::sync::MutexGuard<'_, Status> {
        self.status
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}
// Sequence-bound confirmation metadata: no per-message state or disk-format change.
pub(crate) struct DurabilityFence {
    shared: Arc<Shared>,
    sequence: u64,
}
impl DurabilityFence {
    pub(crate) async fn confirm(self) -> std::result::Result<(), BoxError> {
        // Never block the runtime or retain the Engine mutex while waiting for fsync.
        tokio::task::spawn_blocking(move || self.wait()).await?
    }
    pub(crate) fn wait(self) -> std::result::Result<(), BoxError> {
        self.wait_for(std::time::Duration::from_secs(5))
    }
    fn wait_for(self, timeout: std::time::Duration) -> std::result::Result<(), BoxError> {
        let deadline = std::time::Instant::now() + timeout;
        let mut status = self.shared.status();
        loop {
            // A later compaction error cannot undo an already synchronized sequence.
            if status.durable_sequence >= self.sequence {
                return Ok(());
            }
            if let Some(error) = &status.last_error {
                return Err(format!(
                    "Security durability fence {} failed: {error}",
                    self.sequence
                )
                .into());
            }
            if !self.shared.alive.load(Ordering::Acquire)
                || self.shared.stopping.load(Ordering::Acquire)
            {
                return Err(
                    "Security writer stopped before the requested sequence was durable".into(),
                );
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(
                    format!("Security durability fence {} timed out", self.sequence).into(),
                );
            }
            (status, _) = self
                .shared
                .progress
                .wait_timeout(status, remaining)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }
}
pub(crate) struct Reservation(Arc<Shared>);
impl Drop for Reservation {
    fn drop(&mut self) {
        self.0.pending.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Entry {
    sequence: u64,
    change: Change,
    _reservation: Reservation,
}
pub(crate) struct Journal {
    sender: Option<mpsc::Sender<Entry>>,
    worker: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
    sequence: u64,
    capacity: usize,
    restart_counter: u32,
    owner: String,
    #[cfg(unix)]
    budget: budget::Budget,
}
pub(crate) struct Startup {
    #[cfg(unix)]
    store: disk::Store,
    #[cfg(unix)]
    options: crate::config::SecurityStorage,
}
impl Startup {
    pub(crate) fn open(config: &mut Config) -> std::result::Result<Option<Self>, BoxError> {
        let Some(options) = config.security_storage.clone() else {
            return Ok(None);
        };
        #[cfg(not(unix))]
        {
            let _ = options;
            Err("Security journal requires Unix".into())
        }
        #[cfg(unix)]
        {
            let mut store = disk::Store::open_mode(
                &options.directory,
                &config.service.id,
                State::from_config(config),
                config.require_initialized_security,
                disk::TailPolicy::Defer,
                options.minimum_sequence,
            )?;
            store.state.restore(config)?;
            Ok(Some(Self { store, options }))
        }
    }
    pub(crate) fn start(self, config: &Config) -> std::result::Result<Journal, BoxError> {
        #[cfg(not(unix))]
        {
            let _ = config;
            Err("Security journal requires Unix".into())
        }
        #[cfg(unix)]
        {
            let Self { mut store, options } = self;
            store.state.restart_counter = store.state.restart_counter.wrapping_add(1);
            let restart_counter = store.state.restart_counter;
            // Leave headroom for counter digit growth as well as the snapshot envelope.
            budget::Budget::check_state(&store.state, &config.service.id)?;
            // Compact recovered records before accepting connections. A fresh baseline is durable too.
            store.checkpoint()?;
            let sequence = store.sequence;
            let budget = budget::Budget::new(&store.state, &config.service.id);
            let shared = Arc::new(Shared {
                progress: Condvar::new(),
                status: Mutex::new(Status {
                    enabled: true,
                    sync: options.sync,
                    submitted_sequence: sequence,
                    durable_sequence: sequence,
                    snapshot_sequence: sequence,
                    ..Status::default()
                }),
                pending: AtomicUsize::new(0),
                stopping: AtomicBool::new(false),
                alive: AtomicBool::new(true),
                #[cfg(all(test, unix))]
                write_gate: Mutex::new(None),
            });
            let (sender, receiver) = mpsc::channel();
            let background = shared.clone();
            let worker = std::thread::Builder::new()
                .name("mynofy-security".into())
                .spawn(move || {
                    struct Alive(Arc<Shared>);
                    impl Drop for Alive {
                        fn drop(&mut self) {
                            let _status = self.0.status();
                            self.0.alive.store(false, Ordering::Release);
                            self.0.progress.notify_all();
                        }
                    }
                    let _alive = Alive(background.clone());
                    store.run(receiver, background, options.compact_after_records);
                })?;
            Ok(Journal {
                sender: Some(sender),
                worker: Some(worker),
                shared,
                sequence,
                capacity: options.queue_capacity,
                restart_counter,
                owner: config.service.id.clone(),
                budget,
            })
        }
    }
}
impl Journal {
    pub(crate) fn durability_fence(&self) -> DurabilityFence {
        DurabilityFence {
            shared: self.shared.clone(),
            sequence: self.sequence,
        }
    }
    pub(crate) fn restart_counter(&self) -> u32 {
        self.restart_counter
    }
    pub(crate) fn reserve(&self) -> Result<Reservation> {
        if self.sequence == u64::MAX || !self.shared.alive.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "security_storage_unavailable",
                "Security writer is unavailable.",
            ));
        }
        let mut pending = self.shared.pending.load(Ordering::Acquire);
        loop {
            if pending >= self.capacity {
                return Err(ApiError::new(
                    "security_storage_busy",
                    "Security write queue is full; retry later.",
                ));
            }
            match self.shared.pending.compare_exchange_weak(
                pending,
                pending + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(current) => pending = current,
            }
        }
        Ok(Reservation(self.shared.clone()))
    }
    pub(crate) fn validate_change(&self, change: &Change) -> Result<()> {
        #[cfg(unix)]
        disk::validate_change(&self.owner, change).map_err(|_| {
            ApiError::new(
                "security_record_too_large",
                "Security change exceeds the journal record limit.",
            )
        })?;
        #[cfg(unix)]
        self.budget.check(change)?;
        Ok(())
    }
    pub(crate) fn submit(&mut self, reservation: Reservation, change: Change) -> Result<()> {
        self.validate_change(&change)?;
        #[cfg(unix)]
        self.budget.apply(&change);
        self.sequence += 1;
        self.shared.status().submitted_sequence = self.sequence;
        let entry = Entry {
            sequence: self.sequence,
            change,
            _reservation: reservation,
        };
        if self
            .sender
            .as_ref()
            .is_none_or(|sender| sender.send(entry).is_err())
        {
            self.shared.status().last_error = Some("Security writer stopped".into());
            return Err(ApiError::new(
                "security_storage_unavailable",
                "Security writer stopped after the RAM update.",
            ));
        }
        Ok(())
    }
    pub(crate) fn status(&self) -> Status {
        let mut status = self.shared.status().clone();
        status.pending_records = self.shared.pending.load(Ordering::Acquire);
        if !self.shared.alive.load(Ordering::Acquire) && status.last_error.is_none() {
            status.last_error = Some("Security writer stopped".into());
        }
        status
    }
}
impl Journal {
    #[cfg(all(test, unix))]
    pub(crate) fn set_write_gate(&self, gate: Arc<WriteGate>) {
        *self.shared.write_gate.lock().unwrap() = Some(gate);
    }

    pub(crate) fn drain(&mut self) -> std::result::Result<(), BoxError> {
        {
            let _status = self.shared.status();
            self.shared.stopping.store(true, Ordering::Release);
            self.shared.progress.notify_all();
        }
        self.sender.take();
        let panicked = self
            .worker
            .take()
            .is_some_and(|worker| worker.join().is_err());
        let status = self.shared.status();
        if panicked || status.durable_sequence < status.submitted_sequence {
            return Err(format!(
                "security_drain_failed: submitted_sequence={} durable_sequence={}",
                status.submitted_sequence, status.durable_sequence
            )
            .into());
        }
        Ok(())
    }
}
impl Drop for Journal {
    fn drop(&mut self) {
        let _ = self.drain();
    }
}
#[cfg(unix)]
fn check_recovery_floor(floor: Option<u64>, recovered: u64) -> std::result::Result<(), BoxError> {
    if let Some(floor) = floor
        && recovered < floor
    {
        return Err(format!(
            "Recovered security sequence {recovered} is below configured minimum_sequence {floor}"
        )
        .into());
    }
    Ok(())
}
#[cfg(unix)]
mod disk;

#[cfg(unix)]
pub(crate) struct OfflineLock {
    _store: disk::Store,
}
#[cfg(unix)]
impl OfflineLock {
    pub(crate) fn open(config: &mut Config) -> std::result::Result<Self, BoxError> {
        Self::open_mode(config, false)
    }

    pub(crate) fn open_mode(
        config: &mut Config,
        preview: bool,
    ) -> std::result::Result<Self, BoxError> {
        let options = config
            .security_storage
            .as_ref()
            .ok_or("Offline maintenance requires security storage")?;
        let mut store = disk::Store::open_mode(
            &options.directory,
            &config.service.id,
            State::from_config(config),
            preview || config.require_initialized_security,
            if preview {
                disk::TailPolicy::Preview
            } else {
                disk::TailPolicy::Repair
            },
            options.minimum_sequence,
        )?;
        store.state.restore(config)?;
        Ok(Self { _store: store })
    }
}

pub(crate) fn initialize(config: &Config) -> std::result::Result<(), BoxError> {
    #[cfg(unix)]
    {
        let options = config
            .security_storage
            .as_ref()
            .ok_or("Security storage must be explicit")?;
        if options.minimum_sequence.is_some_and(|floor| floor > 0) {
            return Err("security-init cannot meet a positive minimum_sequence; restore a qualifying Security store".into());
        }
        disk::Store::initialize(
            &options.directory,
            &config.service.id,
            State::from_config(config),
        )
    }
    #[cfg(not(unix))]
    {
        let _ = config;
        Err("Security storage requires Unix".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_peer_policy_rejects_equivalent_certificate_identities() {
        use crate::config::{DUPLICATE_PEER_CERTIFICATE_IDENTITY, Peer};

        let mut config: Config =
            serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
        let mut state = State::from_config(&config);
        for (service_id, certificate_identity) in
            [("edge", "2001:db8::1"), ("other", "2001:0db8:0:0:0:0:0:1")]
        {
            state.apply(&Change::Peer {
                service_id: service_id.into(),
                value: Some(Peer {
                    service_id: service_id.into(),
                    certificate_identity: Some(certificate_identity.into()),
                    topics: vec![config.topics[0].name.clone()],
                }),
            });
        }
        assert_eq!(
            state.restore(&mut config).unwrap_err().to_string(),
            DUPLICATE_PEER_CERTIFICATE_IDENTITY
        );
    }

    #[tokio::test]
    async fn confirmation_wait_keeps_runtime_responsive_and_waits_for_its_sequence() {
        let shared = Arc::new(Shared {
            progress: Condvar::new(),
            status: Mutex::new(Status::default()),
            pending: AtomicUsize::new(1),
            stopping: AtomicBool::new(false),
            alive: AtomicBool::new(true),
            #[cfg(all(test, unix))]
            write_gate: Mutex::new(None),
        });
        let fence = DurabilityFence {
            shared: shared.clone(),
            sequence: 2,
        };
        let waiter = tokio::spawn(fence.confirm());
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(!waiter.is_finished());
        {
            let mut status = shared.status();
            status.durable_sequence = 1;
            shared.progress.notify_all();
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(!waiter.is_finished());
        {
            let mut status = shared.status();
            status.durable_sequence = 2;
            shared.progress.notify_all();
        }
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    #[test]
    fn content_fence_waits_for_exact_sequence_and_bounds_failures() {
        fn shared(durable: u64) -> Arc<Shared> {
            Arc::new(Shared {
                progress: Condvar::new(),
                status: Mutex::new(Status {
                    durable_sequence: durable,
                    ..Status::default()
                }),
                pending: AtomicUsize::new(0),
                stopping: AtomicBool::new(false),
                alive: AtomicBool::new(true),
                #[cfg(all(test, unix))]
                write_gate: Mutex::new(None),
            })
        }
        let state = shared(1);
        let background = state.clone();
        let writer = std::thread::spawn(move || {
            let mut status = background.status();
            status.durable_sequence = 2;
            background.progress.notify_all();
        });
        DurabilityFence {
            shared: state.clone(),
            sequence: 2,
        }
        .wait()
        .unwrap();
        writer.join().unwrap();
        state.status().last_error = Some("later compaction error".into());
        DurabilityFence {
            shared: state.clone(),
            sequence: 2,
        }
        .wait()
        .unwrap();
        assert!(
            DurabilityFence {
                shared: state,
                sequence: 3
            }
            .wait()
            .is_err()
        );
        let state = shared(0);
        assert!(
            DurabilityFence {
                shared: state.clone(),
                sequence: 1
            }
            .wait_for(std::time::Duration::from_millis(10))
            .unwrap_err()
            .to_string()
            .contains("timed out")
        );
        state.alive.store(false, Ordering::Release);
        assert!(
            DurabilityFence {
                shared: state.clone(),
                sequence: 1
            }
            .wait()
            .is_err()
        );
        state.alive.store(true, Ordering::Release);
        state.stopping.store(true, Ordering::Release);
        assert!(
            DurabilityFence {
                shared: state,
                sequence: 1
            }
            .wait()
            .is_err()
        );
    }
    #[test]
    fn legacy_security_snapshot_uses_configured_peer_baseline_once() {
        let mut cfg: Config =
            serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
        cfg.accepted_peers.push(crate::config::Peer {
            service_id: "edge".into(),
            certificate_identity: None,
            topics: vec!["local/events".into()],
        });
        let mut old = serde_json::to_value(State::from_config(&cfg)).unwrap();
        for field in ["peers", "disabled_peers", "revoked_peer_certificates"] {
            old.as_object_mut().unwrap().remove(field);
        }
        let mut state: State = serde_json::from_value(old).unwrap();
        state.restore(&mut cfg).unwrap();
        assert_eq!(cfg.accepted_peers[0].service_id, "edge");
        assert_eq!(state.peers.as_ref().unwrap().len(), 1);
        state.apply(&Change::Peer {
            service_id: "edge".into(),
            value: None,
        });
        state.restore(&mut cfg).unwrap();
        assert!(cfg.accepted_peers.is_empty());
    }

    #[test]
    fn cancelled_reservations_release_capacity_and_submission_order_is_exact() {
        let shared = Arc::new(Shared {
            progress: Condvar::new(),
            status: Mutex::new(Status::default()),
            pending: AtomicUsize::new(0),
            stopping: AtomicBool::new(false),
            alive: AtomicBool::new(true),
            #[cfg(all(test, unix))]
            write_gate: Mutex::new(None),
        });
        let (sender, receiver) = mpsc::channel();
        let mut journal = Journal {
            sender: Some(sender),
            worker: None,
            shared,
            sequence: 0,
            capacity: 1,
            restart_counter: 0,
            owner: "test".into(),
            budget: budget::Budget::new(&State::default(), "test"),
        };
        let permit = journal.reserve().unwrap();
        assert!(journal.reserve().is_err());
        drop(permit);
        let permit = journal.reserve().unwrap();
        journal
            .submit(
                permit,
                Change::Acl {
                    client: "c".into(),
                    topic: "t".into(),
                    allow: vec![],
                },
            )
            .unwrap();
        assert_eq!(journal.status().submitted_sequence, 1);
        assert_eq!(journal.status().pending_records, 1);
        assert!(journal.reserve().is_err());
        let entry = receiver.recv().unwrap();
        assert_eq!(entry.sequence, 1);
        drop(entry);
        assert!(journal.reserve().is_ok());
        assert_eq!(journal.status().pending_records, 0);
    }
}
