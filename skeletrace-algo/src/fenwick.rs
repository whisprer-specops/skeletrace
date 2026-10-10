// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Fenwick tree (binary indexed tree): running totals that stay cheap while the numbers keep changing.
//!
//! **Why O(log N) matters.** With a plain array, "total of the first i buckets" costs O(N) per query; keeping
//! prefix sums instead makes each update O(N). With a million buckets and a million updates and queries,
//! either way is about 10^12 steps. A Fenwick tree does both in about 20 steps each (log2 of a million), so
//! the same workload takes about 4 × 10^7 steps: seconds instead of days.
//!
//! **How it works.** Entry `i` (counting from 1) holds the total of the `lowbit(i)` values ending at `i`, where
//! `lowbit(i) = i & -i` is the lowest set bit of `i`. A prefix query strips the lowest bit each step
//! (`i -= lowbit(i)`); an update adds it (`i += lowbit(i)`). Either way, at most log2(N) + 1 steps.
//!
//! **SkeleTrace use:** event counts per time bucket, updated as events arrive and queried by window; see
//! [`crate::osint::TimeBuckets`].

/// Prefix sums over `len` integer values, all starting at 0, with O(log N) updates and queries.
#[derive(Clone, Debug)]
pub struct Fenwick {
    tree: Vec<i64>,
}

impl Fenwick {
    /// `len` values, all 0.
    pub fn new(len: usize) -> Self {
        Self {
            tree: vec![0; len + 1],
        }
    }

    /// A tree holding `values`, built in O(N): each entry passes its total up to the one entry that covers it.
    pub fn from_slice(values: &[i64]) -> Self {
        let len = values.len();
        let mut tree = vec![0; len + 1];
        tree[1..].copy_from_slice(values);
        for i in 1..=len {
            let parent = i + lowbit(i);
            if parent <= len {
                tree[parent] += tree[i];
            }
        }
        Self { tree }
    }

    /// Number of values.
    pub fn len(&self) -> usize {
        self.tree.len() - 1
    }

    /// True when there are no values.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Adds `delta` to the value at `index`. O(log N).
    ///
    /// # Panics
    /// If `index >= self.len()`.
    pub fn add(&mut self, index: usize, delta: i64) {
        assert!(
            index < self.len(),
            "Fenwick::add: index {index} out of range for length {}",
            self.len()
        );
        let mut i = index + 1;
        while i < self.tree.len() {
            self.tree[i] += delta;
            i += lowbit(i);
        }
    }

    /// Sum of the first `count` values (indices `0..count`). O(log N).
    ///
    /// # Panics
    /// If `count > self.len()`.
    pub fn prefix_sum(&self, count: usize) -> i64 {
        assert!(
            count <= self.len(),
            "Fenwick::prefix_sum: count {count} exceeds length {}",
            self.len()
        );
        let mut i = count;
        let mut sum = 0;
        while i > 0 {
            sum += self.tree[i];
            i -= lowbit(i);
        }
        sum
    }

    /// Sum of the values at indices `start..end`. O(log N).
    ///
    /// # Panics
    /// If `start > end` or `end > self.len()`.
    pub fn range_sum(&self, start: usize, end: usize) -> i64 {
        assert!(
            start <= end,
            "Fenwick::range_sum: start {start} is after end {end}"
        );
        self.prefix_sum(end) - self.prefix_sum(start)
    }

    /// The smallest index `i` where the sum of values `0..=i` reaches `target`, or `len()` if the total stays
    /// below it. Assumes no negative values. O(log N): walk down the powers of two instead of binary-searching
    /// over prefix sums, which would cost O(log² N).
    pub fn lower_bound(&self, target: i64) -> usize {
        if target <= 0 {
            return 0;
        }
        let len = self.len();
        let mut position = 0;
        let mut remaining = target;
        let mut step = if len == 0 { 0 } else { 1usize << len.ilog2() };
        while step > 0 {
            let next = position + step;
            if next <= len && self.tree[next] < remaining {
                position = next;
                remaining -= self.tree[next];
            }
            step >>= 1;
        }
        position
    }
}

/// The lowest set bit of `i`.
fn lowbit(i: usize) -> usize {
    i & i.wrapping_neg()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Lcg;

    #[test]
    fn matches_naive_sums_under_updates() {
        let mut rng = Lcg(11);
        let n = 257;
        let mut naive = vec![0i64; n];
        let mut tree = Fenwick::new(n);
        for _ in 0..2000 {
            let i = rng.below(n as u64) as usize;
            let delta = rng.below(21) as i64 - 10;
            naive[i] += delta;
            tree.add(i, delta);
            let a = rng.below(n as u64 + 1) as usize;
            let b = rng.below(n as u64 + 1) as usize;
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            assert_eq!(tree.range_sum(lo, hi), naive[lo..hi].iter().sum::<i64>());
        }
    }

    #[test]
    fn linear_build_is_correct() {
        let values: Vec<i64> = (1..=100).collect();
        let tree = Fenwick::from_slice(&values);
        for end in 0..=100 {
            assert_eq!(tree.prefix_sum(end), values[..end].iter().sum::<i64>());
        }
    }

    #[test]
    fn lower_bound_finds_first_index_reaching_target() {
        // inclusive prefix sums: 3, 3, 5, 10, 10, 11
        let tree = Fenwick::from_slice(&[3, 0, 2, 5, 0, 1]);
        let cases = [
            (0, 0),
            (1, 0),
            (3, 0),
            (4, 2),
            (5, 2),
            (6, 3),
            (10, 3),
            (11, 5),
            (12, 6),
        ];
        for (target, expected) in cases {
            assert_eq!(tree.lower_bound(target), expected, "target {target}");
        }
        assert_eq!(Fenwick::new(0).lower_bound(5), 0);
    }
}
