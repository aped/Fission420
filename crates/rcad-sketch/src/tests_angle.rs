//! Angle dimension between two lines.
use crate::*;

fn dir_deg(s: &Sketch, line: EntityId) -> f64 {
    let Some(Entity::Line { a, b, .. }) = s.entity(line) else {
        panic!()
    };
    let (p, q) = (s.point_xy(*a).unwrap(), s.point_xy(*b).unwrap());
    (q[1] - p[1]).atan2(q[0] - p[0]).to_degrees()
}

/// Two lines from a fixed origin: L1 horizontal, L2 free, angle L1 -> L2.
fn wedge(l2_end: [f64; 2], deg: f64) -> (Sketch, EntityId, EntityId) {
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let p1 = s.add_point(20.0, 0.0);
    let p2 = s.add_point(l2_end[0], l2_end[1]);
    let l1 = s.add_line(o, p1);
    let l2 = s.add_line(o, p2);
    s.constraints.push(Constraint::Horizontal { line: l1 });
    s.constraints.push(Constraint::Distance {
        a: o,
        b: p1,
        dim: Dimension::new("len1", 20.0),
    });
    s.constraints.push(Constraint::Distance {
        a: o,
        b: p2,
        dim: Dimension::new("len2", 10.0),
    });
    s.constraints.push(Constraint::Angle {
        a: l1,
        b: l2,
        dim: Dimension::new("angle", deg),
    });
    (s, l1, l2)
}

#[test]
fn angle_solves_fully_constrained_and_edits() {
    let (mut s, l1, l2) = wedge([8.0, 5.0], 30.0);
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained, "{r:?}");
    assert!((dir_deg(&s, l2) - dir_deg(&s, l1) - 30.0).abs() < 1e-9);
    // A large edit converges to the requested angle, not its 180° twin.
    s.set_dimension("angle", 135.0).unwrap();
    assert!(s.solve().is_success());
    assert!(
        (dir_deg(&s, l2) - 135.0).abs() < 1e-9,
        "{}",
        dir_deg(&s, l2)
    );
    s.set_dimension("angle", 10.0).unwrap();
    assert!(s.solve().is_success());
    assert!((dir_deg(&s, l2) - 10.0).abs() < 1e-9, "{}", dir_deg(&s, l2));
    assert!(
        s.constraint_label(4).contains("angle = 10°"),
        "{}",
        s.constraint_label(4)
    );
}

#[test]
fn angle_outside_open_interval_is_refused() {
    let (mut s, ..) = wedge([8.0, 5.0], 30.0);
    assert!(s.set_dimension("angle", 180.0).is_err());
    assert!(s.set_dimension("angle", 0.0).is_err());
    let (s2, ..) = wedge([8.0, 5.0], 190.0);
    assert!(s2.validate().is_err());
}

#[test]
fn angle_conflicting_with_parallel_is_reported() {
    let (mut s, l1, l2) = wedge([8.0, 5.0], 30.0);
    s.constraints.push(Constraint::Parallel { a: l1, b: l2 });
    let r = s.solve();
    assert!(!r.is_success());
    assert!(
        r.conflicting.contains(&4) || r.conflicting.contains(&5),
        "{r:?}"
    );
}
