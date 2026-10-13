//! Region enumeration and picking (SESSION_4 §7 item 1): nested islands at any
//! depth, several outer loops, persistent region identities that survive
//! dimension edits, point-in-region queries. Areas are analytic.
use super::*;
use std::f64::consts::PI;

/// Axis-aligned rectangle centred at the origin, with width/height dimensions
/// on its bottom/right edges (names `{tag}_w`, `{tag}_h`). Returns its lines.
fn rect(s: &mut Sketch, w: f64, h: f64, tag: &str) -> Vec<EntityId> {
    let p: Vec<PointId> = [
        [-w / 2.0, -h / 2.0],
        [w / 2.0, -h / 2.0],
        [w / 2.0, h / 2.0],
        [-w / 2.0, h / 2.0],
    ]
    .iter()
    .map(|q| s.add_point(q[0], q[1]))
    .collect();
    let lines: Vec<EntityId> = (0..4).map(|i| s.add_line(p[i], p[(i + 1) % 4])).collect();
    s.constraints.extend([
        Constraint::Horizontal { line: lines[0] },
        Constraint::Horizontal { line: lines[2] },
        Constraint::Vertical { line: lines[1] },
        Constraint::Vertical { line: lines[3] },
        Constraint::HorizontalDistance {
            a: p[0],
            b: p[1],
            dim: Dimension::new(format!("{tag}_w"), w),
        },
        Constraint::VerticalDistance {
            a: p[1],
            b: p[2],
            dim: Dimension::new(format!("{tag}_h"), h),
        },
        // Centred on the origin.
        Constraint::SymmetricAboutPoint {
            a: p[0],
            b: p[2],
            center: PointId(0),
        },
    ]);
    lines
}

/// Origin point 0, outer 60x40, inner 40x20, circle Ø10 at the origin.
fn nested() -> (Sketch, Vec<EntityId>, Vec<EntityId>, EntityId) {
    let mut s = Sketch::default();
    let o = s.add_point(0.0, 0.0);
    s.origin = Some(o);
    let outer = rect(&mut s, 60.0, 40.0, "outer");
    let inner = rect(&mut s, 40.0, 20.0, "inner");
    let c = s.add_point(0.0, 0.0);
    let circle = s.add_circle(c, 5.0);
    s.constraints.extend([
        Constraint::Coincident { a: c, b: o },
        Constraint::Diameter {
            circle,
            dim: Dimension::new("hole", 10.0),
        },
    ]);
    assert!(s.solve().is_success());
    (s, outer, inner, circle)
}

fn sorted(mut v: Vec<EntityId>) -> Vec<EntityId> {
    v.sort();
    v
}

#[test]
fn three_nested_regions_with_identities_and_areas() {
    let (s, outer, inner, circle) = nested();
    // The default whole-sketch profile still refuses the island (unchanged).
    assert!(s.profile_region().is_err());
    let rs = s.regions().unwrap();
    assert_eq!(rs.len(), 3);
    let ring = RegionRef {
        outer: sorted(outer.clone()),
        holes: vec![sorted(inner.clone())],
    };
    let middle = RegionRef {
        outer: sorted(inner.clone()),
        holes: vec![vec![circle]],
    };
    let disc = RegionRef {
        outer: vec![circle],
        holes: vec![],
    };
    assert_eq!(
        rs.iter()
            .map(|r| (r.id.clone(), r.depth))
            .collect::<Vec<_>>(),
        vec![(ring.clone(), 0), (middle.clone(), 1), (disc.clone(), 2)]
    );
    let areas = [2400.0 - 800.0, 800.0 - 25.0 * PI, 25.0 * PI];
    for (r, a) in rs.iter().zip(areas) {
        assert!((r.area - a).abs() < 1e-9, "{:?}: {} vs {a}", r.id, r.area);
    }
    assert_eq!(rs[0].region.holes.len(), 1);
    assert_eq!(ring.label(), "E1-4 − E5-8");
    // Point queries.
    assert_eq!(s.region_at([25.0, 0.0]).unwrap(), Some(ring.clone()));
    assert_eq!(s.region_at([15.0, 5.0]).unwrap(), Some(middle.clone()));
    assert_eq!(s.region_at([1.0, 1.0]).unwrap(), Some(disc.clone()));
    assert_eq!(s.region_at([100.0, 0.0]).unwrap(), None);
    assert_eq!(s.region_at([20.0, 0.0]).unwrap(), None, "on a boundary");
    assert!(s.region_by_ref(&middle).unwrap().is_some());
}

#[test]
fn identities_survive_dimension_edits_and_break_only_for_the_changed_region() {
    let (mut s, outer, inner, circle) = nested();
    let before: Vec<RegionRef> = s.regions().unwrap().into_iter().map(|r| r.id).collect();
    s.set_dimension("outer_w", 80.0).unwrap();
    s.set_dimension("inner_h", 24.0).unwrap();
    s.set_dimension("hole", 12.0).unwrap();
    assert!(s.solve().is_success());
    let after = s.regions().unwrap();
    assert_eq!(
        after.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        before
    );
    let a = [3200.0 - 960.0, 960.0 - 36.0 * PI, 36.0 * PI];
    for (r, want) in after.iter().zip(a) {
        // Solved coordinates are exact to the solve tolerance (~1e-8 mm), so
        // the area to about perimeter x 1e-8 (< 1e-5 mm² here).
        assert!((r.area - want).abs() < 1e-5, "{} vs {want}", r.area);
    }
    // Delete the island: the ring's identity is unchanged; the middle region
    // (which had the island as a hole) no longer exists; nothing else matches it.
    assert!(s.remove_entity(circle));
    let ring = RegionRef {
        outer: sorted(outer),
        holes: vec![sorted(inner.clone())],
    };
    assert!(s.region_by_ref(&ring).unwrap().is_some());
    assert!(s.region_by_ref(&before[1]).unwrap().is_none());
    let plain_inner = RegionRef {
        outer: sorted(inner),
        holes: vec![],
    };
    assert!(s.region_by_ref(&plain_inner).unwrap().is_some());
    // Now one outer + one hole: the default profile works again.
    assert!(s.profile_region().is_ok());
}

#[test]
fn separate_outer_loops_are_separate_regions() {
    let mut s = Sketch::default();
    let o = s.add_point(0.0, 0.0);
    s.origin = Some(o);
    let a = s.add_point(-20.0, 0.0);
    let b = s.add_point(20.0, 0.0);
    let ca = s.add_circle(a, 5.0);
    let cb = s.add_circle(b, 3.0);
    assert!(s.profile_region().is_err(), "two outers refused by default");
    let rs = s.regions().unwrap();
    assert_eq!(rs.len(), 2);
    assert!(rs.iter().all(|r| r.depth == 0));
    assert_eq!(s.region_at([20.0, 1.0]).unwrap().unwrap().outer, vec![cb]);
    assert_eq!(s.region_at([-20.0, 1.0]).unwrap().unwrap().outer, vec![ca]);
    // Touching loops are still refused.
    let mut t = s.clone();
    let c = t.add_point(-12.0, 0.0);
    t.add_circle(c, 3.0);
    assert!(t.regions().is_err());
}

#[test]
fn region_ref_json_shape() {
    let r = RegionRef {
        outer: vec![EntityId(1), EntityId(2), EntityId(3)],
        holes: vec![vec![EntityId(5)]],
    };
    let j = serde_json::to_string(&r).unwrap();
    assert_eq!(j, r#"{"outer":[1,2,3],"holes":[[5]]}"#);
    let plain: RegionRef = serde_json::from_str(r#"{"outer":[7]}"#).unwrap();
    assert!(plain.holes.is_empty());
}
