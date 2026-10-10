// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Merging K sorted sequences into one sorted sequence: O(N log K) for N items in total.
//!
//! A min-heap holds one "head" item from each source. Pop the smallest, then refill from the source it came
//! from. Every item passes through a heap of at most K items once, hence O(log K) per item.
//!
//! **SkeleTrace use:** several feeds (ADS-B receivers, AIS stations, log files), each already in time order,
//! become one timeline without loading everything and sorting it again. Equal items come out in source order,
//! so the merge is deterministic.

use crate::heap::MinHeap;

/// An iterator over the merged output of several sorted iterators. Made by [`merge_sorted`].
pub struct KWayMerge<I: Iterator> {
    heads: MinHeap<(I::Item, usize)>,
    sources: Vec<I>,
}

/// Merges sorted sources into one sorted iterator. Each source must already be in ascending order: the merge
/// does not check, and unsorted sources give unsorted output.
pub fn merge_sorted<S, I>(sources: S) -> KWayMerge<I::IntoIter>
where
    S: IntoIterator<Item = I>,
    I: IntoIterator,
    I::Item: Ord,
{
    let mut sources: Vec<I::IntoIter> = sources.into_iter().map(IntoIterator::into_iter).collect();
    let mut heads = MinHeap::with_capacity(sources.len());
    for (index, source) in sources.iter_mut().enumerate() {
        if let Some(first) = source.next() {
            heads.push((first, index));
        }
    }
    KWayMerge { heads, sources }
}

impl<I> Iterator for KWayMerge<I>
where
    I: Iterator,
    I::Item: Ord,
{
    type Item = I::Item;

    fn next(&mut self) -> Option<I::Item> {
        let (item, index) = self.heads.pop()?;
        if let Some(next) = self.sources[index].next() {
            self.heads.push((next, index));
        }
        Some(item)
    }
}

/// Merges sorted `Vec`s into one sorted `Vec`.
pub fn merge_k_sorted_lists<T: Ord>(lists: Vec<Vec<T>>) -> Vec<T> {
    merge_sorted(lists).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_feeds_into_one_timeline() {
        let feeds = vec![vec![1, 4, 9], vec![2, 3, 10, 11], vec![], vec![0, 4]];
        assert_eq!(
            merge_k_sorted_lists(feeds),
            vec![0, 1, 2, 3, 4, 4, 9, 10, 11]
        );
        assert!(merge_k_sorted_lists(Vec::<Vec<i32>>::new()).is_empty());
    }

    #[test]
    fn works_lazily_on_iterators() {
        let merged: Vec<i32> = merge_sorted([0..5, 3..8, 10..11]).collect();
        assert_eq!(merged, vec![0, 1, 2, 3, 3, 4, 4, 5, 6, 7, 10]);
    }

    #[test]
    fn timestamped_records() {
        let receiver_a = vec![(100, "a1"), (300, "a2")];
        let receiver_b = vec![(150, "b1"), (300, "b2"), (400, "b3")];
        let timeline: Vec<_> = merge_sorted(vec![receiver_a, receiver_b])
            .map(|(_, tag)| tag)
            .collect();
        assert_eq!(timeline, vec!["a1", "b1", "a2", "b2", "b3"]);
    }
}
