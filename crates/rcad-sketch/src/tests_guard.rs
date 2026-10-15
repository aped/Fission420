//! Branch-flip guard (`guard.rs`). Expected areas are the closed form of a
//! rounded rectangle, w·h − (4 − π)·r².
use super::*;
use std::f64::consts::{FRAC_PI_2, PI};

/// Corner-anchored w x h rectangle (origin fixed), dimensions "w"/"h", all
/// four corners filleted r.
fn rounded(w: f64, h: f64, r: f64) -> Sketch {
    let mut s = Sketch::default();
    let p: Vec<PointId> = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
        .iter()
        .map(|q| s.add_point(q[0], q[1]))
        .collect();
    s.origin = Some(p[0]);
    let l: Vec<EntityId> = (0..4).map(|i| s.add_line(p[i], p[(i + 1) % 4])).collect();
    s.constraints.extend([
        Constraint::FixedPoint {
            p: p[0],
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
    ]);
    assert!(s.solve().is_success());
    for (i, c) in p.iter().enumerate() {
        s.fillet_corner(*c, r, Some(if i == 0 { "r" } else { "rr" }))
            .unwrap();
    }
    assert_eq!(s.solve().status, SolveStatus::FullyConstrained);
    s
}

fn arcs(s: &Sketch) -> Vec<f64> {
    s.entities
        .iter()
        .filter_map(|e| match e {
            Entity::Arc { id, .. } => s.arc_geometry(*id).map(|g| g.sweep),
            _ => None,
        })
        .collect()
}

#[test]
fn a_tenfold_width_edit_keeps_every_fillet_a_quarter_arc() {
    let mut s = rounded(20.0, 10.0, 2.0);
    let r = s.set_dimension_solved("w", 200.0).unwrap();
    assert!(r.is_success(), "{r:?}");
    let sweeps = arcs(&s);
    assert_eq!(sweeps.len(), 4);
    for sw in sweeps {
        assert!((sw - FRAC_PI_2).abs() < 1e-9, "{sw}");
    }
    let area = s.regions().unwrap()[0].area;
    let want = 200.0 * 10.0 - (4.0 - PI) * 4.0;
    assert!((area - want).abs() < 1e-6, "{area} vs {want}");
}

#[test]
fn a_jumped_sweep_is_detected_and_a_smooth_one_is_not() {
    // One arc about the origin from (10, 0) to (0, 10): 90°.
    let mut a = Sketch::default();
    let c = a.add_point(0.0, 0.0);
    let p = a.add_point(10.0, 0.0);
    let q = a.add_point(0.0, 10.0);
    let id = a.next_entity_id();
    a.entities.push(Entity::Arc {
        id,
        center: c,
        start: p,
        end: q,
    });
    // The centre moved across the chord: the 270° complement.
    let mut b = a.clone();
    b.points.iter_mut().find(|x| x.id == c).unwrap().x = 10.0;
    b.points.iter_mut().find(|x| x.id == c).unwrap().y = 10.0;
    assert_eq!(guard::sweep_jumps(&a, &b), vec![id]);
    // A small move of the end: 90° -> ~95°.
    let mut d = a.clone();
    let e = d.points.iter_mut().find(|x| x.id == q).unwrap();
    (e.x, e.y) = (-10.0 * 0.0872, 10.0 * 0.9962);
    assert!(guard::sweep_jumps(&a, &d).is_empty());
}

#[test]
fn unknown_or_invalid_values_are_errors_and_failed_solves_are_reported() {
    let mut s = rounded(20.0, 10.0, 2.0);
    let before = s.clone();
    assert!(s.set_dimension_solved("nope", 3.0).is_err());
    assert!(s.set_dimension_solved("w", -1.0).is_err());
    assert_eq!(s, before);
    // Radius 9 at one corner of the 10 mm high side (the next fillet is 2)
    // pushes the fillets past each other (the side line reverses): refused,
    // sketch unchanged.
    let err = s.set_dimension_solved("r", 9.0).unwrap_err();
    assert!(err.to_string().contains("would flip"), "{err}");
    assert_eq!(s, before);
}
