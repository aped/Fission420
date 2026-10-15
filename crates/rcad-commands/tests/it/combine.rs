//! Body Combine: join / cut / intersect of existing
//! bodies with closed-form volumes, keeping or consuming the tools, a
//! coplanar-touching case (exact or refused, never wrong), upstream edits,
//! undo and save/reopen (format v6), refusals leaving the document unchanged.
use crate::support;
use rcad_commands::{Response, Session};
use serde_json::{Value, json};
use std::f64::consts::PI;
use support::tray;

fn ok(s: &mut Session, v: Value) -> Response {
    tray::ok(s, &v)
}

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

fn body_ids(s: &Session) -> Vec<u32> {
    s.summary().bodies.iter().map(|b| b.id.0).collect()
}

/// Body 1: box x [0, 40], y [0, 30], z [0, 20] (F1 sketch, F2 extrude).
/// Body 2: the same box moved by (20, 10, 5) (F3 sketch, F4 extrude, F5 move):
/// no coplanar faces; overlap 20 × 20 × 15 = 6000 mm³.
fn two_boxes(s: &mut Session) {
    ok(s, json!({"cmd": "new_document"}));
    for _ in 0..2 {
        let f = s.summary().features.len() as u32 + 1;
        ok(
            s,
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 40.0, "height": 30.0, "center": [20.0, 15.0]}),
        );
        ok(s, json!({"cmd": "extrude", "sketch": f, "distance": 20.0}));
    }
    ok(
        s,
        json!({"cmd": "move_body", "body": 2, "translate": [20.0, 10.0, 5.0]}),
    );
}

const V_BOX: f64 = 24_000.0;
const V_OVERLAP: f64 = 6_000.0;

fn combine(s: &mut Session, op: &str, keep: bool) -> Response {
    tray::exec(
        s,
        &json!({"cmd": "combine", "target": 1, "tools": [2], "op": op, "keep_tools": keep}),
    )
}

#[test]
fn join_cut_intersect_have_closed_form_volumes() {
    for (op, want) in [
        ("join", 2.0 * V_BOX - V_OVERLAP),
        ("cut", V_BOX - V_OVERLAP),
        ("intersect", V_OVERLAP),
    ] {
        let mut s = Session::new();
        two_boxes(&mut s);
        let r = combine(&mut s, op, false);
        assert!(r.ok, "{op}: {:?}", r.error);
        let v = vol(&s, 1);
        eprintln!("{op}: V = {v:.9} (want {want})");
        assert!((v - want).abs() < 1e-6, "{op}: {v} vs {want}");
        // The tool is consumed: one body left; its creator stays in history.
        assert_eq!(body_ids(&s), vec![1], "{op}");
        let f = s.summary().features.last().unwrap().clone();
        assert_eq!(f.kind, "combine");
        assert!(
            f.notes.iter().any(|n| n.contains(" 0 mismatches")),
            "{op}: {:?}",
            f.notes
        );
        assert_eq!(s.summary().features.len(), 6);
    }
}

#[test]
fn keep_tools_keeps_the_tool_body_unchanged() {
    let mut s = Session::new();
    two_boxes(&mut s);
    let r = combine(&mut s, "cut", true);
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(body_ids(&s), vec![1, 2]);
    assert!((vol(&s, 1) - (V_BOX - V_OVERLAP)).abs() < 1e-6);
    assert!((vol(&s, 2) - V_BOX).abs() < 1e-6);
    // The kept tool can be used again.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "move_body", "body": 2, "translate": [100.0, 0.0, 0.0]}),
    );
    assert!(r.ok, "{:?}", r.error);
}

#[test]
fn consumed_tool_cannot_be_used_later_and_refusals_change_nothing() {
    let mut s = Session::new();
    two_boxes(&mut s);
    let before = s.model().unwrap().document().clone();
    for bad in [
        json!({"cmd": "combine", "target": 1, "tools": [], "op": "join"}),
        json!({"cmd": "combine", "target": 1, "tools": [1], "op": "join"}),
        json!({"cmd": "combine", "target": 1, "tools": [2, 2], "op": "join"}),
        json!({"cmd": "combine", "target": 1, "tools": [7], "op": "join"}),
    ] {
        let r = tray::exec(&mut s, &bad);
        assert!(!r.ok, "{bad}");
        assert_eq!(s.model().unwrap().document(), &before, "{bad}");
    }
    assert!(combine(&mut s, "join", false).ok);
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "move_body", "body": 2, "translate": [1.0, 0.0, 0.0]}),
    );
    assert!(!r.ok, "a consumed body must not be movable");
    let e = format!("{:?}", r.error);
    assert!(e.contains("does not exist"), "{e}");
}

#[test]
fn disjoint_tool_is_refused() {
    let mut s = Session::new();
    two_boxes(&mut s);
    ok(
        &mut s,
        json!({"cmd": "move_body", "body": 2, "translate": [200.0, 0.0, 0.0]}),
    );
    let before = s.model().unwrap().document().clone();
    for op in ["join", "cut", "intersect"] {
        let r = combine(&mut s, op, false);
        assert!(!r.ok, "{op}");
        assert_eq!(s.model().unwrap().document(), &before);
    }
}

#[test]
fn coplanar_touching_boxes_join_exactly_or_refuse() {
    let mut s = Session::new();
    two_boxes(&mut s);
    // Replace the move: body 2 now touches body 1 along the plane x = 40 with
    // coincident top/bottom/side planes.
    ok(&mut s, json!({"cmd": "undo"}));
    ok(
        &mut s,
        json!({"cmd": "move_body", "body": 2, "translate": [40.0, 0.0, 0.0]}),
    );
    let before = s.model().unwrap().document().clone();
    let r = combine(&mut s, "join", false);
    if r.ok {
        let v = vol(&s, 1);
        eprintln!("coplanar join accepted: V = {v}");
        assert!((v - 2.0 * V_BOX).abs() < 1e-6, "{v}");
        assert_eq!(body_ids(&s), vec![1]);
    } else {
        eprintln!("coplanar join refused: {:?}", r.error);
        assert_eq!(s.model().unwrap().document(), &before);
        assert!(r.error.is_some());
    }
    // A cut of a box that only touches removes nothing: never a silent no-op
    // that claims success with a changed body, and never a wrong volume.
    let r = combine(&mut s, "cut", true);
    if r.ok {
        let v = vol(&s, 1);
        assert!(
            (v - V_BOX).abs() < 1e-6 || (v - 2.0 * V_BOX).abs() < 1e-6,
            "{v}"
        );
    }
}

#[test]
fn cylinder_cut_upstream_edit_undo_and_reopen() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    ok(
        &mut s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 40.0, "height": 30.0, "center": [20.0, 15.0]}),
    );
    ok(
        &mut s,
        json!({"cmd": "extrude", "sketch": 1, "distance": 20.0}),
    );
    // Ø10 cylinder at (20, 15), z from -5 to 25 (through the box).
    ok(
        &mut s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
               "diameter": 10.0, "center": [20.0, 15.0]}),
    );
    ok(
        &mut s,
        json!({"cmd": "extrude", "sketch": 3, "distance": 30.0}),
    );
    ok(
        &mut s,
        json!({"cmd": "move_body", "body": 2, "translate": [0.0, 0.0, -5.0]}),
    );
    let r = combine(&mut s, "cut", false);
    assert!(r.ok, "{:?}", r.error);
    let hole = PI * 25.0 * 20.0;
    assert!((vol(&s, 1) - (V_BOX - hole)).abs() < 1e-6, "{}", vol(&s, 1));
    // Membership: the hole axis is empty, the corner is material.
    let p = tray::probe(&s, &[[20.0, 15.0, 10.0], [2.0, 2.0, 10.0]]);
    assert_eq!(p, vec![Some(false), Some(true)]);
    // Upstream edit: a wider box (60) regenerates the combine.
    ok(
        &mut s,
        json!({"cmd": "set_parameter", "feature": 1, "name": "width", "value": 60.0}),
    );
    assert!(
        (vol(&s, 1) - (36_000.0 - hole)).abs() < 1e-6,
        "{}",
        vol(&s, 1)
    );
    // Undo the edit, then the combine: two bodies again.
    ok(&mut s, json!({"cmd": "undo"}));
    assert!((vol(&s, 1) - (V_BOX - hole)).abs() < 1e-6);
    ok(&mut s, json!({"cmd": "undo"}));
    assert_eq!(body_ids(&s), vec![1, 2]);
    assert!((vol(&s, 1) - V_BOX).abs() < 1e-6);
    ok(&mut s, json!({"cmd": "redo"}));
    assert_eq!(body_ids(&s), vec![1]);
    // Save / reopen (v6).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("combine.rcad.json");
    ok(&mut s, json!({"cmd": "save", "path": path}));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(6));
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert!((vol(&t, 1) - (V_BOX - hole)).abs() < 1e-6);
    assert_eq!(body_ids(&t), vec![1]);
    // Deleting the tool's creator is refused while the combine uses it.
    let r = tray::exec(&mut t, &json!({"cmd": "delete_feature", "feature": 4}));
    assert!(!r.ok);
}

#[test]
fn join_mass_properties_follow_the_combined_body() {
    let mut s = Session::new();
    two_boxes(&mut s);
    assert!(combine(&mut s, "join", false).ok);
    ok(
        &mut s,
        json!({"cmd": "set_body_material", "body": 1, "density": 1.0}),
    );
    let r = ok(&mut s, json!({"cmd": "mass_properties"}))
        .mass_properties
        .unwrap();
    let p = r.bodies[0].properties.clone().unwrap();
    // Centroid by inclusion-exclusion of the boxes and their overlap.
    let (ca, cb, co) = ([20.0, 15.0, 10.0], [40.0, 25.0, 15.0], [30.0, 20.0, 12.5]);
    let v = 2.0 * V_BOX - V_OVERLAP;
    for i in 0..3 {
        let want = (V_BOX * ca[i] + V_BOX * cb[i] - V_OVERLAP * co[i]) / v;
        assert!(
            (p.centroid[i] - want).abs() < 1e-9,
            "{:?} {want}",
            p.centroid
        );
    }
    assert!((r.total.unwrap().mass - v * 1e-3).abs() < 1e-9);
}
