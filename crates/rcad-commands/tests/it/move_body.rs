//! Body Move / Copy:
//! rigid moves keep face references made before the move (sketches, fillets
//! and holes land on the moved body), copies are independent of later edits
//! of the source but follow upstream edits before the copy, patterns seeded
//! before a move are refused, undo and save/reopen (format v6).
use crate::support;
use rcad_commands::Session;
use serde_json::{Value, json};
use std::f64::consts::PI;
use support::tray;

fn run(s: &mut Session, steps: &[Value]) {
    for v in steps {
        tray::ok(s, v);
    }
}

fn body(s: &Session, id: u32) -> rcad_commands::BodySummary {
    s.summary()
        .bodies
        .into_iter()
        .find(|b| b.id.0 == id)
        .expect("body")
}

fn vol(s: &Session, id: u32) -> f64 {
    match body(s, id).measurements.expect("measured").volume {
        rcad_kernel::VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn near(a: [f64; 3], b: [f64; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < 1e-6)
}

/// F1 rect 40 x 30 at (20, 15), F2 extrude 10.
fn block(s: &mut Session) {
    run(
        s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 40.0, "height": 30.0, "center": [20.0, 15.0]}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
        ],
    );
}

#[test]
fn moved_body_keeps_its_face_references() {
    let mut s = Session::new();
    block(&mut s);
    // Rotate 90° about Z through the origin, then +100 X.
    tray::ok(
        &mut s,
        &json!({"cmd": "move_body", "body": 1, "translate": [100.0, 0.0, 0.0],
                "angle_deg": 90.0}),
    );
    let b = body(&s, 1).bounds.unwrap();
    assert!(
        near(b.min, [70.0, 0.0, 0.0]) && near(b.max, [100.0, 40.0, 10.0]),
        "{b:?}"
    );
    // A pocket sketched on the (moved) top face, centred on the face frame's
    // own coordinates (20, 15) = the block's centre.
    run(
        &mut s,
        &[
            json!({"cmd": "create_sketch",
                   "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
                   "template": "centered_circle", "diameter": 10.0, "center": [20.0, 15.0]}),
            json!({"cmd": "extrude", "sketch": 4, "distance": 4.0, "op": "cut", "target": 1}),
        ],
    );
    assert!((vol(&s, 1) - (12000.0 - 100.0 * PI)).abs() < 1e-6);
    // The pocket is at the moved centre (85, 20).
    let p = tray::probe(
        &s,
        &[[85.0, 20.0, 9.0], [85.0, 20.0, 5.0], [20.0, 15.0, 9.0]],
    );
    assert_eq!(p, vec![Some(false), Some(true), Some(false)]);
    // A fillet on two side faces of F2 (made before the move).
    tray::ok(
        &mut s,
        &json!({"cmd": "fillet", "a": {"feature": 2, "role": {"ExtrudeSide": {"entity": 1}}},
                "b": {"feature": 2, "role": {"ExtrudeSide": {"entity": 2}}}, "radius": 3.0}),
    );
    let want = 12000.0 - 100.0 * PI - (9.0 - 9.0 * PI / 4.0) * 10.0;
    assert!((vol(&s, 1) - want).abs() < 1e-6, "{}", vol(&s, 1));
    // The move is a parameter: shift +5 Y; everything follows.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "ty", "value": 5.0}),
    );
    assert!((vol(&s, 1) - want).abs() < 1e-6);
    let p = tray::probe(&s, &[[85.0, 25.0, 9.0], [85.0, 25.0, 5.0]]);
    assert_eq!(p, vec![Some(false), Some(true)]);
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    // Save / reopen.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(6));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert!((vol(&t, 1) - want).abs() < 1e-6);
}

#[test]
fn copy_is_independent_of_later_edits_but_follows_upstream() {
    let mut s = Session::new();
    block(&mut s);
    tray::ok(
        &mut s,
        &json!({"cmd": "move_body", "body": 1, "translate": [0.0, 50.0, 0.0], "copy": true,
                "body_name": "Spare"}),
    );
    assert_eq!(s.summary().bodies.len(), 2);
    assert!(near(body(&s, 2).bounds.unwrap().min, [0.0, 50.0, 0.0]));
    // A hole in the original after the copy: the copy keeps its material.
    run(
        &mut s,
        &[
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
                   "diameter": 6.0, "center": [20.0, 15.0]}),
            json!({"cmd": "extrude", "sketch": 4, "op": "cut", "target": 1, "through_all": true}),
        ],
    );
    assert!((vol(&s, 1) - (12000.0 - 90.0 * PI)).abs() < 1e-6);
    assert!((vol(&s, 2) - 12000.0).abs() < 1e-6);
    // Upstream (before the copy): both change.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "distance", "value": 20.0}),
    );
    assert!((vol(&s, 2) - 24000.0).abs() < 1e-6);
    assert!((vol(&s, 1) - (24000.0 - 180.0 * PI)).abs() < 1e-6);
    // The copy is a body creator: deleting the source's extrusion is refused
    // while the copy uses it.
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(&mut s, &json!({"cmd": "delete_feature", "feature": 2}));
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().document(), &before);
}

#[test]
fn moves_refuse_seeds_made_before_them() {
    let mut s = Session::new();
    block(&mut s);
    run(
        &mut s,
        &[
            json!({"cmd": "create_sketch",
                   "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
                   "template": "centered_circle", "diameter": 4.0, "center": [5.0, 15.0]}),
            json!({"cmd": "extrude", "sketch": 3, "distance": 2.0, "op": "cut", "target": 1}),
            json!({"cmd": "move_body", "body": 1, "translate": [0.0, 0.0, 30.0]}),
        ],
    );
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "linear_pattern", "feature": 4, "count": 3, "spacing": 10.0,
                "direction": [1.0, 0.0]}),
    );
    assert!(!r.ok);
    assert!(format!("{:?}", r.error).contains("moved"), "{:?}", r.error);
    assert_eq!(s.model().unwrap().document(), &before);
    // Invalid input: zero axis with an angle.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "move_body", "body": 1, "axis": [0.0, 0.0, 0.0], "angle_deg": 30.0}),
    );
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().document(), &before);
}

#[test]
fn scale_keeps_references_and_scales_volume() {
    let mut s = Session::new();
    block(&mut s);
    // Scale 1.5 about the origin; then a pocket on the (scaled) top face at
    // face coordinates (20, 15): the face frame moved with the body (origin
    // scaled, axes unit), sketch coordinates stay in mm, so the pocket is at
    // world (20, 15) on the new top z = 15.
    tray::ok(
        &mut s,
        &json!({"cmd": "scale_body", "body": 1, "factor": 1.5}),
    );
    assert!((vol(&s, 1) - 12000.0 * 3.375).abs() < 1e-6);
    assert!(near(body(&s, 1).bounds.unwrap().max, [60.0, 45.0, 15.0]));
    run(
        &mut s,
        &[
            json!({"cmd": "create_sketch",
                   "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
                   "template": "centered_circle", "diameter": 10.0, "center": [20.0, 15.0]}),
            json!({"cmd": "extrude", "sketch": 4, "distance": 4.0, "op": "cut", "target": 1}),
        ],
    );
    let p = tray::probe(
        &s,
        &[[20.0, 15.0, 14.0], [20.0, 15.0, 10.0], [30.0, 22.5, 14.0]],
    );
    assert_eq!(p, vec![Some(false), Some(true), Some(true)]);
    // The factor is a parameter; invalid factors refused.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "factor", "value": 2.0}),
    );
    assert!(
        (vol(&s, 1) - (96000.0 - 100.0 * PI)).abs() < 1e-6,
        "{}",
        vol(&s, 1)
    );
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "factor", "value": 0.0}),
    );
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().document(), &before);
}

#[test]
fn align_a_body_onto_another_face_follows_upstream() {
    // Body 1: block 40 x 30 x 10. Body 2: a 20 x 10 plate drawn on World XZ
    // (normal −Y), extruded 5: its start cap faces +Y. Align that start cap
    // onto body 1's top face: body 2 now sits on z = 10, 5 thick.
    let mut s = Session::new();
    block(&mut s);
    run(
        &mut s,
        &[
            json!({"cmd": "create_sketch", "plane": "WorldXZ", "template": "centered_rectangle",
                   "width": 20.0, "height": 10.0, "center": [100.0, 50.0]}),
            json!({"cmd": "extrude", "sketch": 3, "distance": 5.0, "body_name": "Plate"}),
            json!({"cmd": "align_body", "from": {"feature": 4, "role": "ExtrudeStartCap"},
                   "to": {"feature": 2, "role": "ExtrudeEndCap"}}),
        ],
    );
    assert!((vol(&s, 2) - 1000.0).abs() < 1e-6);
    let b = body(&s, 2).bounds.unwrap();
    assert!(
        (b.min[2] - 10.0).abs() < 1e-9 && (b.max[2] - 15.0).abs() < 1e-9,
        "{b:?}"
    );
    // Upstream: the block grows to 15; the plate follows. Offset 2 lifts it.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "distance", "value": 15.0}),
    );
    assert!((body(&s, 2).bounds.unwrap().min[2] - 15.0).abs() < 1e-9);
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 5, "name": "offset", "value": 2.0}),
    );
    assert!((body(&s, 2).bounds.unwrap().min[2] - 17.0).abs() < 1e-9);
    // Faces of one body: refused.
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "align_body", "from": {"feature": 2, "role": "ExtrudeStartCap"},
                "to": {"feature": 2, "role": "ExtrudeEndCap"}}),
    );
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().document(), &before);
}
