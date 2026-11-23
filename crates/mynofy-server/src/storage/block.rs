//! Snapshot sharing without reference-count checks on ordinary unique writes.
use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

// Keeping the Box inside the Arc lets try_unwrap recover the original allocation
// without copying when the snapshot is gone. Arc<[u8]> cannot be unwrapped this way.
#[allow(clippy::redundant_allocation)]
#[derive(Clone)]
pub(super) enum Block {
    Unique(Box<[u8]>),
    Shared(Arc<Box<[u8]>>),
}
impl Default for Block {
    fn default() -> Self {
        Self::Unique(Box::default())
    }
}
impl From<Box<[u8]>> for Block {
    fn from(bytes: Box<[u8]>) -> Self {
        Self::Unique(bytes)
    }
}
impl Block {
    pub(super) fn freeze(&mut self) {
        if matches!(self, Self::Unique(_)) {
            let Self::Unique(bytes) = std::mem::take(self) else {
                unreachable!()
            };
            *self = Self::Shared(Arc::new(bytes));
        }
    }
    fn unshare(&mut self) {
        let Self::Shared(shared) = std::mem::take(self) else {
            unreachable!()
        };
        *self = Self::Unique(Arc::try_unwrap(shared).unwrap_or_else(|shared| (*shared).clone()));
    }
    pub(super) fn sharing_bytes(&self) -> usize {
        match self {
            Self::Unique(_) => 0,
            // Arc control allocation, excluding allocator-specific rounding/headers.
            Self::Shared(_) => 2 * size_of::<usize>() + size_of::<Box<[u8]>>(),
        }
    }
}
impl Deref for Block {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &[u8] {
        match self {
            Self::Unique(bytes) => bytes,
            Self::Shared(bytes) => bytes,
        }
    }
}
impl DerefMut for Block {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        if matches!(self, Self::Shared(_)) {
            self.unshare();
        }
        let Self::Unique(bytes) = self else {
            unreachable!()
        };
        bytes
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_mutation_copies_once_and_unique_recovery_preserves_the_allocation() {
        let mut live = Block::from(vec![7; 128].into_boxed_slice());
        let original = live.as_ptr();
        live.freeze();
        let snapshot = live.clone();
        assert_eq!(snapshot.as_ptr(), original);
        live[0] = 9;
        assert_ne!(live.as_ptr(), original);
        assert_eq!(snapshot[0], 7);
        let unique = live.as_ptr();
        live[1] = 8;
        assert_eq!(live.as_ptr(), unique);
        live.freeze();
        let shared = live.clone();
        drop(shared);
        live[2] = 3;
        assert_eq!(live.as_ptr(), unique);
        assert!(matches!(live, Block::Unique(_)));
    }
}
