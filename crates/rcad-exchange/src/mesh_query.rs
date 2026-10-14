//! Geometric queries on indexed triangle meshes shared by mesh inspection
//! ([`crate::mesh::self_intersections`]) and mesh editing ([`crate::mesh_edit`]).
//!
//! * [`weld_exact`]: vertices with bit-identical coordinates share one index.
//! * [`TriGrid`]: a uniform grid of triangle bounding boxes (cell size from the mean
//!   triangle extent, at most [`MAX_GRID_CELLS`] cells); used for candidate pairs and
//!   nearest-triangle queries.
//! * [`point_triangle_distance`]: exact Euclidean distance from a point to a closed
//!   triangle (closest-point by Voronoi regions, Ericson, *Real-Time Collision
//!   Detection* §5.1.5).
//! * [`triangles_cross`]: whether two triangles **properly** intersect (defined there).
use std::collections::HashMap;

/// Upper bound on grid cells (memory bound for [`TriGrid`]).
pub const MAX_GRID_CELLS: usize = 4_000_000;
/// Upper bound on (triangle, cell) registrations: a mix of tiny and huge
/// triangles coarsens the grid instead of registering a huge triangle in
/// millions of cells.
pub const MAX_GRID_REGISTRATIONS: u64 = 64_000_000;

pub(crate) fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(crate) fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub(crate) fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub(crate) fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub(crate) fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(crate) fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

/// Axis-aligned bounds of `pts` (`None` when empty).
pub fn bounds_of<'a>(pts: impl IntoIterator<Item = &'a [f64; 3]>) -> Option<([f64; 3], [f64; 3])> {
    pts.into_iter().fold(None, |acc, p| {
        Some(match acc {
            None => (*p, *p),
            Some((lo, hi)) => (
                [lo[0].min(p[0]), lo[1].min(p[1]), lo[2].min(p[2])],
                [hi[0].max(p[0]), hi[1].max(p[1]), hi[2].max(p[2])],
            ),
        })
    })
}

/// Exact weld: returns (unique positions, triangles re-indexed). Vertex order is
/// first occurrence (deterministic). Out-of-range indices are left out with their
/// triangle (the caller diagnoses those first).
pub fn weld_exact(
    positions: &[[f64; 3]],
    triangles: &[[u32; 3]],
) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
    let mut map: HashMap<[u64; 3], u32> = HashMap::with_capacity(positions.len());
    let mut remap = Vec::with_capacity(positions.len());
    let mut out = Vec::new();
    for p in positions {
        // +0.0 and -0.0 are the same point.
        let key = p.map(|x| if x == 0.0 { 0u64 } else { x.to_bits() });
        let i = *map.entry(key).or_insert_with(|| {
            out.push(*p);
            (out.len() - 1) as u32
        });
        remap.push(i);
    }
    let tris = triangles
        .iter()
        .filter(|t| t.iter().all(|&i| (i as usize) < remap.len()))
        .map(|t| t.map(|i| remap[i as usize]))
        .collect();
    (out, tris)
}

/// Uniform grid over triangle bounding boxes.
pub struct TriGrid {
    lo: [f64; 3],
    cell: f64,
    dims: [usize; 3],
    /// Cell -> triangle indices (CSR layout).
    start: Vec<u32>,
    items: Vec<u32>,
}

impl TriGrid {
    /// Build over the given triangles (indices into `pos`, all valid). `None` when
    /// there are no triangles.
    pub fn new(pos: &[[f64; 3]], tris: &[[u32; 3]]) -> Option<TriGrid> {
        let (lo, hi) = bounds_of(
            tris.iter()
                .flat_map(|t| t.iter().map(|&i| &pos[i as usize])),
        )?;
        let ext = sub(hi, lo);
        let diag = norm(ext).max(1e-12);
        // Mean triangle extent as the cell size, bounded so the grid has at most
        // MAX_GRID_CELLS cells.
        let mean = tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| pos[i as usize]);
                (0..3)
                    .map(|k| a[k].max(b[k]).max(c[k]) - a[k].min(b[k]).min(c[k]))
                    .fold(0.0, f64::max)
            })
            .sum::<f64>()
            / tris.len() as f64;
        let mut cell = mean.max(diag * 1e-6);
        // Registrations of every triangle's bounding box for a cell size.
        let registrations = |cell: f64| -> u64 {
            tris.iter()
                .map(|t| {
                    let [a, b, c] = t.map(|i| pos[i as usize]);
                    (0..3)
                        .map(|k| {
                            let lo_c = ((a[k].min(b[k]).min(c[k]) - lo[k]) / cell).floor();
                            let hi_c = ((a[k].max(b[k]).max(c[k]) - lo[k]) / cell).floor();
                            (hi_c - lo_c).max(0.0) as u64 + 1
                        })
                        .fold(1u64, |p, n| p.saturating_mul(n))
                })
                .fold(0u64, |s, n| s.saturating_add(n))
        };
        loop {
            let d = ext.map(|e| ((e / cell).floor() as usize + 1).max(1));
            if d[0].saturating_mul(d[1]).saturating_mul(d[2]) <= MAX_GRID_CELLS
                && registrations(cell) <= MAX_GRID_REGISTRATIONS
            {
                break;
            }
            cell *= 1.5;
        }
        let dims = ext.map(|e| ((e / cell).floor() as usize + 1).max(1));
        let mut g = TriGrid {
            lo,
            cell,
            dims,
            start: vec![],
            items: vec![],
        };
        let ncell = dims[0] * dims[1] * dims[2];
        let mut count = vec![0u32; ncell + 1];
        for t in tris {
            let (a, b) = g.tri_range(pos, t);
            g.for_cells(a, b, |c| count[c] += 1);
        }
        let mut acc = 0u32;
        for c in count.iter_mut() {
            let n = *c;
            *c = acc;
            acc += n;
        }
        let mut fill = count.clone();
        let mut items = vec![0u32; acc as usize];
        for (ti, t) in tris.iter().enumerate() {
            let (a, b) = g.tri_range(pos, t);
            g.for_cells(a, b, |c| {
                items[fill[c] as usize] = ti as u32;
                fill[c] += 1;
            });
        }
        g.start = count;
        g.items = items;
        Some(g)
    }

    pub fn cell_size(&self) -> f64 {
        self.cell
    }

    fn coord(&self, p: [f64; 3]) -> [usize; 3] {
        let mut c = [0usize; 3];
        for k in 0..3 {
            let v = ((p[k] - self.lo[k]) / self.cell).floor();
            c[k] = if v.is_nan() || v < 0.0 {
                0
            } else {
                (v as usize).min(self.dims[k] - 1)
            };
        }
        c
    }

    /// Cell coordinate range of a triangle's bounding box.
    pub fn tri_range(&self, pos: &[[f64; 3]], t: &[u32; 3]) -> ([usize; 3], [usize; 3]) {
        let [a, b, c] = t.map(|i| pos[i as usize]);
        let lo = [0, 1, 2].map(|k| a[k].min(b[k]).min(c[k]));
        let hi = [0, 1, 2].map(|k| a[k].max(b[k]).max(c[k]));
        (self.coord(lo), self.coord(hi))
    }

    fn for_cells(&self, a: [usize; 3], b: [usize; 3], mut f: impl FnMut(usize)) {
        for z in a[2]..=b[2] {
            for y in a[1]..=b[1] {
                for x in a[0]..=b[0] {
                    f((z * self.dims[1] + y) * self.dims[0] + x);
                }
            }
        }
    }

    /// Triangles registered in cell `(x, y, z)` (linear index).
    pub fn cell_items(&self, c: usize) -> &[u32] {
        &self.items[self.start[c] as usize..self.start[c + 1] as usize]
    }

    pub fn cell_count(&self) -> usize {
        self.dims[0] * self.dims[1] * self.dims[2]
    }

    /// Linear index -> cell coordinates.
    pub fn cell_coord(&self, c: usize) -> [usize; 3] {
        [
            c % self.dims[0],
            (c / self.dims[0]) % self.dims[1],
            c / (self.dims[0] * self.dims[1]),
        ]
    }

    /// Distance from `p` to the nearest of the grid's triangles and its index.
    /// Exact (searches growing shells of cells until no closer cell can exist).
    pub fn nearest(&self, pos: &[[f64; 3]], tris: &[[u32; 3]], p: [f64; 3]) -> (f64, u32) {
        let c = self.coord(p);
        // Distance from p to the box of cell c, lower bound for the shell `r`.
        let mut best = (f64::INFINITY, u32::MAX);
        let maxr = self.dims.iter().copied().max().unwrap_or(1);
        for r in 0..=maxr {
            // Lower bound on the distance of anything in shell r (outside the
            // (2r-1)^3 block around c): (r - 1) cells plus p's offset, at least 0.
            let lower = if r == 0 {
                0.0
            } else {
                let mut m = f64::INFINITY;
                for k in 0..3 {
                    let cl = self.lo[k] + c[k] as f64 * self.cell;
                    let off_lo = p[k] - cl;
                    let off_hi = cl + self.cell - p[k];
                    m = m.min(off_lo.max(0.0) + (r - 1) as f64 * self.cell);
                    m = m.min(off_hi.max(0.0) + (r - 1) as f64 * self.cell);
                }
                m.max(0.0)
            };
            if lower > best.0 {
                break;
            }
            let lo = c.map(|v| v.saturating_sub(r));
            let hi = [0, 1, 2].map(|k| (c[k] + r).min(self.dims[k] - 1));
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        let on_shell = [x, y, z]
                            .iter()
                            .zip(c.iter())
                            .any(|(&a, &b)| a.abs_diff(b) == r);
                        if !on_shell {
                            continue;
                        }
                        let cell = (z * self.dims[1] + y) * self.dims[0] + x;
                        // A triangle in several cells is measured more than once
                        // (harmless).
                        for &ti in self.cell_items(cell) {
                            let t = tris[ti as usize];
                            let d = point_triangle_distance(
                                p,
                                pos[t[0] as usize],
                                pos[t[1] as usize],
                                pos[t[2] as usize],
                            );
                            if d < best.0 {
                                best = (d, ti);
                            }
                        }
                    }
                }
            }
        }
        best
    }
}

/// Closest point on triangle `abc` to `p` (Ericson §5.1.5).
pub fn closest_point_on_triangle(p: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> [f64; 3] {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return add(a, scale(ab, v));
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return add(a, scale(ac, w));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return add(b, scale(sub(c, b), w));
    }
    let denom = va + vb + vc;
    if denom == 0.0 || !denom.is_finite() {
        // Degenerate triangle: nearest of the three edges.
        let cands = [
            closest_on_segment(p, a, b),
            closest_on_segment(p, b, c),
            closest_on_segment(p, c, a),
        ];
        return cands
            .into_iter()
            .min_by(|x, y| norm(sub(*x, p)).total_cmp(&norm(sub(*y, p))))
            .unwrap_or(a);
    }
    let v = vb / denom;
    let w = vc / denom;
    add(a, add(scale(ab, v), scale(ac, w)))
}

fn closest_on_segment(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    let ab = sub(b, a);
    let l2 = dot(ab, ab);
    if l2 == 0.0 {
        return a;
    }
    let t = (dot(sub(p, a), ab) / l2).clamp(0.0, 1.0);
    add(a, scale(ab, t))
}

/// Euclidean distance from `p` to the closed triangle `abc`.
pub fn point_triangle_distance(p: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> f64 {
    norm(sub(p, closest_point_on_triangle(p, a, b, c)))
}

/// Whether triangles `t` and `u` **properly** intersect: some edge of one passes
/// through the interior of the other, with every sign test decided by more than
/// `eps` (a length, mm). Contacts within `eps` (touching at a vertex, an edge or
/// along a face) are not intersections. Coplanar pairs (all three vertices of one
/// within `eps` of the other's plane) are tested in 2-D: they intersect when two
/// edges cross properly or a vertex lies strictly inside the other triangle.
pub fn triangles_cross(t: [[f64; 3]; 3], u: [[f64; 3]; 3], eps: f64) -> bool {
    let nu = cross(sub(u[1], u[0]), sub(u[2], u[0]));
    let nt = cross(sub(t[1], t[0]), sub(t[2], t[0]));
    let (lu, lt) = (norm(nu), norm(nt));
    if lu == 0.0 || lt == 0.0 {
        return false;
    }
    let du = t.map(|p| dot(sub(p, u[0]), nu) / lu);
    if du.iter().all(|d| d.abs() <= eps) {
        return coplanar_cross(t, u, nu, eps);
    }
    if du.iter().all(|&d| d > -eps) || du.iter().all(|&d| d < eps) {
        return false;
    }
    let dt = u.map(|p| dot(sub(p, t[0]), nt) / lt);
    if dt.iter().all(|&d| d > -eps) || dt.iter().all(|&d| d < eps) {
        return false;
    }
    // Both triangles straddle the other's plane: the two intersection
    // segments on the common line overlap properly iff they intersect.
    let dir = cross(nt, nu);
    let interval = |tri: [[f64; 3]; 3], d: [f64; 3]| -> Option<(f64, f64)> {
        let mut ts = vec![];
        for k in 0..3 {
            let (i, j) = (k, (k + 1) % 3);
            if (d[i] > eps && d[j] < -eps) || (d[i] < -eps && d[j] > eps) {
                let s = d[i] / (d[i] - d[j]);
                let p = add(tri[i], scale(sub(tri[j], tri[i]), s));
                ts.push(dot(p, dir));
            } else if d[i].abs() <= eps {
                ts.push(dot(tri[i], dir));
            }
        }
        let lo = ts.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = ts.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (lo <= hi).then_some((lo, hi))
    };
    let (Some(a), Some(b)) = (interval(t, du), interval(u, dt)) else {
        return false;
    };
    let dl = norm(dir);
    let overlap = (a.1.min(b.1) - a.0.max(b.0)) / dl;
    overlap > eps
}

fn coplanar_cross(t: [[f64; 3]; 3], u: [[f64; 3]; 3], n: [f64; 3], eps: f64) -> bool {
    // Drop the dominant axis of the normal.
    let k = (0..3)
        .max_by(|&a, &b| n[a].abs().total_cmp(&n[b].abs()))
        .unwrap_or(2);
    let (i, j) = ((k + 1) % 3, (k + 2) % 3);
    let p2 = |p: [f64; 3]| [p[i], p[j]];
    let t2 = t.map(p2);
    let u2 = u.map(p2);
    let orient = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])
    };
    // Signed distance of c from line ab, in length units.
    let side = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        let l = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        if l == 0.0 { 0.0 } else { orient(a, b, c) / l }
    };
    for e in 0..3 {
        let (a, b) = (t2[e], t2[(e + 1) % 3]);
        for f in 0..3 {
            let (c, d) = (u2[f], u2[(f + 1) % 3]);
            let (s1, s2) = (side(a, b, c), side(a, b, d));
            let (s3, s4) = (side(c, d, a), side(c, d, b));
            if ((s1 > eps && s2 < -eps) || (s1 < -eps && s2 > eps))
                && ((s3 > eps && s4 < -eps) || (s3 < -eps && s4 > eps))
            {
                return true;
            }
        }
    }
    let inside = |tri: [[f64; 2]; 3], p: [f64; 2]| {
        let s = [0, 1, 2].map(|e| side(tri[e], tri[(e + 1) % 3], p));
        s.iter().all(|&v| v > eps) || s.iter().all(|&v| v < -eps)
    };
    u2.iter().any(|&p| inside(t2, p)) || t2.iter().any(|&p| inside(u2, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_triangle_regions() {
        let (a, b, c) = ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert_eq!(point_triangle_distance([0.2, 0.2, 3.0], a, b, c), 3.0);
        assert!((point_triangle_distance([-1.0, -1.0, 0.0], a, b, c) - 2f64.sqrt()).abs() < 1e-15);
        assert!((point_triangle_distance([1.0, 1.0, 0.0], a, b, c) - 0.5f64.sqrt()).abs() < 1e-15);
        assert_eq!(point_triangle_distance([0.5, -2.0, 0.0], a, b, c), 2.0);
    }

    #[test]
    fn crossing_and_touching_pairs() {
        let t = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]];
        // Pierces t's interior.
        let u = [[0.5, 0.5, -1.0], [0.5, 0.5, 1.0], [1.5, 1.5, 1.0]];
        assert!(triangles_cross(t, u, 1e-9));
        // Touches t at one vertex from above: not proper.
        let v = [[0.5, 0.5, 0.0], [0.5, 0.5, 1.0], [1.0, 0.5, 1.0]];
        assert!(!triangles_cross(t, v, 1e-9));
        // Shares an edge, folded up: not proper.
        let w = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 0.0, 1.0]];
        assert!(!triangles_cross(t, w, 1e-9));
        // Far away.
        let x = u.map(|p| [p[0] + 10.0, p[1], p[2]]);
        assert!(!triangles_cross(t, x, 1e-9));
        // Coplanar overlap.
        let y = [[0.5, 0.5, 0.0], [3.0, 0.5, 0.0], [0.5, 3.0, 0.0]];
        assert!(triangles_cross(t, y, 1e-9));
        // Coplanar, sharing only an edge.
        let z = [[2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [2.0, 2.0, 0.0]];
        assert!(!triangles_cross(t, z, 1e-9));
    }

    #[test]
    fn grid_registrations_are_bounded() {
        // 216 000 tiny triangles filling a 10 mm cube make the cells small;
        // 300 sloped triangles each span the whole cube. Unbounded, each
        // would be registered in every one of ~4 M cells (~1.2 G entries).
        let mut pos = vec![];
        let mut tris = vec![];
        for i in 0..216_000u32 {
            let (x, y, z) = (i % 60, (i / 60) % 60, i / 3600);
            let p = [f64::from(x), f64::from(y), f64::from(z)].map(|v| v / 6.0);
            let b = pos.len() as u32;
            pos.extend([p, [p[0] + 0.01, p[1], p[2]], [p[0], p[1] + 0.01, p[2]]]);
            tris.push([b, b + 1, b + 2]);
        }
        for k in 0..300 {
            let b = pos.len() as u32;
            let e = 1e-3 * f64::from(k);
            pos.extend([[0.0, 0.0, e], [10.0, 0.0, 10.0], [0.0, 10.0, 10.0 - e]]);
            tris.push([b, b + 1, b + 2]);
        }
        let g = TriGrid::new(&pos, &tris).unwrap();
        assert!((g.items.len() as u64) <= MAX_GRID_REGISTRATIONS);
        assert!(g.cell_count() <= MAX_GRID_CELLS);
    }

    #[test]
    fn grid_nearest_matches_brute_force() {
        // A strip of triangles along x.
        let mut pos = vec![];
        let mut tris = vec![];
        for i in 0..50 {
            let x = i as f64;
            let b = pos.len() as u32;
            pos.extend([[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x, 1.0, (i % 3) as f64]]);
            tris.push([b, b + 1, b + 2]);
        }
        let g = TriGrid::new(&pos, &tris).unwrap();
        for k in 0..40 {
            let p = [
                k as f64 * 1.37 - 3.0,
                (k % 7) as f64 - 2.0,
                (k % 5) as f64 * 0.7,
            ];
            let brute = tris
                .iter()
                .map(|t| {
                    point_triangle_distance(
                        p,
                        pos[t[0] as usize],
                        pos[t[1] as usize],
                        pos[t[2] as usize],
                    )
                })
                .fold(f64::INFINITY, f64::min);
            let (d, _) = g.nearest(&pos, &tris, p);
            assert!((d - brute).abs() < 1e-12, "{p:?}: {d} vs {brute}");
        }
    }
}
