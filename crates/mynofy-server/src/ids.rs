//! Owner-local RID ranges. No per-ID heap allocation or random generation.
const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
pub(crate) const SPACE: u64 = 218_340_105_584_896; // 62^8
const BLOCK: u64 = 1024;

#[repr(align(8))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Id(pub(crate) [u8; 8]);
impl Id {
    pub(crate) fn from_index(mut index: u64) -> Self {
        let mut chars = [b'0'; 8];
        for c in chars.iter_mut().rev() {
            *c = ALPHABET[(index % 62) as usize];
            index /= 62;
        }
        Self(chars)
    }
    pub(crate) fn parse(text: &str) -> Option<Self> {
        if text.len() != 8 || !text.bytes().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        Some(Self(text.as_bytes().try_into().ok()?))
    }
    fn increment(&mut self) {
        for c in self.0.iter_mut().rev() {
            *c = match *c {
                b'0'..=b'8' | b'A'..=b'Y' | b'a'..=b'y' => *c + 1,
                b'9' => b'A',
                b'Z' => b'a',
                _ => b'0',
            };
            if *c != b'0' {
                break;
            }
        }
    }
}
struct Range {
    next: Id,
    remaining: u64,
}
impl Range {
    fn reserve(position: &mut u64) -> Self {
        let count = BLOCK.min(SPACE - *position);
        let range = Self {
            next: Id::from_index(*position),
            remaining: count,
        };
        *position = (*position + count) % SPACE;
        range
    }
}

pub(crate) struct IdPool {
    reserved: u64,
    current: Range,
}
impl IdPool {
    pub(crate) fn new() -> Self {
        Self::at(0)
    }
    pub(crate) fn at(mut position: u64) -> Self {
        let current = Range::reserve(&mut position);
        Self {
            reserved: position,
            current,
        }
    }
    /// Checkpoints resume at the first ID beyond the current reserved range.
    pub(crate) fn reserved_position(&self) -> u64 {
        self.reserved
    }
    pub(crate) fn take(&mut self) -> [u8; 8] {
        if self.current.remaining == 0 {
            self.current = Range::reserve(&mut self.reserved);
        }
        let id = self.current.next.0;
        self.current.next.increment();
        self.current.remaining -= 1;
        id
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(std::str::from_utf8(&self.0).map_err(|_| std::fmt::Error)?)
    }
}
impl serde::Serialize for Id {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layout_and_carries() {
        assert_eq!(std::mem::size_of::<Id>(), 8);
        assert_eq!(std::mem::align_of::<Id>(), 8);
        for index in [8, 9, 35, 61, 62, SPACE - 2, SPACE - 1] {
            let mut id = Id::from_index(index);
            id.increment();
            assert_eq!(id, Id::from_index((index + 1) % SPACE));
        }
    }
    #[test]
    fn synchronous_refill_crosses_block_boundaries_and_wraps() {
        let start = SPACE - 3;
        let mut pool = IdPool::at(start);
        assert_eq!(pool.reserved_position(), 0);
        for i in 0..BLOCK * 8 {
            assert_eq!(pool.take(), Id::from_index((start + i) % SPACE).0);
        }
    }

    #[test]
    fn restore_resumes_beyond_the_reserved_range() {
        let mut pool = IdPool::new();
        for i in 0..BLOCK + 1 {
            assert_eq!(pool.take(), Id::from_index(i).0);
        }
        assert_eq!(pool.reserved_position(), BLOCK * 2);
        let mut restored = IdPool::at(pool.reserved_position());
        assert_eq!(restored.take(), Id::from_index(BLOCK * 2).0);
        assert_eq!(restored.reserved_position(), BLOCK * 3);
    }
}
