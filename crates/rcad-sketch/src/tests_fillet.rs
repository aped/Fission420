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

// ---- Run 5: line–arc corners ----

/// ½∮(x dy − y dx) over the arc of centre `c`, radius `r`, from angle `a0`
/// sweeping `d` (signed) radians: exact Green's-theorem term.
fn arc_term(c: [f64; 2], r: f64, a0: f64, d: f64) -> f64 {
    let a1 = a0 + d;
    0.5 * (r * r * d + r * (c[0] * (a1.sin() - a0.sin()) - c[1] * (a1.cos() - a0.cos())))
}

fn seg_term(p: [f64; 2], q: [f64; 2]) -> f64 {
    0.5 * (p[0] * q[1] - q[0] * p[1])
}

/// D-like profile: top line A(0,0)–P(20,0), quarter arc (centre (30,0), r 10)
/// from P down to E(30,−10), bottom line E–B(0,−10), left line B–A. Area
/// 300 − 25π (the quarter disk near (30, 0) is outside). Returns (sketch, P,
/// line A–P, arc).
fn d_shape() -> (Sketch, PointId, EntityId, EntityId) {
    let mut s = Sketch::default();
    let a = s.add_point(0.0, 0.0);
    let p = s.add_point(20.0, 0.0);
    let e = s.add_point(30.0, -10.0);
    let b = s.add_point(0.0, -10.0);
    let o = s.add_point(30.0, 0.0);
    s.origin = Some(a);
    let top = s.add_line(a, p);
    let arc = s.next_entity_id();
    s.entities.push(Entity::Arc {
        id: arc,
        center: o,
        start: p,
        end: e,
    });
    s.add_line(e, b);
    s.add_line(b, a);
    s.constraints.extend([
        Constraint::FixedPoint {
            p: a,
            x: 0.0,
            y: 0.0,
        },
        Constraint::FixedPoint {
            p: o,
            x: 30.0,
            y: 0.0,
        },
        Constraint::Horizontal { line: top },
        Constraint::Radius {
            entity: arc,
            dim: Dimension::new("R", 10.0),
        },
    ]);
    assert!(s.solve().is_success());
    (s, p, top, arc)
}

#[test]
fn line_arc_fillet_external_is_tangent_with_the_analytic_area() {
    let (mut s, p, _top, arc) = d_shape();
    let a0 = area(&s);
    assert!((a0 - (300.0 - 25.0 * PI)).abs() < 1e-9, "{a0}");
    let dof0 = s.clone().solve().dof;
    let r = 2.0;
    let res = s.fillet_corner(p, r, None).unwrap();
    // Closed form: F = (20 − σ, −r) with |F − O| = 10 + r, σ = √(12² − 2²) − 10.
    let sigma = (144.0f64 - 4.0).sqrt() - 10.0;
    let t1 = [20.0 - sigma, 0.0];
    let f = [t1[0], -r];
    let fo = [f[0] - 30.0, f[1]];
    let fol = fo[0].hypot(fo[1]);
    assert!((fol - 12.0).abs() < 1e-12);
    let t2 = [30.0 + 10.0 * fo[0] / fol, 10.0 * fo[1] / fol];
    // The sketch holds exactly these points (within the solve tolerance).
    let near = |q: [f64; 2]| {
        s.points
            .iter()
            .any(|x| (x.x - q[0]).hypot(x.y - q[1]) < 1e-8)
    };
    assert!(near(t1) && near(t2) && near(f), "{:?}", s.points);
    // Removed corner piece: T1 → P (line), P → T2 on the arc (counter-clockwise
    // from angle π), T2 → T1 on the fillet (the short way).
    let ang = |c: [f64; 2], q: [f64; 2]| (q[1] - c[1]).atan2(q[0] - c[0]);
    let o = [30.0, 0.0];
    let d_arc = (ang(o, t2) - PI).rem_euclid(2.0 * PI);
    let (fa0, fa1) = (ang(f, t2), ang(f, t1));
    let mut d_fil = fa1 - fa0;
    if d_fil > PI {
        d_fil -= 2.0 * PI;
    }
    if d_fil < -PI {
        d_fil += 2.0 * PI;
    }
    let removed =
        (seg_term(t1, [20.0, 0.0]) + arc_term(o, 10.0, PI, d_arc) + arc_term(f, r, fa0, d_fil))
            .abs();
    assert!(removed > 0.0 && removed < r * r);
    let a1 = area(&s);
    assert!(
        (a1 - (a0 - removed)).abs() < 1e-9,
        "{a1} vs {}",
        a0 - removed
    );
    // Same freedom as before: the fillet adds as many equations as unknowns.
    let rep = s.clone().solve();
    assert!(rep.is_success());
    assert_eq!(rep.dof, dof0);
    // The radius is a named, editable dimension; the arc stays tangent.
    assert_eq!(res.dimension, "fillet");
    s.set_dimension("fillet", 3.0).unwrap();
    assert!(s.solve().is_success());
    assert!(
        s.constraints
            .iter()
            .any(|c| matches!(c, Constraint::TangentArcs { a, b } if *a == res.arc && *b == arc))
    );
}

#[test]
fn line_arc_fillet_internal_and_refusals() {
    // Quarter disk: arc (centre O = origin, r 10) from P(10,0) to (0,10), line
    // P–O along the radius: the corner at P lies inside the circle.
    let mut s = Sketch::default();
    let o = s.add_point(0.0, 0.0);
    let p = s.add_point(10.0, 0.0);
    let q = s.add_point(0.0, 10.0);
    s.origin = Some(o);
    let l1 = s.add_line(o, p);
    let arc = s.next_entity_id();
    s.entities.push(Entity::Arc {
        id: arc,
        center: o,
        start: p,
        end: q,
    });
    s.add_line(q, o);
    s.constraints.extend([
        Constraint::FixedPoint {
            p: o,
            x: 0.0,
            y: 0.0,
        },
        Constraint::Horizontal { line: l1 },
        Constraint::Radius {
            entity: arc,
            dim: Dimension::new("R", 10.0),
        },
    ]);
    assert!(s.solve().is_success());
    let a0 = area(&s);
    assert!((a0 - 25.0 * PI).abs() < 1e-9);
    // Too large: inside the arc it must be < R, and here it must also fit
    // between the radius line and the arc.
    for bad in [10.0, 12.0, 0.0, -1.0] {
        let before = s.clone();
        assert!(s.fillet_corner(p, bad, None).is_err(), "{bad}");
        assert_eq!(s, before, "refused fillet leaves the sketch unchanged");
    }
    s.fillet_corner(p, 2.0, None).unwrap();
    // |F| = 10 − 2 with F = (σ', 2): σ' = √(8² − 2²).
    let fx = (64.0f64 - 4.0).sqrt();
    assert!(
        s.points.iter().any(|x| (x.x - fx).hypot(x.y - 2.0) < 1e-8),
        "{:?}",
        s.points
    );
    assert!(area(&s) < a0);
    // Tangent continuation is not a corner.
    let (mut t, _, _, _) = d_shape();
    let tail = t.add_point(40.0, -10.0);
    let e = t
        .entities
        .iter()
        .find_map(|x| match x {
            Entity::Arc { end, .. } => Some(*end),
            _ => None,
        })
        .unwrap();
    // E(30,−10) is the arc's end; a line from E going +x is tangent there.
    t.entities
        .retain(|x| !matches!(x, Entity::Line { a, .. } if *a == e));
    t.add_line(e, tail);
    let before = t.clone();
    let err = t.fillet_corner(e, 1.0, None).unwrap_err();
    assert!(format!("{err}").contains("tangent"), "{err}");
    assert_eq!(t, before);
}

/// Review run 5: at the largest radius that fits (the fillet is a half
/// circle, double root) the fillet must run through the corner side.
#[test]
fn line_arc_fillet_half_circle_goes_through_the_corner_side() {
    // Upper half disk: arc (centre O, r 10) from (10, 0) to (−10, 0), line
    // (−10, 0)–(10, 0). Corner at P = (10, 0); r = 5 is the double root.
    let mut s = Sketch::default();
    let o = s.add_point(0.0, 0.0);
    let p = s.add_point(10.0, 0.0);
    let q = s.add_point(-10.0, 0.0);
    s.origin = Some(o);
    let arc = s.next_entity_id();
    s.entities.push(Entity::Arc {
        id: arc,
        center: o,
        start: p,
        end: q,
    });
    s.add_line(q, p);
    s.constraints.push(Constraint::FixedPoint {
        p: o,
        x: 0.0,
        y: 0.0,
    });
    assert!(s.solve().is_success());
    match s.fillet_corner(p, 5.0, None) {
        Ok(r) => {
            let g = s.arc_geometry(r.arc).expect("fillet arc");
            // Its midpoint lies towards the corner: (5, 5), not (5, −5) or (−5, 5).
            let a = g.start_angle + 0.5 * g.sweep;
            let mid = [
                g.center[0] + g.radius * a.cos(),
                g.center[1] + g.radius * a.sin(),
            ];
            assert!(mid[1] > 0.0 && mid[0] > 0.0, "{g:?} mid {mid:?}");
        }
        // Refusing the degenerate double root is also acceptable.
        Err(e) => assert!(format!("{e}").contains("does not fit"), "{e}"),
    }
}
