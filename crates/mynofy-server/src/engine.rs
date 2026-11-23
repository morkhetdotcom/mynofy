//! Transport-independent RAM owner. All mutations are atomic under the caller's short-lived lock.
use crate::{
    config::{Action, Config, Policy},
    crypto::{self, CursorCodec},
    protocol::{self, ApiError, Command, Result},
};
use crate::{
    expiration::{Expiration, Slots},
    ordered::{IdTable, OrderedIndex},
};
use crate::{
    ids::Id,
    storage::{Arena, Blobs, ContentTypes, Record as Stored, RecordFields, StoredPayload},
};
use prost::Message as _;
use protocol::{payload::Payload, pb};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use zeroize::Zeroizing;

#[cfg(test)]
thread_local! {
    static STORAGE_STATS_SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct Topic {
    slot: u16,
    head: u64,
    messages: OrderedIndex,
    policy: Policy,
}
struct Credential {
    client: String,
    public: [u8; 32],
    until: u64,
}
struct Challenge {
    id: String,
    token: String,
    nonce: [u8; 32],
    revision: u64,
}
struct Auth {
    client: String,
    token: String,
    revision: u64,
}
struct Session {
    invalidated: tokio::sync::watch::Sender<bool>,
    started: u64,
    auth: Option<Auth>,
    challenge: Option<Challenge>,
}
struct Rotation {
    id: String,
    session: u64,
    old_token: String,
    revision: u64,
    until: u64,
    new_until: u64,
    secret: Option<Zeroizing<[u8; 32]>>,
    nonce: [u8; 12],
    challenge: [u8; 32],
}
#[derive(Clone)]
struct Bucket {
    units: u64,
    updated: u64,
}
impl Bucket {
    fn new(burst: u64, now: u64) -> Self {
        Self {
            units: burst.saturating_mul(1000),
            updated: now,
        }
    }
    fn take(&mut self, rate: u64, burst: u64, now: u64) -> Result<()> {
        let elapsed = now.saturating_sub(self.updated);
        self.units = self
            .units
            .saturating_add(elapsed.saturating_mul(rate))
            .min(burst.saturating_mul(1000));
        self.updated = self.updated.max(now);
        if self.units < 1000 {
            return Err(ApiError::limited((1000 - self.units).div_ceil(rate)));
        }
        self.units -= 1000;
        Ok(())
    }
}
struct ClientState {
    disabled: bool,
    display_name: Option<String>,
    request_slots: std::sync::Arc<tokio::sync::Semaphore>,
    slot: u16,
    produced: u64,
    revision: u64,
    bucket: Bucket,
    rotation_attempts: Vec<u64>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    kind: String,
    generation: String,
    client: String,
    topic: Option<String>,
    pos: u64,
    limit: Option<usize>,
    watermark: Option<u64>,
    until: Option<u64>,
    revision: Option<u64>,
}

/// Requested storage bytes/capacities for profiling, not allocator- or RSS accounting.
#[derive(Serialize, Debug)]
pub struct StorageStats {
    pub active_records: usize,
    pub resident_records: usize,
    pub expired_resident_records: usize,
    pub record_bytes: usize,
    pub slot_bytes: usize,
    pub inline_event_bytes: usize,
    pub payload_descriptor_bytes: usize,
    pub record_capacity: usize,
    pub arena_allocated_bytes: usize,
    pub payload_data_bytes: usize,
    pub payload_allocated_bytes: usize,
    pub payload_pages: usize,
    pub content_type_table_bytes: usize,
    pub id_index_capacity: usize,
    pub id_index_entry_bytes: usize,
    pub id_index_data_bytes: usize,
    pub topic_index_entries: usize,
    pub producer_index_entries: usize,
    pub expiry_entries: usize,
    pub ordered_data_bytes: usize,
    pub ordered_blocks: usize,
    pub expiry_data_bytes: usize,
    pub expiry_buckets: usize,
    pub extended_time_entries: usize,
    pub interned_content_types: usize,
}
// A unit deletes one record, or one expired rotation. Never an entire fan-out.
const REQUEST_CLEANUP_UNITS: usize = 64;
const MAINTENANCE_CLEANUP_UNITS: usize = 512;
const READ_SCAN_LIMIT: usize = 256;
#[derive(Default, Debug, Clone, Copy, Serialize)]
pub struct CleanupProgress {
    pub work: usize,
    pub records: usize,
    pub rotations: usize,
    pub more: bool,
}
type PollEntry = (u64, u32);

pub struct Engine {
    poll_scratch: Vec<PollEntry>,
    restart_counter: u32,
    content: Option<crate::content::ContentRuntime>,
    security: Option<crate::security::Journal>,
    pub config: Config,
    generation: String,
    codec: CursorCodec,
    keys: HashMap<String, Credential>,
    clients: HashMap<String, ClientState>,
    acl: crate::acl::Acl,
    pub(crate) link_status: std::sync::Arc<std::sync::Mutex<crate::links::Status>>,
    pub(crate) link_changes: tokio::sync::watch::Sender<u64>,
    pub(crate) peer_revisions: BTreeMap<String, u64>,
    topics: BTreeMap<String, Topic>,
    topic_names: Vec<String>,
    client_names: Vec<String>,
    records: Arena,
    content_types: ContentTypes,
    payloads: Blobs,
    sessions: HashMap<u64, Session>,
    next_session: u64,
    next_message: u64,
    message_ids: crate::ids::IdPool,
    index: IdTable<u32>,
    own: HashMap<String, OrderedIndex>,
    expiry: Expiration,
    cleanup_batch: Option<Slots>,
    cleanup_record: Option<u32>,
    last_cleanup_now: u64,
    rotations: HashMap<String, Rotation>,
    rotation_expiry: BTreeSet<(u64, String)>,
    active_bytes: usize,
    maintenance_cursor: usize,
}
impl Engine {
    pub fn restart_counter(&self) -> u32 {
        self.restart_counter
    }
    pub fn new(config: Config, now: u64) -> Result<Self> {
        Self::new_with_content_secret(config, now, None)
    }
    /// All content loading and decryption occurs here, before any session or listener is admitted.
    pub fn new_with_content_secret(
        mut config: Config,
        now: u64,
        secret: Option<crate::content::ContentSecret>,
    ) -> Result<Self> {
        config.validate().map_err(|_| ApiError::invalid())?;
        let mut startup =
            crate::content::Startup::open(&config, secret, now).map_err(content_error)?;
        let configured_topics: std::collections::HashSet<_> =
            config.topics.iter().map(|t| t.name.clone()).collect();
        let checkpoint = startup.as_mut().and_then(|startup| startup.loaded.take());
        if let Some(checkpoint) = &checkpoint {
            checkpoint
                .prepare_topics(&mut config)
                .map_err(content_error)?;
            if let Some(startup) = &startup {
                startup.apply_storages(&mut config, &configured_topics);
            }
        }

        let security = crate::security::Journal::open(&mut config).map_err(|error| {
            eprintln!("mynofy: security recovery failed: {error}");
            ApiError::new(
                "security_storage_error",
                "Security state could not be safely loaded; see service stderr.",
            )
        })?;
        if let Some(checkpoint) = &checkpoint {
            checkpoint
                .order_clients(&mut config)
                .map_err(content_error)?;
        }
        protocol::catalog::builtin_catalog()?;
        let mut keys = HashMap::new();
        let mut clients = HashMap::new();
        let mut own = HashMap::new();
        let client_names: Vec<_> = config.clients.iter().map(|c| c.client_id.clone()).collect();
        for (client_slot, c) in config.clients.iter().enumerate() {
            clients.insert(
                c.client_id.clone(),
                ClientState {
                    disabled: c.disabled,
                    display_name: c.display_name.clone(),
                    request_slots: std::sync::Arc::new(tokio::sync::Semaphore::new(
                        config.limits.max_parallel_requests_per_client,
                    )),
                    slot: client_slot as u16,
                    produced: 0,
                    revision: 0,
                    bucket: Bucket::new(config.limits.client_burst, now),
                    rotation_attempts: Vec::new(),
                },
            );
            own.insert(c.client_id.clone(), OrderedIndex::new());
            for key in &c.keys {
                let (public, until) = crate::config::read_public(key)?;
                keys.insert(
                    crypto::token_id(&public),
                    Credential {
                        client: c.client_id.clone(),
                        public,
                        until,
                    },
                );
            }
        }
        let topic_names: Vec<_> = config
            .topics
            .iter()
            .filter(|t| t.owner_id == config.service.id)
            .map(|t| t.name.clone())
            .collect();
        let topics: BTreeMap<String, Topic> = config
            .topics
            .iter()
            .filter(|t| t.owner_id == config.service.id)
            .enumerate()
            .filter_map(|(slot, t)| {
                t.policy.clone().map(|p| {
                    (
                        t.name.clone(),
                        Topic {
                            slot: slot as u16,
                            head: 0,
                            messages: OrderedIndex::new(),
                            policy: p,
                        },
                    )
                })
            })
            .collect();
        let mut acl = crate::acl::Acl::default();
        for topic in &config.topics {
            if let Some(local) = topics.get(&topic.name) {
                for rule in topic.acl.iter().flatten() {
                    let client = clients.get(&rule.client_id).ok_or_else(ApiError::invalid)?;
                    acl.set(client.slot, local.slot, &rule.allow);
                }
            }
        }
        let restart_counter = security
            .as_ref()
            .map_or(0, |journal| journal.restart_counter());
        let mut engine = Self {
            poll_scratch: Vec::new(),
            restart_counter,
            content: None,
            security,
            config,
            generation: crypto::encode(&crypto::random::<16>()?),
            codec: CursorCodec::new(crypto::random()?),
            keys,
            clients,
            acl,
            topics,
            link_status: Default::default(),
            link_changes: tokio::sync::watch::channel(0).0,
            peer_revisions: BTreeMap::new(),
            topic_names,
            client_names,
            records: Arena::new(),
            content_types: ContentTypes::default(),
            payloads: Blobs::default(),
            sessions: HashMap::new(),
            next_session: 0,
            next_message: 0,
            message_ids: crate::ids::IdPool::new(),
            // Boot-local random starting point avoids deterministic reuse across RAM-only restarts.
            index: IdTable::new(),
            own,
            expiry: Expiration::new(),
            cleanup_batch: None,
            cleanup_record: None,
            last_cleanup_now: now,
            rotations: HashMap::new(),
            rotation_expiry: BTreeSet::new(),
            active_bytes: 0,
            maintenance_cursor: 0,
        };
        let loaded = checkpoint.is_some();
        let saved_revision = checkpoint
            .as_ref()
            .map_or(0, |checkpoint| checkpoint.revision);
        if let Some(checkpoint) = checkpoint {
            engine.restore_content(checkpoint, now)?;
        }
        if let Some(startup) = startup {
            engine.content = Some(
                startup
                    .start(
                        saved_revision.saturating_add(1),
                        saved_revision,
                        loaded,
                        &engine.config,
                    )
                    .map_err(content_error)?,
            );
        }
        Ok(engine)
    }
    /// Copy the initialized binary blocks under the caller's lock; write the returned image outside it.
    /// This captures messages and bindings, not confirmations, security state or a full owner checkpoint.
    pub fn capture_message_image(
        &self,
        limits: crate::message_image::ImageLimits,
    ) -> Result<crate::message_image::MessageImage> {
        crate::message_image::MessageImage::capture(
            &self.config.service.id,
            &self.client_names,
            &self.topic_names,
            &self.records,
            &self.content_types,
            &self.payloads,
            limits,
        )
    }
    /// Explicit diagnostics only: scans resident records, not used by basic status.
    pub fn storage_stats(&self) -> StorageStats {
        #[cfg(test)]
        STORAGE_STATS_SCANS.with(|scans| scans.set(scans.get() + 1));
        // Diagnose all resident records once, including expired payloads that
        // still occupy RAM. Do not add counters to the publish/poll hot path.
        let mut active_records = 0;
        let mut payload_data_bytes = 0;
        for record in self.records.iter() {
            active_records += usize::from(record.until() > self.last_cleanup_now);
            payload_data_bytes += record.payload().external_bytes();
        }
        let resident_records = self.records.len();
        StorageStats {
            active_records,
            resident_records,
            expired_resident_records: resident_records - active_records,
            record_bytes: std::mem::size_of::<Stored>(),
            slot_bytes: std::mem::size_of::<Stored>(),
            inline_event_bytes: std::mem::size_of::<crate::storage::InlineEvent>(),
            payload_descriptor_bytes: std::mem::size_of::<StoredPayload>(),
            record_capacity: self.records.capacity(),
            arena_allocated_bytes: self.records.allocated_bytes(),
            payload_allocated_bytes: self.payloads.allocated_bytes(),
            payload_pages: self.payloads.page_count(),
            content_type_table_bytes: self.content_types.table_bytes(),
            payload_data_bytes,
            id_index_capacity: self.index.capacity(),
            id_index_entry_bytes: std::mem::size_of::<u32>(),
            id_index_data_bytes: self.index.data_bytes(),
            topic_index_entries: self.topics.values().map(|t| t.messages.len()).sum(),
            producer_index_entries: self.own.values().map(OrderedIndex::len).sum(),
            expiry_entries: self.expiry.len()
                + self
                    .cleanup_batch
                    .as_ref()
                    .map_or(0, ExactSizeIterator::len)
                + usize::from(self.cleanup_record.is_some()),
            ordered_data_bytes: self
                .topics
                .values()
                .map(|t| t.messages.data_bytes())
                .sum::<usize>()
                + self
                    .own
                    .values()
                    .map(OrderedIndex::data_bytes)
                    .sum::<usize>(),
            ordered_blocks: self
                .topics
                .values()
                .map(|t| t.messages.blocks())
                .sum::<usize>()
                + self.own.values().map(OrderedIndex::blocks).sum::<usize>(),
            expiry_data_bytes: self.expiry.data_bytes()
                + self
                    .cleanup_batch
                    .as_ref()
                    .map_or(0, Slots::allocated_bytes),
            expiry_buckets: self.expiry.buckets(),
            extended_time_entries: 0,
            interned_content_types: self.content_types.len(),
        }
    }
    /// Periodic maintenance, separate from the command hot path. Empty chunk scanning is budgeted.
    pub fn maintenance(&mut self, now: u64) -> CleanupProgress {
        let mut progress = self.cleanup_step(now, MAINTENANCE_CLEANUP_UNITS);
        let due = self
            .content
            .as_mut()
            .map_or_else(Vec::new, |content| content.due());
        if !due.is_empty()
            && let Err(error) = self.save_content_topics(Some(&due))
        {
            eprintln!("mynofy: content autosave could not be queued: {error}");
        }
        self.records.reclaim(8);
        self.payloads.reclaim(8);
        progress.more |= self.records.needs_reclaim() || self.payloads.needs_reclaim();
        if progress.more {
            return progress;
        }
        self.content_types.reclaim();
        self.expiry.reclaim();

        self.index.reclaim();
        let total = self.topic_names.len() + self.client_names.len();
        for _ in 0..32.min(total) {
            let i = self.maintenance_cursor % total;
            self.maintenance_cursor = (i + 1) % total;
            if i < self.topic_names.len() {
                if let Some(topic) = self.topics.get_mut(&self.topic_names[i]) {
                    topic.messages.reclaim();
                }
            } else if let Some(own) = self
                .own
                .get_mut(&self.client_names[i - self.topic_names.len()])
            {
                own.reclaim();
            }
        }
        progress
    }
    /// Start bounded asynchronous RID refill when attaching the core to a runtime.
    pub fn start_id_allocator(&mut self) {
        self.message_ids.start();
    }
    pub fn connect(&mut self, now: u64) -> Result<u64> {
        let now = now.max(self.last_cleanup_now);
        if self.sessions.len() >= self.config.limits.max_connections {
            return Err(ApiError::exhausted());
        }
        self.next_session = self
            .next_session
            .checked_add(1)
            .ok_or_else(ApiError::exhausted)?;
        self.sessions.insert(
            self.next_session,
            Session {
                invalidated: tokio::sync::watch::channel(false).0,
                started: now,
                auth: None,
                challenge: None,
            },
        );
        Ok(self.next_session)
    }
    pub(crate) fn session_changes(
        &self,
        session: u64,
    ) -> Result<tokio::sync::watch::Receiver<bool>> {
        Ok(self
            .sessions
            .get(&session)
            .ok_or_else(ApiError::unauthorized)?
            .invalidated
            .subscribe())
    }
    pub fn disconnect(&mut self, session: u64) {
        if let Some(state) = self.sessions.get(&session) {
            state.invalidated.send_replace(true);
        }
        if let Some(client) = self
            .sessions
            .remove(&session)
            .and_then(|s| s.auth.map(|a| a.client))
            && self
                .rotations
                .get(&client)
                .is_some_and(|r| r.session == session)
        {
            self.take_rotation(&client);
        }
    }
    pub fn charge(&mut self, session: u64, now: u64) -> Result<()> {
        let slot = self.identity_slot(session, now)?;
        self.charge_slot(slot, now)
    }
    fn charge_slot(&mut self, slot: u16, now: u64) -> Result<()> {
        let client = self
            .client_names
            .get(usize::from(slot))
            .ok_or_else(ApiError::internal)?;
        self.clients
            .get_mut(client)
            .ok_or_else(ApiError::unauthorized)?
            .bucket
            .take(
                self.config.limits.client_requests_per_second,
                self.config.limits.client_burst,
                now,
            )
    }
    pub fn request_semaphore(
        &self,
        session: u64,
        now: u64,
    ) -> Result<std::sync::Arc<tokio::sync::Semaphore>> {
        let (client, _, _) = self.identity_ref(session, now)?;
        self.clients
            .get(client)
            .map(|state| state.request_slots.clone())
            .ok_or_else(ApiError::unauthorized)
    }
    pub fn client_id(&self, session: u64, now: u64) -> Result<String> {
        self.identity_ref(session, now).map(|a| a.0.to_owned())
    }
    pub fn authenticated(&self, session: u64, now: u64) -> bool {
        self.identity_ref(session, now).is_ok()
    }
    pub fn auth_deadline(&self, session: u64) -> Option<u64> {
        self.sessions.get(&session).map(|s| {
            s.started
                .saturating_add(self.config.limits.auth_timeout_seconds * 1000)
        })
    }
    fn identity(&self, session: u64, now: u64) -> Result<(String, String, u64)> {
        let (client, token, revision) = self.identity_ref(session, now)?;
        Ok((client.to_owned(), token.to_owned(), revision))
    }
    fn identity_slot(&self, session: u64, now: u64) -> Result<u16> {
        let (client, _, _) = self.identity_ref(session, now)?;
        self.clients
            .get(client)
            .map(|client| client.slot)
            .ok_or_else(ApiError::unauthorized)
    }
    pub(crate) fn credential_until(&self, session: u64, now: u64) -> Result<u64> {
        let (_, token, _) = self.identity_ref(session, now)?;
        Ok(self
            .keys
            .get(token)
            .ok_or_else(ApiError::unauthorized)?
            .until)
    }
    fn identity_ref(&self, session: u64, now: u64) -> Result<(&str, &str, u64)> {
        let now = now.max(self.last_cleanup_now);
        let auth = self
            .sessions
            .get(&session)
            .and_then(|s| s.auth.as_ref())
            .ok_or_else(ApiError::unauthorized)?;
        let key = self
            .keys
            .get(&auth.token)
            .filter(|k| k.until > now && k.client == auth.client)
            .ok_or_else(ApiError::unauthorized)?;
        let client = self
            .clients
            .get(&key.client)
            .ok_or_else(ApiError::unauthorized)?;
        if client.disabled || auth.revision != client.revision {
            return Err(ApiError::unauthorized());
        }
        Ok((&auth.client, &auth.token, auth.revision))
    }

    pub fn set_acl(&mut self, client: &str, topic: &str, actions: Vec<Action>) -> Result<()> {
        if actions.len() > 3
            || actions
                .iter()
                .enumerate()
                .any(|(index, action)| actions[..index].contains(action))
        {
            return Err(ApiError::invalid());
        }
        let topic_slot = self
            .topics
            .get(topic)
            .ok_or_else(|| ApiError::new("owner_unavailable", "Topic is not owned locally."))?
            .slot;
        let permit = self.security_reserve()?;
        let change = crate::security::Change::Acl {
            client: client.into(),
            topic: topic.into(),
            allow: actions.clone(),
        };
        self.security_check(&change)?;
        let c = self
            .clients
            .get_mut(client)
            .ok_or_else(ApiError::unauthorized)?;
        c.revision = c.revision.checked_add(1).ok_or_else(ApiError::exhausted)?;
        self.acl.set(c.slot, topic_slot, &actions);
        let sessions: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|(id, s)| s.auth.as_ref().filter(|a| a.client == client).map(|_| *id))
            .collect();
        for id in sessions {
            self.disconnect(id);
        }
        // Also invalidate in-flight unauthenticated challenges and pending rotations.
        self.take_rotation(client);
        self.security_submit(permit, change)?;
        Ok(())
    }
    pub fn revoke_key(&mut self, token: &str) -> Result<()> {
        let Some(key) = self.keys.get(token) else {
            return Ok(());
        };
        let client = key.client.clone();
        let public_key = crypto::encode(&key.public);
        let permit = self.security_reserve()?;
        let mut candidate = self.security_client(&client)?;
        candidate.keys.retain(|key| key.public_key != public_key);
        self.security_check(&crate::security::Change::Client { value: candidate })?;
        self.keys.remove(token);
        for session in self.sessions.values_mut() {
            if session
                .challenge
                .as_ref()
                .is_some_and(|challenge| challenge.token == token)
            {
                session.challenge = None;
            }
        }
        let sessions: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|(id, s)| s.auth.as_ref().filter(|a| a.token == token).map(|_| *id))
            .collect();
        for id in sessions {
            self.disconnect(id);
        }
        let value = self.security_client(&client)?;
        self.security_submit(permit, crate::security::Change::Client { value })?;
        Ok(())
    }
    /// Bounded request-path cleanup. Expiry visibility never depends on finishing this work.
    pub fn sweep(&mut self, now: u64) -> CleanupProgress {
        self.cleanup_step(now, REQUEST_CLEANUP_UNITS)
    }
    pub fn cleanup_step(&mut self, now: u64, budget: usize) -> CleanupProgress {
        let now = now.max(self.last_cleanup_now);
        self.last_cleanup_now = now;
        let budget = budget.min(MAINTENANCE_CLEANUP_UNITS);
        let mut progress = CleanupProgress::default();
        if self.cleanup_record.is_none()
            && self.cleanup_batch.is_none()
            && !self.expiry.has_due(now)
            && self
                .rotation_expiry
                .first()
                .is_none_or(|entry| entry.0 > now)
        {
            return progress;
        }

        while progress.work < budget.min(8)
            && self.rotation_expiry.first().is_some_and(|x| x.0 <= now)
        {
            let Some((_, client)) = self.rotation_expiry.pop_first() else {
                break;
            };
            self.take_rotation(&client);
            progress.rotations += 1;
            progress.work += 1;
        }
        while progress.work < budget {
            if self.cleanup_record.is_none() {
                let next = self.cleanup_batch.as_mut().and_then(Iterator::next);
                if let Some(slot) = next {
                    self.cleanup_record = Some(slot);
                } else {
                    self.cleanup_batch = None;
                    let Some(batch) = self.expiry.pop_due(now) else {
                        break;
                    };
                    self.cleanup_batch = Some(batch.slots());
                    continue;
                }
            }
            let Some(slot) = self.cleanup_record else {
                break;
            };
            let Some(record) = self.records.get(slot) else {
                self.cleanup_record = None;
                progress.work += 1;
                continue;
            };
            if record.until() > now {
                break;
            }
            if let Some(stored) = self.records.remove(slot) {
                if let Some(name) = self.topic_names.get(stored.topic() as usize)
                    && let Some(topic) = self.topics.get_mut(name)
                {
                    topic.messages.remove(&stored.seq());
                }
                self.index.remove(&stored.id());
                self.active_bytes = self
                    .active_bytes
                    .saturating_sub(stored.charge(&self.content_types, &self.payloads));
                stored
                    .payload()
                    .release(&mut self.content_types, &mut self.payloads);
                if let Some(name) = self.client_names.get(stored.producer_slot())
                    && let Some(own) = self.own.get_mut(name)
                {
                    own.remove(&stored.own_pos());
                }
                self.content_changed(stored.topic());
                progress.records += 1;
            }
            self.cleanup_record = None;
            progress.work += 1;
        }
        if self
            .cleanup_batch
            .as_ref()
            .is_some_and(|batch| batch.len() == 0)
        {
            self.cleanup_batch = None;
        }
        progress.more = self
            .cleanup_record
            .is_some_and(|slot| self.records.get(slot).is_some_and(|r| r.until() <= now))
            || self
                .cleanup_batch
                .as_ref()
                .is_some_and(|batch| batch.len() > 0)
            || self.expiry.has_due(now)
            || self.rotation_expiry.first().is_some_and(|x| x.0 <= now);
        progress
    }
    fn take_rotation(&mut self, client: &str) -> Option<Rotation> {
        let r = self.rotations.remove(client)?;
        self.rotation_expiry.remove(&(r.until, client.into()));
        Some(r)
    }
    /// In-process JSON adapter for tests, comparisons and readable tooling.
    /// Broker client transports accept only binary Protobuf frames.
    pub fn handle(&mut self, session: u64, raw: &str, now: u64) -> Value {
        let now = now.max(self.last_cleanup_now);
        self.sweep(now);
        if self.authenticated(session, now)
            && let Err(e) = self.charge(session, now)
        {
            return e.frame(None);
        }
        if raw.len() > self.config.limits.max_response_bytes {
            return ApiError::invalid().frame(None);
        }
        let value = match protocol::strict_json(raw) {
            Ok(v) => v,
            Err(e) => return e.frame(None),
        };
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| s.len() <= 128)
            .map(str::to_owned);
        match protocol::parse_value(value).and_then(|c| self.command(session, c, now)) {
            Ok(frame) => frame,
            Err(err) => err.frame(id.as_deref()),
        }
    }
    /// Binary protobuf hot path: native application operations avoid JSON request/response trees.
    pub fn handle_protobuf(&mut self, session: u64, raw: &[u8], now: u64) -> pb::Frame {
        self.handle_protobuf_inner(session, raw, now, None)
    }
    /// Binary transport entry point. Poll payloads encode directly from the record arena.
    pub fn handle_protobuf_into(
        &mut self,
        session: u64,
        raw: &[u8],
        now: u64,
        out: &mut bytes::BytesMut,
    ) -> &'static str {
        out.clear();
        let frame = self.handle_protobuf_inner(session, raw, now, Some(out));
        if out.is_empty() {
            out.reserve(frame.encoded_len());
            frame.encode_raw(out);
        }
        frame.kind()
    }
    fn handle_protobuf_inner(
        &mut self,
        session: u64,
        raw: &[u8],
        now: u64,
        mut output: Option<&mut bytes::BytesMut>,
    ) -> pb::Frame {
        let now = now.max(self.last_cleanup_now);
        self.sweep(now);
        let client_slot = self.identity_slot(session, now).ok();
        if let Some(slot) = client_slot
            && let Err(e) = self.charge_slot(slot, now)
        {
            return pb::Frame::error(e, None);
        }
        if raw.len() > self.config.limits.max_response_bytes {
            return pb::Frame::error(ApiError::invalid(), None);
        }
        let frame = match pb::Frame::decode(raw) {
            Ok(f) if f.version == 1 => f,
            _ => return pb::Frame::error(ApiError::invalid(), None),
        };
        let id = frame.request_id().map(str::to_owned);
        if id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 128)
        {
            return pb::Frame::error(ApiError::invalid(), None);
        }
        let response = (|| -> Result<pb::Frame> {
            let body = match frame.body {
                Some(pb::Body::Publish(x)) => {
                    let client = client_slot.ok_or_else(ApiError::unauthorized)?;
                    let expires = x.expires.map(|e| protocol::Expires { time: e.time });
                    pb::Body::Result(
                        self.publish_typed(client, &x.id, &x.topic, x.payload, expires, now)?,
                    )
                }
                Some(pb::Body::Poll(x)) => pb::Body::Messages(self.poll_validated(
                    client_slot.ok_or_else(ApiError::unauthorized)?,
                    &x.id,
                    &x.topic,
                    x.after.as_deref(),
                    x.limit.map(|n| n as usize),
                    now,
                    true,
                    output.as_deref_mut(),
                )?),

                _ => return self.command_protobuf_other(session, frame, now, client_slot),
            };
            Ok(pb::Frame {
                version: 1,
                body: Some(body),
            })
        })();
        response.unwrap_or_else(|e| {
            if let Some(out) = output {
                out.clear();
            }
            pb::Frame::error(e, id)
        })
    }
    // Keep the primary publish/poll dispatcher small without marking these operations cold.
    #[inline(never)]
    fn command_protobuf_other(
        &mut self,
        session: u64,
        frame: pb::Frame,
        now: u64,
        _client_slot: Option<u16>,
    ) -> Result<pb::Frame> {
        let body = match frame.body {
            Some(pb::Body::GetMessage(x)) => {
                let (client, _, _) = self.identity_ref(session, now)?;
                pb::Body::MessageStatus(self.get_typed(client, &x.id, &x.message_id, now)?)
            }
            Some(pb::Body::ListMessages(x)) => {
                let (client, _, _) = self.identity_ref(session, now)?;
                pb::Body::MessagesPage(self.list_typed(
                    client,
                    &x.id,
                    x.topic,
                    x.limit.map(|n| n as usize),
                    x.cursor,
                    now,
                    true,
                )?)
            }
            Some(pb::Body::GetPermissions(x)) => {
                let (client, _, revision) = self.identity_ref(session, now)?;
                let client = self
                    .clients
                    .get(client)
                    .ok_or_else(ApiError::unauthorized)?
                    .slot;
                let topic = self
                    .topics
                    .get(&x.topic)
                    .ok_or_else(|| {
                        ApiError::new("owner_unavailable", "Topic is not owned locally.")
                    })?
                    .slot;
                pb::Body::Permissions(pb::Permissions {
                    request_id: x.id,
                    topic: x.topic,
                    owner_id: self.config.service.id.clone(),
                    acl_revision: format!("{}:{revision}", self.generation),
                    actions: self
                        .acl
                        .actions(client, topic)
                        .iter()
                        .map(|a| {
                            match a {
                                Action::Publish => "publish",
                                Action::Subscribe => "subscribe",
                                Action::InspectOwn => "inspect_own",
                            }
                            .into()
                        })
                        .collect(),
                })
            }
            Some(pb::Body::GetCatalog(x)) => {
                self.identity_ref(session, now)?;
                if x.name != protocol::catalog::CATALOG_NAME
                    || x.catalog_version != protocol::catalog::CATALOG_VERSION
                {
                    return Err(ApiError::new(
                        "not_found",
                        "Catalog name or version not available.",
                    ));
                }
                let definition = serde_json::to_vec(protocol::catalog::builtin_catalog()?)
                    .map_err(|_| ApiError::internal())?;
                let response = pb::Catalog {
                    request_id: x.id,
                    definition,
                };
                if response.encoded_len() + 16 > self.config.limits.max_response_bytes {
                    return Err(ApiError::exhausted());
                }
                pb::Body::Catalog(response)
            }
            _ => {
                let value = frame.to_json()?;
                let command = protocol::parse_value(value)?;
                let response = self.command(session, command, now)?;
                return pb::Frame::from_json(response);
            }
        };
        Ok(pb::Frame {
            version: 1,
            body: Some(body),
        })
    }
    fn command(&mut self, session: u64, cmd: Command, now: u64) -> Result<Value> {
        if matches!(cmd, Command::AuthHello { .. } | Command::AuthProof { .. })
            && self
                .sessions
                .get(&session)
                .is_some_and(|s| s.auth.is_some())
        {
            self.disconnect(session);
            return Err(ApiError::unauthorized());
        }
        match cmd {
            Command::AuthHello { id, token_id } => return self.hello(session, id, token_id, now),
            Command::AuthProof {
                id,
                auth_id,
                signature,
            } => return self.proof(session, id, auth_id, signature, now),
            _ => {}
        }
        let (client, token, revision) = self.identity(session, now)?;

        match cmd {
            Command::Publish {
                id,
                topic,
                payload,
                expires,
            } => self.publish(&client, &id, &topic, payload, expires, now),
            Command::Poll {
                id,
                topic,
                after,
                limit,
            } => self.poll(session, &id, &topic, after.as_deref(), limit, now),
            Command::GetMessage { id, message_id } => self.get(&client, &id, &message_id, now),
            Command::ListMessages {
                id,
                topic,
                limit,
                cursor,
            } => self.list(&client, &id, topic, limit, cursor, now),
            Command::GetCatalog {
                id,
                name,
                catalog_version,
            } => {
                if name != protocol::catalog::CATALOG_NAME
                    || catalog_version != protocol::catalog::CATALOG_VERSION
                {
                    return Err(ApiError::new(
                        "not_found",
                        "Catalog name or version not available.",
                    ));
                }
                let response = json!({"version":1,"type":"catalog","request_id":id,"catalog":protocol::catalog::builtin_catalog()?});
                if serde_json::to_vec(&response)
                    .map_err(|_| ApiError::internal())?
                    .len()
                    > self.config.limits.max_response_bytes
                {
                    return Err(ApiError::exhausted());
                }
                Ok(response)
            }
            Command::GetPermissions { id, topic } => {
                if !self.topics.contains_key(&topic) {
                    return Err(ApiError::new(
                        "owner_unavailable",
                        "Topic is not owned locally.",
                    ));
                }
                let client_slot = self
                    .clients
                    .get(&client)
                    .ok_or_else(ApiError::unauthorized)?
                    .slot;
                let topic_slot = self.topics.get(&topic).ok_or_else(ApiError::invalid)?.slot;
                Ok(
                    json!({"version":1,"type":"permissions","request_id":id,"topic":topic,"owner_id":self.config.service.id,"acl_revision":format!("{}:{revision}",self.generation),"actions":self.acl.actions(client_slot, topic_slot)}),
                )
            }
            Command::RotationStart { id } => {
                self.rotation_start(session, &client, &token, revision, &id, now)
            }
            Command::RotationConfirm {
                id,
                rotation_id,
                shared_secret,
            } => self.rotation_confirm(session, &id, &rotation_id, &shared_secret, now),
            Command::RotationCommit {
                id,
                rotation_id,
                encrypted_public_key,
                signature,
            } => self.rotation_commit(
                session,
                &id,
                &rotation_id,
                &encrypted_public_key,
                &signature,
                now,
            ),
            _ => Err(ApiError::invalid()),
        }
    }
    fn hello(&mut self, session: u64, id: String, token: String, now: u64) -> Result<Value> {
        let s = self
            .sessions
            .get(&session)
            .ok_or_else(ApiError::unauthorized)?;
        if s.auth.is_some()
            || s.challenge.is_some()
            || now
                >= s.started
                    .saturating_add(self.config.limits.auth_timeout_seconds * 1000)
        {
            return Err(ApiError::unauthorized());
        }
        let key = self
            .keys
            .get(&token)
            .filter(|k| k.until > now)
            .ok_or_else(ApiError::unauthorized)?;
        let revision = self
            .clients
            .get(&key.client)
            .filter(|client| !client.disabled)
            .ok_or_else(ApiError::unauthorized)?
            .revision;
        let nonce = crypto::random()?;
        self.sessions
            .get_mut(&session)
            .ok_or_else(ApiError::unauthorized)?
            .challenge = Some(Challenge {
            id: id.clone(),
            token,
            nonce,
            revision,
        });
        Ok(
            json!({"version":1,"type":"auth_challenge","request_id":id,"service_id":self.config.service.id,"nonce":crypto::encode(&nonce)}),
        )
    }
    fn proof(
        &mut self,
        session: u64,
        id: String,
        auth_id: String,
        signature: String,
        now: u64,
    ) -> Result<Value> {
        let s = self
            .sessions
            .get_mut(&session)
            .ok_or_else(ApiError::unauthorized)?;
        let c = s.challenge.take().ok_or_else(ApiError::unauthorized)?;
        if s.auth.is_some()
            || now
                >= s.started
                    .saturating_add(self.config.limits.auth_timeout_seconds * 1000)
            || c.id != auth_id
        {
            return Err(ApiError::unauthorized());
        }
        let key = self
            .keys
            .get(&c.token)
            .filter(|k| k.until > now)
            .ok_or_else(ApiError::unauthorized)?;
        if self
            .clients
            .get(&key.client)
            .is_none_or(|s| s.disabled || s.revision != c.revision)
        {
            return Err(ApiError::unauthorized());
        }
        crypto::verify(
            &key.public,
            &crypto::auth_bytes(&c.nonce, &key.public, &self.config.service.id),
            &crypto::decode(&signature).map_err(|_| ApiError::unauthorized())?,
        )?;
        s.auth = Some(Auth {
            client: key.client.clone(),
            token: c.token,
            revision: c.revision,
        });
        Ok(
            json!({"version":1,"type":"auth_welcome","request_id":id,"client_id":key.client,"service_id":self.config.service.id,"restart_counter":self.restart_counter}),
        )
    }
    fn publish(
        &mut self,
        client: &str,
        id: &str,
        topic: &str,
        payload: Value,
        expires: Option<protocol::Expires>,
        now: u64,
    ) -> Result<Value> {
        if depth(&payload) > 32 {
            return Err(ApiError::invalid());
        }
        let payload = Payload::from_json(payload)?;
        let slot = self
            .clients
            .get(client)
            .ok_or_else(ApiError::unauthorized)?
            .slot;
        let result = self.publish_typed(slot, id, topic, payload, expires, now)?;
        pb::Frame {
            version: 1,
            body: Some(pb::Body::Result(result)),
        }
        .to_json()
    }
    fn publish_typed(
        &mut self,
        client_slot: u16,
        id: &str,
        topic: &str,
        payload: Payload,
        expires: Option<protocol::Expires>,
        now: u64,
    ) -> Result<pb::Result> {
        let now = now.max(self.last_cleanup_now);
        let t = self.topics.get(topic).ok_or_else(|| {
            ApiError::new(
                "owner_unavailable",
                "Topic is not owned by this local service.",
            )
        })?;
        if !self.acl.allows(client_slot, t.slot, Action::Publish) {
            return Err(ApiError::new(
                "forbidden",
                "Operation not permitted on this topic.",
            ));
        }
        payload.validate()?;
        let bytes = payload.value_len();
        if bytes > self.config.limits.max_payload_bytes {
            return Err(ApiError::invalid());
        }
        let until = match expires {
            Some(e) => protocol::parse_time(&e.time)?,
            None => now.saturating_add(t.policy.default_ttl_seconds * 1000),
        };
        if until <= now || until > now.saturating_add(t.policy.max_ttl_seconds * 1000) {
            return Err(ApiError::invalid());
        }
        let external = match payload.body.as_ref() {
            Some(protocol::payload::Body::Data(data)) => data.len(),
            _ => 0,
        };
        let charge = std::mem::size_of::<Stored>()
            + 128
            + 64
            + external
            + payload.content_type.as_ref().map_or(0, String::len);
        if self.active_bytes.saturating_add(charge) > self.config.limits.max_active_message_bytes
            || self.index.len() >= self.config.limits.max_active_messages
        {
            return Err(ApiError::exhausted());
        }
        let global = self
            .next_message
            .checked_add(1)
            .ok_or_else(ApiError::exhausted)?;
        let seq = t.head.checked_add(1).ok_or_else(ApiError::exhausted)?;
        let raw_id = self.message_ids.take().ok_or_else(ApiError::exhausted)?;
        let mid = Id(raw_id);
        // A wrapped range must never overwrite an active message.
        if self.index.contains_key(&mid) {
            return Err(ApiError::exhausted());
        }
        let expiry = pb::Expires {
            time: protocol::timestamp(until)?,
        };
        let result = pb::Result {
            request_id: id.into(),
            message_id: Some(mid.to_string()),
            expires: Some(expiry),
        };
        let client = self
            .client_names
            .get(usize::from(client_slot))
            .ok_or_else(ApiError::internal)?
            .as_str();
        let producer = client_slot;
        let topic_slot = t.slot;
        let own_pos = self
            .clients
            .get(client)
            .ok_or_else(ApiError::internal)?
            .produced
            .checked_add(1)
            .ok_or_else(ApiError::exhausted)?;
        let stored = Stored::new(
            RecordFields {
                id: mid,
                producer,
                topic: topic_slot,
                created: now,
                until,
                own_pos,
                seq,
            },
            StoredPayload::new(payload, &mut self.content_types, &mut self.payloads)?,
        );
        debug_assert_eq!(charge, stored.charge(&self.content_types, &self.payloads));
        let slot = match self.records.insert(stored) {
            Ok(slot) => slot,
            Err(record) => {
                record
                    .payload()
                    .release(&mut self.content_types, &mut self.payloads);
                return Err(ApiError::exhausted());
            }
        };
        let t = self.topics.get_mut(topic).ok_or_else(ApiError::internal)?;
        t.head = seq;
        t.messages.insert(seq, slot);
        self.next_message = global;
        self.clients
            .get_mut(client)
            .ok_or_else(ApiError::internal)?
            .produced = own_pos;
        self.active_bytes += charge;
        if let Some(content) = &mut self.content {
            content.changed(t.slot);
        }
        self.expiry.insert((until, slot));
        self.index.insert(mid, slot);
        self.own
            .get_mut(client)
            .ok_or_else(ApiError::internal)?
            .insert(own_pos, slot);
        Ok(result)
    }
    fn poll(
        &mut self,
        session: u64,
        id: &str,
        topic: &str,
        after: Option<&str>,
        limit: Option<usize>,
        now: u64,
    ) -> Result<Value> {
        let response = self.poll_typed(session, id, topic, after, limit, now, false, None)?;
        pb::Frame {
            version: 1,
            body: Some(pb::Body::Messages(response)),
        }
        .to_json()
    }
    #[allow(clippy::too_many_arguments)]
    fn poll_typed(
        &mut self,
        session: u64,
        id: &str,
        topic: &str,
        after: Option<&str>,
        limit: Option<usize>,
        now: u64,
        binary: bool,
        output: Option<&mut bytes::BytesMut>,
    ) -> Result<pb::Messages> {
        self.poll_validated(
            self.identity_slot(session, now)?,
            id,
            topic,
            after,
            limit,
            now,
            binary,
            output,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn poll_validated(
        &mut self,
        client_slot: u16,
        id: &str,
        topic: &str,
        after: Option<&str>,
        limit: Option<usize>,
        now: u64,
        binary: bool,
        output: Option<&mut bytes::BytesMut>,
    ) -> Result<pb::Messages> {
        let mut scratch = std::mem::take(&mut self.poll_scratch);
        scratch.clear();
        let result = self.poll_scratch_inner(
            client_slot,
            id,
            topic,
            after,
            limit,
            now,
            binary,
            output,
            &mut scratch,
        );
        scratch.clear();
        self.poll_scratch = scratch;
        result
    }
    #[allow(clippy::too_many_arguments)]
    fn poll_scratch_inner(
        &mut self,
        client_slot: u16,
        id: &str,
        topic: &str,
        after: Option<&str>,
        limit: Option<usize>,
        now: u64,
        binary: bool,
        output: Option<&mut bytes::BytesMut>,
        selected: &mut Vec<PollEntry>,
    ) -> Result<pb::Messages> {
        let now = now.max(self.last_cleanup_now);
        let t = self.topics.get(topic).ok_or_else(|| {
            ApiError::new(
                "owner_unavailable",
                "Topic is not owned by this local service.",
            )
        })?;
        if !self.acl.allows(client_slot, t.slot, Action::Subscribe) {
            return Err(ApiError::new(
                "forbidden",
                "Operation not permitted on this topic.",
            ));
        }
        let limit = self.limit(limit)?;
        let start = match after {
            None => 0,
            Some(text) => crate::cursor::decode(text, t.head).unwrap_or(0),
        };
        if start == t.head {
            let next_cursor = crate::cursor::encode(t.head);
            return Ok(pb::Messages {
                request_id: id.into(),
                items: Vec::new(),
                next_cursor,
                has_more: false,
            });
        }
        let mut items = Vec::new();
        if selected.capacity() < limit {
            selected.reserve_exact(limit);
        }
        let mut encoded_items_len = 0usize;
        let mut position = start;
        let mut used = 4096usize;
        let mut cached_expiry: Option<(u64, protocol::Timestamp)> = None;
        let mut scanned = 0usize;
        let mut iter = t
            .messages
            .range((std::ops::Bound::Excluded(start), std::ops::Bound::Unbounded))
            .peekable();
        while selected.len() < limit && scanned < READ_SCAN_LIMIT {
            let Some((seq, slot)) = iter.peek().copied() else {
                break;
            };
            let s = self.records.get(slot).ok_or_else(ApiError::internal)?;
            scanned += 1;
            if s.until() <= now {
                iter.next();
                position = seq;
                continue;
            }
            if cached_expiry
                .as_ref()
                .is_none_or(|(until, _)| *until != s.until())
            {
                cached_expiry = Some((s.until(), protocol::Timestamp::new(s.until())?));
            }
            let expiry = &cached_expiry.as_ref().ok_or_else(ApiError::internal)?.1;
            let producer = self
                .client_names
                .get(s.producer_slot())
                .ok_or_else(ApiError::internal)?
                .as_str();
            let item = if output.is_none() {
                Some(pb::PolledMessage {
                    message: s.wire(
                        (topic, producer),
                        &self.content_types,
                        &self.payloads,
                        expiry.as_str().to_owned(),
                    ),
                })
            } else {
                None
            };
            let polled_len = if binary {
                s.polled_len(
                    (topic, producer),
                    &self.content_types,
                    &self.payloads,
                    expiry,
                )
            } else {
                0
            };
            let size = if binary {
                polled_len + 8
            } else {
                serde_json::to_vec(item.as_ref().ok_or_else(ApiError::internal)?)
                    .map_err(|_| ApiError::internal())?
                    .len()
                    + 1
            };
            if used.saturating_add(size) > self.config.limits.max_response_bytes {
                break;
            }
            iter.next();
            used += size;
            position = seq;

            selected.push((s.until(), slot));
            encoded_items_len += crate::direct::field_len(2, polled_len);
            if let Some(item) = item {
                items.push(item);
            }
        }
        let more = iter.peek().is_some();
        if more && selected.is_empty() && position == start {
            return Err(ApiError::exhausted());
        }
        if !more {
            position = t.head;
        }
        let cursor = crate::cursor::encode(position);
        let response = pb::Messages {
            request_id: id.into(),
            items,
            next_cursor: cursor,
            has_more: more,
        };
        let response_size = if binary {
            response.encoded_len()
                + if output.is_some() {
                    encoded_items_len
                } else {
                    0
                }
                + 16
        } else {
            serde_json::to_vec(&response)
                .map_err(|_| ApiError::internal())?
                .len()
                + 64
        };
        if response_size > self.config.limits.max_response_bytes {
            return Err(ApiError::exhausted());
        }
        if let Some(out) = output {
            let len = response.encoded_len() + encoded_items_len;
            out.reserve(2 + crate::direct::field_len(11, len));
            crate::direct::number(1, 1, out);
            crate::direct::header(11, len, out);
            crate::direct::data(1, id.as_bytes(), out);
            cached_expiry = None;
            for (until, slot) in selected.iter() {
                if cached_expiry.as_ref().is_none_or(|(time, _)| time != until) {
                    cached_expiry = Some((*until, protocol::Timestamp::new(*until)?));
                }
                let expiry = &cached_expiry.as_ref().ok_or_else(ApiError::internal)?.1;
                let record = self.records.get(*slot).ok_or_else(ApiError::internal)?;
                let producer = self
                    .client_names
                    .get(record.producer_slot())
                    .ok_or_else(ApiError::internal)?
                    .as_str();
                crate::direct::header(
                    2,
                    record.polled_len(
                        (topic, producer),
                        &self.content_types,
                        &self.payloads,
                        expiry,
                    ),
                    out,
                );
                record.encode_polled(
                    (topic, producer),
                    &self.content_types,
                    &self.payloads,
                    expiry,
                    out,
                );
            }
            crate::direct::data(3, response.next_cursor.as_bytes(), out);
            crate::direct::number(4, u64::from(response.has_more), out);
        }
        Ok(response)
    }
    fn limit(&self, limit: Option<usize>) -> Result<usize> {
        let n = limit.unwrap_or(self.config.limits.poll_page_items);
        if n == 0 || n > self.config.limits.poll_page_max_items {
            Err(ApiError::invalid())
        } else {
            Ok(n)
        }
    }
    #[cfg(test)]
    fn allowed(&self, client: &str, topic: &str, action: Action) -> bool {
        let Some(client) = self.clients.get(client) else {
            return false;
        };
        let Some(topic) = self.topics.get(topic) else {
            return false;
        };
        self.acl.allows(client.slot, topic.slot, action)
    }
    fn get(&self, client: &str, id: &str, mid: &str, now: u64) -> Result<Value> {
        pb::Frame {
            version: 1,
            body: Some(pb::Body::MessageStatus(
                self.get_typed(client, id, mid, now)?,
            )),
        }
        .to_json()
    }
    fn get_typed(&self, client: &str, id: &str, mid: &str, now: u64) -> Result<pb::MessageStatus> {
        let now = now.max(self.last_cleanup_now);
        let mid = Id::parse(mid).ok_or_else(not_found)?;
        let s = self
            .index
            .get(&mid)
            .and_then(|&slot| self.records.get(slot))
            .filter(|s| {
                s.until() > now
                    && self
                        .client_names
                        .get(s.producer_slot())
                        .is_some_and(|name| name == client)
            })
            .ok_or_else(not_found)?;
        let topic = self
            .topic_names
            .get(s.topic() as usize)
            .ok_or_else(ApiError::internal)?;
        let message = s.wire(
            (topic, client),
            &self.content_types,
            &self.payloads,
            protocol::timestamp(s.until())?,
        );
        Ok(pb::MessageStatus {
            request_id: id.into(),
            message,
            created_at: protocol::timestamp(s.created_time())?,
        })
    }
    fn list(
        &self,
        client: &str,
        id: &str,
        topic: Option<String>,
        limit: Option<usize>,
        cursor: Option<String>,
        now: u64,
    ) -> Result<Value> {
        pb::Frame {
            version: 1,
            body: Some(pb::Body::MessagesPage(
                self.list_typed(client, id, topic, limit, cursor, now, false)?,
            )),
        }
        .to_json()
    }
    #[allow(clippy::too_many_arguments)]
    fn list_typed(
        &self,
        client: &str,
        id: &str,
        topic: Option<String>,
        limit: Option<usize>,
        cursor: Option<String>,
        now: u64,
        binary: bool,
    ) -> Result<pb::MessagesPage> {
        let now = now.max(self.last_cleanup_now);
        let revision = self
            .clients
            .get(client)
            .ok_or_else(ApiError::unauthorized)?
            .revision;
        if topic
            .as_ref()
            .is_some_and(|t| t.is_empty() || t.len() > 256)
        {
            return Err(ApiError::invalid());
        }
        let limit = self.limit(limit)?;
        let (start, watermark, until) = if let Some(text) = cursor {
            let c: Cursor = self.codec.open(&text)?;
            if c.kind != "list"
                || c.generation != self.generation
                || c.client != client
                || c.topic != topic
                || c.limit != Some(limit)
                || c.revision != Some(revision)
                || c.until.is_none_or(|t| t <= now)
            {
                return Err(bad_cursor());
            }
            (
                c.pos,
                c.watermark.ok_or_else(bad_cursor)?,
                c.until.ok_or_else(bad_cursor)?,
            )
        } else {
            (
                0,
                self.clients
                    .get(client)
                    .ok_or_else(ApiError::unauthorized)?
                    .produced,
                now.saturating_add(60_000),
            )
        };
        let mut items = Vec::new();
        let mut pos = start;
        let mut more = false;
        let mut bytes = 4096usize;
        let own = self.own.get(client).ok_or_else(ApiError::internal)?;
        for (scanned, (global, slot)) in own
            .range((
                std::ops::Bound::Excluded(start),
                std::ops::Bound::Included(watermark),
            ))
            .enumerate()
        {
            if scanned == READ_SCAN_LIMIT {
                more = true;
                break;
            }
            let s = self.records.get(slot).ok_or_else(ApiError::internal)?;
            if s.until() <= now {
                pos = global;
                continue;
            }
            let name = self
                .topic_names
                .get(s.topic() as usize)
                .ok_or_else(ApiError::internal)?;
            if topic.as_ref().is_some_and(|t| t != name) {
                pos = global;
                continue;
            }
            let v = pb::MessageSummary {
                id: s.id().to_string(),
                topic: name.clone(),
                created_at: protocol::timestamp(s.created_time())?,
                expires: pb::Expires {
                    time: protocol::timestamp(s.until())?,
                },
            };
            let size = if binary {
                v.encoded_len() + 8
            } else {
                serde_json::to_vec(&v)
                    .map_err(|_| ApiError::internal())?
                    .len()
                    + 1
            };
            if items.len() >= limit
                || bytes.saturating_add(size) > self.config.limits.max_response_bytes
            {
                more = true;
                break;
            }
            items.push(v);
            bytes += size;
            pos = global;
        }
        let mut response = pb::MessagesPage {
            request_id: id.into(),
            items,
            next_cursor: None,
        };
        if more {
            if response.items.is_empty() && pos == start {
                return Err(ApiError::exhausted());
            }
            response.next_cursor = Some(self.codec.seal(&Cursor {
                kind: "list".into(),
                generation: self.generation.clone(),
                client: client.into(),
                topic,
                pos,
                limit: Some(limit),
                watermark: Some(watermark),
                until: Some(until),
                revision: Some(revision),
            })?);
        }
        Ok(response)
    }
    fn rotation_start(
        &mut self,
        session: u64,
        client: &str,
        token: &str,
        revision: u64,
        id: &str,
        now: u64,
    ) -> Result<Value> {
        if self
            .rotations
            .get(client)
            .is_some_and(|rotation| rotation.until <= now)
        {
            self.take_rotation(client);
        }
        if self.rotations.contains_key(client) {
            return Err(ApiError::new(
                "rotation_in_progress",
                "A rotation is already active for this client and owner.",
            ));
        }
        let c = self
            .clients
            .get_mut(client)
            .ok_or_else(ApiError::unauthorized)?;
        c.rotation_attempts
            .retain(|t| now.saturating_sub(*t) < self.config.rotation.window_seconds * 1000);
        if c.rotation_attempts.len() >= self.config.rotation.attempts_per_client as usize {
            let remaining = c
                .rotation_attempts
                .first()
                .map(|t| {
                    t.saturating_add(self.config.rotation.window_seconds * 1000)
                        .saturating_sub(now)
                })
                .unwrap_or(1);
            return Err(ApiError::limited(remaining));
        }
        c.rotation_attempts.push(now);
        let key = self.keys.get(token).ok_or_else(ApiError::unauthorized)?;
        let until = now
            .saturating_add(self.config.rotation.timeout_seconds * 1000)
            .min(key.until);
        let new_until = now.saturating_add(self.config.rotation.key_validity_seconds * 1000);
        let rid = crypto::encode(&crypto::random::<24>()?);
        self.rotation_expiry.insert((until, client.into()));
        self.rotations.insert(
            client.into(),
            Rotation {
                id: rid.clone(),
                session,
                old_token: token.into(),
                revision,
                until,
                new_until,
                secret: None,
                nonce: crypto::random()?,
                challenge: crypto::random()?,
            },
        );
        Ok(
            json!({"version":1,"type":"rotation_offer","request_id":id,"rotation_id":rid,"expires_at":protocol::timestamp(until)?,"new_valid_until":protocol::timestamp(new_until)?}),
        )
    }
    fn rotation_confirm(
        &mut self,
        session: u64,
        id: &str,
        rid: &str,
        shared_secret: &str,
        now: u64,
    ) -> Result<Value> {
        let (client, _, _) = self.identity(session, now)?;
        let client = client.as_str();
        let r = self
            .rotations
            .get_mut(client)
            .filter(|r| r.session == session && r.id == rid && r.until > now && r.secret.is_none())
            .ok_or_else(|| {
                ApiError::new(
                    "rotation_invalid",
                    "Rotation is absent, expired or in the wrong phase.",
                )
            })?;
        let secret = Zeroizing::new(crypto::decode::<32>(shared_secret)?);
        let aad = rotation_aad(
            &self.config.service.id,
            client,
            &r.id,
            &r.old_token,
            r.new_until,
        );
        let response = json!({"version":1,"type":"rotation_ready","request_id":id,"rotation_id":rid,"nonce":crypto::encode(&r.nonce),"challenge":crypto::encode(&r.challenge),"aad":crypto::encode(&aad),"new_valid_until":protocol::timestamp(r.new_until)?,"expires_at":protocol::timestamp(r.until)?});
        r.secret = Some(secret);
        Ok(response)
    }
    fn rotation_commit(
        &mut self,
        session: u64,
        id: &str,
        rid: &str,
        encrypted: &str,
        signature: &str,
        now: u64,
    ) -> Result<Value> {
        let (client, token, revision) = self.identity(session, now)?;
        let client = client.as_str();
        // Consume only a matching transaction. A failed proof cannot be retried with the same secret/nonce.
        if self
            .rotations
            .get(client)
            .is_none_or(|r| r.id != rid || r.session != session)
        {
            return Err(ApiError::new(
                "rotation_invalid",
                "Rotation is absent or belongs to another session.",
            ));
        }
        let r = self.take_rotation(client).ok_or_else(ApiError::internal)?;
        if r.until <= now || token != r.old_token || revision != r.revision {
            return Err(ApiError::unauthorized());
        }
        let aad = rotation_aad(
            &self.config.service.id,
            client,
            &r.id,
            &r.old_token,
            r.new_until,
        );
        let secret = r.secret.as_ref().ok_or_else(ApiError::invalid)?;
        let public = crypto::decrypt_rotation(secret, r.nonce, &aad, encrypted)?;
        let token = crypto::token_id(&public);
        if self.keys.contains_key(&token) {
            return Err(ApiError::invalid());
        }
        crypto::verify(
            &public,
            &rotation_proof_bytes(&aad, &public, &r.challenge),
            &crypto::decode(signature).map_err(|_| ApiError::unauthorized())?,
        )?;
        let permit = self.security_reserve()?;
        let mut candidate = self.security_client(client)?;
        let old_public = crypto::encode(
            &self
                .keys
                .get(&r.old_token)
                .ok_or_else(ApiError::unauthorized)?
                .public,
        );
        candidate.keys.retain(|key| key.public_key != old_public);
        candidate.keys.push(crate::config::PublicKey {
            algorithm: "ed25519".into(),
            public_key: crypto::encode(&public),
            valid_until: protocol::timestamp(r.new_until)?,
        });
        self.security_check(&crate::security::Change::Client { value: candidate })?;
        // Other sessions cannot continue using the removed credential.
        let old_sessions: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|(id, state)| {
                (id != &session
                    && state
                        .auth
                        .as_ref()
                        .is_some_and(|auth| auth.token == r.old_token))
                .then_some(*id)
            })
            .collect();
        for old in old_sessions {
            self.disconnect(old);
        }
        // No await or external call between the final old-credential check and replacement.
        self.keys.remove(&r.old_token);
        self.keys.insert(
            token.clone(),
            Credential {
                client: client.into(),
                public,
                until: r.new_until,
            },
        );
        self.sessions
            .get_mut(&session)
            .ok_or_else(ApiError::internal)?
            .auth = Some(Auth {
            client: client.into(),
            token: token.clone(),
            revision,
        });
        let value = self.security_client(client)?;
        self.security_submit(permit, crate::security::Change::Client { value })?;
        Ok(
            json!({"version":1,"type":"rotation_complete","request_id":id,"rotation_id":rid,"client_id":client,"token_id":token,"valid_until":protocol::timestamp(r.new_until)?}),
        )
    }
}
fn not_found() -> ApiError {
    ApiError::new("not_found", "Own active message not found.")
}
fn bad_cursor() -> ApiError {
    ApiError::new(
        "invalid_cursor",
        "Cursor scope, generation or position is invalid.",
    )
}
fn depth(v: &Value) -> usize {
    match v {
        Value::Array(a) => 1 + a.iter().map(depth).max().unwrap_or(0),
        Value::Object(o) => 1 + o.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}
pub use mynofy_protocol::crypto::{rotation_aad, rotation_proof_bytes};

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;

#[path = "engine_admin.rs"]
mod administration;
#[path = "engine_peer_admin.rs"]
mod peer_administration;

#[path = "engine_security.rs"]
mod security_state;

fn content_error(error: crate::BoxError) -> ApiError {
    eprintln!("mynofy: content startup failed: {error}");
    ApiError::new(
        "content_storage_error",
        "Content could not be safely restored; see service stderr.",
    )
}
#[path = "engine_content.rs"]
mod content_state;

#[path = "engine_topics.rs"]
mod topic_administration;
