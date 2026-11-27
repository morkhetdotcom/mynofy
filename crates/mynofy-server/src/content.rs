//! Content snapshots are opt-in; only startup can load state. Security is independent.
use crate::{
    BoxError,
    config::{Config, ContentEncryption, ContentStorage, TopicStorage},
    crypto,
    protocol::{ApiError, Result},
};
use serde::Serialize;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Instant,
};
use zeroize::Zeroizing;
/// Read from stdin by a launcher. Never serialized or included in diagnostics.
pub enum ContentSecret {
    Password(Zeroizing<Vec<u8>>),
    #[doc(hidden)]
    PreparedPassword(encryption::PreparedPassword),
    Key(Zeroizing<[u8; 32]>),
}
impl ContentSecret {
    fn matches(&self, mode: ContentEncryption) -> bool {
        matches!(
            (self, mode),
            (
                Self::Password(_) | Self::PreparedPassword(_),
                ContentEncryption::PasswordStdin
            ) | (Self::Key(_), ContentEncryption::KeyStdin)
        )
    }
}
#[derive(Clone, Default, Serialize)]
pub(crate) struct Status {
    pub topics: std::collections::BTreeMap<String, TopicStatus>,
    pub enabled: bool,
    pub encrypted: bool,
    pub loaded: bool,
    pub revision: u64,
    pub saved_revision: u64,
    pub dirty: bool,
    pub save_in_progress: bool,
    pub working_memory_reserved_bytes: usize,
    pub requires_restart: bool,
    pub last_save_time_ms: Option<u64>,
    pub last_writer_duration_us: Option<u64>,
    pub last_error: Option<String>,
    pub cleanup_error: Option<String>,
}
#[derive(Clone, Serialize)]
pub(crate) struct TopicStatus {
    pub storage: TopicStorage,
    pub revision: u64,
    pub saved_revision: u64,
    pub dirty: bool,
    pub loaded: bool,
    pub autosave_isolated: bool,
    pub save_failure: Option<SaveFailure>,
}
#[derive(Clone, Serialize)]
pub(crate) struct SaveFailure {
    revision: u64,
    error: String,
}
fn is_capacity_failure(error: &str) -> bool {
    error.starts_with("content_memory_budget_exceeded:")
        || error.starts_with("resource_exhausted:")
        || error == "Topic content exceeds aggregate size limit"
        || error == "Configured capacity reached."
        || error.contains("exceeds configured size limit")
}
struct Schedule {
    name: String,
    storage: TopicStorage,
    revision: u64,
    next: Instant,
    retry_at: Option<Instant>,
    retry_delay: u64,
    attempted_revision: Option<u64>,
    isolated_retry: bool,
}
struct Save {
    automatic: bool,
    security: Option<crate::security::DurabilityFence>,
    capture: checkpoint::Capture,
    selected: Vec<(String, TopicStorage)>,
}
struct Shared {
    status: Mutex<Status>,
    busy: AtomicBool,
    automatic_capacity_failure: AtomicBool,
}
impl Shared {
    fn snapshot(&self) -> Status {
        // A completed save publishes its metadata before releasing busy. Acquire
        // that flag BEFORE cloning metadata, never combine an old clone with a
        // newly cleared flag. An in-progress snapshot may be conservative.
        let save_in_progress = self.busy.load(Ordering::Acquire);
        let mut status = self.status().clone();
        status.save_in_progress = save_in_progress;
        if !save_in_progress {
            status.working_memory_reserved_bytes = 0;
        }
        status
    }
    fn status(&self) -> std::sync::MutexGuard<'_, Status> {
        self.status
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}
pub(crate) struct ContentRuntime {
    sender: Option<mpsc::SyncSender<Save>>,
    worker: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
    schedules: Vec<Schedule>,
    automatic: Vec<usize>,
    last_automatic: Option<usize>,
    retry: crate::config::AutosaveRetry,
    pub revision: u64,
    pub maximum: u64,
}
impl ContentRuntime {
    pub(crate) fn topics_idle(&self) -> Result<()> {
        if self.shared.busy.load(Ordering::Acquire) {
            return Err(ApiError::new(
                "content_save_busy",
                "Wait for the pending save before changing topics.",
            ));
        }
        if self.shared.status().requires_restart {
            return Err(ApiError::new(
                "content_storage_requires_restart",
                "Reopen content storage before changing topics.",
            ));
        }
        Ok(())
    }
    pub(crate) fn add_ram_topic(&mut self, name: String) {
        self.schedules.push(Schedule {
            name,
            storage: TopicStorage::default(),
            revision: self.revision,
            retry_at: None,
            retry_delay: 0,
            attempted_revision: None,
            isolated_retry: false,
            next: Instant::now(),
        });
    }
    pub(crate) fn remove_ram_topic(&mut self, slot: usize) {
        self.schedules.swap_remove(slot);
        self.automatic = self
            .schedules
            .iter()
            .enumerate()
            .filter(|(_, s)| s.storage.interval().is_some())
            .map(|(i, _)| i)
            .collect();
    }

    pub(crate) fn enable(&mut self, slot: u16, storage: TopicStorage) -> Result<()> {
        self.topics_idle()?;
        let schedule = self
            .schedules
            .get_mut(slot as usize)
            .ok_or_else(ApiError::invalid)?;
        if schedule.storage.persistent() || !storage.persistent() {
            return Err(ApiError::new(
                "storage_transition_forbidden",
                "Only RAM to storage is allowed online.",
            ));
        }
        schedule.storage = storage.clone();
        schedule.next =
            Instant::now() + std::time::Duration::from_secs(storage.interval().unwrap_or(1));
        self.revision = self.revision.saturating_add(1);
        schedule.revision = self.revision;
        if storage.interval().is_some() {
            self.automatic.push(slot as usize);
        }
        self.shared.status().topics.insert(
            schedule.name.clone(),
            TopicStatus {
                storage,
                revision: self.revision,
                saved_revision: 0,
                dirty: true,
                loaded: false,
                autosave_isolated: false,
                save_failure: None,
            },
        );
        Ok(())
    }
    pub(crate) fn changed(&mut self, topic: u16) {
        if let Some(schedule) = self.schedules.get_mut(topic as usize)
            && schedule.storage.persistent()
        {
            self.revision = self.revision.saturating_add(1);
            schedule.revision = self.revision;
        }
    }
    pub(crate) fn topic_status(&self, name: &str, slot: u16) -> Option<TopicStatus> {
        let mut topic = self.shared.status().topics.get(name)?.clone();
        Self::update_topic_status(&mut topic, self.schedules.get(usize::from(slot))?);
        Some(topic)
    }

    fn update_topic_status(topic: &mut TopicStatus, schedule: &Schedule) {
        topic.revision = schedule.revision;
        topic.autosave_isolated = schedule.isolated_retry;
        topic.dirty = topic.revision > topic.saved_revision || topic.revision == u64::MAX;
    }

    pub(crate) fn status(&self) -> Status {
        let mut status = self.shared.snapshot();
        status.revision = self.revision;
        for schedule in self.schedules.iter().filter(|s| s.storage.persistent()) {
            if let Some(topic) = status.topics.get_mut(&schedule.name) {
                Self::update_topic_status(topic, schedule);
            }
        }
        status.dirty = status.topics.values().any(|topic| topic.dirty);
        status
    }
    #[cfg(test)]
    pub(crate) fn force_due(&mut self) {
        for schedule in &mut self.schedules {
            schedule.next = Instant::now();
            schedule.retry_at = None;
            schedule.attempted_revision = None;
        }
    }
    pub(crate) fn due(&mut self) -> Vec<(String, TopicStorage)> {
        if self.shared.busy.load(Ordering::Acquire) || self.automatic.is_empty() {
            return Vec::new();
        }
        let status = self.shared.status();
        if status.requires_restart {
            return Vec::new();
        }
        let now = Instant::now();
        let capacity_failure = self
            .shared
            .automatic_capacity_failure
            .load(Ordering::Relaxed);
        let mut ready = Vec::new();
        for &slot in &self.automatic {
            let schedule = &mut self.schedules[slot];
            let Some(seconds) = schedule.storage.interval() else {
                continue;
            };
            let saved = status.topics[&schedule.name].saved_revision;
            if let Some(revision) = schedule.attempted_revision.take() {
                schedule.next = now + std::time::Duration::from_secs(seconds);
                if saved >= revision {
                    schedule.retry_at = None;
                    schedule.retry_delay = 0;
                    schedule.isolated_retry = false;
                } else if self.retry.enabled {
                    schedule.isolated_retry |= capacity_failure;
                    schedule.retry_delay = if schedule.retry_delay == 0 {
                        self.retry.initial_delay_seconds
                    } else {
                        schedule
                            .retry_delay
                            .saturating_mul(2)
                            .min(self.retry.max_delay_seconds)
                    };
                    schedule.retry_at =
                        Some(now + std::time::Duration::from_secs(schedule.retry_delay));
                }
            }
            let deadline = schedule.retry_at.unwrap_or(schedule.next);
            if now >= deadline && (schedule.revision > saved || schedule.revision == u64::MAX) {
                ready.push((slot, deadline));
            }
        }
        let Some(&(first, _)) = ready.iter().min_by_key(|&&(slot, deadline)| {
            (
                deadline,
                self.last_automatic.is_some_and(|last| slot <= last),
                slot,
            )
        }) else {
            return Vec::new();
        };
        let isolated = self.schedules[first].isolated_retry;
        if isolated {
            self.last_automatic = Some(first);
        }
        let mut due = Vec::new();
        for (slot, _) in ready {
            let schedule = &mut self.schedules[slot];
            if slot != first && (isolated || schedule.isolated_retry) {
                continue;
            }
            schedule.retry_at = None;
            schedule.next = now
                + std::time::Duration::from_secs(
                    schedule
                        .storage
                        .interval()
                        .expect("automatic topic has an interval"),
                );
            schedule.attempted_revision = Some(schedule.revision);
            due.push((schedule.name.clone(), schedule.storage.clone()));
        }
        due
    }
    pub(crate) fn selection(
        &self,
        requested: Option<&[String]>,
    ) -> Result<Vec<(String, TopicStorage)>> {
        let requested_names =
            requested.map(|names| names.iter().collect::<std::collections::HashSet<_>>());
        let selected: Vec<_> = self
            .schedules
            .iter()
            .filter(|s| {
                s.storage.persistent()
                    && requested_names
                        .as_ref()
                        .is_none_or(|names| names.contains(&s.name))
            })
            .map(|s| (s.name.clone(), s.storage.clone()))
            .collect();
        if let Some(names) = requested
            && selected.len() != names.len()
        {
            return Err(ApiError::new(
                "invalid_content_topics",
                "Choose distinct locally persistent topics.",
            ));
        }
        if selected.is_empty() {
            return Err(ApiError::new(
                "invalid_content_topics",
                "No topics have a persistence policy.",
            ));
        }
        Ok(selected)
    }
    pub(crate) fn reserve(&self) -> Result<()> {
        self.shared
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                ApiError::new("content_save_busy", "A content save is already pending.")
            })?;
        if self.shared.status().requires_restart {
            self.cancel();
            return Err(ApiError::new(
                "content_storage_requires_restart",
                "Content commit is uncertain; restart before another save.",
            ));
        }
        Ok(())
    }
    pub(crate) fn reserve_working(&self, bytes: usize) {
        self.shared.status().working_memory_reserved_bytes = bytes;
    }
    pub(crate) fn capture_failed(&self, error: &ApiError, automatic: bool) {
        let message = format!("{}: {}", error.code, error.detail);
        if automatic {
            self.shared
                .automatic_capacity_failure
                .store(is_capacity_failure(&message), Ordering::Relaxed);
        }
        self.shared.status().last_error = Some(message);
        self.cancel();
    }
    pub(crate) fn cancel(&self) {
        self.shared.busy.store(false, Ordering::Release);
    }
    pub(crate) fn submit(
        &self,
        capture: checkpoint::Capture,
        selected: Vec<(String, TopicStorage)>,
        security: Option<crate::security::DurabilityFence>,
        automatic: bool,
    ) -> Result<()> {
        if self.sender.as_ref().is_none_or(|sender| {
            sender
                .try_send(Save {
                    automatic,
                    security,
                    capture,
                    selected,
                })
                .is_err()
        }) {
            if automatic {
                self.shared
                    .automatic_capacity_failure
                    .store(false, Ordering::Relaxed);
            }
            self.shared.status().last_error = Some("Content writer is unavailable".into());
            self.cancel();
            return Err(ApiError::new(
                "content_save_unavailable",
                "Content writer is unavailable.",
            ));
        }
        Ok(())
    }
}
impl ContentRuntime {
    pub(crate) fn drain(&mut self) -> std::result::Result<(), BoxError> {
        self.sender.take();
        let panicked = self
            .worker
            .take()
            .is_some_and(|worker| worker.join().is_err());
        let status = self.shared.status();
        if panicked || self.shared.busy.load(Ordering::Acquire) {
            return Err(
                "content_drain_failed: writer stopped before completing the queued save".into(),
            );
        }
        let failures: Vec<_> = status
            .topics
            .iter()
            .filter_map(|(name, topic)| {
                let failure = topic.save_failure.as_ref()?;
                Some(format!("{name}@{}: {}", failure.revision, failure.error))
            })
            .collect();
        if !failures.is_empty() {
            let revision = status
                .topics
                .values()
                .filter_map(|topic| topic.save_failure.as_ref())
                .map(|failure| failure.revision)
                .max()
                .unwrap_or(0);
            return Err(format!("content_drain_failed: queued_revision={revision} saved_revision={} requires_restart={}: {}", status.saved_revision, status.requires_restart, failures.join("; ")).into());
        }
        if status.requires_restart {
            return Err("content_drain_failed: manifest durability is uncertain".into());
        }
        Ok(())
    }
}
impl Drop for ContentRuntime {
    fn drop(&mut self) {
        let _ = self.drain();
    }
}
pub(crate) mod checkpoint;
mod encryption;
mod topics;
pub(crate) use checkpoint::Checkpoint;
#[cfg(unix)]
mod disk;
pub(crate) struct Startup {
    pub loaded: Option<Checkpoint>,
    entries: topics::Entries,
    #[cfg(unix)]
    store: disk::Store,
    options: ContentStorage,
    secret: Option<ContentSecret>,
}
impl Startup {
    pub(crate) fn open(
        config: &Config,
        secret: Option<ContentSecret>,
        now: u64,
    ) -> std::result::Result<Option<Self>, BoxError> {
        let Some(options) = config.content_storage.clone() else {
            if secret.is_some() {
                return Err("Content secret provided without content storage".into());
            }
            return Ok(None);
        };
        if options.encryption == ContentEncryption::None && secret.is_some()
            || options.encryption != ContentEncryption::None
                && secret
                    .as_ref()
                    .is_none_or(|secret| !secret.matches(options.encryption))
        {
            return Err(
                "Content encryption mode requires exactly its matching startup secret".into(),
            );
        }
        if let Some(ContentSecret::Password(password)) = &secret
            && (password.is_empty() || password.len() > 4096)
        {
            return Err("Content password must contain 1..4096 bytes".into());
        }
        #[cfg(not(unix))]
        {
            let _ = options;
            Err("Content storage requires Unix".into())
        }
        #[cfg(unix)]
        {
            let store = match &options.store_id {
                Some(id) => disk::Store::open_bound(&options.directory, id)?,
                None => disk::Store::open(&options.directory)?,
            };
            let stored = store.read(options.max_snapshot_bytes + 80)?;
            let secret = secret
                .map(|secret| {
                    encryption::prepare(secret, stored.as_deref().map(|bytes| bytes.as_slice()))
                })
                .transpose()?;
            let (loaded, entries) = if let Some(bytes) = stored {
                let plain = encryption::decode(
                    bytes,
                    options.encryption,
                    secret.as_ref(),
                    options.max_snapshot_bytes,
                )?;
                let (checkpoint, entries) = topics::load(
                    &store,
                    &plain,
                    config,
                    secret.as_ref(),
                    options.max_snapshot_bytes,
                    now,
                )?;
                (Some(checkpoint), entries)
            } else {
                store.check_fresh(options.store_id.is_some())?;
                (None, topics::Entries::new())
            };
            Ok(Some(Self {
                loaded,
                entries,
                store,
                options,
                secret,
            }))
        }
    }
    pub(crate) fn validate_storage_encryption(
        &self,
        config: &Config,
    ) -> std::result::Result<(), BoxError> {
        check_topic_encryption(&self.entries, &config.topics)
    }
    pub(crate) fn apply_storages(&self, config: &mut Config) {
        for topic in &mut config.topics {
            if let Some(entry) = self.entries.get(&topic.name) {
                topic.storage = entry.storage.clone();
            }
        }
    }
    pub(crate) fn start(
        self,
        revision: u64,
        saved_revision: u64,
        loaded: bool,
        config: &Config,
    ) -> std::result::Result<ContentRuntime, BoxError> {
        #[cfg(unix)]
        self.store
            .cleanup_topics(&topics::committed_files(&self.entries))?;
        let schedules: Vec<_> = config
            .topics
            .iter()
            .filter(|t| t.owner_id == config.service.id)
            .map(|topic| Schedule {
                name: topic.name.clone(),
                storage: topic.storage.clone(),
                revision,
                retry_at: None,
                retry_delay: 0,
                attempted_revision: None,
                isolated_retry: false,
                next: Instant::now()
                    + std::time::Duration::from_secs(topic.storage.interval().unwrap_or(1)),
            })
            .collect();
        let automatic = schedules
            .iter()
            .enumerate()
            .filter(|(_, s)| s.storage.interval().is_some())
            .map(|(i, _)| i)
            .collect();
        let topic_status = schedules
            .iter()
            .filter(|s| s.storage.persistent())
            .map(|s| {
                let saved = self.entries.get(&s.name);
                (
                    s.name.clone(),
                    TopicStatus {
                        storage: s.storage.clone(),
                        revision,
                        saved_revision: saved.map_or(0, |entry| entry.revision),
                        dirty: true,
                        loaded: saved.is_some(),
                        autosave_isolated: false,
                        save_failure: None,
                    },
                )
            })
            .collect();
        let shared = Arc::new(Shared {
            status: Mutex::new(Status {
                topics: topic_status,
                enabled: true,
                encrypted: self.options.encryption != ContentEncryption::None,
                loaded,
                saved_revision,
                ..Status::default()
            }),
            busy: AtomicBool::new(false),
            automatic_capacity_failure: AtomicBool::new(false),
        });
        let background = shared.clone();
        let (sender, receiver) = mpsc::sync_channel::<Save>(1);
        let maximum = self.options.max_snapshot_bytes;
        let retry = self.options.autosave_retry.clone();
        #[cfg(unix)]
        let worker = std::thread::Builder::new()
            .name("mynofy-content".into())
            .spawn(move || {
                let mut entries = self.entries;
                let mut cleanup_at: Option<std::time::Instant> = None;
                let retry_delay = std::time::Duration::from_secs(1);
                loop {
                    if cleanup_at.is_some_and(|deadline| deadline <= std::time::Instant::now()) {
                        cleanup_at = None;
                        if !self.store.uncertain() {
                            let result = self
                                .store
                                .cleanup_topics(&topics::committed_files(&entries));
                            if result.is_err() {
                                cleanup_at = Some(std::time::Instant::now() + retry_delay);
                            }
                            background.status().cleanup_error =
                                result.err().map(|error| error.to_string());
                        }
                    }
                    let request = match cleanup_at {
                        Some(deadline) => receiver.recv_timeout(
                            deadline.saturating_duration_since(std::time::Instant::now()),
                        ),
                        None => receiver
                            .recv()
                            .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
                    };
                    let Save {
                        automatic,
                        security,
                        capture,
                        selected,
                    } = match request {
                        Ok(save) => save,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    };
                    let revision = capture.checkpoint.revision;
                    let writer_start = Instant::now();
                    let result = (|| -> std::result::Result<(), BoxError> {
                        if let Some(fence) = security {
                            fence.wait()?;
                        }
                        let checkpoint = capture.finish(&selected, maximum)?;
                        topics::save(
                            &self.store,
                            &checkpoint,
                            &selected,
                            &mut entries,
                            self.options.encryption,
                            self.secret.as_ref(),
                            maximum,
                        )?;
                        Ok(())
                    })();
                    let failure = match result {
                        Ok(()) => None,
                        Err(error) => Some(error.to_string()),
                    };
                    {
                        let mut status = background.status();
                        status.last_writer_duration_us = Some(
                            u64::try_from(writer_start.elapsed().as_micros()).unwrap_or(u64::MAX),
                        );
                        status.requires_restart = self.store.uncertain();
                        for (name, _) in &selected {
                            if let Some(topic) = status.topics.get_mut(name) {
                                topic.save_failure = failure.as_ref().map(|error| SaveFailure {
                                    revision,
                                    error: error.clone(),
                                });
                            }
                        }
                        if let Some(error) = self.store.take_cleanup_error() {
                            cleanup_at
                                .get_or_insert_with(|| std::time::Instant::now() + retry_delay);
                            status.cleanup_error = Some(error);
                        }
                        if let Some(message) = &failure {
                            status.last_error = Some(message.clone());
                        } else {
                            status.saved_revision = revision;
                            for (name, _) in &selected {
                                if let Some(topic) = status.topics.get_mut(name) {
                                    topic.saved_revision = revision;
                                }
                            }
                            status.last_error = None;
                            status.last_save_time_ms = Some(crate::transport::now_ms());
                        }
                    }
                    if automatic {
                        background.automatic_capacity_failure.store(
                            failure.as_deref().is_some_and(is_capacity_failure),
                            Ordering::Relaxed,
                        );
                    }
                    background.busy.store(false, Ordering::Release);
                    if let Some(message) = failure {
                        eprintln!("mynofy: content save failed: {message}");
                    }
                }
            })?;
        #[cfg(not(unix))]
        let worker = {
            let _ = (receiver, background);
            return Err("Content storage requires Unix".into());
        };
        Ok(ContentRuntime {
            sender: Some(sender),
            worker: Some(worker),
            shared,
            schedules,
            automatic,
            last_automatic: None,
            retry,
            revision,
            maximum,
        })
    }
}

#[cfg(unix)]
pub mod offline;

fn check_topic_encryption(
    entries: &topics::Entries,
    configured: &[crate::config::Topic],
) -> std::result::Result<(), BoxError> {
    for topic in configured {
        if let Some(entry) = entries.get(&topic.name)
            && topic.storage.persistent()
            && entry.storage.encrypted() != topic.storage.encrypted()
        {
            return Err(format!(
                "Topic {} encryption differs from configuration; migrate it offline with storage topic-encryption",
                topic.name
            ).into());
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod crash_tests;
#[cfg(all(test, unix))]
mod faults;

/// Explicitly bind an offline content store to the configured identity.
pub fn initialize(config: &crate::config::Config) -> std::result::Result<(), crate::BoxError> {
    config.validate()?;
    config.require_local_owner()?;
    let options = config
        .content_storage
        .as_ref()
        .ok_or("Configure content storage first")?;
    let id = options
        .store_id
        .as_deref()
        .filter(|id| id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or("Set content_storage.store_id to 32 hexadecimal digits")?;
    #[cfg(unix)]
    return disk::Store::initialize(&options.directory, id);
    #[cfg(not(unix))]
    {
        let _ = id;
        Err("Content storage requires Unix".into())
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;

    #[test]
    fn overdue_isolated_save_gets_the_next_slot() {
        use std::time::Duration;
        let now = Instant::now();
        let names = ["regular-a", "regular-b", "isolated"];
        let schedules: Vec<_> = names
            .iter()
            .enumerate()
            .map(|(index, name)| Schedule {
                name: (*name).into(),
                storage: TopicStorage::Interval {
                    seconds: 1,
                    encrypted: false,
                },
                revision: 1,
                next: now,
                retry_at: (index == 2).then_some(now - Duration::from_secs(1)),
                retry_delay: 1,
                attempted_revision: None,
                isolated_retry: index == 2,
            })
            .collect();
        let topics = schedules
            .iter()
            .map(|schedule| {
                (
                    schedule.name.clone(),
                    TopicStatus {
                        storage: schedule.storage.clone(),
                        revision: 1,
                        saved_revision: 0,
                        dirty: true,
                        loaded: false,
                        autosave_isolated: schedule.isolated_retry,
                        save_failure: None,
                    },
                )
            })
            .collect();
        let mut runtime = ContentRuntime {
            sender: None,
            worker: None,
            schedules,
            automatic: vec![0, 1, 2],
            last_automatic: None,
            retry: Default::default(),
            revision: 1,
            maximum: 65536,
            shared: Arc::new(Shared {
                status: Mutex::new(Status {
                    topics,
                    ..Status::default()
                }),
                busy: AtomicBool::new(false),
                automatic_capacity_failure: AtomicBool::new(false),
            }),
        };
        assert_eq!(
            runtime
                .due()
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            ["isolated"]
        );
        assert_eq!(
            runtime
                .due()
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            ["regular-a", "regular-b"]
        );
    }

    #[test]
    fn completed_save_snapshot_observes_published_failure() {
        for _ in 0..128 {
            let shared = Arc::new(Shared {
                status: Mutex::new(Status::default()),
                busy: AtomicBool::new(true),
                automatic_capacity_failure: AtomicBool::new(false),
            });
            let writer = shared.clone();
            let worker = std::thread::spawn(move || {
                writer.status().last_error = Some("injected I/O failure".into());
                writer.busy.store(false, Ordering::Release);
            });
            loop {
                let status = shared.snapshot();
                if !status.save_in_progress {
                    assert_eq!(status.last_error.as_deref(), Some("injected I/O failure"));
                    break;
                }
                std::thread::yield_now();
            }
            worker.join().unwrap();
        }
    }
}
