// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Many patterns, one pass: finding every one of thousands of identifiers in a text at once (Aho-Corasick).
//!
//! Searching for each identifier on its own reads the text once per identifier. Aho-Corasick builds one
//! machine from all of them and reads the text once, however many there are. The machine is a trie of the
//! patterns (a tree with one branch per next byte) plus a "failure link" on every node. When the next byte
//! has no branch, the failure link jumps to the longest other pattern start that the text read so far still
//! ends with, so no byte is ever read twice.
//!
//! | Step | Cost |
//! |---|---|
//! | [`PatternSet::new`] | O(total pattern bytes) |
//! | [`PatternSet::find_all`], [`Scanner::feed`] | O(text bytes + matches) |
//!
//! Patterns and text are bytes, so UTF-8 text works as it is: a match of a whole UTF-8 pattern always starts
//! and ends on character boundaries. Case-folding, when asked for, covers ASCII letters only.
//!
//! **SkeleTrace use:** sweeping a leak, a paste or a scraped page for a watch list of emails, handles, phone
//! numbers and domains. [`Scanner`] takes the text in chunks, so a dump never has to fit in memory.

use std::fmt;

/// Marks "no node" in the compact tables.
const NONE: u32 = u32::MAX;

/// Why a pattern set couldn't be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatternSetError {
    /// The patterns add up to more than about four billion bytes, past what the machine's 32-bit tables hold.
    TooLarge,
}

impl fmt::Display for PatternSetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PatternSetError::TooLarge => write!(f, "the patterns are too large to index"),
        }
    }
}

impl std::error::Error for PatternSetError {}

/// One occurrence of a pattern in the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    /// Which pattern, by its position in the list given to [`PatternSet::new`].
    pub pattern: usize,
    /// Byte offset where the match starts, counted from the start of everything read.
    pub start: usize,
    /// Byte offset just past its end.
    pub end: usize,
}

/// A set of patterns compiled into one Aho-Corasick machine, ready to search any amount of text.
///
/// Empty patterns never match. Repeated patterns are each reported, under their own index.
#[derive(Clone, Debug)]
pub struct PatternSet {
    fold_case: bool,
    /// Where each byte leads from the root; a byte with no branch stays at the root.
    root_next: Vec<u32>,
    /// Node v's branches are `edge_bytes[edge_start[v]..edge_start[v + 1]]`, sorted, leading to `edge_to`.
    edge_start: Vec<u32>,
    edge_bytes: Vec<u8>,
    edge_to: Vec<u32>,
    /// The failure link of each node.
    fail: Vec<u32>,
    /// The nearest node down the failure links where some pattern ends, or `NONE`.
    dict: Vec<u32>,
    /// The patterns ending at node v are `outputs[out_start[v]..out_start[v + 1]]`.
    out_start: Vec<u32>,
    outputs: Vec<u32>,
    /// Length in bytes of each pattern, by index.
    lengths: Vec<usize>,
}

impl PatternSet {
    /// Compiles `patterns` (anything that gives bytes: `&str`, `String`, `&[u8]`), matching case exactly.
    pub fn new<I, P>(patterns: I) -> Result<PatternSet, PatternSetError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<[u8]>,
    {
        Self::build(patterns, false)
    }

    /// Compiles `patterns` so that ASCII letters match in either case: "Lantern@Example.org" finds
    /// "lantern@example.ORG". Letters outside ASCII still match exactly.
    pub fn new_ascii_case_insensitive<I, P>(patterns: I) -> Result<PatternSet, PatternSetError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<[u8]>,
    {
        Self::build(patterns, true)
    }

    fn build<I, P>(patterns: I, fold_case: bool) -> Result<PatternSet, PatternSetError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<[u8]>,
    {
        // 1. The trie: node 0 is the root (the empty prefix).
        let mut children: Vec<Vec<(u8, u32)>> = vec![Vec::new()];
        let mut ends_here: Vec<Vec<u32>> = vec![Vec::new()];
        let mut lengths = Vec::new();
        for (index, pattern) in patterns.into_iter().enumerate() {
            let pattern = pattern.as_ref();
            let index = u32::try_from(index)
                .ok()
                .filter(|&i| i != NONE)
                .ok_or(PatternSetError::TooLarge)?;
            lengths.push(pattern.len());
            if pattern.is_empty() {
                continue;
            }
            let mut node = 0usize;
            for &byte in pattern {
                let byte = if fold_case {
                    byte.to_ascii_lowercase()
                } else {
                    byte
                };
                node = match children[node].binary_search_by_key(&byte, |&(b, _)| b) {
                    Ok(at) => children[node][at].1 as usize,
                    Err(at) => {
                        let fresh = u32::try_from(children.len())
                            .ok()
                            .filter(|&n| n != NONE)
                            .ok_or(PatternSetError::TooLarge)?;
                        children[node].insert(at, (byte, fresh));
                        children.push(Vec::new());
                        ends_here.push(Vec::new());
                        fresh as usize
                    }
                };
            }
            ends_here[node].push(index);
        }
        let nodes = children.len();

        // 2. Failure and dictionary links, breadth first, so every link points at a node already finished.
        let child = |node: usize, byte: u8| -> Option<u32> {
            children[node]
                .binary_search_by_key(&byte, |&(b, _)| b)
                .ok()
                .map(|at| children[node][at].1)
        };
        let mut fail = vec![0u32; nodes];
        let mut dict = vec![NONE; nodes];
        let mut queue = std::collections::VecDeque::new();
        for &(_, v) in &children[0] {
            queue.push_back(v as usize); // the root's children fail back to the root
        }
        while let Some(u) = queue.pop_front() {
            for &(byte, v) in &children[u] {
                let v = v as usize;
                let mut f = fail[u] as usize;
                let target = loop {
                    if let Some(next) = child(f, byte) {
                        break next;
                    }
                    if f == 0 {
                        break 0;
                    }
                    f = fail[f] as usize;
                };
                fail[v] = target;
                let t = target as usize;
                dict[v] = if ends_here[t].is_empty() {
                    dict[t]
                } else {
                    target
                };
                queue.push_back(v);
            }
        }

        // 3. Pack everything into flat tables.
        let too_large = |n: usize| u32::try_from(n).map_err(|_| PatternSetError::TooLarge);
        let mut root_next = vec![0u32; 256];
        for &(byte, v) in &children[0] {
            root_next[usize::from(byte)] = v;
        }
        let mut edge_start = Vec::with_capacity(nodes + 1);
        let (mut edge_bytes, mut edge_to) = (Vec::new(), Vec::new());
        let mut out_start = Vec::with_capacity(nodes + 1);
        let mut outputs = Vec::new();
        for (branches, ends) in children.iter().zip(&ends_here) {
            edge_start.push(too_large(edge_bytes.len())?);
            out_start.push(too_large(outputs.len())?);
            for &(byte, v) in branches {
                edge_bytes.push(byte);
                edge_to.push(v);
            }
            outputs.extend_from_slice(ends);
        }
        edge_start.push(too_large(edge_bytes.len())?);
        out_start.push(too_large(outputs.len())?);
        Ok(PatternSet {
            fold_case,
            root_next,
            edge_start,
            edge_bytes,
            edge_to,
            fail,
            dict,
            out_start,
            outputs,
            lengths,
        })
    }

    /// How many patterns were given, empty ones included.
    pub fn len(&self) -> usize {
        self.lengths.len()
    }

    /// True when no patterns were given.
    pub fn is_empty(&self) -> bool {
        self.lengths.is_empty()
    }

    /// Length in bytes of pattern `index`.
    pub fn pattern_len(&self, index: usize) -> Option<usize> {
        self.lengths.get(index).copied()
    }

    /// A scanner that reads text in chunks of any size and reports matches across chunk boundaries.
    pub fn scanner(&self) -> Scanner<'_> {
        Scanner {
            set: self,
            state: 0,
            offset: 0,
        }
    }

    /// Every match in `text`, overlapping ones included. In order of where they end; at the same end, longest
    /// first; for repeated patterns, lowest index first.
    pub fn find_all(&self, text: impl AsRef<[u8]>) -> Vec<Hit> {
        let mut hits = Vec::new();
        self.scanner().feed(text.as_ref(), &mut hits);
        hits
    }

    /// The indexes of the patterns that occur in `text` at least once, ascending.
    pub fn present(&self, text: impl AsRef<[u8]>) -> Vec<usize> {
        let mut seen = vec![false; self.len()];
        let mut scanner = self.scanner();
        let mut hits = Vec::new();
        for chunk in text.as_ref().chunks(1 << 16) {
            scanner.feed(chunk, &mut hits);
            for hit in hits.drain(..) {
                seen[hit.pattern] = true;
            }
        }
        (0..seen.len()).filter(|&i| seen[i]).collect()
    }

    fn step(&self, mut state: u32, byte: u8) -> u32 {
        loop {
            if state == 0 {
                return self.root_next[usize::from(byte)];
            }
            let v = state as usize;
            let (from, to) = (self.edge_start[v] as usize, self.edge_start[v + 1] as usize);
            if let Ok(at) = self.edge_bytes[from..to].binary_search(&byte) {
                return self.edge_to[from + at];
            }
            state = self.fail[v];
        }
    }

    fn report(&self, state: u32, end: usize, hits: &mut Vec<Hit>) {
        let mut node = if self.out_start[state as usize] < self.out_start[state as usize + 1] {
            state
        } else {
            self.dict[state as usize]
        };
        while node != NONE {
            let v = node as usize;
            for &pattern in
                &self.outputs[self.out_start[v] as usize..self.out_start[v + 1] as usize]
            {
                let pattern = pattern as usize;
                hits.push(Hit {
                    pattern,
                    start: end - self.lengths[pattern],
                    end,
                });
            }
            node = self.dict[v];
        }
    }
}

/// Reads text in chunks, carrying its place in the machine from one chunk to the next. Made by
/// [`PatternSet::scanner`].
#[derive(Clone, Debug)]
pub struct Scanner<'a> {
    set: &'a PatternSet,
    state: u32,
    offset: usize,
}

impl Scanner<'_> {
    /// Reads `chunk` and appends its matches to `hits`, in the order [`PatternSet::find_all`] gives. Offsets
    /// count from the start of the first chunk, so a match can start in an earlier chunk.
    pub fn feed(&mut self, chunk: &[u8], hits: &mut Vec<Hit>) {
        for (i, &byte) in chunk.iter().enumerate() {
            let byte = if self.set.fold_case {
                byte.to_ascii_lowercase()
            } else {
                byte
            };
            self.state = self.set.step(self.state, byte);
            self.set.report(self.state, self.offset + i + 1, hits);
        }
        self.offset += chunk.len();
    }

    /// How many bytes have been read so far.
    pub fn offset(&self) -> usize {
        self.offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Lcg;
    use std::cmp::Reverse;

    /// Every match by checking every pattern at every position, in the order `find_all` promises.
    fn naive(patterns: &[Vec<u8>], text: &[u8], fold: bool) -> Vec<Hit> {
        let same = |a: u8, b: u8| {
            if fold {
                a.eq_ignore_ascii_case(&b)
            } else {
                a == b
            }
        };
        let mut hits = Vec::new();
        for (pattern, p) in patterns.iter().enumerate() {
            if p.is_empty() || p.len() > text.len() {
                continue;
            }
            for start in 0..=text.len() - p.len() {
                if p.iter().zip(&text[start..]).all(|(&x, &y)| same(x, y)) {
                    hits.push(Hit {
                        pattern,
                        start,
                        end: start + p.len(),
                    });
                }
            }
        }
        hits.sort_by_key(|h| (h.end, Reverse(h.end - h.start), h.pattern));
        hits
    }

    fn random_bytes(rng: &mut Lcg, len: u64, alphabet: &[u8]) -> Vec<u8> {
        (0..rng.below(len + 1))
            .map(|_| alphabet[rng.below(alphabet.len() as u64) as usize])
            .collect()
    }

    #[test]
    fn the_classic_example() {
        let set = PatternSet::new(["he", "she", "his", "hers"]).unwrap();
        let hits = set.find_all("ushers");
        assert_eq!(
            hits,
            vec![
                Hit {
                    pattern: 1,
                    start: 1,
                    end: 4
                },
                Hit {
                    pattern: 0,
                    start: 2,
                    end: 4
                },
                Hit {
                    pattern: 3,
                    start: 2,
                    end: 6
                },
            ]
        );
        assert_eq!(set.present("ushers"), vec![0, 1, 3]);
        assert_eq!(set.len(), 4);
        assert_eq!(set.pattern_len(3), Some(4));
    }

    #[test]
    fn matches_checking_every_position() {
        let mut rng = Lcg(107);
        for round in 0..600 {
            let fold = round % 3 == 0;
            let alphabet: &[u8] = if fold { b"abAB" } else { b"ab" };
            let patterns: Vec<Vec<u8>> = (0..1 + rng.below(8))
                .map(|_| random_bytes(&mut rng, 4, alphabet))
                .collect();
            let text = random_bytes(&mut rng, 40, alphabet);
            let set = if fold {
                PatternSet::new_ascii_case_insensitive(&patterns).unwrap()
            } else {
                PatternSet::new(&patterns).unwrap()
            };
            let expected = naive(&patterns, &text, fold);
            assert_eq!(set.find_all(&text), expected, "{patterns:?} in {text:?}");
            let mut present: Vec<usize> = expected.iter().map(|h| h.pattern).collect();
            present.sort_unstable();
            present.dedup();
            assert_eq!(set.present(&text), present);
            // The same text in random chunks gives the same hits.
            let mut scanner = set.scanner();
            let mut hits = Vec::new();
            let mut rest: &[u8] = &text;
            while !rest.is_empty() {
                let cut = 1 + rng.below(rest.len() as u64) as usize;
                scanner.feed(&rest[..cut], &mut hits);
                rest = &rest[cut..];
            }
            assert_eq!(hits, expected);
            assert_eq!(scanner.offset(), text.len());
        }
    }

    #[test]
    fn edge_cases() {
        let empty = PatternSet::new(Vec::<&str>::new()).unwrap();
        assert!(empty.is_empty());
        assert!(empty.find_all("anything").is_empty());
        let set = PatternSet::new(["", "a", "a"]).unwrap();
        assert_eq!(
            set.find_all("ba"),
            vec![
                Hit {
                    pattern: 1,
                    start: 1,
                    end: 2
                },
                Hit {
                    pattern: 2,
                    start: 1,
                    end: 2
                },
            ]
        );
        // UTF-8: whole-pattern matches land on character boundaries, and folding leaves É alone.
        let text = "Le café, CAFÉ, Café";
        let folded = PatternSet::new_ascii_case_insensitive(["café"]).unwrap();
        let found: Vec<&str> = folded
            .find_all(text)
            .iter()
            .map(|h| &text[h.start..h.end])
            .collect();
        assert_eq!(found, vec!["café", "Café"]);
    }
}
