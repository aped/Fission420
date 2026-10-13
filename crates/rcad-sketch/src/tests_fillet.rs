//! SKETCH corner fillet: analytic areas (right-angle and oblique corners),
//! full constraint kept through the virtual corner, radius and width edits,
//! refusals. Areas: a fillet of radius r at a corner of interior angle θ
//! removes `t·r − (π − θ)·r²/2` with `t = r / tan(θ/2)` (two right triangles
//! of the kite minus the arc sector); for θ = 90° that is `r²(1 − π/4)`.
use super::*;
use std::f64::consts::PI;

/// Corner-anchored rectangle w x h (origin at (0,0) fixed), dimensions w/h.
/// Returns the sketch and its corners counter-clockwise from the origin.
fn rect(w: f64, h: f64) -> (Sketch, Vec<PointId>) {
    let mut s = Sketch::default();
    let p: Vec<PointId> = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
        .iter()
        .map(|q| s.add_point(q[0], q[1]))
        .collect();
    s.origin = Some(p[0]);
    let l: Vec<EntityId> = (0..4).map(|i| s.add_line(p[i], p[(i + 1) % 4])).collect();
    s.constraints.extend([
        Constraint::FixedPoint {
            p: p[0],
            x: 0.0,
            y: 0.0,
        },
        Constraint::Horizontal { line: l[0] },
        Constraint::Horizontal { line: l[2] },
        Constraint::Vertical { line: l[1] },
        Constraint::Vertical { line: l[3] },
        Constraint::HorizontalDistance {
            a: p[0],
            b: p[1],
            dim: Dimension::new("w", w),
        },
        Constraint::VerticalDistance {
            a: p[1],
            b: p[2],
            dim: Dimension::new("h", h),
        },
    ]);
    assert_eq!(s.solve().status, SolveStatus::FullyConstrained);
    (s, p)
}

fn area(s: &Sketch) -> f64 {
    let r = s.regions().expect("regions");
    assert_eq!(r.len(), 1, "{r:?}");
    r[0].area
}

#[test]
fn right_angle_fillets_keep_full_constraint_and_follow_edits() {
    let (mut s, p) = rect(40.0, 20.0);
    let r = s.fillet_corner(p[2], 5.0, None).unwrap();
    assert_eq!(r.dimension, "fillet");
    assert!((area(&s) - (800.0 - 25.0 * (1.0 - PI / 4.0))).abs() < 1e-9);
    assert_eq!(s.solve().status, SolveStatus::FullyConstrained);
    // All four corners (the origin corner too: its FixedPoint stays on the
    // virtual corner).
    for (i, c) in [p[0], p[1], p[3]].into_iter().enumerate() {
        let r = s.fillet_corner(c, 5.0, Some("r")).unwrap();
        let want = if i == 0 {
            "r".to_string()
        } else {
            format!("r{}", i + 1)
        };
        assert_eq!(r.dimension, want);
    }
    let want = |w: f64, h: f64, r: f64| w * h - (4.0 - PI) * r * r;
    assert!((area(&s) - want(40.0, 20.0, 5.0)).abs() < 1e-9);
    assert_eq!(s.solve().status, SolveStatus::FullyConstrained);
    // Edits: width (measured between the virtual corners) and one radius.
    s.set_dimension("w", 60.0).unwrap();
    assert!(s.solve().is_success());
    assert!((area(&s) - want(60.0, 20.0, 5.0)).abs() < 1e-6);
    s.set_dimension("fillet", 8.0).unwrap();
    assert!(s.solve().is_success());
    let one = 64.0 * (1.0 - PI / 4.0) - 25.0 * (1.0 - PI / 4.0);
    assert!((area(&s) - (want(60.0, 20.0, 5.0) - one)).abs() < 1e-6);
}

#[test]
fn oblique_corner_area() {
    // Triangle (0,0), (30,0), (0,20): fillet the corner at (30,0).
    let mut s = Sketch::default();
    let p: Vec<PointId> = [[0.0, 0.0], [30.0, 0.0], [0.0, 20.0]]
        .iter()
        .map(|q| s.add_point(q[0], q[1]))
        .collect();
    for i in 0..3 {
        s.add_line(p[i], p[(i + 1) % 3]);
    }
    let r = 3.0;
    s.fillet_corner(p[1], r, None).unwrap();
    let theta = (20.0f64 / 30.0).atan();
    let t = r / (theta / 2.0).tan();
    let removed = t * r - (PI - theta) * r * r / 2.0;
    assert!((area(&s) - (300.0 - removed)).abs() < 1e-9, "{}", area(&s));
}

#[test]
fn refusals_leave_the_sketch_unchanged() {
    let (s0, p) = rect(40.0, 20.0);
    let mut s = s0.clone();
    // Radius 25 needs 25 mm along the 20 mm side.
    let e = s.fillet_corner(p[2], 25.0, None).unwrap_err();
    assert!(e.to_string().contains("does not fit"), "{e}");
    assert!(s.fillet_corner(p[2], 0.0, None).is_err());
    assert_eq!(s, s0);
    // Two fillets on one 20 mm side: 11 + 11 > 20.
    s.fillet_corner(p[1], 11.0, None).unwrap();
    let before = s.clone();
    assert!(s.fillet_corner(p[2], 11.0, None).is_err());
    assert_eq!(s, before);
    // Not a corner of two lines: a free point, a circle centre, collinear lines.
    let mut t = Sketch::default();
    let a = t.add_point(0.0, 0.0);
    let m = t.add_point(10.0, 0.0);
    let b = t.add_point(20.0, 0.0);
    t.add_line(a, m);
    t.add_line(m, b);
    let c = t.add_point(5.0, 5.0);
    t.add_circle(c, 1.0);
    let before = t.clone();
    assert!(
        t.fillet_corner(m, 1.0, None)
            .unwrap_err()
            .to_string()
            .contains("collinear")
    );
    assert!(t.fillet_corner(c, 1.0, None).is_err());
    assert!(t.fillet_corner(a, 1.0, None).is_err());
    assert_eq!(t, before);
}
