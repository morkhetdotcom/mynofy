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
    let prepared = match secret {
        Some(ContentSecret::Password(password)) => Some(encryption::prepare(
            ContentSecret::Password(password.clone()),
            Some(&bytes),
        )?),
        _ => None,
    };
    let secret = prepared.as_ref().or(secret);
    let plain = encryption::decode(
        bytes,
        options.encryption,
        secret,
        options.max_snapshot_bytes,
    )?;
    if !plain.starts_with(b"MYNTOP01") {
        return Err(
            "Offline tools require a topic manifest; start the service once to migrate legacy data"
                .into(),
        );
    }
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
        let store = disk::Store::open(&options.directory)?;
        let secret = secret
            .map(|secret| {
                let bytes = store.read(options.max_snapshot_bytes)?;
                encryption::prepare(secret, bytes.as_deref().map(|bytes| bytes.as_slice()))
            })
            .transpose()?;
        let has_manifest = store.has_state()?;

        let (checkpoint, entries) = if allow_empty && !has_manifest {
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
        for name in topics::committed_files(&self.entries) {
            let bytes = self
                .store
                .read_named(&name, self.maximum)?
                .ok_or("Missing committed topic")?;
            target.save_named(&name, &bytes)?;
        }
        let manifest = self.store.read(self.maximum)?.ok_or("Missing manifest")?;
        target.save(&manifest)?;
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
        let source = disk::Store::open(source)?;
        let (mut checkpoint, entries) = read(
            &source,
            &self.config,
            backup_secret.as_ref().or(self.secret.as_ref()),
        )?;
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
        self.cleanup()?;
        Ok(())
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
        self.cleanup()?;
        Ok(())
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
        let checkpoint = self.checkpoint.select(&selected, self.maximum)?;
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
        self.cleanup()?;
        Ok(())
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
