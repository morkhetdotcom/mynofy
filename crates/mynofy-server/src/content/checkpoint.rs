use super::*;
use crate::{
    config::{Policy, Topic},
    message_image::{ImageLimits, MessageImage},
};
use std::{
    collections::{HashMap, HashSet},
    io::Write,
};
const HEADER: usize = 128;
#[derive(Clone)]
pub(crate) struct TopicState {
    pub head: u64,
    pub policy: Policy,
}
pub(crate) struct Checkpoint {
    pub image: MessageImage,
    pub revision: u64,
    pub next_message: u64,
    pub reserved_ids: u64,
    pub cutoff: u64,
    pub produced: Vec<u64>,
    pub topics: Vec<TopicState>,
}
/// An isolated RAM epoch, never written directly. May include unselected RAM
/// topics until the content worker projects it into a persistent checkpoint.
pub(crate) struct Capture {
    pub checkpoint: Checkpoint,
    /// None when a sparse selection was already compacted during capture.
    pub persistent_topics: Option<Vec<String>>,
    pub indices: Option<Vec<u32>>,
}
impl Capture {
    pub(crate) fn finish(
        self,
        selected: &[(String, crate::config::TopicStorage)],
        maximum: u64,
    ) -> crate::protocol::Result<Checkpoint> {
        let source = self.checkpoint;
        let Some(persistent_topics) = self.persistent_topics else {
            return Ok(source);
        };
        let slots: Vec<_> = selected
            .iter()
            .map(|(name, _)| {
                source
                    .image
                    .topics
                    .iter()
                    .position(|topic| topic == name)
                    .and_then(|slot| u16::try_from(slot).ok())
                    .ok_or_else(crate::protocol::ApiError::internal)
            })
            .collect::<crate::protocol::Result<_>>()?;
        // Bound the image itself here; Checkpoint::write and topics::save count
        // actual serialized headers and aggregate files, never live RAM topics.
        // Keep all producer bindings for unselected, previously committed topics.
        // Checkpoint::select would trim that registry to the selected producers.
        let mut image = MessageImage::selected_indexed(
            &source.image.owner,
            &source.image.clients,
            &source.image.topics,
            (
                &source.image.records,
                &source.image.types,
                &source.image.blobs,
            ),
            &slots,
            Checkpoint::image_limits(maximum),
            self.indices.as_deref(),
        )?;
        for name in persistent_topics {
            if !image.topics.contains(&name) {
                image.topics.push(name);
            }
        }
        let topics = image
            .topics
            .iter()
            .map(|name| {
                let slot = source
                    .image
                    .topics
                    .iter()
                    .position(|topic| topic == name)
                    .ok_or_else(crate::protocol::ApiError::internal)?;
                Ok(TopicState {
                    head: source.topics[slot].head,
                    policy: source.topics[slot].policy.clone(),
                })
            })
            .collect::<crate::protocol::Result<_>>()?;
        Ok(Checkpoint {
            image,
            revision: source.revision,
            next_message: source.next_message,
            reserved_ids: source.reserved_ids,
            cutoff: source.cutoff,
            produced: source.produced,
            topics,
        })
    }
}
struct Bounded {
    bytes: Zeroizing<Vec<u8>>,
    maximum: usize,
}
impl Write for Bounded {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if data.len() > self.maximum.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("Content snapshot exceeds size limit"));
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn u32_at(data: &[u8], offset: usize) -> u32 {
    let mut bytes = [0; 4];
    bytes.copy_from_slice(&data[offset..offset + 4]);
    u32::from_le_bytes(bytes)
}
fn u64_at(data: &[u8], offset: usize) -> u64 {
    let mut bytes = [0; 8];
    bytes.copy_from_slice(&data[offset..offset + 8]);
    u64::from_le_bytes(bytes)
}
impl Checkpoint {
    pub(crate) fn image_limits(maximum: u64) -> ImageLimits {
        ImageLimits {
            max_bytes: maximum,
            max_records: (maximum / 80) as usize,
            max_record_chunks: 65536,
            max_blob_pages: 65536,
        }
    }
    pub(crate) fn select(&self, selected: &[u16], maximum: u64) -> crate::protocol::Result<Self> {
        self.select_indexed(selected, maximum, None)
    }
    pub(crate) fn select_indexed(
        &self,
        selected: &[u16],
        maximum: u64,
        indices: Option<&[u32]>,
    ) -> crate::protocol::Result<Self> {
        self.project(selected, maximum, indices, true)
    }
    /// Build a single persisted topic from its already indexed record slots.
    pub(super) fn select_topic(
        &self,
        topic: u16,
        slots: &[u32],
        maximum: u64,
    ) -> crate::protocol::Result<Self> {
        let mut used = std::collections::BTreeSet::new();
        for &slot in slots {
            let record = self
                .image
                .records
                .get(slot)
                .ok_or_else(crate::protocol::ApiError::internal)?;
            if record.topic() != topic {
                return Err(crate::protocol::ApiError::internal());
            }
            used.insert(record.producer_slot());
        }
        let mut mapping = std::collections::HashMap::with_capacity(used.len());
        let mut clients = Vec::with_capacity(used.len());
        let mut produced = Vec::with_capacity(used.len());
        for old in used {
            let client = self
                .image
                .clients
                .get(old)
                .ok_or_else(crate::protocol::ApiError::internal)?;
            let count = self
                .produced
                .get(old)
                .ok_or_else(crate::protocol::ApiError::internal)?;
            mapping.insert(old, clients.len() as u16);
            clients.push(client.clone());
            produced.push(*count);
        }
        let image = MessageImage::selected_slots_mapped(
            &self.image.owner,
            (clients, |slot| mapping.get(&slot).copied()),
            &self.image.topics,
            (&self.image.records, &self.image.types, &self.image.blobs),
            &[topic],
            Self::image_limits(maximum),
            slots.iter().copied(),
        )?;
        Ok(Self {
            image,
            revision: self.revision,
            next_message: self.next_message,
            reserved_ids: self.reserved_ids,
            cutoff: self.cutoff,
            produced,
            topics: vec![self.topics[usize::from(topic)].clone()],
        })
    }
    /// Keep bindings used by topic files that the caller reuses without rewriting.
    pub(super) fn select_shared_registry(
        &self,
        selected: &[u16],
        maximum: u64,
    ) -> crate::protocol::Result<Self> {
        self.project(selected, maximum, None, false)
    }
    fn project(
        &self,
        selected: &[u16],
        maximum: u64,
        indices: Option<&[u32]>,
        compact_clients: bool,
    ) -> crate::protocol::Result<Self> {
        let mut image = MessageImage::selected_indexed(
            &self.image.owner,
            &self.image.clients,
            &self.image.topics,
            (&self.image.records, &self.image.types, &self.image.blobs),
            selected,
            Self::image_limits(maximum),
            indices,
        )?;
        let mut produced = self.produced.clone();
        if compact_clients && !selected.is_empty() {
            let mut used = vec![false; image.clients.len()];
            for record in image.records.iter() {
                used[record.producer_slot()] = true;
            }

            let mut mapping = vec![None; used.len()];
            let mut clients = Vec::new();
            produced.clear();
            for (old, name) in image.clients.iter().enumerate() {
                if used[old] {
                    mapping[old] = Some(clients.len() as u16);
                    clients.push(name.clone());
                    produced.push(self.produced[old]);
                }
            }
            image.records.remap_clients(&mapping)?;

            image.clients = clients;
        }
        Ok(Self {
            image,
            revision: self.revision,
            next_message: self.next_message,
            reserved_ids: self.reserved_ids,
            cutoff: self.cutoff,
            produced,
            topics: selected
                .iter()
                .map(|slot| TopicState {
                    head: self.topics[*slot as usize].head,
                    policy: self.topics[*slot as usize].policy.clone(),
                })
                .collect(),
        })
    }
    pub(crate) fn discard_expired(&mut self, now: u64) {
        let slots: Vec<_> = self
            .image
            .records
            .iter_slots()
            .filter(|(_, record)| record.until() <= now)
            .map(|(slot, _)| slot)
            .collect();
        for slot in slots {
            if let Some(record) = self.image.records.remove(slot) {
                record
                    .payload()
                    .release(&mut self.image.types, &mut self.image.blobs);
            }
        }
    }
    pub(crate) fn merge(
        &mut self,
        other: Self,
        maximum: u64,
        bindings: &HashMap<String, u16>,
    ) -> std::result::Result<(), BoxError> {
        if self.image.owner != other.image.owner {
            return Err("Topic checkpoint owner mismatch".into());
        }
        let mut clients = Vec::new();
        for (name, produced) in other.image.clients.iter().zip(&other.produced) {
            let index = *bindings
                .get(name)
                .ok_or("Topic checkpoint refers to an unknown shared client")?;
            if *produced > self.produced[index as usize] {
                return Err("Topic producer position exceeds committed owner metadata".into());
            }
            clients.push(index);
        }
        let mut topics = Vec::new();
        for (name, state) in other.image.topics.iter().zip(other.topics) {
            topics.push(u16::try_from(self.image.topics.len())?);
            self.image.topics.push(name.clone());
            self.topics.push(state);
        }
        self.image
            .append(&other.image, &topics, &clients, maximum)?;

        self.next_message = self.next_message.max(other.next_message);
        self.cutoff = self.cutoff.max(other.cutoff);
        Ok(())
    }
    pub fn write(&self, maximum: u64) -> std::result::Result<Zeroizing<Vec<u8>>, BoxError> {
        let mut output = Bounded {
            bytes: Zeroizing::new(vec![0; HEADER]),
            maximum: usize::try_from(maximum)?,
        };
        for position in &self.produced {
            output.write_all(&position.to_le_bytes())?;
        }
        for topic in &self.topics {
            for value in [
                topic.head,
                topic.policy.default_ttl_seconds,
                topic.policy.max_ttl_seconds,
                0,
            ] {
                output.write_all(&value.to_le_bytes())?;
            }
        }

        self.image.write_to(&mut output)?;
        let header = &mut output.bytes[..HEADER];
        header[..8].copy_from_slice(b"MYNCP001");
        header[8..12].copy_from_slice(&1u32.to_le_bytes());
        for (offset, value) in [(12, self.produced.len()), (16, self.topics.len()), (20, 0)] {
            header[offset..offset + 4].copy_from_slice(&u32::try_from(value)?.to_le_bytes());
        }
        for (offset, value) in [
            (24, self.revision),
            (32, self.next_message),
            (40, 0),
            (48, self.reserved_ids),
            (56, self.cutoff),
        ] {
            header[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        let payload_hash = crypto::sha256(&output.bytes[HEADER..]);
        output.bytes[64..96].copy_from_slice(&payload_hash);
        let header_hash = crypto::sha256(&output.bytes[..96]);
        output.bytes[96..128].copy_from_slice(&header_hash);
        Ok(output.bytes)
    }
    pub fn read(
        bytes: &[u8],
        config: &Config,
        maximum: u64,
    ) -> std::result::Result<Self, BoxError> {
        if bytes.len() < HEADER
            || bytes.len() as u64 > maximum
            || &bytes[..8] != b"MYNCP001"
            || u32_at(bytes, 8) != 1
            || crypto::sha256(&bytes[..96]) != bytes[96..128]
            || crypto::sha256(&bytes[HEADER..]) != bytes[64..96]
        {
            return Err("Invalid or corrupt content checkpoint".into());
        }
        let clients = u32_at(bytes, 12) as usize;
        let topics = u32_at(bytes, 16) as usize;
        let pairs = u32_at(bytes, 20) as usize;
        let size = clients
            .checked_mul(8)
            .and_then(|value| {
                topics
                    .checked_mul(32)
                    .and_then(|topics| value.checked_add(topics))
            })
            .and_then(|value| {
                pairs
                    .checked_mul(24)
                    .and_then(|pairs| value.checked_add(pairs))
            })
            .ok_or("Invalid checkpoint sizes")?;
        if clients > 10_000 || topics > 10_000 || size > bytes.len() - HEADER {
            return Err("Invalid content checkpoint counts".into());
        }
        let mut at = HEADER;
        let mut produced = Vec::with_capacity(clients);
        for _ in 0..clients {
            produced.push(u64_at(bytes, at));
            at += 8;
        }
        let mut topic_states = Vec::with_capacity(topics);
        for _ in 0..topics {
            let head = u64_at(bytes, at);
            let default = u64_at(bytes, at + 8);
            let maximum = u64_at(bytes, at + 16);
            if default == 0
                || default > maximum
                || maximum > crate::config::MAX_MESSAGE_TTL_SECONDS
                || u64_at(bytes, at + 24) != 0
            {
                return Err("Invalid saved topic policy".into());
            }
            topic_states.push(TopicState {
                head,
                policy: Policy {
                    default_ttl_seconds: default,
                    max_ttl_seconds: maximum,
                },
            });
            at += 32;
        }
        let mut pair_states = Vec::with_capacity(pairs);
        let mut seen = HashSet::new();
        let next_delivery = u64_at(bytes, 40);
        for _ in 0..pairs {
            let reference = u64_at(bytes, at);
            let mut raw = [0; 16];
            raw.copy_from_slice(&bytes[at + 8..at + 24]);
            if reference == 0
                || reference > next_delivery
                || !seen.insert(reference)
                || raw[10] > 2
                || raw[11..].iter().any(|byte| *byte != 0)
            {
                return Err("Invalid saved confirmation reference".into());
            }
            // Legacy MYNCP001 confirmations are validated, then discarded. New
            // snapshots write no confirmations or delivery allocator state.
            pair_states.push((
                crate::ids::Id(raw[..8].try_into().unwrap()),
                u16::from_le_bytes(raw[8..10].try_into().unwrap()),
            ));
            at += 24;
        }
        let image = MessageImage::read_from(&bytes[at..], Self::image_limits(maximum))?;
        if image.owner() != config.service.id
            || image.clients.len() != clients
            || image.topics.len() != topics
        {
            return Err("Content owner or registry mismatch".into());
        }
        let cutoff = u64_at(bytes, 56);
        let reserved_ids = u64_at(bytes, 48);
        let next_message = u64_at(bytes, 32);
        if cutoff > 253_402_300_799_999
            || reserved_ids >= crate::ids::SPACE
            || produced.iter().any(|value| *value > next_message)
            || topic_states.iter().any(|topic| topic.head > next_message)
        {
            return Err("Invalid saved content counters".into());
        }
        let records: HashMap<_, _> = image
            .records
            .iter()
            .map(|record| (record.id(), record))
            .collect();
        let mut combinations = HashSet::new();
        // Policies govern admission, not the lifetime of previously accepted records.
        // Retain the protocol-wide bound while allowing later policy reductions.
        for record in records.values() {
            if record.created_time() > cutoff
                || record.own_pos() > produced[record.producer_slot()]
                || record.seq() > topic_states[record.topic() as usize].head
                || record.until() - record.created_time()
                    > crate::config::MAX_MESSAGE_TTL_SECONDS * 1000
            {
                return Err("Record exceeds saved content positions".into());
            }
        }
        for (message, client) in &pair_states {
            if *client as usize >= clients
                || !records.contains_key(message)
                || !combinations.insert((*message, *client))
            {
                return Err("Invalid saved message/client confirmation".into());
            }
        }
        Ok(Self {
            image,
            revision: u64_at(bytes, 24),
            next_message,
            reserved_ids,
            cutoff,
            produced,
            topics: topic_states,
        })
    }
    pub fn prepare_topics(&self, config: &mut Config) -> std::result::Result<(), BoxError> {
        let mut existing: HashMap<_, _> = config
            .topics
            .drain(..)
            .map(|topic| (topic.name.clone(), topic))
            .collect();
        let mut ordered = Vec::new();
        for (name, state) in self.image.topics.iter().zip(&self.topics) {
            let mut topic = existing.remove(name).unwrap_or_else(|| Topic {
                name: name.clone(),
                owner_id: config.service.id.clone(),
                policy: None,
                storage: crate::config::TopicStorage::Ram {},
                acl: Some(Vec::new()),
                via: None,
            });
            if topic.owner_id != config.service.id {
                return Err("Saved local topic conflicts with configured remote topic".into());
            }
            topic.policy = Some(state.policy.clone());
            ordered.push(topic);
        }
        let mut rest: Vec<_> = existing.into_values().collect();
        rest.sort_by(|a, b| a.name.cmp(&b.name));
        ordered.extend(rest);
        config.topics = ordered;
        config.validate().map_err(Into::into)
    }
    pub fn order_clients(&self, config: &mut Config) -> std::result::Result<(), BoxError> {
        let mut existing: HashMap<_, _> = config
            .clients
            .drain(..)
            .map(|client| (client.client_id.clone(), client))
            .collect();
        let mut ordered = Vec::new();
        for id in &self.image.clients {
            ordered.push(
                existing
                    .remove(id)
                    .ok_or("Saved content refers to a client absent from current security state")?,
            );
        }
        let mut rest: Vec<_> = existing.into_values().collect();
        rest.sort_by(|a, b| a.client_id.cmp(&b.client_id));
        ordered.extend(rest);
        config.clients = ordered;
        Ok(())
    }
}
