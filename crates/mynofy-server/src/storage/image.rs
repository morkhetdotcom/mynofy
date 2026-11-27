//! Portable message-storage image. Record/type/blob blocks are written verbatim.
use super::*;
use crate::{BoxError, crypto};
use std::{
    collections::HashSet,
    io::{Read, Write},
};
const HEADER: usize = 96;
const MAGIC: &[u8; 8] = b"MYNIMG01";
/// Bounds for allocation and validation when reading/capturing an image.
#[derive(Clone, Copy)]
pub struct ImageLimits {
    pub max_bytes: u64,
    pub max_records: usize,
    pub max_record_chunks: usize,
    pub max_blob_pages: usize,
}
impl Default for ImageLimits {
    fn default() -> Self {
        Self {
            max_bytes: 64 * 1024 * 1024,
            max_records: 100_000,
            max_record_chunks: 4096,
            max_blob_pages: 65536,
        }
    }
}
/// An owned, consistent copy of message storage, not a full owner checkpoint.
/// Contains identity/topic bindings, but no keys, ACLs, sessions or confirmations.
pub struct MessageImage {
    pub(crate) owner: String,
    pub(crate) clients: Vec<String>,
    pub(crate) topics: Vec<String>,
    pub(crate) records: Arena,
    pub(crate) types: ContentTypes,
    pub(crate) blobs: Blobs,
}
fn invalid() -> BoxError {
    "Invalid, incompatible or corrupt message image".into()
}
impl MessageImage {
    /// Build an isolated image, retaining only selected topic blocks. No wire conversion.
    #[cfg(test)]
    pub(crate) fn selected(
        owner: &str,
        clients: &[String],
        topics: &[String],
        storage: (&Arena, &ContentTypes, &Blobs),
        selected: &[u16],
        limits: ImageLimits,
    ) -> Result<Self> {
        Self::selected_indexed(owner, clients, topics, storage, selected, limits, None)
    }
    pub(crate) fn selected_indexed(
        owner: &str,
        clients: &[String],
        topics: &[String],
        storage: (&Arena, &ContentTypes, &Blobs),
        selected: &[u16],
        limits: ImageLimits,
        indices: Option<&[u32]>,
    ) -> Result<Self> {
        let slots: Box<dyn Iterator<Item = u32> + '_> = match indices {
            Some(indices) => Box::new(indices.iter().copied()),
            None => Box::new(storage.0.iter_slots().map(|(slot, _)| slot)),
        };
        Self::selected_slots(owner, clients, topics, storage, selected, limits, slots)
    }

    pub(crate) fn selected_slots(
        owner: &str,
        clients: &[String],
        topics: &[String],
        storage: (&Arena, &ContentTypes, &Blobs),
        selected: &[u16],
        limits: ImageLimits,
        slots: impl Iterator<Item = u32>,
    ) -> Result<Self> {
        Self::selected_slots_mapped(
            owner,
            (clients.to_vec(), |slot| u16::try_from(slot).ok()),
            topics,
            storage,
            selected,
            limits,
            slots,
        )
    }

    pub(crate) fn selected_slots_mapped(
        owner: &str,
        registry: (Vec<String>, impl Fn(usize) -> Option<u16>),
        topics: &[String],
        storage: (&Arena, &ContentTypes, &Blobs),
        selected: &[u16],
        limits: ImageLimits,
        slots: impl Iterator<Item = u32>,
    ) -> Result<Self> {
        let (clients, producer) = registry;
        let (records, types, blobs) = storage;
        let mut output = Self {
            owner: owner.into(),
            clients,
            topics: selected
                .iter()
                .map(|slot| topics[*slot as usize].clone())
                .collect(),
            records: Arena::new(),
            types: ContentTypes::default(),
            blobs: Blobs::default(),
        };
        let mapping = if selected.len() == 1 {
            Vec::new()
        } else {
            let mut mapping = vec![None; topics.len()];
            for (new, old) in selected.iter().enumerate() {
                mapping[*old as usize] = Some(new as u16);
            }
            mapping
        };
        let registry_bytes = 512
            + owner.len()
            + output
                .clients
                .iter()
                .chain(&output.topics)
                .map(|n| n.len() + 48)
                .sum::<usize>();
        for slot in slots {
            let record = records.get(slot).ok_or_else(ApiError::internal)?;
            let topic = if let [only] = selected {
                (record.topic() == *only).then_some(0)
            } else {
                mapping[record.topic() as usize]
            };
            let Some(topic) = topic else {
                continue;
            };
            let relocated = record.relocate(
                producer(record.producer_slot()).ok_or_else(ApiError::internal)?,
                topic,
                types,
                blobs,
                &mut output.types,
                &mut output.blobs,
            )?;
            output
                .records
                .insert(relocated)
                .map_err(|_| ApiError::exhausted())?;
            // Conservative framing/page budget, bounded by at most one insertion's growth.
            let size = registry_bytes
                + output.records.chunks.len() * (arena::BYTES + 48)
                + output.blobs.pages.len() * (16520 + 48)
                + output.types.entries.len();
            if size as u64 > limits.max_bytes
                || output.records.len() > limits.max_records
                || output.records.chunks.len() > limits.max_record_chunks
                || output.blobs.pages.len() > limits.max_blob_pages
            {
                return Err(ApiError::exhausted());
            }
        }
        if registry_bytes as u64 > limits.max_bytes {
            return Err(ApiError::exhausted());
        }
        Ok(output)
    }
    pub(crate) fn append(
        &mut self,
        other: &Self,
        topic_map: &[u16],
        client_map: &[u16],
        maximum: u64,
    ) -> Result<()> {
        for record in other.records.iter() {
            let relocated = record.relocate(
                client_map[record.producer_slot()],
                topic_map[record.topic() as usize],
                &other.types,
                &other.blobs,
                &mut self.types,
                &mut self.blobs,
            )?;
            self.records
                .insert(relocated)
                .map_err(|_| ApiError::exhausted())?;
            if (self.records.chunks.len() * (arena::BYTES + 48)
                + self.blobs.pages.len() * (16520 + 48)
                + self.types.entries.len()) as u64
                > maximum
            {
                return Err(ApiError::exhausted());
            }
        }
        Ok(())
    }
    /// Build a read-only sparse source for worker projection. The slot list
    /// is the only enumeration of selected records. Pinned chunks can also
    /// contain unrelated records and must never be serialized.
    pub(crate) fn pin_selected(
        owner: &str,
        clients: &[String],
        topics: &[String],
        storage: (&mut Arena, &ContentTypes, &mut Blobs),
        indices: &[u32],
        extra_budget: usize,
    ) -> Option<(Self, usize)> {
        let (records, types, blobs) = storage;
        let mut charge = indices
            .len()
            .checked_mul(32)?
            .checked_add(
                records
                    .chunks
                    .len()
                    .checked_mul(size_of::<Option<arena::Chunk>>())?,
            )?
            .checked_add(
                blobs
                    .pages
                    .len()
                    .checked_mul(size_of::<Option<blobs::Page>>())?,
            )?
            .checked_add(types.entries.len())?;
        if charge > extra_budget {
            return None;
        }
        let mut chunk_ids = HashSet::new();
        let mut page_ids = HashSet::new();
        for &slot in indices {
            let id = slot as usize / arena::CHUNK;
            if chunk_ids.insert(id) {
                // Allow both the pinned block and a concurrent detached copy.
                charge = charge.checked_add(2 * (arena::BYTES + 64))?;
            }
            let record = records.get(slot)?;
            let payload = record.payload();
            let location = (payload.external_bytes() != 0).then(|| u64_at(payload.data, 8));
            let ty = (payload.content_type != 0)
                .then(|| u64_at(&types.entries, (payload.content_type as usize - 1) * 16));
            for location in [location, ty]
                .into_iter()
                .flatten()
                .filter(|&location| location != 0)
            {
                let id = ((location >> 32) - 1) as usize;
                if page_ids.insert(id) {
                    charge =
                        charge.checked_add(2 * (blobs.pages[id].as_ref()?.bytes.len() + 64))?;
                }
            }
            if charge > extra_budget {
                return None;
            }
        }
        let mut snapshot_records = Arena::new();
        snapshot_records
            .chunks
            .resize_with(records.chunks.len(), || None);
        for id in chunk_ids {
            let chunk = records.chunks[id].as_mut()?;
            chunk.bytes.freeze();
            snapshot_records.chunks[id] = Some(chunk.clone());
        }
        let mut snapshot_blobs = Blobs::default();
        snapshot_blobs.pages.resize_with(blobs.pages.len(), || None);
        for id in page_ids {
            let page = blobs.pages[id].as_mut()?;
            page.bytes.freeze();
            snapshot_blobs.pages[id] = Some(page.clone());
        }
        Some((
            Self {
                owner: owner.to_owned(),
                clients: clients.to_vec(),
                topics: topics.to_vec(),
                records: snapshot_records,
                types: ContentTypes {
                    entries: types.entries.clone(),
                    ..Default::default()
                },
                blobs: snapshot_blobs,
            },
            charge,
        ))
    }

    /// Pin initialized blocks in one owner epoch. Later live writes detach only
    /// the affected block; unique writes outside snapshot lifetimes stay ordinary.
    pub(crate) fn capture_shared(
        owner: &str,
        clients: &[String],
        topics: &[String],
        records: &mut Arena,
        types: &ContentTypes,
        blobs: &mut Blobs,
        limits: ImageLimits,
    ) -> Result<Self> {
        records.freeze();
        blobs.freeze();
        Self::capture(owner, clients, topics, records, types, blobs, limits)
    }
    pub(crate) fn capture(
        owner: &str,
        clients: &[String],
        topics: &[String],
        records: &Arena,
        types: &ContentTypes,
        blobs: &Blobs,
        limits: ImageLimits,
    ) -> Result<Self> {
        let bytes = HEADER as u64
            + records
                .chunks
                .iter()
                .flatten()
                .map(|chunk| chunk.bytes.len() as u64)
                .sum::<u64>()
            + blobs
                .pages
                .iter()
                .flatten()
                .map(|page| page.bytes.len() as u64)
                .sum::<u64>()
            + types.entries.len() as u64
            + ((records.chunks.len() + blobs.pages.len() + clients.len() + topics.len() + 2)
                as u64)
                * 48
            + owner.len() as u64
            + clients
                .iter()
                .chain(topics)
                .map(|name| name.len() as u64)
                .sum::<u64>();
        if bytes > limits.max_bytes
            || records.len() > limits.max_records
            || records.chunks.len() > limits.max_record_chunks
            || blobs.pages.len() > limits.max_blob_pages
        {
            return Err(ApiError::exhausted());
        }
        Ok(Self {
            owner: owner.into(),
            clients: clients.to_vec(),
            topics: topics.to_vec(),
            records: records.clone(),
            types: types.clone(),
            blobs: blobs.clone(),
        })
    }
    pub fn owner(&self) -> &str {
        &self.owner
    }
    pub fn message_count(&self) -> usize {
        self.records.len()
    }
    /// Inspect a message offline; this does not authenticate a client or grant topic rights.
    pub fn message(&self, id: &str) -> Option<pb::StoredMessage> {
        let id = Id::parse(id)?;
        let record = self.records.iter().find(|record| record.id() == id)?;
        Some(record.wire(
            (
                self.topics.get(record.topic() as usize)?.as_str(),
                self.clients.get(record.producer_slot())?.as_str(),
            ),
            &self.types,
            &self.blobs,
            crate::protocol::timestamp(record.until()).ok()?,
        ))
    }
    /// Framing/checksums are generated; record, type and payload blocks are not re-encoded.
    /// Caller controls file creation, atomic replacement and filesystem synchronization.
    pub fn write_to(&self, mut writer: impl Write) -> std::result::Result<(), BoxError> {
        let mut header = [0; HEADER];
        put(&mut header, 0, MAGIC);
        put(&mut header, 8, &1u32.to_le_bytes());
        put(&mut header, 12, &(RECORD_BYTES as u32).to_le_bytes());
        for (offset, count) in [
            (16, self.records.chunks.len()),
            (20, self.blobs.pages.len()),
            (24, self.types.entries.len()),
            (28, self.clients.len()),
            (32, self.topics.len()),
        ] {
            let count = u32::try_from(count)?;
            put(&mut header, offset, &count.to_le_bytes());
        }
        put(&mut header, 40, &(self.records.len() as u64).to_le_bytes());
        let hash = crypto::sha256(&header[..64]);
        put(&mut header, 64, &hash);
        writer.write_all(&header)?;
        let mut index = 0u64;
        let mut block = |bytes: &[u8]| -> std::result::Result<(), BoxError> {
            let mut frame = [0; 48];
            put(&mut frame, 0, &index.to_le_bytes());
            put(&mut frame, 8, &u32::try_from(bytes.len())?.to_le_bytes());
            put(&mut frame, 16, &crypto::sha256(bytes));
            writer.write_all(&frame)?;
            writer.write_all(bytes)?;
            index += 1;
            Ok(())
        };
        block(self.owner.as_bytes())?;
        for name in self.clients.iter().chain(&self.topics) {
            block(name.as_bytes())?;
        }
        block(&self.types.entries)?;
        for chunk in &self.records.chunks {
            block(chunk.as_ref().map_or(&[], |chunk| chunk.bytes.as_ref()))?;
        }
        for page in &self.blobs.pages {
            block(page.as_ref().map_or(&[], |page| page.bytes.as_ref()))?;
        }
        Ok(())
    }
    /// Read bytes directly into the runtime blocks, validate them, then rebuild transient allocators.
    pub fn read_from(
        mut reader: impl Read,
        limits: ImageLimits,
    ) -> std::result::Result<Self, BoxError> {
        if limits.max_bytes < HEADER as u64 {
            return Err(invalid());
        }
        let mut header = [0; HEADER];
        reader.read_exact(&mut header)?;
        if &header[..8] != MAGIC
            || u32_at(&header, 8) != 1
            || u32_at(&header, 12) as usize != RECORD_BYTES
            || header[36..40]
                .iter()
                .chain(&header[48..64])
                .any(|byte| *byte != 0)
            || crypto::sha256(&header[..64]) != header[64..96]
        {
            return Err(invalid());
        }
        let chunks = u32_at(&header, 16) as usize;
        let pages = u32_at(&header, 20) as usize;
        let table = u32_at(&header, 24) as usize;
        let clients = u32_at(&header, 28) as usize;
        let topics = u32_at(&header, 32) as usize;
        let count = u64_at(&header, 40);
        if chunks > limits.max_record_chunks
            || chunks > u32::MAX as usize / arena::CHUNK
            || pages > limits.max_blob_pages
            || clients > 10_000
            || topics > 10_000
            || count > limits.max_records as u64
            || !table.is_multiple_of(16)
            || table / 16 > limits.max_records
        {
            return Err(invalid());
        }
        let mut budget = limits.max_bytes - HEADER as u64;
        let mut index = 0u64;
        let mut block = |maximum: usize| -> std::result::Result<Box<[u8]>, BoxError> {
            if budget < 48 {
                return Err(invalid());
            }
            budget -= 48;
            let mut frame = [0; 48];
            reader.read_exact(&mut frame)?;
            let len = u32_at(&frame, 8) as usize;
            if u64_at(&frame, 0) != index
                || frame[12..16].iter().any(|byte| *byte != 0)
                || len > maximum
                || len as u64 > budget
            {
                return Err(invalid());
            }
            budget -= len as u64;
            let mut bytes = vec![0; len].into_boxed_slice();
            reader.read_exact(&mut bytes)?;
            if crypto::sha256(&bytes) != frame[16..48] {
                return Err(invalid());
            }
            index += 1;
            Ok(bytes)
        };
        let owner = String::from_utf8(block(256)?.into_vec())?;
        if owner.is_empty() {
            return Err(invalid());
        }
        let mut client_names = Vec::with_capacity(clients);
        let mut seen = HashSet::new();
        for _ in 0..clients {
            let name = String::from_utf8(block(128)?.into_vec())?;
            if !crypto::valid_client_id(&name) || !seen.insert(name.clone()) {
                return Err(invalid());
            }
            client_names.push(name);
        }
        let mut topic_names = Vec::with_capacity(topics);
        seen.clear();
        for _ in 0..topics {
            let name = String::from_utf8(block(256)?.into_vec())?;
            if name.is_empty() || !seen.insert(name.clone()) {
                return Err(invalid());
            }
            topic_names.push(name);
        }
        let entries = block(table)?;
        if entries.len() != table {
            return Err(invalid());
        }
        let mut raw_chunks = Vec::with_capacity(chunks);
        for _ in 0..chunks {
            let bytes = block(arena::BYTES)?;
            raw_chunks.push((!bytes.is_empty()).then_some(bytes));
        }
        let mut raw_pages = Vec::with_capacity(pages);
        for _ in 0..pages {
            let bytes = block(16 * 1024 + 136)?;
            raw_pages.push((!bytes.is_empty()).then_some(bytes));
        }
        let mut extra = [0];
        if reader.read(&mut extra)? != 0 {
            return Err(invalid());
        }
        let records = Arena::rebuild(raw_chunks)?;
        let blobs = Blobs::rebuild(raw_pages)?;
        if records.len() as u64 != count {
            return Err(invalid());
        }
        let types = ContentTypes {
            entries: entries.into_vec(),
            ..ContentTypes::default()
        };
        let mut image = Self {
            owner,
            clients: client_names,
            topics: topic_names,
            records,
            types,
            blobs,
        };
        image.validate()?;
        Ok(image)
    }
    fn validate(&mut self) -> std::result::Result<(), BoxError> {
        let mut allocations = HashSet::new();
        let mut references = vec![0u32; self.types.entries.len() / 16];
        for (index, entry) in self.types.entries.as_chunks::<16>().0.iter().enumerate() {
            let count = u32_at(entry, 12);
            if count == 0 {
                if entry.iter().any(|byte| *byte != 0) {
                    return Err(invalid());
                }
                self.types.free.push(index as u32);
                continue;
            }
            let location = u64_at(entry, 0);
            let len = u32_at(entry, 8) as usize;
            if len == 0 || len > 128 || !allocations.insert(location) {
                return Err(invalid());
            }
            let name = std::str::from_utf8(self.blobs.checked_data(location, len)?)?.to_owned();
            if self.types.names.insert(name, index as u32 + 1).is_some() {
                return Err(invalid());
            }
        }
        let mut ids = HashSet::new();
        let mut positions = HashSet::new();
        let mut own = HashSet::new();
        for record in self.records.iter() {
            if !record.id().0.iter().all(u8::is_ascii_alphanumeric)
                || !ids.insert(record.id())
                || record.created_time() >= record.until()
                || record.until() > 253_402_300_799_999
                || record.producer_slot() >= self.clients.len()
                || record.topic() as usize >= self.topics.len()
                || record.seq() == 0
                || record.own_pos() == 0
                || !positions.insert((record.topic(), record.seq()))
                || !own.insert((record.producer_slot(), record.own_pos()))
            {
                return Err(invalid());
            }
            let payload = record.payload();
            let data = payload.data;
            let type_id = payload.content_type;
            if type_id != 0 {
                let count = references
                    .get_mut(type_id as usize - 1)
                    .ok_or_else(invalid)?;
                if self.types.name(type_id, &self.blobs).is_none() {
                    return Err(invalid());
                }
                *count = count.checked_add(1).ok_or_else(invalid)?;
            }
            if data[7] != 0 {
                return Err(invalid());
            }
            match data[6] {
                1 => {
                    let len = u32_at(data, 0) as usize;
                    let location = u64_at(data, 8);
                    if len > 4096
                        || data[4..6]
                            .iter()
                            .chain(&data[16..32])
                            .any(|byte| *byte != 0)
                        || (len != 0 && !allocations.insert(location))
                    {
                        return Err(invalid());
                    }
                    let bytes = self.blobs.checked_data(location, len)?;
                    if self.types.name(type_id, &self.blobs)
                        == Some(crate::protocol::payload::LEGACY_JSON)
                    {
                        crate::protocol::strict_json(std::str::from_utf8(bytes)?)?;
                    }
                }
                2 => {
                    if type_id != 0 {
                        return Err(invalid());
                    }
                    let key = u32_at(data, 0);
                    let layout = u16_at(data, 4);
                    if layout > 511
                        || !crate::protocol::catalog_values::valid_category(key & 65535)
                        || !crate::protocol::catalog_values::valid_action((key >> 16) & 255)
                        || !crate::protocol::catalog_values::valid_status(key >> 24)
                    {
                        return Err(invalid());
                    }
                    let mut end = false;
                    let mut at = 8;
                    for index in 0..3 {
                        let kind = (layout >> (index * 3)) & 7;
                        if kind == 0 {
                            end = true;
                            continue;
                        }
                        if end {
                            return Err(invalid());
                        }
                        let len = match kind {
                            1 | 3 | 5 => 4,
                            7 => 1,
                            _ => 8,
                        };
                        if kind == 5 && !f32::from_bits(u32_at(data, at)).is_finite()
                            || kind == 6 && !f64::from_bits(u64_at(data, at)).is_finite()
                            || kind == 7 && data[at] > 1
                        {
                            return Err(invalid());
                        }
                        at += len;
                    }
                    if data[at..32].iter().any(|byte| *byte != 0) {
                        return Err(invalid());
                    }
                }
                _ => return Err(invalid()),
            }
        }
        for (index, count) in references.into_iter().enumerate() {
            if count != u32_at(&self.types.entries, index * 16 + 12) {
                return Err(invalid());
            }
        }
        if allocations.len() != self.blobs.allocations() {
            return Err(invalid());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ProtobufMessage, payload::ParameterValue};
    pub(super) fn fixture() -> MessageImage {
        let mut records = Arena::new();
        let mut types = ContentTypes::default();
        let mut blobs = Blobs::default();
        for (index, payload) in [
            Payload::bytes(vec![], None),
            Payload::bytes(vec![0x73; 32], Some("text/ümlaut".into())),
            Payload::bytes(vec![0x7a; 4096], Some("text/ümlaut".into())),
            Payload::event(Event {
                catalog: 1,
                catalog_version: 1,
                category: 12,
                action: 40,
                status: 2,
                parameters: vec![
                    Parameter {
                        value: Some(ParameterValue::U64(u64::MAX)),
                    },
                    Parameter {
                        value: Some(ParameterValue::F32(-0.0)),
                    },
                    Parameter {
                        value: Some(ParameterValue::Bool(true)),
                    },
                ],
            }),
        ]
        .into_iter()
        .enumerate()
        {
            let payload = StoredPayload::new(payload, &mut types, &mut blobs).unwrap();
            let record = Record::new(
                RecordFields {
                    id: Id::from_index(index as u64),
                    producer: 0,
                    topic: 0,
                    created: 1_800_000_000_000,
                    until: 1_900_000_000_000,
                    seq: index as u64 + 1,
                    own_pos: index as u64 + 1,
                },
                payload,
            );
            records.insert(record).ok().unwrap();
        }
        MessageImage::capture(
            "owner",
            &[crypto::initial_client_id(&[7; 32])],
            &["test/topic".into()],
            &records,
            &types,
            &blobs,
            ImageLimits::default(),
        )
        .unwrap()
    }
    pub(super) fn bytes(image: &MessageImage) -> Vec<u8> {
        let mut bytes = Vec::new();
        image.write_to(&mut bytes).unwrap();
        bytes
    }
    #[test]
    fn indexed_selection_matches_full_scan_and_rejects_unknown_slots() {
        let image = fixture();
        let slots: Vec<_> = image.records.iter_slots().map(|(slot, _)| slot).collect();
        let storage = (&image.records, &image.types, &image.blobs);
        let scanned = MessageImage::selected(
            &image.owner,
            &image.clients,
            &image.topics,
            storage,
            &[0],
            ImageLimits::default(),
        )
        .unwrap();
        let indexed = MessageImage::selected_indexed(
            &image.owner,
            &image.clients,
            &image.topics,
            storage,
            &[0],
            ImageLimits::default(),
            Some(&slots),
        )
        .unwrap();
        assert_eq!(bytes(&scanned), bytes(&indexed));
        assert!(
            MessageImage::selected_indexed(
                &image.owner,
                &image.clients,
                &image.topics,
                storage,
                &[0],
                ImageLimits::default(),
                Some(&[u32::MAX])
            )
            .is_err()
        );
    }
    #[test]
    fn records_blobs_and_types_are_written_verbatim_and_reload_to_the_same_bytes() {
        let image = fixture();
        let bytes = bytes(&image);
        let mut at = HEADER;
        let mut blocks = Vec::new();
        while at < bytes.len() {
            let length = u32_at(&bytes, at + 8) as usize;
            blocks.push(&bytes[at + 48..at + 48 + length]);
            at += 48 + length;
        }
        assert_eq!(blocks[3], image.types.entries);
        assert_eq!(
            blocks[4],
            image.records.chunks[0].as_ref().unwrap().bytes.as_ref()
        );
        for (page, block) in image.blobs.pages.iter().zip(&blocks[5..]) {
            assert_eq!(*block, page.as_ref().unwrap().bytes.as_ref());
        }
        let restored = MessageImage::read_from(bytes.as_slice(), ImageLimits::default()).unwrap();
        assert_eq!(restored.message_count(), 4);
        assert_eq!(restored.owner(), "owner");
        for index in 0..4 {
            let id = Id::from_index(index).to_string();
            assert_eq!(
                image.message(&id).unwrap().encode_to_vec(),
                restored.message(&id).unwrap().encode_to_vec()
            );
        }
        let mut rewritten = Vec::new();
        restored.write_to(&mut rewritten).unwrap();
        assert_eq!(rewritten, bytes);
        let slot = restored.records.get(1).unwrap();
        assert_eq!(slot.created_time(), 1_800_000_000_000);
    }
    #[test]
    fn snapshot_read_rejects_corruption_truncation_versions_trailing_data_and_limits() {
        let original = bytes(&fixture());
        for length in [0, 1, 95, 96, HEADER + 48, original.len() - 1] {
            assert!(MessageImage::read_from(&original[..length], ImageLimits::default()).is_err());
        }
        for offset in [
            0,
            8,
            12,
            64,
            HEADER,
            HEADER + 8,
            HEADER + 16,
            original.len() - 1,
        ] {
            let mut altered = original.clone();
            altered[offset] ^= 1;
            assert!(MessageImage::read_from(altered.as_slice(), ImageLimits::default()).is_err());
        }
        let mut trailing = original.clone();
        trailing.push(0);
        assert!(MessageImage::read_from(trailing.as_slice(), ImageLimits::default()).is_err());
        for limits in [
            ImageLimits {
                max_bytes: 95,
                ..ImageLimits::default()
            },
            ImageLimits {
                max_bytes: original.len() as u64 - 1,
                ..ImageLimits::default()
            },
            ImageLimits {
                max_records: 3,
                ..ImageLimits::default()
            },
            ImageLimits {
                max_record_chunks: 0,
                ..ImageLimits::default()
            },
            ImageLimits {
                max_blob_pages: 0,
                ..ImageLimits::default()
            },
        ] {
            assert!(MessageImage::read_from(original.as_slice(), limits).is_err());
        }
    }
    #[test]
    fn checksummed_but_invalid_records_offsets_padding_and_ownership_are_rejected() {
        for (offset, value) in [(0, b'!'), (40, 100), (42, 100), (48 + 6, 9), (48 + 7, 1)] {
            let mut image = fixture();
            image.records.chunks[0].as_mut().unwrap().bytes[arena::HEADER + offset] = value;
            assert!(
                MessageImage::read_from(bytes(&image).as_slice(), ImageLimits::default()).is_err()
            );
        }
        // Real bytes record: invalid address, nonzero reserved bytes, illegal lengths.
        for (offset, value) in [(56, 1), (48 + 16, 1), (48 + 4, 1), (48 + 1, 255)] {
            let mut image = fixture();
            image.records.chunks[0].as_mut().unwrap().bytes
                [arena::HEADER + RECORD_BYTES + offset] = value;
            assert!(
                MessageImage::read_from(bytes(&image).as_slice(), ImageLimits::default()).is_err()
            );
        }
        let mut image = fixture();
        let chunk = image.records.chunks[0].as_mut().unwrap();
        let id = chunk.bytes[arena::HEADER..arena::HEADER + 8].to_vec();
        chunk.bytes[arena::HEADER + RECORD_BYTES..arena::HEADER + RECORD_BYTES + 8]
            .copy_from_slice(&id);
        assert!(MessageImage::read_from(bytes(&image).as_slice(), ImageLimits::default()).is_err());
        let mut image = fixture();
        image.types.entries[12] = 0;
        assert!(MessageImage::read_from(bytes(&image).as_slice(), ImageLimits::default()).is_err());
        let mut image = fixture();
        image.records.chunks[0].as_mut().unwrap().bytes[arena::HEADER + 10 * RECORD_BYTES] = 1;
        assert!(MessageImage::read_from(bytes(&image).as_slice(), ImageLimits::default()).is_err());
    }
    #[test]
    fn loaded_allocators_reuse_slots_and_removed_data_is_scrubbed() {
        let image = fixture();
        let mut loaded =
            MessageImage::read_from(bytes(&image).as_slice(), ImageLimits::default()).unwrap();
        let removed = loaded.records.remove(2).unwrap();
        removed
            .payload()
            .release(&mut loaded.types, &mut loaded.blobs);
        let serialized = bytes(&loaded);
        assert!(!serialized.windows(64).any(|window| window == [0x7a; 64]));
        let new = StoredPayload::new(
            Payload::bytes(vec![0xab; 4096], None),
            &mut loaded.types,
            &mut loaded.blobs,
        )
        .unwrap();
        let slot = loaded
            .records
            .insert(Record::new(
                RecordFields {
                    id: Id::from_index(20),
                    producer: 0,
                    topic: 0,
                    created: 2,
                    until: 3,
                    seq: 20,
                    own_pos: 20,
                },
                new,
            ))
            .ok()
            .unwrap();
        assert_eq!(slot, 2);
        let serialized = bytes(&loaded);
        let restored =
            MessageImage::read_from(serialized.as_slice(), ImageLimits::default()).unwrap();
        assert_eq!(restored.message_count(), 4);
        assert!(restored.message(&Id::from_index(20).to_string()).is_some());
    }
    #[test]
    fn pointer_free_record_has_a_fixed_endian_layout_and_all_union_padding_is_zero() {
        let image = fixture();
        let record = image.records.get(0).unwrap();
        assert_eq!(record.bytes.len(), 80);
        assert_eq!(size_of::<Record>(), 80);
        assert_eq!(&record.bytes[..8], b"00000000");
        assert_eq!(&record.bytes[8..16], &1_900_000_000_000u64.to_le_bytes());
        assert_eq!(&record.bytes[32..40], &1_800_000_000_000u64.to_le_bytes());
        assert_eq!(record.bytes[54], 1);
        assert!(
            record.bytes[48..54]
                .iter()
                .chain(&record.bytes[55..80])
                .all(|byte| *byte == 0)
        );
    }
}

#[cfg(test)]
mod functional {
    use super::*;
    use crate::test_support::{self, Rng};
    // Recompute framing checksums to exercise semantic validation behind the hash gate.
    fn repair(bytes: &mut [u8]) {
        if bytes.len() < HEADER {
            return;
        }
        let hash = crypto::sha256(&bytes[..64]);
        bytes[64..96].copy_from_slice(&hash);
        let mut at = HEADER;
        while bytes.len() - at >= 48 {
            let length = u32_at(bytes, at + 8) as usize;
            if length > bytes.len() - at - 48 {
                break;
            }
            let end = at + 48 + length;
            let hash = crypto::sha256(&bytes[at + 48..end]);
            bytes[at + 16..at + 48].copy_from_slice(&hash);
            at = end;
        }
    }
    #[test]
    fn functional_seeded_image_mutations_validate_or_reject_without_panics() {
        let fixture = tests::fixture();
        let original = tests::bytes(&fixture);
        let record = fixture.records.get(1).unwrap();
        let location = u64_at(record.payload().data, 8);
        let page = ((location >> 32) - 1) as usize;
        let log = fixture.blobs.pages[page].as_ref().unwrap().bytes[0];
        let page_header = 8 + (16384usize >> log).div_ceil(64) * 8;
        let mut at = HEADER;
        for _ in 0..5 + page {
            at += 48 + u32_at(&original, at + 8) as usize;
        }
        let payload_byte = at + 48 + page_header + location as u32 as usize;

        let mut rng = Rng(test_support::seed());
        let cases = test_support::fuzz_cases();
        let limits = ImageLimits {
            max_bytes: (original.len() + 64) as u64,
            max_records: 64,
            max_record_chunks: 8,
            max_blob_pages: 8,
        };
        let mut accepted = 0;
        let mut accepted_modified = 0;
        for case in 0..cases {
            let mut input = if case % 16 == 0 {
                original.clone()
            } else {
                rng.mutate(&original, original.len() + 64)
            };
            if case % 16 == 1 {
                input = original.clone();
                input[payload_byte] ^= 1 << rng.pick(8);
            }
            if case % 2 == 0 || case % 16 == 1 {
                repair(&mut input);
            }
            let outcome =
                std::panic::catch_unwind(|| MessageImage::read_from(input.as_slice(), limits));
            let decoded = outcome.unwrap_or_else(|_| {
                panic!(
                    "Image parser panic: seed=0x{:016x} case={case}",
                    test_support::seed()
                )
            });
            if let Ok(image) = decoded {
                accepted += 1;
                accepted_modified += usize::from(input != original);
                for record in image.records.iter() {
                    let message = image.message(&record.id().to_string()).unwrap();
                    message.payload.validate().unwrap();
                }
                let mut encoded = Vec::new();
                image.write_to(&mut encoded).unwrap();
                assert_eq!(
                    encoded, input,
                    "accepted image changed on roundtrip at case {case}"
                );
            }
        }
        assert!(accepted > 0);
        if cases > 1 {
            assert!(accepted_modified > 0);
        }
        eprintln!(
            "Image mutations: {cases} cases, {accepted} accepted and roundtripped ({accepted_modified} modified valid images); checksums repaired on alternate cases"
        );
    }
    #[test]
    fn functional_binary_images_read_correctly_with_short_reads() {
        struct Short<'a>(&'a [u8], usize);
        impl Read for Short<'_> {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                let count = output.len().min(self.1);
                self.0.read(&mut output[..count])
            }
        }
        let original = tests::bytes(&tests::fixture());
        for length in [1, 2, 3, 7, 31] {
            let image =
                MessageImage::read_from(Short(&original, length), ImageLimits::default()).unwrap();
            let mut encoded = Vec::new();
            image.write_to(&mut encoded).unwrap();
            assert_eq!(encoded, original);
        }
    }
}
