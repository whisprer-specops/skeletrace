// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Complexity analysis in plain terms.
//!
//! Big-O describes how work grows as the input grows, ignoring constant factors. If doubling the input
//! doubles the work, that's O(N); if it quadruples the work, O(N²); if it adds a single step, O(log N).
//!
//! | N | log2 N | N log2 N | N² |
//! |---:|---:|---:|---:|
//! | 1,000 | 10 | 10,000 | 1,000,000 |
//! | 1,000,000 | 20 | 20,000,000 | 10^12 |
//! | 1,000,000,000 | 30 | 3 × 10^10 | 10^18 |
//!
//! At a billion items, an O(log N) lookup takes about 30 steps, an O(N) scan a billion, and comparing every
//! pair, O(N²), longer than anyone will wait. That gap is why the range queries here use Fenwick and segment
//! trees, and why time lookups use binary search.
//!
//! Costs in this crate:
//!
//! | Operation | Cost |
//! |---|---|
//! | BFS, DFS, components, weak points | O(V + E) |
//! | Topological sort (smallest order first) | O(V log V + E) |
//! | Union-find `union`, `find` | O(α(N)): effectively constant |
//! | Heap `push`, `pop`, `peek` | O(log N), O(log N), O(1) |
//! | Merging K sorted lists of N items in total | O(N log K) |
//! | Fenwick update, prefix sum | O(log N) |
//! | Segment tree update, range query | O(log N) |
//! | Binary search | O(log N) |
//! | Selection, insertion, bubble sort | O(N²) |
//! | Merge sort | O(N log N) |
//! | Sorting when every item is at most k places out | O(N log k) |
//! | Largest gap, Kadane, MEX | O(N) |
//! | Contained intervals, closest pair of points | O(N log N) |
//!
//! To check a cost by experiment, time an operation at several sizes and fit the exponent with
//! [`growth_exponent`]. `examples/growth.rs` does this for a naive prefix sum against a Fenwick tree.

/// Fits `time ≈ c × n^k` to `(n, time)` measurements and returns `k`: about 1 for linear work, about 2 for
/// quadratic, near 0 for logarithmic or constant work. A least-squares line through `(ln n, ln time)`.
///
/// Measurements with a non-positive or non-finite value are skipped. `None` if fewer than two remain, or if all
/// the remaining `n` are equal.
pub fn growth_exponent(measurements: &[(f64, f64)]) -> Option<f64> {
    let points: Vec<(f64, f64)> = measurements
        .iter()
        .filter(|(n, t)| n.is_finite() && t.is_finite() && *n > 0.0 && *t > 0.0)
        .map(|&(n, t)| (n.ln(), t.ln()))
        .collect();
    if points.len() < 2 {
        return None;
    }
    let count = points.len() as f64;
    let mean_x = points.iter().map(|p| p.0).sum::<f64>() / count;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / count;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for &(x, y) in &points {
        sxy += (x - mean_x) * (y - mean_y);
        sxx += (x - mean_x) * (x - mean_x);
    }
    if sxx <= 0.0 {
        return None;
    }
    Some(sxy / sxx)
}

/// The most looks binary search needs among `n` sorted items: ⌈log2(n + 1)⌉, which is the bit length of `n`.
/// A billion items need 30.
pub fn binary_search_steps(n: u64) -> u32 {
    64 - n.leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponents_of_known_curves() {
        let sizes = [1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0];
        let linear: Vec<(f64, f64)> = sizes.iter().map(|&n| (n, 3.0 * n)).collect();
        let quadratic: Vec<(f64, f64)> = sizes.iter().map(|&n| (n, 0.5 * n * n)).collect();
        let logarithmic: Vec<(f64, f64)> = sizes.iter().map(|&n: &f64| (n, n.log2())).collect();
        assert!((growth_exponent(&linear).unwrap() - 1.0).abs() < 1e-9);
        assert!((growth_exponent(&quadratic).unwrap() - 2.0).abs() < 1e-9);
        assert!(growth_exponent(&logarithmic).unwrap() < 0.2);
        assert_eq!(growth_exponent(&[(10.0, 1.0)]), None);
        assert_eq!(growth_exponent(&[(10.0, 1.0), (10.0, 2.0)]), None);
        assert_eq!(growth_exponent(&[(10.0, 1.0), (-5.0, 2.0)]), None);
    }

    #[test]
    fn binary_search_step_counts() {
        assert_eq!(binary_search_steps(0), 0);
        assert_eq!(binary_search_steps(1), 1);
        assert_eq!(binary_search_steps(3), 2);
        assert_eq!(binary_search_steps(4), 3);
        assert_eq!(binary_search_steps(1_000_000_000), 30);
        assert_eq!(binary_search_steps(u64::MAX), 64);
    }
}
