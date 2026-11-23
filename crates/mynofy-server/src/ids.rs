//! Owner-local RID ranges. No per-ID heap allocation or random generation.
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::mpsc;

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
    reserved: Arc<AtomicU64>,
    current: Range,
    ready: mpsc::Receiver<Range>,
    worker: Option<(mpsc::Sender<Range>, u64)>,
}
impl IdPool {
    pub(crate) fn new() -> Self {
        Self::at(0)
    }
    pub(crate) fn at(mut position: u64) -> Self {
        let current = Range::reserve(&mut position);
        let (sender, ready) = mpsc::channel(1);
        // Channel is empty and open here. Warm both ranges before accepting clients.
        let result = sender.try_send(Range::reserve(&mut position));
        debug_assert!(result.is_ok());
        Self {
            reserved: Arc::new(AtomicU64::new(position)),
            current,
            ready,
            worker: Some((sender, position)),
        }
    }
    pub(crate) fn start(&mut self) {
        if let Some((sender, mut position)) = self.worker.take() {
            let reserved = self.reserved.clone();
            tokio::spawn(async move {
                loop {
                    // Reserve only when the bounded channel has room; no busy polling.
                    let Ok(permit) = sender.reserve().await else {
                        break;
                    };
                    let range = Range::reserve(&mut position);
                    reserved.store(position, Ordering::Release);
                    permit.send(range);
                }
            });
        }
    }
    pub(crate) fn reserved_position(&self) -> u64 {
        self.reserved.load(Ordering::Acquire)
    }
    pub(crate) fn take(&mut self) -> Option<[u8; 8]> {
        if self.current.remaining == 0 {
            self.current = self.ready.try_recv().ok()?;
        }
        let id = self.current.next.0;
        self.current.next.increment();
        self.current.remaining -= 1;
        Some(id)
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
    fn bounded_exhaustion_and_wrap() {
        let mut pool = IdPool::at(SPACE - 1);
        assert_eq!(pool.take(), Some(*b"zzzzzzzz"));
        assert_eq!(pool.take(), Some(*b"00000000"));
        for _ in 1..BLOCK {
            assert!(pool.take().is_some());
        }
        assert!(pool.take().is_none());
    }
    #[tokio::test]
    async fn asynchronous_refill_is_ordered_and_stops_on_drop() {
        let mut pool = IdPool::new();
        pool.start();
        for i in 0..BLOCK * 5 {
            let value = loop {
                if let Some(value) = pool.take() {
                    break value;
                }
                tokio::task::yield_now().await;
            };
            assert_eq!(value, Id::from_index(i).0);
        }
        drop(pool); // receiver drop wakes the blocked producer
        tokio::task::yield_now().await;
    }
}
