// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Number theory: fast powers, divisors, Fibonacci numbers, integer square roots.

/// `base^exp mod modulus` by repeated squaring: O(log exp) multiplications instead of `exp`. `None` if
/// `modulus` is 0. Products are taken in 128 bits, so any `u64` inputs are safe.
///
/// **SkeleTrace use:** rolling hashes for spotting repeated text fragments.
pub fn pow_mod(base: u64, mut exp: u64, modulus: u64) -> Option<u64> {
    if modulus == 0 {
        return None;
    }
    let m = u128::from(modulus);
    let mut result: u128 = 1 % m;
    let mut square = u128::from(base) % m;
    while exp > 0 {
        if exp & 1 == 1 {
            result = result * square % m;
        }
        square = square * square % m;
        exp >>= 1;
    }
    u64::try_from(result).ok()
}

/// `base^exp` by repeated squaring ("power function"), or `None` if the result overflows `u64`.
pub fn pow_checked(mut base: u64, mut exp: u32) -> Option<u64> {
    let mut result: u64 = 1;
    while exp > 0 {
        if exp & 1 == 1 {
            result = result.checked_mul(base)?;
        }
        exp >>= 1;
        if exp > 0 {
            base = base.checked_mul(base)?; // only square when another bit still needs it
        }
    }
    Some(result)
}

/// Greatest common divisor, by Euclid's algorithm. `gcd(0, 0)` is 0.
pub fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

/// The integer square root: the largest `r` with `r * r <= n`.
pub fn isqrt(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    // A floating-point guess lands within a step or two; integer arithmetic then makes it exact.
    let mut r = (n as f64).sqrt() as u64;
    while !matches!(r.checked_mul(r), Some(square) if square <= n) {
        r -= 1;
    }
    while matches!((r + 1).checked_mul(r + 1), Some(square) if square <= n) {
        r += 1;
    }
    r
}

/// Number of odd divisors of `n` ("odd divisors": 12 has two, 1 and 3). `None` for 0, which every number
/// divides. Strips the factors of 2, then counts divisors from the prime factorisation of what is left.
/// O(√n): about a million steps for `n` near 10^12.
pub fn count_odd_divisors(n: u64) -> Option<u64> {
    if n == 0 {
        return None;
    }
    let mut m = n >> n.trailing_zeros();
    let mut count = 1;
    let mut p = 3;
    while p <= m / p {
        let mut exponent = 0;
        while m % p == 0 {
            m /= p;
            exponent += 1;
        }
        count *= exponent + 1;
        p += 2;
    }
    if m > 1 {
        count *= 2;
    }
    Some(count)
}

/// How many numbers in `low..=high` have an odd number of divisors ("odd divisor count"). Divisors pair up as
/// (d, n / d), so the count is odd only when a pair collapses (d = n / d): exactly the perfect squares. O(1).
pub fn count_with_odd_divisor_count(low: u64, high: u64) -> u64 {
    let low = low.max(1);
    if low > high {
        return 0;
    }
    isqrt(high) - isqrt(low - 1)
}

/// Sum of the greatest odd divisor of every number in `1..=n`. Odd numbers are their own greatest odd divisor,
/// and the odd numbers up to n sum to a square; each even number 2j shares its greatest odd divisor with j, so
/// repeat on n / 2. O(log n).
pub fn sum_of_greatest_odd_divisors(mut n: u64) -> u128 {
    let mut total: u128 = 0;
    while n > 0 {
        let odds = u128::from(n / 2 + n % 2);
        total += odds * odds;
        n /= 2;
    }
    total
}

/// The `n`th Fibonacci number (F(0) = 0, F(1) = 1), or `None` past F(186), the largest that fits in `u128`.
pub fn fibonacci(n: u64) -> Option<u128> {
    if n == 0 {
        return Some(0);
    }
    let (mut previous, mut current) = (0u128, 1u128);
    for _ in 1..n {
        let next = previous.checked_add(current)?;
        previous = current;
        current = next;
    }
    Some(current)
}

/// F(0) + F(1) + … + F(n) ("Fibonacci sum"), using the identity that this equals F(n + 2) − 1. `None` on
/// overflow.
pub fn fibonacci_sum(n: u64) -> Option<u128> {
    Some(fibonacci(n.checked_add(2)?)? - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Lcg;

    #[test]
    fn powers() {
        assert_eq!(pow_mod(2, 10, 1000), Some(24));
        assert_eq!(pow_mod(7, 0, 1), Some(0));
        assert_eq!(pow_mod(5, 3, 0), None);
        assert_eq!(
            pow_mod(3, 1_000_000_006, 1_000_000_007),
            Some(1),
            "Fermat's little theorem"
        );
        assert_eq!(pow_mod(u64::MAX, u64::MAX, u64::MAX), Some(0));
        assert_eq!(pow_checked(3, 4), Some(81));
        assert_eq!(pow_checked(2, 63), Some(1 << 63));
        assert_eq!(pow_checked(2, 64), None);
        assert_eq!(pow_checked(0, 0), Some(1));
        assert_eq!(pow_checked(10, 19), Some(10_000_000_000_000_000_000));
        assert_eq!(pow_checked(10, 20), None);
    }

    #[test]
    fn gcd_and_isqrt() {
        assert_eq!(gcd(12, 18), 6);
        assert_eq!(gcd(0, 5), 5);
        assert_eq!(gcd(0, 0), 0);
        for (n, r) in [
            (0, 0),
            (1, 1),
            (2, 1),
            (3, 1),
            (4, 2),
            (15, 3),
            (16, 4),
            (17, 4),
            (u64::MAX, 4_294_967_295),
        ] {
            assert_eq!(isqrt(n), r, "isqrt({n})");
        }
        let mut rng = Lcg(31);
        for _ in 0..1000 {
            let n = (rng.next_u64() << 33) | rng.next_u64();
            let r = isqrt(n);
            assert!(r * r <= n && (r + 1).checked_mul(r + 1).map_or(true, |s| s > n));
        }
    }

    #[test]
    fn divisors() {
        assert_eq!(count_odd_divisors(12), Some(2));
        assert_eq!(count_odd_divisors(45), Some(6));
        assert_eq!(count_odd_divisors(1), Some(1));
        assert_eq!(count_odd_divisors(1024), Some(1));
        assert_eq!(count_odd_divisors(0), None);
        assert_eq!(count_with_odd_divisor_count(1, 100), 10);
        assert_eq!(count_with_odd_divisor_count(0, 100), 10);
        assert_eq!(count_with_odd_divisor_count(5, 15), 1);
        assert_eq!(count_with_odd_divisor_count(10, 5), 0);
        assert_eq!(sum_of_greatest_odd_divisors(10), 36);
        let brute: u128 = (1..=1000u64)
            .map(|k| u128::from(k >> k.trailing_zeros()))
            .sum();
        assert_eq!(sum_of_greatest_odd_divisors(1000), brute);
    }

    #[test]
    fn fibonacci_numbers() {
        assert_eq!(fibonacci(0), Some(0));
        assert_eq!(fibonacci(1), Some(1));
        assert_eq!(fibonacci(10), Some(55));
        assert_eq!(fibonacci(90), Some(2_880_067_194_370_816_120));
        assert!(fibonacci(186).is_some());
        assert_eq!(fibonacci(187), None);
        assert_eq!(fibonacci_sum(0), Some(0));
        assert_eq!(fibonacci_sum(10), Some(143));
        assert_eq!(fibonacci_sum(u64::MAX), None);
    }
}
