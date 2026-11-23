//! Fixed-size-class pages. Only relative offsets occur in stored records.
use super::*;
const PAGE: usize = 16 * 1024;
const MIN_LOG: u8 = 4;
const MAX_LOG: u8 = 12;
const CLASSES: usize = (MAX_LOG - MIN_LOG + 1) as usize;
#[derive(Clone)]
pub(super) struct Page {
    pub bytes: Box<[u8]>,
    first_word: usize,
}
impl Page {
    fn new(log: u8) -> Self {
        let slots = PAGE >> log;
        let mut bytes = vec![0; 8 + slots.div_ceil(64) * 8 + PAGE].into_boxed_slice();
        bytes[0] = log;
        Self {
            bytes,
            first_word: 0,
        }
    }
    fn log(&self) -> u8 {
        self.bytes[0]
    }
    fn slots(&self) -> usize {
        PAGE >> self.log()
    }
    fn words(&self) -> usize {
        self.slots().div_ceil(64)
    }
    fn header(&self) -> usize {
        8 + self.words() * 8
    }
    fn live(&self) -> usize {
        u32_at(&self.bytes, 4) as usize
    }
    fn mask(&self, word: usize) -> u64 {
        let left = self.slots().saturating_sub(word * 64);
        if left >= 64 {
            u64::MAX
        } else {
            (1u64 << left) - 1
        }
    }
    fn first_free(&mut self) -> usize {
        while self.first_word < self.words() {
            let free = !u64_at(&self.bytes, 8 + self.first_word * 8) & self.mask(self.first_word);
            if free != 0 {
                return self.first_word * 64 + free.trailing_zeros() as usize;
            }
            self.first_word += 1;
        }
        unreachable!("available blob page must have a free slot")
    }
}
#[derive(Clone, Default)]
pub(crate) struct Blobs {
    pub(super) pages: Vec<Option<Page>>,
    available: [Vec<u32>; CLASSES],
    vacant: Vec<u32>,
    empty: usize,
    cursor: usize,
}
impl Blobs {
    pub(crate) fn insert(&mut self, data: &[u8]) -> Result<u64> {
        if data.is_empty() {
            return Ok(0);
        }
        if data.len() > 4096 {
            return Err(ApiError::invalid());
        }
        let log = data
            .len()
            .max(1 << MIN_LOG)
            .next_power_of_two()
            .trailing_zeros() as u8;
        let class = (log - MIN_LOG) as usize;
        if self.available[class].is_empty() {
            let id = if let Some(id) = self.vacant.pop() {
                id
            } else {
                let id = u32::try_from(self.pages.len()).map_err(|_| ApiError::exhausted())?;
                if id == u32::MAX {
                    return Err(ApiError::exhausted());
                }
                self.pages.push(None);
                id
            };
            self.pages[id as usize] = Some(Page::new(log));
            self.available[class].push(id);
            self.empty += 1;
        }
        let id = self.available[class][self.available[class].len() - 1];
        let page = self.pages[id as usize]
            .as_mut()
            .expect("available page exists");
        let slot = page.first_free();
        let word = 8 + (slot / 64) * 8;
        let bits = u64_at(&page.bytes, word) | (1u64 << (slot % 64));
        put(&mut page.bytes, word, &bits.to_le_bytes());
        let live = page.live();
        if live == 0 {
            self.empty -= 1;
        }
        put(&mut page.bytes, 4, &((live + 1) as u32).to_le_bytes());
        let offset = slot << log;
        let begin = page.header() + offset;
        page.bytes[begin..begin + data.len()].copy_from_slice(data);
        if live + 1 == page.slots() {
            self.available[class].pop();
        }
        Ok((u64::from(id) + 1) << 32 | offset as u64)
    }
    pub(crate) fn data(&self, location: u64, len: usize) -> &[u8] {
        if len == 0 {
            return &[];
        }
        let id = ((location >> 32) - 1) as usize;
        let page = self.pages[id].as_ref().expect("live blob page exists");
        let begin = page.header() + (location as u32) as usize;
        &page.bytes[begin..begin + len]
    }
    pub(crate) fn release(&mut self, location: u64, len: usize) {
        if len == 0 {
            return;
        }
        let id = ((location >> 32) - 1) as usize;
        let page = self.pages[id].as_mut().expect("live blob page exists");
        let offset = (location as u32) as usize;
        let slot = offset >> page.log();
        let word = 8 + (slot / 64) * 8;
        let live = page.live();
        debug_assert!(u64_at(&page.bytes, word) & (1u64 << (slot % 64)) != 0);
        let bits = u64_at(&page.bytes, word) & !(1u64 << (slot % 64));
        put(&mut page.bytes, word, &bits.to_le_bytes());
        put(&mut page.bytes, 4, &((live - 1) as u32).to_le_bytes());
        let begin = page.header() + offset;
        let end = begin + (1 << page.log());
        page.bytes[begin..end].fill(0);
        page.first_word = page.first_word.min(slot / 64);
        if live == page.slots() {
            self.available[(page.log() - MIN_LOG) as usize].push(id as u32);
        }
        if live == 1 {
            self.empty += 1;
        }
    }
    pub(crate) fn allocated_bytes(&self) -> usize {
        self.pages
            .iter()
            .flatten()
            .map(|page| page.bytes.len())
            .sum::<usize>()
            + self.pages.capacity() * size_of::<Option<Page>>()
            + self
                .available
                .iter()
                .map(|list| list.capacity() * 4)
                .sum::<usize>()
            + self.vacant.capacity() * 4
    }
    pub(crate) fn page_count(&self) -> usize {
        self.pages.iter().flatten().count()
    }
    pub(crate) fn needs_reclaim(&self) -> bool {
        self.empty != 0
    }
    pub(crate) fn reclaim(&mut self, budget: usize) {
        let mut freed = 0;
        for _ in 0..budget.saturating_mul(16).min(self.pages.len()) {
            self.cursor %= self.pages.len();
            let id = self.cursor;
            self.cursor += 1;
            if self.pages[id].as_ref().is_some_and(|page| page.live() == 0) {
                self.pages[id] = None;
                self.empty -= 1;
                self.vacant.push(id as u32);
                freed += 1;
                if freed == budget {
                    break;
                }
            }
        }
        if freed > 0 {
            for list in &mut self.available {
                list.retain(|id| self.pages[*id as usize].is_some());
            }
            while self.pages.last().is_some_and(Option::is_none) {
                self.pages.pop();
            }
            self.vacant.retain(|id| (*id as usize) < self.pages.len());
            if self.pages.len().saturating_mul(4) < self.pages.capacity() {
                self.pages.shrink_to_fit();
            }
            self.vacant.shrink_to_fit();
            for list in &mut self.available {
                list.shrink_to_fit();
            }
        }
    }
    pub(super) fn rebuild(pages: Vec<Option<Box<[u8]>>>) -> Result<Self> {
        let mut pool = Self::default();
        for (id, bytes) in pages.into_iter().enumerate() {
            let Some(bytes) = bytes else {
                pool.vacant.push(id as u32);
                pool.pages.push(None);
                continue;
            };
            if bytes.len() < 8
                || !(MIN_LOG..=MAX_LOG).contains(&bytes[0])
                || bytes[1..4].iter().any(|byte| *byte != 0)
            {
                return Err(ApiError::invalid());
            }
            let page = Page {
                bytes,
                first_word: 0,
            };
            if page.bytes.len() != page.header() + PAGE || page.live() > page.slots() {
                return Err(ApiError::invalid());
            }
            let mut live = 0;
            for word in 0..page.words() {
                let bits = u64_at(&page.bytes, 8 + word * 8);
                if bits & !page.mask(word) != 0 {
                    return Err(ApiError::invalid());
                }
                live += bits.count_ones() as usize;
            }
            if live != page.live() {
                return Err(ApiError::invalid());
            }
            for slot in 0..page.slots() {
                if u64_at(&page.bytes, 8 + (slot / 64) * 8) & (1u64 << (slot % 64)) == 0 {
                    let begin = page.header() + (slot << page.log());
                    if page.bytes[begin..begin + (1 << page.log())]
                        .iter()
                        .any(|byte| *byte != 0)
                    {
                        return Err(ApiError::invalid());
                    }
                }
            }
            if live < page.slots() {
                pool.available[(page.log() - MIN_LOG) as usize].push(id as u32);
            }
            if live == 0 {
                pool.empty += 1;
            }
            pool.pages.push(Some(page));
        }
        Ok(pool)
    }
    pub(super) fn checked_data(&self, location: u64, len: usize) -> Result<&[u8]> {
        if len == 0 {
            return if location == 0 {
                Ok(&[])
            } else {
                Err(ApiError::invalid())
            };
        }
        let id = (location >> 32)
            .checked_sub(1)
            .ok_or_else(ApiError::invalid)? as usize;
        let page = self
            .pages
            .get(id)
            .and_then(Option::as_ref)
            .ok_or_else(ApiError::invalid)?;
        let offset = location as u32 as usize;
        let width = 1usize << page.log();
        let slot = offset >> page.log();
        if len > width
            || !offset.is_multiple_of(width)
            || slot >= page.slots()
            || u64_at(&page.bytes, 8 + (slot / 64) * 8) & (1u64 << (slot % 64)) == 0
        {
            return Err(ApiError::invalid());
        }
        let begin = page.header() + offset;
        if page.bytes[begin + len..begin + width]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(ApiError::invalid());
        }
        Ok(&page.bytes[begin..begin + len])
    }
    pub(super) fn allocations(&self) -> usize {
        self.pages.iter().flatten().map(Page::live).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_size_classes_recycle_live_offsets_and_zero_the_entire_freed_slot() {
        let mut pool = Blobs::default();
        for len in [
            0, 1, 16, 17, 31, 32, 33, 127, 128, 129, 255, 256, 257, 2048, 2049, 4096,
        ] {
            let expected = vec![0xa5; len];
            let address = pool.insert(&expected).unwrap();
            assert_eq!(pool.data(address, len), expected);
            assert_eq!(pool.checked_data(address, len).unwrap(), expected);
            pool.release(address, len);
        }
        for page in pool.pages.iter().flatten() {
            assert!(page.bytes[page.header()..].iter().all(|byte| *byte == 0));
        }
        let rebuilt = Blobs::rebuild(
            pool.pages
                .iter()
                .map(|page| page.as_ref().map(|page| page.bytes.clone()))
                .collect(),
        )
        .unwrap();
        assert_eq!(rebuilt.allocations(), 0);
        while pool.needs_reclaim() {
            pool.reclaim(2);
        }
        assert_eq!(pool.page_count(), 0);
        assert!(pool.insert(&[0; 4097]).is_err());
    }
    #[test]
    fn full_pages_and_reused_holes_preserve_other_payloads() {
        let mut pool = Blobs::default();
        let addresses: Vec<_> = (0..1100)
            .map(|index| pool.insert(&(index as u32).to_le_bytes()).unwrap())
            .collect();
        assert_eq!(pool.page_count(), 2);
        for index in (0..1100).step_by(2) {
            pool.release(addresses[index], 4);
        }
        for index in 0..550 {
            pool.insert(&(index as u32 + 5000).to_le_bytes()).unwrap();
        }
        assert_eq!(pool.page_count(), 2);
        for index in (1..1100).step_by(2) {
            assert_eq!(
                pool.data(addresses[index], 4),
                &(index as u32).to_le_bytes()
            );
        }
    }
}
