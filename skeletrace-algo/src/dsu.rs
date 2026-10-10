// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Disjoint sets (union-find): "do these belong together?", answered in near-constant time.
//!
//! **SkeleTrace use:** entity resolution. When evidence ties two identifiers to one actor (an email address
//! seen with a handle, a handle with a phone number), call [`DisjointSet::union`]. After that,
//! [`DisjointSet::same`] answers "same actor?" at once, however long the chain of evidence.
//! [`crate::osint::EntityClusters`] wraps this for your own identifier types.
//!
//! **Complexity:** with union by size and path halving, m operations on n elements cost O(m · α(n)), where α
//! is the inverse Ackermann function. α(n) is below 5 for any n that fits in memory, so in practice each
//! operation takes constant time.

/// A forest of disjoint sets over the elements `0..len`.
#[derive(Clone, Debug, Default)]
pub struct DisjointSet {
    parent: Vec<usize>,
    size: Vec<usize>,
    sets: usize,
}

impl DisjointSet {
    /// `len` single-element sets: `{0}`, `{1}`, … `{len - 1}`.
    pub fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
            size: vec![1; len],
            sets: len,
        }
    }

    /// Adds one new single-element set and returns its element.
    pub fn add(&mut self) -> usize {
        let element = self.parent.len();
        self.parent.push(element);
        self.size.push(1);
        self.sets += 1;
        element
    }

    /// Number of elements.
    pub fn len(&self) -> usize {
        self.parent.len()
    }

    /// True when there are no elements.
    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }

    /// Number of separate sets.
    pub fn set_count(&self) -> usize {
        self.sets
    }

    /// The representative of the set holding `x`. Two elements share a set exactly when their representatives
    /// match.
    ///
    /// # Panics
    /// If `x >= self.len()`.
    pub fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            // Path halving: point x at its grandparent, then step there. The tree flattens as we walk.
            let grandparent = self.parent[self.parent[x]];
            self.parent[x] = grandparent;
            x = grandparent;
        }
        x
    }

    /// Merges the sets holding `a` and `b`. Returns `false` if they were already one set.
    ///
    /// # Panics
    /// If `a` or `b` is out of range.
    pub fn union(&mut self, a: usize, b: usize) -> bool {
        let (mut big, mut small) = (self.find(a), self.find(b));
        if big == small {
            return false;
        }
        if self.size[big] < self.size[small] {
            std::mem::swap(&mut big, &mut small);
        }
        // Union by size: hang the smaller tree under the larger one, so trees stay shallow.
        self.parent[small] = big;
        self.size[big] += self.size[small];
        self.sets -= 1;
        true
    }

    /// True when `a` and `b` are in the same set.
    pub fn same(&mut self, a: usize, b: usize) -> bool {
        self.find(a) == self.find(b)
    }

    /// Number of elements in the set holding `x`.
    pub fn set_size(&mut self, x: usize) -> usize {
        let root = self.find(x);
        self.size[root]
    }

    /// Every set as an ascending list of its elements, sets ordered by their smallest element.
    pub fn groups(&mut self) -> Vec<Vec<usize>> {
        let mut slot_of_root = vec![usize::MAX; self.len()];
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for x in 0..self.len() {
            let root = self.find(x);
            if slot_of_root[root] == usize::MAX {
                slot_of_root[root] = groups.len();
                groups.push(Vec::new());
            }
            groups[slot_of_root[root]].push(x);
        }
        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unions_and_finds() {
        let mut d = DisjointSet::new(6);
        assert_eq!(d.set_count(), 6);
        assert!(d.union(0, 1));
        assert!(d.union(1, 2));
        assert!(!d.union(0, 2), "already together");
        assert!(d.union(4, 5));
        assert!(d.same(0, 2));
        assert!(!d.same(2, 3));
        assert_eq!(d.set_count(), 3);
        assert_eq!(d.set_size(1), 3);
        assert_eq!(d.groups(), vec![vec![0, 1, 2], vec![3], vec![4, 5]]);
    }

    #[test]
    fn long_chain_needs_no_recursion() {
        let n = 200_000;
        let mut d = DisjointSet::new(n);
        for i in 1..n {
            d.union(i - 1, i);
        }
        assert_eq!(d.set_count(), 1);
        assert!(d.same(0, n - 1));
    }

    #[test]
    fn grows() {
        let mut d = DisjointSet::new(0);
        let a = d.add();
        let b = d.add();
        assert_eq!((a, b, d.set_count()), (0, 1, 2));
        d.union(a, b);
        assert_eq!(d.set_count(), 1);
    }
}
