//! A cut that separates its target into pieces makes one body per piece:
//! the first piece (lowest bounding-box corner) stays the target, the others
//! become new bodies assigned in the same transaction, including when an
//! upstream edit makes an existing cut split the body. Closed-form volumes,
//! undo/redo, a downstream Join on a new piece, save/reopen (format v8), a
//! three-piece cut, and an enclosed void still refused unchanged.
mod support;
use rcad_commands::Session;
use rcad_document::FeatureKind;
use serde_json::{Value, json};
use support::tray;

fn vol(s: &Session, id: u32) -> f64 {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id.0 == id)
        .unwrap_or_else(|| panic!("body {id}"));
    match b.measurements.expect("measured").volume {
        rcad_kernel::VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn bodies(s: &Session) -> Vec<u32> {
    s.summary().bodies.iter().map(|b| b.id.0).collect()
}

fn pieces(s: &Session, feature: u32) -> Vec<u32> {
    let d = s.model().unwrap().document();
    match &d.feature(rcad_document::FeatureId(feature)).unwrap().kind {
        FeatureKind::Extrude { pieces, .. } => pieces.iter().map(|b| b.0).collect(),
        k => panic!("{k:?}"),
    }
}

fn run(s: &mut Session, steps: &[Value]) {
    for v in steps {
        tray::ok(s, v);
    }
}

/// Block 40 x 30 x 10 (x 0..40, y 0..30) with a 4 mm wide through slot at
/// x 18..22 of length `h` centred on y = 15 (feature 3 is its sketch).
fn slotted_block(s: &mut Session, h: f64) {
    run(
        s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 40.0, "height": 30.0, "center": [20.0, 15.0]}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 4.0, "height": h, "center": [20.0, 15.0]}),
            json!({"cmd": "extrude", "sketch": 3, "op": "cut", "target": 1, "through_all": true}),
        ],
    );
}

#[test]
fn a_cut_across_the_whole_part_makes_one_body_per_piece() {
    let mut s = Session::new();
    slotted_block(&mut s, 20.0);
    assert_eq!(bodies(&s), vec![1]);
    assert!((vol(&s, 1) - (12000.0 - 4.0 * 20.0 * 10.0)).abs() < 1e-6);
    // Upstream edit: the slot now crosses the whole width (y -1..31).
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "height", "value": 32.0}),
    );
    assert_eq!(bodies(&s), vec![1, 2]);
    assert_eq!(pieces(&s, 4), vec![2]);
    assert!(
        (vol(&s, 1) - 18.0 * 30.0 * 10.0).abs() < 1e-6,
        "{}",
        vol(&s, 1)
    );
    assert!((vol(&s, 2) - 18.0 * 30.0 * 10.0).abs() < 1e-6);
    let b1 = s.summary().bodies[0].bounds.unwrap();
    let b2 = s.summary().bodies[1].bounds.unwrap();
    assert!((b1.min[0] - 0.0).abs() < 1e-9 && (b1.max[0] - 18.0).abs() < 1e-9);
    assert!((b2.min[0] - 22.0).abs() < 1e-9 && (b2.max[0] - 40.0).abs() < 1e-9);
    assert_eq!(s.summary().bodies[1].name, "Body2");
    // Undo restores one body and no assigned piece; redo both.
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert_eq!(bodies(&s), vec![1]);
    assert!(pieces(&s, 4).is_empty());
    tray::ok(&mut s, &json!({"cmd": "redo"}));
    assert_eq!(bodies(&s), vec![1, 2]);
    // The new piece is an ordinary body: join a 4 x 4 post to it (sketched
    // on its bottom face's plane, so it grows away from the material).
    run(
        &mut s,
        &[
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 4.0, "height": 4.0, "center": [30.0, 15.0]}),
            json!({"cmd": "extrude", "sketch": 5, "distance": 15.0, "op": "join", "target": 2}),
        ],
    );
    assert!(
        (vol(&s, 2) - (5400.0 + 16.0 * 15.0)).abs() < 1e-6,
        "{}",
        vol(&s, 2)
    );
    assert!((s.summary().bodies[1].bounds.unwrap().min[2] + 15.0).abs() < 1e-9);
    assert!((vol(&s, 1) - 5400.0).abs() < 1e-6);
    // Save / reopen: format v8, same document and bodies.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("split.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(8));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert_eq!(bodies(&t), vec![1, 2]);
    assert!((vol(&t, 2) - 5640.0).abs() < 1e-6);
}

#[test]
fn a_shorter_slot_rejoins_the_part_and_the_join_on_the_piece_then_fails_clearly() {
    let mut s = Session::new();
    slotted_block(&mut s, 32.0);
    assert_eq!(bodies(&s), vec![1, 2]);
    // Shortening the slot again: one piece; the assigned id has no body.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "height", "value": 20.0}),
    );
    assert_eq!(bodies(&s), vec![1]);
    assert!((vol(&s, 1) - 11200.0).abs() < 1e-6);
    assert_eq!(pieces(&s, 4), vec![2], "the id stays assigned");
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "height", "value": 32.0}),
    );
    assert_eq!(bodies(&s), vec![1, 2]);
    // A Join on piece 2, then an edit that removes piece 2: refused, unchanged.
    run(
        &mut s,
        &[
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 4.0, "height": 4.0, "center": [30.0, 15.0]}),
            json!({"cmd": "extrude", "sketch": 5, "distance": 15.0, "op": "join", "target": 2}),
        ],
    );
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "height", "value": 20.0}),
    );
    assert!(!r.ok, "a join on a vanished piece was accepted");
    assert_eq!(s.model().unwrap().document(), &before);
}

#[test]
fn a_cut_across_both_arms_of_a_u_makes_three_pieces() {
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "polygon",
                   "vertices": [[0.0, 0.0], [30.0, 0.0], [30.0, 30.0], [20.0, 30.0],
                                [20.0, 10.0], [10.0, 10.0], [10.0, 30.0], [0.0, 30.0]]}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 5.0}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 32.0, "height": 5.0, "center": [15.0, 22.5]}),
            json!({"cmd": "extrude", "sketch": 3, "op": "cut", "target": 1, "through_all": true}),
        ],
    );
    assert_eq!(bodies(&s), vec![1, 2, 3]);
    assert_eq!(pieces(&s, 4), vec![2, 3]);
    // Base with the lower arms (area 900 - 200 - 100 - 100), then the left
    // and right arm tops (10 x 5 each), all 5 thick.
    assert!((vol(&s, 1) - 500.0 * 5.0).abs() < 1e-6, "{}", vol(&s, 1));
    assert!((vol(&s, 2) - 250.0).abs() < 1e-6);
    assert!((vol(&s, 3) - 250.0).abs() < 1e-6);
    assert!(s.summary().bodies[1].bounds.unwrap().max[0] < 10.0 + 1e-9);
    assert!(s.summary().bodies[2].bounds.unwrap().min[0] > 20.0 - 1e-9);
    // One undo step removes all three bodies' split.
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert_eq!(bodies(&s), vec![1]);
}

#[test]
fn an_enclosed_void_is_still_refused_unchanged() {
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 40.0, "height": 30.0, "center": [20.0, 15.0]}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            json!({"cmd": "create_sketch", "plane": {"Offset": {"base": "WorldXY", "distance": 10.0}},
                   "template": "centered_rectangle", "width": 10.0, "height": 10.0,
                   "center": [15.0, 15.0]}),
            json!({"cmd": "extrude", "sketch": 3, "distance": 4.0, "op": "cut", "target": 1}),
        ],
    );
    assert!(
        (vol(&s, 1) - (12000.0 - 400.0)).abs() < 1e-6,
        "{}",
        vol(&s, 1)
    );
    let before = s.model().unwrap().document().clone();
    run(
        &mut s,
        &[
            json!({"cmd": "create_sketch", "plane": {"Offset": {"base": "WorldXY", "distance": 10.0}},
                 "template": "centered_rectangle", "width": 20.0, "height": 20.0,
                 "center": [15.0, 15.0]}),
        ],
    );
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 5, "distance": 2.0, "op": "join", "target": 1}),
    );
    assert!(!r.ok, "a closed cavity was accepted");
    let msg = format!("{:?}", r.error);
    assert!(msg.contains("void"), "{msg}");
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert_eq!(s.model().unwrap().document(), &before);
}
