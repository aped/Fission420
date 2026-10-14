//! Mesh inspection and edits against independently known answers: voxel solids
//! (exact volumes and bounds by counting unit cubes) and a UV sphere (volume bounds).
use rcad_exchange::mesh::{SelfIntersectionLimits, diagnose, self_intersections};
use rcad_exchange::mesh_edit::{
    CutKeep, Decimate, MeshEditOp, MeshTransform, PlaneCut, edit, measure_deviation,
};
use std::collections::{HashMap, HashSet};

type Mesh = (Vec<[f64; 3]>, Vec<[u32; 3]>);

/// Closed, outward mesh of the union of unit voxels (faces between filled voxels
/// are left out; each exposed face is two triangles).
fn voxels(cells: &[[i32; 3]]) -> Mesh {
    let set: HashSet<[i32; 3]> = cells.iter().copied().collect();
    let mut pos = vec![];
    let mut idx: HashMap<[i32; 3], u32> = HashMap::new();
    let mut tris = vec![];
    let mut vid = |p: [i32; 3], pos: &mut Vec<[f64; 3]>| {
        *idx.entry(p).or_insert_with(|| {
            pos.push(p.map(f64::from));
            (pos.len() - 1) as u32
        })
    };
    for c in cells {
        for axis in 0..3 {
            for dir in [-1, 1] {
                let mut nb = *c;
                nb[axis] += dir;
                if set.contains(&nb) {
                    continue;
                }
                let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                let mut corner = |du: i32, dv: i32| {
                    let mut p = *c;
                    if dir > 0 {
                        p[axis] += 1;
                    }
                    p[u] += du;
                    p[v] += dv;
                    vid(p, &mut pos)
                };
                let (a, b, cc, d) = (corner(0, 0), corner(1, 0), corner(1, 1), corner(0, 1));
                if dir > 0 {
                    tris.push([a, b, cc]);
                    tris.push([a, cc, d]);
                } else {
                    tris.push([a, cc, b]);
                    tris.push([a, d, cc]);
                }
            }
        }
    }
    (pos, tris)
}

fn block(nx: i32, ny: i32, nz: i32) -> Vec<[i32; 3]> {
    let mut v = vec![];
    for x in 0..nx {
        for y in 0..ny {
            for z in 0..nz {
                v.push([x, y, z]);
            }
        }
    }
    v
}

fn sphere(r: f64, nu: usize, nv: usize) -> Mesh {
    let mut pos = vec![[0.0, 0.0, r]];
    for j in 1..nv {
        let th = std::f64::consts::PI * j as f64 / nv as f64;
        for i in 0..nu {
            let ph = 2.0 * std::f64::consts::PI * i as f64 / nu as f64;
            pos.push([
                r * th.sin() * ph.cos(),
                r * th.sin() * ph.sin(),
                r * th.cos(),
            ]);
        }
    }
    pos.push([0.0, 0.0, -r]);
    let south = (pos.len() - 1) as u32;
    let ring = |j: usize, i: usize| (1 + (j - 1) * nu + i % nu) as u32;
    let mut t = vec![];
    for i in 0..nu {
        t.push([0, ring(1, i), ring(1, i + 1)]);
        t.push([south, ring(nv - 1, i + 1), ring(nv - 1, i)]);
    }
    for j in 1..nv - 1 {
        for i in 0..nu {
            let (a, b, c, d) = (
                ring(j, i),
                ring(j, i + 1),
                ring(j + 1, i + 1),
                ring(j + 1, i),
            );
            t.push([a, d, c]);
            t.push([a, c, b]);
        }
    }
    (pos, t)
}

fn vol(m: &Mesh) -> f64 {
    let d = diagnose(&m.0, &m.1);
    assert!(d.closed, "not closed: {d:?}");
    d.signed_volume.unwrap()
}

#[test]
fn diagnosis_reports_duplicates_and_component_details() {
    let (mut p, mut t) = voxels(&block(2, 1, 1));
    // A second component: a unit cube at x = 5.
    let (p2, t2) = voxels(&[[5, 0, 0]]);
    let off = p.len() as u32;
    p.extend(p2);
    t.extend(t2.iter().map(|f| f.map(|i| i + off)));
    let d = diagnose(&p, &t);
    assert!(d.closed);
    assert_eq!(d.components, 2);
    assert_eq!(d.duplicate_triangles, 0);
    assert_eq!(d.component_details.len(), 2);
    let big = &d.component_details[0];
    assert_eq!(big.triangles, 20);
    assert!((big.signed_volume.unwrap() - 2.0).abs() < 1e-12);
    assert_eq!(big.bounds, ([0.0; 3], [2.0, 1.0, 1.0]));
    let small = &d.component_details[1];
    assert!((small.signed_volume.unwrap() - 1.0).abs() < 1e-12);
    assert_eq!(small.bounds, ([5.0, 0.0, 0.0], [6.0, 1.0, 1.0]));

    // A duplicate (reversed) triangle: counted, and that component is no longer closed.
    let mut t3 = t.clone();
    let f = t3[0];
    t3.push([f[0], f[2], f[1]]);
    let d = diagnose(&p, &t3);
    assert_eq!(d.duplicate_triangles, 1);
    assert!(!d.closed);
    assert!(!d.component_details[0].closed);
    assert!(d.component_details[0].signed_volume.is_none());
    assert!(d.component_details[1].closed);
}

#[test]
fn self_intersection_finds_interpenetrating_shells_only() {
    let lim = SelfIntersectionLimits::default();
    let (p, t) = voxels(&block(3, 3, 3));
    let r = self_intersections(&p, &t, &lim);
    assert!(r.complete);
    assert_eq!(r.intersecting_pairs, 0, "{r:?}");
    assert!(r.pairs_tested > 0);

    // Second cube shifted by (0.5, 0.5, 0.5): interpenetrates.
    let (q, u) = voxels(&block(1, 1, 1));
    let mut p2 = p.clone();
    let mut t2 = t.clone();
    let off = p2.len() as u32;
    p2.extend(q.iter().map(|v| [v[0] + 2.5, v[1] + 2.5, v[2] + 2.5]));
    t2.extend(u.iter().map(|f| f.map(|i| i + off)));
    let r = self_intersections(&p2, &t2, &lim);
    assert!(r.complete);
    assert!(r.intersecting_pairs > 0);
    assert!(!r.examples.is_empty());
    for (a, b) in &r.examples {
        assert!((*a as usize) < t.len() && (*b as usize) >= t.len());
    }

    // Same but disjoint (shifted by 5): none.
    let mut p3 = p.clone();
    p3.extend(q.iter().map(|v| [v[0] + 5.0, v[1], v[2]]));
    let r = self_intersections(&p3, &t2, &lim);
    assert_eq!(r.intersecting_pairs, 0);

    // A pair budget of 1: incomplete, labelled so.
    let r = self_intersections(
        &p2,
        &t2,
        &SelfIntersectionLimits {
            max_pair_tests: 1,
            ..lim
        },
    );
    assert!(!r.complete);
}

#[test]
fn transform_scales_rotates_and_translates() {
    let m = voxels(&block(2, 1, 1));
    let op = MeshEditOp::Transform(MeshTransform::unit_conversion("inch").unwrap());
    let (out, rep) = edit(&m.0, &m.1, &op).unwrap();
    assert!((vol(&out[0]) - 2.0 * 25.4f64.powi(3)).abs() < 1e-6);
    assert_eq!(rep.vertices_moved, out[0].0.len() - 1); // the origin stays
    let op = MeshEditOp::Transform(MeshTransform {
        scale: [2.0, 3.0, 0.5],
        axis: [0.0, 0.0, 1.0],
        angle_deg: 90.0,
        translate: [10.0, 0.0, 0.0],
    });
    let (out, _) = edit(&m.0, &m.1, &op).unwrap();
    assert!((vol(&out[0]) - 2.0 * 3.0).abs() < 1e-9);
    let d = diagnose(&out[0].0, &out[0].1);
    // (x,y) scaled to (4,3) then turned +90° about z: x' = -y, y' = x.
    let (lo, hi) = d.bounds.unwrap();
    for (a, b) in lo
        .iter()
        .zip([7.0, 0.0, 0.0])
        .chain(hi.iter().zip([10.0, 4.0, 0.5]))
    {
        assert!((a - b).abs() < 1e-12, "{lo:?} {hi:?}");
    }
    // Mirroring and zero scale refused.
    for s in [[-1.0, 1.0, 1.0], [0.0, 1.0, 1.0]] {
        let op = MeshEditOp::Transform(MeshTransform {
            scale: s,
            ..Default::default()
        });
        assert!(edit(&m.0, &m.1, &op).is_err());
    }
}

fn cut(
    m: &Mesh,
    origin: [f64; 3],
    normal: [f64; 3],
    keep: CutKeep,
    cap: bool,
) -> Result<Vec<Mesh>, String> {
    edit(
        &m.0,
        &m.1,
        &MeshEditOp::PlaneCut(PlaneCut {
            origin,
            normal,
            offset: 0.0,
            keep,
            cap,
        }),
    )
    .map(|(o, _)| o)
}

#[test]
fn plane_cut_caps_closed_meshes() {
    let m = voxels(&block(2, 2, 2));
    // Through vertices and along existing edges (z = 1).
    let out = cut(&m, [0.0, 0.0, 1.0], [0.0, 0.0, 1.0], CutKeep::Below, true).unwrap();
    assert!((vol(&out[0]) - 4.0).abs() < 1e-12);
    // Oblique cut through the cube's diagonal: two halves of 4 each.
    let (out, rep) = edit(
        &m.0,
        &m.1,
        &MeshEditOp::PlaneCut(PlaneCut {
            origin: [1.0, 1.0, 1.0],
            normal: [1.0, 1.0, 0.3],
            offset: 0.0,
            keep: CutKeep::Both,
            cap: true,
        }),
    )
    .unwrap();
    assert_eq!(rep.triangles_removed, 0, "both sides keep everything");
    let (a, b) = (vol(&out[0]), vol(&out[1]));
    assert!((a - 4.0).abs() < 1e-9 && (b - 4.0).abs() < 1e-9, "{a} {b}");
    // A generic plane: halves sum to the whole; each half's bounds end at the plane.
    let out = cut(&m, [0.3, 0.2, 0.77], [0.2, -0.1, 1.0], CutKeep::Both, true).unwrap();
    assert!((vol(&out[0]) + vol(&out[1]) - 8.0).abs() < 1e-9);

    // A ring (3×3 minus the centre): the cap has a hole.
    let mut ring = block(3, 3, 1);
    ring.retain(|c| *c != [1, 1, 0]);
    let m = voxels(&ring);
    let (out, rep) = edit(
        &m.0,
        &m.1,
        &MeshEditOp::PlaneCut(PlaneCut {
            origin: [0.0, 0.0, 0.4],
            normal: [0.0, 0.0, 1.0],
            offset: 0.0,
            keep: CutKeep::Above,
            cap: true,
        }),
    )
    .unwrap();
    assert!((vol(&out[0]) - 8.0 * 0.6).abs() < 1e-9);
    assert_eq!(rep.cut_loops, 2);
    assert_eq!(rep.capped_loops, 2);
    // Keep above: the 8 bottom-face cells (16 triangles) are dropped whole.
    assert_eq!(rep.triangles_removed, 16);
    assert!(rep.triangles_cut > 0 && rep.triangles_added > 0);

    // Two components cut at once.
    let mut two = block(1, 1, 2);
    two.extend(block(1, 1, 2).iter().map(|c| [c[0] + 3, c[1], c[2]]));
    let m = voxels(&two);
    let out = cut(&m, [0.0, 0.0, 1.5], [0.0, 0.0, 1.0], CutKeep::Below, true).unwrap();
    assert!((vol(&out[0]) - 3.0).abs() < 1e-12);
    assert_eq!(diagnose(&out[0].0, &out[0].1).components, 2);

    // The sphere: halves sum to the whole, max z of the lower half is the plane.
    let s = sphere(10.0, 48, 24);
    let v = vol(&s);
    let out = cut(&s, [0.0, 0.0, 3.0], [0.0, 0.0, 1.0], CutKeep::Both, true).unwrap();
    assert!((vol(&out[0]) + vol(&out[1]) - v).abs() < 1e-9 * v);
    let hi = diagnose(&out[0].0, &out[0].1).bounds.unwrap().1;
    assert!((hi[2] - 3.0).abs() < 1e-12);
}

#[test]
fn plane_cut_refusals_and_open_results() {
    let m = voxels(&block(2, 2, 2));
    assert!(cut(&m, [0.0, 0.0, 5.0], [0.0, 0.0, 1.0], CutKeep::Below, true).is_err());
    assert!(cut(&m, [0.0, 0.0, 2.0], [0.0, 0.0, 1.0], CutKeep::Below, true).is_err());
    assert!(cut(&m, [0.0; 3], [0.0; 3], CutKeep::Below, true).is_err());
    // Without cap: open, boundary is the cut square (8 edges on the 2×2 grid).
    let out = cut(&m, [0.0, 0.0, 1.0], [0.0, 0.0, 1.0], CutKeep::Below, false).unwrap();
    let d = diagnose(&out[0].0, &out[0].1);
    assert!(!d.closed);
    assert_eq!(d.boundary_edges, 8);
    // Open input (top face removed): result reported open, not capped.
    let (p, mut t) = voxels(&block(1, 1, 1));
    t.retain(|f| !f.iter().all(|&i| p[i as usize][2] == 1.0));
    let (out, rep) = edit(
        &p,
        &t,
        &MeshEditOp::PlaneCut(PlaneCut {
            origin: [0.0, 0.0, 0.5],
            normal: [0.0, 0.0, 1.0],
            offset: 0.0,
            keep: CutKeep::Below,
            cap: true,
        }),
    )
    .unwrap();
    assert!(!diagnose(&out[0].0, &out[0].1).closed);
    assert_eq!(rep.capped_loops, 0);
    assert!(rep.notes.iter().any(|n| n.contains("not closed")));
}

fn decimate(
    m: &Mesh,
    target: u64,
    tol: f64,
) -> Result<(Mesh, rcad_exchange::mesh_edit::MeshEditReport), String> {
    edit(
        &m.0,
        &m.1,
        &MeshEditOp::Decimate(Decimate {
            target_triangles: target,
            max_deviation: tol,
        }),
    )
    .map(|(mut o, r)| (o.remove(0), r))
}

#[test]
fn decimation_of_flat_faces_is_exact() {
    let m = voxels(&block(4, 4, 4));
    assert_eq!(m.1.len(), 192);
    let (out, rep) = decimate(&m, 0, 1e-6).unwrap();
    let d = diagnose(&out.0, &out.1);
    assert!(d.closed);
    assert!((vol(&out) - 64.0).abs() < 1e-9);
    assert_eq!(d.bounds, Some(([0.0; 3], [4.0; 3])));
    assert!(out.1.len() < 80, "{} triangles", out.1.len());
    let dev = rep.deviation.unwrap();
    assert!(dev.max <= 1e-6);
    assert_eq!(rep.collapses * 2, 192 - out.1.len());
}

#[test]
fn decimation_respects_the_deviation_and_target() {
    let s = sphere(10.0, 64, 32);
    let n = s.1.len();
    let (out, rep) = decimate(&s, 0, 0.1).unwrap();
    let dev = rep.deviation.clone().unwrap();
    assert!(dev.max <= 0.1, "{dev:?}");
    assert!(out.1.len() < n / 2, "{} of {n}", out.1.len());
    assert!(diagnose(&out.0, &out.1).closed);
    // Independent re-measure agrees with the report.
    let again = measure_deviation(&s.0, &s.1, &out.0, &out.1).unwrap();
    assert_eq!(again.max, dev.max);
    // A target the deviation allows is met exactly (collapses remove two each).
    let (out, _) = decimate(&s, (n - 200) as u64, 0.5).unwrap();
    assert_eq!(out.1.len(), n - 200);
    // A target below what the deviation allows stops early, with a note.
    let (out, rep) = decimate(&s, 10, 0.01).unwrap();
    assert!(out.1.len() > 10);
    assert!(rep.notes.iter().any(|x| x.contains("stopped")));
    // Refusals.
    assert!(decimate(&s, 0, 0.0).is_err());
    assert!(decimate(&s, 0, f64::NAN).is_err());
}

#[test]
fn decimation_preserves_open_boundaries() {
    // The top faces of a 6×6 block: an open, flat, subdivided plate.
    let (p, mut t) = voxels(&block(6, 6, 1));
    t.retain(|f| f.iter().all(|&i| p[i as usize][2] == 1.0));
    let before = diagnose(&p, &t);
    let boundary = |p: &[[f64; 3]], t: &[[u32; 3]]| {
        let mut c: HashMap<(u32, u32), u32> = HashMap::new();
        for f in t {
            for k in 0..3 {
                let (a, b) = (f[k], f[(k + 1) % 3]);
                *c.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let mut pts: Vec<[i64; 3]> = c
            .iter()
            .filter(|(_, n)| **n == 1)
            .flat_map(|(e, _)| [e.0, e.1])
            .map(|i| p[i as usize].map(|x| (x * 1e6).round() as i64))
            .collect();
        pts.sort_unstable();
        pts.dedup();
        pts
    };
    let (out, _) = decimate(&(p.clone(), t.clone()), 0, 1e-6).unwrap();
    assert!(out.1.len() < t.len());
    assert_eq!(boundary(&p, &t), boundary(&out.0, &out.1));
    let after = diagnose(&out.0, &out.1);
    assert!((after.surface_area - before.surface_area).abs() < 1e-9);
}

/// A fan of 20 000 triangles around one vertex (all pairs are neighbours and
/// skipped): the visit budget still bounds the work and the result is partial.
#[test]
fn self_intersection_budget_counts_skipped_pairs() {
    let n = 20_000u32;
    let mut p = vec![[0.0, 0.0, 0.0]];
    for i in 0..=n {
        let a = 2.0 * std::f64::consts::PI * f64::from(i) / f64::from(n + 1);
        p.push([a.cos(), a.sin(), 0.01 * (f64::from(i) % 3.0)]);
    }
    let t: Vec<[u32; 3]> = (1..=n).map(|i| [0, i, i + 1]).collect();
    let start = std::time::Instant::now();
    let r = self_intersections(
        &p,
        &t,
        &SelfIntersectionLimits {
            max_pair_tests: 100_000,
            ..Default::default()
        },
    );
    assert!(!r.complete, "{r:?}");
    assert_eq!(r.pairs_visited, 100_000);
    assert!(r.pairs_tested <= r.pairs_visited);
    assert!(start.elapsed().as_secs() < 10, "{:?}", start.elapsed());
}
