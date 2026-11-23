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
    pub last_error: Option<String>,
}
#[derive(Clone, Serialize)]
pub(crate) struct TopicStatus {
    pub storage: TopicStorage,
    pub revision: u64,
    pub saved_revision: u64,
    pub dirty: bool,
    pub loaded: bool,
}
struct Schedule {
    name: String,
    storage: TopicStorage,
    revision: u64,
    next: Instant,
}
struct Save {
    security: Option<crate::security::DurabilityFence>,
    checkpoint: Checkpoint,
    selected: Vec<(String, TopicStorage)>,
}
struct Shared {
    status: Mutex<Status>,
    busy: AtomicBool,
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
    pub(crate) fn status(&self) -> Status {
        let mut status = self.shared.snapshot();
        status.revision = self.revision;
        for schedule in self.schedules.iter().filter(|s| s.storage.persistent()) {
            if let Some(topic) = status.topics.get_mut(&schedule.name) {
                topic.revision = schedule.revision;
                topic.dirty = topic.revision > topic.saved_revision || topic.revision == u64::MAX;
            }
        }
        status.dirty = status.topics.values().any(|topic| topic.dirty);
        status
    }
    #[cfg(test)]
    pub(crate) fn force_due(&mut self) {
        for schedule in &mut self.schedules {
            schedule.next = Instant::now();
        }
    }
    pub(crate) fn due(&mut self) -> Vec<String> {
        if self.shared.busy.load(Ordering::Acquire) {
            return Vec::new();
        }
        if self.automatic.is_empty() {
            return Vec::new();
        }
        let status = self.shared.status();
        if status.requires_restart {
            return Vec::new();
        }
        let now = Instant::now();
        let mut due = Vec::new();
        for &slot in &self.automatic {
            let schedule = &mut self.schedules[slot];
            if let Some(seconds) = schedule.storage.interval()
                && now >= schedule.next
            {
                schedule.next = now + std::time::Duration::from_secs(seconds);
                if schedule.revision > status.topics[&schedule.name].saved_revision
                    || schedule.revision == u64::MAX
                {
                    due.push(schedule.name.clone());
                }
            }
        }
        due
    }
    pub(crate) fn selection(
        &self,
        requested: Option<&[String]>,
    ) -> Result<Vec<(String, TopicStorage)>> {
        if let Some(names) = requested {
            let mut seen = std::collections::HashSet::new();
            if names.is_empty()
                || names.iter().any(|name| {
                    !seen.insert(name)
                        || !self
                            .schedules
                            .iter()
                            .any(|s| s.name == *name && s.storage.persistent())
                })
            {
                return Err(ApiError::new(
                    "invalid_content_topics",
                    "Choose distinct locally persistent topics.",
                ));
            }
        }
        let selected: Vec<_> = self
            .schedules
            .iter()
            .filter(|s| {
                s.storage.persistent() && requested.is_none_or(|names| names.contains(&s.name))
            })
            .map(|s| (s.name.clone(), s.storage.clone()))
            .collect();
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
    pub(crate) fn capture_failed(&self, error: &ApiError) {
        self.shared.status().last_error = Some(format!("{}: {}", error.code, error.detail));
        self.cancel();
    }
    pub(crate) fn cancel(&self) {
        self.shared.busy.store(false, Ordering::Release);
    }
    pub(crate) fn submit(
        &self,
        checkpoint: Checkpoint,
        selected: Vec<(String, TopicStorage)>,
        security: Option<crate::security::DurabilityFence>,
    ) -> Result<()> {
        if self.sender.as_ref().is_none_or(|sender| {
            sender
                .try_send(Save {
                    security,
                    checkpoint,
                    selected,
                })
                .is_err()
        }) {
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
impl Drop for ContentRuntime {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
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
            let store = disk::Store::open(&options.directory)?;
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
                store.require_fresh()?;
                (None, topics::Entries::new())
            };
            store.cleanup_topics(&topics::committed_files(&entries))?;
            Ok(Some(Self {
                loaded,
                entries,
                store,
                options,
                secret,
            }))
        }
    }
    pub(crate) fn apply_storages(
        &self,
        config: &mut Config,
        _configured: &std::collections::HashSet<String>,
    ) {
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
        let schedules: Vec<_> = config
            .topics
            .iter()
            .filter(|t| t.owner_id == config.service.id)
            .map(|topic| Schedule {
                name: topic.name.clone(),
                storage: topic.storage.clone(),
                revision,
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
        });
        let background = shared.clone();
        let (sender, receiver) = mpsc::sync_channel::<Save>(1);
        let maximum = self.options.max_snapshot_bytes;
        #[cfg(unix)]
        let worker = std::thread::Builder::new()
            .name("mynofy-content".into())
            .spawn(move || {
                let mut entries = self.entries;
                while let Ok(Save {
                    security,
                    checkpoint,
                    selected,
                }) = receiver.recv()
                {
                    let revision = checkpoint.revision;
                    let result = (|| -> std::result::Result<(), BoxError> {
                        if let Some(fence) = security {
                            fence.wait()?;
                        }
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
                    drop(checkpoint);
                    let failure = match result {
                        Ok(()) => None,
                        Err(error) => Some(error.to_string()),
                    };
                    {
                        let mut status = background.status();
                        status.requires_restart = self.store.uncertain();
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
            revision,
            maximum,
        })
    }
}

#[cfg(unix)]
pub mod offline;

#[cfg(all(test, unix))]
mod crash_tests;
#[cfg(all(test, unix))]
mod faults;

#[cfg(test)]
mod status_tests {
    use super::*;

    #[test]
    fn completed_save_snapshot_observes_published_failure() {
        for _ in 0..128 {
            let shared = Arc::new(Shared {
                status: Mutex::new(Status::default()),
                busy: AtomicBool::new(true),
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
