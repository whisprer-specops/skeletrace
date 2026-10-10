// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Points on a plane, on the Earth and on a street grid: the closest pair, great-circle distance, and the cells
//! equally far from two points.

use std::cmp::Ordering;

/// Mean Earth radius in metres.
pub const EARTH_RADIUS_M: f64 = 6_371_008.8;

/// The two closest points: their indices in the input (`first < second`) and the distance between them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClosestPair {
    /// Index of one point.
    pub first: usize,
    /// Index of the other point.
    pub second: usize,
    /// Distance between them: plane units for [`closest_pair`], metres for [`closest_pair_geo`].
    pub distance: f64,
}

#[derive(Clone, Copy)]
struct Best {
    d2: f64,
    a: usize,
    b: usize,
}

fn d2(p: (f64, f64), q: (f64, f64)) -> f64 {
    let (dx, dy) = (p.0 - q.0, p.1 - q.1);
    dx * dx + dy * dy
}

/// The closest pair of points on a plane ("closest pair"), by divide and conquer: O(N log N) instead of
/// checking all N²/2 pairs. Split at the middle x, solve each half, then check only points within the best
/// distance so far of the split line; taken in y order, each needs comparing with a handful of others at most.
/// `None` for fewer than two points, or if any coordinate isn't finite.
pub fn closest_pair(points: &[(f64, f64)]) -> Option<ClosestPair> {
    if points.len() < 2
        || points
            .iter()
            .any(|&(x, y)| !x.is_finite() || !y.is_finite())
    {
        return None;
    }
    let mut order: Vec<usize> = (0..points.len()).collect();
    order.sort_by(|&a, &b| {
        points[a]
            .0
            .total_cmp(&points[b].0)
            .then(points[a].1.total_cmp(&points[b].1))
    });
    let mut buffer = vec![0usize; points.len()];
    let mut best = Best {
        d2: f64::INFINITY,
        a: order[0],
        b: order[1],
    };
    solve(points, &mut order, &mut buffer, &mut best);
    let (first, second) = if best.a < best.b {
        (best.a, best.b)
    } else {
        (best.b, best.a)
    };
    Some(ClosestPair {
        first,
        second,
        distance: best.d2.sqrt(),
    })
}

/// Finds the closest pair within `idx` (sorted by x on entry) and leaves `idx` sorted by y on return.
fn solve(pts: &[(f64, f64)], idx: &mut [usize], buf: &mut [usize], best: &mut Best) {
    let n = idx.len();
    if n <= 3 {
        for i in 0..n {
            for j in i + 1..n {
                let d = d2(pts[idx[i]], pts[idx[j]]);
                if d < best.d2 {
                    *best = Best {
                        d2: d,
                        a: idx[i],
                        b: idx[j],
                    };
                }
            }
        }
        idx.sort_by(|&a, &b| pts[a].1.total_cmp(&pts[b].1));
        return;
    }
    let mid = n / 2;
    let mid_x = pts[idx[mid]].0;
    {
        let (left, right) = idx.split_at_mut(mid);
        let (left_buf, right_buf) = buf.split_at_mut(mid);
        solve(pts, left, left_buf, best);
        solve(pts, right, right_buf, best);
    }
    // Merge the two y-sorted halves (merge sort's merge step), so the strip below can be scanned in y order.
    let (mut i, mut j, mut k) = (0, mid, 0);
    while i < mid && j < n {
        if pts[idx[i]].1.total_cmp(&pts[idx[j]].1) != Ordering::Greater {
            buf[k] = idx[i];
            i += 1;
        } else {
            buf[k] = idx[j];
            j += 1;
        }
        k += 1;
    }
    buf[k..k + (mid - i)].copy_from_slice(&idx[i..mid]);
    k += mid - i;
    buf[k..k + (n - j)].copy_from_slice(&idx[j..n]);
    idx.copy_from_slice(&buf[..n]);
    // The strip: points closer to the split line than the best distance so far, in y order.
    let mut strip: Vec<usize> = Vec::new();
    for &p in idx.iter() {
        let dx = pts[p].0 - mid_x;
        if dx * dx < best.d2 {
            for &q in strip.iter().rev() {
                let dy = pts[p].1 - pts[q].1;
                if dy * dy >= best.d2 {
                    break;
                }
                let d = d2(pts[p], pts[q]);
                if d < best.d2 {
                    *best = Best { d2: d, a: q, b: p };
                }
            }
            strip.push(p);
        }
    }
}

/// The closest pair of (latitude, longitude) points in degrees, with the distance in metres along the Earth's
/// surface. Points are first flattened onto a local plane around their mean latitude, so the choice is exact
/// for areas a few hundred kilometres across; across continents the flattening can pick a pair that isn't quite
/// the closest. Groups that straddle the ±180° meridian aren't handled.
///
/// **SkeleTrace use:** the closest approach between sightings: two ships, two aircraft, two reports.
pub fn closest_pair_geo(points: &[(f64, f64)]) -> Option<ClosestPair> {
    if points.is_empty() {
        return None;
    }
    let mean_lat = points.iter().map(|p| p.0).sum::<f64>() / points.len() as f64;
    let shrink = mean_lat.to_radians().cos();
    let flat: Vec<(f64, f64)> = points
        .iter()
        .map(|&(lat, lon)| (lon.to_radians() * shrink, lat.to_radians()))
        .collect();
    let pair = closest_pair(&flat)?;
    Some(ClosestPair {
        distance: haversine_m(points[pair.first], points[pair.second]),
        ..pair
    })
}

/// Great-circle distance in metres between two (latitude, longitude) points in degrees (haversine formula).
pub fn haversine_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (lat1, lat2) = (a.0.to_radians(), b.0.to_radians());
    let dlat = lat2 - lat1;
    let dlon = (b.1 - a.1).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * h.sqrt().min(1.0).asin()
}

/// How many cells of a `rows` × `cols` grid are exactly as far from cell `a` as from cell `b`, moving up, down,
/// left and right ("Donkey Paradox", CS Academy: a donkey on such a cell can't choose between two haystacks, so
/// it starves). Cells are (row, column), counted from 0. `None` if `a` or `b` is off the grid. If they are the
/// same cell, every cell ties.
///
/// On an open grid the walking distance is the taxicab distance, |Δrow| + |Δcolumn|. Cell (r, c) ties when
/// `|r − a.row| − |r − b.row| = |c − b.col| − |c − a.col|`: the left side depends only on the row, the right side
/// only on the column. Along each axis that difference is flat outside the two points and climbs in steps of 2
/// between them, so matching values can be counted without visiting a single cell. O(1), for any grid size.
///
/// **SkeleTrace use:** on a street grid, the blocks where two candidate places are equally close on foot, so
/// "whichever was nearer" can't settle which one a person used.
pub fn taxicab_ties(rows: u64, cols: u64, a: (u64, u64), b: (u64, u64)) -> Option<u128> {
    if a.0 >= rows || b.0 >= rows || a.1 >= cols || b.1 >= cols {
        return None;
    }
    // Rows give |r − a.row| − |r − b.row|; columns give |c − b.col| − |c − a.col|. A cell ties when they match.
    let down = Axis::new(a.0, b.0, rows);
    let across = Axis::new(b.1, a.1, cols);
    let mut ties: u128 = 0;
    for &(value, rows_with_it) in &down.flat {
        for &(other, cols_with_it) in &across.flat {
            if value == other {
                ties += rows_with_it * cols_with_it;
            }
        }
        if across.between(value) {
            ties += rows_with_it;
        }
    }
    for &(value, cols_with_it) in &across.flat {
        if down.between(value) {
            ties += cols_with_it;
        }
    }
    // Values that both axes reach only between their two points: one position on each axis apiece.
    if down.gap > 0 && across.gap > 0 && (down.gap - across.gap) % 2 == 0 {
        ties += (down.gap.min(across.gap) - 1) as u128;
    }
    Some(ties)
}

/// One axis of [`taxicab_ties`]: the values `|x − p| − |x − q|` takes as `x` runs over `0..len`.
struct Axis {
    /// |p − q|.
    gap: i128,
    /// The flat stretches, as (value, positions): `x` up to the nearer point gives `p − q`, and `x` from the
    /// further point on gives `q − p`. One stretch covering everything when `p == q`.
    flat: Vec<(i128, u128)>,
}

impl Axis {
    /// `p` and `q` must lie in `0..len`.
    fn new(p: u64, q: u64, len: u64) -> Axis {
        let (p, q, len) = (i128::from(p), i128::from(q), i128::from(len));
        if p == q {
            return Axis {
                gap: 0,
                flat: vec![(0, len as u128)],
            };
        }
        let (near, far) = (p.min(q), p.max(q));
        Axis {
            gap: far - near,
            flat: vec![(p - q, (near + 1) as u128), (q - p, (len - far) as u128)],
        }
    }

    /// True when exactly one position strictly between the two points gives `value`. Those positions give every
    /// value with the gap's parity and a size below the gap, once each.
    fn between(&self, value: i128) -> bool {
        value.abs() < self.gap && (self.gap - value) % 2 == 0
    }
}

/// Where a point lies relative to a [`Fence`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Strictly inside.
    Inside,
    /// Exactly on the outline.
    OnEdge,
    /// Outside.
    Outside,
}

/// A closed outline (a geofence), for asking which side of it points fall.
///
/// Any flat coordinates work. So do (latitude, longitude) pairs for areas away from the poles and the ±180°
/// meridian, where the fence's edges are taken as straight lines on a latitude/longitude map.
///
/// **SkeleTrace use:** which sightings, check-ins or photo locations fall inside an area of interest.
#[derive(Clone, Debug)]
pub struct Fence {
    corners: Vec<(f64, f64)>,
    low: (f64, f64),
    high: (f64, f64),
}

impl Fence {
    /// A fence through `corners` in order, closing back to the first. `None` with fewer than three corners, or
    /// if any coordinate isn't finite.
    pub fn new(corners: &[(f64, f64)]) -> Option<Fence> {
        if corners.len() < 3
            || corners
                .iter()
                .any(|&(x, y)| !x.is_finite() || !y.is_finite())
        {
            return None;
        }
        let mut low = corners[0];
        let mut high = corners[0];
        for &(x, y) in corners {
            low = (low.0.min(x), low.1.min(y));
            high = (high.0.max(x), high.1.max(y));
        }
        Some(Fence {
            corners: corners.to_vec(),
            low,
            high,
        })
    }

    /// The corners, in order.
    pub fn corners(&self) -> &[(f64, f64)] {
        &self.corners
    }

    /// Inside, outside, or exactly on the outline, as the numbers are given (no tolerance). A point that isn't
    /// finite is outside. O(corners), after an O(1) check against the fence's bounding box.
    ///
    /// Crossing count: follow a ray from the point in the direction of growing first coordinate, and count the
    /// edges it crosses. Odd means inside. Each edge counts its lower end but not its upper end, so a ray through
    /// a corner is counted once. An outline that crosses itself follows the same rule, so an area it wraps twice
    /// counts as outside.
    pub fn locate(&self, point: (f64, f64)) -> Placement {
        let (x, y) = point;
        if !(x >= self.low.0 && x <= self.high.0 && y >= self.low.1 && y <= self.high.1) {
            return Placement::Outside; // also catches NaN
        }
        let n = self.corners.len();
        let mut inside = false;
        for i in 0..n {
            let p = self.corners[i];
            let q = self.corners[(i + 1) % n];
            let cross = (q.0 - p.0) * (y - p.1) - (q.1 - p.1) * (x - p.0);
            if cross == 0.0
                && x >= p.0.min(q.0)
                && x <= p.0.max(q.0)
                && y >= p.1.min(q.1)
                && y <= p.1.max(q.1)
            {
                return Placement::OnEdge;
            }
            if (p.1 > y) != (q.1 > y) {
                let crossing = p.0 + (y - p.1) * (q.0 - p.0) / (q.1 - p.1);
                if x < crossing {
                    inside = !inside;
                }
            }
        }
        if inside {
            Placement::Inside
        } else {
            Placement::Outside
        }
    }

    /// True when the point is inside or on the outline.
    pub fn contains(&self, point: (f64, f64)) -> bool {
        self.locate(point) != Placement::Outside
    }
}

/// Twice the signed area of the triangle o, a, b: positive when o → a → b turns anticlockwise (first coordinate
/// to the right, second up).
fn turn(o: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// The corners of the smallest convex outline around `points` (the convex hull), as indices into `points`.
/// `None` for no points, or if any coordinate isn't finite.
///
/// The corners run anticlockwise (with the first coordinate across and the second up), starting from the point
/// with the smallest first coordinate, then the smallest second. Points along a straight stretch of the outline
/// are left out, and of identical points only the first is used. One distinct point gives one index, and points
/// all on one line give the line's two ends.
///
/// Andrew's monotone chain: sort the points, sweep along the bottom and back along the top, and drop any point
/// that would make the outline turn the wrong way. O(N log N).
///
/// (Latitude, longitude) pairs work as they do for [`Fence`], away from the poles and the ±180° meridian.
///
/// **SkeleTrace use:** the area a set of sightings covers. For its size, pass the corners to
/// [`polygon_area_m2`].
pub fn convex_hull(points: &[(f64, f64)]) -> Option<Vec<usize>> {
    if points.is_empty()
        || points
            .iter()
            .any(|&(x, y)| !x.is_finite() || !y.is_finite())
    {
        return None;
    }
    let mut order: Vec<usize> = (0..points.len()).collect();
    order.sort_by(|&a, &b| {
        let (p, q) = (points[a], points[b]);
        p.0.total_cmp(&q.0)
            .then(p.1.total_cmp(&q.1))
            .then(a.cmp(&b))
    });
    order.dedup_by(|later, earlier| points[*later] == points[*earlier]);
    if order.len() < 3 {
        return Some(order);
    }
    let mut hull: Vec<usize> = Vec::with_capacity(2 * order.len());
    // Bottom, left to right.
    for &i in &order {
        while hull.len() >= 2
            && turn(
                points[hull[hull.len() - 2]],
                points[hull[hull.len() - 1]],
                points[i],
            ) <= 0.0
        {
            hull.pop();
        }
        hull.push(i);
    }
    // Top, right to left. The bottom's last point starts it and must stay.
    let bottom = hull.len();
    for &i in order.iter().rev().skip(1) {
        while hull.len() > bottom
            && turn(
                points[hull[hull.len() - 2]],
                points[hull[hull.len() - 1]],
                points[i],
            ) <= 0.0
        {
            hull.pop();
        }
        hull.push(i);
    }
    hull.pop(); // the sweep ends back at the starting point
    Some(hull)
}

/// The area inside an outline of (latitude, longitude) corners, in degrees, in square metres on the Earth (taken
/// as a sphere of the mean radius). `None` with fewer than three corners, or if any coordinate isn't finite.
///
/// Each corner goes onto an equal-area map, with longitude across and the sine of latitude up, where the
/// shoelace formula gives the area exactly. That is the true area when the edges run straight on that map, as
/// edges along parallels and meridians do. For other edges a few hundred kilometres long or less, it is close
/// to the area with edges drawn straight on an ordinary map. Outlines across the ±180° meridian aren't
/// handled.
pub fn polygon_area_m2(corners: &[(f64, f64)]) -> Option<f64> {
    if corners.len() < 3
        || corners
            .iter()
            .any(|&(lat, lon)| !lat.is_finite() || !lon.is_finite())
    {
        return None;
    }
    let origin = corners[0].1.to_radians();
    let map = |&(lat, lon): &(f64, f64)| (lon.to_radians() - origin, lat.to_radians().sin());
    let n = corners.len();
    let mut twice = 0.0;
    for i in 0..n {
        let (x1, y1) = map(&corners[i]);
        let (x2, y2) = map(&corners[(i + 1) % n]);
        twice += x1 * y2 - x2 * y1;
    }
    Some(twice.abs() / 2.0 * EARTH_RADIUS_M * EARTH_RADIUS_M)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Lcg;

    fn brute(points: &[(f64, f64)]) -> f64 {
        let mut best = f64::INFINITY;
        for i in 0..points.len() {
            for j in i + 1..points.len() {
                best = best.min(d2(points[i], points[j]).sqrt());
            }
        }
        best
    }

    #[test]
    fn matches_brute_force() {
        let mut rng = Lcg(71);
        for len in [2, 3, 4, 5, 17, 100, 333] {
            let points: Vec<(f64, f64)> = (0..len)
                .map(|_| {
                    (
                        rng.below(10_000) as f64 / 7.0,
                        rng.below(10_000) as f64 / 3.0,
                    )
                })
                .collect();
            let found = closest_pair(&points).unwrap();
            assert!(found.first < found.second);
            assert!(
                (found.distance - brute(&points)).abs() < 1e-9,
                "length {len}"
            );
            assert!(
                (d2(points[found.first], points[found.second]).sqrt() - found.distance).abs()
                    < 1e-9
            );
        }
    }

    #[test]
    fn edge_cases() {
        assert_eq!(closest_pair(&[(0.0, 0.0)]), None);
        assert_eq!(closest_pair(&[(0.0, 0.0), (f64::NAN, 1.0)]), None);
        let same = closest_pair(&[(1.0, 1.0), (5.0, 5.0), (1.0, 1.0)]).unwrap();
        assert_eq!((same.first, same.second, same.distance), (0, 2, 0.0));
        let line: Vec<(f64, f64)> = (0..50).map(|i| (0.0, f64::from(i * i))).collect();
        assert_eq!(closest_pair(&line).unwrap().distance, 1.0);
    }

    #[test]
    fn on_the_earth() {
        let london = (51.5074, -0.1278);
        let paris = (48.8566, 2.3522);
        let d = haversine_m(london, paris);
        assert!(
            (340_000.0..347_000.0).contains(&d),
            "London to Paris was {d} m"
        );
        let berlin = (52.5200, 13.4050);
        let near_paris = (48.8606, 2.3376);
        let pair = closest_pair_geo(&[london, paris, berlin, near_paris]).unwrap();
        assert_eq!((pair.first, pair.second), (1, 3));
        assert!(pair.distance < 2_000.0);
    }

    fn ties_by_walking(rows: u64, cols: u64, a: (u64, u64), b: (u64, u64)) -> u128 {
        let mut ties = 0;
        for r in 0..rows {
            for c in 0..cols {
                let to_a = r.abs_diff(a.0) + c.abs_diff(a.1);
                let to_b = r.abs_diff(b.0) + c.abs_diff(b.1);
                if to_a == to_b {
                    ties += 1;
                }
            }
        }
        ties
    }

    #[test]
    fn taxicab_ties_match_every_small_grid() {
        for rows in 1..=6 {
            for cols in 1..=6 {
                for a in 0..rows * cols {
                    for b in 0..rows * cols {
                        let (a, b) = ((a / cols, a % cols), (b / cols, b % cols));
                        assert_eq!(
                            taxicab_ties(rows, cols, a, b),
                            Some(ties_by_walking(rows, cols, a, b)),
                            "{rows} × {cols}, {a:?} and {b:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn taxicab_ties_edge_cases() {
        // Opposite corners of a 3 × 3 grid: the other diagonal ties.
        assert_eq!(taxicab_ties(3, 3, (0, 0), (2, 2)), Some(3));
        // Next-door cells never tie: every distance to one is odd and to the other even, or the reverse.
        assert_eq!(taxicab_ties(1000, 1000, (500, 500), (500, 501)), Some(0));
        assert_eq!(taxicab_ties(3, 3, (0, 0), (3, 0)), None);
        assert_eq!(taxicab_ties(0, 5, (0, 0), (0, 1)), None);
        let side = u64::MAX;
        let all = u128::from(side) * u128::from(side);
        assert_eq!(taxicab_ties(side, side, (7, 7), (7, 7)), Some(all));
        assert_eq!(taxicab_ties(1, side, (0, 0), (0, 10)), Some(1));
        assert_eq!(
            taxicab_ties(side, 1, (side - 1, 0), (side - 11, 0)),
            Some(1)
        );
        // Random grids up to 40 × 40, against checking every cell.
        let mut rng = Lcg(81);
        for _ in 0..300 {
            let (rows, cols) = (1 + rng.below(40), 1 + rng.below(40));
            let a = (rng.below(rows), rng.below(cols));
            let b = (rng.below(rows), rng.below(cols));
            assert_eq!(
                taxicab_ties(rows, cols, a, b),
                Some(ties_by_walking(rows, cols, a, b))
            );
        }
    }

    #[test]
    fn fences_simple_concave_and_self_crossing() {
        use Placement::{Inside, OnEdge, Outside};
        let square = Fence::new(&[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]).unwrap();
        assert_eq!(square.locate((2.0, 2.0)), Inside);
        assert_eq!(square.locate((5.0, 2.0)), Outside);
        assert_eq!(square.locate((4.0, 2.0)), OnEdge);
        assert_eq!(square.locate((0.0, 0.0)), OnEdge);
        assert_eq!(square.locate((2.0, 4.0)), OnEdge);
        assert_eq!(square.locate((-1.0, -1.0)), Outside);
        assert_eq!(square.locate((f64::NAN, 1.0)), Outside);
        assert!(square.contains((4.0, 4.0)) && !square.contains((4.5, 4.0)));
        // An L shape: the notch is outside.
        let l_shape = Fence::new(&[
            (0.0, 0.0),
            (4.0, 0.0),
            (4.0, 1.0),
            (1.0, 1.0),
            (1.0, 4.0),
            (0.0, 4.0),
        ])
        .unwrap();
        assert_eq!(l_shape.locate((2.0, 2.0)), Outside);
        assert_eq!(l_shape.locate((0.5, 3.0)), Inside);
        assert_eq!(l_shape.locate((2.0, 0.5)), Inside);
        assert_eq!(l_shape.locate((1.0, 2.0)), OnEdge);
        // A five-pointed star drawn in one stroke: the tips are inside, the middle (wrapped twice) is not.
        let star: Vec<(f64, f64)> = [0, 2, 4, 1, 3]
            .iter()
            .map(|&k| {
                let angle = (90.0 + 72.0 * f64::from(k)).to_radians();
                (10.0 * angle.cos(), 10.0 * angle.sin())
            })
            .collect();
        let star = Fence::new(&star).unwrap();
        assert_eq!(star.locate((0.0, 8.0)), Inside);
        assert_eq!(star.locate((0.0, 0.0)), Outside);
        assert!(Fence::new(&[(0.0, 0.0), (1.0, 1.0)]).is_none());
        assert!(Fence::new(&[(0.0, 0.0), (1.0, 1.0), (f64::INFINITY, 0.0)]).is_none());
    }

    /// Winding number of `fence` around `p` (Sunday's method): non-zero means inside, for an outline that
    /// doesn't cross itself.
    fn winding(fence: &[(f64, f64)], p: (f64, f64)) -> i32 {
        let mut w = 0;
        for i in 0..fence.len() {
            let (a, b) = (fence[i], fence[(i + 1) % fence.len()]);
            let side = turn(a, b, p);
            if a.1 <= p.1 {
                if b.1 > p.1 && side > 0.0 {
                    w += 1;
                }
            } else if b.1 <= p.1 && side < 0.0 {
                w -= 1;
            }
        }
        w
    }

    #[test]
    fn fences_agree_with_winding_numbers() {
        let mut rng = Lcg(83);
        let unit = |rng: &mut Lcg| rng.below(1_000_000) as f64 / 1_000_000.0;
        for _ in 0..300 {
            // A star-shaped outline: corners at increasing angles round the origin never cross.
            let corners = 3 + rng.below(10) as usize;
            let mut angles: Vec<f64> = (0..corners)
                .map(|_| unit(&mut rng) * std::f64::consts::TAU)
                .collect();
            angles.sort_by(f64::total_cmp);
            let outline: Vec<(f64, f64)> = angles
                .iter()
                .map(|&a| {
                    let r = 1.0 + 9.0 * unit(&mut rng);
                    (r * a.cos(), r * a.sin())
                })
                .collect();
            let fence = Fence::new(&outline).unwrap();
            for _ in 0..50 {
                let p = (unit(&mut rng) * 24.0 - 12.0, unit(&mut rng) * 24.0 - 12.0);
                let expected = if winding(&outline, p) != 0 {
                    Placement::Inside
                } else {
                    Placement::Outside
                };
                let got = fence.locate(p);
                if got != Placement::OnEdge {
                    assert_eq!(got, expected, "{outline:?} {p:?}");
                }
            }
        }
    }

    #[test]
    fn hulls() {
        let points = [
            (0.0, 0.0),
            (2.0, 0.0),
            (4.0, 0.0),
            (4.0, 4.0),
            (0.0, 4.0),
            (2.0, 2.0),
            (0.0, 0.0),
        ];
        assert_eq!(convex_hull(&points), Some(vec![0, 2, 3, 4]));
        assert_eq!(convex_hull(&[]), None);
        assert_eq!(convex_hull(&[(1.0, f64::NAN)]), None);
        assert_eq!(convex_hull(&[(1.0, 1.0), (1.0, 1.0)]), Some(vec![0]));
        assert_eq!(
            convex_hull(&[(2.0, 2.0), (0.0, 0.0), (1.0, 1.0)]),
            Some(vec![1, 0])
        );
        let mut rng = Lcg(89);
        for _ in 0..500 {
            // Small whole-number coordinates: lots of repeats and points in a line, and exact arithmetic.
            let points: Vec<(f64, f64)> = (0..1 + rng.below(25))
                .map(|_| (rng.below(7) as f64, rng.below(7) as f64))
                .collect();
            let hull = convex_hull(&points).unwrap();
            let mut seen = hull.clone();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), hull.len(), "no corner twice");
            let first =
                points.iter().copied().fold(
                    points[0],
                    |m, p| if (p.0, p.1) < (m.0, m.1) { p } else { m },
                );
            assert_eq!(points[hull[0]], first, "starts bottom left");
            if hull.len() >= 3 {
                for k in 0..hull.len() {
                    let a = points[hull[k]];
                    let b = points[hull[(k + 1) % hull.len()]];
                    let c = points[hull[(k + 2) % hull.len()]];
                    assert!(turn(a, b, c) > 0.0, "every corner turns left");
                    for &p in &points {
                        assert!(turn(a, b, p) >= 0.0, "every point on or inside");
                    }
                }
            } else {
                // One point, or every point on one line between the two ends.
                let (a, b) = (points[hull[0]], points[hull[hull.len() - 1]]);
                for &p in &points {
                    assert_eq!(turn(a, b, p), 0.0);
                    assert!(p.0 >= a.0.min(b.0) && p.0 <= a.0.max(b.0));
                    assert!(p.1 >= a.1.min(b.1) && p.1 <= a.1.max(b.1));
                }
            }
        }
    }

    #[test]
    fn areas_on_the_earth() {
        let r2 = EARTH_RADIUS_M * EARTH_RADIUS_M;
        let near = |got: f64, want: f64| ((got - want) / want).abs() < 1e-12;
        // A box along parallels and meridians: exactly R² × Δlongitude × Δsin(latitude).
        let equator = [(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)];
        let want = r2 * 1f64.to_radians() * 1f64.to_radians().sin();
        assert!(near(polygon_area_m2(&equator).unwrap(), want));
        assert!((12_300e6..12_400e6).contains(&want), "about 12,360 km²");
        let england = [(50.0, -2.0), (50.0, -1.0), (51.0, -1.0), (51.0, -2.0)];
        let want = r2 * 1f64.to_radians() * (51f64.to_radians().sin() - 50f64.to_radians().sin());
        assert!(near(polygon_area_m2(&england).unwrap(), want));
        let mut backwards = england;
        backwards.reverse();
        assert!(near(polygon_area_m2(&backwards).unwrap(), want));
        let half = [(50.0, -2.0), (50.0, -1.0), (51.0, -1.0)];
        assert!(near(polygon_area_m2(&half).unwrap(), want / 2.0));
        assert_eq!(polygon_area_m2(&[(0.0, 0.0), (1.0, 1.0)]), None);
    }
}
