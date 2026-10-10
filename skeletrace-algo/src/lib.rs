// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! # skeletrace-algo
//!
//! Std-only building blocks for SkeleTrace: graph traversal, ordering, searching, range queries, and the OSINT
//! helpers built on them. No dependencies and no `unsafe`.
//!
//! The algorithms are textbook ideas. The code is original: nothing was copied from the sources consulted.
//!
//! | Module | What it gives SkeleTrace |
//! |---|---|
//! | [`graph`] | BFS (how is A connected to B, k-hop neighbourhoods), DFS, topological order, components, weak points, Dijkstra's cheapest routes |
//! | [`dsu`] | Union-find: merging identifiers that belong to one actor |
//! | [`heap`] | A min-heap with visible sift-up and sift-down, and top-k |
//! | [`merge`] | Merging K time-sorted feeds into one timeline |
//! | [`fenwick`] | Running totals over time buckets, O(log N) per update and per query |
//! | [`segtree`] | Range minimum, maximum or sum with point updates, O(log N) each |
//! | [`search`] | Binary search, and IPv4 range tables (IP to ASN or country lookups) |
//! | [`fuzzy`] | Edit distances, Jaro-Winkler scores and near-miss sets, for handles, names and watch lists with typos |
//! | [`textsearch`] | Aho-Corasick: a whole watch list found in one pass over a dump, read in chunks |
//! | [`sort`] | Selection, insertion, bubble and merge sort, and sorting nearly sorted feeds in O(N log k) |
//! | [`stack`] | Bracket checks, the longest balanced run, next greater element, smallest number after removing digits |
//! | [`arrays`] | Intersections, contained intervals, largest gap, max and min subarray, the shortest stretch from lowest to highest, MEX, duplicates, anagrams |
//! | [`matrix`] | Rotation, largest square, largest rectangle, largest connected zone |
//! | [`geo`] | Closest pair of points, great-circle distance, geofences, convex hulls, areas on the Earth, cells equally far from two points on a grid |
//! | [`bits`] | Bit counts, lowest set bit, Hamming distance, Gray-code cycles |
//! | [`numtheory`] | Fast powers, divisor counts, Fibonacci numbers and sums, integer square roots |
//! | [`combinatorics`] | Next and previous permutation, unique permutations, combinations, balanced brackets, BST preorders, look-and-say, largest concatenation |
//! | [`osint`] | Time-bucket counts, entity clusters, near-duplicate image hashes, username candidates, silences and bursts, the strongest chain of evidence |
//! | [`complexity`] | Big-O in plain terms, and measuring growth |

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod arrays;
pub mod bits;
pub mod combinatorics;
pub mod complexity;
pub mod dsu;
pub mod fenwick;
pub mod fuzzy;
pub mod geo;
pub mod graph;
pub mod heap;
pub mod matrix;
pub mod merge;
pub mod numtheory;
pub mod osint;
pub mod search;
pub mod segtree;
pub mod sort;
pub mod stack;
pub mod textsearch;

#[cfg(test)]
pub(crate) mod testutil {
    /// A tiny deterministic pseudo-random generator, so tests repeat exactly without a dependency.
    pub struct Lcg(pub u64);

    impl Lcg {
        pub fn next_u64(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }

        pub fn below(&mut self, n: u64) -> u64 {
            self.next_u64() % n
        }
    }
}
