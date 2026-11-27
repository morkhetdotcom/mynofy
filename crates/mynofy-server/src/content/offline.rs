//! Locked, offline-only content maintenance. Never installs state in a live Engine.
use super::*;
use std::path::Path;

pub struct OfflineStorage {
    store: disk::Store,
    config: Config,
    secret: Option<ContentSecret>,
    checkpoint: Checkpoint,
    entries: topics::Entries,
    maximum: u64,
    _security: crate::security::OfflineLock,
    configured_topics: Vec<crate::config::Topic>,
    ram_baseline: std::collections::HashSet<String>,
}
fn read(
    store: &disk::Store,
    config: &Config,
    secret: Option<&ContentSecret>,
) -> std::result::Result<(Checkpoint, topics::Entries), BoxError> {
    let options = config
        .content_storage
        .as_ref()
        .ok_or("Content storage is not configured")?;
    let bytes = store
        .read(options.max_snapshot_bytes)?
        .ok_or("No saved content manifest")?;
    let prepared = encryption::prepare_source(secret, &bytes)?;
    let secret = prepared.as_ref().or(secret);
    let plain = encryption::decode(
        bytes,
        options.encryption,
        secret,
        options.max_snapshot_bytes,
    )?;
    topics::load(
        store,
        &plain,
        config,
        secret,
        options.max_snapshot_bytes,
        crate::transport::now_ms(),
    )
}
impl OfflineStorage {
    /// Convert an entire legacy generation after validating its topics and security bindings.
    pub fn migrate_legacy(
        mut config: Config,
        secret: Option<ContentSecret>,
        preview: bool,
    ) -> std::result::Result<serde_json::Value, BoxError> {
        config.validate()?;
        config.require_local_owner()?;
        let options = config
            .content_storage
            .clone()
            .ok_or("Content storage is not configured")?;
        if !options.directory.is_dir() {
            return Err("Legacy content directory does not exist".into());
        }
        let store = match (&options.store_id, preview) {
            (Some(id), false) => disk::Store::open_bound(&options.directory, id)?,
            (expected, _) => {
                let store = disk::Store::open_mode(&options.directory, preview)?;
                if let Some(id) = expected {
                    store.verify_id(id)?;
                }
                store
            }
        };
        let bytes = store
            .read(options.max_snapshot_bytes + 80)?
            .ok_or("No legacy manifest")?;
        let secret = secret
            .map(|secret| encryption::prepare(secret, Some(&bytes)))
            .transpose()?;
        let plain = encryption::decode(
            bytes,
            options.encryption,
            secret.as_ref(),
            options.max_snapshot_bytes,
        )?;
        if !plain.starts_with(b"MYNCP001") {
            return Err("Content manifest is not in the legacy format".into());
        }
        let checkpoint = Checkpoint::read(&plain, &config, options.max_snapshot_bytes)?;
        let mut selected = Vec::with_capacity(checkpoint.image.topics.len());
        for name in &checkpoint.image.topics {
            let topic = config
                .topics
                .iter()
                .find(|topic| {
                    &topic.name == name
                        && topic.owner_id == config.service.id
                        && topic.storage.persistent()
                })
                .ok_or_else(|| {
                    format!("Configure saved topic {name} as locally persistent before migrating")
                })?;
            selected.push((name.clone(), topic.storage.clone()));
        }
        if selected.is_empty() {
            return Err("Legacy store has no topic to migrate".into());
        }
        let _security = crate::security::OfflineLock::open_mode(&mut config, preview)?;
        checkpoint.prepare_topics(&mut config)?;
        validate_checkpoint(&checkpoint, &mut config)?;
        let mut counts = vec![0usize; selected.len()];
        for record in checkpoint.image.records.iter() {
            counts[record.topic() as usize] += 1;
        }
        let topics: Vec<_> = selected.iter().zip(counts).map(|((name, storage), messages)| {
            serde_json::json!({"topic":name, "storage":storage, "messages":messages})
        }).collect();
        if !preview {
            topics::save(
                &store,
                &checkpoint,
                &selected,
                &mut topics::Entries::new(),
                options.encryption,
                secret.as_ref(),
                options.max_snapshot_bytes,
            )?;
        }
        Ok(serde_json::json!({"preview":preview,"topics":topics}))
    }

    pub fn open(
        config: Config,
        secret: Option<ContentSecret>,
    ) -> std::result::Result<Self, BoxError> {
        Self::open_inner(config, secret, false)
    }
    pub fn open_for_restore(
        config: Config,
        secret: Option<ContentSecret>,
    ) -> std::result::Result<Self, BoxError> {
        Self::open_inner(config, secret, true)
    }
    fn open_inner(
        mut config: Config,
        secret: Option<ContentSecret>,
        allow_empty: bool,
    ) -> std::result::Result<Self, BoxError> {
        config.validate()?;
        let mode = config
            .content_storage
            .as_ref()
            .ok_or("Content storage is not configured")?
            .encryption;
        if (mode == ContentEncryption::None && secret.is_some())
            || (mode != ContentEncryption::None
                && secret.as_ref().is_none_or(|value| !value.matches(mode)))
        {
            return Err("Content encryption mode requires its matching startup secret".into());
        }
        if let Some(ContentSecret::Password(password)) = &secret
            && (password.is_empty() || password.len() > 4096)
        {
            return Err("Invalid password length".into());
        }

        let ram_baseline = config
            .topics
            .iter()
            .filter(|topic| !topic.storage.persistent())
            .map(|topic| topic.name.clone())
            .collect();
        config.require_local_owner()?;
        let options = config
            .content_storage
            .clone()
            .ok_or("Content storage is not configured")?;
        if !allow_empty && !options.directory.exists() {
            return Err("Content storage directory does not exist".into());
        }
        let store = match &options.store_id {
            Some(id) => disk::Store::open_bound(&options.directory, id)?,
            None => disk::Store::open(&options.directory)?,
        };
        let secret = secret
            .map(|secret| {
                let bytes = store.read(options.max_snapshot_bytes)?;
                encryption::prepare(secret, bytes.as_deref().map(|bytes| bytes.as_slice()))
            })
            .transpose()?;
        let has_manifest = store.has_state()?;

        let (checkpoint, entries) = if allow_empty && !has_manifest {
            // An initialized target has an unknown cutoff after manifest loss.
            // Never turn it into a fresh target and permit expiry rollback.
            store.check_fresh(options.store_id.is_some())?;
            let image = crate::message_image::MessageImage::capture(
                &config.service.id,
                &[],
                &[],
                &crate::storage::Arena::new(),
                &crate::storage::ContentTypes::default(),
                &crate::storage::Blobs::default(),
                Checkpoint::image_limits(options.max_snapshot_bytes),
            )?;
            (
                Checkpoint {
                    image,
                    revision: 0,
                    next_message: 0,
                    reserved_ids: 0,
                    cutoff: 0,
                    produced: Vec::new(),
                    topics: Vec::new(),
                },
                topics::Entries::new(),
            )
        } else {
            read(&store, &config, secret.as_ref())?
        };
        let configured_topics = config.topics.clone();
        if allow_empty {
            check_topic_encryption(&entries, &configured_topics)?;
        }
        checkpoint.prepare_topics(&mut config)?;
        for topic in &mut config.topics {
            if let Some(entry) = entries.get(&topic.name) {
                topic.storage = entry.storage.clone();
            }
        }
        let security = crate::security::OfflineLock::open(&mut config)?;
        let mut validation = config.clone();
        validate_checkpoint(&checkpoint, &mut validation)?;
        Ok(Self {
            store,
            config,
            secret,
            checkpoint,
            entries,
            maximum: options.max_snapshot_bytes,
            _security: security,
            configured_topics,
            ram_baseline,
        })
    }
    /// A rename succeeded but directory synchronization failed; reopen before any maintenance.
    pub fn needs_reopen(&self) -> bool {
        self.store.uncertain()
    }
    fn ensure_certain(&self) -> std::result::Result<(), BoxError> {
        if self.needs_reopen() {
            return Err("Content commit is uncertain; reopen storage before maintenance".into());
        }
        Ok(())
    }
    pub fn check(&self) -> serde_json::Value {
        serde_json::json!({"service_id": self.config.service.id, "topics": self.entries.len(), "active_messages": self.checkpoint.image.message_count(), "revision": self.checkpoint.revision, "durability_uncertain": self.needs_reopen()})
    }
    /// Metadata only; no message bodies or secrets are exposed.
    pub fn inspect(&self) -> serde_json::Value {
        let mut messages = vec![0usize; self.checkpoint.image.topics.len()];
        for record in self.checkpoint.image.records.iter() {
            messages[record.topic() as usize] += 1;
        }
        let slots: std::collections::HashMap<_, _> = self
            .checkpoint
            .image
            .topics
            .iter()
            .enumerate()
            .map(|(slot, name)| (name.as_str(), slot))
            .collect();
        let topics: Vec<_> = self
            .entries
            .iter()
            .map(|(name, entry)| {
                let slot = slots[name.as_str()];
                serde_json::json!({"topic":name,"storage":entry.storage,"revision":entry.revision,
                "stored_bytes":entry.bytes,"active_messages":messages[slot]})
            })
            .collect();
        let mut report = self.check();
        report["topic_details"] = serde_json::Value::Array(topics);
        report
    }
    /// Remove only unreferenced internal slots and staging files under both owner locks.
    pub fn cleanup(&self) -> std::result::Result<(), BoxError> {
        self.ensure_certain()?;
        self.store
            .cleanup_topics(&topics::committed_files(&self.entries))?;
        self.store.cleanup_temporary()?;
        Ok(())
    }
    fn cleanup_after_commit(&self) -> std::result::Result<(), BoxError> {
        let prior = self.store.take_cleanup_error();
        let cleanup = self.cleanup();
        match cleanup {
            Ok(()) => Ok(()),
            Err(error) => Err(format!(
                "commit=durable cleanup=pending: {error}. Run storage cleanup before retrying the operation.{}",
                prior.map_or_else(String::new, |detail| format!(" Earlier cleanup: {detail}"))
            ).into()),
        }
    }
    /// Copy only a fully committed content generation into a new private directory.
    pub fn backup(&self, destination: &Path) -> std::result::Result<(), BoxError> {
        self.ensure_certain()?;
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(destination)?;
        std::fs::File::open(
            destination
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?
        .sync_all()?;
        let target = disk::Store::open(destination)?;
        target.begin_backup()?;
        for name in topics::committed_files(&self.entries) {
            let bytes = self
                .store
                .read_named(&name, self.maximum)?
                .ok_or("Missing committed topic")?;
            target.stage_topic(&name, &bytes)?;
        }
        if !self.entries.is_empty() {
            target.sync_staged_topics()?;
        }
        let manifest = self.store.read(self.maximum)?.ok_or("Missing manifest")?;
        target.save(&manifest)?;
        if let Some(id) = self
            .config
            .content_storage
            .as_ref()
            .and_then(|storage| storage.store_id.as_deref())
        {
            target.write_id(id)?;
        }
        Ok(())
    }
    /// Restore content only. Current security and its boot counter are never replaced.
    pub fn restore(&mut self, source: &Path) -> std::result::Result<(), BoxError> {
        self.restore_with_secret(source, None)
    }
    pub fn restore_with_secret(
        &mut self,
        source: &Path,
        backup_secret: Option<ContentSecret>,
    ) -> std::result::Result<(), BoxError> {
        self.ensure_certain()?;
        if !source.exists() {
            return Err("Backup directory does not exist".into());
        }
        let source = disk::Store::open_mode(source, true)?;
        source.require_committed()?;
        let (mut checkpoint, entries) = read(
            &source,
            &self.config,
            backup_secret.as_ref().or(self.secret.as_ref()),
        )?;
        check_topic_encryption(&entries, &self.configured_topics)?;
        checkpoint.cutoff = checkpoint
            .cutoff
            .max(self.checkpoint.cutoff)
            .max(crate::transport::now_ms());
        checkpoint.discard_expired(checkpoint.cutoff);
        let mut validation = self.config.clone();
        checkpoint.prepare_topics(&mut validation)?;
        validate_checkpoint(&checkpoint, &mut validation)?;
        let selected: Vec<_> = entries
            .iter()
            .map(|(name, entry)| (name.clone(), entry.storage.clone()))
            .collect();
        topics::save(
            &self.store,
            &checkpoint,
            &selected,
            &mut self.entries,
            self.config
                .content_storage
                .as_ref()
                .ok_or("Missing content config")?
                .encryption,
            self.secret.as_ref(),
            self.maximum,
        )?;
        self.checkpoint = checkpoint;
        self.cleanup_after_commit()
    }
    /// Change the shared secret, keeping its password/key mode and per-topic encryption choices.
    pub fn rekey(&mut self, replacement: ContentSecret) -> std::result::Result<(), BoxError> {
        self.ensure_certain()?;
        let common = self
            .config
            .content_storage
            .as_ref()
            .ok_or("Missing content config")?
            .encryption;
        if common == ContentEncryption::None || !replacement.matches(common) {
            return Err(
                "Rekey requires a replacement secret in the current encryption mode".into(),
            );
        }
        if let ContentSecret::Password(password) = &replacement
            && (password.is_empty() || password.len() > 4096)
        {
            return Err("Invalid replacement password length".into());
        }
        let replacement = encryption::prepare(replacement, None)?;
        let selected: Vec<_> = self
            .entries
            .iter()
            .map(|(name, entry)| (name.clone(), entry.storage.clone()))
            .collect();
        topics::save(
            &self.store,
            &self.checkpoint,
            &selected,
            &mut self.entries,
            common,
            Some(&replacement),
            self.maximum,
        )?;
        self.secret = Some(replacement);
        self.cleanup_after_commit()
    }
    /// Rewrite one committed topic using the persistent policy in the config.
    pub fn migrate_topic_encryption(&mut self, topic: &str) -> std::result::Result<(), BoxError> {
        self.ensure_certain()?;
        let storage = self
            .configured_topics
            .iter()
            .find(|configured| configured.name == topic && configured.storage.persistent())
            .ok_or("Topic needs an explicit persistent storage policy in the configuration")?
            .storage
            .clone();
        let current = self
            .entries
            .get(topic)
            .ok_or("Topic has no saved content")?;
        if current.storage.encrypted() == storage.encrypted() {
            return self.cleanup();
        }
        let common = self
            .config
            .content_storage
            .as_ref()
            .ok_or("Missing content config")?
            .encryption;
        topics::save(
            &self.store,
            &self.checkpoint,
            &[(topic.to_owned(), storage)],
            &mut self.entries,
            common,
            self.secret.as_ref(),
            self.maximum,
        )?;
        self.cleanup_after_commit()
    }
    /// Storage -> RAM is offline-only and deletes the saved topic slots after manifest commit.
    pub fn topic_to_ram(&mut self, topic: &str) -> std::result::Result<(), BoxError> {
        self.ensure_certain()?;
        if !self.ram_baseline.contains(topic) {
            return Err(
                "Set this topic's CFG storage.mode to ram before the offline conversion".into(),
            );
        }
        if !self.entries.contains_key(topic) {
            // A prior invocation may have committed RAM but stopped before deleting old slots.
            return self.cleanup();
        }
        let selected: Vec<_> = self
            .checkpoint
            .image
            .topics
            .iter()
            .enumerate()
            .filter(|(_, name)| name.as_str() != topic)
            .map(|(slot, _)| slot as u16)
            .collect();
        let checkpoint = self
            .checkpoint
            .select_shared_registry(&selected, self.maximum)?;
        topics::save(
            &self.store,
            &checkpoint,
            &[],
            &mut self.entries,
            self.config
                .content_storage
                .as_ref()
                .ok_or("Missing content config")?
                .encryption,
            self.secret.as_ref(),
            self.maximum,
        )?;
        self.checkpoint = checkpoint;
        self.cleanup_after_commit()
    }
}

fn validate_checkpoint(
    checkpoint: &Checkpoint,
    config: &mut Config,
) -> std::result::Result<(), BoxError> {
    checkpoint.order_clients(config)?;
    let bytes: usize = checkpoint
        .image
        .records
        .iter()
        .map(|record| record.charge(&checkpoint.image.types, &checkpoint.image.blobs))
        .sum();
    let limits = &config.limits;
    if checkpoint.image.message_count() > limits.max_active_messages
        || bytes > limits.max_active_message_bytes
    {
        return Err("Content exceeds configured active limits".into());
    }
    Ok(())
}
