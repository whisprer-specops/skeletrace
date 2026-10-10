// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! OSINT helpers built on the general modules: what SkeleTrace gets out of them.
//!
//! | Helper | Built on | Answers |
//! |---|---|---|
//! | [`TimeBuckets`] | Fenwick tree, segment tree | How many events between t1 and t2? Busiest bucket in that window? |
//! | [`EntityClusters`] | union-find | Which identifiers belong to the same actor? |
//! | [`similar_hashes`] | bit counts | Which images are near-duplicates, by perceptual hash? |
//! | [`username_candidates`] | permutations | Which usernames should an account sweep try first? |
//! | [`silences`], [`longest_silence`] | sorting, largest gap | When did a feed go quiet? |
//! | [`burst`] | Kadane's maximum subarray | Which run of buckets stands out most above normal? |
//! | [`strongest_chain`] | Dijkstra's method | Which chain of evidence ties A to B most believably? |
//!
//! For the closest approach between sightings, see [`crate::geo::closest_pair_geo`]; for IP-to-ASN lookups,
//! [`crate::search::Ipv4RangeTable`]; for merging feeds, [`crate::merge`].

use crate::arrays::{max_gap, max_subarray, Subarray};
use crate::bits::hamming_distance;
use crate::combinatorics::unique_permutations;
use crate::dsu::DisjointSet;
use crate::fenwick::Fenwick;
use crate::graph::{dijkstra, WeightedGraph};
use crate::segtree::{Max, SegTree};
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::hash::Hash;

/// Event counts in fixed-width time buckets. Recording an event, counting events in a window and finding the
/// busiest bucket in a window each cost O(log N), however many events arrive.
pub struct TimeBuckets {
    origin_ms: i64,
    width_ms: i64,
    counts: Vec<i64>,
    totals: Fenwick,
    peaks: SegTree<Max>,
}

impl TimeBuckets {
    /// `buckets` buckets of `width_ms` milliseconds, the first starting at `origin_ms`. `None` if `width_ms` is
    /// not positive or `buckets` is 0.
    pub fn new(origin_ms: i64, width_ms: i64, buckets: usize) -> Option<Self> {
        if width_ms <= 0 || buckets == 0 {
            return None;
        }
        Some(Self {
            origin_ms,
            width_ms,
            counts: vec![0; buckets],
            totals: Fenwick::new(buckets),
            peaks: SegTree::from_slice(&vec![0; buckets]),
        })
    }

    /// Number of buckets.
    pub fn len(&self) -> usize {
        self.counts.len()
    }

    /// Always false: there is at least one bucket.
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// The bucket holding `at_ms`, if it falls inside the covered span.
    pub fn bucket_of(&self, at_ms: i64) -> Option<usize> {
        let offset = i128::from(at_ms) - i128::from(self.origin_ms);
        if offset < 0 {
            return None;
        }
        let index = usize::try_from(offset / i128::from(self.width_ms)).ok()?;
        if index < self.counts.len() {
            Some(index)
        } else {
            None
        }
    }

    /// Records `count` events at `at_ms`. Returns `false`, recording nothing, if `at_ms` is outside the span.
    pub fn record(&mut self, at_ms: i64, count: i64) -> bool {
        let Some(b) = self.bucket_of(at_ms) else {
            return false;
        };
        self.counts[b] += count;
        self.totals.add(b, count);
        self.peaks.update(b, self.counts[b]);
        true
    }

    /// The count in bucket `index`, if it exists.
    pub fn count(&self, index: usize) -> Option<i64> {
        self.counts.get(index).copied()
    }

    /// The buckets overlapping `from_ms..to_ms`, as an index range clamped to the span.
    fn bucket_range(&self, from_ms: i64, to_ms: i64) -> (usize, usize) {
        let n = self.counts.len() as i128;
        let (origin, width) = (i128::from(self.origin_ms), i128::from(self.width_ms));
        let first = (i128::from(from_ms) - origin).div_euclid(width).clamp(0, n);
        let past_last = (-(-(i128::from(to_ms) - origin)).div_euclid(width)).clamp(0, n); // ceiling division
        if past_last <= first {
            (0, 0)
        } else {
            (first as usize, past_last as usize)
        }
    }

    /// Events in the buckets overlapping `from_ms..to_ms`. Buckets are counted whole, so the answer is exact to
    /// bucket width.
    pub fn count_between(&self, from_ms: i64, to_ms: i64) -> i64 {
        let (start, end) = self.bucket_range(from_ms, to_ms);
        self.totals.range_sum(start, end)
    }

    /// The largest single-bucket count among the buckets overlapping `from_ms..to_ms`, or `None` if the window
    /// misses the span.
    pub fn busiest_between(&self, from_ms: i64, to_ms: i64) -> Option<i64> {
        let (start, end) = self.bucket_range(from_ms, to_ms);
        if start == end {
            None
        } else {
            Some(self.peaks.query(start, end))
        }
    }
}

/// Groups identifiers that evidence ties to one actor: an email address seen with a handle, a handle with a
/// phone number. Linking and asking "same actor?" are both near O(1).
#[derive(Clone, Debug)]
pub struct EntityClusters<K> {
    index: HashMap<K, usize>,
    keys: Vec<K>,
    sets: DisjointSet,
}

impl<K: Eq + Hash + Clone> Default for EntityClusters<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Eq + Hash + Clone> EntityClusters<K> {
    /// No identifiers yet.
    pub fn new() -> Self {
        Self {
            index: HashMap::new(),
            keys: Vec::new(),
            sets: DisjointSet::new(0),
        }
    }

    /// Adds an identifier on its own, if new, and returns its internal index.
    pub fn insert(&mut self, key: &K) -> usize {
        if let Some(&i) = self.index.get(key) {
            return i;
        }
        let i = self.sets.add();
        self.index.insert(key.clone(), i);
        self.keys.push(key.clone());
        i
    }

    /// Records that `a` and `b` belong to the same actor. Returns `false` if that was already known.
    pub fn link(&mut self, a: &K, b: &K) -> bool {
        let (x, y) = (self.insert(a), self.insert(b));
        self.sets.union(x, y)
    }

    /// True when the evidence so far ties `a` and `b` together; false if either is unknown.
    pub fn same_actor(&mut self, a: &K, b: &K) -> bool {
        match (self.index.get(a), self.index.get(b)) {
            (Some(&x), Some(&y)) => self.sets.same(x, y),
            _ => false,
        }
    }

    /// Number of identifiers.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// True when no identifiers have been added.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Number of separate actors.
    pub fn cluster_count(&self) -> usize {
        self.sets.set_count()
    }

    /// Every actor's identifiers, in the order first seen; actors ordered by their first-seen identifier.
    pub fn clusters(&mut self) -> Vec<Vec<K>> {
        self.sets
            .groups()
            .into_iter()
            .map(|g| g.into_iter().map(|i| self.keys[i].clone()).collect())
            .collect()
    }
}

/// Pairs of 64-bit perceptual image hashes (aHash, dHash, pHash) at most `max_distance` bits apart, as
/// `(i, j, distance)` with `i < j`: likely the same picture resized, cropped slightly or re-encoded. Compares
/// every pair, O(N²): fine for a few thousand images; for more, group hashes by some of their bits first.
pub fn similar_hashes(hashes: &[u64], max_distance: u32) -> Vec<(usize, usize, u32)> {
    let mut pairs = Vec::new();
    for (i, &a) in hashes.iter().enumerate() {
        for (j, &b) in hashes.iter().enumerate().skip(i + 1) {
            let d = hamming_distance(a, b);
            if d <= max_distance {
                pairs.push((i, j, d));
            }
        }
    }
    pairs
}

/// Likely usernames for a person, from name parts such as `["John", "Smith"]`: the patterns an account sweep
/// usually tries first. Each part alone; every ordered pair joined directly or with `.`, `_` or `-`, in full or
/// with one side cut to its initial; and with three or four parts, every ordering of all of them.
///
/// Parts are lower-cased and reduced to ASCII letters and digits, so transliterate first if needed (José to
/// Jose). Empty parts are dropped and only the first four are used. Output is sorted, without duplicates.
pub fn username_candidates(parts: &[&str]) -> Vec<String> {
    const SEPARATORS: [&str; 4] = ["", ".", "_", "-"];
    let parts: Vec<String> = parts
        .iter()
        .map(|p| {
            p.chars()
                .filter(char::is_ascii_alphanumeric)
                .map(|c| c.to_ascii_lowercase())
                .collect::<String>()
        })
        .filter(|p| !p.is_empty())
        .take(4)
        .collect();
    let mut out: BTreeSet<String> = parts.iter().cloned().collect();
    for (i, a) in parts.iter().enumerate() {
        for (j, b) in parts.iter().enumerate() {
            if i == j {
                continue;
            }
            let (initial_a, initial_b) = (&a[..1], &b[..1]); // ASCII only, so one byte is one character
            for sep in SEPARATORS {
                out.insert(format!("{a}{sep}{b}"));
                out.insert(format!("{initial_a}{sep}{b}"));
                out.insert(format!("{a}{sep}{initial_b}"));
            }
        }
    }
    if parts.len() >= 3 {
        for ordering in unique_permutations(&parts) {
            for sep in SEPARATORS {
                out.insert(ordering.join(sep));
            }
        }
    }
    out.into_iter().collect()
}

/// Quiet spells: every gap of at least `min_gap_ms` between consecutive events, as `(last event before,
/// first event after)`, in time order. `timestamps` needn't be sorted. O(N log N).
pub fn silences(timestamps: &[i64], min_gap_ms: u64) -> Vec<(i64, i64)> {
    let mut sorted = timestamps.to_vec();
    sorted.sort_unstable();
    sorted
        .windows(2)
        .filter(|w| w[1].abs_diff(w[0]) >= min_gap_ms)
        .map(|w| (w[0], w[1]))
        .collect()
}

/// Length of the longest quiet spell between events, in O(N) without sorting. `None` for fewer than two events.
pub fn longest_silence(timestamps: &[i64]) -> Option<u64> {
    max_gap(timestamps)
}

/// The run of consecutive buckets standing furthest above `baseline`: Kadane's maximum subarray on
/// `count - baseline`. Its `sum` is the total excess over the baseline. `None` for no buckets.
pub fn burst(counts: &[i64], baseline: i64) -> Option<Subarray> {
    let excess: Vec<i64> = counts.iter().map(|&c| c.saturating_sub(baseline)).collect();
    max_subarray(&excess)
}

/// The best-supported chain of links between two identifiers, from [`strongest_chain`].
#[derive(Clone, Debug, PartialEq)]
pub struct Chain<K> {
    /// The identifiers along the chain, from the first asked about to the second, both included.
    pub nodes: Vec<K>,
    /// Which links the chain uses, as positions in the list given, in order along the chain.
    pub links: Vec<usize>,
    /// The confidences of those links multiplied together: how likely the whole chain is to hold, if each
    /// link holds or fails independently of the others.
    pub confidence: f64,
}

/// Why [`strongest_chain`] refused its links.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChainError {
    /// The link at this position has a confidence that isn't a number above 0 and at most 1.
    BadConfidence(usize),
}

impl fmt::Display for ChainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChainError::BadConfidence(at) => {
                write!(f, "link {at} needs a confidence above 0 and at most 1")
            }
        }
    }
}

impl std::error::Error for ChainError {}

/// The most believable chain of links from `from` to `to`. Each link `(a, b, confidence)` is a piece of
/// evidence that `a` and `b` are connected, with a confidence above 0 and at most 1, and works both ways.
///
/// A chain is only as believable as all its links together, so its confidence is their product. The chain with
/// the highest product wins: two solid links at 0.9 each (0.81) beat one shaky link at 0.5. `Ok(None)` when the
/// two aren't connected, or either appears in no link. Asking from an identifier to itself gives a chain of
/// one, with confidence 1.
///
/// Dijkstra's method finds the route whose costs add up to the least. Here confidences multiply, but taking
/// −ln of each one turns multiplying into adding, and the largest product into the smallest sum. So each link
/// costs −ln(confidence), and the cheapest route is the strongest chain. O((V + E) log V).
///
/// **SkeleTrace use:** when the case graph's links carry confidence scores, the route between two identifiers
/// that an analyst should check first, and which pieces of evidence it rests on.
pub fn strongest_chain<K: Eq + Hash + Clone>(
    links: &[(K, K, f64)],
    from: &K,
    to: &K,
) -> Result<Option<Chain<K>>, ChainError> {
    for (at, link) in links.iter().enumerate() {
        if !(link.2 > 0.0 && link.2 <= 1.0) {
            return Err(ChainError::BadConfidence(at));
        }
    }
    if from == to {
        return Ok(Some(Chain {
            nodes: vec![from.clone()],
            links: Vec::new(),
            confidence: 1.0,
        }));
    }
    let mut index: HashMap<&K, usize> = HashMap::new();
    let mut keys: Vec<&K> = Vec::new();
    for (a, b, _) in links {
        for key in [a, b] {
            if !index.contains_key(key) {
                index.insert(key, keys.len());
                keys.push(key);
            }
        }
    }
    let (Some(&start), Some(&goal)) = (index.get(from), index.get(to)) else {
        return Ok(None);
    };
    let mut graph = WeightedGraph::undirected(keys.len());
    for (at, (a, b, confidence)) in links.iter().enumerate() {
        // 0 − ln(c) rather than −ln(c), so that c = 1 costs +0.0, not −0.0. Edge ids then match link positions.
        let cost = 0.0 - confidence.ln();
        if graph.add_edge(index[a], index[b], cost).is_err() {
            return Err(ChainError::BadConfidence(at));
        }
    }
    let Ok(result) = dijkstra(&graph, start) else {
        return Ok(None);
    };
    let (Some(path), Some(used)) = (result.path_to(goal), result.edges_to(goal)) else {
        return Ok(None);
    };
    Ok(Some(Chain {
        nodes: path.into_iter().map(|i| keys[i].clone()).collect(),
        confidence: used.iter().map(|&e| links[e].2).product(),
        links: used,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_buckets_count_and_find_peaks() {
        // 10 one-minute buckets from t = 0
        let mut tb = TimeBuckets::new(0, 60_000, 10).unwrap();
        assert!(tb.record(5_000, 1));
        assert!(tb.record(61_000, 3));
        assert!(tb.record(119_999, 2));
        assert!(tb.record(540_000, 4));
        assert!(!tb.record(600_000, 1), "past the span");
        assert!(!tb.record(-1, 1), "before the span");
        assert_eq!(tb.count(1), Some(5));
        assert_eq!(tb.count_between(0, 120_000), 6);
        assert_eq!(
            tb.count_between(60_000, 61_000),
            5,
            "a partial window counts the whole bucket"
        );
        assert_eq!(tb.count_between(-1_000_000, 1_000_000), 10);
        assert_eq!(tb.count_between(200_000, 100_000), 0);
        assert_eq!(tb.busiest_between(0, 600_000), Some(5));
        assert_eq!(tb.busiest_between(120_000, 600_000), Some(4));
        assert_eq!(tb.busiest_between(700_000, 800_000), None);
        assert_eq!(tb.bucket_of(i64::MAX), None);
        assert!(TimeBuckets::new(0, 0, 5).is_none());
        assert!(TimeBuckets::new(0, 10, 0).is_none());
        let far = TimeBuckets::new(i64::MIN, 1, 3).unwrap();
        assert_eq!(far.bucket_of(i64::MAX), None);
        assert_eq!(far.count_between(i64::MIN, i64::MAX), 0);
    }

    #[test]
    fn entity_clusters_follow_chains_of_evidence() {
        let mut ec = EntityClusters::new();
        ec.link(&"alice@example.org", &"@alice");
        ec.link(&"@alice", &"+44 7700 900123");
        ec.insert(&"carol@example.net");
        ec.link(&"@bob", &"bob@example.org");
        assert!(ec.same_actor(&"alice@example.org", &"+44 7700 900123"));
        assert!(!ec.same_actor(&"alice@example.org", &"@bob"));
        assert!(!ec.same_actor(&"alice@example.org", &"unknown"));
        assert_eq!(ec.cluster_count(), 3);
        assert_eq!(ec.len(), 6);
        assert_eq!(
            ec.clusters(),
            vec![
                vec!["alice@example.org", "@alice", "+44 7700 900123"],
                vec!["carol@example.net"],
                vec!["@bob", "bob@example.org"]
            ]
        );
        assert!(!ec.link(&"@alice", &"alice@example.org"), "already known");
    }

    #[test]
    fn similar_image_hashes() {
        let hashes = [
            0xFFFF_0000_FFFF_0000,
            0xFFFF_0000_FFFF_0001,
            0x0000_FFFF_0000_FFFF,
            0xFFFF_0000_FFFF_0003,
        ];
        assert_eq!(
            similar_hashes(&hashes, 2),
            vec![(0, 1, 1), (0, 3, 2), (1, 3, 1)]
        );
        assert!(similar_hashes(&[], 5).is_empty());
    }

    #[test]
    fn usernames() {
        let names = username_candidates(&["John", "Smith"]);
        assert_eq!(names.len(), 26);
        for expected in [
            "john",
            "smith",
            "johnsmith",
            "john.smith",
            "jsmith",
            "j.smith",
            "smithj",
            "johns",
            "smith_john",
        ] {
            assert!(
                names.binary_search(&expected.to_string()).is_ok(),
                "missing {expected}"
            );
        }
        assert!(
            names.windows(2).all(|w| w[0] < w[1]),
            "sorted, no duplicates"
        );
        let three = username_candidates(&["Mary", "Ann", "Lee"]);
        assert!(three.binary_search(&"lee.mary.ann".to_string()).is_ok());
        assert_eq!(
            username_candidates(&["", "!!", "Zoë"]),
            vec!["zo".to_string()]
        );
        assert!(username_candidates(&[]).is_empty());
    }

    #[test]
    fn silences_and_bursts() {
        let times = [1_000, 61_000, 2_000, 400_000, 3_000];
        assert_eq!(
            silences(&times, 50_000),
            vec![(3_000, 61_000), (61_000, 400_000)]
        );
        assert_eq!(longest_silence(&times), Some(339_000));
        assert_eq!(longest_silence(&[5]), None);
        let counts = [3, 2, 4, 15, 18, 12, 3, 2];
        assert_eq!(
            burst(&counts, 5),
            Some(Subarray {
                start: 3,
                end: 6,
                sum: 30
            })
        );
    }

    #[test]
    fn strongest_chains() {
        let links = [
            ("a@example.org", "@a", 0.9),
            ("@a", "+44 7700 900123", 0.9),
            ("a@example.org", "+44 7700 900123", 0.5),
            ("@lonely", "@other", 1.0),
        ];
        let chain = strongest_chain(&links, &"a@example.org", &"+44 7700 900123")
            .unwrap()
            .unwrap();
        assert_eq!(chain.nodes, vec!["a@example.org", "@a", "+44 7700 900123"]);
        assert_eq!(chain.links, vec![0, 1]);
        assert!((chain.confidence - 0.81).abs() < 1e-12);
        assert_eq!(
            strongest_chain(&links, &"a@example.org", &"@other").unwrap(),
            None
        );
        assert_eq!(
            strongest_chain(&links, &"a@example.org", &"@nobody").unwrap(),
            None
        );
        let alone = strongest_chain(&links, &"@nobody", &"@nobody")
            .unwrap()
            .unwrap();
        assert_eq!(
            (alone.nodes, alone.links, alone.confidence),
            (vec!["@nobody"], vec![], 1.0)
        );
        for (bad, at) in [(0.0, 1), (1.5, 1), (f64::NAN, 1), (-0.2, 1)] {
            let broken = [("x", "y", 0.5), ("y", "z", bad)];
            assert_eq!(
                strongest_chain(&broken, &"x", &"z"),
                Err(ChainError::BadConfidence(at))
            );
        }
    }

    /// The best product over every simple path, by trying them all.
    fn best_product(links: &[(u8, u8, f64)], at: u8, goal: u8, used: &mut Vec<u8>) -> Option<f64> {
        if at == goal {
            return Some(1.0);
        }
        used.push(at);
        let mut best: Option<f64> = None;
        for &(a, b, c) in links {
            for (x, y) in [(a, b), (b, a)] {
                if x == at && !used.contains(&y) {
                    if let Some(rest) = best_product(links, y, goal, used) {
                        let p = c * rest;
                        if best.map_or(true, |q| p > q) {
                            best = Some(p);
                        }
                    }
                }
            }
        }
        used.pop();
        best
    }

    #[test]
    fn strongest_chains_match_trying_every_route() {
        let mut rng = crate::testutil::Lcg(109);
        let levels = [0.2, 0.5, 0.6, 0.75, 0.9, 0.95, 1.0];
        for _ in 0..300 {
            let nodes = 2 + rng.below(6) as u8;
            let links: Vec<(u8, u8, f64)> = (0..rng.below(12))
                .map(|_| {
                    (
                        rng.below(u64::from(nodes)) as u8,
                        rng.below(u64::from(nodes)) as u8,
                        levels[rng.below(levels.len() as u64) as usize],
                    )
                })
                .collect();
            let (from, to) = (
                rng.below(u64::from(nodes)) as u8,
                rng.below(u64::from(nodes)) as u8,
            );
            let known =
                |k: u8| k == from && from == to || links.iter().any(|l| l.0 == k || l.1 == k);
            let want = if known(from) && known(to) {
                best_product(&links, from, to, &mut Vec::new())
            } else {
                None
            };
            let got = strongest_chain(&links, &from, &to).unwrap();
            match (got, want) {
                (None, None) => {}
                (Some(chain), Some(p)) => {
                    assert!(
                        (chain.confidence - p).abs() < 1e-12,
                        "{links:?} {from}→{to}"
                    );
                    assert_eq!(chain.nodes.first(), Some(&from));
                    assert_eq!(chain.nodes.last(), Some(&to));
                    let mut product = 1.0;
                    for (k, &l) in chain.links.iter().enumerate() {
                        let (a, b, c) = links[l];
                        let (x, y) = (chain.nodes[k], chain.nodes[k + 1]);
                        assert!((a, b) == (x, y) || (b, a) == (x, y));
                        product *= c;
                    }
                    assert_eq!(product, chain.confidence);
                }
                other => panic!("{links:?} {from}→{to}: {other:?}"),
            }
        }
    }
}
