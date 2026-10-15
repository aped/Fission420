//! Fix, Concentric, Collinear and Symmetric. Expected positions come from
//! the construction (reflection about a line through (5, 0) at 30°, shared
//! centres, a common line), DOF counts from equations per constraint.
use super::*;
use std::f64::consts::FRAC_PI_2;

const SOLVED: f64 = 1e-8;

fn at(s: &Sketch, p: PointId, want: [f64; 2]) {
    let got = s.point_xy(p).unwrap();
    assert!(
        (got[0] - want[0]).abs() <= SOLVED && (got[1] - want[1]).abs() <= SOLVED,
        "{p:?}: {got:?} vs {want:?}"
    );
}

fn dof(s: &Sketch) -> usize {
    s.clone().solve().dof
}

#[test]
fn fixing_a_free_line_removes_four_dof_and_blocks_drags() {
    let mut s = Sketch::default();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(10.0, 3.0);
    s.add_line(a, b);
    assert_eq!(dof(&s), 4);
    s.fix_points(&[a, b]);
    let r = s.solve();
    assert_eq!((r.status, r.dof), (SolveStatus::FullyConstrained, 0));
    let d = s.drag_point(b, [20.0, 20.0]);
    assert_eq!(d.outcome, DragOutcome::Blocked);
    at(&s, b, [10.0, 3.0]);
    // Fixing again adds nothing; unfixing restores the freedom.
    assert_eq!(s.fix_points(&[a, b]), 0);
    assert_eq!(s.unfix_points(&[a, b]), 2);
    assert_eq!(dof(&s), 4);
}

#[test]
fn concentric_circles_share_a_centre_and_drop_two_dof() {
    let mut s = Sketch::default();
    let c1 = s.add_point(0.0, 0.0);
    let c2 = s.add_point(3.0, 1.0);
    let k1 = s.add_circle(c1, 10.0);
    let k2 = s.add_circle(c2, 4.0);
    s.constraints.push(Constraint::FixedPoint {
        p: c1,
        x: 0.0,
        y: 0.0,
    });
    let before = dof(&s);
    s.constraints.push(Constraint::Concentric { a: k1, b: k2 });
    let r = s.solve();
    assert!(r.is_success());
    assert_eq!(r.dof, before - 2);
    at(&s, c2, [0.0, 0.0]);
    // Redundant with a coincidence of the centres: reported.
    s.constraints.push(Constraint::Coincident { a: c1, b: c2 });
    assert_eq!(s.clone().solve().status, SolveStatus::OverConstrained);
    // Same centre point: refused by validation.
    let mut t = Sketch::default();
    let c = t.add_point(0.0, 0.0);
    let a = t.add_circle(c, 1.0);
    let b = t.add_circle(c, 2.0);
    t.constraints.push(Constraint::Concentric { a, b });
    assert!(t.validate().is_err());
}

#[test]
fn collinear_lines_share_one_line_and_parallel_is_then_redundant() {
    let mut s = Sketch::default();
    let p = [
        s.add_point(0.0, 0.0),
        s.add_point(10.0, 0.0),
        s.add_point(15.0, 2.0),
        s.add_point(25.0, 3.0),
    ];
    let a = s.add_line(p[0], p[1]);
    let b = s.add_line(p[2], p[3]);
    s.fix_points(&[p[0], p[1]]);
    let before = dof(&s);
    s.constraints.push(Constraint::Collinear { a, b });
    let r = s.solve();
    assert!(r.is_success());
    assert_eq!(r.dof, before - 2);
    for q in [p[2], p[3]] {
        assert!(s.point_xy(q).unwrap()[1].abs() <= SOLVED, "on y = 0");
    }
    s.constraints.push(Constraint::Parallel { a, b });
    assert_eq!(s.clone().solve().status, SolveStatus::OverConstrained);
    // Lines joined end to end are refused (use Parallel).
    let mut t = Sketch::default();
    let q0 = t.add_point(0.0, 0.0);
    let q1 = t.add_point(1.0, 0.0);
    let q2 = t.add_point(2.0, 0.1);
    let l1 = t.add_line(q0, q1);
    let l2 = t.add_line(q1, q2);
    t.constraints.push(Constraint::Collinear { a: l1, b: l2 });
    assert!(t.validate().is_err());
}

#[test]
fn symmetric_points_reflect_about_an_oblique_line() {
    // Mirror line through (5, 0) at 30°, fixed; point a fixed at (10, 6).
    let mut s = Sketch::default();
    let (t, c) = (30f64.to_radians(), [5.0, 0.0]);
    let l0 = s.add_point(c[0], c[1]);
    let l1 = s.add_point(c[0] + 10.0 * t.cos(), c[1] + 10.0 * t.sin());
    let line = s.add_line(l0, l1);
    let a = s.add_point(10.0, 6.0);
    let b = s.add_point(0.0, 0.0);
    s.fix_points(&[l0, l1, a]);
    assert_eq!(dof(&s), 2);
    s.constraints.push(Constraint::Symmetric {
        a,
        b,
        line,
        perpendicular_only: false,
    });
    let r = s.solve();
    assert_eq!((r.status, r.dof), (SolveStatus::FullyConstrained, 0));
    // Reflection of (10, 6) about the line: c + R(2t)(a − c).
    let (dx, dy) = (10.0 - c[0], 6.0);
    let (c2, s2) = ((2.0 * t).cos(), (2.0 * t).sin());
    let want = [c[0] + c2 * dx + s2 * dy, c[1] + s2 * dx - c2 * dy];
    at(&s, b, want);
    // The label names the pair and the line; deleting the line removes it.
    let i = s.constraints.len() - 1;
    assert!(s.constraint_label(i).contains("symmetric about"));
    s.remove_entity(line);
    assert!(
        !s.constraints
            .iter()
            .any(|c| matches!(c, Constraint::Symmetric { .. }))
    );
}

/// Fixed mirror line through (5, 0) at 30°, as in the point-pair test.
fn mirror_sketch() -> (Sketch, EntityId, [f64; 2], f64) {
    let mut s = Sketch::default();
    let (t, c) = (30f64.to_radians(), [5.0, 0.0]);
    let l0 = s.add_point(c[0], c[1]);
    let l1 = s.add_point(c[0] + 10.0 * t.cos(), c[1] + 10.0 * t.sin());
    let line = s.add_line(l0, l1);
    s.fix_points(&[l0, l1]);
    (s, line, c, t)
}

fn reflect(c: [f64; 2], t: f64, p: [f64; 2]) -> [f64; 2] {
    let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
    let (c2, s2) = ((2.0 * t).cos(), (2.0 * t).sin());
    [c[0] + c2 * dx + s2 * dy, c[1] + s2 * dx - c2 * dy]
}

#[test]
fn symmetric_lines_mirror_end_to_end() {
    let (mut s, line, c, t) = mirror_sketch();
    let a0 = s.add_point(10.0, 6.0);
    let a1 = s.add_point(14.0, 9.0);
    let a = s.add_line(a0, a1);
    s.fix_points(&[a0, a1]);
    // b drawn roughly mirrored but reversed (b0 near the image of a1).
    let (ia0, ia1) = (reflect(c, t, [10.0, 6.0]), reflect(c, t, [14.0, 9.0]));
    let b0 = s.add_point(ia1[0] + 0.7, ia1[1] - 0.4);
    let b1 = s.add_point(ia0[0] - 0.5, ia0[1] + 0.6);
    let b = s.add_line(b0, b1);
    assert_eq!(s.clone().solve().dof, 4);
    let cs = s.symmetric_entities(a, b, line).unwrap();
    assert_eq!(cs.len(), 2);
    s.constraints.extend(cs);
    let r = s.solve();
    assert_eq!((r.status, r.dof), (SolveStatus::FullyConstrained, 0));
    at(&s, b0, ia1);
    at(&s, b1, ia0);
}

#[test]
fn symmetric_circles_mirror_centre_and_radius() {
    let (mut s, line, c, t) = mirror_sketch();
    let ca = s.add_point(10.0, 6.0);
    let a = s.add_circle(ca, 3.0);
    s.fix_points(&[ca]);
    s.constraints.push(Constraint::Diameter {
        circle: a,
        dim: Dimension::new("d", 6.0),
    });
    let want = reflect(c, t, [10.0, 6.0]);
    let cb = s.add_point(want[0] + 1.0, want[1] - 1.0);
    let b = s.add_circle(cb, 2.0);
    let cs = s.symmetric_entities(a, b, line).unwrap();
    s.constraints.extend(cs);
    let r = s.solve();
    assert_eq!((r.status, r.dof), (SolveStatus::FullyConstrained, 0));
    at(&s, cb, want);
    let Some(Entity::Circle { radius, .. }) = s.entity(b) else {
        unreachable!()
    };
    assert!((radius - 3.0).abs() <= SOLVED);
}

#[test]
fn symmetric_entities_refuse_arcs_mixed_kinds_and_the_mirror_itself() {
    let (mut s, line, _, _) = mirror_sketch();
    let p = s.add_point(0.0, 5.0);
    let q = s.add_point(3.0, 5.0);
    let l = s.add_line(p, q);
    let c = s.add_point(0.0, -5.0);
    let k = s.add_circle(c, 1.0);
    for (a, b) in [(l, k), (l, line), (l, l)] {
        assert!(s.symmetric_entities(a, b, line).is_err(), "{a:?} {b:?}");
    }
    let (cc, s0, e0) = (
        s.add_point(0.0, 10.0),
        s.add_point(1.0, 10.0),
        s.add_point(0.0, 11.0),
    );
    let arc = s.next_entity_id();
    s.entities.push(Entity::Arc {
        id: arc,
        center: cc,
        start: s0,
        end: e0,
    });
    let err = s.symmetric_entities(arc, k, line).unwrap_err();
    assert!(err.to_string().contains("arc mirrors only"), "{err}");
}

/// Vertical mirror line x = 0, fixed.
fn y_axis_mirror() -> (Sketch, EntityId) {
    let mut s = Sketch::default();
    let m0 = s.add_point(0.0, -20.0);
    let m1 = s.add_point(0.0, 20.0);
    let line = s.add_line(m0, m1);
    s.fix_points(&[m0, m1]);
    (s, line)
}

fn arc(s: &mut Sketch, c: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (EntityId, [PointId; 3]) {
    let p = [
        s.add_point(c[0], c[1]),
        s.add_point(a[0], a[1]),
        s.add_point(b[0], b[1]),
    ];
    let id = s.next_entity_id();
    s.entities.push(Entity::Arc {
        id,
        center: p[0],
        start: p[1],
        end: p[2],
    });
    (id, p)
}

/// Arc a: centre (10, 0) from (15, 0) CCW to (10, 5), fixed. Its image
/// about x = 0 runs CCW from (−10, 5) to (−15, 0) about (−10, 0): b's start
/// is the image of a's end and b's end of a's start. Exactly determined.
#[test]
fn symmetric_arcs_mirror_with_swapped_ends_and_stay_square() {
    let (mut s, line) = y_axis_mirror();
    let (a, pa) = arc(&mut s, [10.0, 0.0], [15.0, 0.0], [10.0, 5.0]);
    // Centre and start fixed; the end by its x (its own row gives the rest).
    s.fix_points(&pa[..2]);
    s.constraints.push(Constraint::FixedX { p: pa[2], x: 10.0 });
    let (b, pb) = arc(&mut s, [-9.0, 1.0], [-10.5, 4.0], [-14.5, 0.5]);
    assert_eq!(s.clone().solve().dof, 5, "a free arc has 5 DOF");
    let cs = s.symmetric_entities(a, b, line).unwrap();
    s.constraints.extend(cs);
    let r = s.solve();
    assert_eq!(
        (r.status, r.dof),
        (SolveStatus::FullyConstrained, 0),
        "{r:?}"
    );
    at(&s, pb[0], [-10.0, 0.0]);
    at(&s, pb[1], [-10.0, 5.0]);
    at(&s, pb[2], [-15.0, 0.0]);
    let g = s.arc_geometry(b).unwrap();
    assert!((g.radius - 5.0).abs() <= SOLVED && (g.sweep - FRAC_PI_2).abs() < 1e-9);
}

/// Two arcs meeting on the mirror line at both ends (a lens): one row
/// (equal radii) puts b's centre at the image of a's.
#[test]
fn symmetric_arcs_sharing_their_ends_mirror_only_the_centres() {
    let (mut s, line) = y_axis_mirror();
    let (a, pa) = arc(&mut s, [3.0, 0.0], [0.0, -4.0], [0.0, 4.0]);
    // Ends fixed on the mirror line; the centre by its x.
    s.fix_points(&pa[1..]);
    s.constraints.push(Constraint::FixedX { p: pa[0], x: 3.0 });
    let cb = s.add_point(-2.0, 0.5);
    let b = s.next_entity_id();
    s.entities.push(Entity::Arc {
        id: b,
        center: cb,
        start: pa[2],
        end: pa[1],
    });
    let cs = s.symmetric_entities(a, b, line).unwrap();
    assert_eq!(cs.len(), 1);
    s.constraints.extend(cs);
    let r = s.solve();
    assert_eq!(
        (r.status, r.dof),
        (SolveStatus::FullyConstrained, 0),
        "{r:?}"
    );
    at(&s, cb, [-3.0, 0.0]);
}
