//! Feature-edge extraction from a display mesh.
//!
//! A feature edge is a triangle edge on a group boundary: its adjacent
//! triangles carry different `tri_group` ids, or it has exactly one adjacent
//! triangle (open boundary, or a T-junction where neighbouring faces were
//! tessellated independently). Edges inside a single group are never drawn,
//! so smooth curved faces show no facet lines.
//!
//! Tessellations usually duplicate vertices per face (so normals can differ),
//! hence edges are matched by quantized *position*, not by index. The quantum
//! is `max(diag * 1e-5, max|coord| * 4e-7, 1e-9)`:
//! - `diag * 1e-5` merges positions that agree to 10 ppm of the mesh size
//!   (1 µm on a 100 mm part), far below any modelled feature;
//! - `max|coord| * 4e-7` keeps the quantum above a few f32 ulps for parts far
//!   from the origin;
//! - `1e-9` avoids division by zero for degenerate meshes.
//!
//! Rounding to a grid can split two nearly-equal values that straddle a cell
//! boundary; the result is then an extra (duplicate) edge drawn on top of the
//! true one, which is visually harmless. This is a display cache only.

use std::collections::HashMap;

use crate::math::{self, Vec3};
use crate::mesh::RenderMesh;

type QPos = [i64; 3];

struct EdgeInfo {
    a: Vec3,
    b: Vec3,
    first_group: u32,
    count: u32,
    mixed: bool,
}

/// Quantum used to match positions for `mesh` (see module docs).
pub fn edge_quantum(mesh: &RenderMesh) -> f32 {
    let Some(b) = mesh.bounds() else {
        return 1.0e-9;
    };
    let diag = math::length(math::sub(b.max, b.min));
    let max_abs = b
        .min
        .iter()
        .chain(b.max.iter())
        .fold(0.0f32, |m, v| m.max(v.abs()));
    (diag * 1.0e-5).max(max_abs * 4.0e-7).max(1.0e-9)
}

/// Group-boundary and open-boundary edges of `mesh` as world-space segments.
/// Invalid triangles (out-of-range indices) are skipped. Output order is
/// deterministic (sorted by quantized endpoints).
pub fn feature_edges(mesh: &RenderMesh) -> Vec<[Vec3; 2]> {
    let q = edge_quantum(mesh);
    let quant = |p: Vec3| -> QPos { p.map(|c| (f64::from(c) / f64::from(q)).round() as i64) };
    let mut map: HashMap<(QPos, QPos), EdgeInfo> = HashMap::new();
    for (t, idx) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        let group = mesh.tri_group.get(t).copied().unwrap_or(u32::MAX);
        let p = |k: usize| mesh.positions.get(idx[k] as usize).copied();
        let (Some(a), Some(b), Some(c)) = (p(0), p(1), p(2)) else {
            continue;
        };
        for (u, v) in [(a, b), (b, c), (c, a)] {
            let (qu, qv) = (quant(u), quant(v));
            if qu == qv {
                continue; // degenerate edge
            }
            let (key, ends) = if qu < qv {
                ((qu, qv), (u, v))
            } else {
                ((qv, qu), (v, u))
            };
            map.entry(key)
                .and_modify(|e| {
                    e.count += 1;
                    e.mixed |= e.first_group != group;
                })
                .or_insert(EdgeInfo {
                    a: ends.0,
                    b: ends.1,
                    first_group: group,
                    count: 1,
                    mixed: false,
                });
        }
    }
    let mut out: Vec<((QPos, QPos), [Vec3; 2])> = map
        .into_iter()
        .filter(|(_, e)| e.count == 1 || e.mixed)
        .map(|(k, e)| (k, [e.a, e.b]))
        .collect();
    out.sort_unstable_by_key(|x| x.0);
    out.into_iter().map(|(_, s)| s).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::box_mesh;

    #[test]
    fn box_has_twelve_feature_edges() {
        let m = box_mesh([0.0; 3], [10.0, 20.0, 30.0]);
        let e = feature_edges(&m);
        assert_eq!(e.len(), 12);
        // Every edge is axis-aligned with one of the box's three lengths;
        // no quad diagonal survives.
        let mut lengths: Vec<f32> = e
            .iter()
            .map(|s| math::length(math::sub(s[1], s[0])))
            .collect();
        lengths.sort_by(f32::total_cmp);
        let expect = [
            10.0, 10.0, 10.0, 10.0, 20.0, 20.0, 20.0, 20.0, 30.0, 30.0, 30.0, 30.0,
        ];
        for (l, x) in lengths.iter().zip(expect) {
            assert!((l - x).abs() < 1e-4, "{lengths:?}");
        }
    }

    #[test]
    fn single_group_box_has_no_edges_and_open_quad_has_four() {
        let mut m = box_mesh([0.0; 3], [1.0; 3]);
        m.tri_group.iter_mut().for_each(|g| *g = 7);
        assert!(feature_edges(&m).is_empty());
        // One face only (two triangles): the four outline edges are open.
        let quad = RenderMesh {
            positions: m.positions[..4].to_vec(),
            normals: m.normals[..4].to_vec(),
            indices: m.indices[..6].to_vec(),
            tri_group: vec![0, 0],
        };
        assert_eq!(feature_edges(&quad).len(), 4);
    }

    #[test]
    fn near_duplicate_positions_are_merged() {
        // Perturb one face's copies of the shared corners by far less than the
        // quantum: edge count is unchanged.
        let mut m = box_mesh([100.0, 100.0, 0.0], [150.0, 160.0, 40.0]);
        let q = edge_quantum(&m);
        for p in &mut m.positions[..4] {
            p[0] += q * 0.01;
        }
        assert_eq!(feature_edges(&m).len(), 12);
    }

    #[test]
    fn bad_indices_are_skipped() {
        let mut m = box_mesh([0.0; 3], [1.0; 3]);
        m.indices[0] = 9999;
        // Must not panic; the broken triangle simply contributes nothing.
        let _ = feature_edges(&m);
    }
}
