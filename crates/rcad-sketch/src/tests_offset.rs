//! Loop offset: exact geometry with analytic areas
//! (sharp-cornered rectangles and L shapes, slots, rounded rectangles,
//! circles), the editable distance dimension, the copy following source edits,
//! and refusals for collapsed or touching copies.
use super::*;
use std::f64::consts::PI;

const AREA_TOL: f64 = 1e-6;

/// Centred rectangle with dimensions `w`/`h` and a fixed centre (origin 0).
fn rect(w: f64, h: f64) -> (Sketch, EntityId) {
    let mut s = Sketch::default();
    let o = s.add_point(0.0, 0.0);
    s.origin = Some(o);
    let p: Vec<PointId> = [
        [-w / 2.0, -h / 2.0],
        [w / 2.0, -h / 2.0],
        [w / 2.0, h / 2.0],
        [-w / 2.0, h / 2.0],
    ]
    .iter()
    .map(|q| s.add_point(q[0], q[1]))
    .collect();
    let l: Vec<EntityId> = (0..4).map(|i| s.add_line(p[i], p[(i + 1) % 4])).collect();
    s.constraints.extend([
        Constraint::FixedPoint {
            p: o,
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
        Constraint::SymmetricAboutPoint {
            a: p[0],
            b: p[2],
            center: o,
        },
    ]);
    assert_eq!(s.solve().status, SolveStatus::FullyConstrained);
    (s, l[0])
}

/// Area enclosed by the loop through `copy` (as its own region, outer only).
fn loop_area(s: &Sketch, copy: &[EntityId]) -> f64 {
    let mut ids = copy.to_vec();
    ids.sort();
    let rs = s.regions().expect("regions");
    let r = rs
        .iter()
        .find(|r| r.id.outer == ids)
        .unwrap_or_else(|| panic!("no region bounded by {ids:?}: {rs:?}"));
    r.area + hole_areas(&rs, r)
}

/// Sum of the areas of the regions directly inside `r` (to recover the outer
/// loop's full area from the region area).
fn hole_areas(rs: &[SketchRegion], r: &SketchRegion) -> f64 {
    r.id.holes
        .iter()
        .map(|h| {
            let inner = rs.iter().find(|x| &x.id.outer == h).expect("hole region");
            inner.area + hole_areas(rs, inner)
        })
        .sum()
}

#[test]
fn rectangle_offsets_with_sharp_corners_and_editable_distance() {
    for (d, want) in [(2.0, 44.0 * 24.0), (-2.0, 36.0 * 16.0), (0.5, 41.0 * 21.0)] {
        let (mut s, edge) = rect(40.0, 20.0);
        let r = s.offset_loop(edge, d, Some("wall")).unwrap();
        assert_eq!(r.entities.len(), 4);
        assert_eq!(r.dimension, "wall");
        assert!(
            (loop_area(&s, &r.entities) - want).abs() < AREA_TOL,
            "d={d}"
        );
        let rep = s.solve();
        assert_eq!(rep.status, SolveStatus::FullyConstrained, "{rep:?}");
        assert!(s.dimensions().contains(&("wall".to_string(), d.abs())));
    }
    // The distance and the source are editable; the copy follows.
    let (mut s, edge) = rect(40.0, 20.0);
    let r = s.offset_loop(edge, -2.0, Some("wall")).unwrap();
    s.set_dimension("wall", 3.0).unwrap();
    assert!(s.solve().is_success());
    assert!((loop_area(&s, &r.entities) - 34.0 * 14.0).abs() < AREA_TOL);
    s.set_dimension("w", 60.0).unwrap();
    s.set_dimension("h", 30.0).unwrap();
    let rep = s.solve();
    assert_eq!(rep.status, SolveStatus::FullyConstrained, "{rep:?}");
    assert!((loop_area(&s, &r.entities) - 54.0 * 24.0).abs() < AREA_TOL);
    // The ring between source and copy is one region.
    let ring = s
        .regions()
        .unwrap()
        .into_iter()
        .find(|x| x.depth == 0)
        .unwrap();
    assert!((ring.area - (1800.0 - 54.0 * 24.0)).abs() < AREA_TOL);
}

#[test]
fn concave_corner_is_trimmed_and_convex_corners_mitred() {
    // L shape: 20 x 20 with the top-right 10 x 10 removed (area 300).
    let mut s = Sketch::default();
    let p: Vec<PointId> = [
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 10.0],
        [10.0, 10.0],
        [10.0, 20.0],
        [0.0, 20.0],
    ]
    .iter()
    .map(|q| s.add_point(q[0], q[1]))
    .collect();
    let l: Vec<EntityId> = (0..6).map(|i| s.add_line(p[i], p[(i + 1) % 6])).collect();
    for (d, want) in [(1.0, 22.0 * 22.0 - 100.0), (-1.0, 18.0 * 18.0 - 100.0)] {
        let mut t = s.clone();
        let r = t.offset_loop(l[2], d, None).unwrap();
        assert!(
            (loop_area(&t, &r.entities) - want).abs() < AREA_TOL,
            "d={d}"
        );
        assert_eq!(r.dimension, "offset");
    }
    // Inward by more than half an arm (arms are 10 wide): edges vanish.
    let mut t = s.clone();
    let e = t.offset_loop(l[0], -6.0, None).unwrap_err();
    assert!(
        e.to_string().contains("vanish") || e.to_string().contains("meet"),
        "{e}"
    );
    assert_eq!(t, s, "unchanged on refusal");
}

/// Slot: centres (±L/2, 0), radius r; lines top/bottom, half-circle arcs.
fn slot(len: f64, r: f64) -> (Sketch, EntityId) {
    let mut s = Sketch::default();
    let h = len / 2.0;
    let bl = s.add_point(-h, -r);
    let br = s.add_point(h, -r);
    let tr = s.add_point(h, r);
    let tl = s.add_point(-h, r);
    let c1 = s.add_point(-h, 0.0);
    let c2 = s.add_point(h, 0.0);
    let bottom = s.add_line(bl, br);
    let top = s.add_line(tr, tl);
    let a = s.next_entity_id();
    s.entities.push(Entity::Arc {
        id: a,
        center: c2,
        start: br,
        end: tr,
    });
    let b = s.next_entity_id();
    s.entities.push(Entity::Arc {
        id: b,
        center: c1,
        start: tl,
        end: bl,
    });
    // Every source point fixed, so a distance edit moves only the copy. (Fixed
    // arc ends make each source arc's implied equal radius redundant: the
    // source alone is "over-constrained (consistent)"; the offset must not add
    // to that redundancy.)
    let fixed: Vec<Constraint> = s
        .points
        .iter()
        .map(|q| Constraint::FixedPoint {
            p: q.id,
            x: q.x,
            y: q.y,
        })
        .collect();
    s.constraints.extend(fixed);
    let _ = top;
    (s, bottom)
}

#[test]
fn slot_offsets_concentric_arcs_and_collapses_inward_past_the_radius() {
    let slot_area = |l: f64, r: f64| l * 2.0 * r + PI * r * r;
    for (d, rr) in [(1.5, 6.5), (-2.0, 3.0)] {
        let (mut s, bottom) = slot(30.0, 5.0);
        let r = s.offset_loop(bottom, d, None).unwrap();
        assert!(
            (loop_area(&s, &r.entities) - slot_area(30.0, rr)).abs() < AREA_TOL,
            "d={d}"
        );
        // Copy arcs share the source centres; the system is consistent and
        // adds no redundancy.
        let rep = s.solve();
        let off = s.constraints.len() - 1;
        assert!(rep.is_success(), "{rep:?}");
        assert!(
            !rep.conflicting.contains(&off),
            "the offset is not part of a redundant set: {rep:?}"
        );
        // Distance edit moves the copy.
        s.set_dimension("offset", 1.0).unwrap();
        assert!(s.solve().is_success());
        let want = if d > 0.0 { 6.0 } else { 4.0 };
        assert!((loop_area(&s, &r.entities) - slot_area(30.0, want)).abs() < AREA_TOL);
    }
    let (mut s, bottom) = slot(30.0, 5.0);
    let before = s.clone();
    let e = s.offset_loop(bottom, -5.0, None).unwrap_err();
    assert!(e.to_string().contains("collapse"), "{e}");
    assert_eq!(s, before);
}

#[test]
fn rounded_rectangle_offsets_follow_the_rounded_corner_formula() {
    // w x h with corner radius rc: area = w h − (4 − π) rc².
    let (w, h, rc) = (40.0, 24.0, 4.0);
    let mut s = Sketch::default();
    let (x0, y0) = (w / 2.0 - rc, h / 2.0 - rc);
    let c: Vec<PointId> = [[x0, -y0], [x0, y0], [-x0, y0], [-x0, -y0]]
        .iter()
        .map(|q| s.add_point(q[0], q[1]))
        .collect();
    // Corner tangent points, counter-clockwise from the bottom edge's right end.
    let t: Vec<PointId> = [
        [x0, -h / 2.0],
        [w / 2.0, -y0],
        [w / 2.0, y0],
        [x0, h / 2.0],
        [-x0, h / 2.0],
        [-w / 2.0, y0],
        [-w / 2.0, -y0],
        [-x0, -h / 2.0],
    ]
    .iter()
    .map(|q| s.add_point(q[0], q[1]))
    .collect();
    let mut first = None;
    for k in 0..4 {
        let id = s.next_entity_id();
        s.entities.push(Entity::Arc {
            id,
            center: c[k],
            start: t[2 * k],
            end: t[2 * k + 1],
        });
        let l = s.add_line(t[2 * k + 1], t[(2 * k + 2) % 8]);
        first.get_or_insert(l);
    }
    let area = |w: f64, h: f64, r: f64| w * h - (4.0 - PI) * r * r;
    for (d, want) in [(2.0, area(44.0, 28.0, 6.0)), (-3.0, area(34.0, 18.0, 1.0))] {
        let mut t2 = s.clone();
        let r = t2.offset_loop(first.unwrap(), d, None).unwrap();
        assert!(
            (loop_area(&t2, &r.entities) - want).abs() < AREA_TOL,
            "d={d}"
        );
        assert!(t2.solve().is_success());
    }
    // Inward past the corner radius: the corner arcs collapse (no automatic
    // sharpening).
    let mut t2 = s.clone();
    assert!(t2.offset_loop(first.unwrap(), -4.5, None).is_err());
}

#[test]
fn circle_offset_and_touching_loops_are_refused() {
    let (mut s, edge) = rect(40.0, 20.0);
    let c = s.add_point(10.0, 0.0);
    let circle = s.add_circle(c, 4.0);
    // A fully defined source, so a distance edit moves only the copy.
    s.constraints.extend([
        Constraint::FixedPoint {
            p: c,
            x: 10.0,
            y: 0.0,
        },
        Constraint::Diameter {
            circle,
            dim: Dimension::new("d", 8.0),
        },
    ]);
    let mut t = s.clone();
    let r = t.offset_loop(circle, 2.0, Some("ring")).unwrap();
    let Some(Entity::Circle { radius, .. }) = t.entity(r.entities[0]).cloned() else {
        panic!()
    };
    assert!((radius - 6.0).abs() < 1e-9);
    t.set_dimension("ring", 1.0).unwrap();
    assert!(t.solve().is_success());
    let Some(Entity::Circle { radius, .. }) = t.entity(r.entities[0]).cloned() else {
        panic!()
    };
    assert!((radius - 5.0).abs() < 1e-9);
    // Outward 6 would reach the rectangle's edge (10 mm away): refused.
    let mut t = s.clone();
    assert!(t.offset_loop(circle, 6.0, None).is_err());
    assert!(t.offset_loop(circle, -4.0, None).is_err(), "radius 0");
    assert!(t.offset_loop(edge, 0.0, None).is_err());
    assert_eq!(t, s);
}

#[test]
fn offset_constraint_serde_and_label() {
    let (mut s, edge) = rect(40.0, 20.0);
    s.offset_loop(edge, -2.0, Some("wall")).unwrap();
    let json = serde_json::to_string(&s).unwrap();
    assert!(json.contains("\"Offset\"") && json.contains("\"wall\""));
    let back: Sketch = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s);
    let i = s.constraints.len() - 1;
    assert!(
        s.constraint_label(i)
            .starts_with("wall = 2 (offset of 4 curves)")
    );
    // Removing a source edge removes the offset with it.
    assert!(s.remove_entity(edge));
    assert!(
        !s.constraints
            .iter()
            .any(|c| matches!(c, Constraint::Offset { .. }))
    );
}

/// An ellipse that only shares a point with the loop (here: centred on a
/// slot's arc centre, inside the slot) does not block the loop's offset; a
/// spline that is part of the loop is refused by name.
#[test]
fn a_free_form_curve_touching_the_loop_is_allowed_one_in_it_is_refused() {
    // Slot: centres (0,0) and (20,0), radius 5; an ellipse 2 x 1 about the
    // left centre point (an island inside the slot).
    let mut s = Sketch::default();
    let q = |s: &mut Sketch, x: f64, y: f64| s.add_point(x, y);
    let (c1, c2) = (q(&mut s, 0.0, 0.0), q(&mut s, 20.0, 0.0));
    let (b0, b1, t1, t0) = (
        q(&mut s, 0.0, -5.0),
        q(&mut s, 20.0, -5.0),
        q(&mut s, 20.0, 5.0),
        q(&mut s, 0.0, 5.0),
    );
    s.add_line(b0, b1);
    let right = s.next_entity_id();
    s.entities.push(Entity::Arc {
        id: right,
        center: c2,
        start: b1,
        end: t1,
    });
    s.add_line(t1, t0);
    let left = s.next_entity_id();
    s.entities.push(Entity::Arc {
        id: left,
        center: c1,
        start: t0,
        end: b0,
    });
    let m = q(&mut s, 2.0, 0.0);
    s.entities.push(Entity::Ellipse {
        id: s.next_entity_id(),
        center: c1,
        major: m,
        minor: 1.0,
    });
    s.validate().unwrap();
    let r = s.offset_loop(left, 1.0, None).unwrap();
    assert_eq!(r.entities.len(), 4);
    assert!(s.solve().is_success());
    let want = 20.0 * 12.0 + PI * 36.0;
    let rs = s.regions().unwrap();
    let mut ids = r.entities.clone();
    ids.sort();
    let outer = rs
        .iter()
        .find(|x| {
            let mut o = x.id.outer.clone();
            o.sort();
            o == ids
        })
        .expect("copy loop region");
    // The copy's region is the ring around the slot (the slot is its hole).
    let ring = want - (20.0 * 10.0 + PI * 25.0);
    assert!(
        (outer.area - ring).abs() < AREA_TOL,
        "{} vs {ring}",
        outer.area
    );

    // Two lines closed by a spline: the spline is part of the loop.
    let mut t = Sketch::default();
    let p: Vec<PointId> = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]
        .iter()
        .map(|q| t.add_point(q[0], q[1]))
        .collect();
    let l0 = t.add_line(p[0], p[1]);
    t.add_line(p[1], p[2]);
    let (sp, pts) = t
        .add_spline(&[[10.0, 10.0], [5.0, 12.0], [-2.0, 6.0], [0.0, 0.0]], false)
        .unwrap();
    for e in &mut t.entities {
        if let Entity::Spline { id, points, .. } = e
            && *id == sp
        {
            points[0] = p[2];
            points[3] = p[0];
        }
    }
    t.points.retain(|q| q.id != pts[0] && q.id != pts[3]);
    t.validate().unwrap();
    let before = t.clone();
    let err = t.offset_loop(l0, 1.0, None).unwrap_err();
    assert!(err.to_string().contains("part of the loop"), "{err}");
    assert_eq!(t, before);
}
