// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Stack algorithms: last in, first out. A stack remembers what is still open, or still waiting for an
//! answer, which is exactly what bracket matching and next-greater questions need.

use std::ops::Range;

/// True when every bracket in `text` is closed by the matching bracket in the right order: `()`, `[]`, `{}`.
/// Other characters are ignored. O(N).
pub fn balanced(text: &str) -> bool {
    let mut open = Vec::new();
    for c in text.chars() {
        match c {
            '(' | '[' | '{' => open.push(c),
            ')' | ']' | '}' => {
                let expected = match c {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                if open.pop() != Some(expected) {
                    return false;
                }
            }
            _ => {}
        }
    }
    open.is_empty()
}

/// The longest run of `text` that is a well-formed sequence of `(` and `)` ("longest parenthesised
/// substring"), as a byte range: empty if there is none. Any other character breaks a run. O(N).
///
/// `")()())"` gives `1..5`, the run `"()()"`.
pub fn longest_balanced_run(text: &str) -> Range<usize> {
    let mut open: Vec<usize> = Vec::new(); // positions of '(' not matched yet
    let mut barrier: Option<usize> = None; // the last position no run can cross
    let mut best = 0..0;
    for (i, byte) in text.bytes().enumerate() {
        match byte {
            b'(' => open.push(i),
            b')' => {
                if open.pop().is_some() {
                    // The run now reaches back to just after the nearest thing still unmatched.
                    let start = match open.last() {
                        Some(&unmatched) => unmatched + 1,
                        None => barrier.map_or(0, |b| b + 1),
                    };
                    if i + 1 - start > best.len() {
                        best = start..i + 1;
                    }
                } else {
                    barrier = Some(i);
                }
            }
            _ => {
                open.clear();
                barrier = Some(i);
            }
        }
    }
    best
}

/// For each position, the index of the next item to the right that is strictly greater, or `None`. One pass
/// with a stack of positions still waiting for their answer: O(N), since each position is pushed and popped
/// once. The same pattern answers "how far along the row can each one see" questions.
pub fn next_greater<T: Ord>(values: &[T]) -> Vec<Option<usize>> {
    let mut answer = vec![None; values.len()];
    let mut waiting: Vec<usize> = Vec::new();
    for (i, value) in values.iter().enumerate() {
        while let Some(&top) = waiting.last() {
            if values[top] >= *value {
                break;
            }
            answer[top] = Some(i);
            waiting.pop();
        }
        waiting.push(i);
    }
    answer
}

/// The smallest number left after removing exactly `k` digits from `digits`, keeping the rest in order
/// ("optimal digits removal"), with leading zeros dropped. `None` if `digits` is empty or holds anything but
/// ASCII digits. O(N).
///
/// Greedy, with a stack: while the last kept digit is larger than the next one, removing it makes the number
/// smaller, so remove it.
pub fn smallest_after_removing(digits: &str, k: usize) -> Option<String> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut kept: Vec<u8> = Vec::with_capacity(digits.len());
    let mut remaining = k;
    for d in digits.bytes() {
        while remaining > 0 && kept.last().is_some_and(|&top| top > d) {
            kept.pop();
            remaining -= 1;
        }
        kept.push(d);
    }
    kept.truncate(kept.len().saturating_sub(remaining));
    let first_nonzero = kept.iter().position(|&d| d != b'0').unwrap_or(kept.len());
    let trimmed = &kept[first_nonzero..];
    if trimmed.is_empty() {
        return Some("0".to_string());
    }
    Some(trimmed.iter().map(|&b| char::from(b)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brackets() {
        assert!(balanced("([]{})"));
        assert!(balanced("a(b)c"));
        assert!(balanced(""));
        assert!(!balanced("([)]"));
        assert!(!balanced("(("));
        assert!(!balanced("())"));
    }

    #[test]
    fn longest_run() {
        assert_eq!(longest_balanced_run(")()())"), 1..5);
        assert_eq!(longest_balanced_run("(()"), 1..3);
        assert_eq!(longest_balanced_run("()(()"), 0..2);
        assert_eq!(longest_balanced_run("()(())"), 0..6);
        assert_eq!(longest_balanced_run("(x)()"), 3..5);
        assert_eq!(longest_balanced_run(""), 0..0);
        assert_eq!(longest_balanced_run(")("), 0..0);
        let text = "é(())";
        let run = longest_balanced_run(text);
        assert_eq!(&text[run], "(())");
    }

    #[test]
    fn next_greater_element() {
        assert_eq!(
            next_greater(&[2, 1, 2, 4, 3]),
            vec![Some(3), Some(2), Some(3), None, None]
        );
        assert!(next_greater::<i32>(&[]).is_empty());
    }

    #[test]
    fn digit_removal() {
        assert_eq!(
            smallest_after_removing("1432219", 3).as_deref(),
            Some("1219")
        );
        assert_eq!(smallest_after_removing("10200", 1).as_deref(), Some("200"));
        assert_eq!(smallest_after_removing("10", 2).as_deref(), Some("0"));
        assert_eq!(smallest_after_removing("12345", 2).as_deref(), Some("123"));
        assert_eq!(smallest_after_removing("9", 0).as_deref(), Some("9"));
        assert_eq!(smallest_after_removing("12a", 1), None);
        assert_eq!(smallest_after_removing("", 0), None);
    }
}
