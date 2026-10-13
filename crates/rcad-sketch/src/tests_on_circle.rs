//! Point-on-circle / point-on-arc constraint: validation, DOF accounting,
//! solving, endpoint movement after upstream edits and drags, conflicts, and
//! serde. Expected positions are analytic (distances from known centres).
use super::*;

/// Solved coordinates are compared at 1e-8 mm (docs/TOLERANCES.md).
const SOLVED: f64 = 1e-8;

fn xy(s: &Sketch, p: PointId) -> [f64; 2] {
    s.point_xy(p).expect("point exists")
}

fn d(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Circle centred (0, 0), Ø20 (dimension "dia", centre fixed); a line from a
/// fixed point (30, 0) to a free end `q` near the circle. Returns
/// (sketch, circle, centre, q).
fn circle_and_line(q0: [f64; 2]) -> (Sketch, EntityId, PointId, PointId) {
    let mut s = Sketch::default();
    let c = s.add_point(0.0, 0.0);
    let circle = s.add_circle(c, 10.0);
    let a = s.add_point(30.0, 0.0);
    let q = s.add_point(q0[0], q0[1]);
    s.add_line(a, q);
    s.constraints = vec![
        Constraint::FixedPoint {
            p: c,
            x: 0.0,
            y: 0.0,
        },
        Constraint::Diameter {
            circle,
            dim: Dimension::new("dia", 20.0),
        },
        Constraint::FixedPoint {
            p: a,
            x: 30.0,
            y: 0.0,
        },
    ];
    (s, circle, c, q)
}

#[test]
fn validation_refuses_centre_arc_ends_and_lines() {
    let (mut s, circle, c, q) = circle_and_line([12.0, 3.0]);
    s.constraints
        .push(Constraint::PointOnCircle { point: q, circle });
    s.validate().unwrap();
    // The centre is not on its circle.
    let mut bad = s.clone();
    bad.constraints
        .push(Constraint::PointOnCircle { point: c, circle });
    assert!(matches!(bad.validate(), Err(SketchError::InvalidValue(m)) if m.contains("centre")));
    // A line is not a circle.
    let line = s.entities[1].id();
    let mut bad = s.clone();
    bad.constraints.push(Constraint::PointOnCircle {
        point: q,
        circle: line,
    });
    assert!(matches!(
        bad.validate(),
        Err(SketchError::WrongEntityKind { .. })
    ));
    // An arc's own end lies on it by definition: refused as redundant.
    let mut s2 = Sketch::default();
    let a0 = s2.add_point(0.0, 0.0);
    let a1 = s2.add_point(10.0, 0.0);
    let arc = s2.add_arc_3pt(a1, a0, [5.0, 5.0]).unwrap();
    s2.constraints.push(Constraint::PointOnCircle {
        point: a0,
        circle: arc,
    });
    assert!(matches!(s2.validate(), Err(SketchError::InvalidValue(_))));
    // Missing point / entity.
    let mut bad = s.clone();
    bad.constraints.push(Constraint::PointOnCircle {
        point: PointId(999),
        circle,
    });
    assert!(bad.validate().is_err());
}

#[test]
fn solve_puts_the_end_on_the_circle_and_removes_one_dof() {
    let (mut free, _, _, q) = circle_and_line([12.0, 3.0]);
    let before = free.solve();
    assert!(before.is_success(), "{before:?}");
    let (mut s, circle, c, _) = circle_and_line([12.0, 3.0]);
    s.constraints
        .push(Constraint::PointOnCircle { point: q, circle });
    let r = s.solve();
    assert!(r.is_success(), "{r:?}");
    assert_eq!(r.status, SolveStatus::UnderConstrained);
    assert_eq!(r.dof, before.dof - 1, "one equation, one DOF");
    assert!(
        (d(xy(&s, q), xy(&s, c)) - 10.0).abs() < SOLVED,
        "{:?}",
        xy(&s, q)
    );
    // Fully constrained: also fix the end's y; then q is one of the two
    // intersections, the one nearest the start (y = 6 => x = 8).
    s.constraints.push(Constraint::VerticalDistance {
        a: c,
        b: q,
        dim: Dimension::new("h", 6.0),
    });
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained, "{r:?}");
    assert_eq!(r.dof, 0);
    let p = xy(&s, q);
    assert!(
        (p[0] - 8.0).abs() < SOLVED && (p[1] - 6.0).abs() < SOLVED,
        "{p:?}"
    );
}

#[test]
fn end_follows_the_circle_after_a_dimension_edit_and_a_drag() {
    let (mut s, circle, c, q) = circle_and_line([12.0, 3.0]);
    s.constraints
        .push(Constraint::PointOnCircle { point: q, circle });
    assert!(s.solve().is_success());
    // Upstream edit: Ø20 -> Ø30; the end moves onto the new circle.
    s.set_dimension("dia", 30.0).unwrap();
    let r = s.solve();
    assert!(r.is_success(), "{r:?}");
    assert!((d(xy(&s, q), xy(&s, c)) - 15.0).abs() < SOLVED);
    // Drag the end toward a point off the circle: it stays on the circle.
    let res = s.drag_point(q, [3.0, 20.0]);
    assert!(res.report.is_success(), "{res:?}");
    let p = xy(&s, q);
    assert!((d(p, xy(&s, c)) - 15.0).abs() < SOLVED, "{p:?}");
    assert!(p[1] > 10.0, "moved toward the target: {p:?}");
}

#[test]
fn arc_end_stays_on_the_arc_circle_when_the_arc_changes() {
    // Arc centre (5, 0) radius 5 through (10,0) -> (0,0) CCW via (5,5); a free
    // point held on it; then the arc's radius dimension changes.
    let mut s = Sketch::default();
    let a0 = s.add_point(0.0, 0.0);
    let a1 = s.add_point(10.0, 0.0);
    let arc = s.add_arc_3pt(a1, a0, [5.0, 5.0]).unwrap();
    let Some(Entity::Arc { center, .. }) = s.entity(arc).cloned() else {
        panic!()
    };
    let p = s.add_point(6.0, 7.0);
    s.constraints = vec![
        Constraint::FixedPoint {
            p: center,
            x: 5.0,
            y: 0.0,
        },
        Constraint::PointOnCircle {
            point: p,
            circle: arc,
        },
        Constraint::Radius {
            entity: arc,
            dim: Dimension::new("r", 5.0),
        },
    ];
    let r = s.solve();
    assert!(r.is_success(), "{r:?}");
    assert!((d(xy(&s, p), [5.0, 0.0]) - 5.0).abs() < SOLVED);
    s.set_dimension("r", 8.0).unwrap();
    let r = s.solve();
    assert!(r.is_success(), "{r:?}");
    for q in [p, a0, a1] {
        assert!((d(xy(&s, q), [5.0, 0.0]) - 8.0).abs() < SOLVED, "{q:?}");
    }
}

#[test]
fn inconsistent_point_on_circle_is_reported() {
    // Fixed circle Ø20 at the origin, end fixed at (12, 0): on-circle is impossible.
    let (mut s, circle, _, q) = circle_and_line([12.0, 0.0]);
    s.constraints.push(Constraint::FixedPoint {
        p: q,
        x: 12.0,
        y: 0.0,
    });
    s.constraints
        .push(Constraint::PointOnCircle { point: q, circle });
    let before = s.clone();
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::Inconsistent, "{r:?}");
    assert!(r.conflicting.contains(&(s.constraints.len() - 1)), "{r:?}");
    assert_eq!(s, before, "sketch unchanged on failure");
}

#[test]
fn serde_round_trip_and_label() {
    let (mut s, circle, _, q) = circle_and_line([12.0, 3.0]);
    s.constraints
        .push(Constraint::PointOnCircle { point: q, circle });
    let json = serde_json::to_string(&s).unwrap();
    assert!(json.contains("\"PointOnCircle\""), "{json}");
    let back: Sketch = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s);
    let label = s.constraint_label(s.constraints.len() - 1);
    assert!(label.contains("on C"), "{label}");
    // Deleting the circle removes the constraint with it.
    assert!(s.remove_entity(circle));
    assert!(
        !s.constraints
            .iter()
            .any(|c| matches!(c, Constraint::PointOnCircle { .. }))
    );
}
