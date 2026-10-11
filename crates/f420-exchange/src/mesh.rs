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

use crate::stl::{cross, sub};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    for t in triangles {
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
        volume6 += dot(ra, cross(rb, rc));
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
    for &(uses, forward) in edges.values() {
        match uses {
            1 => boundary += 1,
            2 => {
                if forward != 1 {
                    inconsistent += 1;
                }
            }
            _ => non_manifold += 1,
        }
    }
    let mut roots = std::collections::HashSet::new();
    for (i, _) in used.iter().enumerate().filter(|(_, u)| **u) {
        roots.insert(find(&mut parent, i as u32));
    }
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
    }
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
