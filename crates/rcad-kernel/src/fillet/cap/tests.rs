//! Cap-loop fillet. Expected volumes are derived by hand per case: the
//! removed section is the spandrel of area A = r²(1 − π/4) with first moment
//! M = r³(5/6 − π/4) about either wall (∫ over the square [0,r]² of x is
//! r³/2; the quarter disc's is (πr²/4)(r − 4r/(3π)) = πr³/4 − r³/3). Along a
//! straight edge of length L it removes A·L; a convex 90° miter corner shortens
//! each of its two edges' sections at wall distance d by d, removing M less
//! per edge (a concave one adds M); an arc of radius R and sweep φ removes
//! φ·(A·R − M) (Pappus, centroid at radius R − M/A). Shape checks use point
//! probes just inside / outside the analytic fillet surfaces.
use super::*;
use crate::{
    LoopSegment, Profile, VolumeResult, check_mesh, export_step, extrude, extrude_region, measure,
    point_membership, tessellate,
};

fn spandrel(r: f64) -> (f64, f64) {
    (r * r * (1.0 - PI / 4.0), r.powi(3) * (5.0 / 6.0 - PI / 4.0))
}

fn top(h: f64) -> Frame {
    Frame {
        origin: [0.0, 0.0, h],
        ..Frame::XY
    }
}

fn volume(b: &Body) -> (f64, f64) {
    match measure(b).unwrap().volume {
        VolumeResult::Value {
            volume,
            error_estimate,
            ..
        } => (volume, error_estimate),
        VolumeResult::Unavailable { reason } => panic!("{reason}"),
    }
}

fn evidence(b: &Body, name: &str) {
    if let Some(dir) = std::env::var_os("RCAD_EVIDENCE_DIR") {
        let text = export_step(b, "test").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join(format!("{name}.step")),
            text,
        )
        .unwrap();
    }
}

/// Rounded rectangle [0,a]×[0,b] with corner radius `rr`, counter-clockwise.
fn rounded(a: f64, b: f64, rr: f64) -> Profile {
    let arc = |s: [f64; 2], c: [f64; 2]| LoopSegment::Arc {
        start: s,
        center: c,
        ccw: true,
    };
    let line = |s: [f64; 2]| LoopSegment::Line { start: s };
    Profile::Loop(vec![
        line([rr, 0.0]),
        arc([a - rr, 0.0], [a - rr, rr]),
        line([a, rr]),
        arc([a, b - rr], [a - rr, b - rr]),
        line([a - rr, b]),
        arc([rr, b], [rr, b - rr]),
        line([0.0, b - rr]),
        arc([0.0, rr], [rr, rr]),
    ])
}

fn rect(a: f64, b: f64) -> Profile {
    Profile::Polygon(vec![[0.0, 0.0], [a, 0.0], [a, b], [0.0, b]])
}

fn check(out: &Body, input: &Body, want: f64, faces: usize) {
    let m = measure(out).unwrap();
    assert!(m.all_shells_closed && m.shell_count == 1);
    assert_eq!(m.face_count, faces);
    let (v, err) = volume(out);
    assert!(
        (v - want).abs() <= 1e-5 * want + 3.0 * err,
        "volume {v} (±{err}) vs closed form {want}"
    );
    // The fillet only removes material inside the walls: same bounding box.
    let (bi, bo) = (measure(input).unwrap().bounds, m.bounds);
    for k in 0..3 {
        assert!((bi.min[k] - bo.min[k]).abs() < 1e-9 && (bi.max[k] - bo.max[k]).abs() < 1e-9);
    }
}

/// Probe pairs across the fillet surface at `p` with outward unit normal `n`:
/// outside at +eps, inside at −eps.
fn probe(b: &Body, p: [f64; 3], n: [f64; 3]) {
    let eps = 0.02;
    let at = |s: f64| [p[0] + s * n[0], p[1] + s * n[1], p[2] + s * n[2]];
    let got = point_membership(b, &[at(eps), at(-eps)]).unwrap();
    assert_eq!(got, vec![Some(false), Some(true)], "probe at {p:?}");
}

#[test]
fn sharp_box_gets_cylinders_and_mitered_corners() {
    let (a, b, h, r) = (40.0, 30.0, 10.0, 2.0);
    let input = extrude(&rect(a, b), &Frame::XY, h).unwrap();
    let out = fillet_cap_loop(&input, &top(h), r).unwrap();
    let (sa, m) = spandrel(r);
    let want = a * b * h - sa * 2.0 * (a + b) + 8.0 * m;
    // 4 walls + 4 cylinders + top + bottom.
    check(&out, &input, want, 10);
    let s = std::f64::consts::FRAC_1_SQRT_2;
    // Mid front edge: section circle centre (20, r, h − r).
    probe(&out, [20.0, r - r * s, h - r + r * s], [0.0, -s, s]);
    // Miter at the (0,0) corner, 45° up the ellipse in the bisector plane:
    // on both cylinders, e.g. the front one at x = y.
    let y = r - r * s;
    probe(&out, [y, y, h - r + r * s], [-0.5, -0.5, s]);
    // The vertical corner edge stays sharp below h − r.
    assert_eq!(
        point_membership(&out, &[[0.05, 0.05, h - r - 0.1], [0.05, 0.05, h - 0.05]]).unwrap(),
        vec![Some(true), Some(false)]
    );
    evidence(&out, "cap_fillet_sharp_box");
}

#[test]
fn rounded_rectangle_gets_tori() {
    let (a, b, h, rr, r) = (40.0, 30.0, 12.0, 5.0, 2.0);
    let input = extrude(&rounded(a, b, rr), &Frame::XY, h).unwrap();
    let out = fillet_cap_loop(&input, &top(h), r).unwrap();
    let (sa, m) = spandrel(r);
    let area = a * b - (4.0 - PI) * rr * rr;
    let straight = 2.0 * (a - 2.0 * rr) + 2.0 * (b - 2.0 * rr);
    let want = area * h - sa * straight - 2.0 * PI * (sa * rr - m);
    check(&out, &input, want, 18);
    // Torus at the (0,0) corner, 45° around the arc and 45° up the section:
    // tube centre at radius rr − r from (rr, rr), height h − r.
    let (s, k) = (
        std::f64::consts::FRAC_1_SQRT_2,
        (rr - r + r * std::f64::consts::FRAC_1_SQRT_2),
    );
    let p = [rr - k * s, rr - k * s, h - r + r * s];
    probe(&out, p, [-0.5, -0.5, s]);
    evidence(&out, "cap_fillet_rounded_rect");
}

#[test]
fn equal_radii_give_sphere_corners() {
    let (a, b, h, r) = (30.0, 20.0, 8.0, 3.0);
    let input = extrude(&rounded(a, b, r), &Frame::XY, h).unwrap();
    let out = fillet_cap_loop(&input, &top(h), r).unwrap();
    let (sa, m) = spandrel(r);
    let area = a * b - (4.0 - PI) * r * r;
    let straight = 2.0 * (a - 2.0 * r) + 2.0 * (b - 2.0 * r);
    let want = area * h - sa * straight - 2.0 * PI * (sa * r - m);
    check(&out, &input, want, 18);
    // Sphere about (r, r, h − r): probe along (−1, −1, 1)/√3.
    let d = 1.0 / 3f64.sqrt();
    let c = [r, r, h - r];
    probe(
        &out,
        [c[0] - r * d, c[1] - r * d, c[2] + r * d],
        [-d, -d, d],
    );
    // Every point of the corner surface is on that sphere: probe others.
    for (u, v) in [(0.2f64, 0.3f64), (1.2, 0.9), (0.7, 1.3)] {
        // Spherical direction with azimuth in (π, 3π/2), elevation v.
        let az = PI + u;
        let n = [v.cos() * az.cos(), v.cos() * az.sin(), v.sin()];
        probe(&out, [c[0] + r * n[0], c[1] + r * n[1], c[2] + r * n[2]], n);
    }
    evidence(&out, "cap_fillet_sphere_corners");
}

#[test]
fn concave_corner_of_an_l_plate() {
    let (h, r) = (6.0, 1.5);
    let poly = Profile::Polygon(vec![
        [0.0, 0.0],
        [40.0, 0.0],
        [40.0, 15.0],
        [15.0, 15.0],
        [15.0, 30.0],
        [0.0, 30.0],
    ]);
    let input = extrude(&poly, &Frame::XY, h).unwrap();
    let out = fillet_cap_loop(&input, &top(h), r).unwrap();
    let (sa, m) = spandrel(r);
    // Perimeter 140; five convex corners (−2M each), one concave (+2M).
    let want = 825.0 * h - sa * 140.0 + 10.0 * m - 2.0 * m;
    check(&out, &input, want, 14);
    evidence(&out, "cap_fillet_l_plate");
}

#[test]
fn holes_away_from_the_rim_are_kept() {
    let (a, b, h, r) = (40.0, 30.0, 10.0, 2.0);
    let hole = Profile::Circle {
        center: [20.0, 15.0],
        radius: 5.0,
    };
    let input = extrude_region(&rounded(a, b, 4.0), &[hole], &Frame::XY, h).unwrap();
    let out = fillet_cap_loop(&input, &top(h), r).unwrap();
    let (sa, m) = spandrel(r);
    let area = a * b - (4.0 - PI) * 16.0 - PI * 25.0;
    let straight = 2.0 * (a - 8.0) + 2.0 * (b - 8.0);
    let want = area * h - sa * straight - 2.0 * PI * (sa * 4.0 - m);
    // Truck splits the extruded circle into 120° walls; each is rebuilt as
    // two arcs of at most a quarter turn.
    let holes = measure(&input).unwrap().face_count - 10;
    check(&out, &input, want, 18 + 2 * holes);
    // The hole wall still runs the full height.
    assert_eq!(
        point_membership(&out, &[[20.0, 15.0, h - 0.1], [20.0, 9.9, h - 0.1]]).unwrap(),
        vec![Some(false), Some(true)]
    );
}

#[test]
fn vertical_edge_fillets_then_cap_loop() {
    // The common print workflow: round the four vertical edges with the
    // edge fillet (booleans), then the top loop with an equal or smaller radius.
    let (a, b, h, rr) = (40.0, 30.0, 10.0, 4.0);
    let mut body = extrude(&rect(a, b), &Frame::XY, h).unwrap();
    let side = |o: [f64; 3], n: [f64; 3]| Frame {
        origin: o,
        x: [n[1], -n[0], 0.0],
        y: [0.0, 0.0, 1.0],
        z: n,
    };
    let sides = [
        side([0.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
        side([a, 0.0, 0.0], [1.0, 0.0, 0.0]),
        side([a, b, 0.0], [0.0, 1.0, 0.0]),
        side([0.0, b, 0.0], [-1.0, 0.0, 0.0]),
    ];
    for i in 0..4 {
        body = crate::fillet_edge(&body, &sides[i], &sides[(i + 1) % 4], rr).unwrap();
    }
    for r in [rr, 1.5] {
        let out = fillet_cap_loop(&body, &top(h), r).unwrap();
        let (sa, m) = spandrel(r);
        let area = a * b - (4.0 - PI) * rr * rr;
        let straight = 2.0 * (a - 2.0 * rr) + 2.0 * (b - 2.0 * rr);
        let want = area * h - sa * straight - 2.0 * PI * (sa * rr - m);
        check(&out, &body, want, 18);
        evidence(&out, &format!("cap_fillet_after_edge_fillets_r{r}"));
    }
}

#[test]
fn mesh_volume_converges_to_the_closed_form() {
    let (a, b, h, r) = (30.0, 20.0, 8.0, 3.0);
    let input = extrude(&rounded(a, b, 5.0), &Frame::XY, h).unwrap();
    let out = fillet_cap_loop(&input, &top(h), r).unwrap();
    let (sa, m) = spandrel(r);
    let area = a * b - (4.0 - PI) * 25.0;
    let want = area * h - sa * (2.0 * (a - 10.0) + 2.0 * (b - 10.0)) - 2.0 * PI * (sa * 5.0 - m);
    let mut errs = Vec::new();
    for chord in [0.1, 0.01, 0.001] {
        let c = check_mesh(&tessellate(&out, chord).unwrap());
        assert!(c.watertight, "chord {chord}: not watertight");
        errs.push((c.mesh_volume - want).abs());
    }
    assert!(errs[1] < errs[0] && errs[2] < errs[1], "{errs:?}");
    assert!(errs[2] < 1e-4 * want, "{errs:?}");
}

#[test]
fn refusals_name_the_reason() {
    let h = 10.0;
    let rr = extrude(&rounded(40.0, 30.0, 2.0), &Frame::XY, h).unwrap();
    let e = fillet_cap_loop(&rr, &top(h), 3.0).unwrap_err().to_string();
    assert!(e.contains("larger than the convex arc radius"), "{e}");
    // Arc meeting a line at a sharp corner (a D shape: half disc + chord).
    let d = Profile::Loop(vec![
        LoopSegment::Line {
            start: [-10.0, 0.0],
        },
        LoopSegment::Arc {
            start: [10.0, 0.0],
            center: [0.0, 0.0],
            ccw: true,
        },
    ]);
    let body = extrude(&d, &Frame::XY, h).unwrap();
    let e = fillet_cap_loop(&body, &top(h), 1.0)
        .unwrap_err()
        .to_string();
    assert!(e.contains("sharp corner"), "{e}");
    // A hole too close to the rim.
    let hole = Profile::Circle {
        center: [5.0, 15.0],
        radius: 3.0,
    };
    let body = extrude_region(&rect(40.0, 30.0), &[hole], &Frame::XY, h).unwrap();
    let e = fillet_cap_loop(&body, &top(h), 2.5)
        .unwrap_err()
        .to_string();
    assert!(e.contains("hole"), "{e}");
    // Narrow slot: walls 3 mm apart cannot take r = 2.
    let body = extrude(&rect(40.0, 3.0), &Frame::XY, h).unwrap();
    assert!(fillet_cap_loop(&body, &top(h), 2.0).is_err());
    // Filleting the same loop again: the body is no longer a prism.
    let box_ = extrude(&rect(40.0, 30.0), &Frame::XY, h).unwrap();
    let once = fillet_cap_loop(&box_, &top(h), 2.0).unwrap();
    let e = fillet_cap_loop(&once, &top(h), 1.0)
        .unwrap_err()
        .to_string();
    assert!(e.contains("not a straight prism"), "{e}");
    // Not the cap: a side face of the box (the body is not a prism along x).
    let side = Frame {
        origin: [40.0, 0.0, 0.0],
        x: [0.0, 1.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [1.0, 0.0, 0.0],
    };
    assert!(
        fillet_cap_loop(&box_, &side, 2.0).is_ok(),
        "a box is a prism along x too"
    );
    // Radius deeper than the body.
    let e = fillet_cap_loop(&box_, &top(h), 10.0)
        .unwrap_err()
        .to_string();
    assert!(e.contains("deep"), "{e}");
}
