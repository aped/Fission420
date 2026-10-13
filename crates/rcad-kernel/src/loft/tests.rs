//! Exact lofts. Expected volumes from closed forms independent of the
//! prismatoid check inside `loft`: the section area integrated directly
//! (rectangles with linearly varying sides), the pyramid-frustum formula
//! h/3 (A1 + A2 + √(A1 A2)) where the sections are similar, and the cone
//! frustum πh/3 (r1² + r1 r2 + r2²). Bounds and probes from the analytic
//! shapes; STEP export (FreeCAD: tools/check_loft.sh).
use super::*;
use crate::{VolumeResult, export_step, point_membership};

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

fn centered_rect(w: f64, h: f64) -> Profile {
    Profile::Polygon(vec![
        [-w / 2.0, -h / 2.0],
        [w / 2.0, -h / 2.0],
        [w / 2.0, h / 2.0],
        [-w / 2.0, h / 2.0],
    ])
}

fn check_bounds(b: &Body, lo: [f64; 3], hi: [f64; 3]) {
    let m = measure(b).unwrap();
    assert_eq!((m.shell_count, m.all_shells_closed), (1, true));
    for i in 0..3 {
        assert!(
            (m.bounds.min[i] - lo[i]).abs() < 1e-6 && (m.bounds.max[i] - hi[i]).abs() < 1e-6,
            "bounds {:?}",
            m.bounds
        );
    }
}

fn probe(b: &Body, pts: &[([f64; 3], bool)]) {
    let got = point_membership(b, &pts.iter().map(|p| p.0).collect::<Vec<_>>()).unwrap();
    for (p, g) in pts.iter().zip(got) {
        assert_eq!(g, Some(p.1), "probe {:?}", p.0);
    }
}

#[test]
fn tapered_container_blank_60x40_to_70x50() {
    let b = loft(
        &centered_rect(60.0, 40.0),
        &Frame::XY,
        &centered_rect(70.0, 50.0),
        &Frame::XY.offset(30.0),
    )
    .unwrap();
    // Sides 60 + z/3 by 40 + z/3: the integral of their product over [0, 30]
    // (not a pyramid frustum: 60 x 40 and 70 x 50 are not similar).
    let want = 2400.0 * 30.0 + 100.0 / 3.0 * 450.0 + 27000.0 / 27.0;
    assert!((want - 88000.0f64).abs() < 1e-9);
    assert!((volume(&b) - want).abs() < 1e-6, "{} vs {want}", volume(&b));
    check_bounds(&b, [-35.0, -25.0, 0.0], [35.0, 25.0, 30.0]);
    assert_eq!(measure(&b).unwrap().face_count, 6);
    // The side x(z) = 30 + z/6: probes just inside / outside at three heights.
    let mut pts = Vec::new();
    for z in [1.0, 15.0, 29.0] {
        let x = 30.0 + z / 6.0;
        pts.extend([([x - 0.05, 0.0, z], true), ([x + 0.05, 0.0, z], false)]);
        let y = 20.0 + z / 6.0;
        pts.extend([([0.0, y - 0.05, z], true), ([0.0, y + 0.05, z], false)]);
    }
    probe(&b, &pts);
    evidence(&b, "loft_tray_blank");
    // Similar sections (60 x 40 -> 75 x 50): a true pyramid frustum.
    let f = loft(
        &centered_rect(60.0, 40.0),
        &Frame::XY,
        &centered_rect(75.0, 50.0),
        &Frame::XY.offset(30.0),
    )
    .unwrap();
    let (a1, a2) = (2400.0f64, 3750.0f64);
    let want = 30.0 / 3.0 * (a1 + a2 + (a1 * a2).sqrt());
    assert!((volume(&f) - want).abs() < 1e-6, "{} vs {want}", volume(&f));
}

#[test]
fn circle_frustum_and_cylinder() {
    let pi = std::f64::consts::PI;
    let c = |r: f64| Profile::Circle {
        center: [0.0, 0.0],
        radius: r,
    };
    let b = loft(&c(20.0), &Frame::XY, &c(15.0), &Frame::XY.offset(30.0)).unwrap();
    let want = pi * 30.0 / 3.0 * (400.0 + 300.0 + 225.0);
    // A cone about the loft axis: `measure`'s exact surface-of-revolution
    // integral (FreeCAD also checks the surfaces: tools/check_loft.sh).
    match measure(&b).unwrap().volume {
        VolumeResult::Value {
            volume,
            error_estimate,
            method,
        } => {
            assert_eq!(method, crate::VolumeMethod::BrepBoundaryIntegralRevolution);
            assert!(error_estimate < 1e-9 * want, "{error_estimate}");
            assert!(
                (volume - want).abs() <= 1e-9 * want,
                "{volume} vs {want} (± {error_estimate})"
            );
        }
        v => panic!("{v:?}"),
    }
    check_bounds(&b, [-20.0, -20.0, 0.0], [20.0, 20.0, 30.0]);
    let mut pts = Vec::new();
    for z in [1.0, 15.0, 29.0] {
        let r = 20.0 - z / 6.0;
        for k in 0..6 {
            let t = 0.4 + k as f64 * pi / 3.0;
            pts.push(([(r - 0.05) * t.cos(), (r - 0.05) * t.sin(), z], true));
            pts.push(([(r + 0.05) * t.cos(), (r + 0.05) * t.sin(), z], false));
        }
    }
    probe(&b, &pts);
    evidence(&b, "loft_frustum_d40_d30");
    // Equal radii: a cylinder; profile b below a (the loft goes downwards).
    let cyl = loft(&c(5.0), &Frame::XY.offset(10.0), &c(5.0), &Frame::XY).unwrap();
    assert!((volume(&cyl) - pi * 25.0 * 10.0).abs() < 1e-6);
    check_bounds(&cyl, [-5.0, -5.0, 0.0], [5.0, 5.0, 10.0]);
}

#[test]
fn correspondence_is_normalized_and_placement_is_general() {
    // b given clockwise, starting at another vertex, in a frame rotated 90°
    // in-plane, on a translated and tilted pair of parallel planes.
    let fa = Frame {
        origin: [5.0, -3.0, 2.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.6, 0.8],
        z: [0.0, -0.8, 0.6],
    };
    let fb = Frame {
        origin: [5.0 + 0.0, -3.0 - 0.8 * 12.0, 2.0 + 0.6 * 12.0],
        x: fa.y,
        y: [-1.0, 0.0, 0.0],
        z: fa.z,
    };
    // Rectangle 70 x 50 in fa coordinates is (x in [-35, 35], y in [-25, 25]);
    // in fb (x' = y, y' = -x) clockwise from another corner.
    let b_prof = Profile::Polygon(vec![
        [25.0, -35.0],
        [-25.0, -35.0],
        [-25.0, 35.0],
        [25.0, 35.0],
    ]);
    let b_prof = match b_prof {
        Profile::Polygon(mut v) => {
            v.reverse();
            Profile::Polygon(v)
        }
        p => p,
    };
    let l = loft(&centered_rect(60.0, 40.0), &fa, &b_prof, &fb).unwrap();
    // Sides 60 + z k by 40 + z k, k = 10/12, integrated over [0, 12].
    let k = 10.0 / 12.0;
    let want = 2400.0 * 12.0 + 100.0 * k * 72.0 + k * k * 576.0;
    assert!((volume(&l) - want).abs() < 1e-6, "{} vs {want}", volume(&l));
    // Same solid with the profiles swapped (loft from b down to a).
    let r = loft(&b_prof, &fb, &centered_rect(60.0, 40.0), &fa).unwrap();
    assert!((volume(&r) - want).abs() < 1e-6);
}

#[test]
fn loft_refusals() {
    let sq = centered_rect(10.0, 10.0);
    let up = Frame::XY.offset(10.0);
    let msg = |r: Result<Body>| match r {
        Ok(_) => panic!("accepted"),
        Err(e) => e.to_string(),
    };
    let tri = Profile::Polygon(vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]]);
    assert!(msg(loft(&sq, &Frame::XY, &tri, &up)).contains("different vertex counts"));
    // Square to a 45°-rotated square: no parallel corresponding edges, and
    // vertex 0 is equally close to two vertices.
    let s = 50f64.sqrt();
    let diamond = Profile::Polygon(vec![[0.0, -s], [s, 0.0], [0.0, s], [-s, 0.0]]);
    let e = msg(loft(&sq, &Frame::XY, &diamond, &up));
    assert!(e.contains("ambiguous"), "{e}");
    // A slightly rotated square: unambiguous but twisted sides.
    let (c, sn) = (0.2f64.cos(), 0.2f64.sin());
    let rot = Profile::Polygon(
        [[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]]
            .iter()
            .map(|p: &[f64; 2]| [c * p[0] - sn * p[1], sn * p[0] + c * p[1]])
            .collect(),
    );
    let e = msg(loft(&sq, &Frame::XY, &rot, &up));
    assert!(e.contains("not parallel"), "{e}");
    assert!(msg(loft(&sq, &Frame::XY, &sq, &Frame::XY)).contains("same plane"));
    let tilted = Frame {
        origin: [0.0, 0.0, 10.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.8, 0.6],
        z: [0.0, -0.6, 0.8],
    };
    assert!(msg(loft(&sq, &Frame::XY, &sq, &tilted)).contains("not parallel"));
    let circ = Profile::Circle {
        center: [0.0, 0.0],
        radius: 5.0,
    };
    assert!(msg(loft(&sq, &Frame::XY, &circ, &up)).contains("both circles"));
    let off = Profile::Circle {
        center: [1.0, 0.0],
        radius: 5.0,
    };
    assert!(msg(loft(&circ, &Frame::XY, &off, &up)).contains("coaxial"));
}
