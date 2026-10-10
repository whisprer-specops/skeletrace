// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Combinatorics: walking through permutations and combinations, and generating every balanced bracket string,
//! binary search tree and look-and-say term.
//!
//! Counts grow fast: n items have n! orderings (10! is 3,628,800), and `bst_preorders(n)` returns the Catalan
//! number C(n) of sequences (C(15) is 9,694,845). Keep inputs small.

#![allow(clippy::needless_range_loop)] // index loops read more clearly here

/// Rearranges `items` into the next permutation in dictionary order and returns `true`, or wraps the last one
/// round to the first (ascending) and returns `false`. O(N).
///
/// From the right, find the first item smaller than its right neighbour (the pivot); swap it with the
/// rightmost item larger than it; reverse everything after the pivot's position.
pub fn next_permutation<T: Ord>(items: &mut [T]) -> bool {
    let len = items.len();
    if len < 2 {
        return false;
    }
    let mut i = len - 1;
    while i > 0 && items[i - 1] >= items[i] {
        i -= 1;
    }
    if i == 0 {
        items.reverse();
        return false;
    }
    let pivot = i - 1;
    let mut j = len - 1;
    while items[j] <= items[pivot] {
        j -= 1;
    }
    items.swap(pivot, j);
    items[i..].reverse();
    true
}

/// Rearranges `items` into the previous permutation in dictionary order ("previous permutation") and returns
/// `true`, or wraps the first one round to the last (descending) and returns `false`. O(N). The mirror image of
/// [`next_permutation`].
pub fn prev_permutation<T: Ord>(items: &mut [T]) -> bool {
    let len = items.len();
    if len < 2 {
        return false;
    }
    let mut i = len - 1;
    while i > 0 && items[i - 1] <= items[i] {
        i -= 1;
    }
    if i == 0 {
        items.reverse();
        return false;
    }
    let pivot = i - 1;
    let mut j = len - 1;
    while items[j] >= items[pivot] {
        j -= 1;
    }
    items.swap(pivot, j);
    items[i..].reverse();
    true
}

/// Every distinct ordering of `items` in dictionary order, with repeated items handled ("permutations with
/// duplicates"): `[1, 1, 2]` gives three orderings, not six.
pub fn unique_permutations<T: Ord + Clone>(items: &[T]) -> Vec<Vec<T>> {
    let mut current = items.to_vec();
    current.sort();
    let mut all = vec![current.clone()];
    while next_permutation(&mut current) {
        all.push(current.clone());
    }
    all
}

/// Every distinct rearrangement of the characters of `word`, in dictionary order ("word permutations").
pub fn word_permutations(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    unique_permutations(&chars)
        .into_iter()
        .map(|p| p.into_iter().collect())
        .collect()
}

/// Every way to choose `k` of the indices `0..n`, each ascending, in dictionary order ("generate
/// combinations"). `k = 0` gives one empty choice; `k > n` gives none.
pub fn combinations(n: usize, k: usize) -> Vec<Vec<usize>> {
    if k > n {
        return Vec::new();
    }
    let mut current: Vec<usize> = (0..k).collect();
    let mut all = Vec::new();
    loop {
        all.push(current.clone());
        // The rightmost position that can still move right without running out of room.
        let Some(i) = (0..k).rev().find(|&i| current[i] < n - k + i) else {
            return all;
        };
        current[i] += 1;
        for j in i + 1..k {
            current[j] = current[j - 1] + 1;
        }
    }
}

/// Every way to choose `k` of `items`, in the order of [`combinations`].
pub fn combinations_of<T: Clone>(items: &[T], k: usize) -> Vec<Vec<T>> {
    combinations(items.len(), k)
        .into_iter()
        .map(|c| c.into_iter().map(|i| items[i].clone()).collect())
        .collect()
}

/// Every well-formed string of `pairs` pairs of brackets, in dictionary order ("generate parentheses").
/// Backtracking: add `(` while any are left; add `)` only while it would close something open.
pub fn balanced_parentheses(pairs: usize) -> Vec<String> {
    let mut all = Vec::new();
    let mut current = String::with_capacity(2 * pairs);
    extend_brackets(&mut current, pairs, pairs, &mut all);
    all
}

fn extend_brackets(
    current: &mut String,
    opens_left: usize,
    closes_left: usize,
    all: &mut Vec<String>,
) {
    if closes_left == 0 {
        all.push(current.clone());
        return;
    }
    if opens_left > 0 {
        current.push('(');
        extend_brackets(current, opens_left - 1, closes_left, all);
        current.pop();
    }
    if closes_left > opens_left {
        current.push(')');
        extend_brackets(current, opens_left, closes_left - 1, all);
        current.pop();
    }
}

/// The preorder sequence (root, then left subtree, then right subtree) of every binary search tree holding the
/// keys `1..=n` ("all BST preorders"). Each key in turn is the root; the smaller keys form the left subtree and
/// the larger ones the right, so the answer combines the answers for those two ranges.
pub fn bst_preorders(n: usize) -> Vec<Vec<usize>> {
    preorders(1, n)
}

fn preorders(low: usize, high: usize) -> Vec<Vec<usize>> {
    if low > high {
        return vec![Vec::new()];
    }
    let mut all = Vec::new();
    for root in low..=high {
        let lefts = preorders(low, root - 1);
        let rights = preorders(root + 1, high);
        for left in &lefts {
            for right in &rights {
                let mut sequence = Vec::with_capacity(1 + left.len() + right.len());
                sequence.push(root);
                sequence.extend_from_slice(left);
                sequence.extend_from_slice(right);
                all.push(sequence);
            }
        }
    }
    all
}

/// The term after `term` in a look-and-say sequence: read each run aloud ("1211" is "one 1, one 2, two 1s"),
/// giving "111221". This is run-length encoding written as text; see [`crate::arrays::run_lengths`].
pub fn look_and_say(term: &str) -> String {
    let mut out = String::new();
    let mut chars = term.chars().peekable();
    while let Some(c) = chars.next() {
        let mut run = 1;
        while chars.peek() == Some(&c) {
            chars.next();
            run += 1;
        }
        out.push_str(&run.to_string());
        out.push(c);
    }
    out
}

/// The first `terms` terms of the look-and-say sequence starting from `first`.
pub fn look_and_say_sequence(first: &str, terms: usize) -> Vec<String> {
    let mut all = Vec::with_capacity(terms);
    let mut current = first.to_string();
    for _ in 0..terms {
        let next = look_and_say(&current);
        all.push(std::mem::replace(&mut current, next));
    }
    all
}

/// The largest number you can write by joining `numbers` end to end ("largest concatenated number"). Put `a`
/// before `b` when "ab" beats "ba": that ordering is consistent, so sorting by it works. All zeros give "0".
pub fn largest_concatenation(numbers: &[u64]) -> String {
    let mut parts: Vec<String> = numbers.iter().map(u64::to_string).collect();
    parts.sort_by(|a, b| format!("{b}{a}").cmp(&format!("{a}{b}")));
    let joined = parts.concat();
    if joined.starts_with('0') {
        "0".to_string()
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permutations_forwards_and_backwards() {
        let mut p = vec![1, 2, 3];
        let mut seen = vec![p.clone()];
        while next_permutation(&mut p) {
            seen.push(p.clone());
        }
        assert_eq!(seen.len(), 6);
        assert_eq!(p, vec![1, 2, 3], "wrapped round to the first");
        let mut q = vec![1, 3, 2];
        assert!(prev_permutation(&mut q));
        assert_eq!(q, vec![1, 2, 3]);
        assert!(!prev_permutation(&mut q));
        assert_eq!(q, vec![3, 2, 1], "wrapped round to the last");
        let mut r = vec![2, 1, 3];
        assert!(prev_permutation(&mut r));
        assert_eq!(r, vec![1, 3, 2]);
    }

    #[test]
    fn unique_and_word_permutations() {
        assert_eq!(
            unique_permutations(&[1, 1, 2]),
            vec![vec![1, 1, 2], vec![1, 2, 1], vec![2, 1, 1]]
        );
        assert_eq!(word_permutations("aab"), vec!["aab", "aba", "baa"]);
        assert_eq!(word_permutations("abcd").len(), 24);
        assert_eq!(word_permutations(""), vec![String::new()]);
    }

    #[test]
    fn choosing() {
        assert_eq!(
            combinations(4, 2),
            vec![
                vec![0, 1],
                vec![0, 2],
                vec![0, 3],
                vec![1, 2],
                vec![1, 3],
                vec![2, 3]
            ]
        );
        assert_eq!(combinations(3, 0), vec![Vec::<usize>::new()]);
        assert!(combinations(2, 3).is_empty());
        assert_eq!(combinations(10, 3).len(), 120);
        assert_eq!(
            combinations_of(&['a', 'b', 'c'], 2),
            vec![vec!['a', 'b'], vec!['a', 'c'], vec!['b', 'c']]
        );
    }

    #[test]
    fn brackets_and_trees() {
        assert_eq!(
            balanced_parentheses(3),
            vec!["((()))", "(()())", "(())()", "()(())", "()()()"]
        );
        assert_eq!(balanced_parentheses(0), vec![String::new()]);
        assert_eq!(balanced_parentheses(6).len(), 132);
        assert_eq!(
            bst_preorders(3),
            vec![
                vec![1, 2, 3],
                vec![1, 3, 2],
                vec![2, 1, 3],
                vec![3, 1, 2],
                vec![3, 2, 1]
            ]
        );
        assert_eq!(bst_preorders(7).len(), 429);
    }

    #[test]
    fn look_and_say_terms() {
        assert_eq!(
            look_and_say_sequence("1", 6),
            vec!["1", "11", "21", "1211", "111221", "312211"]
        );
        assert_eq!(look_and_say(""), "");
    }

    #[test]
    fn largest_concatenated_number() {
        assert_eq!(largest_concatenation(&[3, 30, 34, 5, 9]), "9534330");
        assert_eq!(largest_concatenation(&[0, 0]), "0");
        assert_eq!(largest_concatenation(&[]), "");
        assert_eq!(largest_concatenation(&[10, 2]), "210");
    }
}
