//! Ordered positions: small vector, then bitmap blocks with packed live values.
use std::{
    collections::BTreeMap,
    mem::size_of,
    ops::{Bound, RangeBounds},
};
const WIDTH: u64 = 64;
#[derive(Clone, Copy)]
pub(crate) struct Entry<T: Copy> {
    position: u64,
    slot: T,
}
pub(crate) struct Block<T: Copy> {
    base: u64,
    live: u64,
    slots: Vec<T>,
}
impl<T: Copy> Block<T> {
    fn insert(&mut self, position: u64, slot: T) -> Option<T> {
        let bit = position - self.base;
        let mask = 1u64 << bit;
        let rank = (self.live & (mask - 1)).count_ones() as usize;
        if self.live & mask != 0 {
            return Some(std::mem::replace(&mut self.slots[rank], slot));
        }
        self.live |= mask;
        self.slots.insert(rank, slot);
        None
    }
    fn remove(&mut self, position: u64) -> Option<T> {
        let bit = position - self.base;
        let mask = 1u64 << bit;
        if self.live & mask == 0 {
            return None;
        }
        let rank = (self.live & (mask - 1)).count_ones() as usize;
        self.live &= !mask;
        Some(self.slots.remove(rank))
    }
}
enum State<T: Copy> {
    Small(Vec<Entry<T>>),
    Blocked(BTreeMap<u64, Block<T>>),
}
pub(crate) struct OrderedIndex<T: Copy = u32> {
    state: State<T>,
    len: usize,
    reclaim_after: Option<u64>,
}
impl<T: Copy> OrderedIndex<T> {
    pub(crate) fn new() -> Self {
        Self {
            state: State::Small(Vec::new()),
            len: 0,
            reclaim_after: None,
        }
    }
    pub(crate) fn len(&self) -> usize {
        self.len
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub(crate) fn insert(&mut self, position: u64, slot: T) {
        if let State::Small(entries) = &mut self.state {
            if entries.len() < 32 {
                let index = entries.partition_point(|e| e.position < position);
                if entries.get(index).is_some_and(|e| e.position == position) {
                    entries[index].slot = slot;
                    return;
                }
                entries.insert(index, Entry { position, slot });
                self.len += 1;
                return;
            }
            let old = std::mem::take(entries);
            let mut blocks = BTreeMap::new();
            for entry in old {
                Self::append(&mut blocks, entry.position, entry.slot);
            }
            self.state = State::Blocked(blocks);
        }
        if let State::Blocked(blocks) = &mut self.state
            && Self::append(blocks, position, slot).is_some()
        {
            return;
        }
        self.len += 1;
    }
    fn append(blocks: &mut BTreeMap<u64, Block<T>>, position: u64, slot: T) -> Option<T> {
        let key = position / WIDTH;
        blocks
            .entry(key)
            .or_insert_with(|| Block {
                base: key * WIDTH,
                live: 0,
                slots: Vec::new(),
            })
            .insert(position, slot)
    }
    #[cfg(test)]
    pub(crate) fn get_mut(&mut self, position: u64) -> Option<&mut T> {
        match &mut self.state {
            State::Small(entries) => entries
                .binary_search_by_key(&position, |e| e.position)
                .ok()
                .map(|i| &mut entries[i].slot),
            State::Blocked(blocks) => {
                let b = blocks.get_mut(&(position / WIDTH))?;
                let bit = 1u64 << (position - b.base);
                if b.live & bit == 0 {
                    return None;
                }
                b.slots.get_mut((b.live & (bit - 1)).count_ones() as usize)
            }
        }
    }
    pub(crate) fn remove(&mut self, position: &u64) -> Option<T> {
        let result = match &mut self.state {
            State::Small(entries) => {
                let index = entries
                    .binary_search_by_key(position, |e| e.position)
                    .ok()?;
                Some(entries.remove(index).slot)
            }
            State::Blocked(blocks) => {
                let key = position / WIDTH;
                let block = blocks.get_mut(&key)?;
                let slot = block.remove(*position)?;
                if block.live == 0 {
                    blocks.remove(&key);
                }
                Some(slot)
            }
        };
        if result.is_some() {
            self.len -= 1;
        }
        if self.len <= 16 && matches!(self.state, State::Blocked(_)) {
            let entries = self
                .range(..)
                .map(|(position, slot)| Entry { position, slot })
                .collect();
            self.state = State::Small(entries);
            self.reclaim_after = None;
        }
        result
    }
    pub(crate) fn get(&self, position: u64) -> Option<&T> {
        match &self.state {
            State::Small(entries) => entries
                .binary_search_by_key(&position, |e| e.position)
                .ok()
                .map(|i| &entries[i].slot),
            State::Blocked(blocks) => {
                let b = blocks.get(&(position / WIDTH))?;
                let bit = 1u64 << (position - b.base);
                if b.live & bit == 0 {
                    return None;
                }
                b.slots.get((b.live & (bit - 1)).count_ones() as usize)
            }
        }
    }

    pub(crate) fn capacity(&self) -> usize {
        match &self.state {
            State::Small(e) => e.capacity(),
            State::Blocked(b) => b.values().map(|v| v.slots.capacity()).sum(),
        }
    }
    pub(crate) fn range<R: RangeBounds<u64>>(&self, range: R) -> Range<'_, T> {
        let lower = match range.start_bound() {
            Bound::Included(&p) => p,
            Bound::Excluded(&p) => match p.checked_add(1) {
                Some(p) => p,
                None => return Range::empty(),
            },
            Bound::Unbounded => 0,
        };
        let upper = match range.end_bound() {
            Bound::Included(&p) => p,
            Bound::Excluded(&p) => match p.checked_sub(1) {
                Some(p) => p,
                None => return Range::empty(),
            },
            Bound::Unbounded => u64::MAX,
        };
        if lower > upper {
            return Range::empty();
        }
        match &self.state {
            State::Small(entries) => {
                let start = entries.partition_point(|e| e.position < lower);
                let end = entries.partition_point(|e| e.position <= upper);
                Range::Small(entries[start..end].iter())
            }
            State::Blocked(blocks) => Range::Blocked {
                blocks: blocks.range((lower / WIDTH)..=(upper / WIDTH)),
                current: None,
                lower,
                upper,
            },
        }
    }
    #[cfg(test)]
    pub(crate) fn values(&self) -> impl Iterator<Item = T> + '_ {
        self.range(..).map(|(_, slot)| slot)
    }
    pub(crate) fn reclaim(&mut self) {
        match &mut self.state {
            State::Small(entries) => entries.shrink_to_fit(),
            State::Blocked(blocks) => {
                let keys: Vec<_> = blocks
                    .range((
                        self.reclaim_after.map_or(Bound::Unbounded, Bound::Excluded),
                        Bound::Unbounded,
                    ))
                    .take(16)
                    .map(|(&k, _)| k)
                    .collect();
                for key in &keys {
                    if let Some(block) = blocks.get_mut(key) {
                        block.slots.shrink_to_fit();
                    }
                }
                self.reclaim_after = keys.last().copied();
                if keys.len() < 16 {
                    self.reclaim_after = None;
                }
            }
        }
    }
    pub(crate) fn data_bytes(&self) -> usize {
        match &self.state {
            State::Small(e) => e.capacity() * size_of::<Entry<T>>(),
            State::Blocked(b) => b
                .values()
                .map(|v| v.slots.capacity() * size_of::<T>())
                .sum(),
        }
    }
    pub(crate) fn blocks(&self) -> usize {
        match &self.state {
            State::Small(e) => usize::from(!e.is_empty()),
            State::Blocked(b) => b.len(),
        }
    }
}
pub(crate) enum Range<'a, T: Copy> {
    Small(std::slice::Iter<'a, Entry<T>>),
    Blocked {
        blocks: std::collections::btree_map::Range<'a, u64, Block<T>>,
        current: Option<BlockIter<'a, T>>,
        lower: u64,
        upper: u64,
    },
}
pub(crate) struct BlockIter<'a, T: Copy> {
    base: u64,
    bits: u64,
    index: usize,
    slots: &'a [T],
}
impl<'a, T: Copy> Range<'a, T> {
    fn empty() -> Self {
        Self::Small([].iter())
    }
}
impl<T: Copy> Iterator for Range<'_, T> {
    type Item = (u64, T);
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Small(entries) => entries.next().map(|e| (e.position, e.slot)),
            Self::Blocked {
                blocks,
                current,
                lower,
                upper,
            } => loop {
                if let Some(block) = current
                    && block.bits != 0
                {
                    let bit = block.bits.trailing_zeros();
                    block.bits &= block.bits - 1;
                    let slot = block.slots[block.index];
                    block.index += 1;
                    return Some((block.base + u64::from(bit), slot));
                }
                let (_, block) = blocks.next()?;
                let lo = lower.saturating_sub(block.base).min(64) as u32;
                let hi = upper.saturating_sub(block.base).min(63) as u32;
                let below = if lo == 64 { u64::MAX } else { (1u64 << lo) - 1 };
                let above = if hi == 63 {
                    u64::MAX
                } else {
                    (1u64 << (hi + 1)) - 1
                };
                *current = Some(BlockIter {
                    base: block.base,
                    bits: block.live & !below & above,
                    index: (block.live & below).count_ones() as usize,
                    slots: &block.slots,
                });
            },
        }
    }
}
pub(crate) struct IdTable<T: Copy> {
    index: OrderedIndex<T>,
}
impl<T: Copy> IdTable<T> {
    pub(crate) fn new() -> Self {
        Self {
            index: OrderedIndex::new(),
        }
    }
    pub(crate) fn get(&self, id: &crate::ids::Id) -> Option<&T> {
        self.index.get(u64::from_be_bytes(id.0))
    }
    pub(crate) fn contains_key(&self, id: &crate::ids::Id) -> bool {
        self.get(id).is_some()
    }
    pub(crate) fn insert(&mut self, id: crate::ids::Id, value: T) {
        self.index.insert(u64::from_be_bytes(id.0), value);
    }

    pub(crate) fn remove(&mut self, id: &crate::ids::Id) -> Option<T> {
        self.index.remove(&u64::from_be_bytes(id.0))
    }
    pub(crate) fn len(&self) -> usize {
        self.index.len()
    }
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.index.len() == 0
    }
    pub(crate) fn capacity(&self) -> usize {
        self.index.capacity()
    }
    pub(crate) fn data_bytes(&self) -> usize {
        self.index.data_bytes()
    }
    pub(crate) fn reclaim(&mut self) {
        self.index.reclaim();
    }
}
impl<T: Copy> std::ops::Index<&crate::ids::Id> for IdTable<T> {
    type Output = T;
    fn index(&self, id: &crate::ids::Id) -> &T {
        self.get(id).expect("Existing internal record key")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn id_table_accepts_wrapped_and_out_of_order_keys() {
        let mut table = IdTable::new();
        for n in (0..200u64).rev() {
            let id = crate::ids::Id((u64::MAX - n).to_be_bytes());
            table.insert(id, n);
        }
        let zero = crate::ids::Id([0; 8]);
        table.insert(zero, 99);
        table.insert(zero, 100);
        assert_eq!(table.len(), 201);
        assert_eq!(table.get(&zero), Some(&100));
        for n in 0..200u64 {
            let id = crate::ids::Id((u64::MAX - n).to_be_bytes());
            assert_eq!(table.remove(&id), Some(n));
        }
        table.reclaim();
        assert_eq!(table.remove(&zero), Some(100));
        table.reclaim();
        assert_eq!(table.data_bytes(), 0);
    }
    #[test]
    fn matches_tree_across_sparse_deletion_and_mode_changes() {
        let mut index = OrderedIndex::new();
        let mut tree = BTreeMap::new();
        for position in 1..3000u64 {
            index.insert(position, position as u32);
            tree.insert(position, position as u32);
        }
        for position in 1..3000u64 {
            if position % 17 != 0 {
                assert_eq!(index.remove(&position), tree.remove(&position));
            }
        }
        for start in [0, 1, 63, 64, 65, 2000, 2999, 3000] {
            for end in [0, 17, 63, 64, 150, 2999, u64::MAX] {
                if start <= end {
                    assert_eq!(
                        index.range(start..=end).collect::<Vec<_>>(),
                        tree.range(start..=end)
                            .map(|(&p, &s)| (p, s))
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
        for (&position, _) in tree.iter().take(tree.len() - 10) {
            index.remove(&position);
        }
        assert_eq!(index.len(), 10);
        assert!(matches!(index.state, State::Small(_)));
        index.reclaim();
        for position in 4000..5000 {
            index.insert(position, position as u32);
        }
        assert_eq!(
            index
                .range((Bound::Excluded(u64::MAX), Bound::Unbounded))
                .count(),
            0
        );
    }
}

#[cfg(test)]
mod functional {
    use super::*;
    use crate::test_support::{self, Rng};
    #[test]
    fn functional_seeded_ordered_index_matches_btree_through_reclaim_and_boundaries() {
        let mut rng = Rng(test_support::seed());
        let mut index = OrderedIndex::<u64>::new();
        let mut model = BTreeMap::new();
        let cases = test_support::fuzz_cases();
        for case in 0..cases {
            let position = match rng.pick(4) {
                0 => u64::MAX - rng.pick(128) as u64,
                1 => rng.pick(128) as u64,
                _ => rng.pick(512) as u64,
            };
            match rng.pick(5) {
                0 | 1 => {
                    let value = rng.next();
                    index.insert(position, value);
                    model.insert(position, value);
                }
                2 => assert_eq!(index.remove(&position), model.remove(&position)),
                3 => {
                    if let Some(value) = index.get_mut(position) {
                        *value ^= 0x55;
                    }
                    if let Some(value) = model.get_mut(&position) {
                        *value ^= 0x55;
                    }
                }
                _ => index.reclaim(),
            }
            assert_eq!(index.get(position), model.get(&position));
            assert_eq!(index.len(), model.len());
            let lower = match rng.pick(3) {
                0 => Bound::Unbounded,
                1 => Bound::Included(position),
                _ => Bound::Excluded(position),
            };
            let high = match rng.pick(3) {
                0 => Bound::Unbounded,
                1 => Bound::Included(rng.next()),
                _ => Bound::Excluded(if rng.pick(2) == 0 { position } else { 0 }),
            };
            let expected: Vec<_> = model
                .iter()
                .filter(|(key, _)| match lower {
                    Bound::Unbounded => true,
                    Bound::Included(p) => **key >= p,
                    Bound::Excluded(p) => **key > p,
                })
                .filter(|(key, _)| match high {
                    Bound::Unbounded => true,
                    Bound::Included(p) => **key <= p,
                    Bound::Excluded(p) => **key < p,
                })
                .map(|(key, value)| (*key, *value))
                .collect();
            assert_eq!(
                index.range((lower, high)).collect::<Vec<_>>(),
                expected,
                "case={case}"
            );
            if case % 127 == 0 {
                assert_eq!(
                    index.range(..).collect::<Vec<_>>(),
                    model.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>()
                );
                // Force transitions back from bitmap blocks to the small representation.
                let remove: Vec<_> = model
                    .keys()
                    .copied()
                    .take(model.len().saturating_sub(3))
                    .collect();
                for key in remove {
                    assert_eq!(index.remove(&key), model.remove(&key));
                }
                index.reclaim();
            }
        }
        eprintln!("Ordered-index model: {cases} operations including U64 boundaries and shrinking");
    }
}
