//! Trim, Extend and Break (`trim.rs`). Expected positions and lengths come
//! from the construction (crossings of axis-aligned lines, a 6-8-10 triangle
//! on a radius-10 circle), not from solver output.
use super::*;
use std::f64::consts::{PI, TAU};

const SOLVED: f64 = 1e-8;

fn line(s: &mut Sketch, a: [f64; 2], b: [f64; 2]) -> EntityId {
    let p = s.add_point(a[0], a[1]);
    let q = s.add_point(b[0], b[1]);
    s.add_line(p, q)
}

fn ends(s: &Sketch, l: EntityId) -> ([f64; 2], [f64; 2]) {
    let Some(Entity::Line { a, b, .. }) = s.entity(l) else {
        panic!("{l:?} is not a line")
    };
    (s.point_xy(*a).unwrap(), s.point_xy(*b).unwrap())
}

fn near(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() <= SOLVED && (a[1] - b[1]).abs() <= SOLVED
}

fn solve_ok(s: &mut Sketch) -> SolveReport {
    s.validate().expect("valid after edit");
    let r = s.solve();
    assert!(r.is_success(), "{r:?}");
    r
}

/// Horizontal line 0..30 on y = 0 crossed by verticals at x = 10 and x = 20.
fn crossed_line() -> (Sketch, EntityId) {
    let mut s = Sketch::default();
    let h = line(&mut s, [0.0, 0.0], [30.0, 0.0]);
    line(&mut s, [10.0, -5.0], [10.0, 5.0]);
    line(&mut s, [20.0, -5.0], [20.0, 5.0]);
    (s, h)
}

#[test]
fn trim_middle_of_a_line_splits_it_at_the_crossings() {
    let (mut s, h) = crossed_line();
    s.constraints.push(Constraint::Horizontal { line: h });
    let next = s.next_entity_id();
    let out = s.trim(h, [15.0, 0.0]).unwrap();
    assert_eq!(out.created, vec![next], "the second piece gets a fresh id");
    assert!(out.removed.is_empty());
    assert_eq!(out.free_ends, 0, "both cut ends are held on the verticals");
    solve_ok(&mut s);
    let (p, q) = ends(&s, h);
    assert!(near(p, [0.0, 0.0]) && near(q, [10.0, 0.0]), "{p:?} {q:?}");
    let (p, q) = ends(&s, next);
    assert!(near(p, [20.0, 0.0]) && near(q, [30.0, 0.0]), "{p:?} {q:?}");
    let hs = s
        .constraints
        .iter()
        .filter(|c| matches!(c, Constraint::Horizontal { .. }))
        .count();
    assert_eq!(hs, 2, "Horizontal copied to the new piece");
}

#[test]
fn trim_overhang_moves_the_end_to_the_crossing_and_drops_the_old_point() {
    let (mut s, h) = crossed_line();
    let points = s.points.len();
    let out = s.trim(h, [27.0, 0.0]).unwrap();
    assert!(out.created.is_empty() && out.removed.is_empty());
    solve_ok(&mut s);
    let (p, q) = ends(&s, h);
    assert!(near(p, [0.0, 0.0]) && near(q, [20.0, 0.0]), "{p:?} {q:?}");
    assert_eq!(s.points.len(), points, "old end replaced by the cut point");
}

#[test]
fn trim_a_line_with_no_crossing_deletes_it_and_retires_its_id() {
    let mut s = Sketch::default();
    let l = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let out = s.trim(l, [5.0, 0.0]).unwrap();
    assert_eq!(out.removed, vec![l]);
    assert!(s.entities.is_empty() && s.points.is_empty());
    assert!(s.next_entity_id().0 > l.0, "removed ids are not reused");
}

#[test]
fn trim_a_line_across_a_circle_keeps_the_outside_pieces() {
    let mut s = Sketch::default();
    let c = s.add_point(0.0, 0.0);
    s.add_circle(c, 10.0);
    s.constraints.push(Constraint::FixedPoint {
        p: c,
        x: 0.0,
        y: 0.0,
    });
    let h = line(&mut s, [-20.0, 0.0], [20.0, 0.0]);
    let out = s.trim(h, [1.0, 0.0]).unwrap();
    assert_eq!(out.created.len(), 1);
    assert_eq!(out.free_ends, 0);
    let on_circle = s
        .constraints
        .iter()
        .filter(|k| matches!(k, Constraint::PointOnCircle { .. }))
        .count();
    assert_eq!(on_circle, 2, "both cut ends are held on the circle");
    solve_ok(&mut s);
    let (p, q) = ends(&s, h);
    assert!(
        near(p, [-20.0, 0.0]) && near(q, [-10.0, 0.0]),
        "{p:?} {q:?}"
    );
    let (p, q) = ends(&s, out.created[0]);
    assert!(near(p, [10.0, 0.0]) && near(q, [20.0, 0.0]), "{p:?} {q:?}");
}

/// Circle r = 10 at the origin, vertical line x = 6 crossing it at (6, ±8).
fn circle_and_chord() -> (Sketch, EntityId) {
    let mut s = Sketch::default();
    let c = s.add_point(0.0, 0.0);
    let k = s.add_circle(c, 10.0);
    s.constraints.push(Constraint::FixedPoint {
        p: c,
        x: 0.0,
        y: 0.0,
    });
    s.constraints.push(Constraint::Diameter {
        circle: k,
        dim: Dimension::new("d", 20.0),
    });
    line(&mut s, [6.0, -20.0], [6.0, 20.0]);
    (s, k)
}

#[test]
fn trim_a_circle_leaves_an_arc_with_the_same_id_and_a_radius_dimension() {
    let (mut s, k) = circle_and_chord();
    let out = s.trim(k, [10.0, 0.0]).unwrap();
    assert!(out.created.is_empty() && out.removed.is_empty());
    solve_ok(&mut s);
    let g = s
        .arc_geometry(k)
        .expect("circle became an arc of the same id");
    assert!((g.radius - 10.0).abs() <= SOLVED);
    // Kept: the big arc from (6, 8) CCW round to (6, -8).
    let half = 8f64.atan2(6.0);
    assert!((g.sweep - (TAU - 2.0 * half)).abs() <= 1e-9, "{}", g.sweep);
    assert!((g.start_angle - half).abs() <= 1e-9, "{}", g.start_angle);
    assert_eq!(s.dimensions(), vec![("d".to_string(), 10.0)]);
    assert!(
        s.constraints
            .iter()
            .any(|c| matches!(c, Constraint::Radius { entity, .. } if *entity == k))
    );
}

#[test]
fn trim_the_middle_of_an_arc_splits_it_with_equal_radii() {
    // Arc: upper half of r = 10 from (10, 0) to (-10, 0); verticals at x = ±6
    // cross it at (±6, 8).
    let mut s = Sketch::default();
    let c = s.add_point(0.0, 0.0);
    let a0 = s.add_point(10.0, 0.0);
    let a1 = s.add_point(-10.0, 0.0);
    let arc = s.next_entity_id();
    s.entities.push(Entity::Arc {
        id: arc,
        center: c,
        start: a0,
        end: a1,
    });
    s.constraints.push(Constraint::FixedPoint {
        p: c,
        x: 0.0,
        y: 0.0,
    });
    line(&mut s, [6.0, 0.0], [6.0, 20.0]);
    line(&mut s, [-6.0, 0.0], [-6.0, 20.0]);
    let out = s.trim(arc, [0.0, 10.0]).unwrap();
    assert_eq!(out.created.len(), 1);
    solve_ok(&mut s);
    let half = 8f64.atan2(6.0); // angle of (6, 8)
    let g0 = s.arc_geometry(arc).unwrap();
    let g1 = s.arc_geometry(out.created[0]).unwrap();
    assert!((g0.sweep - half).abs() <= 1e-9, "{}", g0.sweep);
    assert!((g1.sweep - half).abs() <= 1e-9, "{}", g1.sweep);
    assert!((g1.start_angle - (PI - half)).abs() <= 1e-9);
    assert!(s.constraints.contains(&Constraint::EqualRadius {
        a: arc,
        b: out.created[0]
    }));
}

#[test]
fn split_line_drops_the_dimension_spanning_both_pieces() {
    let (mut s, h) = crossed_line();
    let Some(Entity::Line { a, b, .. }) = s.entity(h).cloned() else {
        unreachable!()
    };
    s.constraints.push(Constraint::HorizontalDistance {
        a,
        b,
        dim: Dimension::new("len", 30.0),
    });
    let out = s.trim(h, [15.0, 0.0]).unwrap();
    assert_eq!(out.split_dimensions, vec!["len".to_string()]);
    assert!(s.dimensions().is_empty());
    assert!(out.message("Trimmed").contains("'len'"));
}

#[test]
fn trim_moves_a_tangency_to_the_piece_that_keeps_the_shared_point() {
    let mut s = Sketch::default();
    let (c1, c2, c3) = (
        s.add_point(0.0, 0.0),
        s.add_point(20.0, 0.0),
        s.add_point(0.0, 0.1),
    );
    let p = s.add_point(10.0, 0.0);
    let (s1, e2, s3) = (
        s.add_point(-10.0, 0.0),
        s.add_point(30.0, 0.0),
        s.add_point(0.0, 10.1),
    );
    let arc = |sk: &mut Sketch, center, start, end| {
        let id = sk.next_entity_id();
        sk.entities.push(Entity::Arc {
            id,
            center,
            start,
            end,
        });
        id
    };
    let a = arc(&mut s, c1, s1, p);
    let b = arc(&mut s, c2, p, e2);
    s.constraints.push(Constraint::TangentArcs { a, b });
    // `a` moved off P and a new piece (c3) now ends at P.
    let piece = arc(&mut s, c3, s3, p);
    for e in &mut s.entities {
        if let Entity::Arc { id, end, .. } = e
            && *id == a
        {
            *end = s3;
        }
    }
    assert_eq!(trim::retarget_tangents(&mut s, a, &[piece]), 0);
    assert!(
        s.constraints
            .contains(&Constraint::TangentArcs { a: piece, b })
    );
}

/// Rectangle 0..40 x 0..20 (4 lines) and a free line (5,10)-(15,10) inside.
fn rect_and_inner_line() -> (Sketch, EntityId, EntityId) {
    let mut s = Sketch::default();
    let p = [
        s.add_point(0.0, 0.0),
        s.add_point(40.0, 0.0),
        s.add_point(40.0, 20.0),
        s.add_point(0.0, 20.0),
    ];
    let first = s.add_line(p[0], p[1]);
    for i in 1..4 {
        s.add_line(p[i], p[(i + 1) % 4]);
    }
    let l = line(&mut s, [5.0, 10.0], [15.0, 10.0]);
    (s, first, l)
}

#[test]
fn extend_moves_the_nearest_free_end_to_the_next_curve() {
    let (mut s, _, l) = rect_and_inner_line();
    let points = s.points.len();
    let out = s.extend(l, [13.0, 10.0], None).unwrap();
    assert_eq!(out.free_ends, 0, "held on the right edge");
    solve_ok(&mut s);
    let (p, q) = ends(&s, l);
    assert!(near(p, [5.0, 10.0]) && near(q, [40.0, 10.0]), "{p:?} {q:?}");
    s.extend(l, [8.0, 10.0], None).unwrap();
    solve_ok(&mut s);
    let (p, q) = ends(&s, l);
    assert!(near(p, [0.0, 10.0]) && near(q, [40.0, 10.0]), "{p:?} {q:?}");
    assert_eq!(s.points.len(), points, "old ends replaced, not added");
}

#[test]
fn extend_refuses_connected_ends_arcs_and_empty_extensions() {
    let (mut s, corner, l) = rect_and_inner_line();
    let before = s.clone();
    let err = s.extend(corner, [35.0, 0.0], None).unwrap_err();
    assert!(err.to_string().contains("connected"), "{err}");
    assert_eq!(s, before);
    // A vertical line far right: nothing in its extension.
    let mut s2 = Sketch::default();
    let v = line(&mut s2, [0.0, 0.0], [0.0, 10.0]);
    let err = s2.extend(v, [0.0, 9.0], None).unwrap_err();
    assert!(err.to_string().contains("Nothing to extend to"), "{err}");
    let _ = l;
}

#[test]
fn break_a_line_keeps_every_piece_collinear() {
    let (mut s, h) = crossed_line();
    let dof0 = {
        let mut c = s.clone();
        c.solve().dof
    };
    let next = s.next_entity_id();
    let out = s.break_at(h, [15.0, 0.0]).unwrap();
    assert_eq!(out.created, vec![next, EntityId(next.0 + 1)]);
    assert!(out.removed.is_empty());
    let r = solve_ok(&mut s);
    assert_eq!(r.dof, dof0, "breaking adds no freedom");
    let mut total = 0.0;
    for (e, want) in [
        (h, ([0.0, 0.0], [10.0, 0.0])),
        (out.created[0], ([10.0, 0.0], [20.0, 0.0])),
        (out.created[1], ([20.0, 0.0], [30.0, 0.0])),
    ] {
        let (p, q) = ends(&s, e);
        assert!(near(p, want.0) && near(q, want.1), "{e:?}: {p:?} {q:?}");
        total += q[0] - p[0];
    }
    assert!((total - 30.0).abs() <= 3.0 * SOLVED);
    // Moving one end keeps the pieces on one line.
    let Some(Entity::Line { b, .. }) = s.entity(out.created[1]).cloned() else {
        unreachable!()
    };
    let d = s.drag_point(b, [30.0, 6.0]);
    assert_ne!(d.outcome, DragOutcome::Blocked);
    let (p0, _) = ends(&s, h);
    let (_, q1) = ends(&s, out.created[0]);
    let (_, q2) = ends(&s, out.created[1]);
    let cross = |u: [f64; 2], v: [f64; 2], w: [f64; 2]| {
        (v[0] - u[0]) * (w[1] - u[1]) - (v[1] - u[1]) * (w[0] - u[0])
    };
    assert!(cross(p0, q1, q2).abs() < 1e-6, "pieces stay collinear");
}

#[test]
fn break_a_circle_gives_two_arcs_that_close_it() {
    let (mut s, k) = circle_and_chord();
    let out = s.break_at(k, [10.0, 0.0]).unwrap();
    assert_eq!(out.created.len(), 1);
    solve_ok(&mut s);
    let g0 = s.arc_geometry(k).unwrap();
    let g1 = s.arc_geometry(out.created[0]).unwrap();
    assert!((g0.sweep + g1.sweep - TAU).abs() <= 1e-9);
    let half = 8f64.atan2(6.0);
    assert!((g1.sweep - 2.0 * half).abs() <= 1e-9, "clicked span");
    assert!(s.constraints.contains(&Constraint::EqualRadius {
        a: k,
        b: out.created[0]
    }));
    // The two arcs share their end points (one closed loop).
    let (
        Entity::Arc {
            start: s0, end: e0, ..
        },
        Entity::Arc {
            start: s1, end: e1, ..
        },
    ) = (
        s.entity(k).cloned().unwrap(),
        s.entity(out.created[0]).cloned().unwrap(),
    )
    else {
        unreachable!()
    };
    assert_eq!((s0, e0), (e1, s1));
}

#[test]
fn break_without_a_crossing_and_free_form_sketches_are_refused() {
    let mut s = Sketch::default();
    let l = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let before = s.clone();
    let err = s.break_at(l, [5.0, 0.0]).unwrap_err();
    assert!(matches!(err, SketchError::Refused(_)), "{err}");
    assert_eq!(s, before);
    assert!(matches!(
        s.trim(EntityId(999), [0.0, 0.0]),
        Err(SketchError::MissingEntity(_))
    ));
    let c = s.add_point(0.0, 20.0);
    let m = s.add_point(5.0, 20.0);
    s.entities.push(Entity::Ellipse {
        id: s.next_entity_id(),
        center: c,
        major: m,
        minor: 2.0,
    });
    let before = s.clone();
    for err in [
        s.trim(l, [5.0, 0.0]).unwrap_err(),
        s.extend(l, [9.0, 0.0], None).unwrap_err(),
        s.break_at(l, [5.0, 0.0]).unwrap_err(),
    ] {
        assert!(err.to_string().contains("ellipses or splines"), "{err}");
    }
    assert_eq!(s, before);
}
