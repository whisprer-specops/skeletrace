// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Segment tree: the sum, minimum or maximum of any range, with point updates, both O(log N).
//!
//! Leaves hold the values; each parent holds the combination of its two children. A range query climbs from
//! both ends of the range and only touches O(log N) nodes. This version is iterative and stores the tree in a
//! flat `Vec` of 2N entries: leaves at `N..2N`, parents below, the parent of `i` at `i / 2`.
//!
//! **SkeleTrace use:** "the busiest minute between 14:00 and 15:00" while counts keep changing; see
//! [`crate::osint::TimeBuckets`].

/// How to combine values: an associative operation with an identity (combining with it changes nothing).
pub trait Monoid {
    /// The value type.
    type Item: Clone;
    /// The value that changes nothing when combined: 0 for sums, the largest value for minimums.
    fn identity() -> Self::Item;
    /// Combines two values. Must be associative: `combine(a, combine(b, c)) == combine(combine(a, b), c)`.
    fn combine(a: &Self::Item, b: &Self::Item) -> Self::Item;
}

/// Range sums of `i64`.
#[derive(Clone, Copy, Debug)]
pub struct Sum;

impl Monoid for Sum {
    type Item = i64;
    fn identity() -> i64 {
        0
    }
    fn combine(a: &i64, b: &i64) -> i64 {
        a + b
    }
}

/// Range minimums of `i64` (an empty range gives `i64::MAX`).
#[derive(Clone, Copy, Debug)]
pub struct Min;

impl Monoid for Min {
    type Item = i64;
    fn identity() -> i64 {
        i64::MAX
    }
    fn combine(a: &i64, b: &i64) -> i64 {
        *a.min(b)
    }
}

/// Range maximums of `i64` (an empty range gives `i64::MIN`).
#[derive(Clone, Copy, Debug)]
pub struct Max;

impl Monoid for Max {
    type Item = i64;
    fn identity() -> i64 {
        i64::MIN
    }
    fn combine(a: &i64, b: &i64) -> i64 {
        *a.max(b)
    }
}

/// A segment tree over `len` values, combined with `M`.
pub struct SegTree<M: Monoid> {
    len: usize,
    tree: Vec<M::Item>,
    _monoid: std::marker::PhantomData<M>,
}

impl<M: Monoid> SegTree<M> {
    /// `len` values, all set to the identity.
    pub fn new(len: usize) -> Self {
        Self {
            len,
            tree: vec![M::identity(); 2 * len],
            _monoid: std::marker::PhantomData,
        }
    }

    /// A tree holding `values`, built in O(N).
    pub fn from_slice(values: &[M::Item]) -> Self {
        let len = values.len();
        let mut tree = vec![M::identity(); 2 * len];
        tree[len..].clone_from_slice(values);
        for i in (1..len).rev() {
            tree[i] = M::combine(&tree[2 * i], &tree[2 * i + 1]);
        }
        Self {
            len,
            tree,
            _monoid: std::marker::PhantomData,
        }
    }

    /// Number of values.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when there are no values.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The value at `index`.
    ///
    /// # Panics
    /// If `index >= self.len()`.
    pub fn get(&self, index: usize) -> &M::Item {
        assert!(
            index < self.len,
            "SegTree::get: index {index} out of range for length {}",
            self.len
        );
        &self.tree[index + self.len]
    }

    /// Sets the value at `index`, then fixes every parent above it. O(log N).
    ///
    /// # Panics
    /// If `index >= self.len()`.
    pub fn update(&mut self, index: usize, value: M::Item) {
        assert!(
            index < self.len,
            "SegTree::update: index {index} out of range for length {}",
            self.len
        );
        let mut i = index + self.len;
        self.tree[i] = value;
        while i > 1 {
            i /= 2;
            self.tree[i] = M::combine(&self.tree[2 * i], &self.tree[2 * i + 1]);
        }
    }

    /// The combination of the values at `start..end` (the identity for an empty range). O(log N).
    ///
    /// # Panics
    /// If `start > end` or `end > self.len()`.
    pub fn query(&self, start: usize, end: usize) -> M::Item {
        assert!(
            start <= end && end <= self.len,
            "SegTree::query: bad range {start}..{end} for length {}",
            self.len
        );
        let (mut l, mut r) = (start + self.len, end + self.len);
        // Two accumulators keep left-to-right order, which matters for operations that aren't commutative.
        let (mut left, mut right) = (M::identity(), M::identity());
        while l < r {
            if l % 2 == 1 {
                left = M::combine(&left, &self.tree[l]);
                l += 1;
            }
            if r % 2 == 1 {
                r -= 1;
                right = M::combine(&self.tree[r], &right);
            }
            l /= 2;
            r /= 2;
        }
        M::combine(&left, &right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Lcg;

    #[test]
    fn matches_naive_for_sum_min_max() {
        let mut rng = Lcg(5);
        let n = 37;
        let mut values: Vec<i64> = (0..n).map(|_| rng.below(100) as i64 - 50).collect();
        let mut sums = SegTree::<Sum>::from_slice(&values);
        let mut mins = SegTree::<Min>::from_slice(&values);
        let mut maxs = SegTree::<Max>::from_slice(&values);
        for _ in 0..500 {
            let i = rng.below(n as u64) as usize;
            let v = rng.below(100) as i64 - 50;
            values[i] = v;
            sums.update(i, v);
            mins.update(i, v);
            maxs.update(i, v);
            let a = rng.below(n as u64 + 1) as usize;
            let b = rng.below(n as u64 + 1) as usize;
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let slice = &values[lo..hi];
            assert_eq!(sums.query(lo, hi), slice.iter().sum::<i64>());
            assert_eq!(
                mins.query(lo, hi),
                slice.iter().copied().min().unwrap_or(i64::MAX)
            );
            assert_eq!(
                maxs.query(lo, hi),
                slice.iter().copied().max().unwrap_or(i64::MIN)
            );
        }
    }

    #[test]
    fn new_starts_at_identity() {
        let mut tree = SegTree::<Max>::new(4);
        assert_eq!(tree.query(0, 4), i64::MIN);
        tree.update(2, 7);
        assert_eq!(*tree.get(2), 7);
        assert_eq!(tree.query(0, 2), i64::MIN);
        assert_eq!(tree.query(1, 4), 7);
    }
}
