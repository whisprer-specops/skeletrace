// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! A binary min-heap, written out in full so the sift-up and sift-down steps are visible.
//!
//! The heap is a complete binary tree stored in a `Vec`: the children of slot `i` sit at `2i + 1` and `2i + 2`,
//! and its parent at `(i - 1) / 2`. The rule: every parent is no larger than its children, so the minimum sits
//! at slot 0.
//!
//! | Operation | Cost | How |
//! |---|---|---|
//! | [`MinHeap::peek`] | O(1) | read slot 0 |
//! | [`MinHeap::push`] | O(log n) | append, then sift up |
//! | [`MinHeap::pop`] | O(log n) | move the last item to the top, then sift down |
//! | [`MinHeap::from_vec`] | O(n) | sift down every parent, last first |
//!
//! **SkeleTrace use:** top-k lists ("the ten busiest nodes"), the K-way timeline merge in [`crate::merge`],
//! and ordering work by due time. `std::collections::BinaryHeap` (a max-heap) with `std::cmp::Reverse` does the
//! same job in production code; this one exists to show the mechanics without the `Reverse` wrapper.

/// A min-heap: [`MinHeap::pop`] always returns the smallest item.
#[derive(Clone, Debug)]
pub struct MinHeap<T> {
    data: Vec<T>,
}

impl<T: Ord> Default for MinHeap<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Ord> MinHeap<T> {
    /// An empty heap.
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }

    /// An empty heap with room for `capacity` items before it reallocates.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
        }
    }

    /// Builds a heap from `data` in O(n).
    pub fn from_vec(data: Vec<T>) -> Self {
        let mut heap = Self { data };
        // Sift down every parent, last first. This costs O(n), not O(n log n): half the slots are leaves that
        // never move, a quarter move at most one level, an eighth at most two, and the sum stays below n.
        for i in (0..heap.data.len() / 2).rev() {
            heap.sift_down(i);
        }
        heap
    }

    /// Number of items.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// True when the heap holds nothing.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The smallest item, without removing it. O(1).
    pub fn peek(&self) -> Option<&T> {
        self.data.first()
    }

    /// Adds an item. O(log n).
    pub fn push(&mut self, item: T) {
        self.data.push(item);
        let last = self.data.len() - 1;
        self.sift_up(last);
    }

    /// Removes and returns the smallest item. O(log n).
    pub fn pop(&mut self) -> Option<T> {
        let last = self.data.pop()?;
        if self.data.is_empty() {
            return Some(last);
        }
        let smallest = std::mem::replace(&mut self.data[0], last);
        self.sift_down(0);
        Some(smallest)
    }

    /// Empties the heap into an ascending `Vec`. O(n log n): this is heapsort.
    pub fn into_sorted_vec(mut self) -> Vec<T> {
        let mut out = Vec::with_capacity(self.data.len());
        while let Some(item) = self.pop() {
            out.push(item);
        }
        out
    }

    /// Sift up: the item at `i` may be smaller than its parent, so swap it upwards until it isn't.
    fn sift_up(&mut self, mut i: usize) {
        while i > 0 {
            let parent = (i - 1) / 2;
            if self.data[i] >= self.data[parent] {
                break;
            }
            self.data.swap(i, parent);
            i = parent;
        }
    }

    /// Sift down: the item at `i` may be larger than a child, so swap it with its smaller child until it isn't.
    fn sift_down(&mut self, mut i: usize) {
        let len = self.data.len();
        loop {
            let (left, right) = (2 * i + 1, 2 * i + 2);
            let mut smallest = i;
            if left < len && self.data[left] < self.data[smallest] {
                smallest = left;
            }
            if right < len && self.data[right] < self.data[smallest] {
                smallest = right;
            }
            if smallest == i {
                break;
            }
            self.data.swap(i, smallest);
            i = smallest;
        }
    }
}

impl<T: Ord> FromIterator<T> for MinHeap<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::from_vec(iter.into_iter().collect())
    }
}

/// The `k` largest items, largest first, in O(n log k) time and O(k) memory: keep a min-heap of the best `k`
/// seen so far, and let a newcomer in only if it beats the smallest of them.
pub fn top_k_largest<T: Ord>(items: impl IntoIterator<Item = T>, k: usize) -> Vec<T> {
    if k == 0 {
        return Vec::new();
    }
    let mut best = MinHeap::new();
    for item in items {
        if best.len() < k {
            best.push(item);
        } else if best.peek().is_some_and(|smallest| item > *smallest) {
            best.pop();
            best.push(item);
        }
    }
    let mut out = best.into_sorted_vec();
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Lcg;

    #[test]
    fn pops_in_ascending_order() {
        let mut rng = Lcg(7);
        let values: Vec<u64> = (0..1000).map(|_| rng.below(500)).collect();
        let mut heap = MinHeap::new();
        for &v in &values {
            heap.push(v);
        }
        let mut sorted = values.clone();
        sorted.sort_unstable();
        assert_eq!(heap.peek(), sorted.first());
        assert_eq!(heap.len(), 1000);
        assert_eq!(heap.into_sorted_vec(), sorted);
    }

    #[test]
    fn heapify_matches_pushes() {
        let mut rng = Lcg(8);
        let values: Vec<u64> = (0..777).map(|_| rng.below(100)).collect();
        let mut sorted = values.clone();
        sorted.sort_unstable();
        assert_eq!(MinHeap::from_vec(values.clone()).into_sorted_vec(), sorted);
        assert_eq!(
            values.into_iter().collect::<MinHeap<_>>().into_sorted_vec(),
            sorted
        );
    }

    #[test]
    fn empty_and_single() {
        let mut heap: MinHeap<i32> = MinHeap::default();
        assert!(heap.is_empty());
        assert_eq!(heap.pop(), None);
        heap.push(5);
        assert_eq!(heap.peek(), Some(&5));
        assert_eq!(heap.pop(), Some(5));
        assert_eq!(heap.pop(), None);
    }

    #[test]
    fn top_k() {
        assert_eq!(top_k_largest(vec![5, 1, 9, 3, 7, 9], 3), vec![9, 9, 7]);
        assert_eq!(top_k_largest(vec![2, 1], 5), vec![2, 1]);
        assert!(top_k_largest(Vec::<i32>::new(), 3).is_empty());
        assert!(top_k_largest(vec![1], 0).is_empty());
    }
}
