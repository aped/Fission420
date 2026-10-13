//! Join / general cut / face-region tests. Expected values are analytic.
use super::*;
use crate::{VolumeResult, export_step, extrude, measure};

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Profile {
    Profile::Polygon(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
}

fn volume(b: &Body) -> f64 {
    match measure(b).unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        VolumeResult::Unavailable { reason } => panic!("volume unavailable: {reason}"),
    }
}

fn block() -> Body {
    extrude(&rect(0.0, 0.0, 40.0, 30.0), &Frame::XY, 10.0).unwrap()
}

/// Top face of `block()` as a frame (outward +Z).
fn top() -> Frame {
    Frame::XY.offset(10.0)
}

#[test]
fn join_boss_on_top_face_with_overlap() {
    let a = block();
    let boss = rect(5.0, 5.0, 25.0, 15.0);
    assert!(plane_has_face(&a, &top()));
    assert!(profile_within_face(&a, &top(), &boss).unwrap());
    // Exactly coplanar contact: Truck returns no result -> BooleanFailed, the
    // reason the document extends the tool into the support (ADR 0003).
    assert!(join_prism(&a, &boss, &top(), 5.0).is_err());
    let j = join_prism(&a, &boss, &top().offset(-0.5), 5.5).unwrap();
    assert!((volume(&j) - 13000.0).abs() < 1e-6);
    let m = measure(&j).unwrap();
    assert_eq!((m.shell_count, m.all_shells_closed), (1, true));
    assert!((m.bounds.max[2] - 15.0).abs() < 1e-9);
    // Exact plane/plane edges: STEP export works.
    export_step(&j, "test").unwrap();
}

#[test]
fn profile_outside_or_over_a_hole_is_not_within_face() {
    let a = block();
    // Overhangs the +X edge.
    assert!(!profile_within_face(&a, &top(), &rect(30.0, 5.0, 45.0, 15.0)).unwrap());
    // Over a through hole of the face.
    let holed = cut_prism(
        &a,
        &Profile::Circle {
            center: [20.0, 15.0],
            radius: 3.0,
        },
        &Frame::XY.offset(-1.0),
        12.0,
    )
    .unwrap();
    assert!(!profile_within_face(&holed, &top(), &rect(15.0, 10.0, 25.0, 20.0)).unwrap());
    assert!(profile_within_face(&holed, &top(), &rect(1.0, 1.0, 10.0, 8.0)).unwrap());
    // A plane with no face.
    assert!(!profile_within_face(&a, &Frame::XY.offset(5.0), &rect(1.0, 1.0, 2.0, 2.0)).unwrap());
}

#[test]
fn join_far_away_or_disconnected_is_refused() {
    let a = block();
    let far = join_prism(&a, &rect(100.0, 0.0, 110.0, 10.0), &Frame::XY, 10.0);
    assert!(far.is_err(), "{far:?}");
}

#[test]
fn rectangular_through_cut_and_non_intersecting_cut() {
    let a = block();
    // 10 x 5 rectangle through the 10 mm thickness: 12000 - 500.
    let c = cut_prism(
        &a,
        &rect(28.0, 20.0, 38.0, 25.0),
        &Frame::XY.offset(-1.0),
        12.0,
    )
    .unwrap();
    assert!((volume(&c) - 11500.0).abs() < 1e-6);
    export_step(&c, "test").unwrap();
    let miss = cut_prism(
        &a,
        &rect(50.0, 0.0, 60.0, 10.0),
        &Frame::XY.offset(-1.0),
        12.0,
    );
    assert!(miss.is_err());
}

#[test]
fn side_face_blind_cut_scenario_b() {
    let b = extrude(&rect(0.0, 0.0, 60.0, 40.0), &Frame::XY, 20.0).unwrap();
    // Side face x = 60: origin (60,0,0), x = +Y, y = +Z, z = +X (outward).
    let f = Frame {
        origin: [60.0, 0.0, 0.0],
        x: [0.0, 1.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [1.0, 0.0, 0.0],
    };
    assert!(plane_has_face(&b, &f));
    let p = rect(15.0, 7.0, 25.0, 13.0);
    let margin = through_all_margin(&b).unwrap();
    // Cutter from `margin` outside the face to 15 mm deep, along -X.
    let start = Frame {
        origin: [60.0 + margin, 0.0, 0.0],
        x: f.x,
        y: [0.0, 0.0, -1.0],
        z: [-1.0, 0.0, 0.0],
    };
    // Profile in the flipped frame: y -> -y.
    let pf = rect(15.0, -13.0, 25.0, -7.0);
    let c = cut_prism(&b, &pf, &start, margin + 15.0).unwrap();
    assert!((volume(&c) - 47100.0).abs() < 1e-6);
    let m = measure(&c).unwrap();
    assert_eq!((m.shell_count, m.all_shells_closed), (1, true));
    assert_eq!((m.bounds.min, m.bounds.max), ([0.0; 3], [60.0, 40.0, 20.0]));
    let _ = p;
    let (lo, hi) = extent_along(&b, &f).unwrap();
    assert!((lo + 60.0).abs() < 1e-9 && hi.abs() < 1e-9);
}

#[test]
fn region_with_holes_extrudes_cuts_and_exports() {
    use std::f64::consts::PI;
    // 80 x 50 plate, Ø10 hole at (20, 25), 10 x 6 slot at [50,60] x [22,28].
    let outer = rect(0.0, 0.0, 80.0, 50.0);
    let holes = [
        Profile::Circle {
            center: [20.0, 25.0],
            radius: 5.0,
        },
        rect(50.0, 22.0, 60.0, 28.0),
    ];
    let b = crate::extrude_region(&outer, &holes, &Frame::XY, 10.0).unwrap();
    let want = (4000.0 - 25.0 * PI - 60.0) * 10.0;
    assert!((volume(&b) - want).abs() < 1e-6 * want, "{}", volume(&b));
    let m = measure(&b).unwrap();
    assert_eq!((m.shell_count, m.all_shells_closed), (1, true));
    // 6 plate faces + 1 cylinder (Truck may split it) + 4 slot walls.
    assert!(m.face_count >= 11, "{}", m.face_count);
    assert!(export_step(&b, "t").is_ok());
}

#[test]
fn chamfer_box_edge_and_refusals() {
    // 40 x 30 x 10 block; top face (z = 10, +Z) and front face (y = 0, -Y).
    let b = block();
    let front = Frame {
        origin: [0.0, 0.0, 0.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [0.0, -1.0, 0.0],
    };
    let c = crate::chamfer_edge(&b, &top(), &front, 2.0).unwrap();
    // Removes a 2 x 2 right-triangle prism along the 40 mm edge: 0.5 * 4 * 40.
    assert!(
        (volume(&c) - (12000.0 - 80.0)).abs() < 1e-6,
        "{}",
        volume(&c)
    );
    let m = measure(&c).unwrap();
    assert_eq!(
        (m.shell_count, m.all_shells_closed, m.face_count),
        (1, true, 7)
    );
    assert!(export_step(&c, "t").is_ok());
    // Parallel faces, too large a distance, non-adjacent faces.
    let bottom = Frame {
        z: [0.0, 0.0, -1.0],
        y: [0.0, -1.0, 0.0],
        ..Frame::XY
    };
    assert!(crate::chamfer_edge(&b, &top(), &bottom, 1.0).is_err());
    assert!(crate::chamfer_edge(&b, &top(), &front, 12.0).is_err());
    let back = Frame {
        origin: [0.0, 30.0, 0.0],
        x: [-1.0, 0.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [0.0, 1.0, 0.0],
    };
    let _ = back;
    // A boss within the chamfer's reach is not cut silently: the removed-volume
    // check refuses it.
    let boss = join_prism(&b, &rect(10.0, 0.5, 20.0, 5.0), &top().offset(-0.5), 3.5).unwrap();
    let e = crate::chamfer_edge(&boss, &top(), &front, 2.0)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("instead of the edge wedge") || e.contains("cannot tell"),
        "{e}"
    );
}

#[test]
#[ignore = "feasibility spike: fillet via a tangent-arc tool"]
fn spike_fillet_tangent_arc_tool() {
    use crate::LoopSegment;
    // Block 40x30x10, top-front edge along x at y=0, z=10. Frame: origin on the
    // edge before x=0, z = +X (edge direction), x = +Y, y = +Z (local (y, z)
    // offsets from the edge point (y=0, z=10)).
    let b = block();
    let r = 2.0;
    let m = 3.0;
    let frame = Frame {
        origin: [-m, 0.0, 10.0],
        x: [0.0, 1.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [1.0, 0.0, 0.0],
    };
    // Local: face1 (top) runs along +x (local), face2 (front) along -y (local).
    // A = (r, 0) on top, B = (0, -r) on front, C = (r, -r). Removed region: corner
    // (0,0)-A-arc-B; tool also extends outward (+y local up, -x local front).
    let segs = vec![
        LoopSegment::Line { start: [0.0, -r] }, // B -> B + out
        LoopSegment::Line { start: [-m, -r] },  // -> corner out
        LoopSegment::Line { start: [-m, m] },   // -> top out
        LoopSegment::Line { start: [r, m] },    // -> above A
        LoopSegment::Arc {
            start: [r, 0.0],
            center: [r, -r],
            ccw: false,
        }, // A -> B (bulging to corner)
    ];
    let p = Profile::Loop(segs);
    let res = crate::build::subtract_prism(&b, &p, &frame, 40.0 + 2.0 * m, "fillet");
    match &res {
        Ok(f) => eprintln!(
            "fillet spike: volume {} (expected {})",
            volume(f),
            12000.0 - (r * r - std::f64::consts::PI * r * r / 4.0) * 40.0
        ),
        Err(e) => eprintln!(
            "fillet spike failed: {}",
            e.to_string().chars().take(300).collect::<String>()
        ),
    }
}

#[test]
fn join_and_cut_refuse_tools_with_inner_loops_up_front() {
    // These used to reach Truck and end as "kernel panicked" inside `guarded`.
    let a = block();
    let outer = rect(10.0, 5.0, 30.0, 25.0);
    let inner = [rect(15.0, 10.0, 25.0, 20.0)];
    let ring = [Profile::Circle {
        center: [20.0, 15.0],
        radius: 4.0,
    }];
    let down = Frame {
        origin: [0.0, 30.0, 12.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, -1.0, 0.0],
        z: [0.0, 0.0, -1.0],
    };
    for (name, r) in [
        ("cut frame", cut_region(&a, &outer, &inner, &down, 14.0)),
        ("cut ring", cut_region(&a, &outer, &ring, &down, 7.0)),
        (
            "join frame",
            join_region(&a, &outer, &inner, &top().offset(-0.5), 5.0),
        ),
    ] {
        let e = r.expect_err(name);
        assert!(matches!(e, KernelError::InvalidInput(_)), "{name}: {e}");
        assert!(e.to_string().contains("inner loop"), "{name}: {e}");
    }
    // Empty holes still go through (the single-loop path).
    let c = cut_region(&a, &outer, &[], &down, 7.0).unwrap();
    assert!((volume(&a) - volume(&c) - 400.0 * 5.0).abs() < 1e-6);
}

// The engulfed-hole cut (run-3 review sketch/kernel #3) is a Truck 0.6 result
// that `cut_prism` still returns; it is detected by `validate::tests::
// swallowed_hole_cut_is_detected` and refused by the document
// (rcad-commands/tests/geometry_gate.rs).
