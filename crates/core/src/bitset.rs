//! A minimal fixed-capacity bitset.
//!
//! Used for the group ancestor/descendant closures and for entity-by-slot
//! occupancy. Hand-rolled rather than pulled in as a dependency: the operations
//! needed are `insert`/`contains`/`union`/`iter`, all of which sit in the local
//! search hot loop, and a dependency would add a compile-time surface for
//! nothing.

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BitSet {
    words: Vec<u64>,
    capacity: usize,
}

impl BitSet {
    pub fn new(capacity: usize) -> Self {
        Self { words: vec![0; capacity.div_ceil(64)], capacity }
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    pub fn insert(&mut self, i: usize) {
        debug_assert!(i < self.capacity);
        self.words[i / 64] |= 1u64 << (i % 64);
    }

    #[inline]
    pub fn remove(&mut self, i: usize) {
        debug_assert!(i < self.capacity);
        self.words[i / 64] &= !(1u64 << (i % 64));
    }

    #[inline]
    pub fn contains(&self, i: usize) -> bool {
        debug_assert!(i < self.capacity);
        self.words[i / 64] & (1u64 << (i % 64)) != 0
    }

    pub fn union_with(&mut self, other: &BitSet) {
        debug_assert_eq!(self.capacity, other.capacity);
        for (a, b) in self.words.iter_mut().zip(&other.words) {
            *a |= *b;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|&w| w == 0)
    }

    pub fn count(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Set indices, ascending. Order is deterministic, which matters because
    /// derived `Vec`s feed straight into output and test assertions.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.words.iter().enumerate().flat_map(|(w, &word)| {
            (0..64).filter_map(
                move |b| {
                    if word & (1u64 << b) != 0 { Some(w * 64 + b) } else { None }
                },
            )
        })
    }
}

/// A dense `rows x cols` bit matrix, row-major so one row is contiguous.
#[derive(Clone, Debug)]
pub struct BitMatrix {
    words: Vec<u64>,
    cols: usize,
    words_per_row: usize,
}

impl BitMatrix {
    pub fn new(rows: usize, cols: usize) -> Self {
        let words_per_row = cols.div_ceil(64);
        Self { words: vec![0; rows * words_per_row], cols, words_per_row }
    }

    #[inline]
    fn addr(&self, row: usize, col: usize) -> (usize, u64) {
        debug_assert!(col < self.cols);
        (row * self.words_per_row + col / 64, 1u64 << (col % 64))
    }

    #[inline]
    pub fn get(&self, row: usize, col: usize) -> bool {
        let (w, mask) = self.addr(row, col);
        self.words[w] & mask != 0
    }

    #[inline]
    pub fn set(&mut self, row: usize, col: usize) {
        let (w, mask) = self.addr(row, col);
        self.words[w] |= mask;
    }

    #[inline]
    pub fn clear(&mut self, row: usize, col: usize) {
        let (w, mask) = self.addr(row, col);
        self.words[w] &= !mask;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitset_basics_across_word_boundaries() {
        let mut s = BitSet::new(200);
        for i in [0usize, 63, 64, 65, 199] {
            assert!(!s.contains(i));
            s.insert(i);
            assert!(s.contains(i));
        }
        assert_eq!(s.count(), 5);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![0, 63, 64, 65, 199]);
        s.remove(64);
        assert!(!s.contains(64));
        assert_eq!(s.count(), 4);
    }

    #[test]
    fn union_is_elementwise() {
        let mut a = BitSet::new(70);
        let mut b = BitSet::new(70);
        a.insert(1);
        a.insert(65);
        b.insert(2);
        b.insert(65);
        a.union_with(&b);
        assert_eq!(a.iter().collect::<Vec<_>>(), vec![1, 2, 65]);
    }

    #[test]
    fn matrix_rows_do_not_alias() {
        let mut m = BitMatrix::new(3, 130);
        m.set(1, 129);
        assert!(m.get(1, 129));
        assert!(!m.get(0, 129));
        assert!(!m.get(2, 129));
        m.clear(1, 129);
        assert!(!m.get(1, 129));
    }
}
