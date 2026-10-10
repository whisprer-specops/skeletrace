// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Watch O(N) and O(log N) pull apart: time prefix-sum queries done naively (add up the slice, O(N)) and with
//! a Fenwick tree (O(log N)) as N doubles, then fit each growth exponent.
//!
//! Run: cargo run --release --example growth

use skeletrace_algo::complexity::growth_exponent;
use skeletrace_algo::fenwick::Fenwick;
use std::hint::black_box;
use std::time::Instant;

fn main() {
    println!(
        "{:>10} {:>18} {:>18}",
        "N", "naive ns/query", "Fenwick ns/query"
    );
    let (mut naive_points, mut fenwick_points) = (Vec::new(), Vec::new());
    for shift in 12..=20 {
        let n = 1usize << shift;
        let values: Vec<i64> = (0..n as i64).map(|i| i % 7).collect();
        let tree = Fenwick::from_slice(&values);
        let queries = 2_000usize;

        let start = Instant::now();
        let mut total = 0i64;
        for q in 0..queries {
            let end = (q * 7_919) % n + 1;
            total += black_box(&values)[..end].iter().sum::<i64>();
        }
        black_box(total);
        let naive = start.elapsed().as_nanos() as f64 / queries as f64;

        let start = Instant::now();
        let mut total = 0i64;
        for q in 0..queries {
            let end = (q * 7_919) % n + 1;
            total += black_box(&tree).prefix_sum(end);
        }
        black_box(total);
        let fenwick = start.elapsed().as_nanos() as f64 / queries as f64;

        println!("{n:>10} {naive:>18.1} {fenwick:>18.1}");
        naive_points.push((n as f64, naive));
        fenwick_points.push((n as f64, fenwick));
    }
    if let (Some(a), Some(b)) = (
        growth_exponent(&naive_points),
        growth_exponent(&fenwick_points),
    ) {
        println!("\nfitted growth exponent: naive {a:.2} (expect about 1), Fenwick {b:.2} (expect near 0)");
    }
}
