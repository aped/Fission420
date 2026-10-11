//! Join / general cut / face-region tests. Expected values are analytic.
use super::*;
use crate::{VolumeResult, export_step, measure};

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
