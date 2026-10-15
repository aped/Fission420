//! History editing: timeline rollback
//! marker (insert at the marker, roll forward), safe reorder (dependency
//! checked, regeneration-checked, one undo step), suppression with
//! dependents, repair of a broken face reference by re-attaching, undo and
//! save/reopen. Volumes are closed-form.
use crate::support;
use rcad_commands::Session;
use serde_json::{Value, json};
use std::f64::consts::PI;
use support::tray;

fn vol(s: &Session) -> f64 {
    tray::volume(s)
}

fn run(s: &mut Session, steps: &[Value]) {
    for v in steps {
        tray::ok(s, v);
    }
}

/// Block 40 x 30 x 10 (F1 sketch, F2 extrude), Ø6 through hole (F3, F4),
/// 10 x 10 pocket 4 deep on the top face (F5, F6).
fn block(s: &mut Session) {
    run(
        s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 40.0, "height": 30.0}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
                   "diameter": 6.0, "center": [-10.0, 0.0]}),
            json!({"cmd": "extrude", "sketch": 3, "op": "cut", "target": 1, "through_all": true}),
            json!({"cmd": "create_sketch",
                   "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
                   "template": "centered_rectangle", "width": 10.0, "height": 10.0,
                   "center": [8.0, 0.0]}),
            json!({"cmd": "extrude", "sketch": 5, "distance": 4.0, "op": "cut", "target": 1}),
        ],
    );
}

const FULL: f64 = 12000.0 - 90.0 * PI - 400.0;

#[test]
fn rollback_marker_inserts_at_the_marker_and_rolls_forward() {
    let mut s = Session::new();
    block(&mut s);
    assert!((vol(&s) - FULL).abs() < 1e-6);
    // Roll back to after the hole: the pocket is not evaluated.
    tray::ok(&mut s, &json!({"cmd": "set_rollback", "at": 4}));
    assert!((vol(&s) - (12000.0 - 90.0 * PI)).abs() < 1e-6);
    let sum = s.summary();
    let rolled: Vec<u32> = sum
        .features
        .iter()
        .filter(|f| f.rolled_back)
        .map(|f| f.id.0)
        .collect();
    assert_eq!(rolled, vec![5, 6]);
    // Features added now go in at the marker: a second through hole Ø4.
    run(
        &mut s,
        &[
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
                   "diameter": 4.0, "center": [-10.0, 8.0]}),
            json!({"cmd": "extrude", "sketch": 7, "op": "cut", "target": 1, "through_all": true}),
        ],
    );
    let order: Vec<u32> = s.summary().features.iter().map(|f| f.id.0).collect();
    assert_eq!(order, vec![1, 2, 3, 4, 7, 8, 5, 6]);
    assert!((vol(&s) - (12000.0 - 90.0 * PI - 40.0 * PI)).abs() < 1e-6);
    // Roll to the end: everything evaluates in the new order.
    tray::ok(&mut s, &json!({"cmd": "set_rollback", "at": null}));
    assert!((vol(&s) - (FULL - 40.0 * PI)).abs() < 1e-6);
    assert!(s.summary().features.iter().all(|f| !f.rolled_back));
    // One undo step each.
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert!((vol(&s) - (12000.0 - 130.0 * PI)).abs() < 1e-6);
    // Save / reopen with the marker set: format v6, marker kept.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(6));
    assert_eq!(v["document"]["rollback"], json!(8));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert!((vol(&t) - (12000.0 - 130.0 * PI)).abs() < 1e-6);
}

#[test]
fn rollback_refuses_editing_into_rolled_back_references() {
    let mut s = Session::new();
    block(&mut s);
    // Marker before the block's extrusion: the pocket sketch's face (F2) is
    // rolled back with it; its body does not exist.
    tray::ok(&mut s, &json!({"cmd": "set_rollback", "at": 1}));
    assert!(s.summary().bodies.is_empty());
    // An unknown marker feature is refused, unchanged.
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(&mut s, &json!({"cmd": "set_rollback", "at": 99}));
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().document(), &before);
}

#[test]
fn reorder_is_dependency_checked_and_regenerated() {
    let mut s = Session::new();
    block(&mut s);
    let before = s.model().unwrap().document().clone();
    // The pocket's extrusion before its own sketch: refused, named.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "move_feature", "feature": 6, "before": 5}),
    );
    assert!(!r.ok);
    assert!(
        format!("{:?}", r.error).contains("depends on"),
        "{:?}",
        r.error
    );
    // The pocket sketch (on F2's top face) before F2: refused.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "move_feature", "feature": 5, "before": 2}),
    );
    assert!(!r.ok);
    // A cut before the extrusion that creates its body: refused.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "move_feature", "feature": 4, "before": 2}),
    );
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().document(), &before);
    // Safe: move the pocket (sketch + cut) before the hole; same solid.
    tray::ok(
        &mut s,
        &json!({"cmd": "move_feature", "feature": 5, "before": 3}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "move_feature", "feature": 6, "before": 3}),
    );
    let order: Vec<u32> = s.summary().features.iter().map(|f| f.id.0).collect();
    assert_eq!(order, vec![1, 2, 5, 6, 3, 4]);
    assert!((vol(&s) - FULL).abs() < 1e-6);
    // Undo restores the order one step at a time.
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert_eq!(s.model().unwrap().document(), &before);
}

#[test]
fn reorder_that_changes_the_result_on_another_orientation() {
    // Join then cut vs cut then join on a side face (World XZ): a boss joined
    // over the hole's position hides the hole when it comes after it.
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXZ", "template": "centered_rectangle",
                   "width": 40.0, "height": 20.0, "center": [0.0, 10.0]}),
            // XZ normal is −Y: the slab occupies y ∈ [−10, 0].
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            // Ø6 through hole along Y at x = 0, z = 10.
            json!({"cmd": "create_sketch", "plane": "WorldXZ", "template": "centered_circle",
                   "diameter": 6.0, "center": [0.0, 10.0]}),
            json!({"cmd": "extrude", "sketch": 3, "op": "cut", "target": 1, "through_all": true}),
            // Ø12 boss 5 long on the front face (y = −10), over the hole.
            json!({"cmd": "create_sketch",
                   "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
                   "template": "centered_circle", "diameter": 12.0, "center": [0.0, 10.0]}),
        ],
    );
    // The end cap keeps the sketch's x axis; its y is world Z here.
    tray::ok(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 5, "distance": 5.0, "op": "join", "target": 1}),
    );
    let hole_then_boss = vol(&s);
    // Boss after the hole: the boss is solid (the hole only went through the
    // slab). Volume: slab − hole + boss.
    let slab = 40.0 * 20.0 * 10.0;
    assert!(
        (hole_then_boss - (slab - 90.0 * PI + 180.0 * PI)).abs() < 1e-6,
        "{hole_then_boss}"
    );
    // Move the through-all hole after the boss: it now goes through both.
    // (Its sketch cannot move after the cut that uses it: cut first.)
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "move_feature", "feature": 3, "before": null}),
    );
    assert!(!r.ok);
    tray::ok(
        &mut s,
        &json!({"cmd": "move_feature", "feature": 4, "before": null}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "move_feature", "feature": 3, "before": 4}),
    );
    let boss_then_hole = vol(&s);
    assert!(
        (boss_then_hole - (slab + 180.0 * PI - 135.0 * PI)).abs() < 1e-6,
        "{boss_then_hole}"
    );
}

#[test]
fn suppression_cascade_and_reference_repair() {
    let mut s = Session::new();
    block(&mut s);
    let before = s.model().unwrap().document().clone();
    // Suppressing the block extrusion alone would leave dependents evaluated.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "set_suppressed", "feature": 2, "suppressed": true}),
    );
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().document(), &before);
    // With dependents: everything that depends on it goes too.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_suppressed", "feature": 2, "suppressed": true,
                "with_dependents": true}),
    );
    assert!(s.summary().bodies.is_empty());
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert!((vol(&s) - FULL).abs() < 1e-6);
    // Repair: re-attach the pocket sketch from the top face to the bottom
    // face (World XY): the pocket now cuts up from the bottom, same volume.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_sketch_plane", "feature": 5, "plane": "WorldXY"}),
    );
    assert!((vol(&s) - FULL).abs() < 1e-6);
    let p = tray::probe(&s, &[[8.0, 0.0, 1.0], [8.0, 0.0, 9.0]]);
    assert_eq!(p, vec![Some(false), Some(true)]);
}
