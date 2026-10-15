//! Commands built from a pick carry the model revision the pick was made
//! on (`at_revision`): after an edit has been applied they are refused as a
//! stale pick, with nothing changed, instead of acting on whatever face now
//! has the picked index.
mod support;
use rcad_commands::Session;
use serde_json::json;
use support::tray;

#[test]
fn a_pick_from_before_an_edit_is_refused() {
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 40.0, "height": 20.0}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
    ] {
        tray::ok(&mut s, &v);
    }
    let picked_at = s.model().unwrap().revision;
    let measure = json!({"cmd": "measure_faces", "a": {"body": 1, "face": 0}});
    let guarded = json!({"cmd": "at_revision", "revision": picked_at, "command": measure});
    tray::ok(&mut s, &guarded);
    // An edit lands between the pick and the command.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "distance", "value": 12.0}),
    );
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(&mut s, &guarded);
    assert!(!r.ok);
    let e = format!("{:?}", r.error);
    assert!(
        e.contains("stale pick") && e.contains(&format!("revision {picked_at}")),
        "{e}"
    );
    assert_eq!(s.model().unwrap().document(), &before);
    // Picked again at the current revision: accepted.
    let now = s.model().unwrap().revision;
    tray::ok(
        &mut s,
        &json!({"cmd": "at_revision", "revision": now, "command": measure}),
    );
}
