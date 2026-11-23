//! Records and their live bitmap are one initialized byte block per chunk.
use super::*;
pub(super) const CHUNK: usize = 256;
pub(super) const HEADER: usize = 40;
pub(super) const BYTES: usize = HEADER + CHUNK * RECORD_BYTES;
#[derive(Clone)]
pub(super) struct Chunk {
    pub bytes: Box<[u8]>,
}
impl Chunk {
    fn live(&self) -> usize {
        u32_at(&self.bytes, 0) as usize
    }
    fn occupied(&self, offset: usize) -> bool {
        u64_at(&self.bytes, 8 + (offset / 64) * 8) & (1u64 << (offset % 64)) != 0
    }
    fn record(&self, offset: usize) -> Record<&[u8; RECORD_BYTES]> {
        let start = HEADER + offset * RECORD_BYTES;
        Record {
            bytes: self.bytes[start..start + RECORD_BYTES]
                .try_into()
                .expect("fixed-width record slice"),
        }
    }
}
#[derive(Clone, Default)]
pub(crate) struct Arena {
    pub(super) chunks: Vec<Option<Chunk>>,
    available: Vec<u32>,
    vacant: Vec<u32>,
    len: usize,
    cursor: usize,
    empty: usize,
}
impl Arena {
    pub(crate) fn new() -> Self {
        Self::default()
    }
    pub(crate) fn insert(&mut self, value: Record) -> std::result::Result<u32, Record> {
        if self.available.is_empty() {
            let id = if let Some(id) = self.vacant.pop() {
                id
            } else {
                let Ok(id) = u32::try_from(self.chunks.len()) else {
                    return Err(value);
                };
                if id
                    .checked_mul(CHUNK as u32)
                    .and_then(|id| id.checked_add(CHUNK as u32 - 1))
                    .is_none()
                {
                    return Err(value);
                }
                self.chunks.push(None);
                id
            };
            self.chunks[id as usize] = Some(Chunk {
                bytes: vec![0; BYTES].into_boxed_slice(),
            });
            self.available.push(id);
            self.empty += 1;
        }
        let id = self.available[self.available.len() - 1];
        let chunk = self.chunks[id as usize]
            .as_mut()
            .expect("available record chunk exists");
        let word = (0..4)
            .find(|word| u64_at(&chunk.bytes, 8 + word * 8) != u64::MAX)
            .expect("available chunk has a free slot");
        let bits = u64_at(&chunk.bytes, 8 + word * 8);
        let offset = word * 64 + (!bits).trailing_zeros() as usize;
        put(
            &mut chunk.bytes,
            8 + word * 8,
            &(bits | (1u64 << (offset % 64))).to_le_bytes(),
        );
        let live = chunk.live();
        if live == 0 {
            self.empty -= 1;
        }
        put(&mut chunk.bytes, 0, &((live + 1) as u32).to_le_bytes());
        let start = HEADER + offset * RECORD_BYTES;
        chunk.bytes[start..start + RECORD_BYTES].copy_from_slice(&value.bytes);
        self.len += 1;
        if live + 1 == CHUNK {
            self.available.pop();
        }
        Ok(id * CHUNK as u32 + offset as u32)
    }
    pub(crate) fn get(&self, slot: u32) -> Option<Record<&[u8; RECORD_BYTES]>> {
        let chunk = self.chunks.get(slot as usize / CHUNK)?.as_ref()?;
        let offset = slot as usize % CHUNK;
        chunk.occupied(offset).then(|| chunk.record(offset))
    }
    pub(crate) fn remove(&mut self, slot: u32) -> Option<Record> {
        let id = slot as usize / CHUNK;
        let offset = slot as usize % CHUNK;
        let chunk = self.chunks.get_mut(id)?.as_mut()?;
        if !chunk.occupied(offset) {
            return None;
        }
        let mut value = Record {
            bytes: [0; RECORD_BYTES],
        };
        let start = HEADER + offset * RECORD_BYTES;
        value
            .bytes
            .copy_from_slice(&chunk.bytes[start..start + RECORD_BYTES]);
        chunk.bytes[start..start + RECORD_BYTES].fill(0);
        let word = 8 + (offset / 64) * 8;
        let bits = u64_at(&chunk.bytes, word) & !(1u64 << (offset % 64));
        put(&mut chunk.bytes, word, &bits.to_le_bytes());
        let live = chunk.live();
        put(&mut chunk.bytes, 0, &((live - 1) as u32).to_le_bytes());
        if live == CHUNK {
            self.available.push(id as u32);
        }
        if live == 1 {
            self.empty += 1;
        }
        self.len -= 1;
        Some(value)
    }
    // Used only by an atomic administrative removal of a completely empty topic.
    pub(crate) fn move_topic(&mut self, old: u16, new: u16) {
        for chunk in self.chunks.iter_mut().flatten() {
            for offset in 0..CHUNK {
                if chunk.occupied(offset) {
                    let start = HEADER + offset * RECORD_BYTES + 42;
                    if u16_at(&chunk.bytes, start) == old {
                        put(&mut chunk.bytes, start, &new.to_le_bytes());
                    }
                }
            }
        }
    }
    pub(crate) fn remap_clients(&mut self, mapping: &[Option<u16>]) -> Result<()> {
        for chunk in self.chunks.iter_mut().flatten() {
            for offset in 0..CHUNK {
                if chunk.occupied(offset) {
                    let start = HEADER + offset * RECORD_BYTES + 40;
                    let old = u16_at(&chunk.bytes, start) as usize;
                    let client = mapping
                        .get(old)
                        .copied()
                        .flatten()
                        .ok_or_else(ApiError::invalid)?;
                    put(&mut chunk.bytes, start, &client.to_le_bytes());
                }
            }
        }
        Ok(())
    }
    pub(crate) fn iter(&self) -> impl Iterator<Item = Record<&[u8; RECORD_BYTES]>> {
        self.chunks.iter().flatten().flat_map(|chunk| {
            (0..CHUNK)
                .filter(move |offset| chunk.occupied(*offset))
                .map(move |offset| chunk.record(offset))
        })
    }
    pub(crate) fn iter_slots(&self) -> impl Iterator<Item = (u32, Record<&[u8; RECORD_BYTES]>)> {
        self.chunks
            .iter()
            .enumerate()
            .filter_map(|(id, chunk)| chunk.as_ref().map(|chunk| (id, chunk)))
            .flat_map(|(id, chunk)| {
                (0..CHUNK)
                    .filter(move |offset| chunk.occupied(*offset))
                    .map(move |offset| ((id * CHUNK + offset) as u32, chunk.record(offset)))
            })
    }
    pub(crate) fn len(&self) -> usize {
        self.len
    }
    pub(crate) fn capacity(&self) -> usize {
        self.chunks.iter().flatten().count() * CHUNK
    }
    pub(crate) fn allocated_bytes(&self) -> usize {
        self.capacity() / CHUNK * BYTES
            + self.chunks.capacity() * size_of::<Option<Chunk>>()
            + self.available.capacity() * 4
            + self.vacant.capacity() * 4
    }
    pub(crate) fn needs_reclaim(&self) -> bool {
        self.empty != 0
    }
    pub(crate) fn reclaim(&mut self, budget: usize) {
        let mut freed = 0;
        for _ in 0..budget.saturating_mul(16).min(self.chunks.len()) {
            self.cursor %= self.chunks.len();
            let id = self.cursor;
            self.cursor += 1;
            if self.chunks[id]
                .as_ref()
                .is_some_and(|chunk| chunk.live() == 0)
            {
                self.chunks[id] = None;
                self.empty -= 1;
                self.vacant.push(id as u32);
                freed += 1;
                if freed == budget {
                    break;
                }
            }
        }
        if freed > 0 {
            self.available
                .retain(|id| self.chunks[*id as usize].is_some());
            while self.chunks.last().is_some_and(Option::is_none) {
                self.chunks.pop();
            }
            self.vacant.retain(|id| (*id as usize) < self.chunks.len());
            if self.chunks.len().saturating_mul(4) < self.chunks.capacity() {
                self.chunks.shrink_to_fit();
            }
            self.available.shrink_to_fit();
            self.vacant.shrink_to_fit();
        }
    }
    pub(super) fn rebuild(chunks: Vec<Option<Box<[u8]>>>) -> Result<Self> {
        let mut arena = Self::new();
        for (id, bytes) in chunks.into_iter().enumerate() {
            let Some(bytes) = bytes else {
                arena.chunks.push(None);
                arena.vacant.push(id as u32);
                continue;
            };
            if bytes.len() != BYTES || bytes[4..8].iter().any(|byte| *byte != 0) {
                return Err(ApiError::invalid());
            }
            let chunk = Chunk { bytes };
            let live = (0..4)
                .map(|word| u64_at(&chunk.bytes, 8 + word * 8).count_ones() as usize)
                .sum::<usize>();
            if live != chunk.live() {
                return Err(ApiError::invalid());
            }
            for offset in 0..CHUNK {
                if !chunk.occupied(offset)
                    && chunk.record(offset).bytes.iter().any(|byte| *byte != 0)
                {
                    return Err(ApiError::invalid());
                }
            }
            if live < CHUNK {
                arena.available.push(id as u32);
            }
            if live == 0 {
                arena.empty += 1;
            }
            arena.len += live;
            arena.chunks.push(Some(chunk));
        }
        Ok(arena)
    }
}
