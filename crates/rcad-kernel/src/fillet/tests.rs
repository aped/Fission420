//! Exact edge fillet (run 5). Expected removed volume (t·r − (π − θ)·r²/2)·L
//! with t = r·cot(θ/2); cylinder radius and axis from the B-rep surfaces;
//! tangency and section shape from point probes on the result's winding
//! number (a non-tangent cut, e.g. a chamfer, fails the near-tangent probes).
use super::*;
use crate::{VolumeResult, cut_prism, export_step, extrude, measure, point_membership};
use std::f64::consts::PI;

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

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Profile {
    Profile::Polygon(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
}

fn top() -> Frame {
    Frame {
        origin: [0.0, 0.0, 10.0],
        ..Frame::XY
    }
}

fn front() -> Frame {
    Frame {
        origin: [0.0, 0.0, 0.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [0.0, -1.0, 0.0],
    }
}

/// Distances of the result's curved-surface sample points from the line
/// through `o` along `dir` (every curved face, 5 × 5 parameter samples).
fn curved_distances(b: &Body, o: [f64; 3], dir: [f64; 3]) -> Vec<f64> {
    use std::ops::Bound::Included;
    let (o, d) = (
        Vector3::new(o[0], o[1], o[2]),
        Vector3::new(dir[0], dir[1], dir[2]).normalize(),
    );
    let mut out = Vec::new();
    for f in b.solid.face_iter() {
        let s = f.surface();
        if matches!(s, Surface::Plane(_)) {
            continue;
        }
        let ((Included(u0), Included(u1)), (Included(v0), Included(v1))) = s.parameter_range()
        else {
            out.push(f64::NAN);
            continue;
        };
        for i in 0..5 {
            for j in 0..5 {
                let p = s.subs(
                    u0 + (u1 - u0) * i as f64 / 4.0,
                    v0 + (v1 - v0) * j as f64 / 4.0,
                );
                let w = Vector3::new(p.x, p.y, p.z) - o;
                out.push((w - d * w.dot(d)).magnitude());
            }
        }
    }
    out
}

#[test]
fn fillet_of_a_right_angle_box_edge() {
    let b = extrude(&rect(0.0, 0.0, 40.0, 30.0), &Frame::XY, 10.0).unwrap();
    for r in [0.5, 2.0, 4.5] {
        let f = fillet_edge(&b, &top(), &front(), r).unwrap();
        let removed = volume(&b) - volume(&f);
        let want = (r * r - PI * r * r / 4.0) * 40.0;
        assert!((removed - want).abs() < 1e-6, "r {r}: {removed} vs {want}");
        let m = measure(&f).unwrap();
        assert_eq!((m.shell_count, m.all_shells_closed), (1, true));
        // Every curved face lies on the cylinder of radius r about the axis
        // (y = r, z = 10 - r) along x.
        let dist = curved_distances(&f, [0.0, r, 10.0 - r], [1.0, 0.0, 0.0]);
        assert!(!dist.is_empty());
        assert!(dist.iter().all(|x| (x - r).abs() < 1e-9), "r {r}: {dist:?}");
        // Section probes at x = 5, 20, 35: just inside / outside the arc at
        // 5 angles, and near-tangent points (0.1 r from each tangent point,
        // 0.025 r inside the face) that a 45° chamfer would remove.
        let mut pts = Vec::new();
        for x in [5.0, 20.0, 35.0] {
            for k in 1..=5 {
                let a = k as f64 / 6.0 * PI / 2.0; // angle from -z towards +y... about O
                let (cy, cz) = (r, 10.0 - r);
                let dir = [-a.sin(), a.cos()]; // from O towards the corner quadrant
                for (rr, want) in [(r - 0.05 * r, true), (r + 0.05 * r, false)] {
                    pts.push(([x, cy + rr * dir[0], cz + rr * dir[1]], want));
                }
            }
            pts.push(([x, 0.9 * r, 10.0 - 0.025 * r], true));
            pts.push(([x, 0.025 * r, 10.0 - 0.9 * r], true));
            pts.push(([x, 0.2 * r, 10.0 - 0.2 * r], false)); // the old corner
        }
        let got = point_membership(&f, &pts.iter().map(|p| p.0).collect::<Vec<_>>()).unwrap();
        for (p, g) in pts.iter().zip(got) {
            assert_eq!(g, Some(p.1), "r {r}: probe {:?}", p.0);
        }
        if r == 2.0 {
            evidence(&f, "fillet_box_r2");
        }
    }
}

#[test]
fn fillet_of_an_obtuse_edge() {
    // Prism along x of the (y, z) profile (0,0) (30,0) (30,10) (5,10): the top
    // meets the slope (5,10)-(0,0) at an interior angle of 180° - atan2(10, 5).
    let yz = Frame {
        origin: [0.0, 0.0, 0.0],
        x: [0.0, 1.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [1.0, 0.0, 0.0],
    };
    let b = extrude(
        &Profile::Polygon(vec![[0.0, 0.0], [30.0, 0.0], [30.0, 10.0], [5.0, 10.0]]),
        &yz,
        40.0,
    )
    .unwrap();
    let n = Vector3::new(0.0, -10.0, 5.0).normalize();
    let slope = Frame {
        origin: [0.0, 0.0, 0.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, n.z, -n.y],
        z: [0.0, n.y, n.z],
    };
    let theta = PI - 10f64.atan2(5.0);
    let r = 3.0;
    let f = fillet_edge(&b, &top(), &slope, r).unwrap();
    let t = r / (theta / 2.0).tan();
    let want = (t * r - (PI - theta) * r * r / 2.0) * 40.0;
    let removed = volume(&b) - volume(&f);
    assert!((removed - want).abs() < 1e-6, "{removed} vs {want}");
    // Axis: from the edge (y = 5, z = 10) along the bisector, r / sin(θ/2).
    let t1 = Vector3::new(0.0, 1.0, 0.0);
    let t2 = Vector3::new(0.0, -5.0, -10.0).normalize();
    let o = Vector3::new(0.0, 5.0, 10.0) + (t1 + t2).normalize() * (r / (theta / 2.0).sin());
    let dist = curved_distances(&f, [0.0, o.y, o.z], [1.0, 0.0, 0.0]);
    assert!(dist.iter().all(|x| (x - r).abs() < 1e-9), "{dist:?}");
    // Tangent points: on the top at y = 5 + t, on the slope at t from the edge.
    let probes = [
        ([20.0, 5.0 + 0.9 * t, 10.0 - 0.02], true),
        ([20.0, 5.0 + 0.2 * t, 10.0 - 0.05], false),
    ];
    let got = point_membership(&f, &probes.map(|p| p.0)).unwrap();
    for (p, g) in probes.iter().zip(got) {
        assert_eq!(g, Some(p.1), "probe {:?}", p.0);
    }
    evidence(&f, "fillet_obtuse_r3");
}

#[test]
fn fillet_refusals() {
    let b = extrude(&rect(0.0, 0.0, 40.0, 30.0), &Frame::XY, 10.0).unwrap();
    // Radius that does not fit (tangent point past the 10 mm front face).
    let e = fillet_edge(&b, &top(), &front(), 12.0)
        .unwrap_err()
        .to_string();
    assert!(e.contains("does not fit"), "{e}");
    // Concave edge (inner corner of an L-shaped block).
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
    // Concave (run 6): the corner region is added, (r² − πr²/4)·length.
    let filled = fillet_edge(&l, &wall, &step, 1.0).unwrap();
    let want = volume(&l) + (1.0 - std::f64::consts::PI / 4.0) * 20.0;
    assert!((volume(&filled) - want).abs() < 1e-6, "{}", volume(&filled));
    // The wall above the step is 5 mm tall: radius 6 does not fit.
    let e = fillet_edge(&l, &wall, &step, 6.0).unwrap_err().to_string();
    assert!(e.contains("does not fit"), "{e}");
    // Not prismatic along the edge: a round hole across it.
    let holed = cut_prism(
        &b,
        &Profile::Circle {
            center: [20.0, 15.0],
            radius: 3.0,
        },
        &Frame::XY.offset(-1.0),
        12.0,
    )
    .unwrap();
    let e = fillet_edge(&holed, &top(), &front(), 2.0)
        .unwrap_err()
        .to_string();
    assert!(e.contains("not prismatic"), "{e}");
    // Another feature within reach: a groove along the edge, 1 mm behind it.
    let yz = Frame {
        origin: [-1.0, 0.0, 0.0],
        x: [0.0, 1.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [1.0, 0.0, 0.0],
    };
    let grooved = cut_prism(&b, &rect(1.0, 8.5, 1.5, 11.0), &yz, 42.0).unwrap();
    let e = fillet_edge(&grooved, &top(), &front(), 2.0)
        .unwrap_err()
        .to_string();
    assert!(e.contains("within the fillet's reach"), "{e}");
}
