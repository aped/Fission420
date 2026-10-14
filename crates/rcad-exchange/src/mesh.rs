//! Diagnostics of an indexed triangle mesh (e.g. an imported STL).
//!
//! Definitions (all topological tests use vertex **indices**, never coordinates):
//! * An *edge* is an unordered vertex-index pair used by at least one valid triangle.
//! * *Degenerate triangle*: two equal indices, or zero area
//!   (`|(b − a) × (c − a)| <= 1e-12 × (longest edge)²`, i.e. relative; scale free). Triangles
//!   with repeated indices take no part in the edge analysis; zero-area triangles with three
//!   distinct indices do (they are topologically real, e.g. T-junction fillers).
//! * *Invalid triangle*: an index outside `positions` (counted, otherwise ignored).
//! * *Boundary edge*: used by exactly one triangle. *Non-manifold edge*: used by three or more.
//! * *Inconsistently oriented edge*: used by exactly two triangles that traverse it in the
//!   same direction (one of them is flipped).
//! * `closed`: at least one triangle, no invalid or repeated-index triangle, and every edge is
//!   shared by exactly two consistently oriented triangles.
//! * `signed_volume` (divergence theorem, `Σ a·(b×c)/6` about the bounds centre): **only**
//!   when `closed`; positive for outward (counter-clockwise from outside) winding. An open
//!   mesh never gets a volume.
//! * `components`: groups of triangles connected through shared vertex indices.
//!   `component_details` lists the largest [`MAX_COMPONENT_DETAILS`] of them (by triangle
//!   count, then by bounds) with their own bounds, closedness (the `closed` rule applied to
//!   the component's edges) and signed volume (only when that component is closed).
//! * *Duplicate triangle* (`duplicate_triangles`): a valid triangle with the same three
//!   vertex indices (any order or orientation) as an earlier one in the list; the first
//!   occurrence is not counted.
//!
//! Self-intersection is a separate, bounded and more expensive check:
//! [`self_intersections`] (definition and limits there).

use crate::mesh_query::{TriGrid, bounds_of, triangles_cross};
use crate::stl::{cross, sub};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Per-component accumulator: triangles, 6 × signed volume, bounds.
type CompAcc = (usize, f64, ([f64; 3], [f64; 3]));

/// At most this many components are described in `component_details`.
pub const MAX_COMPONENT_DETAILS: usize = 256;

/// One connected component of a mesh (see the module docs).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ComponentInfo {
    pub triangles: usize,
    /// Bounds of the component's vertices `(min, max)`, mm.
    pub bounds: ([f64; 3], [f64; 3]),
    pub closed: bool,
    /// Enclosed volume (outward winding positive); `Some` only when `closed`.
    pub signed_volume: Option<f64>,
}

/// Relative zero-area threshold, see the module docs. Dimensionless.
pub const DEGENERATE_REL_AREA: f64 = 1e-12;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MeshDiagnostics {
    pub triangles: usize,
    /// Number of positions.
    pub vertices: usize,
    /// Axis-aligned bounds `(min, max)` of all positions; `None` when there are none.
    pub bounds: Option<([f64; 3], [f64; 3])>,
    pub degenerate_triangles: usize,
    /// Triangles referencing a vertex index outside `positions`.
    pub invalid_triangles: usize,
    pub boundary_edges: usize,
    pub non_manifold_edges: usize,
    pub inconsistent_orientation_edges: usize,
    pub components: usize,
    /// Every edge shared by exactly two consistently oriented triangles.
    pub closed: bool,
    /// Enclosed volume (positive for outward winding); `Some` only when `closed`.
    pub signed_volume: Option<f64>,
    /// Sum of triangle areas (valid triangles).
    pub surface_area: f64,
    /// Valid triangles repeating an earlier triangle's three vertices.
    #[serde(default)]
    pub duplicate_triangles: usize,
    /// The largest components (at most [`MAX_COMPONENT_DETAILS`]), largest first.
    #[serde(default)]
    pub component_details: Vec<ComponentInfo>,
}

fn find(parent: &mut [u32], mut i: u32) -> u32 {
    while parent[i as usize] != i {
        let p = parent[i as usize];
        parent[i as usize] = parent[p as usize];
        i = p;
    }
    i
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Diagnose a mesh. Linear in the number of triangles (one hash map of edges).
pub fn diagnose(positions: &[[f64; 3]], triangles: &[[u32; 3]]) -> MeshDiagnostics {
    let bounds = positions
        .iter()
        .fold(None, |acc: Option<([f64; 3], [f64; 3])>, p| {
            Some(match acc {
                None => (*p, *p),
                Some((lo, hi)) => (
                    [lo[0].min(p[0]), lo[1].min(p[1]), lo[2].min(p[2])],
                    [hi[0].max(p[0]), hi[1].max(p[1]), hi[2].max(p[2])],
                ),
            })
        });
    let centre = bounds.map_or([0.0; 3], |(lo, hi)| {
        [
            0.5 * (lo[0] + hi[0]),
            0.5 * (lo[1] + hi[1]),
            0.5 * (lo[2] + hi[2]),
        ]
    });
    let n = positions.len();
    let mut degenerate = 0usize;
    let mut invalid = 0usize;
    let mut repeated = 0usize;
    let mut area2_sum = 0.0;
    let mut volume6 = 0.0;
    // (min, max) -> (uses, uses traversed min -> max)
    let mut edges: HashMap<(u32, u32), (u32, u32)> = HashMap::with_capacity(triangles.len() * 2);
    let mut parent: Vec<u32> = (0..n.min(u32::MAX as usize) as u32).collect();
    let mut used = vec![false; n];
    let mut seen_tris: HashSet<[u32; 3]> = HashSet::with_capacity(triangles.len());
    let mut duplicates = 0usize;
    // Valid triangles (index, 6 × signed volume contribution).
    let mut valid_tris: Vec<(usize, f64)> = Vec::with_capacity(triangles.len());
    for (ti, t) in triangles.iter().enumerate() {
        if t.iter().any(|&i| i as usize >= parent.len()) {
            invalid += 1;
            continue;
        }
        let [a, b, c] = t.map(|i| positions[i as usize]);
        let (ab, ac, bc) = (sub(b, a), sub(c, a), sub(c, b));
        let cr = cross(ab, ac);
        let area2 = dot(cr, cr).sqrt();
        let longest2 = dot(ab, ab).max(dot(ac, ac)).max(dot(bc, bc));
        let has_repeat = t[0] == t[1] || t[1] == t[2] || t[0] == t[2];
        let tiny = area2.partial_cmp(&(DEGENERATE_REL_AREA * longest2))
            != Some(std::cmp::Ordering::Greater);
        if has_repeat || tiny {
            degenerate += 1;
        }
        if has_repeat {
            repeated += 1;
            continue;
        }
        area2_sum += area2;
        let (ra, rb, rc) = (sub(a, centre), sub(b, centre), sub(c, centre));
        let v6 = dot(ra, cross(rb, rc));
        volume6 += v6;
        valid_tris.push((ti, v6));
        let mut key = *t;
        key.sort_unstable();
        if !seen_tris.insert(key) {
            duplicates += 1;
        }
        for k in 0..3 {
            let (u, v) = (t[k], t[(k + 1) % 3]);
            let e = edges.entry((u.min(v), u.max(v))).or_insert((0, 0));
            e.0 += 1;
            if u < v {
                e.1 += 1;
            }
        }
        for &i in t {
            used[i as usize] = true;
        }
        let r0 = find(&mut parent, t[0]);
        for &i in &t[1..] {
            let r = find(&mut parent, i);
            if r != r0 {
                parent[r as usize] = r0;
            }
        }
    }
    let (mut boundary, mut non_manifold, mut inconsistent) = (0, 0, 0);
    // Component root -> number of edges violating the closed rule.
    let mut bad_edges: HashMap<u32, usize> = HashMap::new();
    for (&(u, _), &(uses, forward)) in &edges {
        let bad = match uses {
            1 => {
                boundary += 1;
                true
            }
            2 => {
                if forward != 1 {
                    inconsistent += 1;
                }
                forward != 1
            }
            _ => {
                non_manifold += 1;
                true
            }
        };
        if bad {
            *bad_edges.entry(find(&mut parent, u)).or_default() += 1;
        }
    }
    let mut roots = HashSet::new();
    for (i, _) in used.iter().enumerate().filter(|(_, u)| **u) {
        roots.insert(find(&mut parent, i as u32));
    }
    // Per-component triangles, volume and bounds.
    let mut comp: HashMap<u32, CompAcc> = HashMap::new();
    for &(ti, v6) in &valid_tris {
        let t = triangles[ti];
        let r = find(&mut parent, t[0]);
        let tb = bounds_of(t.iter().map(|&i| &positions[i as usize])).unwrap_or_default();
        let e = comp.entry(r).or_insert((0, 0.0, tb));
        e.0 += 1;
        e.1 += v6;
        for k in 0..3 {
            e.2.0[k] = e.2.0[k].min(tb.0[k]);
            e.2.1[k] = e.2.1[k].max(tb.1[k]);
        }
    }
    let mut component_details: Vec<ComponentInfo> = comp
        .iter()
        .map(|(r, &(tris, v6, b))| {
            let closed = invalid == 0 && repeated == 0 && !bad_edges.contains_key(r);
            ComponentInfo {
                triangles: tris,
                bounds: b,
                closed,
                signed_volume: closed.then_some(v6 / 6.0),
            }
        })
        .collect();
    component_details.sort_by(|a, b| {
        b.triangles
            .cmp(&a.triangles)
            .then(a.bounds.0[0].total_cmp(&b.bounds.0[0]))
            .then(a.bounds.0[1].total_cmp(&b.bounds.0[1]))
            .then(a.bounds.0[2].total_cmp(&b.bounds.0[2]))
            .then(a.bounds.1[0].total_cmp(&b.bounds.1[0]))
    });
    component_details.truncate(MAX_COMPONENT_DETAILS);
    let valid = triangles.len() - invalid - repeated;
    let closed = valid > 0
        && invalid == 0
        && repeated == 0
        && boundary == 0
        && non_manifold == 0
        && inconsistent == 0;
    MeshDiagnostics {
        triangles: triangles.len(),
        vertices: n,
        bounds,
        degenerate_triangles: degenerate,
        invalid_triangles: invalid,
        boundary_edges: boundary,
        non_manifold_edges: non_manifold,
        inconsistent_orientation_edges: inconsistent,
        components: roots.len(),
        closed,
        signed_volume: closed.then_some(volume6 / 6.0),
        surface_area: 0.5 * area2_sum,
        duplicate_triangles: duplicates,
        component_details,
    }
}

/// Bounds of a self-intersection check.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct SelfIntersectionLimits {
    /// Triangles indexed; a larger mesh is checked on its first `max_triangles` only.
    pub max_triangles: usize,
    /// Candidate pairs visited (tested or skipped as neighbours / tested in
    /// another cell); the check stops (incomplete) when they are used up, so
    /// the work is bounded even for a fan of many triangles around one vertex.
    pub max_pair_tests: u64,
    /// Intersecting pairs listed in `examples`.
    pub max_examples: usize,
}

impl Default for SelfIntersectionLimits {
    fn default() -> Self {
        SelfIntersectionLimits {
            max_triangles: 2_000_000,
            max_pair_tests: 50_000_000,
            max_examples: 20,
        }
    }
}

/// Result of [`self_intersections`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SelfIntersectionReport {
    /// How the check works and what it does not cover (shown to the user).
    pub method: String,
    pub triangles_total: usize,
    pub triangles_checked: usize,
    pub pairs_tested: u64,
    /// Candidate pairs visited (bounded by `max_pair_tests`).
    #[serde(default)]
    pub pairs_visited: u64,
    /// Pairs of non-adjacent triangles that properly intersect.
    pub intersecting_pairs: usize,
    /// Up to `max_examples` intersecting pairs (triangle indices).
    pub examples: Vec<(u32, u32)>,
    /// Contact tolerance used, mm (`1e-9 ×` bounds diagonal).
    pub tolerance: f64,
    /// Every candidate pair was tested. When false the result is a sample and
    /// "none found" is **not** a proof.
    pub complete: bool,
}

/// Bounded self-intersection check.
///
/// Definition: two triangles intersect when [`crate::mesh_query::triangles_cross`] holds
/// with `eps = 1e-9 ×` the mesh's bounds diagonal, i.e. one passes properly through the
/// other (or coplanar ones overlap with area); touching contacts within `eps` do not count.
/// Pairs that share a vertex (after merging bit-identical coordinates) are **not tested**:
/// neighbours always touch, so a fold between adjacent triangles is not detected. Distinct
/// components that interpenetrate are detected (they are pairs too).
///
/// Method: a uniform grid of triangle bounding boxes ([`TriGrid`]); every pair registered
/// in a common cell with overlapping bounding boxes is tested once (in its lowest common
/// cell). Bounded by [`SelfIntersectionLimits`] (every visited candidate pair counts);
/// when a bound is hit `complete` is false.
/// Floating-point tests with a tolerance: a found pair is reliable to that tolerance, but
/// a complete check finding none is evidence, not an exact proof.
pub fn self_intersections(
    positions: &[[f64; 3]],
    triangles: &[[u32; 3]],
    limits: &SelfIntersectionLimits,
) -> SelfIntersectionReport {
    // Exact weld so adjacency is by coordinates, keeping triangle numbering.
    let mut map: HashMap<[u64; 3], u32> = HashMap::with_capacity(positions.len());
    let mut remap = Vec::with_capacity(positions.len());
    let mut pos: Vec<[f64; 3]> = Vec::new();
    for p in positions {
        let key = p.map(|x| if x == 0.0 { 0u64 } else { x.to_bits() });
        let i = *map.entry(key).or_insert_with(|| {
            pos.push(*p);
            (pos.len() - 1) as u32
        });
        remap.push(i);
    }
    let checked = triangles.len().min(limits.max_triangles);
    // (original index, welded triangle) of the valid triangles checked.
    let mut ids: Vec<u32> = Vec::new();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    for (i, t) in triangles[..checked].iter().enumerate() {
        if t.iter().all(|&v| (v as usize) < remap.len()) {
            let w = t.map(|v| remap[v as usize]);
            if w[0] != w[1] && w[1] != w[2] && w[0] != w[2] {
                ids.push(i as u32);
                tris.push(w);
            }
        }
    }
    let diag = bounds_of(pos.iter()).map_or(0.0, |(lo, hi)| {
        let d = sub(hi, lo);
        dot(d, d).sqrt()
    });
    let eps = 1e-9 * diag;
    let mut report = SelfIntersectionReport {
        method: format!(
            "triangle-pair test on a uniform grid ({checked} of {} triangles); pairs sharing a \
             vertex are not tested; contacts within {eps:.3e} mm are not intersections",
            triangles.len()
        ),
        triangles_total: triangles.len(),
        triangles_checked: checked,
        pairs_tested: 0,
        pairs_visited: 0,
        intersecting_pairs: 0,
        examples: vec![],
        tolerance: eps,
        complete: checked == triangles.len(),
    };
    let Some(grid) = TriGrid::new(&pos, &tris) else {
        return report;
    };
    let ranges: Vec<([usize; 3], [usize; 3])> =
        tris.iter().map(|t| grid.tri_range(&pos, t)).collect();
    let tri_pts = |t: &[u32; 3]| t.map(|i| pos[i as usize]);
    'cells: for c in 0..grid.cell_count() {
        let items = grid.cell_items(c);
        if items.len() < 2 {
            continue;
        }
        let cc = grid.cell_coord(c);
        for (k, &a) in items.iter().enumerate() {
            for &b in &items[k + 1..] {
                // Every visited pair counts, skipped ones included.
                if report.pairs_visited >= limits.max_pair_tests {
                    report.complete = false;
                    break 'cells;
                }
                report.pairs_visited += 1;
                let (ra, rb) = (ranges[a as usize], ranges[b as usize]);
                // Test each pair once: in the lowest cell both occupy.
                let first = [0, 1, 2].map(|i| ra.0[i].max(rb.0[i]));
                if first != cc {
                    continue;
                }
                let (ta, tb) = (tris[a as usize], tris[b as usize]);
                if ta.iter().any(|v| tb.contains(v)) {
                    continue;
                }
                report.pairs_tested += 1;
                if triangles_cross(tri_pts(&ta), tri_pts(&tb), eps) {
                    report.intersecting_pairs += 1;
                    if report.examples.len() < limits.max_examples {
                        let (x, y) = (ids[a as usize], ids[b as usize]);
                        report.examples.push((x.min(y), x.max(y)));
                    }
                }
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIT_TET: [[f64; 3]; 4] = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
    ];
    const TET: [[u32; 3]; 4] = [[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]];

    #[test]
    fn closed_tetrahedron() {
        let d = diagnose(&UNIT_TET, &TET);
        assert!(d.closed);
        assert_eq!(d.components, 1);
        assert!((d.signed_volume.unwrap() - 1.0 / 6.0).abs() < 1e-15);
        let area = 1.5 + 3f64.sqrt() / 2.0;
        assert!((d.surface_area - area).abs() < 1e-14);
        assert_eq!(d.bounds, Some(([0.0; 3], [1.0; 3])));
    }

    #[test]
    fn flipped_face_open_face_nonmanifold_and_bad_indices() {
        let mut t = TET;
        t[3] = [1, 3, 2];
        let d = diagnose(&UNIT_TET, &t);
        assert_eq!(d.inconsistent_orientation_edges, 3);
        assert!(!d.closed && d.signed_volume.is_none());

        let d = diagnose(&UNIT_TET, &TET[..3]);
        assert_eq!(d.boundary_edges, 3);
        assert!(!d.closed && d.signed_volume.is_none());

        let mut t = TET.to_vec();
        t.push([0, 1, 2]);
        let d = diagnose(&UNIT_TET, &t);
        assert_eq!(d.non_manifold_edges, 3);
        assert!(!d.closed);

        let mut t = TET.to_vec();
        t.push([0, 1, 7]);
        t.push([0, 0, 1]);
        let d = diagnose(&UNIT_TET, &t);
        assert_eq!(d.invalid_triangles, 1);
        assert_eq!(d.degenerate_triangles, 1);
        assert!(!d.closed);
        assert!(d.signed_volume.is_none());
    }

    #[test]
    fn two_components_and_inward_winding() {
        let mut p = UNIT_TET.to_vec();
        p.extend(UNIT_TET.iter().map(|v| [v[0] + 5.0, v[1], v[2]]));
        let mut t = TET.to_vec();
        t.extend(TET.iter().map(|f| [f[0] + 4, f[2] + 4, f[1] + 4]));
        let d = diagnose(&p, &t);
        assert_eq!(d.components, 2);
        assert!(d.closed);
        assert!(d.signed_volume.unwrap().abs() < 1e-14); // +1/6 - 1/6

        let inward: Vec<_> = TET.iter().map(|f| [f[0], f[2], f[1]]).collect();
        let d = diagnose(&UNIT_TET, &inward);
        assert!((d.signed_volume.unwrap() + 1.0 / 6.0).abs() < 1e-15);
    }

    #[test]
    fn empty_mesh() {
        let d = diagnose(&[], &[]);
        assert!(!d.closed);
        assert_eq!(d.bounds, None);
        assert_eq!(d.components, 0);
    }
}
