//! Chamfer of edges whose end faces are slanted.
//! Expected volumes: the removed solid is the right triangle T (legs d along
//! each face) swept between the end planes; the axial length is linear over T,
//! so its integral is (d²/2) · length at T's centroid, with the length read
//! off the trapezoid profile here (independent of the kernel's end-plane
//! search). Probes use the result's winding number.
use super::*;
use crate::{Frame, Profile, VolumeResult, export_step, extrude, measure, point_membership};

fn volume(b: &Body) -> f64 {
    match measure(b).unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        VolumeResult::Unavailable { reason } => panic!("{reason}"),
    }
}

fn evidence(b: &Body, name: &str) {
    let text = export_step(b, "test").unwrap();
    if let Some(dir) = std::env::var_os("RCAD_EVIDENCE_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join(format!("{name}.step")),
            text,
        )
        .unwrap();
    }
}

fn top() -> Frame {
    Frame {
        origin: [0.0, 0.0, 10.0],
        ..Frame::XY
    }
}

/// The planar face of `b` with outward normal `n`, its boundary edges all
/// straight (midpoint on the chord within 1e-9 mm): (edge count, endpoints).
fn straight_face_edges(b: &Body, n: Vector3) -> (usize, Vec<[Point3; 2]>) {
    let f = b
        .solid
        .face_iter()
        .find(|f| match f.surface() {
            Surface::Plane(pl) => {
                let m = if f.orientation() {
                    pl.normal()
                } else {
                    -pl.normal()
                };
                m.dot(n) > 1.0 - 1e-9
            }
            _ => false,
        })
        .expect("chamfer face");
    let mut ends = Vec::new();
    for w in f.boundaries() {
        for e in w.edge_iter() {
            let c = e.oriented_curve();
            let (t0, t1) = c.range_tuple();
            let (a, b, m) = (c.subs(t0), c.subs(t1), c.subs(0.5 * (t0 + t1)));
            let off = ((m - a) - (b - a) * ((m - a).dot(b - a) / (b - a).magnitude2())).magnitude();
            assert!(off < 1e-9, "chamfer face edge is not straight ({off})");
            ends.push([a, b]);
        }
    }
    (ends.len(), ends)
}

#[test]
fn chamfer_with_both_end_faces_tilted_about_the_section() {
    // Trapezoid prism: XZ profile (x, z), extruded 30 along -Y (y in [-30, 0]).
    // Right end leans in by 25°, left end leans out by 15°: along z the edge
    // region spans x in [z tan(-15°), 40 - z tan 25°].
    let (ta, tb) = (25f64.to_radians().tan(), (-15f64).to_radians().tan());
    let prof = Profile::Polygon(vec![
        [0.0, 0.0],
        [40.0, 0.0],
        [40.0 - 10.0 * ta, 10.0],
        [10.0 * tb, 10.0],
    ]);
    let b = extrude(&prof, &Frame::XZ, 30.0).unwrap();
    let front = Frame {
        origin: [0.0, 0.0, 0.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.0, -1.0],
        z: [0.0, 1.0, 0.0],
    };
    for d in [1.0, 2.5, 4.0] {
        let c = chamfer_edge(&b, &top(), &front, d).unwrap();
        // Centroid of T at z = 10 - d/3: length 40 - (ta + tb)(10 - d/3).
        let want = 0.5 * d * d * (40.0 - (ta + tb) * (10.0 - d / 3.0));
        let removed = volume(&b) - volume(&c);
        assert!((removed - want).abs() < 1e-6, "d {d}: {removed} vs {want}");
        let m = measure(&c).unwrap();
        assert_eq!((m.shell_count, m.all_shells_closed), (1, true));
        // Chamfer face: normal (0, 1, 1)/√2, four straight edges; its ends lie
        // in the two end planes (x = z tb at the left, x = 40 - z ta at the right).
        let (n, ends) = straight_face_edges(&c, Vector3::new(0.0, 1.0, 1.0).normalize());
        assert_eq!(n, 4);
        let in_left = |p: Point3| (p.x - p.z * tb).abs() < 1e-7;
        let in_right = |p: Point3| (p.x - (40.0 - p.z * ta)).abs() < 1e-7;
        assert_eq!(
            ends.iter()
                .filter(|e| e.iter().all(|p| in_left(*p)))
                .count(),
            1
        );
        assert_eq!(
            ends.iter()
                .filter(|e| e.iter().all(|p| in_right(*p)))
                .count(),
            1
        );
        // Distances d on both faces, mid-edge and near the slanted end.
        let h = 0.05;
        let mut pts = Vec::new();
        for x in [20.0, 40.0 - 10.0 * ta - 0.3] {
            pts.extend([
                ([x, -(d + 0.1), 10.0 - h], true),  // top face beyond A
                ([x, -(d - 0.1), 10.0 - h], false), // top face before A
                ([x, -h, 10.0 - (d + 0.1)], true),  // front face beyond B
                ([x, -h, 10.0 - (d - 0.1)], false),
            ]);
        }
        let got = point_membership(&c, &pts.iter().map(|p| p.0).collect::<Vec<_>>()).unwrap();
        for (p, g) in pts.iter().zip(got) {
            assert_eq!(g, Some(p.1), "d {d}: probe {:?}", p.0);
        }
        if d == 2.5 {
            evidence(&c, "chamfer_slanted_xz_d2.5");
        }
    }
}

#[test]
fn chamfer_with_an_end_face_tilted_in_plan() {
    // Trapezoid in plan (end at x = 40 - y tan 20°), 10 thick; edge top/front.
    let t = 20f64.to_radians().tan();
    let prof = Profile::Polygon(vec![
        [0.0, 0.0],
        [40.0, 0.0],
        [40.0 - 30.0 * t, 30.0],
        [0.0, 30.0],
    ]);
    let b = extrude(&prof, &Frame::XY, 10.0).unwrap();
    let front = Frame {
        origin: [0.0, 0.0, 0.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [0.0, -1.0, 0.0],
    };
    let d = 3.0;
    let c = chamfer_edge(&b, &top(), &front, d).unwrap();
    // Centroid of T at y = d/3.
    let want = 0.5 * d * d * (40.0 - t * d / 3.0);
    let removed = volume(&b) - volume(&c);
    assert!((removed - want).abs() < 1e-6, "{removed} vs {want}");
    let (n, _) = straight_face_edges(&c, Vector3::new(0.0, -1.0, 1.0).normalize());
    assert_eq!(n, 4);
    evidence(&c, "chamfer_slanted_plan_d3");
}

#[test]
fn chamfer_refusals_at_the_ends() {
    let front = Frame {
        origin: [0.0, 0.0, 0.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [0.0, -1.0, 0.0],
    };
    // Edge ending at a cylindrical face (a D-shaped plate: semicircular end).
    let segs = vec![
        crate::LoopSegment::Line { start: [0.0, 0.0] },
        crate::LoopSegment::Arc {
            start: [30.0, 0.0],
            center: [30.0, 15.0],
            ccw: true,
        },
        crate::LoopSegment::Line {
            start: [30.0, 30.0],
        },
        crate::LoopSegment::Line { start: [0.0, 30.0] },
    ];
    let d_plate = extrude(&Profile::Loop(segs), &Frame::XY, 10.0).unwrap();
    let e = chamfer_edge(&d_plate, &top(), &front, 2.0)
        .unwrap_err()
        .to_string();
    assert!(e.contains("curved face"), "{e}");
    // Concave edge: the inner corner of an L-shaped block (faces x = 10,
    // normal +X, and z = 5, normal +Z, meeting along y).
    let l = extrude(
        &Profile::Polygon(vec![
            [0.0, 0.0],
            [30.0, 0.0],
            [30.0, 5.0],
            [10.0, 5.0],
            [10.0, 10.0],
            [0.0, 10.0],
        ]),
        &Frame::XZ,
        20.0,
    )
    .unwrap();
    let wall = Frame {
        origin: [10.0, 0.0, 0.0],
        x: [0.0, 1.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [1.0, 0.0, 0.0],
    };
    let step = Frame {
        origin: [0.0, 0.0, 5.0],
        ..Frame::XY
    };
    let e = chamfer_edge(&l, &wall, &step, 1.0).unwrap_err().to_string();
    assert!(e.contains("concave"), "{e}");
    // Observed (Truck 0.6): a 1 mm edge between ends leaning out by 45° gives
    // a result 3.5e-4 mm³ off the exact wedge (4.666667 mm³); the removed-volume
    // prediction refuses it rather than accept the inexact solid.
    let prof = Profile::Polygon(vec![[0.0, 0.0], [21.0, 0.0], [11.0, 10.0], [10.0, 10.0]]);
    let b = extrude(&prof, &Frame::XZ, 30.0).unwrap();
    let front_y0 = Frame {
        origin: [0.0, 0.0, 0.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.0, -1.0],
        z: [0.0, 1.0, 0.0],
    };
    match chamfer_edge(&b, &top(), &front_y0, 2.0) {
        Ok(c) => {
            let want = 2.0 * (1.0 + 2.0 * 2.0 / 3.0);
            assert!((volume(&b) - volume(&c) - want).abs() < 1e-6);
        }
        Err(e) => assert!(e.to_string().contains("instead of the edge wedge"), "{e}"),
    }
}
