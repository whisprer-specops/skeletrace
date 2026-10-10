// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Grids stored as a `Vec` of rows: rotation, and finding the largest square, rectangle or connected zone.
//!
//! **SkeleTrace use:** pattern-of-life grids. Lay activity out as days × hours, `true` where something
//! happened: [`largest_rectangle`] finds the biggest block of steady activity, and [`largest_zone`] the biggest
//! connected patch of equal cells.

#![allow(clippy::needless_range_loop)] // row and column index loops read more clearly here

use std::collections::VecDeque;
use std::fmt;

/// A grid whose rows are not all the same length (or, for in-place rotation, a grid that isn't square).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RaggedError {
    /// The first row that doesn't fit.
    pub row: usize,
    /// The length it should have had.
    pub expected: usize,
    /// The length it has.
    pub found: usize,
}

impl fmt::Display for RaggedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "row {} has {} cells where {} were expected",
            self.row, self.found, self.expected
        )
    }
}

impl std::error::Error for RaggedError {}

/// Column count, after checking every row has it.
fn width<T>(grid: &[Vec<T>]) -> Result<usize, RaggedError> {
    let expected = grid.first().map_or(0, Vec::len);
    for (row, cells) in grid.iter().enumerate() {
        if cells.len() != expected {
            return Err(RaggedError {
                row,
                expected,
                found: cells.len(),
            });
        }
    }
    Ok(expected)
}

/// The grid turned a quarter clockwise ("matrix rotation"): R rows × C columns become C × R.
pub fn rotate_clockwise<T: Clone>(grid: &[Vec<T>]) -> Result<Vec<Vec<T>>, RaggedError> {
    let cols = width(grid)?;
    let rows = grid.len();
    // new[r][c] = old[rows - 1 - c][r]
    Ok((0..cols)
        .map(|r| (0..rows).map(|c| grid[rows - 1 - c][r].clone()).collect())
        .collect())
}

/// The grid turned a quarter anticlockwise.
pub fn rotate_anticlockwise<T: Clone>(grid: &[Vec<T>]) -> Result<Vec<Vec<T>>, RaggedError> {
    let cols = width(grid)?;
    let rows = grid.len();
    // new[r][c] = old[c][cols - 1 - r]
    Ok((0..cols)
        .map(|r| (0..rows).map(|c| grid[c][cols - 1 - r].clone()).collect())
        .collect())
}

/// Turns a square grid a quarter clockwise in place, with no copy: mirror it across the diagonal, then reverse
/// each row. A ragged or non-square grid is left untouched and reported.
pub fn rotate_square_in_place<T>(grid: &mut [Vec<T>]) -> Result<(), RaggedError> {
    let n = grid.len();
    let cols = width(grid)?;
    if cols != n {
        return Err(RaggedError {
            row: 0,
            expected: n,
            found: cols,
        });
    }
    for r in 0..n {
        for c in r + 1..n {
            // grid[r][c] and grid[c][r] live in different rows: split the slice to borrow both at once.
            let (upper, lower) = grid.split_at_mut(c);
            std::mem::swap(&mut upper[r][c], &mut lower[0][r]);
        }
    }
    for row in grid.iter_mut() {
        row.reverse();
    }
    Ok(())
}

/// Where a square block of `true` cells sits: its top-left corner and side length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Square {
    /// Top row.
    pub row: usize,
    /// Left column.
    pub col: usize,
    /// Side length.
    pub size: usize,
}

/// The largest all-`true` square ("largest square"), by dynamic programming: the square ending at a cell is one
/// bigger than the smallest of the squares ending above it, left of it and diagonally above-left. O(R × C)
/// time, O(C) memory. `None` with no `true` cells; the top-most, then left-most, on ties.
pub fn largest_square(grid: &[Vec<bool>]) -> Result<Option<Square>, RaggedError> {
    let cols = width(grid)?;
    let mut above = vec![0usize; cols];
    let mut here = vec![0usize; cols];
    let mut best: Option<Square> = None;
    for r in 0..grid.len() {
        for c in 0..cols {
            here[c] = if !grid[r][c] {
                0
            } else if r == 0 || c == 0 {
                1
            } else {
                1 + above[c].min(here[c - 1]).min(above[c - 1])
            };
            if here[c] > best.map_or(0, |b| b.size) {
                best = Some(Square {
                    row: r + 1 - here[c],
                    col: c + 1 - here[c],
                    size: here[c],
                });
            }
        }
        std::mem::swap(&mut above, &mut here);
    }
    Ok(best)
}

/// Where a rectangular block of `true` cells sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Top row.
    pub row: usize,
    /// Left column.
    pub col: usize,
    /// Number of rows.
    pub height: usize,
    /// Number of columns.
    pub width: usize,
}

impl Rect {
    /// Number of cells.
    pub fn area(&self) -> usize {
        self.height * self.width
    }
}

/// The largest all-`true` rectangle. Each row turns into a histogram (how many `true` cells stack up above
/// each column), and the largest rectangle under a histogram falls out of one pass with a stack of columns
/// whose heights are still rising. O(R × C). `None` with no `true` cells.
pub fn largest_rectangle(grid: &[Vec<bool>]) -> Result<Option<Rect>, RaggedError> {
    let cols = width(grid)?;
    let mut heights = vec![0usize; cols];
    let mut rising: Vec<usize> = Vec::with_capacity(cols);
    let mut best: Option<Rect> = None;
    for r in 0..grid.len() {
        for c in 0..cols {
            heights[c] = if grid[r][c] { heights[c] + 1 } else { 0 };
        }
        rising.clear();
        for c in 0..=cols {
            let h = if c < cols { heights[c] } else { 0 };
            while let Some(&top) = rising.last() {
                if heights[top] <= h {
                    break;
                }
                // The bar at `top` can't extend past column c; its widest rectangle starts after the bar below it.
                rising.pop();
                let height = heights[top];
                let left = rising.last().map_or(0, |&l| l + 1);
                let rect = Rect {
                    row: r + 1 - height,
                    col: left,
                    height,
                    width: c - left,
                };
                if rect.area() > best.map_or(0, |b| b.area()) {
                    best = Some(rect);
                }
            }
            if c < cols {
                rising.push(c);
            }
        }
    }
    Ok(best)
}

/// A connected patch of equal cells: the first cell (top-most, then left-most) and how many cells it covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zone {
    /// Row of the zone's first cell.
    pub row: usize,
    /// Column of the zone's first cell.
    pub col: usize,
    /// Number of cells.
    pub cells: usize,
}

/// The largest group of equal cells joined edge to edge, up, down, left or right ("largest matrix zone"):
/// breadth-first search from every cell not yet visited. O(R × C). `None` for an empty grid.
pub fn largest_zone<T: PartialEq>(grid: &[Vec<T>]) -> Result<Option<Zone>, RaggedError> {
    let cols = width(grid)?;
    let rows = grid.len();
    let mut seen = vec![vec![false; cols]; rows];
    let mut queue = VecDeque::new();
    let mut best: Option<Zone> = None;
    for r in 0..rows {
        for c in 0..cols {
            if seen[r][c] {
                continue;
            }
            seen[r][c] = true;
            queue.push_back((r, c));
            let mut cells = 0;
            while let Some((y, x)) = queue.pop_front() {
                cells += 1;
                // wrapping_sub turns "one before 0" into usize::MAX, which the bounds check then rejects.
                for (ny, nx) in [
                    (y.wrapping_sub(1), x),
                    (y + 1, x),
                    (y, x.wrapping_sub(1)),
                    (y, x + 1),
                ] {
                    if ny < rows && nx < cols && !seen[ny][nx] && grid[ny][nx] == grid[y][x] {
                        seen[ny][nx] = true;
                        queue.push_back((ny, nx));
                    }
                }
            }
            if cells > best.map_or(0, |z| z.cells) {
                best = Some(Zone {
                    row: r,
                    col: c,
                    cells,
                });
            }
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Lcg;

    fn bools(rows: &[&str]) -> Vec<Vec<bool>> {
        rows.iter()
            .map(|r| r.chars().map(|c| c == '#').collect())
            .collect()
    }

    #[test]
    fn rotations() {
        let g = vec![vec![1, 2, 3], vec![4, 5, 6]];
        assert_eq!(
            rotate_clockwise(&g).unwrap(),
            vec![vec![4, 1], vec![5, 2], vec![6, 3]]
        );
        assert_eq!(
            rotate_anticlockwise(&g).unwrap(),
            vec![vec![3, 6], vec![2, 5], vec![1, 4]]
        );
        let back = rotate_anticlockwise(&rotate_clockwise(&g).unwrap()).unwrap();
        assert_eq!(back, g);
        let mut sq = vec![vec![1, 2, 3], vec![4, 5, 6], vec![7, 8, 9]];
        let expected = rotate_clockwise(&sq).unwrap();
        rotate_square_in_place(&mut sq).unwrap();
        assert_eq!(sq, expected);
        let mut ragged = vec![vec![1, 2], vec![3]];
        assert_eq!(
            rotate_square_in_place(&mut ragged),
            Err(RaggedError {
                row: 1,
                expected: 2,
                found: 1
            })
        );
        assert!(rotate_clockwise::<i32>(&[]).unwrap().is_empty());
    }

    #[test]
    fn squares_and_rectangles() {
        let g = bools(&["#..##", "#.###", "..###", "#####"]);
        assert_eq!(
            largest_square(&g).unwrap(),
            Some(Square {
                row: 1,
                col: 2,
                size: 3
            })
        );
        assert_eq!(
            largest_rectangle(&g).unwrap(),
            Some(Rect {
                row: 1,
                col: 2,
                height: 3,
                width: 3
            })
        );
        let wide = bools(&["######", "##.###"]);
        assert_eq!(largest_rectangle(&wide).unwrap().map(|r| r.area()), Some(6));
        assert_eq!(largest_square(&bools(&["...", "..."])).unwrap(), None);
        assert_eq!(largest_rectangle(&bools(&[])).unwrap(), None);
    }

    #[test]
    fn rectangle_matches_brute_force() {
        let mut rng = Lcg(41);
        for _ in 0..60 {
            let rows = 1 + rng.below(6) as usize;
            let cols = 1 + rng.below(6) as usize;
            let g: Vec<Vec<bool>> = (0..rows)
                .map(|_| (0..cols).map(|_| rng.below(3) > 0).collect())
                .collect();
            let mut brute = 0;
            for r0 in 0..rows {
                for c0 in 0..cols {
                    for r1 in r0..rows {
                        for c1 in c0..cols {
                            if (r0..=r1).all(|r| (c0..=c1).all(|c| g[r][c])) {
                                brute = brute.max((r1 - r0 + 1) * (c1 - c0 + 1));
                            }
                        }
                    }
                }
            }
            assert_eq!(
                largest_rectangle(&g).unwrap().map_or(0, |r| r.area()),
                brute
            );
        }
    }

    #[test]
    fn zones() {
        let g = vec![vec![1, 1, 2], vec![3, 1, 2], vec![3, 3, 3]];
        assert_eq!(
            largest_zone(&g).unwrap(),
            Some(Zone {
                row: 1,
                col: 0,
                cells: 4
            })
        );
        assert_eq!(largest_zone::<u8>(&[]).unwrap(), None);
        assert!(largest_zone(&[vec![1], vec![1, 2]]).is_err());
    }
}
