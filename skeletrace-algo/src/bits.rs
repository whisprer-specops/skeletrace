// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Bit tricks: counting set bits, isolating the lowest one, Hamming distance, Gray codes.
//!
//! Two identities do most of the work. `x & x.wrapping_neg()` keeps only the lowest set bit (in two's
//! complement, negating flips every bit above the lowest set one). `x & (x - 1)` clears the lowest set bit.
//!
//! **SkeleTrace use:** comparing 64-bit perceptual image hashes. The number of differing bits, the Hamming
//! distance, says how alike two pictures are; see [`crate::osint::similar_hashes`].

/// Number of set bits ("bit count"), by clearing the lowest set bit until none are left (Kernighan's method):
/// one step per set bit. `u64::count_ones` does the same in one CPU instruction; this shows the idea.
pub fn bit_count(mut x: u64) -> u32 {
    let mut count = 0;
    while x != 0 {
        x &= x - 1;
        count += 1;
    }
    count
}

/// The lowest set bit as a value ("LSB"): `0b10100` gives `0b100`, and 0 gives 0.
pub fn lowest_set_bit(x: u64) -> u64 {
    x & x.wrapping_neg()
}

/// Position of the lowest set bit: `0b10100` gives `Some(2)`, and 0 gives `None`.
pub fn lowest_set_bit_index(x: u64) -> Option<u32> {
    if x == 0 {
        None
    } else {
        Some(x.trailing_zeros())
    }
}

/// `x` with its lowest set bit cleared: `0b10100` gives `0b10000`.
pub fn clear_lowest_set_bit(x: u64) -> u64 {
    x & x.wrapping_sub(1)
}

/// Positions of the set bits, lowest first.
pub fn set_bit_positions(x: u64) -> SetBitPositions {
    SetBitPositions(x)
}

/// Iterator returned by [`set_bit_positions`].
#[derive(Clone, Copy, Debug)]
pub struct SetBitPositions(u64);

impl Iterator for SetBitPositions {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        if self.0 == 0 {
            return None;
        }
        let position = self.0.trailing_zeros();
        self.0 &= self.0 - 1;
        Some(position)
    }
}

/// Number of bit positions where `a` and `b` differ.
pub fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// All `2^bits` values of `bits` bits, arranged in a cycle where neighbours (last and first included) differ in
/// exactly one bit, starting at `start`: a reflected Gray code shifted by XOR ("circular permutation"). `None`
/// if `bits > 20` (over a million entries) or `start` does not fit in `bits` bits.
pub fn gray_code_cycle(bits: u32, start: u64) -> Option<Vec<u64>> {
    if bits > 20 || start >= 1u64 << bits {
        return None;
    }
    Some((0..1u64 << bits).map(|i| start ^ i ^ (i >> 1)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Lcg;

    #[test]
    fn counting_and_lowest_bits() {
        let mut rng = Lcg(21);
        for _ in 0..1000 {
            let x = (rng.next_u64() << 31) | rng.next_u64();
            assert_eq!(bit_count(x), x.count_ones());
        }
        assert_eq!(bit_count(0), 0);
        assert_eq!(lowest_set_bit(0b10100), 0b100);
        assert_eq!(lowest_set_bit(0), 0);
        assert_eq!(lowest_set_bit_index(12), Some(2));
        assert_eq!(lowest_set_bit_index(0), None);
        assert_eq!(clear_lowest_set_bit(0b10100), 0b10000);
        assert_eq!(clear_lowest_set_bit(0), 0);
        assert_eq!(
            set_bit_positions(0b100101).collect::<Vec<_>>(),
            vec![0, 2, 5]
        );
        assert_eq!(set_bit_positions(u64::MAX).count(), 64);
        assert_eq!(hamming_distance(0b1011, 0b0001), 2);
    }

    #[test]
    fn gray_cycle_visits_everything_one_bit_at_a_time() {
        for bits in 0..=6 {
            let start = if bits == 0 { 0 } else { 5 % (1u64 << bits) };
            let cycle = gray_code_cycle(bits, start).unwrap();
            assert_eq!(cycle.len(), 1 << bits);
            assert_eq!(cycle[0], start);
            let mut sorted = cycle.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, (0..1u64 << bits).collect::<Vec<_>>());
            if cycle.len() > 1 {
                for i in 0..cycle.len() {
                    let next = cycle[(i + 1) % cycle.len()];
                    assert_eq!(hamming_distance(cycle[i], next), 1);
                }
            }
        }
        assert_eq!(gray_code_cycle(3, 8), None);
        assert_eq!(gray_code_cycle(21, 0), None);
    }
}
