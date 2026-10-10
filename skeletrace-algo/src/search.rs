// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Binary search: find a place in sorted data by halving the range each step. About log2(N) looks, so about
//! 30 for a billion entries.
//!
//! **SkeleTrace use:** time-window lookups in sorted event lists, and [`Ipv4RangeTable`] for IP-to-ASN or
//! IP-to-country datasets, which come as sorted address ranges.

use std::fmt;
use std::net::Ipv4Addr;

/// The first index whose item is not "before", for a slice that holds all "before" items first and all others
/// after. Every other function here is built on this one.
pub fn partition_point<T>(sorted: &[T], mut is_before: impl FnMut(&T) -> bool) -> usize {
    // Invariant: everything left of `low` is before; everything at or right of `high` is not.
    let (mut low, mut high) = (0, sorted.len());
    while low < high {
        let mid = low + (high - low) / 2; // written this way, the midpoint can't overflow
        if is_before(&sorted[mid]) {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}

/// The first index whose item is `>= value` (where `value` would go, before any equal items).
pub fn lower_bound<T: Ord>(sorted: &[T], value: &T) -> usize {
    partition_point(sorted, |item| item < value)
}

/// The first index whose item is `> value` (where `value` would go, after any equal items).
pub fn upper_bound<T: Ord>(sorted: &[T], value: &T) -> usize {
    partition_point(sorted, |item| item <= value)
}

/// The index of an item equal to `value`, if there is one (the first, when there are several).
pub fn find<T: Ord>(sorted: &[T], value: &T) -> Option<usize> {
    let i = lower_bound(sorted, value);
    if i < sorted.len() && sorted[i] == *value {
        Some(i)
    } else {
        None
    }
}

/// How many items fall in `low..=high`: two binary searches, O(log N).
pub fn count_in_range<T: Ord>(sorted: &[T], low: &T, high: &T) -> usize {
    upper_bound(sorted, high).saturating_sub(lower_bound(sorted, low))
}

/// Why an address range was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RangeError {
    /// The range starts after it ends.
    Reversed {
        /// First address given.
        start: Ipv4Addr,
        /// Last address given.
        end: Ipv4Addr,
    },
    /// Two ranges share addresses, so a lookup would be ambiguous.
    Overlap {
        /// The earlier range.
        first: (Ipv4Addr, Ipv4Addr),
        /// The range that overlaps it.
        second: (Ipv4Addr, Ipv4Addr),
    },
    /// A CIDR prefix length above 32.
    BadPrefix(u8),
}

impl fmt::Display for RangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RangeError::Reversed { start, end } => {
                write!(f, "range starts at {start}, after its end {end}")
            }
            RangeError::Overlap { first, second } => {
                write!(
                    f,
                    "ranges {}-{} and {}-{} overlap",
                    first.0, first.1, second.0, second.1
                )
            }
            RangeError::BadPrefix(p) => write!(f, "CIDR prefix /{p} is longer than 32 bits"),
        }
    }
}

impl std::error::Error for RangeError {}

/// The first and last address of a CIDR block: `192.0.2.0/24` gives `192.0.2.0` and `192.0.2.255`. Host bits
/// in `network` are ignored.
pub fn cidr_bounds(network: Ipv4Addr, prefix: u8) -> Result<(Ipv4Addr, Ipv4Addr), RangeError> {
    if prefix > 32 {
        return Err(RangeError::BadPrefix(prefix));
    }
    let mask: u32 = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    let start = u32::from(network) & mask;
    Ok((Ipv4Addr::from(start), Ipv4Addr::from(start | !mask)))
}

/// Non-overlapping IPv4 ranges, each with a label (an ASN, a country, an operator), looked up in O(log N).
pub struct Ipv4RangeTable<L> {
    starts: Vec<u32>,
    ends: Vec<u32>,
    labels: Vec<L>,
}

impl<L> Ipv4RangeTable<L> {
    /// Builds a table from (first address, last address, label) triples in any order. Rejects reversed and
    /// overlapping ranges. O(N log N).
    pub fn new(mut ranges: Vec<(Ipv4Addr, Ipv4Addr, L)>) -> Result<Self, RangeError> {
        for (start, end, _) in &ranges {
            if u32::from(*start) > u32::from(*end) {
                return Err(RangeError::Reversed {
                    start: *start,
                    end: *end,
                });
            }
        }
        ranges.sort_by_key(|(start, _, _)| u32::from(*start));
        for pair in ranges.windows(2) {
            if u32::from(pair[1].0) <= u32::from(pair[0].1) {
                return Err(RangeError::Overlap {
                    first: (pair[0].0, pair[0].1),
                    second: (pair[1].0, pair[1].1),
                });
            }
        }
        let mut table = Self {
            starts: Vec::with_capacity(ranges.len()),
            ends: Vec::with_capacity(ranges.len()),
            labels: Vec::with_capacity(ranges.len()),
        };
        for (start, end, label) in ranges {
            table.starts.push(u32::from(start));
            table.ends.push(u32::from(end));
            table.labels.push(label);
        }
        Ok(table)
    }

    /// Number of ranges.
    pub fn len(&self) -> usize {
        self.labels.len()
    }

    /// True when the table holds no ranges.
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }

    /// The label of the range containing `ip`, if any. O(log N): find the last range starting at or before
    /// `ip`, then check that it reaches far enough.
    pub fn lookup(&self, ip: Ipv4Addr) -> Option<&L> {
        let address = u32::from(ip);
        let candidate = upper_bound(&self.starts, &address).checked_sub(1)?;
        if address <= self.ends[candidate] {
            Some(&self.labels[candidate])
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_and_find() {
        let v = [1, 3, 3, 3, 7, 9];
        assert_eq!(lower_bound(&v, &3), 1);
        assert_eq!(upper_bound(&v, &3), 4);
        assert_eq!(lower_bound(&v, &0), 0);
        assert_eq!(upper_bound(&v, &10), 6);
        assert_eq!(find(&v, &7), Some(4));
        assert_eq!(find(&v, &4), None);
        assert_eq!(count_in_range(&v, &2, &7), 4);
        assert_eq!(count_in_range(&v, &8, &2), 0);
        assert_eq!(partition_point(&[true, true, false], |&b| b), 2);
        assert_eq!(find::<i32>(&[], &1), None);
    }

    #[test]
    fn cidr() {
        let (a, b) = cidr_bounds(Ipv4Addr::new(192, 0, 2, 77), 24).unwrap();
        assert_eq!(
            (a, b),
            (Ipv4Addr::new(192, 0, 2, 0), Ipv4Addr::new(192, 0, 2, 255))
        );
        assert_eq!(
            cidr_bounds(Ipv4Addr::new(10, 1, 2, 3), 0).unwrap(),
            (Ipv4Addr::new(0, 0, 0, 0), Ipv4Addr::BROADCAST)
        );
        let host = Ipv4Addr::new(203, 0, 113, 9);
        assert_eq!(cidr_bounds(host, 32).unwrap(), (host, host));
        assert_eq!(cidr_bounds(host, 33), Err(RangeError::BadPrefix(33)));
    }

    #[test]
    fn range_table_lookups() {
        let (a0, a1) = cidr_bounds(Ipv4Addr::new(198, 51, 100, 0), 24).unwrap();
        let (b0, b1) = cidr_bounds(Ipv4Addr::new(203, 0, 113, 0), 24).unwrap();
        let table = Ipv4RangeTable::new(vec![(b0, b1, "AS64501"), (a0, a1, "AS64500")]).unwrap();
        assert_eq!(table.len(), 2);
        assert_eq!(
            table.lookup(Ipv4Addr::new(198, 51, 100, 0)),
            Some(&"AS64500")
        );
        assert_eq!(
            table.lookup(Ipv4Addr::new(198, 51, 100, 255)),
            Some(&"AS64500")
        );
        assert_eq!(
            table.lookup(Ipv4Addr::new(203, 0, 113, 42)),
            Some(&"AS64501")
        );
        assert_eq!(table.lookup(Ipv4Addr::new(198, 51, 101, 0)), None);
        assert_eq!(table.lookup(Ipv4Addr::new(1, 1, 1, 1)), None);
    }

    #[test]
    fn range_table_rejects_bad_input() {
        let x = Ipv4Addr::new(10, 0, 0, 10);
        let y = Ipv4Addr::new(10, 0, 0, 20);
        assert!(matches!(
            Ipv4RangeTable::new(vec![(y, x, ())]),
            Err(RangeError::Reversed { .. })
        ));
        let z = Ipv4Addr::new(10, 0, 0, 15);
        let w = Ipv4Addr::new(10, 0, 0, 30);
        assert!(matches!(
            Ipv4RangeTable::new(vec![(x, y, 1), (z, w, 2)]),
            Err(RangeError::Overlap { .. })
        ));
    }
}
