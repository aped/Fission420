//! Line relations (Parallel, Perpendicular, EqualLength, PointOnLine) and
//! construction geometry. Expected coordinates are derived by hand from
//! elementary geometry (vector sums, 3-4-5 triangles, similar triangles), not
//! from solver output.
use super::*;

/// Solved coordinates are compared at 1e-8 mm (10x the residual tolerance;
/// see docs/TOLERANCES.md).
const SOLVED: f64 = 1e-8;

fn xy(s: &Sketch, p: PointId) -> [f64; 2] {
    s.point_xy(p).expect("point exists")
}

fn at(s: &Sketch, p: PointId, want: [f64; 2]) {
    let got = xy(s, p);
    assert!(
        (got[0] - want[0]).abs() <= SOLVED && (got[1] - want[1]).abs() <= SOLVED,
        "{p:?}: got {got:?}, want {want:?}"
    );
}

fn fix(s: &mut Sketch, p: PointId, x: f64, y: f64) -> usize {
    s.constraints.push(Constraint::FixedPoint { p, x, y });
    s.constraints.len() - 1
}

fn push(s: &mut Sketch, c: Constraint) -> usize {
    s.constraints.push(c);
    s.constraints.len() - 1
}

fn hd(a: PointId, b: PointId, name: &str, v: f64) -> Constraint {
    Constraint::HorizontalDistance {
        a,
        b,
        dim: Dimension::new(name, v),
    }
}

fn vd(a: PointId, b: PointId, name: &str, v: f64) -> Constraint {
    Constraint::VerticalDistance {
        a,
        b,
        dim: Dimension::new(name, v),
    }
}

fn dist(a: PointId, b: PointId, name: &str, v: f64) -> Constraint {
    Constraint::Distance {
        a,
        b,
        dim: Dimension::new(name, v),
    }
}

/// Parallelogram P0 P1 P2 P3 with P0 fixed at the origin, P0->P1 horizontal of
/// width 40, P3 at (10, 20) by dimensions, and the two Parallel pairs. The only
/// parallelogram is P2 = P1 + (P3 − P0) = (50, 20).
#[test]
fn parallel_pairs_close_a_parallelogram() {
    let mut s = Sketch::default();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(38.0, 3.0);
    let p2 = s.add_point(46.0, 26.0);
    let p3 = s.add_point(12.0, 17.0);
    let l1 = s.add_line(p0, p1);
    let l2 = s.add_line(p1, p2);
    let l3 = s.add_line(p2, p3);
    let l4 = s.add_line(p3, p0);
    fix(&mut s, p0, 0.0, 0.0);
    push(&mut s, Constraint::Horizontal { line: l1 });
    push(&mut s, hd(p0, p1, "w", 40.0));
    push(&mut s, hd(p0, p3, "dx", 10.0));
    push(&mut s, vd(p0, p3, "h", 20.0));
    push(&mut s, Constraint::Parallel { a: l1, b: l3 });
    push(&mut s, Constraint::Parallel { a: l2, b: l4 });
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained, "{r:?}");
    assert_eq!(r.dof, 0);
    at(&s, p1, [40.0, 0.0]);
    at(&s, p2, [50.0, 20.0]);
    at(&s, p3, [10.0, 20.0]);
    // Area of the parallelogram = base × height = 40 × 20.
    match s.profile().unwrap() {
        Profile::Polygon(v) => {
            let a: f64 = (0..v.len())
                .map(|i| {
                    let (p, q) = (v[i], v[(i + 1) % v.len()]);
                    0.5 * (p[0] * q[1] - q[0] * p[1])
                })
                .sum();
            assert!((a - 800.0).abs() < 1e-6, "area {a}");
        }
        other => panic!("{other:?}"),
    }
}

/// Slanted parallel: P0 = (0,0) and P1 = (30, 40) fixed; P2 fixed at (0, 10);
/// L2 = P2->P3 parallel to L1 with length 25: P3 = P2 ± 25 (0.6, 0.8); the guess
/// picks the + branch, (15, 30).
#[test]
fn parallel_slanted_line_with_length() {
    let mut s = Sketch::default();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(30.0, 40.0);
    let p2 = s.add_point(0.0, 10.0);
    let p3 = s.add_point(18.0, 26.0);
    let l1 = s.add_line(p0, p1);
    let l2 = s.add_line(p2, p3);
    fix(&mut s, p0, 0.0, 0.0);
    fix(&mut s, p1, 30.0, 40.0);
    fix(&mut s, p2, 0.0, 10.0);
    push(&mut s, dist(p2, p3, "len", 25.0));
    push(&mut s, Constraint::Parallel { a: l1, b: l2 });
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained, "{r:?}");
    at(&s, p3, [15.0, 30.0]);
}

/// L1 = (0,0)->(30,40) fixed; L2 = P1->P2 perpendicular, length 25:
/// P2 = P1 ± 25 (−0.8, 0.6) = (10, 55) or (50, 25); the guess picks (10, 55).
#[test]
fn perpendicular_corner() {
    let mut s = Sketch::default();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(30.0, 40.0);
    let p2 = s.add_point(12.0, 52.0);
    let l1 = s.add_line(p0, p1);
    let l2 = s.add_line(p1, p2);
    fix(&mut s, p0, 0.0, 0.0);
    fix(&mut s, p1, 30.0, 40.0);
    push(&mut s, dist(p1, p2, "len", 25.0));
    push(&mut s, Constraint::Perpendicular { a: l1, b: l2 });
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained, "{r:?}");
    at(&s, p2, [10.0, 55.0]);
    let (u, v) = (xy(&s, p1), xy(&s, p2));
    let dot = 30.0 * (v[0] - u[0]) + 40.0 * (v[1] - u[1]);
    assert!(dot.abs() < 1e-6, "dot {dot}");
}

/// L1 horizontal, width 30, from the fixed origin; L2 vertical from P1 with
/// EqualLength: P2 = (30, ±30), the guess picks (30, 30).
#[test]
fn equal_length_makes_an_l_with_equal_legs() {
    let mut s = Sketch::default();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(28.0, 1.0);
    let p2 = s.add_point(31.0, 25.0);
    let l1 = s.add_line(p0, p1);
    let l2 = s.add_line(p1, p2);
    fix(&mut s, p0, 0.0, 0.0);
    push(&mut s, Constraint::Horizontal { line: l1 });
    push(&mut s, hd(p0, p1, "w", 30.0));
    push(&mut s, Constraint::Vertical { line: l2 });
    push(&mut s, Constraint::EqualLength { a: l1, b: l2 });
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained, "{r:?}");
    at(&s, p1, [30.0, 0.0]);
    at(&s, p2, [30.0, 30.0]);
    // Driving the width drives both legs.
    s.set_dimension("w", 12.5).unwrap();
    assert!(s.solve().is_success());
    at(&s, p2, [12.5, 12.5]);
}

/// L1 = (0,0)->(40,20) fixed; P2 on its infinite line with x = 10 gives
/// y = 10 × 20/40 = 5; with x = 60 (beyond the segment end) y = 30.
#[test]
fn point_on_line_lies_on_the_infinite_line() {
    for (dx, want) in [(10.0, [10.0, 5.0]), (60.0, [60.0, 30.0])] {
        let mut s = Sketch::default();
        let p0 = s.add_point(0.0, 0.0);
        let p1 = s.add_point(40.0, 20.0);
        let p2 = s.add_point(dx + 3.0, -4.0);
        let l1 = s.add_line(p0, p1);
        fix(&mut s, p0, 0.0, 0.0);
        fix(&mut s, p1, 40.0, 20.0);
        push(&mut s, hd(p0, p2, "x", dx));
        push(
            &mut s,
            Constraint::PointOnLine {
                point: p2,
                line: l1,
            },
        );
        let r = s.solve();
        assert_eq!(r.status, SolveStatus::FullyConstrained, "{r:?}");
        at(&s, p2, want);
    }
}

/// A point on a free line with its own endpoints driven: the line follows.
/// P0 fixed (0,0), P1 free; P2 fixed at (20, 15) and on the line; P1 at
/// horizontal distance 40 → P1 = (40, 30) (the line through the origin and
/// (20, 15) has slope 3/4).
#[test]
fn point_on_line_drives_the_line() {
    let mut s = Sketch::default();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(40.0, 10.0);
    let p2 = s.add_point(20.0, 15.0);
    let l1 = s.add_line(p0, p1);
    fix(&mut s, p0, 0.0, 0.0);
    fix(&mut s, p2, 20.0, 15.0);
    push(&mut s, hd(p0, p1, "x", 40.0));
    push(
        &mut s,
        Constraint::PointOnLine {
            point: p2,
            line: l1,
        },
    );
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained, "{r:?}");
    at(&s, p1, [40.0, 30.0]);
}

#[test]
fn validation_of_line_relations() {
    let mut base = Sketch::default();
    let p0 = base.add_point(0.0, 0.0);
    let p1 = base.add_point(10.0, 0.0);
    let p2 = base.add_point(0.0, 10.0);
    let l1 = base.add_line(p0, p1);
    let l2 = base.add_line(p0, p2);
    let circ = base.add_circle(p2, 3.0);
    let with = |c: Constraint| {
        let mut s = base.clone();
        s.constraints.push(c);
        s.validate()
    };
    assert!(with(Constraint::Parallel { a: l1, b: l2 }).is_ok());
    for c in [
        Constraint::Parallel { a: l1, b: l1 },
        Constraint::Perpendicular { a: l2, b: l2 },
        Constraint::EqualLength { a: l1, b: l1 },
    ] {
        assert!(matches!(with(c), Err(SketchError::InvalidValue(_))));
    }
    assert!(matches!(
        with(Constraint::Perpendicular { a: l1, b: circ }),
        Err(SketchError::WrongEntityKind { entity, .. }) if entity == circ
    ));
    assert!(matches!(
        with(Constraint::EqualLength {
            a: l1,
            b: EntityId(99)
        }),
        Err(SketchError::MissingEntity(EntityId(99)))
    ));
    // Point on line: the point may not be the line's own endpoint.
    match with(Constraint::PointOnLine {
        point: p1,
        line: l1,
    }) {
        Err(SketchError::InvalidValue(m)) => assert!(m.contains("endpoint"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(
        with(Constraint::PointOnLine {
            point: p2,
            line: l1
        })
        .is_ok()
    );
    assert!(matches!(
        with(Constraint::PointOnLine {
            point: PointId(77),
            line: l1
        }),
        Err(SketchError::MissingPoint(PointId(77)))
    ));
    assert!(matches!(
        with(Constraint::PointOnLine {
            point: p1,
            line: circ
        }),
        Err(SketchError::WrongEntityKind { .. })
    ));
    // An invalid sketch never solves.
    let mut s = base.clone();
    s.constraints.push(Constraint::Parallel { a: l1, b: l1 });
    assert_eq!(s.solve().status, SolveStatus::Invalid);
}

/// Parallel and Perpendicular on the same pair of lines with fixed lengths
/// cannot both hold: Inconsistent, naming exactly those two.
#[test]
fn parallel_and_perpendicular_on_the_same_pair_is_inconsistent() {
    let mut s = Sketch::default();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(40.0, 0.0);
    let p2 = s.add_point(0.0, 10.0);
    let p3 = s.add_point(15.0, 22.0);
    let l1 = s.add_line(p0, p1);
    let l2 = s.add_line(p2, p3);
    fix(&mut s, p0, 0.0, 0.0);
    fix(&mut s, p1, 40.0, 0.0);
    fix(&mut s, p2, 0.0, 10.0);
    push(&mut s, dist(p2, p3, "len", 20.0));
    let ip = push(&mut s, Constraint::Parallel { a: l1, b: l2 });
    let iq = push(&mut s, Constraint::Perpendicular { a: l1, b: l2 });
    let before = s.clone();
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::Inconsistent, "{r:?}");
    assert_eq!(r.conflicting, vec![ip, iq]);
    assert_eq!(
        s, before,
        "an inconsistent solve leaves the sketch unchanged"
    );
    let labels: Vec<String> = r
        .conflicting
        .iter()
        .map(|&i| s.constraint_label(i))
        .collect();
    assert_eq!(labels, ["Parallel L1, L2", "Perpendicular L1, L2"]);
}

/// The template rectangle's bottom/top lines are already Horizontal; adding
/// Parallel between them is a consistent redundancy naming all three.
#[test]
fn parallel_on_two_horizontal_lines_is_redundant() {
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    // Constraints 1 and 2 are Horizontal L1 / L3 (see `centered_rectangle`).
    let ip = push(
        &mut s,
        Constraint::Parallel {
            a: EntityId(1),
            b: EntityId(3),
        },
    );
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::OverConstrained, "{r:?}");
    assert_eq!(r.conflicting, vec![1, 2, ip]);
    assert_eq!(r.dof, 0);
    at(&s, PointId(3), [40.0, 25.0]);

    // Perpendicular between a Horizontal and a Vertical line: also redundant.
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    let iq = push(
        &mut s,
        Constraint::Perpendicular {
            a: EntityId(1),
            b: EntityId(2),
        },
    );
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::OverConstrained, "{r:?}");
    // Horizontal L1 (1), Vertical L2 (3) and the Perpendicular.
    assert_eq!(r.conflicting, vec![1, 3, iq]);
}

/// Replacing the rectangle's second Horizontal by Parallel keeps it fully
/// constrained (the relation carries exactly one equation).
#[test]
fn parallel_replaces_a_horizontal() {
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    s.constraints[2] = Constraint::Parallel {
        a: EntityId(1),
        b: EntityId(3),
    };
    for p in &mut s.points {
        p.x += 1.3;
        p.y -= 0.7 * p.x / 40.0;
    }
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained, "{r:?}");
    at(&s, PointId(4), [-40.0, 25.0]);
}

/// Drags respect the new relations.
#[test]
fn drags_respect_parallel_and_point_on_line() {
    let mut s = Sketch::default();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(40.0, 0.0);
    let p2 = s.add_point(0.0, 10.0);
    let p3 = s.add_point(40.0, 10.0);
    let p4 = s.add_point(20.0, 0.0);
    let l1 = s.add_line(p0, p1);
    let l2 = s.add_line(p2, p3);
    fix(&mut s, p0, 0.0, 0.0);
    fix(&mut s, p1, 40.0, 0.0);
    push(&mut s, Constraint::Parallel { a: l1, b: l2 });
    push(
        &mut s,
        Constraint::PointOnLine {
            point: p4,
            line: l1,
        },
    );
    assert!(s.solve().is_success());

    // L2 must stay horizontal while its end moves up.
    let r = s.drag_point(p3, [40.0, 20.0]);
    assert_eq!(r.outcome, DragOutcome::Reached, "{r:?}");
    at(&s, p3, [40.0, 20.0]);
    let (a, b) = (xy(&s, p2), xy(&s, p3));
    assert!(
        (a[1] - b[1]).abs() <= SOLVED,
        "L2 not horizontal: {a:?} {b:?}"
    );

    // P4 slides along the fixed line y = 0.
    let r = s.drag_point(p4, [10.0, 5.0]);
    assert_eq!(r.outcome, DragOutcome::Partial, "{r:?}");
    at(&s, p4, [10.0, 0.0]);
}

#[test]
fn labels_use_plain_words() {
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let p1 = s.add_point(10.0, 0.0);
    let p2 = s.add_point(0.0, 10.0);
    let p5 = s.add_point(3.0, 3.0);
    let l1 = s.add_line(o, p1);
    let l2 = s.add_line(o, p2);
    let i = s.constraints.len();
    s.constraints.extend([
        Constraint::Parallel { a: l1, b: l2 },
        Constraint::Perpendicular { a: l1, b: l2 },
        Constraint::EqualLength { a: l1, b: l2 },
        Constraint::PointOnLine {
            point: p5,
            line: l2,
        },
        Constraint::PointOnLine { point: o, line: l2 },
    ]);
    let labels: Vec<String> = (i..i + 5).map(|k| s.constraint_label(k)).collect();
    assert_eq!(
        labels,
        [
            "Parallel L1, L2",
            "Perpendicular L1, L2",
            "Equal length L1, L2",
            "P3 on L2",
            "origin on L2",
        ]
    );
    assert!(labels.iter().all(|l| l.is_ascii()));
}

#[test]
fn removing_a_line_drops_its_relations_and_construction_flag() {
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    let n0 = s.constraints.len();
    // Standalone point on L2, and a separate segment whose end is on L4.
    let p5 = s.add_point(40.0, 0.0);
    let p6 = s.add_point(-40.0, 10.0);
    let p7 = s.add_point(-60.0, 10.0);
    let l5 = s.add_line(p6, p7);
    s.constraints.extend([
        Constraint::Parallel {
            a: EntityId(1),
            b: EntityId(3),
        },
        Constraint::EqualLength {
            a: EntityId(2),
            b: EntityId(4),
        },
        Constraint::PointOnLine {
            point: p5,
            line: EntityId(2),
        },
        Constraint::PointOnLine {
            point: p6,
            line: EntityId(4),
        },
    ]);
    assert!(s.set_construction(EntityId(3), true));
    assert!(s.set_construction(l5, true));
    s.validate().unwrap();

    assert!(s.remove_entity(EntityId(3)));
    assert!(!s.is_construction(EntityId(3)));
    assert!(s.is_construction(l5));
    assert!(
        !s.constraints
            .iter()
            .any(|c| matches!(c, Constraint::Parallel { .. }))
    );
    assert_eq!(
        s.constraints.len(),
        n0 - 1 + 3,
        "Horizontal L3 and Parallel go"
    );

    // Removing L5 drops its unused endpoints and so the PointOnLine on P6.
    assert!(s.remove_entity(l5));
    assert!(s.construction.is_empty());
    assert!(s.point(p6).is_none());
    assert!(
        !s.constraints
            .iter()
            .any(|c| matches!(c, Constraint::PointOnLine { point, .. } if *point == p6))
    );

    // Removing L2 drops EqualLength and the PointOnLine on it; P5 (standalone)
    // is kept.
    assert!(s.remove_entity(EntityId(2)));
    assert!(!s.constraints.iter().any(|c| matches!(
        c,
        Constraint::EqualLength { .. } | Constraint::PointOnLine { .. }
    )));
    assert!(s.point(p5).is_some());
    s.validate().unwrap();
}

#[test]
fn construction_flag_api_and_validation() {
    let mut s = Sketch::centered_rectangle(10.0, 10.0).unwrap();
    assert!(!s.is_construction(EntityId(1)));
    assert!(s.set_construction(EntityId(1), true));
    assert!(s.is_construction(EntityId(1)));
    assert!(s.set_construction(EntityId(1), true), "idempotent");
    assert!(s.set_construction(EntityId(1), false));
    assert!(!s.is_construction(EntityId(1)));
    assert!(!s.set_construction(EntityId(42), true));
    assert!(s.construction.is_empty(), "missing id not inserted");
    s.construction.insert(EntityId(42));
    assert_eq!(s.validate(), Err(SketchError::MissingEntity(EntityId(42))));
    assert_eq!(s.solve().status, SolveStatus::Invalid);
}

fn shoelace(v: &[[f64; 2]]) -> f64 {
    0.5 * (0..v.len())
        .map(|i| {
            let (p, q) = (v[i], v[(i + 1) % v.len()]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum::<f64>()
}

/// A rectangle plus a construction diagonal and a construction circle: the
/// profile is still the 80 × 50 rectangle (area 4000); without the flag the
/// extra curves break it.
#[test]
fn profile_ignores_construction_geometry() {
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    assert!(s.solve().is_success());
    let want = s.profile().unwrap();
    let diag = s.add_line(PointId(1), PointId(3));
    let circ = s.add_circle(PointId(0), 10.0);
    // Construction entities still take part in solving.
    s.constraints.push(Constraint::Diameter {
        circle: circ,
        dim: Dimension::new("ref", 30.0),
    });
    assert!(s.profile().is_err(), "extra curves break a normal profile");
    s.set_construction(diag, true);
    s.set_construction(circ, true);
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained, "{r:?}");
    match s.entity(circ) {
        Some(Entity::Circle { radius, .. }) => assert!((radius - 15.0).abs() <= SOLVED),
        other => panic!("{other:?}"),
    }
    let got = s.profile().unwrap();
    assert_eq!(got, want);
    match got {
        Profile::Polygon(v) => {
            assert_eq!(v.len(), 4);
            assert!((shoelace(&v) - 4000.0).abs() < 1e-6);
        }
        other => panic!("{other:?}"),
    }
}

/// Only construction geometry left (the circle alone would be a profile if it
/// were normal geometry).
#[test]
fn construction_only_sketch_has_no_profile() {
    let mut s = Sketch::centered_circle(20.0).unwrap();
    assert!(matches!(s.profile(), Ok(Profile::Circle { .. })));
    s.set_construction(EntityId(1), true);
    assert!(matches!(
        s.profile(),
        Err(SketchError::UnsupportedProfile(_))
    ));
}

/// A loop whose closing edge is construction is an open profile.
#[test]
fn loop_closed_only_by_a_construction_edge_is_open() {
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    assert!(s.profile().is_ok());
    s.set_construction(EntityId(4), true);
    assert!(matches!(
        s.profile(),
        Err(SketchError::UnsupportedProfile(_))
    ));
}

#[test]
fn serde_round_trip_with_and_without_construction() {
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    s.constraints.push(Constraint::Parallel {
        a: EntityId(1),
        b: EntityId(3),
    });
    let plain = serde_json::to_string(&s).unwrap();
    assert!(!plain.contains("construction"), "{plain}");
    assert_eq!(serde_json::from_str::<Sketch>(&plain).unwrap(), s);

    s.set_construction(EntityId(2), true);
    s.set_construction(EntityId(4), true);
    let json = serde_json::to_string(&s).unwrap();
    assert!(json.contains(r#""construction":[2,4]"#), "{json}");
    let back: Sketch = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s);
    assert!(back.is_construction(EntityId(4)));
}

/// Hand-written JSON: a v2 sketch without the new fields still parses, and the
/// new constraints have the documented external shape.
#[test]
fn json_shapes_of_new_fields() {
    let v2 = r#"{"points":[{"id":0,"x":0.0,"y":0.0},{"id":1,"x":5.0,"y":0.0},
        {"id":2,"x":0.0,"y":5.0},{"id":3,"x":1.0,"y":1.0}],
        "entities":[{"Line":{"id":1,"a":0,"b":1}},{"Line":{"id":2,"a":0,"b":2}}],
        "constraints":[{"FixedPoint":{"p":0,"x":0.0,"y":0.0}}],"origin":0}"#;
    let s: Sketch = serde_json::from_str(v2).unwrap();
    assert!(s.construction.is_empty());
    assert_eq!(s.origin, Some(PointId(0)));
    s.validate().unwrap();

    let with_new = r#"{"points":[{"id":0,"x":0.0,"y":0.0},{"id":1,"x":5.0,"y":0.0},
        {"id":2,"x":0.0,"y":5.0},{"id":3,"x":1.0,"y":1.0}],
        "entities":[{"Line":{"id":1,"a":0,"b":1}},{"Line":{"id":2,"a":0,"b":2}}],
        "constraints":[{"Parallel":{"a":1,"b":2}},{"Perpendicular":{"a":1,"b":2}},
          {"EqualLength":{"a":1,"b":2}},{"PointOnLine":{"point":3,"line":1}}],
        "construction":[2]}"#;
    let s: Sketch = serde_json::from_str(with_new).unwrap();
    assert_eq!(
        s.constraints,
        vec![
            Constraint::Parallel {
                a: EntityId(1),
                b: EntityId(2)
            },
            Constraint::Perpendicular {
                a: EntityId(1),
                b: EntityId(2)
            },
            Constraint::EqualLength {
                a: EntityId(1),
                b: EntityId(2)
            },
            Constraint::PointOnLine {
                point: PointId(3),
                line: EntityId(1)
            },
        ]
    );
    assert!(s.is_construction(EntityId(2)));
    s.validate().unwrap();
}
