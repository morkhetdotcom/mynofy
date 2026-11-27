use super::*;
use crate::content::{
    Checkpoint,
    checkpoint::{Capture, TopicState},
};
impl Engine {
    pub fn enable_topic_storage(
        &mut self,
        topic: &str,
        storage: crate::config::TopicStorage,
    ) -> Result<()> {
        let mut candidate = self.config.clone();
        let target = candidate
            .topics
            .iter_mut()
            .find(|t| t.name == topic && t.owner_id == candidate.service.id)
            .ok_or_else(ApiError::invalid)?;
        target.storage = storage.clone();
        candidate.validate().map_err(|_| ApiError::invalid())?;
        let slot = self.topics.get(topic).ok_or_else(ApiError::invalid)?.slot;
        self.content
            .as_mut()
            .ok_or_else(|| {
                ApiError::new(
                    "content_storage_disabled",
                    "Content storage is not configured.",
                )
            })?
            .enable(slot, storage.clone())?;
        self.topics
            .get_mut(topic)
            .ok_or_else(ApiError::internal)?
            .storage = storage;
        self.config = candidate;
        Ok(())
    }

    pub(super) fn content_changed(&mut self, topic: u16) {
        if let Some(content) = &mut self.content {
            content.changed(topic);
        }
    }
    pub(crate) fn content_status(&self) -> crate::content::Status {
        self.content
            .as_ref()
            .map_or_else(Default::default, |content| content.status())
    }
    /// Queue one consistent snapshot. File I/O/encryption happens on the dedicated writer.
    pub fn save_content(&mut self) -> Result<u64> {
        self.save_content_topics(None)
    }
    pub fn save_content_topics(&mut self, requested: Option<&[String]>) -> Result<u64> {
        let content = self.content.as_ref().ok_or_else(|| {
            ApiError::new(
                "content_storage_disabled",
                "Content storage is not configured.",
            )
        })?;
        let selected = content.selection(requested)?;
        self.queue_content_save(selected, false)
    }
    pub(super) fn queue_content_save(
        &mut self,
        selected: Vec<(String, crate::config::TopicStorage)>,
        automatic: bool,
    ) -> Result<u64> {
        let content = self.content.as_ref().ok_or_else(ApiError::internal)?;
        content.reserve()?;
        let revision = content.revision;
        let snapshot = self.capture_content(&selected);
        let content = self.content.as_ref().ok_or_else(ApiError::internal)?;
        match snapshot {
            Ok(snapshot) => content.submit(
                snapshot,
                selected,
                self.security
                    .as_ref()
                    .map(|journal| journal.durability_fence()),
                automatic,
            )?,
            Err(error) => {
                content.capture_failed(&error, automatic);
                return Err(error);
            }
        }
        Ok(revision)
    }
    /// Caller holds the Engine lock and has reserved the single content-save slot.
    pub(super) fn capture_content(
        &mut self,
        selected: &[(String, crate::config::TopicStorage)],
    ) -> Result<Capture> {
        let content = self.content.as_ref().ok_or_else(ApiError::internal)?;
        let revision = content.revision;
        let selected_records: usize = selected
            .iter()
            .map(|(name, _)| self.topics[name].messages.len())
            .sum();
        let resident_records = self.records.len();
        // Both paths need shared registries. Only the broad snapshot pins
        // unrelated arena and blob blocks.
        let metadata_bytes = self
            .client_names
            .iter()
            .chain(&self.topic_names)
            .fold(0usize, |total, name| {
                total.saturating_add(name.len().saturating_add(256))
            });
        // The sparse projection packs new pages. Account conservatively for
        // slot rounding, per-record scratch and page tails across size classes.
        let sparse_bytes = selected.iter().fold(metadata_bytes, |total, (name, _)| {
            total.saturating_add(self.topics[name].active_bytes.saturating_mul(2))
        });
        let estimate = |bytes: usize| bytes.saturating_mul(6).saturating_add(1024 * 1024);
        let limit = self.config.limits.max_content_working_bytes;
        let broad_working = (selected_records != 0
            && selected_records.saturating_mul(8) >= resident_records.saturating_mul(7))
        .then(|| {
            estimate(
                metadata_bytes
                    .saturating_add(self.records.allocated_bytes())
                    .saturating_add(self.payloads.allocated_bytes())
                    .saturating_add(self.content_types.table_bytes()),
            )
        })
        .filter(|&bytes| bytes <= limit);
        let broad = broad_working.is_some();
        let working = broad_working.unwrap_or_else(|| estimate(sparse_bytes));
        if working > limit {
            return Err(ApiError::new(
                "content_memory_budget_exceeded",
                "Content capture exceeds the configured working-memory allowance.",
            ));
        }
        content.reserve_working(working);
        let persistent_topics: Vec<_> = self
            .config
            .topics
            .iter()
            .filter(|topic| topic.owner_id == self.config.service.id && topic.storage.persistent())
            .map(|topic| topic.name.clone())
            .collect();
        let mut projection_indices = None;
        let mut deferred = broad;
        let mut image = if broad {
            // Pin initialized blocks, not individual records/payloads. Selection
            // and compaction run later over this immutable RAM epoch.
            // These bounds describe live RAM, not the serialized snapshot limit.
            crate::message_image::MessageImage::capture_shared(
                &self.config.service.id,
                &self.client_names,
                &self.topic_names,
                &mut self.records,
                &self.content_types,
                &mut self.payloads,
                crate::message_image::ImageLimits {
                    max_bytes: working as u64,
                    max_records: resident_records,
                    max_record_chunks: usize::MAX,
                    max_blob_pages: usize::MAX,
                },
            )?
        } else {
            let indices = || {
                selected.iter().flat_map(|(name, _)| {
                    self.topics[name].messages.range(..).map(|(_, slot)| slot)
                })
            };
            // Pin sparse blocks only when projection is large enough and the
            // extra copy-on-write reservation fits. Small or highly scattered
            // selections retain the compact copy path.
            let pinned = if selected_records >= 256
                && selected_records.saturating_mul(32) <= limit - working
            {
                let selected_slots: Vec<_> = indices().collect();
                crate::message_image::MessageImage::pin_selected(
                    &self.config.service.id,
                    &self.client_names,
                    &self.topic_names,
                    (&mut self.records, &self.content_types, &mut self.payloads),
                    &selected_slots,
                    limit - working,
                )
                .map(|(image, extra)| (image, extra, selected_slots))
            } else {
                None
            };
            if let Some((image, extra, selected_slots)) = pinned {
                content.reserve_working(working + extra);
                deferred = true;
                projection_indices = Some(selected_slots);
                image
            } else {
                let slots: Vec<_> = selected
                    .iter()
                    .map(|(name, _)| self.topics[name].slot)
                    .collect();
                crate::message_image::MessageImage::selected_slots(
                    &self.config.service.id,
                    &self.client_names,
                    &self.topic_names,
                    (&self.records, &self.content_types, &self.payloads),
                    &slots,
                    Checkpoint::image_limits(content.maximum),
                    indices(),
                )?
            }
        };
        if !deferred {
            for name in &persistent_topics {
                if !image.topics.contains(name) {
                    image.topics.push(name.clone());
                }
            }
        }
        let produced = self
            .client_names
            .iter()
            .map(|name| self.clients[name].produced)
            .collect();
        let topics = image
            .topics
            .iter()
            .map(|name| TopicState {
                head: self.topics[name].head,
                policy: self.topics[name].policy.clone(),
            })
            .collect();
        Ok(Capture {
            checkpoint: Checkpoint {
                image,
                revision,
                next_message: self.next_message,
                reserved_ids: self.message_ids.reserved_position(),
                cutoff: self.last_cleanup_now,
                produced,
                topics,
            },
            persistent_topics: deferred.then_some(persistent_topics),
            indices: projection_indices,
        })
    }
    // Private and called exactly once by the constructor. No live restore/reload API exists.
    pub(super) fn restore_content(&mut self, checkpoint: Checkpoint, now: u64) -> Result<()> {
        if !self.client_names.starts_with(&checkpoint.image.clients)
            || !self.topic_names.starts_with(&checkpoint.image.topics)
        {
            return Err(ApiError::invalid());
        }
        self.records = checkpoint.image.records;
        self.content_types = checkpoint.image.types;
        self.payloads = checkpoint.image.blobs;
        self.next_message = checkpoint.next_message;
        self.message_ids = crate::ids::IdPool::at(checkpoint.reserved_ids);
        self.last_cleanup_now = now.max(checkpoint.cutoff);
        for (name, position) in self.client_names.iter().zip(checkpoint.produced) {
            self.clients
                .get_mut(name)
                .ok_or_else(ApiError::internal)?
                .produced = position;
        }
        for (name, saved) in self.topic_names.iter().zip(checkpoint.topics) {
            self.topics
                .get_mut(name)
                .ok_or_else(ApiError::internal)?
                .head = saved.head;
        }
        let expired: Vec<_> = self
            .records
            .iter_slots()
            .filter(|(_, record)| record.until() <= self.last_cleanup_now)
            .map(|(slot, _)| slot)
            .collect();
        for slot in expired {
            if let Some(record) = self.records.remove(slot) {
                record
                    .payload()
                    .release(&mut self.content_types, &mut self.payloads);
            }
        }
        for (slot, record) in self.records.iter_slots() {
            let topic = self
                .topic_names
                .get(record.topic() as usize)
                .ok_or_else(ApiError::internal)?;
            let charge = record.charge(&self.content_types, &self.payloads);
            let topic = self.topics.get_mut(topic).ok_or_else(ApiError::internal)?;
            topic.messages.insert(record.seq(), slot);
            topic.active_bytes = topic
                .active_bytes
                .checked_add(charge)
                .ok_or_else(ApiError::exhausted)?;
            let client = self
                .client_names
                .get(record.producer_slot())
                .ok_or_else(ApiError::internal)?;
            let publisher = self
                .clients
                .get_mut(client)
                .ok_or_else(ApiError::internal)?;
            publisher.active_bytes = publisher
                .active_bytes
                .checked_add(charge)
                .ok_or_else(ApiError::exhausted)?;
            self.own
                .get_mut(client)
                .ok_or_else(ApiError::internal)?
                .insert(record.own_pos(), slot);
            self.index.insert(record.id(), slot);
            self.expiry.insert((record.until(), slot));
            self.active_bytes = self
                .active_bytes
                .checked_add(charge)
                .ok_or_else(ApiError::exhausted)?;
        }
        if self.records.len() > self.config.limits.max_active_messages
            || self.active_bytes > self.config.limits.max_active_message_bytes
        {
            return Err(ApiError::new(
                "content_restore_limit",
                "Restored active messages exceed configured limits.",
            ));
        }
        while self.records.needs_reclaim() {
            self.records.reclaim(8);
        }
        while self.payloads.needs_reclaim() {
            self.payloads.reclaim(8);
        }
        self.content_types.reclaim();
        Ok(())
    }
}
