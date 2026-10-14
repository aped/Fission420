//! Split Body by a plane:
//! a holed block split by world and offset planes on two orientations into
//! two exact solids whose volumes add up; the plane offset is a parameter;
//! upstream edits regenerate both parts; a plane missing the body
//! is refused unchanged, a side that falls apart makes one body per piece;
//! save/reopen (format v6, v8 with pieces).
mod support;
use rcad_commands::Session;
use serde_json::{Value, json};
use std::f64::consts::PI;
use support::tray;

fn vol(s: &Session, id: u32) -> f64 {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id.0 == id)
        .expect("body");
    match b.measurements.expect("measured").volume {
        rcad_kernel::VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn run(s: &mut Session, steps: &[Value]) {
    for v in steps {
        tray::ok(s, v);
    }
}

/// Block 40 x 30 x 10 (x 0..40, y 0..30), Ø8 through hole at (10, 15).
fn block(s: &mut Session) {
    run(
        s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 40.0, "height": 30.0, "center": [20.0, 15.0]}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
                   "diameter": 8.0, "center": [10.0, 15.0]}),
            json!({"cmd": "extrude", "sketch": 3, "op": "cut", "target": 1, "through_all": true}),
        ],
    );
}

#[test]
fn split_by_offset_planes_on_two_orientations() {
    let mut s = Session::new();
    block(&mut s);
    let total = 12000.0 - 160.0 * PI;
    // Horizontal: z = 4 (offset of World XY). Behind (below) stays body 1.
    tray::ok(
        &mut s,
        &json!({"cmd": "split_body", "body": 1, "body_name": "Top",
                "plane": {"Offset": {"base": "WorldXY", "distance": 4.0}}}),
    );
    assert!((vol(&s, 1) - total * 0.4).abs() < 1e-6, "{}", vol(&s, 1));
    assert!((vol(&s, 2) - total * 0.6).abs() < 1e-6);
    let b = s.summary().bodies[1].bounds.unwrap();
    assert!((b.min[2] - 4.0).abs() < 1e-9 && (b.max[2] - 10.0).abs() < 1e-9);
    // The plane offset is a parameter; upstream edit of the block too.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 5, "name": "plane_offset", "value": 7.0}),
    );
    assert!((vol(&s, 1) - total * 0.7).abs() < 1e-6);
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "distance", "value": 20.0}),
    );
    let total20 = 24000.0 - 320.0 * PI;
    assert!((vol(&s, 1) + vol(&s, 2) - total20).abs() < 1e-6);
    assert!((vol(&s, 2) - total20 * 13.0 / 20.0).abs() < 1e-6);
    // Vertical: World YZ offset 20 (x = 20) through the second part: the
    // hole is all on the x < 20 side.
    tray::ok(
        &mut s,
        &json!({"cmd": "split_body", "body": 2,
                "plane": {"Offset": {"base": "WorldYZ", "distance": 20.0}}}),
    );
    let h = 13.0;
    assert!(
        (vol(&s, 2) - (600.0 - 16.0 * PI) * h).abs() < 1e-6,
        "{}",
        vol(&s, 2)
    );
    assert!((vol(&s, 3) - 600.0 * h).abs() < 1e-6);
    // Save / reopen.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(6));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert_eq!(t.summary().bodies.len(), 3);
}

#[test]
fn split_refusals_leave_the_document_unchanged() {
    let mut s = Session::new();
    block(&mut s);
    let before = s.model().unwrap().document().clone();
    // Above the body.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "split_body", "body": 1,
                "plane": {"Offset": {"base": "WorldXY", "distance": 15.0}}}),
    );
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().document(), &before);
}

/// A U-shaped body split across both arms: the side behind the plane falls
/// apart into the two arm tops, which become separate bodies (one per piece,
/// as Fusion does), in the same undo step; save/reopen as format v8.
#[test]
fn a_side_that_falls_apart_makes_one_body_per_piece() {
    let mut u = Session::new();
    run(
        &mut u,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "polygon",
                   "vertices": [[0.0, 0.0], [30.0, 0.0], [30.0, 30.0], [20.0, 30.0],
                                [20.0, 10.0], [10.0, 10.0], [10.0, 30.0], [0.0, 30.0]]}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 5.0}),
        ],
    );
    let before = u.model().unwrap().document().clone();
    let r = tray::exec(
        &mut u,
        &json!({"cmd": "split_body", "body": 1,
                "plane": {"Offset": {"base": "WorldXZ", "distance": -20.0}}}),
    );
    assert!(r.ok, "{:?}", r.error);
    // Behind (y > 20): left arm top (body 1) and right arm top (body 3);
    // in front: the base with the lower arms (body 2), area 900 - 200 - 200.
    let ids: Vec<u32> = u.summary().bodies.iter().map(|b| b.id.0).collect();
    assert_eq!(ids, vec![1, 2, 3]);
    assert!((vol(&u, 1) - 500.0).abs() < 1e-6, "{}", vol(&u, 1));
    assert!((vol(&u, 2) - 2500.0).abs() < 1e-6, "{}", vol(&u, 2));
    assert!((vol(&u, 3) - 500.0).abs() < 1e-6, "{}", vol(&u, 3));
    let b3 = u.summary().bodies[2].bounds.unwrap();
    assert!((b3.min[0] - 20.0).abs() < 1e-9 && (b3.min[1] - 20.0).abs() < 1e-9);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("u.rcad.json");
    tray::ok(&mut u, &json!({"cmd": "save", "path": path}));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(8));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": path}));
    assert_eq!(t.summary().bodies.len(), 3);
    assert!(u.execute(&rcad_commands::Command::Undo).ok);
    assert_eq!(u.model().unwrap().document(), &before);
}

/// The drawing-set plate (80 × 50 × 10, Ø8 hole at (22, 25), Ø6 at
/// (60, 25)) split by the plane x = 20, which crosses the Ø8 hole 2 mm off
/// its axis. Truck's boolean panics on this cut; the half-space tool extruded
/// along an in-plane axis parallel to the holes (Z) uses the exact slab
/// construction. Left part: (20·50 − (16π/3 − 4√3))·10 (the circular segment
/// of the hole left of a chord 2 mm from its centre).
#[test]
fn split_through_a_hole_off_its_axis() {
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 80.0, "height": 50.0, "center": [40.0, 25.0]}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            json!({"cmd": "hole", "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
                   "center": [22.0, 25.0], "diameter": 8.0}),
            json!({"cmd": "hole", "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
                   "center": [60.0, 25.0], "diameter": 6.0}),
        ],
    );
    let total = (4000.0 - PI * (16.0 + 9.0)) * 10.0;
    tray::ok(
        &mut s,
        &json!({"cmd": "split_body", "body": 1, "body_name": "Right",
                "plane": {"Offset": {"base": "WorldYZ", "distance": 20.0}}}),
    );
    let left = (1000.0 - (16.0 * PI / 3.0 - 4.0 * 3f64.sqrt())) * 10.0;
    assert!((vol(&s, 1) - left).abs() < 1e-6, "{} vs {left}", vol(&s, 1));
    assert!((vol(&s, 2) - (total - left)).abs() < 1e-6, "{}", vol(&s, 2));
}
