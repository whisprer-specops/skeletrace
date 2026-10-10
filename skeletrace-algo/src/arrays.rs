// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Array problems that come up in investigations: shared items, nested intervals, gaps, standout runs,
//! duplicates, anagrams.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::Hash;
use std::ops::Range;

/// Items present in both slices, in any order ("Array Intersection", CS Academy): a value found x times in `a`
/// and y times in `b` comes out min(x, y) times, in the order it appears in `a`. Count `b` in a hash map, then
/// walk `a`: O(N + M).
///
/// With no memory to spare, sort both slices in place (`sort_unstable` doesn't allocate) and use
/// [`intersect_sorted`]: O(1) extra memory besides the output, at O(N log N + M log M) time.
///
/// **SkeleTrace use:** infrastructure two domains share, straight from unsorted lookup results.
pub fn intersect_counted<T: Eq + Hash + Clone>(a: &[T], b: &[T]) -> Vec<T> {
    let mut unmatched: HashMap<&T, usize> = HashMap::with_capacity(b.len());
    for item in b {
        *unmatched.entry(item).or_insert(0) += 1;
    }
    let mut out = Vec::new();
    for item in a {
        if let Some(left) = unmatched.get_mut(item) {
            if *left > 0 {
                *left -= 1;
                out.push(item.clone());
            }
        }
    }
    out
}

/// Items present in both sorted slices, as a sorted `Vec`; an item present twice in each appears twice. Two
/// pointers: O(N + M), with no extra memory besides the output.
///
/// **SkeleTrace use:** infrastructure two domains share (IP addresses, name servers, certificate hashes).
pub fn intersect_sorted<T: Ord + Clone>(a: &[T], b: &[T]) -> Vec<T> {
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(a[i].clone());
                i += 1;
                j += 1;
            }
        }
    }
    out
}

/// The distinct items found in both slices, in the order they first appear in `a`. Hash set: O(N + M).
pub fn intersect_unique<T: Eq + Hash + Clone>(a: &[T], b: &[T]) -> Vec<T> {
    let in_b: HashSet<&T> = b.iter().collect();
    let mut emitted: HashSet<&T> = HashSet::new();
    a.iter()
        .filter(|x| in_b.contains(x) && emitted.insert(*x))
        .cloned()
        .collect()
}

/// Indices, ascending, of the intervals that lie inside another interval of the list ("contained intervals"):
/// `j` contains `i` when `start_j <= start_i` and `end_i <= end_j`. Identical intervals contain each other.
/// Ends are inclusive; a reversed pair is read as if swapped. Sort, then sweep: O(N log N).
///
/// **SkeleTrace use:** address blocks nested inside larger blocks, sessions inside a campaign's window.
pub fn contained_intervals(intervals: &[(i64, i64)]) -> Vec<usize> {
    let normal = |i: usize| {
        let (a, b) = intervals[i];
        if a <= b {
            (a, b)
        } else {
            (b, a)
        }
    };
    // Starts ascending, and for equal starts the longest first: everything that could contain an interval
    // then comes before it.
    let mut order: Vec<usize> = (0..intervals.len()).collect();
    order.sort_by(|&x, &y| {
        let ((sx, ex), (sy, ey)) = (normal(x), normal(y));
        sx.cmp(&sy).then(ey.cmp(&ex))
    });
    let mut contained = vec![false; intervals.len()];
    for pair in order.windows(2) {
        if normal(pair[0]) == normal(pair[1]) {
            contained[pair[0]] = true;
            contained[pair[1]] = true;
        }
    }
    let mut furthest_end: Option<i64> = None;
    for &i in &order {
        let (_, end) = normal(i);
        if furthest_end.is_some_and(|f| end <= f) {
            contained[i] = true;
        }
        furthest_end = Some(furthest_end.map_or(end, |f| f.max(end)));
    }
    (0..intervals.len()).filter(|&i| contained[i]).collect()
}

/// The largest difference between neighbours once the values are sorted ("consecutive max difference"),
/// without sorting: O(N), with buckets. `None` for fewer than two values.
///
/// With N values spread over a range R, the largest gap is at least R / (N − 1). Buckets narrower than that can
/// never hold the largest gap inside one bucket, so only each bucket's minimum and maximum matter.
///
/// **SkeleTrace use:** the longest silence in a feed, from its event times.
pub fn max_gap(values: &[i64]) -> Option<u64> {
    if values.len() < 2 {
        return None;
    }
    let (mut low, mut high) = (values[0], values[0]);
    for &v in values {
        low = low.min(v);
        high = high.max(v);
    }
    let range = high.abs_diff(low);
    if range == 0 {
        return Some(0);
    }
    let slots = (values.len() - 1) as u64;
    let bucket_width = (range - 1) / slots + 1; // ⌈range / slots⌉, at least 1
    let buckets = (range / bucket_width) as usize + 1;
    let mut bounds: Vec<Option<(i64, i64)>> = vec![None; buckets];
    for &v in values {
        let b = (v.abs_diff(low) / bucket_width) as usize;
        bounds[b] = Some(match bounds[b] {
            None => (v, v),
            Some((lo, hi)) => (lo.min(v), hi.max(v)),
        });
    }
    let mut best = 0;
    let mut previous_high = low;
    for (lo, hi) in bounds.into_iter().flatten() {
        best = best.max(lo.abs_diff(previous_high));
        previous_high = hi;
    }
    Some(best)
}

/// A contiguous stretch `start..end` of a slice, and the sum of its values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subarray {
    /// First index.
    pub start: usize,
    /// One past the last index.
    pub end: usize,
    /// Sum of the values (128 bits, so long runs of large values can't overflow).
    pub sum: i128,
}

/// The contiguous stretch with the largest sum, by Kadane's algorithm: carry a running sum, and start afresh
/// whenever it has gone negative, because a negative prefix can only drag the total down. O(N). `None` for an
/// empty slice; the earliest stretch on ties.
///
/// **SkeleTrace use:** the burst that stands out most; see [`crate::osint::burst`].
pub fn max_subarray(values: &[i64]) -> Option<Subarray> {
    let first = i128::from(*values.first()?);
    let mut best = Subarray {
        start: 0,
        end: 1,
        sum: first,
    };
    let (mut run_start, mut run_sum) = (0, first);
    for (i, &v) in values.iter().enumerate().skip(1) {
        let v = i128::from(v);
        if run_sum < 0 {
            run_start = i;
            run_sum = v;
        } else {
            run_sum += v;
        }
        if run_sum > best.sum {
            best = Subarray {
                start: run_start,
                end: i + 1,
                sum: run_sum,
            };
        }
    }
    Some(best)
}

/// The contiguous stretch with the smallest sum: Kadane's algorithm with the comparisons flipped.
pub fn min_subarray(values: &[i64]) -> Option<Subarray> {
    let first = i128::from(*values.first()?);
    let mut best = Subarray {
        start: 0,
        end: 1,
        sum: first,
    };
    let (mut run_start, mut run_sum) = (0, first);
    for (i, &v) in values.iter().enumerate().skip(1) {
        let v = i128::from(v);
        if run_sum > 0 {
            run_start = i;
            run_sum = v;
        } else {
            run_sum += v;
        }
        if run_sum < best.sum {
            best = Subarray {
                start: run_start,
                end: i + 1,
                sum: run_sum,
            };
        }
    }
    Some(best)
}

/// The shortest stretch `start..end` that holds at least one copy of the smallest value and one copy of the
/// largest ("Min Max Subarray", CS Academy). `None` for an empty slice; the earliest stretch on ties. When all
/// the values are equal, one value is both, so the answer is `0..1`.
///
/// The best stretch ends on a lowest or a highest value. Ending at position i, it starts at whichever of the
/// two kinds was seen less recently, so one pass that remembers where each kind last appeared finds it: O(N).
///
/// **SkeleTrace use:** the quickest full swing in a series, from its lowest reading to its highest or back.
pub fn shortest_min_max_window<T: Ord>(values: &[T]) -> Option<Range<usize>> {
    let lowest = values.iter().min()?;
    let highest = values.iter().max()?;
    let (mut last_low, mut last_high): (Option<usize>, Option<usize>) = (None, None);
    let mut best: Option<Range<usize>> = None;
    for (i, value) in values.iter().enumerate() {
        let is_low = value == lowest;
        let is_high = value == highest;
        if is_low {
            last_low = Some(i);
        }
        if is_high {
            last_high = Some(i);
        }
        if !(is_low || is_high) {
            continue;
        }
        if let (Some(low), Some(high)) = (last_low, last_high) {
            let start = low.min(high);
            if best.as_ref().map_or(true, |b| i + 1 - start < b.len()) {
                best = Some(start..i + 1);
            }
        }
    }
    best
}

/// The smallest non-negative integer missing from `values` ("array MEX"). Only values up to the length can
/// matter, so one pass with a table that size: O(N).
pub fn mex(values: &[u64]) -> u64 {
    let len = values.len();
    let mut seen = vec![false; len + 1];
    for &v in values {
        if let Ok(i) = usize::try_from(v) {
            if i <= len {
                seen[i] = true;
            }
        }
    }
    seen.iter()
        .position(|&s| !s)
        .map_or(len as u64, |i| i as u64)
}

/// Some value that appears more than once ("find any duplicate"), or `None` if all are distinct. Hash set:
/// O(N).
pub fn find_any_duplicate<T: Eq + Hash>(values: &[T]) -> Option<&T> {
    let mut seen = HashSet::with_capacity(values.len());
    values.iter().find(|v| !seen.insert(*v))
}

/// The repeated value in a slice of `n + 1` values that all lie in `1..=n`, so one must repeat, using O(1) extra
/// memory: read the slice as a linked list (`i` points to `values[i]`), where the repeat closes a cycle, and
/// find the cycle's start with Floyd's tortoise and hare. `None` if the input breaks those rules.
pub fn find_duplicate_floyd(values: &[usize]) -> Option<usize> {
    let n = values.len().checked_sub(1)?;
    if n == 0 || values.iter().any(|&v| v == 0 || v > n) {
        return None;
    }
    let (mut tortoise, mut hare) = (values[0], values[values[0]]);
    while tortoise != hare {
        tortoise = values[tortoise];
        hare = values[values[hare]];
    }
    tortoise = 0;
    while tortoise != hare {
        tortoise = values[tortoise];
        hare = values[hare];
    }
    Some(hare)
}

/// How many pairs `i < j` have `values[i] + values[j]` divisible by 3 ("3-divisible pairs"). Count remainders
/// 0, 1 and 2: pairs come from two remainder-0 values, or a remainder-1 with a remainder-2. O(N).
pub fn count_pairs_divisible_by_three(values: &[i64]) -> u128 {
    let mut by_remainder = [0u128; 3];
    for &v in values {
        by_remainder[v.rem_euclid(3) as usize] += 1;
    }
    let [zero, one, two] = by_remainder;
    zero * zero.saturating_sub(1) / 2 + one * two
}

/// True when `a` and `b` use the same characters the same number of times ("anagrams"). O(N).
pub fn is_anagram(a: &str, b: &str) -> bool {
    let mut counts: HashMap<char, i64> = HashMap::new();
    for c in a.chars() {
        *counts.entry(c).or_insert(0) += 1;
    }
    for c in b.chars() {
        *counts.entry(c).or_insert(0) -= 1;
    }
    counts.values().all(|&n| n == 0)
}

/// Words grouped by anagram (same letters, any order), groups in the order of their first word.
///
/// **SkeleTrace use:** spotting handles that reuse the same letters.
pub fn group_anagrams<'a>(words: &[&'a str]) -> Vec<Vec<&'a str>> {
    let mut slot: HashMap<Vec<char>, usize> = HashMap::new();
    let mut groups: Vec<Vec<&'a str>> = Vec::new();
    for &word in words {
        let mut key: Vec<char> = word.chars().collect();
        key.sort_unstable();
        let index = *slot.entry(key).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[index].push(word);
    }
    groups
}

/// How many words are in the largest set of anagrams of each other ("Anagrams", CS Academy); 0 for no words.
/// Each word's characters, sorted, are its key in a hash map of counts: O(L log L) for a word of length L.
pub fn largest_anagram_group(words: &[&str]) -> usize {
    let mut counts: HashMap<Vec<char>, usize> = HashMap::with_capacity(words.len());
    let mut largest = 0;
    for word in words {
        let mut key: Vec<char> = word.chars().collect();
        key.sort_unstable();
        let count = counts.entry(key).or_insert(0);
        *count += 1;
        largest = largest.max(*count);
    }
    largest
}

/// The alphabetically first palindrome that uses every character of `text` exactly once ("palindrome
/// creation"), or `None` when more than one character occurs an odd number of times. O(N log N).
pub fn palindrome_from_letters(text: &str) -> Option<String> {
    let mut counts: BTreeMap<char, usize> = BTreeMap::new();
    for c in text.chars() {
        *counts.entry(c).or_insert(0) += 1;
    }
    let mut middle: Option<char> = None;
    for (&c, &n) in &counts {
        if n % 2 == 1 {
            if middle.is_some() {
                return None;
            }
            middle = Some(c);
        }
    }
    let half: String = counts
        .iter()
        .flat_map(|(&c, &n)| std::iter::repeat(c).take(n / 2))
        .collect();
    let mut out = half.clone();
    out.extend(middle);
    out.extend(half.chars().rev());
    Some(out)
}

/// Run-length encoding: each run of equal neighbours as (value, run length). One look-and-say step is this.
///
/// **SkeleTrace use:** compact storage for long on/off activity series.
pub fn run_lengths<T: PartialEq + Clone>(items: &[T]) -> Vec<(T, usize)> {
    let mut runs: Vec<(T, usize)> = Vec::new();
    for item in items {
        if let Some((value, count)) = runs.last_mut() {
            if *value == *item {
                *count += 1;
                continue;
            }
        }
        runs.push((item.clone(), 1));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Lcg;

    #[test]
    fn intersections() {
        assert_eq!(
            intersect_sorted(&[1, 2, 2, 3, 5], &[2, 2, 2, 5, 7]),
            vec![2, 2, 5]
        );
        assert_eq!(intersect_unique(&[5, 1, 5, 3, 2], &[2, 5, 9]), vec![5, 2]);
        assert!(intersect_sorted::<i32>(&[], &[1]).is_empty());
    }

    #[test]
    fn counted_intersection_keeps_the_smaller_count() {
        assert_eq!(
            intersect_counted(&[5, 1, 5, 3, 5, 2], &[5, 2, 5, 9, 2]),
            vec![5, 5, 2]
        );
        assert!(intersect_counted::<u8>(&[], &[1]).is_empty());
        let mut rng = Lcg(53);
        for _ in 0..300 {
            let a: Vec<u8> = (0..rng.below(20)).map(|_| rng.below(6) as u8).collect();
            let b: Vec<u8> = (0..rng.below(20)).map(|_| rng.below(6) as u8).collect();
            let got = intersect_counted(&a, &b);
            // Every value min(x, y) times.
            let count = |v: &[u8], x: u8| v.iter().filter(|&&y| y == x).count();
            for x in 0..6 {
                assert_eq!(
                    count(&got, x),
                    count(&a, x).min(count(&b, x)),
                    "{a:?} {b:?}"
                );
            }
            // In the order of `a`: the result is `a` with some items left out.
            let mut rest = a.iter();
            assert!(got.iter().all(|x| rest.any(|y| y == x)), "{a:?} {b:?}");
            // The same items as sorting both and walking them together.
            let (mut sa, mut sb, mut sg) = (a.clone(), b.clone(), got.clone());
            sa.sort_unstable();
            sb.sort_unstable();
            sg.sort_unstable();
            assert_eq!(sg, intersect_sorted(&sa, &sb));
        }
    }

    /// Checks every stretch, shortest first, then leftmost first.
    fn min_max_window_by_trying(values: &[i64]) -> Option<Range<usize>> {
        let lowest = *values.iter().min()?;
        let highest = *values.iter().max()?;
        for len in 1..=values.len() {
            for start in 0..=values.len() - len {
                let stretch = &values[start..start + len];
                if stretch.contains(&lowest) && stretch.contains(&highest) {
                    return Some(start..start + len);
                }
            }
        }
        None
    }

    #[test]
    fn shortest_min_max_windows() {
        assert_eq!(
            shortest_min_max_window(&[3, 1, 4, 4, 9, 2, 1, 9]),
            Some(6..8)
        );
        assert_eq!(shortest_min_max_window(&[7, 7, 7]), Some(0..1));
        assert_eq!(shortest_min_max_window(&[5]), Some(0..1));
        assert_eq!(shortest_min_max_window::<u8>(&[]), None);
        let mut rng = Lcg(57);
        for _ in 0..500 {
            let values: Vec<i64> = (0..1 + rng.below(30))
                .map(|_| rng.below(5) as i64 - 2)
                .collect();
            assert_eq!(
                shortest_min_max_window(&values),
                min_max_window_by_trying(&values),
                "{values:?}"
            );
        }
    }

    #[test]
    fn largest_anagram_groups() {
        assert_eq!(
            largest_anagram_group(&["tea", "eat", "tan", "ate", "nat", "bat"]),
            3
        );
        assert_eq!(largest_anagram_group(&[]), 0);
        assert_eq!(largest_anagram_group(&["moth", "thom", "moth"]), 3);
        let mut rng = Lcg(59);
        for _ in 0..200 {
            let words: Vec<String> = (0..rng.below(12))
                .map(|_| {
                    (0..rng.below(4))
                        .map(|_| char::from(b'a' + rng.below(3) as u8))
                        .collect()
                })
                .collect();
            let words: Vec<&str> = words.iter().map(String::as_str).collect();
            let from_groups = group_anagrams(&words)
                .iter()
                .map(Vec::len)
                .max()
                .unwrap_or(0);
            assert_eq!(largest_anagram_group(&words), from_groups, "{words:?}");
        }
    }

    #[test]
    fn nested_intervals() {
        let iv = [
            (1, 10),
            (2, 5),
            (3, 12),
            (6, 9),
            (11, 11),
            (20, 15),
            (15, 20),
        ];
        assert_eq!(contained_intervals(&iv), vec![1, 3, 4, 5, 6]);
        assert!(contained_intervals(&[(1, 2), (3, 4)]).is_empty());
        assert!(contained_intervals(&[]).is_empty());
    }

    #[test]
    fn gaps_match_sorting() {
        let mut rng = Lcg(51);
        for len in 2..60 {
            let values: Vec<i64> = (0..len).map(|_| rng.below(1000) as i64 - 500).collect();
            let mut sorted = values.clone();
            sorted.sort_unstable();
            let brute = sorted.windows(2).map(|w| w[1].abs_diff(w[0])).max();
            assert_eq!(max_gap(&values), brute);
        }
        assert_eq!(max_gap(&[i64::MIN, i64::MAX]), Some(u64::MAX));
        assert_eq!(max_gap(&[7, 7, 7]), Some(0));
        assert_eq!(max_gap(&[1]), None);
    }

    #[test]
    fn kadane() {
        let v = [-2, 1, -3, 4, -1, 2, 1, -5, 4];
        assert_eq!(
            max_subarray(&v),
            Some(Subarray {
                start: 3,
                end: 7,
                sum: 6
            })
        );
        assert_eq!(
            max_subarray(&[-3, -1, -2]),
            Some(Subarray {
                start: 1,
                end: 2,
                sum: -1
            })
        );
        assert_eq!(
            min_subarray(&[3, -4, 2, -3, -1, 7, -5]),
            Some(Subarray {
                start: 1,
                end: 5,
                sum: -6
            })
        );
        assert_eq!(max_subarray(&[]), None);
        assert_eq!(
            max_subarray(&[i64::MAX, i64::MAX]).map(|s| s.sum),
            Some(2 * i128::from(i64::MAX))
        );
    }

    #[test]
    fn mex_and_duplicates() {
        assert_eq!(mex(&[0, 1, 3]), 2);
        assert_eq!(mex(&[1, 2]), 0);
        assert_eq!(mex(&[]), 0);
        assert_eq!(mex(&[2, 0, 1]), 3);
        assert_eq!(mex(&[u64::MAX]), 0);
        assert_eq!(find_any_duplicate(&["a", "b", "a"]), Some(&"a"));
        assert_eq!(find_any_duplicate(&[1, 2, 3]), None);
        assert_eq!(find_duplicate_floyd(&[1, 3, 4, 2, 2]), Some(2));
        assert_eq!(find_duplicate_floyd(&[3, 1, 3, 4, 2]), Some(3));
        assert_eq!(find_duplicate_floyd(&[1, 1]), Some(1));
        assert_eq!(find_duplicate_floyd(&[0, 1]), None);
        assert_eq!(find_duplicate_floyd(&[5, 1, 2]), None);
    }

    #[test]
    fn pairs_divisible_by_three() {
        assert_eq!(count_pairs_divisible_by_three(&[1, 2, 3, 4, 5, 6]), 5);
        assert_eq!(count_pairs_divisible_by_three(&[-1, 1, 3, -3]), 2);
        let mut rng = Lcg(61);
        let v: Vec<i64> = (0..80).map(|_| rng.below(100) as i64 - 50).collect();
        let mut brute = 0;
        for i in 0..v.len() {
            for j in i + 1..v.len() {
                if (v[i] + v[j]).rem_euclid(3) == 0 {
                    brute += 1;
                }
            }
        }
        assert_eq!(count_pairs_divisible_by_three(&v), brute);
    }

    #[test]
    fn anagrams_and_palindromes() {
        assert!(is_anagram("listen", "silent"));
        assert!(!is_anagram("listen", "silence"));
        assert_eq!(
            group_anagrams(&["tea", "eat", "tan", "ate", "nat", "bat"]),
            vec![vec!["tea", "eat", "ate"], vec!["tan", "nat"], vec!["bat"]]
        );
        assert_eq!(palindrome_from_letters("aabbc").as_deref(), Some("abcba"));
        assert_eq!(palindrome_from_letters("abc"), None);
        assert_eq!(palindrome_from_letters("").as_deref(), Some(""));
        assert_eq!(
            run_lengths(&[1, 1, 2, 2, 2, 1]),
            vec![(1, 2), (2, 3), (1, 1)]
        );
        assert!(run_lengths::<u8>(&[]).is_empty());
    }
}
