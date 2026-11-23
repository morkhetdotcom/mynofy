//! One initialized binary representation for RAM records, blobs and message images.
use crate::{
    direct,
    ids::Id,
    protocol::{
        ApiError, Result,
        payload::{Body, Event, Parameter, ParameterValue, Payload},
        pb,
    },
};
use bytes::{Bytes, BytesMut};
#[cfg(test)]
use prost::Message as _;
use std::{collections::HashMap, mem::size_of};
pub(crate) const RECORD_BYTES: usize = 80;
fn raw<const N: usize>(bytes: &[u8], at: usize) -> [u8; N] {
    let mut result = [0; N];
    result.copy_from_slice(&bytes[at..at + N]);
    result
}
fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(raw(bytes, at))
}
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(raw(bytes, at))
}
fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(raw(bytes, at))
}
fn put(bytes: &mut [u8], at: usize, value: &[u8]) {
    bytes[at..at + value.len()].copy_from_slice(value);
}
mod arena;
mod block;
pub(crate) use arena::Arena;
mod blobs;
pub(crate) use blobs::Blobs;
#[derive(Clone)]
pub(crate) struct InlineEvent<B = [u8; 32]> {
    bytes: B,
}
impl InlineEvent {
    fn from_event(event: Event) -> Result<InlineEvent> {
        if event.catalog != 1 || event.catalog_version != 1 {
            return Err(ApiError::invalid());
        }
        let category = u16::try_from(event.category).map_err(|_| ApiError::invalid())?;
        let action = u8::try_from(event.action).map_err(|_| ApiError::invalid())?;
        let status = u8::try_from(event.status).map_err(|_| ApiError::invalid())?;
        let mut out = InlineEvent { bytes: [0; 32] };
        put(
            &mut out.bytes,
            0,
            &(u32::from(category) | (u32::from(action) << 16) | (u32::from(status) << 24))
                .to_le_bytes(),
        );
        out.bytes[6] = 2;
        let mut offset = 0;
        for (i, p) in event.parameters.into_iter().enumerate() {
            let (kind, raw, len) = match p.value.ok_or_else(ApiError::invalid)? {
                ParameterValue::U32(v) => (1, u64::from(v).to_le_bytes(), 4),
                ParameterValue::U64(v) => (2, v.to_le_bytes(), 8),
                ParameterValue::I32(v) => (3, (i64::from(v) as u64).to_le_bytes(), 4),
                ParameterValue::I64(v) => (4, v.to_le_bytes(), 8),
                ParameterValue::F32(v) => (5, u64::from(v.to_bits()).to_le_bytes(), 4),
                ParameterValue::F64(v) => (6, v.to_bits().to_le_bytes(), 8),
                ParameterValue::Bool(v) => (7, u64::from(v).to_le_bytes(), 1),
            };
            if i >= 3 || offset + len > 24 {
                return Err(ApiError::invalid());
            }
            let layout = u16_at(&out.bytes, 4) | (kind << (i * 3));
            put(&mut out.bytes, 4, &layout.to_le_bytes());
            out.bytes[8 + offset..8 + offset + len].copy_from_slice(&raw[..len]);
            offset += len;
        }
        Ok(out)
    }
}
impl<B: AsRef<[u8]>> InlineEvent<B> {
    fn parameters(&self) -> impl Iterator<Item = Parameter> + '_ {
        let count = (0..3)
            .take_while(|i| ((u16_at(self.bytes.as_ref(), 4) >> (i * 3)) & 7) != 0)
            .count();
        let mut offset = 0;
        (0..count).map(move |i| {
            let kind = (u16_at(self.bytes.as_ref(), 4) >> (i * 3)) & 7;
            let len = match kind {
                1 | 3 | 5 => 4,
                7 => 1,
                _ => 8,
            };
            let mut raw = [0u8; 8];
            raw[..len].copy_from_slice(&self.bytes.as_ref()[8 + offset..8 + offset + len]);
            offset += len;
            let value = match kind {
                1 => ParameterValue::U32(u64::from_le_bytes(raw) as u32),
                2 => ParameterValue::U64(u64::from_le_bytes(raw)),
                3 => ParameterValue::I32(u64::from_le_bytes(raw) as i32),
                4 => ParameterValue::I64(i64::from_le_bytes(raw)),
                5 => ParameterValue::F32(f32::from_bits(u64::from_le_bytes(raw) as u32)),
                6 => ParameterValue::F64(f64::from_bits(u64::from_le_bytes(raw))),
                _ => ParameterValue::Bool(raw[0] != 0),
            };
            Parameter { value: Some(value) }
        })
    }
    fn event(&self) -> Event {
        Event {
            catalog: 1,
            catalog_version: 1,
            category: u32_at(self.bytes.as_ref(), 0) & 65535,
            action: (u32_at(self.bytes.as_ref(), 0) >> 16) & 255,
            status: u32_at(self.bytes.as_ref(), 0) >> 24,
            parameters: self.parameters().collect(),
        }
    }
    fn encoded_len(&self) -> usize {
        use prost::encoding::{message, uint32};
        uint32::encoded_len(1, &1)
            + uint32::encoded_len(2, &1)
            + uint32::encoded_len(3, &(u32_at(self.bytes.as_ref(), 0) & 65535))
            + uint32::encoded_len(4, &((u32_at(self.bytes.as_ref(), 0) >> 16) & 255))
            + uint32::encoded_len(5, &(u32_at(self.bytes.as_ref(), 0) >> 24))
            + self
                .parameters()
                .map(|p| message::encoded_len(6, &p))
                .sum::<usize>()
    }
    fn encode(&self, out: &mut BytesMut) {
        for (tag, value) in [
            (1, 1),
            (2, 1),
            (3, u32_at(self.bytes.as_ref(), 0) & 65535),
            (4, (u32_at(self.bytes.as_ref(), 0) >> 16) & 255),
            (5, u32_at(self.bytes.as_ref(), 0) >> 24),
        ] {
            direct::number(tag, u64::from(value), out);
        }
        for p in self.parameters() {
            prost::encoding::message::encode(6, &p, out);
        }
    }
}
#[derive(Clone)]
pub(crate) struct StoredPayload<B = [u8; 32]> {
    data: B,
    content_type: u32,
}
impl StoredPayload {
    pub(crate) fn new(
        payload: Payload,
        types: &mut ContentTypes,
        blobs: &mut Blobs,
    ) -> Result<Self> {
        let mut data = [0; 32];
        match payload.body.ok_or_else(ApiError::invalid)? {
            Body::Data(bytes) => {
                let location = blobs.insert(&bytes)?;
                put(&mut data, 0, &(bytes.len() as u32).to_le_bytes());
                data[6] = 1;
                put(&mut data, 8, &location.to_le_bytes());
            }
            Body::Event(event) => data = InlineEvent::from_event(event)?.bytes,
        }
        let content_type = match types.acquire(payload.content_type, blobs) {
            Ok(id) => id,
            Err(error) => {
                if data[6] == 1 {
                    blobs.release(u64_at(&data, 8), u32_at(&data, 0) as usize);
                }
                return Err(error);
            }
        };
        Ok(Self { data, content_type })
    }
}
impl<B: AsRef<[u8]>> StoredPayload<B> {
    fn event(&self) -> InlineEvent<&[u8; 32]> {
        InlineEvent {
            bytes: self
                .data
                .as_ref()
                .try_into()
                .expect("fixed-width payload union"),
        }
    }
    pub(crate) fn payload(&self, types: &ContentTypes, blobs: &Blobs) -> Payload {
        let body = if self.data.as_ref()[6] == 1 {
            Body::Data(Bytes::copy_from_slice(
                blobs.data(u64_at(self.data.as_ref(), 8), self.external_bytes()),
            ))
        } else {
            Body::Event(self.event().event())
        };
        Payload {
            body: Some(body),
            content_type: types.name(self.content_type, blobs).map(str::to_owned),
        }
    }
    fn encoded_len(&self, types: &ContentTypes, blobs: &Blobs) -> usize {
        types
            .name(self.content_type, blobs)
            .map_or(0, |name| direct::field_len(1, name.len()))
            + if self.data.as_ref()[6] == 1 {
                direct::field_len(2, self.external_bytes())
            } else {
                direct::field_len(3, self.event().encoded_len())
            }
    }
    fn encode(&self, types: &ContentTypes, blobs: &Blobs, out: &mut BytesMut) {
        if let Some(name) = types.name(self.content_type, blobs) {
            direct::data(1, name.as_bytes(), out);
        }
        if self.data.as_ref()[6] == 1 {
            direct::data(
                2,
                blobs.data(u64_at(self.data.as_ref(), 8), self.external_bytes()),
                out,
            );
        } else {
            let event = self.event();
            direct::header(3, event.encoded_len(), out);
            event.encode(out);
        }
    }
    pub(crate) fn release(&self, types: &mut ContentTypes, blobs: &mut Blobs) {
        if self.data.as_ref()[6] == 1 {
            blobs.release(u64_at(self.data.as_ref(), 8), self.external_bytes());
        }
        types.release(self.content_type, blobs);
    }
    pub(crate) fn external_bytes(&self) -> usize {
        if self.data.as_ref()[6] == 1 {
            u32_at(self.data.as_ref(), 0) as usize
        } else {
            0
        }
    }
}
#[repr(align(8))]
#[derive(Clone)]
pub(crate) struct Record<B = [u8; RECORD_BYTES]> {
    bytes: B,
}
pub(crate) struct RecordFields {
    pub id: Id,
    pub producer: u16,
    pub topic: u16,
    pub created: u64,
    pub until: u64,
    pub own_pos: u64,
    pub seq: u64,
}
impl Record {
    pub(crate) fn new(fields: RecordFields, payload: StoredPayload) -> Self {
        let mut bytes = [0; RECORD_BYTES];
        put(&mut bytes, 0, &fields.id.0);
        for (offset, value) in [
            (8, fields.until),
            (16, fields.own_pos),
            (24, fields.seq),
            (32, fields.created),
        ] {
            put(&mut bytes, offset, &value.to_le_bytes());
        }
        put(&mut bytes, 40, &fields.producer.to_le_bytes());
        put(&mut bytes, 42, &fields.topic.to_le_bytes());
        put(&mut bytes, 44, &payload.content_type.to_le_bytes());
        put(&mut bytes, 48, &payload.data);
        Self { bytes }
    }
}
impl<B: AsRef<[u8]>> Record<B> {
    pub(crate) fn relocate(
        &self,
        producer: u16,
        topic: u16,
        source_types: &ContentTypes,
        source_blobs: &Blobs,
        types: &mut ContentTypes,
        blobs: &mut Blobs,
    ) -> Result<Record> {
        let old = self.payload();
        let mut data = *old.data;
        if data[6] == 1 {
            let location =
                blobs.insert(source_blobs.data(u64_at(&data, 8), old.external_bytes()))?;
            put(&mut data, 8, &location.to_le_bytes());
        }
        let content_type = types.acquire(
            source_types
                .name(old.content_type, source_blobs)
                .map(str::to_owned),
            blobs,
        )?;
        let mut bytes = [0; RECORD_BYTES];
        bytes.copy_from_slice(self.bytes.as_ref());
        put(&mut bytes, 40, &producer.to_le_bytes());
        put(&mut bytes, 42, &topic.to_le_bytes());
        put(&mut bytes, 44, &content_type.to_le_bytes());
        put(&mut bytes, 48, &data);
        Ok(Record { bytes })
    }
    #[inline]
    pub(crate) fn id(&self) -> Id {
        Id(raw(self.bytes.as_ref(), 0))
    }
    #[inline]
    pub(crate) fn until(&self) -> u64 {
        u64_at(self.bytes.as_ref(), 8)
    }
    #[inline]
    pub(crate) fn own_pos(&self) -> u64 {
        u64_at(self.bytes.as_ref(), 16)
    }
    #[inline]
    pub(crate) fn seq(&self) -> u64 {
        u64_at(self.bytes.as_ref(), 24)
    }
    #[inline]
    pub(crate) fn created_time(&self) -> u64 {
        u64_at(self.bytes.as_ref(), 32)
    }
    #[inline]
    pub(crate) fn producer_slot(&self) -> usize {
        u16_at(self.bytes.as_ref(), 40) as usize
    }
    #[inline]
    pub(crate) fn topic(&self) -> u16 {
        u16_at(self.bytes.as_ref(), 42)
    }
    #[inline]
    pub(crate) fn payload(&self) -> StoredPayload<&[u8; 32]> {
        StoredPayload {
            data: self.bytes.as_ref()[48..80]
                .try_into()
                .expect("fixed-width payload union"),
            content_type: u32_at(self.bytes.as_ref(), 44),
        }
    }
    fn wire_len(
        &self,
        names: (&str, &str),
        types: &ContentTypes,
        blobs: &Blobs,
        expiry: &str,
    ) -> usize {
        let (topic, producer_id) = names;
        direct::field_len(1, 8)
            + direct::field_len(2, topic.len())
            + direct::field_len(3, self.payload().encoded_len(types, blobs))
            + direct::field_len(4, direct::field_len(1, expiry.len()))
            + direct::field_len(5, producer_id.len())
    }
    pub(crate) fn polled_len(
        &self,
        names: (&str, &str),
        types: &ContentTypes,
        blobs: &Blobs,
        expiry: &str,
    ) -> usize {
        direct::field_len(2, self.wire_len(names, types, blobs, expiry))
    }
    pub(crate) fn encode_polled(
        &self,
        names: (&str, &str),
        types: &ContentTypes,
        blobs: &Blobs,
        expiry: &str,
        out: &mut BytesMut,
    ) {
        let (topic, producer_id) = names;
        direct::header(2, self.wire_len(names, types, blobs, expiry), out);
        direct::data(1, &self.id().0, out);
        direct::data(2, topic.as_bytes(), out);
        let payload = self.payload();
        direct::header(3, payload.encoded_len(types, blobs), out);
        payload.encode(types, blobs, out);
        direct::header(4, direct::field_len(1, expiry.len()), out);
        direct::data(1, expiry.as_bytes(), out);
        direct::data(5, producer_id.as_bytes(), out);
    }
    pub(crate) fn charge(&self, types: &ContentTypes, blobs: &Blobs) -> usize {
        RECORD_BYTES
            + 128
            + 64
            + self.payload().external_bytes()
            + types
                .name(self.payload().content_type, blobs)
                .map_or(0, str::len)
    }
    pub(crate) fn wire(
        &self,
        names: (&str, &str),
        types: &ContentTypes,
        blobs: &Blobs,
        expires: String,
    ) -> pb::StoredMessage {
        let (topic, producer_id) = names;
        pb::StoredMessage {
            producer_id: producer_id.into(),
            id: self.id().to_string(),
            topic: topic.into(),
            payload: self.payload().payload(types, blobs),
            expires: pb::Expires { time: expires },
        }
    }
}
#[derive(Clone, Default)]
pub(crate) struct ContentTypes {
    names: HashMap<String, u32>,
    pub(super) entries: Vec<u8>,
    free: Vec<u32>,
}
impl ContentTypes {
    fn acquire(&mut self, name: Option<String>, blobs: &mut Blobs) -> Result<u32> {
        let Some(name) = name else {
            return Ok(0);
        };
        if let Some(&id) = self.names.get(&name) {
            let offset = (id as usize - 1) * 16;
            let count = u32_at(&self.entries, offset + 12)
                .checked_add(1)
                .ok_or_else(ApiError::exhausted)?;
            put(&mut self.entries, offset + 12, &count.to_le_bytes());
            return Ok(id);
        }
        let index = if let Some(index) = self.free.pop() {
            index
        } else {
            let index =
                u32::try_from(self.entries.len() / 16).map_err(|_| ApiError::exhausted())?;
            index.checked_add(1).ok_or_else(ApiError::exhausted)?;
            self.entries.extend([0; 16]);
            index
        };
        let location = match blobs.insert(name.as_bytes()) {
            Ok(location) => location,
            Err(error) => {
                self.free.push(index);
                return Err(error);
            }
        };
        let offset = index as usize * 16;
        put(&mut self.entries, offset, &location.to_le_bytes());
        put(
            &mut self.entries,
            offset + 8,
            &(name.len() as u32).to_le_bytes(),
        );
        put(&mut self.entries, offset + 12, &1u32.to_le_bytes());
        self.names.insert(name, index + 1);
        Ok(index + 1)
    }
    fn name<'a>(&'a self, id: u32, blobs: &'a Blobs) -> Option<&'a str> {
        if id == 0 {
            return None;
        }
        let offset = (id as usize - 1) * 16;
        if offset + 16 > self.entries.len() || u32_at(&self.entries, offset + 12) == 0 {
            return None;
        }
        std::str::from_utf8(blobs.data(
            u64_at(&self.entries, offset),
            u32_at(&self.entries, offset + 8) as usize,
        ))
        .ok()
    }
    fn release(&mut self, id: u32, blobs: &mut Blobs) {
        if id == 0 {
            return;
        }
        let offset = (id as usize - 1) * 16;
        let count = u32_at(&self.entries, offset + 12);
        debug_assert!(count > 0);
        if count == 1 {
            if let Ok(name) = std::str::from_utf8(blobs.data(
                u64_at(&self.entries, offset),
                u32_at(&self.entries, offset + 8) as usize,
            )) {
                self.names.remove(name);
            }
            let location = u64_at(&self.entries, offset);
            let len = u32_at(&self.entries, offset + 8) as usize;
            blobs.release(location, len);
            self.entries[offset..offset + 16].fill(0);
            self.free.push(id - 1);
        } else {
            put(&mut self.entries, offset + 12, &(count - 1).to_le_bytes());
        }
    }
    pub(crate) fn reclaim(&mut self) {
        if self.names.is_empty() {
            *self = Self::default();
            return;
        }
        while self.entries.len() >= 16 && u32_at(&self.entries, self.entries.len() - 4) == 0 {
            self.entries.truncate(self.entries.len() - 16);
        }
        self.free
            .retain(|index| (*index as usize) < self.entries.len() / 16);
        if self.names.capacity() > self.names.len().saturating_mul(4) {
            self.names.shrink_to(self.names.len() * 2);
        }
        if self.entries.len().saturating_mul(4) < self.entries.capacity() {
            self.entries.shrink_to_fit();
            self.free.shrink_to_fit();
        }
    }
    pub(crate) fn len(&self) -> usize {
        self.names.len()
    }
    pub(crate) fn table_bytes(&self) -> usize {
        self.entries.capacity()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inline_values_roundtrip_all_types_without_heap_storage() {
        assert_eq!(size_of::<InlineEvent>(), 32);
        assert!(size_of::<Record>() <= 104);
        let mut types = ContentTypes::default();
        let mut blobs = Blobs::default();
        for value in [
            ParameterValue::U32(u32::MAX),
            ParameterValue::U64(u64::MAX),
            ParameterValue::I32(i32::MIN),
            ParameterValue::I64(i64::MIN),
            ParameterValue::F32(-0.0),
            ParameterValue::F64(f64::MIN_POSITIVE),
            ParameterValue::Bool(false),
        ] {
            let event = Event {
                catalog: 1,
                catalog_version: 1,
                category: 304,
                action: 121,
                status: 21,
                parameters: vec![Parameter { value: Some(value) }; 3],
            };
            let payload = Payload::event(event);
            payload.validate().unwrap();
            let before = payload.encode_to_vec();
            let stored = StoredPayload::new(payload, &mut types, &mut blobs).unwrap();
            assert_eq!(stored.payload(&types, &blobs).encode_to_vec(), before);
            let mut direct = BytesMut::new();
            stored.encode(&types, &blobs, &mut direct);
            assert_eq!(stored.encoded_len(&types, &blobs), before.len());
            assert_eq!(direct.as_ref(), before);
        }
    }
    #[test]
    fn mixed_parameter_layouts_preserve_values_and_float_bits() {
        let values = [
            ParameterValue::U32(u32::MAX),
            ParameterValue::U64(u64::MAX),
            ParameterValue::I32(-17),
            ParameterValue::I64(i64::MIN),
            ParameterValue::F32(-0.0),
            ParameterValue::F64(-3.25),
            ParameterValue::Bool(true),
        ];
        let mut types = ContentTypes::default();
        let mut blobs = Blobs::default();
        for a in &values {
            for b in &values {
                for c in &values {
                    let event = Event {
                        catalog: 1,
                        catalog_version: 1,
                        category: 12,
                        action: 40,
                        status: 2,
                        parameters: vec![a.clone(), b.clone(), c.clone()]
                            .into_iter()
                            .map(|value| Parameter { value: Some(value) })
                            .collect(),
                    };
                    let payload = Payload::event(event);
                    let expected = payload.encode_to_vec();
                    let stored = StoredPayload::new(payload, &mut types, &mut blobs).unwrap();
                    assert_eq!(stored.payload(&types, &blobs).encode_to_vec(), expected);
                    let mut direct = BytesMut::new();
                    stored.encode(&types, &blobs, &mut direct);
                    assert_eq!(stored.encoded_len(&types, &blobs), expected.len());
                    assert_eq!(direct.as_ref(), expected);
                }
            }
        }
    }
    fn record(number: u64) -> Record {
        Record::new(
            RecordFields {
                id: Id::from_index(number),
                producer: 0,
                topic: 0,
                created: 1,
                until: 2,
                own_pos: number + 1,
                seq: number + 1,
            },
            StoredPayload {
                data: {
                    let mut data = [0; 32];
                    data[6] = 1;
                    data
                },
                content_type: 0,
            },
        )
    }
    #[test]
    fn arena_recycles_slots_without_relocating_live_records() {
        let mut a = Arena::new();
        let slots: Vec<_> = (0..600)
            .map(|i| a.insert(record(i)).ok().unwrap())
            .collect();
        assert_eq!(a.capacity(), 768);
        for &slot in slots.iter().step_by(2) {
            a.remove(slot).unwrap();
        }
        for i in 0..300 {
            a.insert(record(1000 + i)).ok().unwrap();
        }
        assert_eq!(a.len(), 600);
        assert_eq!(a.capacity(), 768);
        assert_eq!(a.get(slots[1]).unwrap().id(), Id::from_index(1));
        let address = a.get(slots[1]).unwrap().bytes.as_ptr();
        for i in 0..300 {
            a.insert(record(9999 + i)).ok().unwrap();
        }
        assert_eq!(a.get(slots[1]).unwrap().bytes.as_ptr(), address);
        assert_eq!(a.capacity(), 1024);
    }
    #[test]
    fn content_type_refs_reclaim_and_reuse_metadata() {
        let mut pool = ContentTypes::default();
        let mut blobs = Blobs::default();
        let one = StoredPayload::new(
            Payload::bytes(vec![1], Some("a/b".into())),
            &mut pool,
            &mut blobs,
        )
        .unwrap();
        let two = StoredPayload::new(
            Payload::bytes(vec![2], Some("a/b".into())),
            &mut pool,
            &mut blobs,
        )
        .unwrap();
        assert_eq!(pool.len(), 1);
        one.release(&mut pool, &mut blobs);
        assert_eq!(pool.len(), 1);
        two.release(&mut pool, &mut blobs);
        assert_eq!(pool.len(), 0);
        for i in 0..1000 {
            let p = StoredPayload::new(
                Payload::bytes(vec![], Some(format!("type/{i}"))),
                &mut pool,
                &mut blobs,
            )
            .unwrap();
            p.release(&mut pool, &mut blobs);
        }
        assert_eq!(pool.entries.len(), 16);
        assert_eq!(pool.len(), 0);
    }
}

#[cfg(test)]
mod direct_tests {
    use super::*;
    #[test]
    fn borrowed_payload_encoding_preserves_empty_bytes_lengths_and_content_types() {
        let mut types = ContentTypes::default();
        let mut blobs = Blobs::default();
        for size in [0, 1, 127, 128, 4096] {
            for content_type in [
                None,
                Some("application/octet-stream".to_owned()),
                Some("text/ümlaut".to_owned()),
            ] {
                let payload = Payload::bytes(vec![0xff; size], content_type);
                let expected = payload.encode_to_vec();
                let stored = StoredPayload::new(payload, &mut types, &mut blobs).unwrap();
                let mut actual = BytesMut::new();
                stored.encode(&types, &blobs, &mut actual);
                assert_eq!(stored.encoded_len(&types, &blobs), expected.len());
                assert_eq!(actual.as_ref(), expected);
                stored.release(&mut types, &mut blobs);
            }
        }
    }
}

pub mod image;
