//! Profile regions (outer loop + holes).
use crate::*;

fn poly(s: &mut Sketch, v: &[[f64; 2]]) -> Vec<EntityId> {
    let p: Vec<_> = v.iter().map(|q| s.add_point(q[0], q[1])).collect();
    (0..p.len())
        .map(|i| s.add_line(p[i], p[(i + 1) % p.len()]))
        .collect()
}

fn circle(s: &mut Sketch, c: [f64; 2], r: f64) -> EntityId {
    let p = s.add_point(c[0], c[1]);
    s.add_circle(p, r)
}

fn base() -> Sketch {
    let mut s = Sketch::default();
    s.ensure_origin();
    s
}

#[test]
fn rectangle_with_circle_and_slot_holes() {
    let mut s = base();
    let outer = poly(
        &mut s,
        &[[0.0, 0.0], [80.0, 0.0], [80.0, 50.0], [0.0, 50.0]],
    );
    let c = circle(&mut s, [20.0, 25.0], 5.0);
    let slot = poly(
        &mut s,
        &[[50.0, 22.0], [60.0, 22.0], [60.0, 28.0], [50.0, 28.0]],
    );
    let r = s.profile_region().unwrap();
    assert!(matches!(r.outer, Profile::Polygon(ref v) if v.len() == 4));
    let mut o = r.outer_entities.clone();
    o.sort();
    assert_eq!(o, outer);
    assert_eq!(r.holes.len(), 2);
    assert!(
        r.holes
            .iter()
            .any(|(p, e)| matches!(p, Profile::Circle { .. }) && e == &vec![c])
    );
    assert!(r.holes.iter().any(|(_, e)| {
        let mut e = e.clone();
        e.sort();
        e == slot
    }));
    // The single-loop API still refuses several loops.
    assert!(s.profile().is_err());
}

#[test]
fn single_loop_region_matches_profile() {
    let mut s = base();
    poly(&mut s, &[[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]]);
    let r = s.profile_region().unwrap();
    assert_eq!(r.outer, s.profile().unwrap());
    assert!(r.holes.is_empty());
}

#[test]
fn islands_separate_outers_and_touching_loops_are_refused() {
    // Island inside a hole.
    let mut s = base();
    poly(
        &mut s,
        &[[0.0, 0.0], [80.0, 0.0], [80.0, 50.0], [0.0, 50.0]],
    );
    circle(&mut s, [40.0, 25.0], 20.0);
    circle(&mut s, [40.0, 25.0], 5.0);
    let e = s.profile_region().unwrap_err().to_string();
    assert!(e.contains("island"), "{e}");
    // Two separate outer loops.
    let mut s = base();
    poly(
        &mut s,
        &[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
    );
    circle(&mut s, [30.0, 5.0], 3.0);
    let e = s.profile_region().unwrap_err().to_string();
    assert!(e.contains("separate outer loops"), "{e}");
    // Hole crossing the outer boundary.
    let mut s = base();
    poly(
        &mut s,
        &[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
    );
    circle(&mut s, [10.0, 5.0], 2.0);
    let e = s.profile_region().unwrap_err().to_string();
    assert!(e.contains("touch or cross"), "{e}");
    // Hole tangent to the outer boundary (touching counts).
    let mut s = base();
    poly(
        &mut s,
        &[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
    );
    circle(&mut s, [5.0, 3.0], 3.0);
    let e = s.profile_region().unwrap_err().to_string();
    assert!(e.contains("touch or cross"), "{e}");
}

#[test]
fn hole_inside_an_arc_bulge_is_classified_by_the_arc() {
    // D-shaped outer loop: line (0,0)-(20,0), then a CCW half circle back over
    // the top (centre (10,0), r 10). A hole at (10, 7) lies only inside the arc's
    // circular segment (above the chord), so the arc term decides containment.
    let mut s = base();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(20.0, 0.0);
    let c = s.add_point(10.0, 0.0);
    s.add_line(a, b);
    s.entities.push(Entity::Arc {
        id: s.next_entity_id(),
        center: c,
        start: b,
        end: a,
    });
    circle(&mut s, [10.0, 7.0], 1.0);
    let r = s.profile_region().unwrap();
    assert_eq!(r.holes.len(), 1);
    // Outside the D (below the line): two outer loops.
    let mut s2 = s.clone();
    if let Some(Entity::Circle { center, .. }) = s2.entities.last().cloned() {
        let p = s2.points.iter_mut().find(|p| p.id == center).unwrap();
        p.y = -5.0;
    }
    assert!(s2.profile_region().is_err());
}
