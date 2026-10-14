//! Validator tests. Expected values are analytic.
use super::*;
use crate::{VolumeResult, cut_prism, extrude, join_prism, measure};

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Profile {
    Profile::Polygon(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
}

fn volume(b: &Body) -> f64 {
    match measure(b).unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        VolumeResult::Unavailable { reason } => panic!("volume unavailable: {reason}"),
    }
}

/// Frame looking down -Z from z = `z` (the plate's top, cutting downward).
fn down(z: f64) -> Frame {
    Frame {
        origin: [0.0, 50.0, z],
        x: [1.0, 0.0, 0.0],
        y: [0.0, -1.0, 0.0],
        z: [0.0, 0.0, -1.0],
    }
}

fn plate() -> Body {
    extrude(&rect(0.0, 0.0, 80.0, 50.0), &Frame::XY, 10.0).unwrap()
}

#[test]
fn ordinary_cut_and_join_pass() {
    let p = plate();
    let hole = Profile::Circle {
        center: [20.0, 25.0],
        radius: 5.0,
    };
    let start = down(12.0);
    let out = cut_prism(&p, &hole, &start, 14.0).unwrap();
    let nominal = Prism {
        profile: &hole,
        holes: &[],
        start: &start,
        length: 14.0,
    };
    let s = verify_prism_boolean(&p, &out, PrismOp::Cut, &nominal, (0.0, 14.0)).unwrap();
    assert!(s.faces_probed >= 3, "{s:?}");
    let boss = rect(5.0, 5.0, 25.0, 15.0);
    let top = Frame::XY.offset(10.0);
    let j = join_prism(&p, &boss, &top.offset(-0.5), 5.5).unwrap();
    let nominal = Prism {
        profile: &boss,
        holes: &[],
        start: &top,
        length: 5.0,
    };
    let s = verify_prism_boolean(&p, &j, PrismOp::Join, &nominal, (-0.5, 5.0)).unwrap();
    assert!(s.faces_probed >= 5, "{s:?}");
}

/// Truck keeps the engulfed hole's walls.
#[test]
fn swallowed_hole_cut_is_detected() {
    let p = plate();
    let small = Profile::Circle {
        center: [20.0, 25.0],
        radius: 5.0,
    };
    let big = Profile::Circle {
        center: [20.0, 25.0],
        radius: 8.0,
    };
    let start = down(12.0);
    let holed = cut_prism(&p, &small, &start, 14.0).unwrap();
    let out = cut_prism(&holed, &big, &start, 14.0).unwrap();
    let removed = volume(&holed) - volume(&out);
    eprintln!(
        "swallowed: removed {removed}, faces {} -> {}",
        measure(&holed).unwrap().face_count,
        measure(&out).unwrap().face_count
    );
    let nominal = Prism {
        profile: &big,
        holes: &[],
        start: &start,
        length: 14.0,
    };
    let e = verify_prism_boolean(&holed, &out, PrismOp::Cut, &nominal, (0.0, 14.0));
    eprintln!("{e:?}");
    let want = std::f64::consts::PI * (64.0 - 25.0) * 10.0;
    if (removed - want).abs() > 1e-3 {
        assert!(matches!(e, Err(KernelError::InvalidResult(_))), "{e:?}");
    }
    // Workaround candidate: (T ∪ tool) \ tool = T \ tool.
    if let Ok(u) = join_prism(&holed, &big, &start, 14.0) {
        let u_ok = verify_prism_boolean(&holed, &u, PrismOp::Join, &nominal, (0.0, 14.0));
        eprintln!("union: vol {} check {u_ok:?}", volume(&u));
        if let Ok(c) = cut_prism(&u, &big, &start, 14.0) {
            let c_ok = verify_prism_boolean(&holed, &c, PrismOp::Cut, &nominal, (0.0, 14.0));
            eprintln!(
                "union-then-cut: removed {} (want {want}) check {c_ok:?}",
                volume(&holed) - volume(&c)
            );
        }
    }
}

/// Skin over a cavity: a 0.5 mm overlap under a
/// 0.3 mm skin leaks into the cavity; the check sees it and clear_depth sizes
/// the overlap so it cannot.
#[test]
fn join_overlap_into_a_cavity_is_detected_and_clear_depth_avoids_it() {
    let block = extrude(&rect(-10.0, -10.0, 10.0, 10.0), &Frame::XY, 10.0).unwrap();
    // Pocket from below, 9.7 deep: skin 0.3 under the top face.
    let up = Frame::XY.offset(-1.0);
    let skinned = cut_prism(&block, &rect(-5.0, -5.0, 5.0, 5.0), &up, 10.7).unwrap();
    assert!((volume(&skinned) - (4000.0 - 970.0)).abs() < 1e-6);
    let top = Frame::XY.offset(10.0);
    let boss = rect(-2.0, -2.0, 2.0, 2.0);
    let nominal = Prism {
        profile: &boss,
        holes: &[],
        start: &top,
        length: 5.0,
    };
    let leaky = join_prism(&skinned, &boss, &top.offset(-0.5), 5.5).unwrap();
    let e = verify_prism_boolean(&skinned, &leaky, PrismOp::Join, &nominal, (-0.5, 5.0));
    assert!(matches!(e, Err(KernelError::InvalidResult(_))), "{e:?}");
    let d = clear_depth(&skinned, &top, &boss, 0.5).unwrap();
    assert!((d - 0.3).abs() < 1e-9, "clear depth {d}");
    let good = join_prism(&skinned, &boss, &top.offset(-d / 2.0), 5.0 + d / 2.0).unwrap();
    verify_prism_boolean(&skinned, &good, PrismOp::Join, &nominal, (-d / 2.0, 5.0)).unwrap();
    assert!((volume(&good) - 3110.0).abs() < 1e-6, "{}", volume(&good));
}

#[test]
fn clear_depth_cases() {
    let block = extrude(&rect(0.0, 0.0, 40.0, 30.0), &Frame::XY, 10.0).unwrap();
    let top = Frame::XY.offset(10.0);
    // Whole face, boundary on the walls: the walls do not count.
    let all = rect(0.0, 0.0, 40.0, 30.0);
    assert_eq!(clear_depth(&block, &top, &all, 0.5).unwrap(), 0.5);
    assert!((clear_depth(&block, &top, &all, 50.0).unwrap() - 10.0).abs() < 1e-9);
    // A Ø6 through hole under part of the profile.
    let holed = cut_prism(
        &block,
        &Profile::Circle {
            center: [20.0, 15.0],
            radius: 3.0,
        },
        &Frame::XY.offset(-1.0),
        12.0,
    )
    .unwrap();
    // The hole's wall reaches the face itself: depth ~0 (after the chord margin).
    let over = rect(15.0, 10.0, 25.0, 20.0);
    assert!(clear_depth(&holed, &top, &over, 0.5).unwrap() < 0.05);
    // Beside the hole: the full depth.
    let beside = rect(30.0, 5.0, 38.0, 25.0);
    assert!((clear_depth(&holed, &top, &beside, 50.0).unwrap() - 10.0).abs() < 1e-9);
    // Cylinder stacked on a cylinder of the same radius: its own wall is ignored.
    let disc = Profile::Circle {
        center: [0.0, 0.0],
        radius: 10.0,
    };
    let cyl = extrude(&disc, &Frame::XY, 5.0).unwrap();
    let d = clear_depth(&cyl, &Frame::XY.offset(5.0), &disc, 0.5).unwrap();
    assert_eq!(d, 0.5);
}

/// Feasibility spike: a 90° countersink as a revolved annular cone cut
/// from a plate with a Ø5.5 hole, and from a plain plate. Observed with Truck
/// 0.6: `and` returns None at every tolerance (holed plate) and panics (plain
/// plate), so the Hole feature offers no countersink. Prints, asserts nothing.
#[test]
#[ignore = "feasibility spike: countersink via a revolved cone tool"]
fn spike_countersink_cone() {
    use std::f64::consts::PI;
    let p = plate();
    let start = down(12.0);
    let r_hole = 2.75;
    let holed = cut_prism(
        &p,
        &Profile::Circle {
            center: [20.0, 25.0],
            radius: r_hole,
        },
        &start,
        14.0,
    )
    .unwrap();
    // Revolve frame: origin at the hole centre on the top face (z = 10), y = +Z (axis).
    let f = Frame {
        origin: [20.0, 25.0, 10.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [0.0, -1.0, 0.0],
    };
    let (big_r, m, r_in) = (5.5, 1.0, 1.0);
    // 90° countersink: radius shrinks 1 per unit depth.
    let tri = Profile::Polygon(vec![[r_in, m], [r_in, -(big_r - r_in)], [big_r + m, m]]);
    let tri = match crate::revolve(&tri, &f, 2.0 * PI) {
        Ok(_) => tri,
        Err(e) => {
            eprintln!("revolve (ccw?) failed: {e}");
            Profile::Polygon(vec![[r_in, m], [big_r + m, m], [r_in, -(big_r - r_in)]])
        }
    };
    let cone = crate::revolve(&tri, &f, 2.0 * PI).unwrap();
    eprintln!("cone volume {:?}", measure(&cone).map(|m| m.volume));
    let plain = crate::build::subtract_solid_for_test(&p, &cone);
    eprintln!(
        "plain plate cone cut: {:?}",
        plain
            .as_ref()
            .map(|b| volume(&p) - volume(b))
            .map_err(|e| e.to_string())
    );
    let out = crate::build::subtract_solid_for_test(&holed, &cone);
    match out {
        Ok(b) => {
            let removed = volume(&holed) - volume(&b);
            let h = big_r - r_hole;
            // Frustum between radii r_hole and big_r, height h, minus the hole cylinder part.
            let frustum = PI * h / 3.0 * (big_r * big_r + big_r * r_hole + r_hole * r_hole);
            let want = frustum - PI * r_hole * r_hole * h;
            eprintln!(
                "removed {removed} want {want} closed {:?}",
                measure(&b).map(|m| (m.shell_count, m.all_shells_closed))
            );
        }
        Err(e) => eprintln!("cut failed: {e}"),
    }
}

/// A tool the kernel never applied (result = target) left no
/// faces near it to probe; the interior probes now refuse it.
#[test]
fn unapplied_tool_is_detected() {
    let p = extrude(&rect(0.0, 0.0, 100.0, 60.0), &Frame::XY, 10.0).unwrap();
    let small = Profile::Circle {
        center: [50.0, 30.0],
        radius: 2.0,
    };
    let start = Frame::XY.offset(-1.0);
    let cut = Prism {
        profile: &small,
        holes: &[],
        start: &start,
        length: 12.0,
    };
    let e = verify_prism_boolean(&p, &p, PrismOp::Cut, &cut, (0.0, 12.0));
    assert!(
        matches!(e, Err(KernelError::InvalidResult(ref m)) if m.contains("not")),
        "{e:?}"
    );
    let top = Frame::XY.offset(10.0);
    let boss = rect(40.0, 20.0, 44.0, 24.0);
    let join = Prism {
        profile: &boss,
        holes: &[],
        start: &top,
        length: 5.0,
    };
    let e = verify_prism_boolean(&p, &p, PrismOp::Join, &join, (-0.5, 5.0));
    assert!(matches!(e, Err(KernelError::InvalidResult(_))), "{e:?}");
}

#[test]
#[ignore = "diagnostic: tessellation determinism"]
fn zz_tess_determinism() {
    let p = plate();
    let hole = Profile::Circle {
        center: [20.0, 25.0],
        radius: 3.15,
    };
    let start = down(12.0);
    let b = cut_prism(&p, &hole, &start, 14.0).unwrap();
    let pos = |x: &Body| crate::tessellate(x, 0.01).unwrap().positions;
    let a1 = pos(&b);
    let a2 = pos(&b);
    let c = b.deep_copy().unwrap();
    let a3 = pos(&c);
    let worst = |x: &Vec<[f64; 3]>, y: &Vec<[f64; 3]>| {
        x.iter()
            .map(|p| {
                y.iter()
                    .map(|q| {
                        ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2))
                            .sqrt()
                    })
                    .fold(f64::INFINITY, f64::min)
            })
            .fold(0.0, f64::max)
    };
    eprintln!(
        "same body twice: identical {}  copy: identical {} worst {:e}",
        a1 == a2,
        a1 == a3,
        worst(&a1, &a3)
    );
    let b2 = cut_prism(&p, &hole, &start, 14.0).unwrap();
    let a4 = pos(&b2);
    eprintln!(
        "boolean redone: identical {} worst {:e} nverts {} vs {}",
        a1 == a4,
        worst(&a1, &a4),
        a1.len(),
        a4.len()
    );
}

#[test]
#[ignore = "diagnostic: B-rep vertex determinism"]
fn zz_brep_vertex_determinism() {
    let p = plate();
    let hole = Profile::Circle {
        center: [20.0, 25.0],
        radius: 3.15,
    };
    let start = down(12.0);
    let verts = |b: &Body| {
        let mut v: Vec<[f64; 3]> = b
            .solid
            .compress()
            .boundaries
            .iter()
            .flat_map(|s| s.vertices.iter().map(|p| [p.x, p.y, p.z]))
            .collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v
    };
    let curves = |b: &Body| {
        let mut k: Vec<String> = b
            .solid
            .compress()
            .boundaries
            .iter()
            .flat_map(|s| {
                s.edges
                    .iter()
                    .map(|e| format!("{:?}", std::mem::discriminant(&e.curve)))
            })
            .collect();
        k.sort();
        k.dedup();
        k
    };
    let (b1, b2) = (
        cut_prism(&p, &hole, &start, 14.0).unwrap(),
        cut_prism(&p, &hole, &start, 14.0).unwrap(),
    );
    let (v1, v2) = (verts(&b1), verts(&b2));
    for (a, b) in v1.iter().zip(&v2) {
        let d = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        eprintln!("{a:?} d={d:e}");
    }
    eprintln!("curve kinds {:?}", curves(&b1));
}

#[test]
#[ignore = "diagnostic: B-rep edge determinism"]
fn zz_brep_edge_determinism() {
    let p = plate();
    let hole = Profile::Circle {
        center: [20.0, 25.0],
        radius: 3.15,
    };
    let start = down(12.0);
    let desc = |b: &Body| {
        let cs = b.solid.compress();
        let s = &cs.boundaries[0];
        let mut edges: Vec<String> = s
            .edges
            .iter()
            .map(|e| {
                let (a, b) = (s.vertices[e.vertices.0], s.vertices[e.vertices.1]);
                format!(
                    "[{:.6},{:.6},{:.6}]->[{:.6},{:.6},{:.6}] {:?}",
                    a.x, a.y, a.z, b.x, b.y, b.z, e.curve
                )
            })
            .collect();
        edges.sort();
        let faces: Vec<String> = s
            .faces
            .iter()
            .map(|f| {
                format!(
                    "{:?} {:?}",
                    f.orientation,
                    f.boundaries.iter().map(|w| w.len()).collect::<Vec<_>>()
                )
            })
            .collect();
        (edges, faces)
    };
    let (d1, d2) = (
        desc(&cut_prism(&p, &hole, &start, 14.0).unwrap()),
        desc(&cut_prism(&p, &hole, &start, 14.0).unwrap()),
    );
    eprintln!("edges equal {} faces equal {}", d1.0 == d2.0, d1.1 == d2.1);
    for (a, b) in d1.0.iter().zip(&d2.0) {
        if a != b {
            eprintln!("A {}\nB {}", &a[..a.len().min(300)], &b[..b.len().min(300)]);
            break;
        }
    }
    eprintln!("faces A {:?}\nfaces B {:?}", d1.1, d2.1);
}

/// A 7 x 7 grid of the outer loop's bounding box with a 1 % margin gives a
/// ring (Ø40 minus Ø38) or a 0.5 mm slot no sample, so an unapplied tool
/// would pass with no evidence. The samples come from the actual region
/// (grid plus chord midpoints along inward normals at boundary edges).
#[test]
fn ring_and_thin_slot_tools_are_sampled() {
    let block = extrude(&rect(0.0, 0.0, 60.0, 60.0), &Frame::XY, 10.0).unwrap();
    let start = Frame::XY.offset(6.0);
    let ring_outer = Profile::Circle {
        center: [30.0, 30.0],
        radius: 20.0,
    };
    let ring_hole = [Profile::Circle {
        center: [30.0, 30.0],
        radius: 19.0,
    }];
    let slot = rect(10.0, 29.75, 50.0, 30.25);
    for (outer, holes) in [(&ring_outer, &ring_hole[..]), (&slot, &[][..])] {
        let prism = Prism {
            profile: outer,
            holes,
            start: &start,
            length: 4.0,
        };
        // Applied exactly (slab construction): passes.
        let cut = crate::prismatic::prism_region_boolean(
            &block,
            outer,
            holes,
            &start,
            4.0,
            crate::prismatic::SlabOp::Cut,
        )
        .unwrap();
        verify_prism_boolean(&block, &cut, PrismOp::Cut, &prism, (0.0, 4.0)).unwrap();
        // Not applied (the target returned unchanged): refused.
        let e = verify_prism_boolean(&block, &block, PrismOp::Cut, &prism, (0.0, 4.0))
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("was not applied") || e.contains("does not bound"),
            "{e}"
        );
    }
}

/// The ray-crossing fast path of `Winding` agrees with the exact
/// solid-angle sum wherever the latter is decisive (a plate with eight
/// through holes, ~10^4 triangles; 4000 deterministic pseudo-random probes,
/// including points on hole axes and face planes).
#[test]
fn ray_crossing_winding_matches_the_exact_sum() {
    let p = crate::Profile::Polygon(vec![[0.0, 0.0], [80.0, 0.0], [80.0, 50.0], [0.0, 50.0]]);
    let mut b = crate::extrude(&p, &crate::Frame::XY, 10.0).unwrap();
    for i in 0..8 {
        let c = crate::Profile::Circle {
            center: [8.0 + i as f64 * 9.0, 25.0],
            radius: 3.0,
        };
        b = crate::cut_through_all(&b, &c, &crate::Frame::XY.offset(10.0)).unwrap();
    }
    let m = crate::tessellate(&b, 0.005).unwrap();
    let w = Winding::new(&m);
    assert_eq!(w.grids.len(), 3, "watertight mesh gets a grid per axis");
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let (mut fast, mut agree) = (0, 0);
    for k in 0..4000 {
        let mut q = [
            rnd() * 100.0 - 10.0,
            rnd() * 70.0 - 10.0,
            rnd() * 30.0 - 10.0,
        ];
        // Every 10th probe on a hole axis or a face plane (degenerate rays).
        match k % 10 {
            0 => q[1] = 25.0,
            1 => q[2] = 10.0,
            _ => {}
        }
        let exact = w.brute(q);
        for g in &w.grids {
            if let Some(c) = w.crossings(g, q) {
                fast += 1;
                if (exact - 1.0).abs() < 0.1 || exact.abs() < 0.1 {
                    assert_eq!(
                        c,
                        exact.round(),
                        "probe {q:?} axis {}: crossings {c} vs exact {exact}",
                        g.axis
                    );
                    agree += 1;
                }
            }
        }
    }
    assert!(fast > 3000 && agree > 3000, "fast {fast}, compared {agree}");
    // Rays through mesh vertices and edge midpoints
    // (probes on the vertex's y/z line, 1 mm before it in x) must fall back
    // or agree.
    let (mut agreed, mut fell_back) = (0, 0);
    for t in m.triangles.iter().step_by(7) {
        let [a, b, _] = t.map(|i| m.positions[i as usize]);
        for q in [
            a,
            [
                (a[0] + b[0]) / 2.0,
                (a[1] + b[1]) / 2.0,
                (a[2] + b[2]) / 2.0,
            ],
        ] {
            let p = [q[0] - 1.0, q[1], q[2]];
            let exact = w.brute(p);
            if !((exact - 1.0).abs() < 0.1 || exact.abs() < 0.1) {
                continue; // not decisive (p on a surface)
            }
            let Some(g) = w.grids.first() else { continue };
            match w.crossings(g, p) {
                Some(c) => {
                    assert_eq!(c, exact.round(), "vertex-aimed probe {p:?}");
                    agreed += 1;
                }
                None => fell_back += 1,
            }
        }
    }
    // Rays through vertices/edges are detected and use the exact sum.
    assert!(fell_back > 0, "no grazing ray detected ({agreed} agreed)");
}
