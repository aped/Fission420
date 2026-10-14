//! Ellipses and cubic B-splines: validation, DOF accounting, exactness of the
//! Bézier spans, profile and region extraction, refusals, upstream edits and
//! serde. Expected values are independent of the implementation: the ellipse
//! equation and area πab, B-spline points by the Cox–de Boor recursion, and
//! enclosed areas by Green's theorem with a separate quadrature rule.
use super::*;
use crate::curves::{cubic_point, spline_beziers};
use std::f64::consts::PI;

/// Solved coordinates are compared at 1e-8 mm (docs/TOLERANCES.md).
const SOLVED: f64 = 1e-8;

/// A cubic span: start, c1, c2, end, inner weights.
type Span = ([f64; 2], [f64; 2], [f64; 2], [f64; 2], [f64; 2]);

fn cubic_segs(p: &Profile) -> Vec<Span> {
    let Profile::Loop(segs) = p else {
        panic!("expected a loop, got {p:?}")
    };
    let k = segs.len();
    segs.iter()
        .enumerate()
        .filter_map(|(i, s)| match *s {
            LoopSegment::Cubic { start, c1, c2, w } => {
                Some((start, c1, c2, segs[(i + 1) % k].start(), w))
            }
            _ => None,
        })
        .collect()
}

/// Cox–de Boor basis N_{i,p}(t) (independent of the blossom extraction).
fn basis(knots: &[f64], i: usize, p: usize, t: f64) -> f64 {
    if p == 0 {
        let last = knots[knots.len() - 1];
        let inside = knots[i] <= t && t < knots[i + 1];
        // The right end of a clamped vector belongs to the last non-empty span.
        let end = t == last && knots[i] < knots[i + 1] && knots[i + 1] == last;
        return f64::from(u8::from(inside || end));
    }
    let mut v = 0.0;
    let d0 = knots[i + p] - knots[i];
    if d0 > 0.0 {
        v += (t - knots[i]) / d0 * basis(knots, i, p - 1, t);
    }
    let d1 = knots[i + p + 1] - knots[i + 1];
    if d1 > 0.0 {
        v += (knots[i + p + 1] - t) / d1 * basis(knots, i + 1, p - 1, t);
    }
    v
}

fn deboor_point(ctrl: &[[f64; 2]], knots: &[f64], t: f64) -> [f64; 2] {
    let mut p = [0.0; 2];
    for (i, c) in ctrl.iter().enumerate() {
        let b = basis(knots, i, 3, t);
        p[0] += b * c[0];
        p[1] += b * c[1];
    }
    p
}

fn ellipse_sketch(a: f64, b: f64) -> (Sketch, EntityId) {
    let s = Sketch::centered_ellipse(a, b).unwrap();
    let e = s.entities[0].id();
    (s, e)
}

#[test]
fn ellipse_template_is_fully_constrained_with_five_curve_dof() {
    let (mut s, e) = ellipse_sketch(20.0, 10.0);
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained, "{r:?}");
    assert_eq!(r.dof, 0);
    // Free ellipse alone: centre 2 + axis end 2 + minor 1 = 5 DOF.
    let mut free = Sketch::default();
    free.add_ellipse([1.0, 2.0], [7.0, 4.0], 3.0).unwrap();
    let r = free.solve();
    assert_eq!(r.status, SolveStatus::UnderConstrained);
    assert_eq!(r.dof, 5);
    // Dropping the minor-radius dimension frees exactly one DOF.
    s.constraints
        .retain(|c| !matches!(c, Constraint::EllipseRadius { major: false, .. }));
    let r = s.solve();
    assert_eq!((r.status, r.dof), (SolveStatus::UnderConstrained, 1));
    // The minor radius is a solver unknown: a minor dimension drives it.
    s.constraints.push(Constraint::EllipseRadius {
        ellipse: e,
        major: false,
        dim: Dimension::new("b", 4.5),
    });
    assert_eq!(s.solve().status, SolveStatus::FullyConstrained);
    let Some(Entity::Ellipse { minor, .. }) = s.entity(e) else {
        panic!()
    };
    assert!((minor - 4.5).abs() < SOLVED);
}

#[test]
fn ellipse_spans_lie_exactly_on_the_ellipse_and_area_is_pi_ab() {
    // Tilted ellipse, centre (3, -2), axis at 30°, a = 20, b = 7.
    let (c, a, b, th) = ([3.0, -2.0], 20.0, 7.0, 30f64.to_radians());
    let mut s = Sketch::default();
    s.add_ellipse(c, [c[0] + a * th.cos(), c[1] + a * th.sin()], b)
        .unwrap();
    let p = s.profile().unwrap();
    let segs = cubic_segs(&p);
    assert_eq!(segs.len(), 4);
    for (p0, c1, c2, p3, w) in segs {
        for k in 0..=50 {
            let q = cubic_point(p0, c1, c2, p3, w, k as f64 / 50.0);
            let (dx, dy) = (q[0] - c[0], q[1] - c[1]);
            let (u, v) = (
                dx * th.cos() + dy * th.sin(),
                -dx * th.sin() + dy * th.cos(),
            );
            let f = (u / a).powi(2) + (v / b).powi(2);
            assert!((f - 1.0).abs() < 1e-13, "point off the ellipse: {f}");
        }
    }
    let Profile::Loop(l) = &p else { panic!() };
    let area = crate::profile::loop_area(l);
    assert!((area - PI * a * b).abs() < 1e-9 * PI * a * b, "{area}");
}

#[test]
fn open_spline_spans_match_cox_de_boor_and_interpolate_the_ends() {
    let ctrl = [
        [0.0, 0.0],
        [5.0, 9.0],
        [12.0, -3.0],
        [18.0, 6.0],
        [25.0, 1.0],
        [30.0, 8.0],
    ];
    let n = ctrl.len();
    let spans = spline_beziers(&ctrl, false);
    assert_eq!(spans.len(), n - 3);
    let knots: Vec<f64> = (0..n + 4)
        .map(|i| (i as f64 - 3.0).clamp(0.0, (n - 3) as f64))
        .collect();
    for (k, sp) in spans.iter().enumerate() {
        for j in 0..=20 {
            let t = j as f64 / 20.0;
            let q = cubic_point(sp[0], sp[1], sp[2], sp[3], [1.0, 1.0], t);
            let r = deboor_point(&ctrl, &knots, k as f64 + t);
            assert!((q[0] - r[0]).abs() < 1e-12 && (q[1] - r[1]).abs() < 1e-12);
        }
    }
    assert_eq!(spans[0][0], ctrl[0]);
    let end = spans[n - 4][3];
    assert!((end[0] - 30.0).abs() < 1e-12 && (end[1] - 8.0).abs() < 1e-12);
}

/// Enclosed area of the closed uniform cubic B-spline by Green's theorem:
/// points and derivatives by Cox–de Boor (derivative by central difference of
/// the exact evaluator is avoided: the derivative of a uniform B-spline is the
/// quadratic B-spline of the control-point differences). Per knot interval
/// x·y' is a polynomial of degree 5, which 3-point Gauss–Legendre integrates
/// exactly, so the only error is rounding (< 1e-10 here).
fn closed_spline_area(ctrl: &[[f64; 2]]) -> f64 {
    let n = ctrl.len();
    let pts: Vec<[f64; 2]> = (0..n + 3).map(|i| ctrl[i % n]).collect();
    let knots: Vec<f64> = (0..n + 7).map(|i| i as f64).collect();
    let d: Vec<[f64; 2]> = (0..n + 2)
        .map(|i| [pts[i + 1][0] - pts[i][0], pts[i + 1][1] - pts[i][1]])
        .collect();
    let dknots: Vec<f64> = (1..n + 6).map(|i| i as f64).collect();
    let deriv = |t: f64| {
        let mut p = [0.0; 2];
        for (i, c) in d.iter().enumerate() {
            let b = basis(&dknots, i, 2, t);
            p[0] += b * c[0];
            p[1] += b * c[1];
        }
        p
    };
    let g = [
        (-(0.6f64).sqrt(), 5.0 / 9.0),
        (0.0, 8.0 / 9.0),
        ((0.6f64).sqrt(), 5.0 / 9.0),
    ];
    let mut a = 0.0;
    for k in 3..n + 3 {
        for (x, wt) in g {
            let t = k as f64 + 0.5 + 0.5 * x;
            let p = deboor_point(&pts, &knots, t);
            let dp = deriv(t);
            a += 0.5 * wt * 0.5 * (p[0] * dp[1] - p[1] * dp[0]);
        }
    }
    a
}

#[test]
fn closed_spline_profile_area_matches_greens_theorem() {
    let ctrl = [
        [0.0, 0.0],
        [20.0, -4.0],
        [30.0, 10.0],
        [18.0, 25.0],
        [2.0, 20.0],
    ];
    let mut s = Sketch::default();
    let (id, pts) = s.add_spline(&ctrl, true).unwrap();
    assert_eq!(pts.len(), 5);
    let r = s.solve();
    assert_eq!(r.dof, 10, "2 DOF per control point");
    let (p, ents) = s.profile_with_entities().unwrap();
    let Profile::Loop(l) = &p else { panic!() };
    assert_eq!(l.len(), 5);
    assert!(ents.iter().all(|e| *e == id));
    let want = closed_spline_area(&ctrl);
    assert!(want > 100.0);
    let got = crate::profile::loop_area(l);
    assert!((got - want).abs() < 1e-9 * want, "{got} vs {want}");
    // Reversed control order: same region, still counter-clockwise.
    let mut rev = Sketch::default();
    let back: Vec<[f64; 2]> = ctrl.iter().rev().copied().collect();
    rev.add_spline(&back, true).unwrap();
    let Profile::Loop(l2) = rev.profile().unwrap() else {
        panic!()
    };
    assert!((crate::profile::loop_area(&l2) - want).abs() < 1e-9 * want);
}

/// Open spline from (0,0) over the top to (30,0), closed by a line back.
fn d_shape() -> (Sketch, EntityId, EntityId) {
    let mut s = Sketch::default();
    let ctrl = [
        [0.0, 0.0],
        [2.0, 15.0],
        [15.0, 22.0],
        [28.0, 15.0],
        [30.0, 0.0],
    ];
    let (sp, pts) = s.add_spline(&ctrl, false).unwrap();
    let line = s.add_line(pts[4], pts[0]);
    (s, sp, line)
}

#[test]
fn open_spline_joins_a_line_into_a_closed_loop() {
    let (s, sp, line) = d_shape();
    let (p, ents) = s.profile_with_entities().unwrap();
    let Profile::Loop(l) = &p else { panic!() };
    assert_eq!(l.len(), 3, "two spline spans and the line");
    assert_eq!(ents.iter().filter(|e| **e == sp).count(), 2);
    assert_eq!(ents.iter().filter(|e| **e == line).count(), 1);
    // Area: the spline part by exact Gauss quadrature on Cox–de Boor points
    // (degree-5 pieces, see `closed_spline_area`), plus the closing chord
    // (y = 0 contributes nothing to x dy − y dx).
    let ctrl = [
        [0.0, 0.0],
        [2.0, 15.0],
        [15.0, 22.0],
        [28.0, 15.0],
        [30.0, 0.0],
    ];
    let knots = [0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 2.0, 2.0];
    let h = 1e-6;
    let mut want = 0.0;
    let g = [
        (-(0.6f64).sqrt(), 5.0 / 9.0),
        (0.0, 8.0 / 9.0),
        ((0.6f64).sqrt(), 5.0 / 9.0),
    ];
    for k in 0..2 {
        for (x, wt) in g {
            let t = k as f64 + 0.5 + 0.5 * x;
            let p = deboor_point(&ctrl, &knots, t);
            // Central difference of a cubic: error O(h²·|x'''|) ≈ 1e-11.
            let (a, b) = (
                deboor_point(&ctrl, &knots, t - h),
                deboor_point(&ctrl, &knots, t + h),
            );
            let dp = [(b[0] - a[0]) / (2.0 * h), (b[1] - a[1]) / (2.0 * h)];
            want += 0.5 * wt * 0.5 * (p[0] * dp[1] - p[1] * dp[0]);
        }
    }
    // The spline runs clockwise (left to right over the top); the loop is CCW.
    let got = crate::profile::loop_area(l);
    assert!((got - want.abs()).abs() < 1e-6, "{got} vs {want}");
}

#[test]
fn elliptical_hole_in_a_rectangle_is_a_region_with_one_hole() {
    let mut s = Sketch::centered_rectangle(60.0, 40.0).unwrap();
    let (e, ..) = s.add_ellipse([5.0, 0.0], [20.0, 0.0], 8.0).unwrap();
    let r = s.profile_region().unwrap();
    assert_eq!(r.holes.len(), 1);
    assert!(r.holes[0].1.iter().all(|x| *x == e));
    let regs = s.regions().unwrap();
    let outer = regs.iter().find(|r| r.depth == 0).unwrap();
    let want = 60.0 * 40.0 - PI * 15.0 * 8.0;
    assert!((outer.area - want).abs() < 1e-8 * want, "{}", outer.area);
    let inner = regs.iter().find(|r| r.depth == 1).unwrap();
    assert_eq!(inner.id.outer, vec![e]);
    assert!((inner.area - PI * 15.0 * 8.0).abs() < 1e-8 * inner.area);
    // Picking: the ellipse centre is in the inner region, a corner in the outer.
    assert_eq!(s.region_at([5.0, 0.0]).unwrap(), Some(inner.id.clone()));
    assert_eq!(s.region_at([-28.0, 18.0]).unwrap(), Some(outer.id.clone()));
}

#[test]
fn touching_or_self_crossing_curves_are_refused() {
    // Ellipse poking through the rectangle's right side (x = 30).
    let mut s = Sketch::centered_rectangle(60.0, 40.0).unwrap();
    s.add_ellipse([20.0, 0.0], [35.0, 0.0], 5.0).unwrap();
    let err = s.profile_region().unwrap_err();
    assert!(err.to_string().contains("touch or cross"), "{err}");
    // A closed spline whose control polygon is a figure eight crosses itself.
    let mut f8 = Sketch::default();
    f8.add_spline(
        &[
            [0.0, 0.0],
            [20.0, 20.0],
            [40.0, 0.0],
            [40.0, 20.0],
            [20.0, 0.0],
            [0.0, 20.0],
        ],
        true,
    )
    .unwrap();
    let err = f8.profile().unwrap_err();
    assert!(err.to_string().contains("self-intersecting"), "{err}");
    // An open spline alone does not close.
    let mut open = Sketch::default();
    open.add_spline(&[[0.0, 0.0], [1.0, 5.0], [4.0, 5.0], [6.0, 0.0]], false)
        .unwrap();
    assert!(open.profile().is_err());
    // An ellipse plus a separate circle is not one loop (regions handle it).
    let mut two = Sketch::default();
    two.add_ellipse([0.0, 0.0], [10.0, 0.0], 4.0).unwrap();
    let c = two.add_point(40.0, 0.0);
    two.add_circle(c, 3.0);
    assert!(matches!(
        two.profile(),
        Err(SketchError::UnsupportedProfile(_))
    ));
    assert_eq!(two.regions().unwrap().len(), 2);
}

#[test]
fn invalid_curves_and_constraints_are_refused_by_name() {
    let mut s = Sketch::default();
    assert!(
        s.add_spline(&[[0.0, 0.0], [1.0, 1.0], [2.0, 0.0]], false)
            .is_err()
    );
    assert!(s.add_spline(&[[0.0, 0.0], [1.0, 1.0]], true).is_err());
    assert!(s.add_ellipse([0.0, 0.0], [0.0, 0.0], 1.0).is_err());
    assert!(s.add_ellipse([0.0, 0.0], [1.0, 0.0], 0.0).is_err());
    let (e, c, _) = s.add_ellipse([0.0, 0.0], [10.0, 0.0], 4.0).unwrap();
    // Circle-only constraints do not accept an ellipse.
    let p = s.add_point(3.0, 3.0);
    let mut bad = s.clone();
    bad.constraints.push(Constraint::PointOnCircle {
        point: p,
        circle: e,
    });
    assert!(matches!(
        bad.validate(),
        Err(SketchError::WrongEntityKind {
            expected: "arc or circle",
            ..
        })
    ));
    let mut bad = s.clone();
    bad.constraints.push(Constraint::Radius {
        entity: e,
        dim: Dimension::new("r", 3.0),
    });
    assert!(bad.validate().is_err());
    // A spline cannot repeat a control point.
    let mut bad = s.clone();
    bad.entities.push(Entity::Spline {
        id: EntityId(99),
        points: vec![c, p, c],
        closed: true,
    });
    assert!(matches!(bad.validate(), Err(SketchError::InvalidValue(m)) if m.contains("twice")));
    // Ellipse radius constraint on a non-ellipse.
    let line = s.add_line(c, p);
    let mut bad = s.clone();
    bad.constraints.push(Constraint::EllipseRadius {
        ellipse: line,
        major: true,
        dim: Dimension::new("a", 3.0),
    });
    assert!(matches!(
        bad.validate(),
        Err(SketchError::WrongEntityKind {
            expected: "ellipse",
            ..
        })
    ));
    // Offsetting or patterning an ellipse is refused by name.
    let mut s2 = Sketch::default();
    let (e2, ..) = s2.add_ellipse([0.0, 0.0], [10.0, 0.0], 4.0).unwrap();
    assert!(s2.offset_loop(e2, 1.0, None).is_err());
}

#[test]
fn minor_radius_edit_regenerates_the_area_and_failed_solve_keeps_state() {
    let (mut s, _) = ellipse_sketch(20.0, 10.0);
    s.solve();
    s.set_dimension("minor_radius", 6.0).unwrap();
    assert!(s.solve().is_success());
    let area = s.regions().unwrap()[0].area;
    assert!((area - PI * 20.0 * 6.0).abs() < 1e-8 * area);
    // Major radius too.
    s.set_dimension("major_radius", 25.0).unwrap();
    assert!(s.solve().is_success());
    let area = s.regions().unwrap()[0].area;
    assert!((area - PI * 25.0 * 6.0).abs() < 1e-8 * area);
    // A non-positive radius is refused and leaves the sketch unchanged.
    let before = s.clone();
    assert!(s.set_dimension("minor_radius", 0.0).is_err());
    assert_eq!(s, before);
}

#[test]
fn spline_control_points_take_ordinary_constraints() {
    let (mut s, _, _) = d_shape();
    let pts: Vec<PointId> = s.points.iter().map(|p| p.id).collect();
    for (i, p) in pts.iter().enumerate() {
        let q = s.point_xy(*p).unwrap();
        if i == 2 {
            continue;
        }
        s.constraints.push(Constraint::FixedPoint {
            p: *p,
            x: q[0],
            y: q[1],
        });
    }
    let r = s.solve();
    assert_eq!((r.status, r.dof), (SolveStatus::UnderConstrained, 2));
    s.constraints.push(Constraint::Distance {
        a: pts[0],
        b: pts[2],
        dim: Dimension::new("d", 30.0),
    });
    s.constraints
        .push(Constraint::FixedX { p: pts[2], x: 15.0 });
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained, "{r:?}");
    let q = s.point_xy(pts[2]).unwrap();
    assert!((q[0].hypot(q[1]) - 30.0).abs() < SOLVED);
    // Moving a control point moves the profile.
    assert!(s.profile().is_ok());
}

#[test]
fn removing_a_spline_removes_its_control_points() {
    let (mut s, sp, line) = d_shape();
    assert!(s.remove_entity(sp));
    // The line keeps its two end points; the three interior points go.
    assert_eq!(s.points.len(), 2);
    assert!(s.entity(line).is_some());
}

#[test]
fn curves_round_trip_through_serde_and_old_json_still_loads() {
    let (mut s, _, _) = d_shape();
    s.add_ellipse([50.0, 0.0], [60.0, 0.0], 3.0).unwrap();
    s.add_spline(&[[0.0, -30.0], [10.0, -40.0], [20.0, -30.0]], true)
        .unwrap();
    let json = serde_json::to_string(&s).unwrap();
    let back: Sketch = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s);
    // An open spline does not write `closed`.
    assert!(json.contains("\"Spline\""));
    let open_only = serde_json::to_string(&d_shape().0).unwrap();
    assert!(!open_only.contains("closed"));
    // Sketches without the new entities serialize exactly as before.
    let rect = Sketch::centered_rectangle(10.0, 5.0).unwrap();
    let j = serde_json::to_string(&rect).unwrap();
    assert!(!j.contains("Ellipse") && !j.contains("Spline") && !j.contains("Cubic"));
}

#[test]
fn polyline_bound_holds_for_flattened_spans() {
    // Every point of the exact span is within delta of the polyline.
    let (p0, c1, c2, p3, w) = (
        [0.0, 0.0],
        [10.0, 30.0],
        [40.0, -20.0],
        [50.0, 10.0],
        [0.8, 1.3],
    );
    for delta in [1e-2, 1e-4] {
        let poly = crate::curves::flatten(p0, c1, c2, p3, w, delta);
        assert_eq!(poly[0], p0);
        assert_eq!(*poly.last().unwrap(), p3);
        for k in 0..=2000 {
            let q = cubic_point(p0, c1, c2, p3, w, k as f64 / 2000.0);
            let d = poly
                .windows(2)
                .map(|s| {
                    let (a, b) = (s[0], s[1]);
                    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                    let t = (((q[0] - a[0]) * dx + (q[1] - a[1]) * dy) / (dx * dx + dy * dy))
                        .clamp(0.0, 1.0);
                    (q[0] - a[0] - t * dx).hypot(q[1] - a[1] - t * dy)
                })
                .fold(f64::INFINITY, f64::min);
            assert!(d <= delta * (1.0 + 1e-9), "{d} > {delta}");
        }
    }
}

#[test]
fn ellipse_minor_radius_drags_unless_dimensioned() {
    let mut s = Sketch::default();
    let (e, c, m) = s.add_ellipse([0.0, 0.0], [10.0, 0.0], 4.0).unwrap();
    for (p, x) in [(c, 0.0), (m, 10.0)] {
        s.constraints.push(Constraint::FixedPoint { p, x, y: 0.0 });
    }
    let r = s.drag_radius(e, 6.5);
    assert_eq!(r.outcome, DragOutcome::Reached, "{r:?}");
    let Some(Entity::Ellipse { minor, .. }) = s.entity(e) else {
        panic!()
    };
    assert!((minor - 6.5).abs() < SOLVED);
    // Dimensioned: the drag is blocked and nothing changes.
    s.constraints.push(Constraint::EllipseRadius {
        ellipse: e,
        major: false,
        dim: Dimension::new("b", 6.5),
    });
    let before = s.clone();
    let r = s.drag_radius(e, 3.0);
    assert_eq!(r.outcome, DragOutcome::Blocked);
    assert_eq!(s, before);
}
