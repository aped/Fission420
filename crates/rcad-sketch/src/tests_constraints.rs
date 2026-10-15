//! Fix, Concentric, Collinear and Symmetric. Expected positions come from
//! the construction (reflection about a line through (5, 0) at 30°, shared
//! centres, a common line), DOF counts from equations per constraint.
use super::*;

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
    s.constraints.push(Constraint::Symmetric { a, b, line });
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
