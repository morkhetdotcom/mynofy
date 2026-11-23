use super::*;
use crate::content::{Checkpoint, checkpoint::TopicState};
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
            .enable(slot, storage)?;
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
        content.reserve()?;
        let revision = content.revision;
        let maximum = content.maximum;
        let snapshot = (|| -> Result<Checkpoint> {
            // Conservative aggregate allowance for capture, topic copy, encoding,
            // encryption growth and index scratch. This is not a process RSS cap.
            let storage_bytes = self
                .records
                .allocated_bytes()
                .saturating_add(self.payloads.allocated_bytes())
                .saturating_add(self.content_types.table_bytes())
                .saturating_add(
                    self.client_names
                        .iter()
                        .chain(&self.topic_names)
                        .map(|name| name.len().saturating_add(256))
                        .sum::<usize>(),
                );
            let working = storage_bytes
                .checked_mul(6)
                .and_then(|bytes| bytes.checked_add(1024 * 1024))
                .ok_or_else(|| {
                    ApiError::new(
                        "content_memory_budget_exceeded",
                        "Content working-memory estimate overflow.",
                    )
                })?;
            if working > self.config.limits.max_content_working_bytes {
                return Err(ApiError::new(
                    "content_memory_budget_exceeded",
                    "Content capture exceeds the configured working-memory allowance.",
                ));
            }

            content.reserve_working(working);
            let selected_slots: Vec<_> = selected
                .iter()
                .map(|(name, _)| self.topics[name].slot)
                .collect();
            let controls = (self.client_names.len() * 8 + self.topic_names.len() * 32 + 128) as u64;
            let image_maximum = maximum
                .checked_sub(controls)
                .ok_or_else(ApiError::exhausted)?;
            let mut image = crate::message_image::MessageImage::selected(
                &self.config.service.id,
                &self.client_names,
                &self.topic_names,
                (&self.records, &self.content_types, &self.payloads),
                &selected_slots,
                Checkpoint::image_limits(image_maximum),
            )?;
            // Retain registry names for unselected persistent topics, without copying their records.
            for topic in &self.config.topics {
                if topic.owner_id == self.config.service.id
                    && topic.storage.persistent()
                    && !image.topics.contains(&topic.name)
                {
                    image.topics.push(topic.name.clone());
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
            Ok(Checkpoint {
                image,
                revision,
                next_message: self.next_message,
                reserved_ids: self.message_ids.reserved_position(),
                cutoff: self.last_cleanup_now,
                produced,
                topics,
            })
        })();
        let content = self.content.as_ref().ok_or_else(ApiError::internal)?;
        match snapshot {
            Ok(snapshot) => content.submit(
                snapshot,
                selected,
                self.security
                    .as_ref()
                    .map(|journal| journal.durability_fence()),
            )?,
            Err(error) => {
                content.capture_failed(&error);
                return Err(error);
            }
        }
        Ok(revision)
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
            self.topics
                .get_mut(topic)
                .ok_or_else(ApiError::internal)?
                .messages
                .insert(record.seq(), slot);
            let client = self
                .client_names
                .get(record.producer_slot())
                .ok_or_else(ApiError::internal)?;
            self.own
                .get_mut(client)
                .ok_or_else(ApiError::internal)?
                .insert(record.own_pos(), slot);
            self.index.insert(record.id(), slot);
            self.expiry.insert((record.until(), slot));
            self.active_bytes = self
                .active_bytes
                .checked_add(record.charge(&self.content_types, &self.payloads))
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
