//! Tests for the editor API (edit.rs), the Distance constraint, and profile
//! validation of editor-built line loops. Expected values are derived by hand.
use super::*;

const EPS: f64 = 1e-9;

fn xy(s: &Sketch, p: PointId) -> [f64; 2] {
    s.point_xy(p).expect("point exists")
}

fn close(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() <= EPS && (a[1] - b[1]).abs() <= EPS
}

/// A centred rectangle built the way the interactive editor builds one: origin,
/// four corners (BL, BR, TR, TL), H/V constraints, symmetric about `center`,
/// optional width/height dimensions.
struct Rect {
    corners: [PointId; 4],
    lines: [EntityId; 4],
}

fn editor_rect(s: &mut Sketch, center: PointId, w: f64, h: f64, dims: bool) -> Rect {
    let [cx, cy] = xy(s, center);
    let c = [
        s.add_point(cx - w / 2.0, cy - h / 2.0),
        s.add_point(cx + w / 2.0, cy - h / 2.0),
        s.add_point(cx + w / 2.0, cy + h / 2.0),
        s.add_point(cx - w / 2.0, cy + h / 2.0),
    ];
    let l = [
        s.add_line(c[0], c[1]),
        s.add_line(c[1], c[2]),
        s.add_line(c[2], c[3]),
        s.add_line(c[3], c[0]),
    ];
    s.constraints.extend([
        Constraint::Horizontal { line: l[0] },
        Constraint::Horizontal { line: l[2] },
        Constraint::Vertical { line: l[1] },
        Constraint::Vertical { line: l[3] },
        Constraint::SymmetricAboutPoint {
            a: c[0],
            b: c[2],
            center,
        },
    ]);
    if dims {
        let wn = s.unique_dimension_name("width");
        let hn = s.unique_dimension_name("height");
        s.constraints.push(Constraint::HorizontalDistance {
            a: c[0],
            b: c[1],
            dim: Dimension::new(wn, w),
        });
        s.constraints.push(Constraint::VerticalDistance {
            a: c[0],
            b: c[3],
            dim: Dimension::new(hn, h),
        });
    }
    Rect {
        corners: c,
        lines: l,
    }
}

// ---------------------------------------------------------------- Distance

#[test]
fn distance_constraint_solves_3_4_5_triangle() {
    // A = origin, AB horizontal with |AB| = 4, BC vertical, |AC| = 5
    // => |BC| = sqrt(5^2 - 4^2) = 3 (Pythagoras), C = (4, 3) for the given seed.
    let mut s = Sketch::default();
    let a = s.ensure_origin();
    let b = s.add_point(3.5, 0.4);
    let c = s.add_point(4.3, 2.4);
    let ab = s.add_line(a, b);
    let bc = s.add_line(b, c);
    s.add_line(c, a);
    s.constraints.extend([
        Constraint::Horizontal { line: ab },
        Constraint::Vertical { line: bc },
        Constraint::Distance {
            a,
            b,
            dim: Dimension::new("ab", 4.0),
        },
        Constraint::Distance {
            a,
            b: c,
            dim: Dimension::new("ac", 5.0),
        },
    ]);
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained, "{r:?}");
    assert_eq!(r.dof, 0);
    // Convergence is judged on equation residuals (<= 1e-9 mm here). Through the
    // nonlinear rows (|d|AC|/dy| = 0.6) coordinates may differ by a few residuals,
    // so compare coordinates at 10x the residual tolerance.
    let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs().max((a[1] - b[1]).abs()) <= 1e-8;
    assert!(near(xy(&s, b), [4.0, 0.0]), "{:?}", xy(&s, b));
    assert!(near(xy(&s, c), [4.0, 3.0]), "{:?}", xy(&s, c));
    let p = s.profile().unwrap();
    let Profile::Polygon(v) = p else {
        panic!("polygon expected")
    };
    // Shoelace by hand: right triangle legs 4 and 3 => area 6.
    let area: f64 = 0.5
        * (0..v.len())
            .map(|i| {
                let (p, q) = (v[i], v[(i + 1) % v.len()]);
                p[0] * q[1] - q[0] * p[1]
            })
            .sum::<f64>();
    assert!((area - 6.0).abs() < 1e-7, "{area}");

    // A consistent redundant |BC| = 3 is OverConstrained; |BC| = 3.5 conflicts.
    let mut red = s.clone();
    red.constraints.push(Constraint::Distance {
        a: b,
        b: c,
        dim: Dimension::new("bc", 3.0),
    });
    assert_eq!(red.solve().status, SolveStatus::OverConstrained);
    let mut bad = s.clone();
    bad.constraints.push(Constraint::Distance {
        a: b,
        b: c,
        dim: Dimension::new("bc", 3.5),
    });
    let before = bad.clone();
    let r = bad.solve();
    assert_eq!(r.status, SolveStatus::Inconsistent, "{r:?}");
    assert_eq!(bad, before, "failed solve must not write back");
    assert!(!r.conflicting.is_empty());
}

#[test]
fn distance_from_zero_length_start_separates_points() {
    let mut s = Sketch::default();
    let a = s.ensure_origin();
    let b = s.add_point(0.0, 0.0);
    s.constraints.push(Constraint::Distance {
        a,
        b,
        dim: Dimension::new("d", 5.0),
    });
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::UnderConstrained, "{r:?}");
    assert_eq!(r.dof, 1, "point on a circle of radius 5 about the origin");
    let [x, y] = xy(&s, b);
    assert!((x.hypot(y) - 5.0).abs() < 1e-9, "({x}, {y})");
}

#[test]
fn distance_rejects_non_positive_value() {
    let mut s = Sketch::default();
    let a = s.ensure_origin();
    let b = s.add_point(1.0, 0.0);
    s.constraints.push(Constraint::Distance {
        a,
        b,
        dim: Dimension::new("d", 0.0),
    });
    assert!(matches!(
        s.validate(),
        Err(SketchError::InvalidDimension { .. })
    ));
    assert_eq!(s.solve().status, SolveStatus::Invalid);
}

// ---------------------------------------------------------------- origin, ids

#[test]
fn ensure_origin_is_fixed_and_idempotent() {
    let mut s = Sketch::default();
    assert_eq!(s.next_point_id(), PointId(0));
    assert_eq!(s.next_entity_id(), EntityId(1));
    let o = s.ensure_origin();
    assert_eq!(s.origin, Some(o));
    assert_eq!(xy(&s, o), [0.0, 0.0]);
    assert_eq!(
        s.constraints,
        vec![Constraint::FixedPoint {
            p: o,
            x: 0.0,
            y: 0.0
        }]
    );
    assert_eq!(s.ensure_origin(), o);
    assert_eq!(s.points.len(), 1);
    assert_eq!(s.constraints.len(), 1);
    // The origin cannot be dragged away.
    let r = s.drag_point(o, [5.0, 5.0]);
    assert_eq!(r.outcome, DragOutcome::Blocked);
    assert_eq!(r.blocked_by, vec![0]);
    assert_eq!(xy(&s, o), [0.0, 0.0]);
    // A v1 sketch (no "origin" key) deserializes with origin = None, and a
    // sketch without origin serializes without the key.
    let v1: Sketch =
        serde_json::from_str(r#"{"points":[],"entities":[],"constraints":[]}"#).unwrap();
    assert_eq!(v1.origin, None);
    assert!(!serde_json::to_string(&v1).unwrap().contains("origin"));
    let back: Sketch = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back, s);
}

#[test]
fn ids_are_never_reused_after_append() {
    let mut s = Sketch::default();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(1.0, 0.0);
    let l = s.add_line(p0, p1);
    assert_eq!((p0, p1, l), (PointId(0), PointId(1), EntityId(1)));
    assert_eq!(s.next_point_id(), PointId(2));
    assert_eq!(s.next_entity_id(), EntityId(2));
}

// ---------------------------------------------------------------- remove_entity

#[test]
fn removing_a_line_keeps_construction_centres_that_are_still_used() {
    let mut s = Sketch::default();
    s.ensure_origin();
    // Two rectangles with their own construction centres (not the origin).
    let c1 = s.add_point(-30.0, 0.0);
    let c2 = s.add_point(30.0, 0.0);
    let r1 = editor_rect(&mut s, c1, 20.0, 10.0, true);
    let r2 = editor_rect(&mut s, c2, 20.0, 10.0, true);
    assert!(s.remove_entity(r1.lines[0]));
    assert!(
        s.point(c1).is_some(),
        "rect 1 centre still constrains corners"
    );
    assert!(s.point(c2).is_some(), "rect 2 centre untouched");
    for p in r1.corners.iter().chain(r2.corners.iter()) {
        assert!(s.point(*p).is_some(), "corners are still line endpoints");
    }
    // The Horizontal constraint on the removed line is gone, the rest stay.
    assert!(
        !s.constraints
            .contains(&Constraint::Horizontal { line: r1.lines[0] })
    );
    assert!(s.constraints.contains(&Constraint::SymmetricAboutPoint {
        a: r1.corners[0],
        b: r1.corners[2],
        center: c1
    }));
    assert!(s.validate().is_ok());
    assert!(!s.remove_entity(r1.lines[0]), "already removed");
}

#[test]
fn removing_all_lines_of_a_rectangle_drops_its_centre_and_constraints() {
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let lone = s.add_point(99.0, 99.0); // standalone user point: unrelated
    let c1 = s.add_point(-30.0, 0.0);
    // The centre is tied to the origin (that link alone must not keep it alive).
    s.constraints.push(Constraint::HorizontalDistance {
        a: o,
        b: c1,
        dim: Dimension::new("offset", 30.0),
    });
    let c2 = s.add_point(30.0, 0.0);
    let r1 = editor_rect(&mut s, c1, 20.0, 10.0, true);
    let r2 = editor_rect(&mut s, c2, 20.0, 10.0, true);
    for l in r1.lines {
        assert!(s.remove_entity(l));
    }
    assert!(s.point(c1).is_none(), "construction centre dropped");
    for p in r1.corners {
        assert!(s.point(p).is_none());
    }
    assert!(
        !s.constraints
            .iter()
            .any(|c| edit::constraint_points(c).contains(&c1)),
        "symmetric + offset constraints on the centre dropped"
    );
    assert!(s.point(o).is_some() && s.origin == Some(o));
    assert!(s.point(lone).is_some(), "unrelated standalone point kept");
    assert!(s.point(c2).is_some());
    for p in r2.corners {
        assert!(s.point(p).is_some());
    }
    // Rectangle 2 is still fully dimensioned, with its names intact.
    assert_eq!(
        s.dimensions(),
        vec![("width2".into(), 20.0), ("height2".into(), 10.0)]
    );
    assert!(s.validate().is_ok());
}

#[test]
fn removing_a_rectangle_centred_on_the_origin_keeps_the_origin() {
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let r = editor_rect(&mut s, o, 80.0, 50.0, true);
    for l in r.lines {
        assert!(s.remove_entity(l));
    }
    assert_eq!(s.points.len(), 1);
    assert_eq!(
        s.constraints,
        vec![Constraint::FixedPoint {
            p: o,
            x: 0.0,
            y: 0.0
        }]
    );
}

#[test]
fn removing_a_circle_drops_its_diameter_and_centre() {
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let c = s.add_point(0.0, 0.0);
    let circle = s.add_circle(c, 5.0);
    s.constraints.extend([
        Constraint::Coincident { a: o, b: c },
        Constraint::Diameter {
            circle,
            dim: Dimension::new("diameter", 10.0),
        },
    ]);
    assert!(s.remove_entity(circle));
    assert!(s.point(c).is_none());
    assert_eq!(
        s.constraints.len(),
        1,
        "only the origin's FixedPoint is left"
    );
    assert!(s.dimensions().is_empty());
}

// ---------------------------------------------------------------- labels, names

#[test]
fn constraint_labels_are_human_readable() {
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let r = editor_rect(&mut s, o, 80.0, 50.0, true);
    let width_idx = s
        .constraints
        .iter()
        .position(|c| matches!(c, Constraint::HorizontalDistance { .. }))
        .unwrap();
    assert_eq!(s.constraint_label(0), "Fixed origin at (0, 0)");
    assert_eq!(
        s.constraint_label(width_idx),
        format!(
            "width = 80 mm (horizontal P{} to P{})",
            r.corners[0].0, r.corners[1].0
        )
    );
    assert_eq!(
        s.constraint_label(1),
        format!("Horizontal L{}", r.lines[0].0)
    );
    let sym = s
        .constraints
        .iter()
        .position(|c| matches!(c, Constraint::SymmetricAboutPoint { .. }))
        .unwrap();
    assert!(s.constraint_label(sym).ends_with("about origin"));
    assert_eq!(s.constraint_label(999), "constraint #999");
    assert_eq!(edit::fmt_mm(12.5), "12.5");
    assert_eq!(edit::fmt_mm(-0.00001), "0");
    assert_eq!(edit::fmt_mm(1.0 / 3.0), "0.3333");
}

#[test]
fn unique_dimension_names_skip_taken_ones() {
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    assert_eq!(s.unique_dimension_name("width"), "width");
    editor_rect(&mut s, o, 10.0, 10.0, true);
    assert_eq!(s.unique_dimension_name("width"), "width2");
    let c = s.add_point(50.0, 0.0);
    editor_rect(&mut s, c, 10.0, 10.0, true);
    assert_eq!(s.unique_dimension_name("width"), "width3");
    assert_eq!(s.unique_dimension_name("diameter"), "diameter");
    assert!(s.validate().is_ok(), "generated names are unique");
}

// ---------------------------------------------------------------- drags

#[test]
fn free_vertex_drag_reaches_target() {
    let mut s = Sketch::default();
    s.ensure_origin();
    let a = s.add_point(1.0, 1.0);
    let b = s.add_point(5.0, 2.0);
    s.add_line(a, b);
    let r = s.drag_point(b, [7.25, -3.5]);
    assert_eq!(r.outcome, DragOutcome::Reached);
    assert!(r.blocked_by.is_empty());
    assert!(close(xy(&s, b), [7.25, -3.5]));
    assert!(close(xy(&s, a), [1.0, 1.0]), "other end untouched");
    assert_eq!(r.report.status, SolveStatus::UnderConstrained);
}

#[test]
fn horizontal_line_vertex_slides_along_x() {
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let b = s.add_point(10.0, 0.0);
    let l = s.add_line(o, b);
    s.constraints.push(Constraint::Horizontal { line: l });
    let r = s.drag_point(b, [20.0, 5.0]);
    assert_eq!(r.outcome, DragOutcome::Partial, "{r:?}");
    // Closest reachable point on the line y = 0: (20, 0).
    assert!(close(xy(&s, b), [20.0, 0.0]), "{:?}", xy(&s, b));
    assert!(close(xy(&s, o), [0.0, 0.0]));
    // Removing the Horizontal constraint (index 1) or the origin's FixedPoint
    // (index 0) would each let the vertex reach (20, 5).
    assert_eq!(r.blocked_by, vec![0, 1]);
    assert!(r.report.is_success());
}

#[test]
fn fully_dimensioned_rectangle_corner_is_blocked_and_unchanged() {
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let r = editor_rect(&mut s, o, 80.0, 50.0, true);
    assert_eq!(s.solve().status, SolveStatus::FullyConstrained);
    let before = s.clone();
    let d = s.drag_point(r.corners[2], [45.0, 30.0]);
    assert_eq!(d.outcome, DragOutcome::Blocked, "{d:?}");
    assert!(!d.blocked_by.is_empty());
    assert!(d.blocked_by.iter().all(|&i| i < s.constraints.len()));
    assert_eq!(s, before, "a blocked drag leaves the sketch unchanged");
    assert_eq!(d.report.status, SolveStatus::FullyConstrained);
    // Dragging to where it already is counts as reached.
    let d = s.drag_point(r.corners[2], [40.0, 25.0]);
    assert_eq!(d.outcome, DragOutcome::Reached);
    // Only width/height free (no dims): dragging a corner resizes symmetrically.
    let mut free = Sketch::default();
    let o = free.ensure_origin();
    let r = editor_rect(&mut free, o, 80.0, 50.0, false);
    let d = free.drag_point(r.corners[2], [50.0, 30.0]);
    assert_eq!(d.outcome, DragOutcome::Reached, "{d:?}");
    assert!(close(xy(&free, r.corners[0]), [-50.0, -30.0]));
    assert!(close(xy(&free, r.corners[1]), [50.0, -30.0]));
}

#[test]
fn drag_of_missing_or_invalid_target_is_blocked() {
    let mut s = Sketch::default();
    s.ensure_origin();
    let before = s.clone();
    let d = s.drag_point(PointId(42), [1.0, 1.0]);
    assert_eq!(d.outcome, DragOutcome::Blocked);
    let p = s.add_point(1.0, 1.0);
    let before2 = s.clone();
    assert_eq!(
        s.drag_point(p, [f64::NAN, 0.0]).outcome,
        DragOutcome::Blocked
    );
    assert_eq!(s, before2);
    assert_eq!(
        s.drag_radius(EntityId(9), 3.0).outcome,
        DragOutcome::Blocked
    );
    s = before;
    assert_eq!(s.points.len(), 1);
}

#[test]
fn radius_drag_respects_diameter_dimension() {
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let c = s.add_circle(o, 5.0);
    let r = s.drag_radius(c, 8.0);
    assert_eq!(r.outcome, DragOutcome::Reached);
    assert!(
        matches!(s.entity(c), Some(Entity::Circle { radius, .. }) if (radius - 8.0).abs() < EPS)
    );
    assert_eq!(
        s.drag_radius(c, -1.0).outcome,
        DragOutcome::Blocked,
        "non-positive radius"
    );

    s.constraints.push(Constraint::Diameter {
        circle: c,
        dim: Dimension::new("diameter", 16.0),
    });
    let before = s.clone();
    let r = s.drag_radius(c, 3.0);
    assert_eq!(r.outcome, DragOutcome::Blocked);
    assert_eq!(r.blocked_by, vec![1], "the Diameter constraint blocks it");
    assert_eq!(s, before);
}

// ---------------------------------------------------------------- profiles

fn polygon(vertices: &[[f64; 2]]) -> (Sketch, Vec<PointId>) {
    let mut s = Sketch::default();
    s.ensure_origin();
    let ids: Vec<PointId> = vertices.iter().map(|v| s.add_point(v[0], v[1])).collect();
    for i in 0..ids.len() {
        s.add_line(ids[i], ids[(i + 1) % ids.len()]);
    }
    (s, ids)
}

#[test]
fn profile_of_editor_loop_ignores_isolated_origin() {
    let (s, _) = polygon(&[
        [0.0, 0.0],
        [60.0, 0.0],
        [60.0, 30.0],
        [50.0, 40.0],
        [0.0, 40.0],
    ]);
    let Profile::Polygon(v) = s.profile().unwrap() else {
        panic!()
    };
    assert_eq!(v.len(), 5, "origin point is not a profile vertex");
    // Area by hand: 60x40 rectangle minus the 10x10/2 corner triangle = 2350.
    let a2: f64 = (0..5)
        .map(|i| v[i][0] * v[(i + 1) % 5][1] - v[(i + 1) % 5][0] * v[i][1])
        .sum();
    assert!((a2 / 2.0 - 2350.0).abs() < 1e-9);
}

#[test]
fn profile_rejects_loop_with_dangling_line() {
    let (mut s, ids) = polygon(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
    let tail = s.add_point(20.0, 20.0);
    s.add_line(ids[2], tail);
    assert!(matches!(
        s.profile(),
        Err(SketchError::UnsupportedProfile(_))
    ));
}

#[test]
fn profile_rejects_open_polyline() {
    let (mut s, _) = polygon(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
    let closing = s
        .entities
        .last()
        .map(Entity::id)
        .expect("closing line exists");
    s.entities.retain(|e| e.id() != closing);
    assert!(matches!(
        s.profile(),
        Err(SketchError::UnsupportedProfile(_))
    ));
}

#[test]
fn profile_rejects_zero_length_edge() {
    // Two distinct points at the same location joined by a line.
    let (s, _) = polygon(&[
        [0.0, 0.0],
        [10.0, 0.0],
        [10.0, 0.0],
        [10.0, 10.0],
        [0.0, 10.0],
    ]);
    assert!(matches!(
        s.profile(),
        Err(SketchError::DegenerateProfile(_))
    ));
}

#[test]
fn profile_rejects_self_intersections_including_touching() {
    // Crossing edges.
    let (s, _) = polygon(&[[0.0, 0.0], [10.0, 0.0], [0.0, 10.0], [10.0, 10.0]]);
    assert!(matches!(
        s.profile(),
        Err(SketchError::UnsupportedProfile(_))
    ));
    // A vertex touching a non-adjacent edge (pinched loop).
    let (s, _) = polygon(&[
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 20.0],
        [10.0, 0.0],
        [0.0, 20.0],
    ]);
    assert!(matches!(
        s.profile(),
        Err(SketchError::UnsupportedProfile(_))
    ));
    // Collinear backtrack (zero area spike).
    let (s, _) = polygon(&[[0.0, 0.0], [10.0, 0.0], [5.0, 0.0]]);
    assert!(s.profile().is_err());
}
