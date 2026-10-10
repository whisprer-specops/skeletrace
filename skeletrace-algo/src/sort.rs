// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Sorting: three simple O(N²) sorts to learn from, merge sort in O(N log N), and two special cases that beat
//! O(N log N) when you know something about the data.
//!
//! | Function | Time | Extra memory | Stable |
//! |---|---|---|---|
//! | [`selection_sort`] | O(N²) always | O(1) | no |
//! | [`insertion_sort`] | O(N²), but O(N) on nearly sorted data | O(1) | yes |
//! | [`bubble_sort`] | O(N²), but O(N) if already sorted | O(1) | yes |
//! | [`merge_sort`] | O(N log N) always | O(N) | yes |
//! | [`sort_bounded_offset`] | O(N log k) | O(k) | no |
//! | [`binary_stable_sort`] | O(N) | O(N) | yes |
//!
//! "Stable" means items that compare equal keep their original order. Sort observations by source, then
//! stable-sort them by time, and observations with the same time stay grouped by source.
//!
//! Why merge sort is O(N log N): halving the input until pieces hold one item takes log2 N levels, and merging
//! all the pieces on one level touches every item once, N steps per level.
//!
//! In production code `slice::sort` (stable) and `slice::sort_unstable` are the right defaults. These versions
//! exist to show how each method works.

#![allow(clippy::needless_range_loop)] // index loops read more clearly than iterator chains here

use crate::heap::MinHeap;
use std::cmp::Ordering;

/// Selection sort: find the smallest remaining item, swap it to the front, repeat. Always about N²/2
/// comparisons, but at most N − 1 swaps, which matters when moving items is expensive.
pub fn selection_sort<T: Ord>(items: &mut [T]) {
    let len = items.len();
    for i in 0..len {
        let mut smallest = i;
        for j in i + 1..len {
            if items[j] < items[smallest] {
                smallest = j;
            }
        }
        items.swap(i, smallest);
    }
}

/// Insertion sort: grow a sorted prefix by sliding each new item left into place. Fast on nearly sorted data,
/// where each item only moves a little.
pub fn insertion_sort<T: Ord>(items: &mut [T]) {
    for i in 1..items.len() {
        let mut j = i;
        while j > 0 && items[j - 1] > items[j] {
            items.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// Bubble sort: sweep left to right, swapping neighbours that are out of order. Everything after a sweep's last
/// swap is already in place, so the next sweep stops there; a sweep with no swaps ends the sort.
pub fn bubble_sort<T: Ord>(items: &mut [T]) {
    let mut unsorted = items.len();
    while unsorted > 1 {
        let mut last_swap = 0;
        for i in 1..unsorted {
            if items[i - 1] > items[i] {
                items.swap(i - 1, i);
                last_swap = i;
            }
        }
        unsorted = last_swap;
    }
}

/// Merge sort: sort each half, then merge the two sorted halves. Stable, and O(N log N) in every case.
pub fn merge_sort<T: Ord + Clone>(items: &mut [T]) {
    merge_sort_by(items, T::cmp);
}

/// Merge sort with your own comparison, for example `|a, b| a.time.cmp(&b.time)`. Stable.
pub fn merge_sort_by<T: Clone>(items: &mut [T], mut compare: impl FnMut(&T, &T) -> Ordering) {
    sort_range(items, &mut compare);
}

fn sort_range<T: Clone, F: FnMut(&T, &T) -> Ordering>(items: &mut [T], compare: &mut F) {
    let len = items.len();
    if len <= 1 {
        return;
    }
    let mid = len / 2;
    sort_range(&mut items[..mid], compare);
    sort_range(&mut items[mid..], compare);
    if compare(&items[mid - 1], &items[mid]) != Ordering::Greater {
        return; // the halves are already in order
    }
    // Copy the left half out, then refill `items` from the front with the smaller head each time. Writing
    // never overtakes reading: the write position always stays behind the head of the right half.
    let left: Vec<T> = items[..mid].to_vec();
    let (mut i, mut j, mut write) = (0, mid, 0);
    while i < left.len() && j < len {
        // Take from the right only when strictly smaller, so equal items keep their order: that is stability.
        if compare(&items[j], &left[i]) == Ordering::Less {
            items[write] = items[j].clone();
            j += 1;
        } else {
            items[write] = left[i].clone();
            i += 1;
        }
        write += 1;
    }
    while i < left.len() {
        items[write] = left[i].clone();
        i += 1;
        write += 1;
    }
}

/// Sorts data in which every item is at most `k` places from where it belongs ("bounded offset"), in
/// O(N log k) instead of O(N log N): a min-heap holding the next `k + 1` items always contains the next
/// smallest one.
///
/// **SkeleTrace use:** telemetry that arrives nearly in order, such as ADS-B or AIS messages a few places late.
///
/// If some item is further than `k` places out, the result still holds every item exactly once, but not
/// necessarily in order. Not stable.
pub fn sort_bounded_offset<T: Ord>(items: Vec<T>, k: usize) -> Vec<T> {
    let mut window = MinHeap::with_capacity(k.saturating_add(1).min(items.len()));
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        window.push(item);
        if window.len() > k {
            if let Some(smallest) = window.pop() {
                out.push(smallest);
            }
        }
    }
    while let Some(smallest) = window.pop() {
        out.push(smallest);
    }
    out
}

/// Stable sort by a yes/no key ("binary stable sort"): every item whose key is `false`, in original order,
/// then every item whose key is `true`, in original order. O(N).
pub fn binary_stable_sort<T>(items: Vec<T>, mut key: impl FnMut(&T) -> bool) -> Vec<T> {
    let (mut falses, trues): (Vec<T>, Vec<T>) = items.into_iter().partition(|item| !key(item));
    falses.extend(trues);
    falses
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Lcg;

    fn random_vec(seed: u64, len: usize) -> Vec<u64> {
        let mut rng = Lcg(seed);
        (0..len).map(|_| rng.below(50)).collect()
    }

    #[test]
    fn every_sort_matches_std() {
        let sorters: [fn(&mut [u64]); 4] =
            [selection_sort, insertion_sort, bubble_sort, merge_sort];
        for len in [0, 1, 2, 3, 10, 101, 500] {
            let original = random_vec(len as u64 + 1, len);
            let mut expected = original.clone();
            expected.sort();
            for sorter in sorters {
                let mut v = original.clone();
                sorter(&mut v);
                assert_eq!(v, expected, "length {len}");
            }
        }
    }

    #[test]
    fn merge_sort_is_stable() {
        let mut records: Vec<(u32, usize)> = random_vec(9, 300)
            .into_iter()
            .enumerate()
            .map(|(i, k)| ((k % 5) as u32, i))
            .collect();
        merge_sort_by(&mut records, |a, b| a.0.cmp(&b.0));
        for pair in records.windows(2) {
            assert!(pair[0].0 < pair[1].0 || (pair[0].0 == pair[1].0 && pair[0].1 < pair[1].1));
        }
    }

    #[test]
    fn bounded_offset_sorts_nearly_sorted_data() {
        let k = 4;
        let sorted: Vec<u32> = (0..500).collect();
        let mut nearly = sorted.clone();
        let mut rng = Lcg(3);
        // Shuffle inside blocks of k + 1, so nothing moves more than k places.
        for block in nearly.chunks_mut(k + 1) {
            for i in (1..block.len()).rev() {
                let j = rng.below(i as u64 + 1) as usize;
                block.swap(i, j);
            }
        }
        assert_ne!(nearly, sorted);
        assert_eq!(sort_bounded_offset(nearly, k), sorted);
        assert_eq!(sort_bounded_offset(Vec::<u32>::new(), 3), Vec::<u32>::new());
        assert_eq!(
            sort_bounded_offset(vec![3, 1, 2], 0),
            vec![3, 1, 2],
            "k = 0 trusts the order given"
        );
        assert_eq!(
            sort_bounded_offset(vec![3, 1, 2], usize::MAX),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn binary_stable_sort_keeps_order_within_groups() {
        let words = vec!["b1", "a1", "b2", "a2", "a3"];
        assert_eq!(
            binary_stable_sort(words, |w| w.starts_with('b')),
            vec!["a1", "a2", "a3", "b1", "b2"]
        );
    }
}
