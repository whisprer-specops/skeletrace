// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Fuzzy matching for names and handles: how many typing edits apart two strings are, and how alike they look.
//!
//! | Function | Answers | Cost |
//! |---|---|---|
//! | [`levenshtein`] | Fewest one-character insertions, deletions and substitutions turning one string into the other | O(N × M) time, O(min(N, M)) memory |
//! | [`osa_distance`] | The same, with swapping two neighbouring characters counted as one edit | O(N × M) |
//! | [`levenshtein_within`], [`osa_within`] | Is it at most k edits, and if so, how many? | O(N × k) |
//! | [`jaro`], [`jaro_winkler`] | A likeness score from 0 to 1; Jaro-Winkler rewards a shared start | O(N × M) at worst |
//! | [`handle_key`] | A handle with case, punctuation and spacing stripped | O(N) |
//! | [`close_handles`] | Which handles in a long list are within k edits of this one? | O(list × N × k) |
//! | [`NearSet`] | Which of many terms, each with its own allowance of typos, is this word near? | about 37 lookups per word, then O(N × k) per candidate |
//!
//! Everything compares Unicode characters (`char`s). A letter with an accent typed as one character, and the same
//! letter typed as a base letter plus a combining accent, count as different. Normalise the text first if that
//! matters.
//!
//! **SkeleTrace use:** matching handles and names across platforms despite typos, swapped letters and
//! punctuation, so "J.Doe_88", "jdoe88" and "jdeo88" get looked at together.

use std::collections::HashMap;
use std::fmt;
use std::hash::{BuildHasherDefault, Hasher};

fn chars(text: &str) -> Vec<char> {
    text.chars().collect()
}

/// Edit distance between `a` and `b` if it is at most `max`, else `None`. `swaps` also counts swapping two
/// neighbouring characters as one edit, as long as no part of the text is edited twice ("optimal string
/// alignment").
///
/// Dynamic programming over a band: cell (i, j) holds the distance between the first i characters of `a` and
/// the first j of `b`. Every insertion or deletion moves one step off the diagonal, so an answer of `max` or
/// less never leaves the cells with |i − j| ≤ `max`. Anything outside reads as "too far" (`max + 1`).
fn banded(a: &[char], b: &[char], max: usize, swaps: bool) -> Option<usize> {
    // Both distances are symmetric, so keep the rows as short as possible.
    let (a, b) = if b.len() > a.len() { (b, a) } else { (a, b) };
    let (n, m) = (a.len(), b.len());
    if n - m > max {
        return None;
    }
    let max = max.min(n); // no distance is more than the longer length
    let over = max + 1; // stands for "more than max"
    let mut two_up = vec![over; m + 1]; // row i − 2
    let mut up: Vec<usize> = (0..=m).map(|j| j.min(over)).collect(); // row i − 1, starting as row 0
    let mut row = vec![over; m + 1];
    for i in 1..=n {
        let lo = i.saturating_sub(max).max(1);
        let hi = (i + max).min(m);
        row[0] = i.min(over);
        if lo > 1 {
            row[lo - 1] = over;
        }
        let mut best = row[0];
        for j in lo..=hi {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut d = (up[j - 1] + cost).min(up[j] + 1).min(row[j - 1] + 1);
            if swaps && i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d = d.min(two_up[j - 2] + 1);
            }
            row[j] = d.min(over);
            best = best.min(row[j]);
        }
        if hi < m {
            row[hi + 1] = over;
        }
        // Row minimums never fall, so once a whole row is too far, so is the answer.
        if best > max {
            return None;
        }
        std::mem::swap(&mut two_up, &mut up);
        std::mem::swap(&mut up, &mut row);
    }
    let d = up[m];
    (d <= max).then_some(d)
}

/// The fewest one-character insertions, deletions and substitutions that turn `a` into `b` (Levenshtein
/// distance). "kitten" to "sitting" is 3. O(N × M) time, O(min(N, M)) memory.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b) = (chars(a), chars(b));
    let longest = a.len().max(b.len());
    banded(&a, &b, longest, false).unwrap_or(longest)
}

/// Levenshtein distance, also counting a swap of two neighbouring characters as one edit ("optimal string
/// alignment"): "jdoe" to "jdeo" is 1, not 2. No part of the text is edited twice, so this can be larger than
/// the unrestricted Damerau-Levenshtein distance: "ca" to "abc" is 3 here, against 2 there. O(N × M).
pub fn osa_distance(a: &str, b: &str) -> usize {
    let (a, b) = (chars(a), chars(b));
    let longest = a.len().max(b.len());
    banded(&a, &b, longest, true).unwrap_or(longest)
}

/// [`levenshtein`], when it is at most `max`; otherwise `None`, found early. Only cells near the diagonal are
/// worked out, so it costs O(N × max) instead of O(N × M): the check to run against a long list.
pub fn levenshtein_within(a: &str, b: &str, max: usize) -> Option<usize> {
    banded(&chars(a), &chars(b), max, false)
}

/// [`osa_distance`], when it is at most `max`; otherwise `None`, found early. O(N × max).
pub fn osa_within(a: &str, b: &str, max: usize) -> Option<usize> {
    banded(&chars(a), &chars(b), max, true)
}

/// How alike two strings are, from 0 (nothing in common) to 1 (identical), by Jaro's measure. Two empty strings
/// score 1.
///
/// Characters match when they are equal and not too far apart: within half the longer length, minus one. With m
/// matches, and t pairs of matched characters that come in a different order, the score is the average of
/// m / len(a), m / len(b) and (m − t) / m. O(N × M) at worst.
pub fn jaro(a: &str, b: &str) -> f64 {
    let (a, b) = (chars(a), chars(b));
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let window = (a.len().max(b.len()) / 2).saturating_sub(1);
    let mut taken = vec![false; b.len()];
    let mut matched_in_a: Vec<char> = Vec::new();
    for (i, &c) in a.iter().enumerate() {
        let lo = i.saturating_sub(window);
        let hi = (i + window + 1).min(b.len());
        for j in lo..hi {
            if !taken[j] && b[j] == c {
                taken[j] = true;
                matched_in_a.push(c);
                break;
            }
        }
    }
    if matched_in_a.is_empty() {
        return 0.0;
    }
    let matched_in_b = b.iter().zip(&taken).filter(|(_, &t)| t).map(|(&c, _)| c);
    let out_of_order = matched_in_a
        .iter()
        .zip(matched_in_b)
        .filter(|(x, y)| **x != *y)
        .count();
    let m = matched_in_a.len() as f64;
    let t = out_of_order as f64 / 2.0;
    (m / a.len() as f64 + m / b.len() as f64 + (m - t) / m) / 3.0
}

/// [`jaro`], boosted when the strings start the same way (Winkler's version): each of up to 4 shared leading
/// characters closes a tenth of the remaining gap to 1. "MARTHA" and "MARHTA" score 0.961.
///
/// Names and handles tend to keep their first letters even when the rest is mangled, which is why this beats
/// plain edit distance at ranking candidates.
pub fn jaro_winkler(a: &str, b: &str) -> f64 {
    let j = jaro(a, b);
    let shared_start = a
        .chars()
        .zip(b.chars())
        .take_while(|(x, y)| x == y)
        .take(4)
        .count();
    j + shared_start as f64 * 0.1 * (1.0 - j)
}

/// A handle reduced to its lowercase letters and digits: "@J.Doe_88" becomes "jdoe88". Letters and digits from
/// any alphabet are kept.
pub fn handle_key(handle: &str) -> String {
    handle
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// An entry in a list that is close to the string asked about, from [`close_handles`] or a [`NearSet`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameMatch {
    /// Where it sits in the list.
    pub index: usize,
    /// How many edits apart the two are. [`close_handles`] counts them after [`handle_key`].
    pub edits: usize,
}

/// The handles in `candidates` within `max_edits` edits of `query`, closest first, and in list order on ties.
/// Both sides go through [`handle_key`] first. Edits are counted as in [`osa_distance`], so a swapped pair of
/// letters is one typo. O(list × N × max_edits).
///
/// **SkeleTrace use:** from one known handle, the accounts on another platform worth opening first.
pub fn close_handles(query: &str, candidates: &[&str], max_edits: usize) -> Vec<NameMatch> {
    let key = chars(&handle_key(query));
    let mut found: Vec<NameMatch> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            banded(&key, &chars(&handle_key(candidate)), max_edits, true)
                .map(|edits| NameMatch { index, edits })
        })
        .collect();
    found.sort_by_key(|m| (m.edits, m.index));
    found
}

/// The base of the polynomial hash behind every key, with arithmetic wrapping at 2^64. It is odd, so no power of it
/// is ever 0. A fixed base makes a set behave the same on every run.
const BASE: u64 = 0x0915_F2A3_C6B8_E5D7;

/// Deletions are only made among the first this-many characters of a term or a word (SymSpell's prefix trick). A
/// term then needs at most 1 + 8 + 28 = 37 keys at two edits, however long it is. It never loses a match, it only
/// lets a few more candidates through to the final check.
const PREFIX: usize = 8;

/// `POWERS[i]` is the base to the power i.
const POWERS: [u64; PREFIX + 1] = {
    let mut powers = [1u64; PREFIX + 1];
    let mut i = 1;
    while i <= PREFIX {
        powers[i] = powers[i - 1].wrapping_mul(BASE);
        i += 1;
    }
    powers
};

/// Calls `key` once for each deletion variant of `text`, repeats included: its first [`PREFIX`] characters with up
/// to `depth` of them taken out. A key is a polynomial hash of what is left, worked out in O(1) from prefix hashes
/// rather than by building each variant. Equal variants always share a key. Different variants share one only by
/// a rare collision, which costs one wasted check and never a missed match.
fn variant_keys(text: &[char], depth: usize, mut key: impl FnMut(u64)) {
    let n = text.len().min(PREFIX);
    // `prefix[i]` is the hash of the first i characters. Characters count from 1, so a leading U+0000 still
    // changes the hash.
    let mut prefix = [0u64; PREFIX + 1];
    for (i, &c) in text[..n].iter().enumerate() {
        prefix[i + 1] = prefix[i].wrapping_mul(BASE).wrapping_add(u64::from(c) + 1);
    }
    // The hash of characters a..b on their own.
    let span = |a: usize, b: usize| prefix[b].wrapping_sub(prefix[a].wrapping_mul(POWERS[b - a]));
    key(prefix[n]);
    if depth >= 1 {
        // Without character p: everything before it moves one place closer to the end.
        for p in 0..n {
            key(prefix[p]
                .wrapping_mul(POWERS[n - 1 - p])
                .wrapping_add(span(p + 1, n)));
        }
    }
    if depth >= 2 {
        // Without characters p and q.
        for p in 0..n {
            let head = prefix[p].wrapping_mul(POWERS[n.saturating_sub(2 + p)]);
            for q in p + 1..n {
                let middle = span(p + 1, q).wrapping_mul(POWERS[n - 1 - q]);
                key(head.wrapping_add(middle).wrapping_add(span(q + 1, n)));
            }
        }
    }
}

/// Spreads a key's bits before the hash table uses them. The low bits of a polynomial hash that wraps at 2^64 depend
/// only on the low bits of the characters, and the standard table picks its slot from the low bits.
#[derive(Clone, Copy, Debug, Default)]
struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_u64((self.0 << 8) ^ u64::from(byte));
        }
    }

    fn write_u64(&mut self, n: u64) {
        // SplitMix64's finishing steps.
        let mut z = n.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        self.0 = z ^ (z >> 31);
    }
}

/// Each key's run of term indexes in the postings list: (start, length).
type KeyTable = HashMap<u64, (u32, u32), BuildHasherDefault<KeyHasher>>;

/// Why a [`NearSet`] couldn't be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NearSetError {
    /// A term asked for more edits than [`NearSet::MAX_EDITS`].
    TooManyEdits {
        /// Where the term sits in the list.
        index: usize,
        /// How many edits it asked for.
        edits: usize,
    },
    /// The terms add up to more than the set's 32-bit tables hold.
    TooLarge,
}

impl fmt::Display for NearSetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NearSetError::TooManyEdits { index, edits } => write!(
                f,
                "term {index} asks for {edits} edits; at most {} are allowed",
                NearSet::MAX_EDITS
            ),
            NearSetError::TooLarge => write!(f, "the terms are too large to index"),
        }
    }
}

impl std::error::Error for NearSetError {}

/// Many terms, each allowed its own number of typos, ready to check any word against all of them at once.
///
/// It works by symmetric deletion, the idea behind SymSpell. Two strings within k edits of each other can always
/// be cut down to the same string by deleting at most k characters from each. So every term is filed under each
/// of its deletion variants, and a word looks up its own. A term sharing a slot is a candidate, and a banded
/// edit-distance check settles it. Most words share no slot with anything and cost a few hash lookups.
///
/// Edits are counted as in [`osa_distance`]: swapping two neighbouring characters is one typo. Characters are
/// compared exactly, so fold capitals first if they shouldn't count. Empty terms never match. Repeated terms are
/// each reported, under their own index.
///
/// | Step | Cost |
/// |---|---|
/// | [`NearSet::new`] | up to 37 keys per term at two edits, 9 at one |
/// | [`NearSearcher::find`] | up to 37 lookups per word, then O(N × k) per candidate |
///
/// **SkeleTrace use:** sweeping a dump for near-misses of a watch list, "jdeo88" for "jdoe88", without comparing
/// every word with every term.
#[derive(Clone, Debug)]
pub struct NearSet {
    terms: Vec<Vec<char>>,
    limits: Vec<usize>,
    /// `depth[n]` is how deep a word of n characters is cut: the largest allowance among the terms it could be
    /// near, or `None` when it couldn't be near any.
    depth: Vec<Option<u8>>,
    table: KeyTable,
    postings: Vec<u32>,
}

impl NearSet {
    /// The most edits a term may allow.
    pub const MAX_EDITS: usize = 2;

    /// Builds the set from `(term, edits allowed)` pairs. Each allowance is at most [`NearSet::MAX_EDITS`].
    pub fn new<'a, I>(terms: I) -> Result<NearSet, NearSetError>
    where
        I: IntoIterator<Item = (&'a str, usize)>,
    {
        let mut all_chars: Vec<Vec<char>> = Vec::new();
        let mut limits = Vec::new();
        let mut keyed: Vec<(u64, u32)> = Vec::new();
        for (index, (term, edits)) in terms.into_iter().enumerate() {
            if edits > Self::MAX_EDITS {
                return Err(NearSetError::TooManyEdits { index, edits });
            }
            let id = u32::try_from(index).map_err(|_| NearSetError::TooLarge)?;
            let chars: Vec<char> = term.chars().collect();
            if !chars.is_empty() {
                variant_keys(&chars, edits, |key| keyed.push((key, id)));
            }
            all_chars.push(chars);
            limits.push(edits);
        }
        keyed.sort_unstable();
        keyed.dedup();
        u32::try_from(keyed.len()).map_err(|_| NearSetError::TooLarge)?;
        let mut table = KeyTable::default();
        let mut postings = Vec::with_capacity(keyed.len());
        let mut at = 0;
        while at < keyed.len() {
            let key = keyed[at].0;
            let start = postings.len();
            while at < keyed.len() && keyed[at].0 == key {
                postings.push(keyed[at].1);
                at += 1;
            }
            // Both fit: there are no more postings than keyed entries, which fit in a u32.
            table.insert(key, (start as u32, (postings.len() - start) as u32));
        }
        let longest = all_chars
            .iter()
            .zip(&limits)
            .filter(|(chars, _)| !chars.is_empty())
            .map(|(chars, &edits)| chars.len() + edits)
            .max()
            .unwrap_or(0);
        // A word can only be near a term whose length is within the term's allowance of its own. It needs cutting
        // as deep as the largest such allowance, and no deeper.
        let mut depth: Vec<Option<u8>> = vec![None; longest + 1];
        for (chars, &edits) in all_chars.iter().zip(&limits) {
            if !chars.is_empty() {
                // Allowances are at most MAX_EDITS, so they fit in a u8.
                let edits8 = edits as u8;
                for slot in &mut depth[chars.len().saturating_sub(edits)..=chars.len() + edits] {
                    *slot = Some(slot.map_or(edits8, |d| d.max(edits8)));
                }
            }
        }
        Ok(NearSet {
            terms: all_chars,
            limits,
            depth,
            table,
            postings,
        })
    }

    /// How many terms were given, empty ones included.
    pub fn len(&self) -> usize {
        self.terms.len()
    }

    /// True when no terms were given.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// How many edits term `index` allows.
    pub fn edits_allowed(&self, index: usize) -> Option<usize> {
        self.limits.get(index).copied()
    }

    /// Whether a word of `chars` characters could be near any term at all: a quick test before the real one.
    pub fn could_match(&self, chars: usize) -> bool {
        self.depth_for(chars).is_some()
    }

    /// How deep to cut a word of `chars` characters, if it could be near anything.
    fn depth_for(&self, chars: usize) -> Option<usize> {
        self.depth.get(chars).copied().flatten().map(usize::from)
    }

    /// A searcher, which keeps its scratch space from one word to the next.
    pub fn searcher(&self) -> NearSearcher<'_> {
        NearSearcher {
            set: self,
            chars: Vec::new(),
            seen: vec![0; self.terms.len()],
            round: 0,
            candidates: Vec::new(),
        }
    }

    /// Every term `word` is near, closest first and then in list order. For many words, reuse a
    /// [`NearSearcher`] instead.
    pub fn find(&self, word: &str) -> Vec<NameMatch> {
        let mut found = Vec::new();
        self.searcher().find(word, &mut found);
        found
    }
}

/// Checks words against a [`NearSet`], reusing its scratch space from one word to the next. Made by
/// [`NearSet::searcher`].
#[derive(Clone, Debug)]
pub struct NearSearcher<'a> {
    set: &'a NearSet,
    chars: Vec<char>,
    /// The round in which each term last became a candidate, so each is checked once per word.
    seen: Vec<u32>,
    round: u32,
    candidates: Vec<u32>,
}

impl NearSearcher<'_> {
    /// Appends to `found` every term that `word` is within that term's allowance of, closest first and then in
    /// list order. Each match gives the term's index and the edits between them.
    pub fn find(&mut self, word: &str, found: &mut Vec<NameMatch>) {
        let set = self.set;
        self.chars.clear();
        self.chars.extend(word.chars());
        let n = self.chars.len();
        let Some(depth) = set.depth_for(n) else {
            return;
        };
        self.round = self.round.wrapping_add(1);
        if self.round == 0 {
            // The counter has gone all the way round: forget every old mark.
            self.seen.fill(0);
            self.round = 1;
        }
        let round = self.round;
        self.candidates.clear();
        let (seen, candidates) = (&mut self.seen, &mut self.candidates);
        variant_keys(&self.chars, depth, |key| {
            if let Some(&(start, len)) = set.table.get(&key) {
                let (start, len) = (start as usize, len as usize);
                for &term in &set.postings[start..start + len] {
                    let mark = &mut seen[term as usize];
                    if *mark != round {
                        *mark = round;
                        candidates.push(term);
                    }
                }
            }
        });
        let first = found.len();
        for &term in candidates.iter() {
            let (chars, limit) = (&set.terms[term as usize], set.limits[term as usize]);
            if n.abs_diff(chars.len()) > limit {
                continue;
            }
            if let Some(edits) = banded(&self.chars, chars, limit, true) {
                found.push(NameMatch {
                    index: term as usize,
                    edits,
                });
            }
        }
        found[first..].sort_unstable_by_key(|m| (m.edits, m.index));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Lcg;
    use std::collections::{HashMap, VecDeque};

    /// Every string over "abc" up to `len` characters long.
    fn all_strings(len: usize) -> Vec<String> {
        let mut out = vec![String::new()];
        let mut layer = vec![String::new()];
        for _ in 0..len {
            let mut next = Vec::new();
            for s in &layer {
                for c in ['a', 'b', 'c'] {
                    next.push(format!("{s}{c}"));
                }
            }
            out.extend(next.iter().cloned());
            layer = next;
        }
        out
    }

    /// Fewest edits from `start` to every string over "abc" of up to 4 characters, by breadth-first search over
    /// single edits: the definition itself, with nothing clever.
    fn edits_by_search(start: &str, swaps: bool) -> HashMap<String, usize> {
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut queue = VecDeque::new();
        seen.insert(start.to_string(), 0);
        queue.push_back(start.to_string());
        while let Some(s) = queue.pop_front() {
            let d = seen[&s];
            let cs: Vec<char> = s.chars().collect();
            let mut next: Vec<Vec<char>> = Vec::new();
            for i in 0..=cs.len() {
                for c in ['a', 'b', 'c'] {
                    let mut t = cs.clone();
                    t.insert(i, c);
                    next.push(t);
                }
            }
            for i in 0..cs.len() {
                let mut t = cs.clone();
                t.remove(i);
                next.push(t);
                for c in ['a', 'b', 'c'] {
                    let mut t = cs.clone();
                    t[i] = c;
                    next.push(t);
                }
                if swaps && i + 1 < cs.len() {
                    let mut t = cs.clone();
                    t.swap(i, i + 1);
                    next.push(t);
                }
            }
            for t in next {
                if t.len() > 4 {
                    continue;
                }
                let t: String = t.into_iter().collect();
                if !seen.contains_key(&t) {
                    seen.insert(t.clone(), d + 1);
                    queue.push_back(t);
                }
            }
        }
        seen
    }

    /// The optimal-string-alignment recurrence written out as a full table.
    fn osa_by_table(a: &str, b: &str) -> usize {
        let (a, b) = (chars(a), chars(b));
        let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for (i, row) in d.iter_mut().enumerate() {
            row[0] = i;
        }
        for (j, cell) in d[0].iter_mut().enumerate() {
            *cell = j;
        }
        for i in 1..=a.len() {
            for j in 1..=b.len() {
                let cost = usize::from(a[i - 1] != b[j - 1]);
                let mut best = (d[i - 1][j] + 1)
                    .min(d[i][j - 1] + 1)
                    .min(d[i - 1][j - 1] + cost);
                if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                    best = best.min(d[i - 2][j - 2] + 1);
                }
                d[i][j] = best;
            }
        }
        d[a.len()][b.len()]
    }

    #[test]
    fn known_distances() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("flaw", "lawn"), 2);
        assert_eq!(levenshtein("Saturday", "Sunday"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("jdoe88", "jdeo88"), 2);
        assert_eq!(osa_distance("jdoe88", "jdeo88"), 1);
        assert_eq!(osa_distance("abcdef", "abdcef"), 1);
        assert_eq!(osa_distance("ca", "abc"), 3);
        assert_eq!(levenshtein("café", "cafe"), 1);
        assert_eq!(levenshtein_within("kitten", "sitting", 2), None);
        assert_eq!(levenshtein_within("kitten", "sitting", 3), Some(3));
        assert_eq!(osa_within("jdoe88", "jdeo88", 1), Some(1));
        assert_eq!(levenshtein_within("a", "abcdef", 4), None);
    }

    #[test]
    fn distances_match_searching_every_edit() {
        let strings = all_strings(3);
        for a in &strings {
            let plain = edits_by_search(a, false);
            let with_swaps = edits_by_search(a, true);
            for b in &strings {
                assert_eq!(levenshtein(a, b), plain[b], "{a:?} to {b:?}");
                let osa = osa_distance(a, b);
                assert_eq!(osa, osa_by_table(a, b), "{a:?} to {b:?}");
                // Restricting swaps can only cost more, and swaps can only help.
                assert!(osa >= with_swaps[b] && osa <= plain[b], "{a:?} to {b:?}");
            }
        }
    }

    #[test]
    fn bounded_checks_agree_with_full_distances() {
        let mut rng = Lcg(101);
        let word = |rng: &mut Lcg| -> String {
            (0..rng.below(13))
                .map(|_| char::from(b'a' + rng.below(3) as u8))
                .collect()
        };
        for _ in 0..400 {
            let (a, b) = (word(&mut rng), word(&mut rng));
            let (lev, osa) = (levenshtein(&a, &b), osa_distance(&a, &b));
            assert_eq!(osa, osa_by_table(&a, &b), "{a:?} {b:?}");
            for max in 0..14 {
                assert_eq!(
                    levenshtein_within(&a, &b, max),
                    (lev <= max).then_some(lev),
                    "{a:?} {b:?} {max}"
                );
                assert_eq!(
                    osa_within(&a, &b, max),
                    (osa <= max).then_some(osa),
                    "{a:?} {b:?} {max}"
                );
            }
        }
        assert_eq!(levenshtein_within("abc", "abd", usize::MAX), Some(1));
    }

    #[test]
    fn jaro_scores() {
        let close = |x: f64, y: f64| (x - y).abs() < 0.0005;
        // The standard worked examples, checked by hand, to three places.
        assert!(close(jaro("MARTHA", "MARHTA"), 0.944));
        assert!(close(jaro_winkler("MARTHA", "MARHTA"), 0.961));
        assert!(close(jaro("DWAYNE", "DUANE"), 0.822));
        assert!(close(jaro_winkler("DWAYNE", "DUANE"), 0.840));
        assert!(close(jaro("DIXON", "DICKSONX"), 0.767));
        assert!(close(jaro_winkler("DIXON", "DICKSONX"), 0.813));
        assert_eq!(jaro("", ""), 1.0);
        assert_eq!(jaro("abc", ""), 0.0);
        assert_eq!(jaro("abc", "xyz"), 0.0);
        assert_eq!(jaro_winkler("same", "same"), 1.0);
        let mut rng = Lcg(103);
        for _ in 0..300 {
            let a: String = (0..rng.below(9))
                .map(|_| char::from(b'a' + rng.below(4) as u8))
                .collect();
            let b: String = (0..rng.below(9))
                .map(|_| char::from(b'a' + rng.below(4) as u8))
                .collect();
            let (j, jw) = (jaro(&a, &b), jaro_winkler(&a, &b));
            assert!((0.0..=1.0).contains(&j) && (0.0..=1.0).contains(&jw));
            assert!(jw >= j, "{a:?} {b:?}");
        }
    }

    #[test]
    fn handles() {
        assert_eq!(handle_key("@J.Doe_88"), "jdoe88");
        assert_eq!(handle_key("Lantern-Ops "), "lanternops");
        assert_eq!(handle_key("Ünïcödé"), "ünïcödé");
        let list = ["jdoe88", "jdeo88", "john.doe", "j_doe_89", "xyz"];
        assert_eq!(
            close_handles("J.Doe_88", &list, 1),
            vec![
                NameMatch { index: 0, edits: 0 },
                NameMatch { index: 1, edits: 1 },
                NameMatch { index: 3, edits: 1 },
            ]
        );
        assert_eq!(close_handles("J.Doe_88", &list, 0).len(), 1);
    }

    /// Every match, by measuring the distance to every term with the full table.
    fn near_by_table(terms: &[(String, usize)], word: &str) -> Vec<NameMatch> {
        let mut found: Vec<NameMatch> = terms
            .iter()
            .enumerate()
            .filter(|(_, (term, _))| !term.is_empty())
            .filter_map(|(index, (term, limit))| {
                let edits = osa_by_table(word, term);
                (edits <= *limit).then_some(NameMatch { index, edits })
            })
            .collect();
        found.sort_by_key(|m| (m.edits, m.index));
        found
    }

    fn near_set(terms: &[(String, usize)]) -> NearSet {
        NearSet::new(terms.iter().map(|(term, limit)| (term.as_str(), *limit))).unwrap()
    }

    /// Every string one typo from `s` over "abc": an insertion, deletion, substitution or neighbouring swap.
    fn one_typo_away(s: &str) -> Vec<String> {
        let cs: Vec<char> = s.chars().collect();
        let mut out = Vec::new();
        for i in 0..=cs.len() {
            for c in ['a', 'b', 'c'] {
                let mut t = cs.clone();
                t.insert(i, c);
                out.push(t.into_iter().collect());
            }
        }
        for i in 0..cs.len() {
            let mut t = cs.clone();
            t.remove(i);
            out.push(t.into_iter().collect());
            for c in ['a', 'b', 'c'] {
                let mut t = cs.clone();
                t[i] = c;
                out.push(t.into_iter().collect());
            }
            if i + 1 < cs.len() {
                let mut t = cs.clone();
                t.swap(i, i + 1);
                out.push(t.into_iter().collect());
            }
        }
        out
    }

    #[test]
    fn near_sets_find_what_checking_every_term_finds() {
        let mut rng = Lcg(109);
        for round in 0..300 {
            let alphabet: &[char] = if round % 3 == 0 {
                &['a', 'b', 'c']
            } else {
                &['a', 'b']
            };
            let word = |rng: &mut Lcg, longest: u64| -> String {
                (0..rng.below(longest + 1))
                    .map(|_| alphabet[rng.below(alphabet.len() as u64) as usize])
                    .collect()
            };
            // Up to 12 characters, so terms and words run past the eight-character prefix.
            let terms: Vec<(String, usize)> = (0..1 + rng.below(8))
                .map(|_| (word(&mut rng, 12), rng.below(3) as usize))
                .collect();
            let set = near_set(&terms);
            // One searcher for every word, so its marks carry from word to word as they do in a sweep.
            let mut searcher = set.searcher();
            let mut found = Vec::new();
            for _ in 0..40 {
                let w = word(&mut rng, 14);
                found.clear();
                searcher.find(&w, &mut found);
                assert_eq!(found, near_by_table(&terms, &w), "{w:?} against {terms:?}");
            }
        }
    }

    #[test]
    fn near_sets_agree_on_every_short_word_and_every_typo_of_long_terms() {
        let terms: Vec<(String, usize)> = [
            ("", 2),
            ("a", 1),
            ("ab", 2),
            ("abc", 0),
            ("cab", 1),
            ("abca", 2),
            ("bbbb", 1),
            ("abcabcab", 2),
            ("aabbccaab", 2),
            ("abcabcabca", 1),
            ("ccccccccccc", 2),
        ]
        .iter()
        .map(|&(term, limit)| (term.to_string(), limit))
        .collect();
        let set = near_set(&terms);
        let mut searcher = set.searcher();
        let mut found = Vec::new();
        let mut check = |w: &str| {
            found.clear();
            searcher.find(w, &mut found);
            assert_eq!(found, near_by_table(&terms, w), "{w:?}");
        };
        for w in all_strings(4) {
            check(&w);
        }
        // Every word one or two typos from the long terms, wherever the typos fall: inside the prefix, past it, or
        // one each side.
        for base in ["abcabcab", "aabbccaab", "abcabcabca", "ccccccccccc"] {
            for once in one_typo_away(base) {
                check(&once);
                for twice in one_typo_away(&once) {
                    check(&twice);
                }
            }
        }
    }

    #[test]
    fn near_set_edges() {
        let empty = NearSet::new(Vec::<(&str, usize)>::new()).unwrap();
        assert!(empty.is_empty());
        assert!(empty.find("anything").is_empty());
        assert!(!empty.could_match(0));
        assert_eq!(
            NearSet::new([("jdoe88", 3)]).unwrap_err(),
            NearSetError::TooManyEdits { index: 0, edits: 3 }
        );
        let set = NearSet::new([
            ("", 2),
            ("jdoe88", 1),
            ("jdoe88", 2),
            ("lantern@example.org", 2),
            ("café", 1),
        ])
        .unwrap();
        assert_eq!(set.len(), 5);
        assert_eq!(set.edits_allowed(2), Some(2));
        assert_eq!(set.edits_allowed(9), None);
        // A swap is one typo, repeats are each reported, and the empty term never matches.
        assert_eq!(
            set.find("jdeo88"),
            vec![
                NameMatch { index: 1, edits: 1 },
                NameMatch { index: 2, edits: 1 }
            ]
        );
        assert_eq!(set.find("jdoe1988"), vec![NameMatch { index: 2, edits: 2 }]);
        assert!(set.find("x").is_empty());
        // Typos past the first eight characters are found as surely as typos inside them.
        assert_eq!(
            set.find("lantern@example.orh"),
            vec![NameMatch { index: 3, edits: 1 }]
        );
        assert_eq!(
            set.find("lnatern@exmaple.org"),
            vec![NameMatch { index: 3, edits: 2 }]
        );
        assert_eq!(set.find("cafe"), vec![NameMatch { index: 4, edits: 1 }]);
        assert!(set.could_match(5) && set.could_match(21) && !set.could_match(22));
        assert!(!set.could_match(1));
        // The first word leaves its candidates marked with round 1. When the counter later wraps back to 1, those
        // marks must be forgotten, or the same candidates would be skipped as already seen.
        let mut searcher = set.searcher();
        let mut found = Vec::new();
        searcher.find("jdeo88", &mut found);
        assert_eq!(found.len(), 2);
        searcher.round = u32::MAX;
        searcher.find("jdeo88", &mut found);
        assert_eq!(
            found.len(),
            4,
            "find appends, and the wrap forgot the old marks"
        );
        assert_eq!(
            NearSetError::TooLarge.to_string(),
            "the terms are too large to index"
        );
    }
}
