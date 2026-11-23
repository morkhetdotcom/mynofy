//! Exact-deadline batches. Isolated deadlines fall back to compact heap entries.
use std::{cmp::Reverse, collections::BinaryHeap, mem::size_of};
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Ref {
    Single(u32),
    Group(u32),
}
pub(crate) enum Batch {
    Single(u32),
    Many(Vec<u32>),
}
impl Batch {
    fn push(&mut self, slot: u32) {
        match self {
            Self::Single(old) => *self = Self::Many(vec![*old, slot]),
            Self::Many(slots) => slots.push(slot),
        }
    }
    fn len(&self) -> usize {
        match self {
            Self::Single(_) => 1,
            Self::Many(v) => v.len(),
        }
    }
    pub(crate) fn slots(self) -> Slots {
        match self {
            Self::Single(slot) => Slots::Single(Some(slot)),
            Self::Many(slots) => {
                let capacity = slots.capacity();
                Slots::Many {
                    values: slots.into_iter(),
                    capacity,
                }
            }
        }
    }
}
pub(crate) enum Slots {
    Single(Option<u32>),
    Many {
        values: std::vec::IntoIter<u32>,
        capacity: usize,
    },
}
impl Slots {
    pub(crate) fn allocated_bytes(&self) -> usize {
        match self {
            Self::Single(_) => 0,
            Self::Many { capacity, .. } => capacity * 4,
        }
    }
}
impl Iterator for Slots {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        match self {
            Self::Single(slot) => slot.take(),
            Self::Many { values, .. } => values.next(),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = match self {
            Self::Single(slot) => usize::from(slot.is_some()),
            Self::Many { values, .. } => values.len(),
        };
        (len, Some(len))
    }
}
impl ExactSizeIterator for Slots {}

pub(crate) struct Expiration {
    hot: Vec<(u64, Batch)>,
    eviction: usize,
    heap: BinaryHeap<Reverse<(u64, Ref)>>,
    groups: Vec<Option<Vec<u32>>>,
    free: Vec<u32>,
    len: usize,
}
impl Expiration {
    pub(crate) fn new() -> Self {
        Self {
            hot: Vec::new(),
            eviction: 0,
            heap: BinaryHeap::new(),
            groups: Vec::new(),
            free: Vec::new(),
            len: 0,
        }
    }
    pub(crate) fn insert(&mut self, (deadline, slot): (u64, u32)) {
        if let Some((_, batch)) = self.hot.iter_mut().find(|(time, _)| *time == deadline) {
            batch.push(slot);
            self.len += 1;
            return;
        }
        if self.hot.len() == 16 {
            let index = self.eviction % 16;
            self.eviction = (index + 1) % 16;
            let (time, batch) =
                std::mem::replace(&mut self.hot[index], (deadline, Batch::Single(slot)));
            let reference = match batch {
                Batch::Single(slot) => Ref::Single(slot),
                Batch::Many(slots) => {
                    let id = if let Some(id) = self.free.pop() {
                        id
                    } else {
                        self.groups.push(None);
                        (self.groups.len() - 1) as u32
                    };
                    self.groups[id as usize] = Some(slots);
                    Ref::Group(id)
                }
            };
            self.heap.push(Reverse((time, reference)));
        } else {
            self.hot.push((deadline, Batch::Single(slot)));
        }
        self.len += 1;
    }
    pub(crate) fn has_due(&self, now: u64) -> bool {
        self.hot.iter().any(|(deadline, _)| *deadline <= now)
            || self.heap.peek().is_some_and(|entry| entry.0.0 <= now)
    }
    pub(crate) fn pop_due(&mut self, now: u64) -> Option<Batch> {
        let hot = self
            .hot
            .iter()
            .enumerate()
            .min_by_key(|(_, h)| h.0)
            .map(|(i, h)| (i, h.0));
        let heap = self.heap.peek().map(|e| e.0.0);
        let batch = if hot.is_some_and(|(_, time)| time <= now && heap.is_none_or(|h| time <= h)) {
            self.hot.swap_remove(hot?.0).1
        } else if heap.is_some_and(|d| d <= now) {
            match self.heap.pop()?.0.1 {
                Ref::Single(slot) => Batch::Single(slot),
                Ref::Group(id) => {
                    let slots = self.groups[id as usize].take()?;
                    self.free.push(id);
                    Batch::Many(slots)
                }
            }
        } else {
            return None;
        };
        self.len -= batch.len();
        Some(batch)
    }
    pub(crate) fn len(&self) -> usize {
        self.len
    }
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub(crate) fn buckets(&self) -> usize {
        self.heap.len() + self.hot.len()
    }
    pub(crate) fn data_bytes(&self) -> usize {
        self.heap.capacity() * size_of::<Reverse<(u64, Ref)>>()
            + self.groups.capacity() * size_of::<Option<Vec<u32>>>()
            + self.free.capacity() * 4
            + self
                .groups
                .iter()
                .flatten()
                .map(|v| v.capacity() * 4)
                .sum::<usize>()
            + self.hot.capacity() * size_of::<(u64, Batch)>()
            + self
                .hot
                .iter()
                .map(|(_, batch)| match batch {
                    Batch::Many(v) => v.capacity() * 4,
                    _ => 0,
                })
                .sum::<usize>()
    }
    pub(crate) fn reclaim(&mut self) {
        if self.len == 0 {
            *self = Self::new();
        } else {
            if self.heap.capacity() > self.heap.len().saturating_mul(4).max(64) {
                self.heap.shrink_to(self.heap.len() * 2);
            }
            while self.groups.last().is_some_and(Option::is_none) {
                self.groups.pop();
            }
            self.free.retain(|id| (*id as usize) < self.groups.len());
            if self.groups.capacity() > self.groups.len().saturating_mul(4) {
                self.groups.shrink_to_fit();
                self.free.shrink_to_fit();
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_deadlines_and_mixed_order_match_sorted_reference() {
        let mut e = Expiration::new();
        let mut reference = std::collections::BTreeSet::new();
        for slot in 0..1000 {
            let time = 10 + (slot * 17) % 103;
            e.insert((time as u64, slot));
            reference.insert((time as u64, slot));
        }
        for now in 0..120 {
            let mut actual = Vec::new();
            while let Some(batch) = e.pop_due(now) {
                actual.extend(batch.slots());
            }
            actual.sort();
            let expected: Vec<_> = reference
                .iter()
                .filter(|(d, _)| *d <= now)
                .copied()
                .collect();
            let mut slots: Vec<_> = expected.iter().map(|e| e.1).collect();
            slots.sort();
            assert_eq!(actual, slots);
            for entry in expected {
                reference.remove(&entry);
            }
        }
        assert!(e.is_empty());
        e.reclaim();
        assert_eq!(e.data_bytes(), 0);
    }
    #[test]
    fn interleaved_common_deadlines_are_coalesced() {
        let mut e = Expiration::new();
        for slot in 0..10000 {
            e.insert((1000 + u64::from(slot % 2), slot));
        }
        assert_eq!(e.buckets(), 2);
        assert_eq!(e.pop_due(1000).unwrap().slots().count(), 5000);
        assert_eq!(e.len(), 5000);
        assert!(e.pop_due(1000).is_none());
        assert_eq!(e.pop_due(1001).unwrap().slots().count(), 5000);
    }
    #[test]
    fn common_deadline_is_one_bucket() {
        let mut e = Expiration::new();
        for slot in 0..10000 {
            e.insert((1234, slot));
        }
        assert_eq!(e.buckets(), 1);
        assert!(e.pop_due(1233).is_none());
        assert_eq!(e.pop_due(1234).unwrap().slots().count(), 10000);
    }
}

#[cfg(test)]
mod functional {
    use super::*;
    use crate::test_support::{self, Rng};
    #[test]
    fn functional_seeded_expiration_matches_reference_under_insert_pop_and_reclaim() {
        let mut rng = Rng(test_support::seed());
        let mut index = Expiration::new();
        let mut model = std::collections::BTreeMap::<u32, u64>::new();
        let cases = test_support::fuzz_cases();
        let mut slot = 0;
        for case in 0..cases {
            if rng.pick(3) != 0 {
                let deadline = match rng.pick(4) {
                    0 => 0,
                    1 => u64::MAX,
                    _ => rng.pick(128) as u64,
                };
                index.insert((deadline, slot));
                model.insert(slot, deadline);
                slot += 1;
            } else {
                let now = if rng.pick(16) == 0 {
                    u64::MAX
                } else {
                    rng.pick(128) as u64
                };
                let mut actual = std::collections::BTreeSet::new();
                while let Some(batch) = index.pop_due(now) {
                    for slot in batch.slots() {
                        assert!(actual.insert(slot), "duplicate slot");
                    }
                }
                let expected = model
                    .iter()
                    .filter(|(_, time)| **time <= now)
                    .map(|(slot, _)| *slot)
                    .collect();
                assert_eq!(actual, expected, "case={case}");
                model.retain(|_, time| *time > now);
                assert!(!index.has_due(now));
            }
            assert_eq!(index.len(), model.len());
            if case % 17 == 0 {
                index.reclaim();
            }
        }
        let mut final_slots = std::collections::BTreeSet::new();
        while let Some(batch) = index.pop_due(u64::MAX) {
            for slot in batch.slots() {
                assert!(final_slots.insert(slot));
            }
        }
        assert_eq!(final_slots, model.into_keys().collect());
        index.reclaim();
        assert!(index.is_empty());
        eprintln!(
            "Expiration model: {cases} operations with exact/duplicate deadlines and reclaim"
        );
    }
}
