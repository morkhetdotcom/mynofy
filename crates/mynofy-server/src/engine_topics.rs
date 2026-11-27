//! Local topic administration. Content topology is RAM state until explicitly saved/exported.
use super::*;
use crate::config::{Topic as ConfigTopic, TopicStorage};
use std::ops::Bound::{Excluded, Unbounded};

impl Engine {
    fn topic_active_count(&self, topic: &Topic, now: u64) -> Result<usize> {
        // Sequential arena scans stay efficient for broad selections; use the
        // index when it avoids visiting at least seven eighths of the owner.
        if topic.messages.len().saturating_mul(8) >= self.records.len() {
            return Ok(self
                .records
                .iter()
                .filter(|r| r.topic() == topic.slot && r.until() > now)
                .count());
        }
        topic.messages.range(..).try_fold(0, |count, (_, slot)| {
            let record = self.records.get(slot).ok_or_else(ApiError::internal)?;
            Ok(count + usize::from(record.until() > now))
        })
    }
    pub(super) fn admin_topic(&self, name: &str, now: u64) -> Result<Value> {
        let topic = self.topics.get(name).ok_or_else(topic_not_found)?;
        let active = self.topic_active_count(topic, now)?;
        Ok(
            json!({"topic":name,"owner_id":self.config.service.id,"policy":topic.policy,"storage":topic.storage,"active_messages":active,"resident_messages":topic.messages.len()}),
        )
    }
    pub(super) fn admin_topics_list(
        &self,
        after: Option<String>,
        limit: Option<u16>,
        now: u64,
    ) -> Result<Value> {
        let limit = usize::from(limit.unwrap_or(20));
        if !(1..=100).contains(&limit)
            || after
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 256)
        {
            return Err(ApiError::invalid());
        }
        let start = after.as_deref().map_or(Unbounded, Excluded);
        let mut names = self
            .topics
            .range::<str, _>((start, Unbounded))
            .take(limit + 1);
        let selected: Vec<_> = names.by_ref().take(limit).collect();
        let next = if names.next().is_some() {
            selected.last().map(|(name, _)| *name)
        } else {
            None
        };
        if selected.is_empty() {
            return Ok(json!({"items":[],"next_after":null}));
        }
        let selected_records: usize = selected.iter().map(|(_, topic)| topic.messages.len()).sum();
        let active =
            if selected_records != 0 && selected_records.saturating_mul(8) >= self.records.len() {
                // Broad pages retain one sequential pass, rather than many index lookups.
                let mut counts = vec![0usize; self.topic_names.len()];
                for record in self.records.iter().filter(|r| r.until() > now) {
                    counts[record.topic() as usize] += 1;
                }
                Some(counts)
            } else {
                None
            };
        let items = selected.iter().map(|(name, topic)| {
            let count = match &active {
                Some(counts) => counts[topic.slot as usize],
                None if topic.messages.is_empty() => 0,
                None => self.topic_active_count(topic, now)?,
            };
            Ok(json!({"topic":name,"owner_id":self.config.service.id,"policy":topic.policy,"storage":topic.storage,"active_messages":count,"resident_messages":topic.messages.len()}))
        }).collect::<Result<Vec<_>>>()?;
        Ok(json!({"items":items,"next_after":next}))
    }
    fn topics_idle(&self) -> Result<()> {
        if let Some(content) = &self.content {
            content.topics_idle()?;
        }
        Ok(())
    }
    pub(super) fn admin_topic_create(
        &mut self,
        name: String,
        policy: Policy,
        now: u64,
    ) -> Result<Value> {
        self.topics_idle()?;
        if self.topic_owners.contains_key(&name) {
            return Err(ApiError::new("conflict", "Topic already exists."));
        }
        if self.config.topics.len() >= 10_000 {
            return Err(ApiError::exhausted());
        }
        let mut candidate = self.config.clone();
        candidate.topics.push(ConfigTopic {
            name: name.clone(),
            owner_id: candidate.service.id.clone(),
            policy: Some(policy.clone()),
            storage: TopicStorage::default(),
            acl: Some(Vec::new()),
            via: None,
        });
        candidate.validate().map_err(|_| ApiError::invalid())?;
        let permit = self.security_reserve()?;
        self.security_check(&crate::security::Change::TopicReset {
            topic: name.clone(),
            transient: true,
        })?;
        let slot = self.topic_names.len() as u16;
        self.topics.insert(
            name.clone(),
            Topic {
                active_bytes: 0,
                slot,
                head: self.next_message,
                messages: OrderedIndex::new(),
                policy,
                storage: TopicStorage::default(),
            },
        );
        self.topic_names.push(name.clone());
        if let Some(content) = &mut self.content {
            content.add_ram_topic(name.clone());
        }
        self.config = candidate;
        self.topic_owners
            .insert(name.clone(), self.config.service.id.clone());
        self.security_submit(
            permit,
            crate::security::Change::TopicReset {
                topic: name.clone(),
                transient: true,
            },
        )?;
        let mut result = self.admin_topic(&name, now)?;
        result["durable"] = false.into();
        Ok(result)
    }
    pub(super) fn admin_topic_policy(
        &mut self,
        name: String,
        policy: Policy,
        now: u64,
    ) -> Result<Value> {
        self.topics_idle()?;
        let slot = self.topics.get(&name).ok_or_else(topic_not_found)?.slot;
        let mut candidate = self.config.clone();
        candidate
            .topics
            .iter_mut()
            .find(|t| t.name == name)
            .ok_or_else(ApiError::internal)?
            .policy = Some(policy.clone());
        candidate.validate().map_err(|_| ApiError::invalid())?;
        self.topics
            .get_mut(&name)
            .ok_or_else(ApiError::internal)?
            .policy = policy;
        self.config = candidate;
        self.content_changed(slot);
        let mut result = self.admin_topic(&name, now)?;
        result["durable"] = false.into();
        Ok(result)
    }
    pub(super) fn admin_topic_delete(&mut self, name: String) -> Result<Value> {
        self.topics_idle()?;
        let topic = self.topics.get(&name).ok_or_else(topic_not_found)?;
        if self
            .config
            .accepted_peers
            .iter()
            .any(|peer| peer.topics.contains(&name))
        {
            return Err(ApiError::new(
                "topic_in_use",
                "Remove peer topic grants before deleting this topic.",
            ));
        }

        if topic.storage.persistent() {
            return Err(ApiError::new(
                "storage_transition_forbidden",
                "Return stored topics to RAM offline before deleting them.",
            ));
        }
        if !topic.messages.is_empty() || self.records.iter().any(|r| r.topic() == topic.slot) {
            return Err(ApiError::new(
                "topic_not_empty",
                "Wait for expiry and physical cleanup before deleting the topic.",
            ));
        }
        let slot = topic.slot;
        let last = (self.topic_names.len() - 1) as u16;
        let affected: Vec<_> = self
            .acl
            .entries()
            .filter(|(_, t, _)| *t == slot)
            .map(|(c, _, _)| self.client_names[c as usize].clone())
            .collect();
        for client in &affected {
            self.clients[client]
                .revision
                .checked_add(1)
                .ok_or_else(ApiError::exhausted)?;
        }
        let permit = self.security_reserve()?;
        self.security_check(&crate::security::Change::TopicReset {
            topic: name.clone(),
            transient: false,
        })?;
        self.topics.remove(&name);
        self.topic_names.swap_remove(slot as usize);
        if slot != last {
            self.topics
                .get_mut(&self.topic_names[slot as usize])
                .ok_or_else(ApiError::internal)?
                .slot = slot;
            self.records.move_topic(last, slot);
        }
        self.acl.remove_topic(slot, last);
        if let Some(content) = &mut self.content {
            content.remove_ram_topic(slot as usize);
        }
        self.config.topics.retain(|t| t.name != name);
        self.topic_owners.remove(&name);
        for client in &affected {
            self.clients
                .get_mut(client)
                .ok_or_else(ApiError::internal)?
                .revision += 1;
            self.take_rotation(client);
        }
        let sessions: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|(id, s)| {
                s.auth
                    .as_ref()
                    .filter(|a| affected.contains(&a.client))
                    .map(|_| *id)
            })
            .collect();
        for session in sessions {
            self.disconnect(session);
        }
        self.security_submit(
            permit,
            crate::security::Change::TopicReset {
                topic: name.clone(),
                transient: false,
            },
        )?;
        Ok(json!({"topic":name,"deleted":true,"durable":false}))
    }
}
fn topic_not_found() -> ApiError {
    ApiError::new("not_found", "Local topic not found.")
}
