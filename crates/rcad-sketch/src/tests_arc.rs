//! Three-point arcs: construction, implied equation, Radius / Tangent
//! constraints, and profile extraction with arcs. Expected values are derived
//! by hand (circumcentres of chosen triangles, areas from elementary formulas)
//! or by an independent dense polygonization of the returned loop.
use super::*;
use crate::profile::{Prim, arc_arc_hits, line_arc_hits, loop_self_intersects, signed_area};
use std::f64::consts::{FRAC_PI_2, PI, TAU};

const EPS: f64 = 1e-9;
/// Solved coordinates are compared at 1e-8 mm (10x the residual tolerance; see
/// docs/TOLERANCES.md).
const SOLVED: f64 = 1e-8;

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn xy(s: &Sketch, p: PointId) -> [f64; 2] {
    s.point_xy(p).expect("point exists")
}

fn d(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn arc_ids(s: &Sketch, id: EntityId) -> (PointId, PointId, PointId) {
    match s.entity(id) {
        Some(Entity::Arc {
            center, start, end, ..
        }) => (*center, *start, *end),
        other => panic!("not an arc: {other:?}"),
    }
}

/// Area enclosed by a loop, by polygonizing each arc into `n` chords (an
/// estimate independent of the closed-form segment formula; error O(1/n²)).
fn sampled_area(segs: &[LoopSegment], n: usize) -> f64 {
    let k = segs.len();
    let mut pts = Vec::new();
    for i in 0..k {
        let p = segs[i].start();
        let q = segs[(i + 1) % k].start();
        match segs[i] {
            LoopSegment::Line { .. } => pts.push(p),
            LoopSegment::Arc { center: c, ccw, .. } => {
                let r = d(p, c);
                let a0 = (p[1] - c[1]).atan2(p[0] - c[0]);
                let a1 = (q[1] - c[1]).atan2(q[0] - c[0]);
                let mut sw = (a1 - a0).rem_euclid(TAU);
                if !ccw {
                    sw -= TAU;
                }
                for j in 0..n {
                    let t = a0 + sw * j as f64 / n as f64;
                    pts.push([c[0] + r * t.cos(), c[1] + r * t.sin()]);
                }
            }
            LoopSegment::Cubic { .. } => panic!("line/arc loops only"),
        }
    }
    let m = pts.len();
    0.5 * (0..m)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % m]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum::<f64>()
}

fn loop_of(s: &Sketch) -> Vec<LoopSegment> {
    match s.profile() {
        Ok(Profile::Loop(v)) => v,
        other => panic!("expected a Loop profile, got {other:?}"),
    }
}

fn exact_area(segs: &[LoopSegment]) -> f64 {
    let k = segs.len();
    let geo: Vec<Prim> = (0..k)
        .map(|i| Prim::of(&segs[i], segs[(i + 1) % k].start()))
        .collect();
    signed_area(&geo)
}

// ---------------------------------------------------------------- construction

#[test]
fn circumcentre_and_orientation() {
    // Unit-ish semicircle: centre (0,0), r 10. Via above: the CCW arc from
    // (10,0) to (-10,0) passes through (0,10).
    let (c, r, ccw) = arc_through_3_points([10.0, 0.0], [-10.0, 0.0], [0.0, 10.0]).unwrap();
    assert!(near(c[0], 0.0, EPS) && near(c[1], 0.0, EPS) && near(r, 10.0, EPS));
    assert!(ccw);
    // Via below: the CCW arc from (10,0) goes over the top, missing (0,-10).
    let (_, _, ccw) = arc_through_3_points([10.0, 0.0], [-10.0, 0.0], [0.0, -10.0]).unwrap();
    assert!(!ccw);
    // Right triangle (0,0), (6,0), (0,8): circumcentre = hypotenuse midpoint (3,4),
    // r = 5. Via (0,8) is left of (0,0)->(6,0); the CCW arc from (0,0) to (6,0)
    // about (3,4) runs below (angles -126.87° -> -53.13°), so it misses via.
    let (c, r, ccw) = arc_through_3_points([0.0, 0.0], [6.0, 0.0], [0.0, 8.0]).unwrap();
    assert!(near(c[0], 3.0, EPS) && near(c[1], 4.0, EPS) && near(r, 5.0, EPS));
    assert!(!ccw);
}

#[test]
fn add_arc_3pt_stores_ccw_and_swaps_for_clockwise_via() {
    let mut s = Sketch::default();
    let a = s.add_point(10.0, 0.0);
    let b = s.add_point(-10.0, 0.0);
    let up = s.add_arc_3pt(a, b, [0.0, 10.0]).unwrap();
    let (c, st, en) = arc_ids(&s, up);
    assert_eq!((st, en), (a, b));
    assert!(d(xy(&s, c), [0.0, 0.0]) <= EPS);
    let g = s.arc_geometry(up).unwrap();
    assert!(near(g.radius, 10.0, EPS) && near(g.start_angle, 0.0, EPS));
    assert!(near(g.sweep, PI, EPS));

    // Clockwise side: stored swapped so the CCW arc (-10,0) -> (10,0) passes (0,-10).
    let down = s.add_arc_3pt(a, b, [0.0, -10.0]).unwrap();
    let (_, st, en) = arc_ids(&s, down);
    assert_eq!((st, en), (b, a));
    let g = s.arc_geometry(down).unwrap();
    assert!(near(g.start_angle, PI, EPS) && near(g.sweep, PI, EPS));

    // Three-quarter arc: (10,0) -> (0,-10) via (-10,0); CCW sweep 270°.
    let e = s.add_point(0.0, -10.0);
    let big = s.add_arc_3pt(a, e, [-10.0, 0.0]).unwrap();
    let g = s.arc_geometry(big).unwrap();
    assert!(near(g.sweep, 1.5 * PI, EPS), "{}", g.sweep);
    assert_eq!(arc_ids(&s, big).1, a);
    s.validate().unwrap();
    // The implied equation holds for every arc at construction.
    assert!(s.residual().unwrap() <= 1e-12);
}

#[test]
fn add_arc_3pt_rejects_degenerate_input_and_leaves_sketch_unchanged() {
    let mut s = Sketch::default();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(10.0, 0.0);
    let before = s.clone();
    // Collinear (height 1e-12 mm below the 1e-9 mm tolerance).
    assert!(matches!(
        s.add_arc_3pt(a, b, [5.0, 1e-12]),
        Err(SketchError::DegenerateArc(_))
    ));
    // Exactly collinear, via beyond the end.
    assert!(s.add_arc_3pt(a, b, [20.0, 0.0]).is_err());
    // Via coincides with an endpoint.
    assert!(s.add_arc_3pt(a, b, [10.0, 0.0]).is_err());
    // Same start and end.
    assert!(s.add_arc_3pt(a, a, [5.0, 5.0]).is_err());
    // Missing point.
    assert!(matches!(
        s.add_arc_3pt(a, PointId(99), [5.0, 5.0]),
        Err(SketchError::MissingPoint(_))
    ));
    // Sagitta 1e-4 mm on a 10 mm chord: R = (25 + 1e-8)/(2e-4) ~ 125000 mm >
    // 1e4 x 10 mm -> rejected. Sagitta 1e-3: R ~ 12500 mm -> accepted.
    assert!(matches!(
        s.add_arc_3pt(a, b, [5.0, 1e-4]),
        Err(SketchError::DegenerateArc(_))
    ));
    assert_eq!(s, before);
    let ok = s.add_arc_3pt(a, b, [5.0, 1e-3]).unwrap();
    let r = s.arc_geometry(ok).unwrap().radius;
    assert!(near(r, (25.0 + 1e-6) / 2e-3, 1e-6), "{r}");
}

// ---------------------------------------------------------------- solving

/// Quarter arc about the origin from (10,0) to (0,10), centre coincident with
/// the fixed origin.
fn quarter() -> (Sketch, EntityId, PointId, PointId, PointId) {
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let a = s.add_point(10.0, 0.0);
    let b = s.add_point(0.0, 10.0);
    let arc = s
        .add_arc_3pt(a, b, [10.0 / 2f64.sqrt(), 10.0 / 2f64.sqrt()])
        .unwrap();
    let (c, _, _) = arc_ids(&s, arc);
    s.constraints.push(Constraint::Coincident { a: o, b: c });
    (s, arc, c, a, b)
}

#[test]
fn drag_keeps_the_implied_equal_radius_equation() {
    let (mut s, _, c, a, b) = quarter();
    let r = s.drag_point(a, [12.0, 3.0]);
    assert_eq!(r.outcome, DragOutcome::Reached, "{r:?}");
    let (pa, pb, pc) = (xy(&s, a), xy(&s, b), xy(&s, c));
    assert!(near(pa[0], 12.0, SOLVED) && near(pa[1], 3.0, SOLVED));
    // Centre pinned to the origin; the end point must follow to radius |(12,3)|.
    assert!(d(pc, [0.0, 0.0]) <= SOLVED);
    assert!(near(d(pb, pc), 153f64.sqrt(), SOLVED), "{}", d(pb, pc));
    // Without the implied equation the end point would not have moved.
    assert!(d(pb, [0.0, 10.0]) > 1.0);
}

#[test]
fn radius_dimension_drives_an_arc_and_a_circle() {
    let (mut s, arc, c, a, b) = quarter();
    s.constraints.push(Constraint::Radius {
        entity: arc,
        dim: Dimension::new("radius", 10.0),
    });
    assert_eq!(s.dimensions(), vec![("radius".to_string(), 10.0)]);
    s.set_dimension("radius", 15.0).unwrap();
    let rep = s.solve();
    assert!(rep.is_success(), "{rep:?}");
    let pc = xy(&s, c);
    assert!(near(d(xy(&s, a), pc), 15.0, SOLVED));
    assert!(near(d(xy(&s, b), pc), 15.0, SOLVED));
    assert!(near(s.arc_geometry(arc).unwrap().radius, 15.0, SOLVED));
    assert_eq!(
        s.constraint_label(s.constraints.len() - 1),
        format!("radius = 15 mm (radius of A{})", arc.0)
    );

    // Circle: the radius unknown itself.
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let circ = s.add_circle(o, 3.0);
    s.constraints.push(Constraint::Radius {
        entity: circ,
        dim: Dimension::new("r", 7.5),
    });
    let rep = s.solve();
    assert_eq!(rep.status, SolveStatus::FullyConstrained, "{rep:?}");
    assert!(
        matches!(s.entity(circ), Some(Entity::Circle { radius, .. }) if near(*radius, 7.5, SOLVED))
    );
    assert!(s.constraint_label(1).contains("radius of C"));
}

#[test]
fn radius_and_tangent_validation() {
    let (mut s, arc, _, a, _) = quarter();
    let p = s.add_point(10.0, -20.0);
    let line = s.add_line(p, a);
    // Radius on a line is the wrong kind.
    let mut bad = s.clone();
    bad.constraints.push(Constraint::Radius {
        entity: line,
        dim: Dimension::new("r", 1.0),
    });
    assert!(matches!(
        bad.validate(),
        Err(SketchError::WrongEntityKind { .. })
    ));
    // Tangent needs a shared endpoint.
    let q = s.add_point(30.0, 0.0);
    let far = s.add_line(p, q);
    let mut bad = s.clone();
    bad.constraints.push(Constraint::Tangent { line: far, arc });
    assert!(matches!(bad.validate(), Err(SketchError::InvalidValue(_))));
    // Arc with repeated points.
    let mut bad = s.clone();
    bad.entities.push(Entity::Arc {
        id: EntityId(99),
        center: a,
        start: a,
        end: p,
    });
    assert!(bad.validate().is_err());
    // A vertical line ending at (10,0) is tangent to the quarter arc there.
    s.constraints.push(Constraint::Tangent { line, arc });
    s.validate().unwrap();
    assert!(s.residual().unwrap() <= 1e-12);
    assert_eq!(
        s.constraint_label(s.constraints.len() - 1),
        format!("Tangent L{} · A{}", line.0, arc.0)
    );
}

#[test]
fn tangent_constraint_rotates_a_line_onto_the_tangent() {
    // Quarter arc about the fixed origin with a line from the arc start (10,0)
    // to (14,-3). Pinning the line's far end x = 10 is not enough; the tangent
    // equation forces the line vertical (radius vector at (10,0) is +x).
    let (mut s, arc, _, a, _) = quarter();
    let p = s.add_point(14.0, -3.0);
    let line = s.add_line(p, a);
    s.constraints.extend([
        Constraint::Tangent { line, arc },
        Constraint::Radius {
            entity: arc,
            dim: Dimension::new("radius", 10.0),
        },
        Constraint::VerticalDistance {
            a: p,
            b: a,
            dim: Dimension::new("len", 5.0),
        },
    ]);
    // Keep the arc start on the x axis: a horizontal helper line to the origin
    // would close a loop, so fix it directly.
    s.constraints.push(Constraint::FixedPoint {
        p: a,
        x: 10.0,
        y: 0.0,
    });
    let rep = s.solve();
    assert!(rep.is_success(), "{rep:?}");
    let pp = xy(&s, p);
    assert!(
        near(pp[0], 10.0, SOLVED) && near(pp[1], -5.0, SOLVED),
        "{pp:?}"
    );
}

#[test]
fn removing_an_arc_drops_its_centre_and_constraints_but_keeps_shared_points() {
    let mut s = Sketch::default();
    s.ensure_origin();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(10.0, 0.0);
    let line = s.add_line(a, b);
    let arc = s.add_arc_3pt(b, a, [5.0, 5.0]).unwrap();
    let (c, _, _) = arc_ids(&s, arc);
    s.constraints.push(Constraint::Radius {
        entity: arc,
        dim: Dimension::new("radius", 5.0),
    });
    assert!(s.point_used_by_entity(c));
    assert!(s.remove_entity(arc));
    assert!(s.point(c).is_none(), "centre removed with its arc");
    assert!(
        s.point(a).is_some() && s.point(b).is_some(),
        "line ends kept"
    );
    assert!(s.constraints.iter().all(|k| k.dimension().is_none()));
    assert!(s.entity(line).is_some());
    s.validate().unwrap();
}

#[test]
fn arc_sketch_json_round_trip() {
    let s = rounded_rect(80.0, 50.0, 8.0, false);
    let j = serde_json::to_string(&s).unwrap();
    let back: Sketch = serde_json::from_str(&j).unwrap();
    assert_eq!(s, back);
}

// ---------------------------------------------------------------- rounded rectangle

/// Rounded rectangle centred on the origin, built CCW like the editor would:
/// 4 lines + 4 three-point arcs sharing endpoints, H/V lines, 8 tangents, the
/// BL/TR arc centres symmetric about the origin, width/height between the
/// straight sides, and either one Radius dimension per corner (`per_corner`)
/// or one "radius" on BL plus three EqualRadius. 26 unknowns, 26 equations.
fn rounded_rect(w: f64, h: f64, r: f64, per_corner: bool) -> Sketch {
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let (x, y) = (w / 2.0, h / 2.0);
    let b0 = s.add_point(-x + r, -y);
    let b1 = s.add_point(x - r, -y);
    let r0 = s.add_point(x, -y + r);
    let r1 = s.add_point(x, y - r);
    let t0 = s.add_point(x - r, y);
    let t1 = s.add_point(-x + r, y);
    let l0 = s.add_point(-x, y - r);
    let l1 = s.add_point(-x, -y + r);
    let k = r * (1.0 - std::f64::consts::FRAC_1_SQRT_2);
    let bottom = s.add_line(b0, b1);
    let br = s.add_arc_3pt(b1, r0, [x - k, -y + k]).unwrap();
    let right = s.add_line(r0, r1);
    let tr = s.add_arc_3pt(r1, t0, [x - k, y - k]).unwrap();
    let top = s.add_line(t0, t1);
    let tl = s.add_arc_3pt(t1, l0, [-x + k, y - k]).unwrap();
    let left = s.add_line(l0, l1);
    let bl = s.add_arc_3pt(l1, b0, [-x + k, -y + k]).unwrap();
    let centre = |s: &Sketch, a| arc_ids(s, a).0;
    let (cbl, ctr) = (centre(&s, bl), centre(&s, tr));
    s.constraints.extend([
        Constraint::Horizontal { line: bottom },
        Constraint::Horizontal { line: top },
        Constraint::Vertical { line: right },
        Constraint::Vertical { line: left },
    ]);
    for (line, arc) in [
        (bottom, br),
        (right, br),
        (right, tr),
        (top, tr),
        (top, tl),
        (left, tl),
        (left, bl),
        (bottom, bl),
    ] {
        s.constraints.push(Constraint::Tangent { line, arc });
    }
    s.constraints.extend([
        Constraint::SymmetricAboutPoint {
            a: cbl,
            b: ctr,
            center: o,
        },
        Constraint::HorizontalDistance {
            a: l0,
            b: r0,
            dim: Dimension::new("width", w),
        },
        Constraint::VerticalDistance {
            a: b0,
            b: t0,
            dim: Dimension::new("height", h),
        },
    ]);
    if per_corner {
        for arc in [bl, br, tr, tl] {
            let name = s.unique_dimension_name("radius");
            s.constraints.push(Constraint::Radius {
                entity: arc,
                dim: Dimension::new(name, r),
            });
        }
    } else {
        s.constraints.push(Constraint::Radius {
            entity: bl,
            dim: Dimension::new("radius", r),
        });
        for b in [br, tr, tl] {
            s.constraints.push(Constraint::EqualRadius { a: bl, b });
        }
    }
    s.validate().unwrap();
    s
}

#[test]
fn rounded_rectangle_is_fully_constrained_with_hand_derived_area() {
    for per_corner in [false, true] {
        let mut s = rounded_rect(80.0, 50.0, 8.0, per_corner);
        let rep = s.solve();
        assert_eq!(rep.status, SolveStatus::FullyConstrained, "{rep:?}");
    }
    let mut s = rounded_rect(80.0, 50.0, 8.0, false);
    let rep = s.solve();
    assert_eq!(rep.status, SolveStatus::FullyConstrained, "{rep:?}");
    let segs = loop_of(&s);
    assert_eq!(segs.len(), 8);
    // Starts at the lowest-leftmost vertex: the bottom line's left end.
    assert!(d(segs[0].start(), [-32.0, -25.0]) <= SOLVED);
    assert!(matches!(segs[0], LoopSegment::Line { .. }));
    assert!(segs.iter().all(|g| match g {
        LoopSegment::Arc { ccw, .. } => *ccw,
        _ => true,
    }));
    // Rectangle minus four corner squares plus four quarter discs:
    // 80*50 - 4*8^2 + pi*8^2 = 4000 - (4 - pi)*64.
    let want = 4000.0 - (4.0 - PI) * 64.0;
    assert!(near(exact_area(&segs), want, 1e-9), "{}", exact_area(&segs));
    assert!(near(sampled_area(&segs, 4000), want, 1e-4));
}

#[test]
fn rounded_rectangle_radius_edit_regenerates_corners() {
    // One "radius" dimension drives all four corners through EqualRadius.
    let mut s = rounded_rect(80.0, 50.0, 8.0, false);
    s.set_dimension("radius", 12.0).unwrap();
    let rep = s.solve();
    assert_eq!(rep.status, SolveStatus::FullyConstrained, "{rep:?}");
    let segs = loop_of(&s);
    // Bottom line from (-40+12, -25) to (40-12, -25); BR centre (28, -13).
    assert!(d(segs[0].start(), [-28.0, -25.0]) <= SOLVED);
    assert!(d(segs[1].start(), [28.0, -25.0]) <= SOLVED);
    let LoopSegment::Arc { center, .. } = segs[1] else {
        panic!("BR arc")
    };
    assert!(d(center, [28.0, -13.0]) <= SOLVED);
    let want = 4000.0 - (4.0 - PI) * 144.0;
    assert!(near(exact_area(&segs), want, 1e-8));

    // A single corner: the outline stays 80 x 50 (BL r = 20). The BL/TR centres
    // stay symmetric about the origin, so the outline shifts: BL centre
    // (-26,-11), TR centre (26,11); x in [-46, 34], y in [-31, 19].
    let mut s = rounded_rect(80.0, 50.0, 8.0, true);
    s.set_dimension("radius", 20.0).unwrap();
    assert!(s.solve().is_success());
    let segs = loop_of(&s);
    let want = 4000.0 - (1.0 - PI / 4.0) * (3.0 * 64.0 + 400.0);
    assert!(near(exact_area(&segs), want, 1e-8));
    let xs: Vec<f64> = segs.iter().map(|g| g.start()[0]).collect();
    let ys: Vec<f64> = segs.iter().map(|g| g.start()[1]).collect();
    let span = |v: &[f64]| {
        (
            v.iter().copied().fold(f64::INFINITY, f64::min),
            v.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        )
    };
    let ((x0, x1), (y0, y1)) = (span(&xs), span(&ys));
    assert!(
        near(x0, -46.0, SOLVED) && near(x1, 34.0, SOLVED),
        "{x0} {x1}"
    );
    assert!(
        near(y0, -31.0, SOLVED) && near(y1, 19.0, SOLVED),
        "{y0} {y1}"
    );
}

#[test]
fn clockwise_drawn_loop_is_normalized_ccw() {
    // D shape drawn clockwise: the arc first from (-10,0) over the top to
    // (10,0), then the line back.
    let mut s = Sketch::default();
    let a = s.add_point(-10.0, 0.0);
    let b = s.add_point(10.0, 0.0);
    let arc = s.add_arc_3pt(a, b, [0.0, 10.0]).unwrap();
    // via above lies on the clockwise side of (-10,0)->(10,0): stored swapped.
    assert_eq!(arc_ids(&s, arc).1, b);
    s.add_line(b, a);
    let segs = loop_of(&s);
    assert_eq!(segs.len(), 2);
    // Half disc r = 10: area 50 pi, CCW: line (-10,0)->(10,0) then arc CCW.
    assert!(near(exact_area(&segs), 50.0 * PI, 1e-9));
    assert!(near(sampled_area(&segs, 4000), 50.0 * PI, 1e-3));
    assert!(matches!(segs[0], LoopSegment::Line { start } if d(start, [-10.0, 0.0]) <= EPS));
    assert!(
        matches!(segs[1], LoopSegment::Arc { start, ccw: true, .. } if d(start, [10.0, 0.0]) <= EPS)
    );

    // The lower half disc: arc stored CCW (-10,0) -> (10,0) through (0,-10)
    // plus the line (-10,0) -> (10,0); the walk direction does not matter.
    let mut s = Sketch::default();
    let a = s.add_point(-10.0, 0.0);
    let b = s.add_point(10.0, 0.0);
    s.add_arc_3pt(a, b, [0.0, -10.0]).unwrap();
    s.add_line(a, b);
    let segs = loop_of(&s);
    assert!(near(exact_area(&segs), 50.0 * PI, 1e-9));
}

#[test]
fn reversed_traversal_flips_arc_sense() {
    // Concave arc: square (0,0),(20,0),(20,20),(0,20) with the top edge replaced
    // by an arc dipping inward through (10,15). Drawn clockwise.
    let mut s = Sketch::default();
    let p = [
        s.add_point(0.0, 0.0),
        s.add_point(0.0, 20.0),
        s.add_point(20.0, 20.0),
        s.add_point(20.0, 0.0),
    ];
    s.add_line(p[0], p[1]);
    s.add_arc_3pt(p[1], p[2], [10.0, 15.0]).unwrap();
    s.add_line(p[2], p[3]);
    s.add_line(p[3], p[0]);
    let segs = loop_of(&s);
    // Circle through (0,20),(20,20),(10,15): centre (10, c) with
    // 100 + (20-c)^2 = (15-c)^2 -> c = 27.5, r = 12.5. Segment height 5,
    // half-angle t = asin(10/12.5); segment area = r^2 (t - sin t cos t).
    let r = 12.5_f64;
    let t = (10.0 / r).asin();
    let want = 400.0 - r * r * (t - t.sin() * t.cos());
    assert!(
        near(exact_area(&segs), want, 1e-9),
        "{} vs {want}",
        exact_area(&segs)
    );
    assert!(near(sampled_area(&segs, 4000), want, 1e-5));
    // CCW traversal goes (20,20) -> (0,20) along the arc: about centre (10,27.5)
    // (20,20) is at angle -36.9°, (0,20) at -143.1° and the arc passes (10,15)
    // at -90°: decreasing angle, so clockwise.
    let arc = segs
        .iter()
        .find(|g| matches!(g, LoopSegment::Arc { .. }))
        .unwrap();
    assert!(
        matches!(arc, LoopSegment::Arc { start, ccw: false, .. } if d(*start, [20.0, 20.0]) <= EPS)
    );
}

#[test]
fn slot_and_two_arc_circle_areas() {
    // Stadium: centres (+-30, 0), r 10: 60*20 + pi*100.
    let mut s = Sketch::default();
    let b0 = s.add_point(-30.0, -10.0);
    let b1 = s.add_point(30.0, -10.0);
    let t0 = s.add_point(30.0, 10.0);
    let t1 = s.add_point(-30.0, 10.0);
    s.add_line(b0, b1);
    s.add_arc_3pt(b1, t0, [40.0, 0.0]).unwrap();
    s.add_line(t0, t1);
    s.add_arc_3pt(t1, b0, [-40.0, 0.0]).unwrap();
    let segs = loop_of(&s);
    let want = 1200.0 + 100.0 * PI;
    assert!(near(exact_area(&segs), want, 1e-9));
    assert!(near(sampled_area(&segs, 4000), want, 1e-3));

    // Two complementary arcs on one circle: a full disc of area pi r^2.
    let mut s = Sketch::default();
    let a = s.add_point(5.0, 0.0);
    let b = s.add_point(-5.0, 0.0);
    s.add_arc_3pt(a, b, [0.0, 5.0]).unwrap();
    s.add_arc_3pt(b, a, [0.0, -5.0]).unwrap();
    let segs = loop_of(&s);
    assert!(near(exact_area(&segs), 25.0 * PI, 1e-9));
}

// ---------------------------------------------------------------- rejection

#[test]
fn open_arc_chain_and_dangling_arc_are_rejected() {
    // a(0,0) -> b(10,0) bulging down, b -> c(10,10) bulging right; the closing
    // diagonal c -> a stays clear of both.
    let mut s = Sketch::default();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(10.0, 0.0);
    let c = s.add_point(10.0, 10.0);
    s.add_arc_3pt(a, b, [5.0, -3.0]).unwrap();
    s.add_arc_3pt(b, c, [13.0, 5.0]).unwrap();
    let e = s.profile().unwrap_err();
    assert!(matches!(e, SketchError::UnsupportedProfile(_)), "{e}");
    assert!(e.to_string().contains("closed loop"), "{e}");
    // Closing it, then adding an extra arc on a vertex.
    s.add_line(c, a);
    assert!(s.profile().is_ok());
    let d0 = s.add_point(0.0, 30.0);
    s.add_arc_3pt(a, d0, [-10.0, 15.0]).unwrap();
    assert!(matches!(
        s.profile(),
        Err(SketchError::UnsupportedProfile(_))
    ));
}

#[test]
fn arc_mixed_with_circle_is_rejected() {
    let mut s = Sketch::default();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(10.0, 0.0);
    s.add_arc_3pt(a, b, [5.0, 5.0]).unwrap();
    s.add_line(b, a);
    let c = s.add_point(50.0, 50.0);
    s.add_circle(c, 2.0);
    let e = s.profile().unwrap_err();
    assert!(e.to_string().contains("arcs"), "{e}");
}

#[test]
fn degenerate_arcs_in_a_loop_are_rejected() {
    // Arc endpoints merged by a Coincident constraint: zero/full sweep.
    let mut s = Sketch::default();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(10.0, 0.0);
    let arc = s.add_arc_3pt(a, b, [5.0, 5.0]).unwrap();
    let mut t = s.clone();
    t.constraints.push(Constraint::Coincident { a, b });
    // Not solved yet (residual 10 mm) -> NotSolved; solve then extract.
    assert!(t.solve().is_success());
    assert!(matches!(
        t.profile(),
        Err(SketchError::DegenerateProfile(_))
    ));
    // An arc whose start and end are distinct points at the same location
    // (zero chord) inside an otherwise closed loop.
    let c = s.add_point(0.0, 0.0);
    s.entities.retain(|e| e.id() != arc);
    s.entities.push(Entity::Arc {
        id: EntityId(50),
        center: b,
        start: a,
        end: c,
    });
    s.add_line(c, b);
    s.add_line(b, a);
    let e = s.profile().unwrap_err();
    assert!(matches!(e, SketchError::DegenerateProfile(_)), "{e}");
}

/// Loop A(0,0) -> B(20,0) -> arc to C(0,10) through `via` -> A.
fn line_arc_loop(via: [f64; 2]) -> Sketch {
    let mut s = Sketch::default();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(20.0, 0.0);
    let c = s.add_point(0.0, 10.0);
    s.add_line(a, b);
    s.add_arc_3pt(b, c, via).unwrap();
    s.add_line(c, a);
    s
}

#[test]
fn line_arc_self_intersection_adjacent_and_near_miss() {
    // Via (10,-4) is below AB: the arc leaves B downward and must re-cross the
    // x axis (to reach C above it) between x = 0 and 10, i.e. on AB, which is
    // adjacent to the arc at B.
    let e = line_arc_loop([10.0, -4.0]).profile().unwrap_err();
    assert!(e.to_string().contains("self-intersecting"), "{e}");
    // Via (10,4) is 1 mm inside the chord BC (y = 10 - x/2 = 5 at x = 10): a
    // shallow inward bulge that stays above the x axis and right of x = 0.
    assert!(matches!(
        line_arc_loop([10.0, 4.0]).profile(),
        Ok(Profile::Loop(_))
    ));
}

#[test]
fn line_arc_self_intersection_non_adjacent() {
    // Square-ish loop whose arc (from (20,10) to (0,10)) dips below y = 0 through
    // (10,-5), crossing the bottom line (not adjacent to it).
    let build = |via: [f64; 2]| {
        let mut s = Sketch::default();
        let p = [
            s.add_point(0.0, 0.0),
            s.add_point(20.0, 0.0),
            s.add_point(20.0, 10.0),
            s.add_point(0.0, 10.0),
        ];
        s.add_line(p[0], p[1]);
        s.add_line(p[1], p[2]);
        s.add_arc_3pt(p[2], p[3], via).unwrap();
        s.add_line(p[3], p[0]);
        s
    };
    assert!(build([10.0, -5.0]).profile().is_err());
    // Touching: the arc's lowest point exactly on the bottom line.
    assert!(build([10.0, 0.0]).profile().is_err());
    // Near miss: 1e-3 mm above the bottom line.
    assert!(matches!(
        build([10.0, 1e-3]).profile(),
        Ok(Profile::Loop(_))
    ));
}

#[test]
fn arc_arc_self_intersection_non_adjacent() {
    // A(0,0) -> B(30,0) -> arc to C(30,40) bulging left to x = xr -> D(0,40) ->
    // arc to A bulging right to x = xl. The arcs cross when xr < xl.
    // Height 40 so each arc (sagitta 15 on a 40 mm chord) is less than a
    // semicircle and stays between its chord and its via point.
    let build = |xr: f64, xl: f64| {
        let mut s = Sketch::default();
        let a = s.add_point(0.0, 0.0);
        let b = s.add_point(30.0, 0.0);
        let c = s.add_point(30.0, 40.0);
        let dd = s.add_point(0.0, 40.0);
        s.add_line(a, b);
        s.add_arc_3pt(b, c, [xr, 20.0]).unwrap();
        s.add_line(c, dd);
        s.add_arc_3pt(dd, a, [xl, 20.0]).unwrap();
        s
    };
    assert!(build(14.999, 15.001).profile().is_err());
    assert!(build(15.0, 15.0).profile().is_err(), "touching");
    assert!(matches!(
        build(15.001, 14.999).profile(),
        Ok(Profile::Loop(_))
    ));
}

fn arc_prim(start: [f64; 2], center: [f64; 2], ccw: bool, end: [f64; 2]) -> Prim {
    Prim::of(&LoopSegment::Arc { start, center, ccw }, end)
}

fn rot(p: [f64; 2], c: [f64; 2], t: f64) -> [f64; 2] {
    let (x, y) = (p[0] - c[0], p[1] - c[1]);
    [
        c[0] + x * t.cos() - y * t.sin(),
        c[1] + x * t.sin() + y * t.cos(),
    ]
}

#[test]
fn arc_arc_adjacent_second_intersection() {
    // arc1: upper half of the circle |p| = 10, traversed (-10,0) -> V = (10,0)
    // clockwise. arc2 starts at V on the circle about (2,6) (r = 10). The circles
    // meet at V and at V mirrored in the line of centres: (-8,6), which is on
    // arc1. About (2,6), V is at -36.87° and (-8,6) at 180°: a CCW sweep of
    // 216.87° reaches it.
    let tol = 1e-9;
    let v = [10.0, 0.0];
    let arc1 = arc_prim([-10.0, 0.0], [0.0, 0.0], false, v);
    let c2 = [2.0, 6.0];
    let long = arc_prim(v, c2, true, rot(v, c2, 250f64.to_radians()));
    let short = arc_prim(v, c2, true, rot(v, c2, 200f64.to_radians()));
    assert!(arc_arc_hits(&arc1, &long, &[v], tol));
    assert!(!arc_arc_hits(&arc1, &short, &[v], tol));
    // Same circle, the second arc doubling back over the first.
    let back = arc_prim(v, [0.0, 0.0], true, rot(v, [0.0, 0.0], 0.5));
    assert!(arc_arc_hits(&arc1, &back, &[v], tol));
    // Same circle, continuing onward (lower half): touches only at V.
    let on = arc_prim(v, [0.0, 0.0], false, [0.0, -10.0]);
    assert!(!arc_arc_hits(&arc1, &on, &[v], tol));
}

#[test]
fn line_arc_tangent_joint_is_not_an_intersection() {
    // Line (0,-10) -> (10,-10) is tangent to the circle |p - (10,0)| = 10 at
    // V = (10,-10); the arc continues CCW a quarter turn to (20,0).
    let tol = 1e-9;
    let v = [10.0, -10.0];
    let arc = arc_prim(v, [10.0, 0.0], true, [20.0, 0.0]);
    assert!(!line_arc_hits([0.0, -10.0], v, &arc, &[v], tol));
    // A line from (25,-5) to V passes through the disc and crosses the arc
    // again. Second crossing t2 = -2 u.(V - c), u from V to
    // (25,-5): (15,5)/|..|, V - c = (0,-10): t2 = 100/sqrt(250) = 6.32 <= 15.8,
    // at (16,-8): angle about (10,0) = -53.1°, inside the quarter (-90°, 0°).
    assert!(line_arc_hits([25.0, -5.0], v, &arc, &[v], tol));
    let geo = vec![
        Prim::Seg {
            a: [0.0, 0.0],
            b: [1.0, 0.0],
        },
        arc_prim([1.0, 0.0], [0.5, 0.0], true, [0.0, 0.0]),
    ];
    assert!(!loop_self_intersects(&geo, tol), "D shape");
    let _ = FRAC_PI_2;
}

#[test]
fn equal_radius_validation_and_labels() {
    let (mut s, arc, _, _, _) = quarter();
    let p = s.add_point(30.0, 30.0);
    let circ = s.add_circle(p, 2.0);
    let line = s.add_line(p, s.origin.unwrap());
    let mut bad = s.clone();
    bad.constraints
        .push(Constraint::EqualRadius { a: arc, b: arc });
    assert!(matches!(bad.validate(), Err(SketchError::InvalidValue(_))));
    let mut bad = s.clone();
    bad.constraints
        .push(Constraint::EqualRadius { a: arc, b: line });
    assert!(matches!(
        bad.validate(),
        Err(SketchError::WrongEntityKind { .. })
    ));
    // Arc (r = 10) and circle (r = 2): the solve brings them to a common radius.
    s.constraints
        .push(Constraint::EqualRadius { a: arc, b: circ });
    let i = s.constraints.len() - 1;
    assert_eq!(
        s.constraint_label(i),
        format!("Equal radius A{} · C{}", arc.0, circ.0)
    );
    assert!(s.solve().is_success());
    let ra = s.arc_geometry(arc).unwrap().radius;
    let Some(Entity::Circle { radius: rc, .. }) = s.entity(circ) else {
        panic!()
    };
    assert!(near(ra, *rc, SOLVED), "{ra} {rc}");
    // Removing the circle drops the constraint.
    assert!(s.remove_entity(circ));
    assert!(
        !s.constraints
            .iter()
            .any(|c| matches!(c, Constraint::EqualRadius { .. }))
    );
}
