//! Associative sketch projection (run 6, SESSION_6 §B / coverage SK03):
//! project a model face (all loops, exact lines and arcs) or another sketch
//! into a parallel sketch; upstream edits move the projected geometry;
//! topology changes break the link by name (never a silent rebinding);
//! unlink freezes it; cycles and oblique sources are refused; undo and
//! save/reopen (format v6). Volumes are closed-form.
mod support;
use rcad_commands::Session;
use serde_json::{Value, json};
use std::f64::consts::PI;
use support::tray;

fn run(s: &mut Session, steps: &[Value]) {
    for v in steps {
        tray::ok(s, v);
    }
}

fn body_vol(s: &Session, id: u32) -> f64 {
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

/// F1 rect 40 x 30 centred, F2 extrude 10, F3 circle Ø8 at (10, 0), F4 cut
/// through all; F5 sketch 20 mm above.
fn holed_block(s: &mut Session) {
    run(
        s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 40.0, "height": 30.0}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
                   "diameter": 8.0, "center": [10.0, 0.0]}),
            json!({"cmd": "extrude", "sketch": 3, "op": "cut", "target": 1, "through_all": true}),
            json!({"cmd": "create_sketch",
                   "plane": {"Offset": {"base": "WorldXY", "distance": 20.0}},
                   "template": "polygon", "vertices": [[30.0, 30.0], [31.0, 30.0], [30.0, 31.0]]}),
        ],
    );
}

fn top_face() -> Value {
    json!({"Face": {"feature": 2, "role": "ExtrudeEndCap"}})
}

#[test]
fn projected_face_follows_upstream_edits() {
    let mut s = Session::new();
    holed_block(&mut s);
    // F5 holds a small triangle of its own; project the top face as profile
    // geometry, then extrude the holed rectangle region as a new body.
    tray::ok(
        &mut s,
        &json!({"cmd": "project", "sketch": 5, "source": top_face()}),
    );
    let sk = s.summary().features[4].sketch.clone().unwrap();
    // 4 lines and the hole (one circle or arcs), pinned.
    let regions = sk.regions().unwrap();
    let big = regions
        .iter()
        .max_by(|a, b| a.area.total_cmp(&b.area))
        .unwrap()
        .id
        .clone();
    tray::ok(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 5, "distance": 5.0, "region": big}),
    );
    let want = |w: f64, d: f64| (w * 30.0 - PI * d * d / 4.0) * 5.0;
    assert!(
        (body_vol(&s, 2) - want(40.0, 8.0)).abs() < 1e-6,
        "{}",
        body_vol(&s, 2)
    );
    // Upstream edits: block width and hole diameter.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 1, "name": "width", "value": 50.0}),
    );
    assert!(
        (body_vol(&s, 2) - want(50.0, 8.0)).abs() < 1e-6,
        "{}",
        body_vol(&s, 2)
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "diameter", "value": 10.0}),
    );
    assert!((body_vol(&s, 2) - want(50.0, 10.0)).abs() < 1e-6);
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert!((body_vol(&s, 2) - want(50.0, 8.0)).abs() < 1e-6);
    // The projection is a dependency of the sketch.
    let deps = s.summary().features[4].depends_on.clone();
    assert!(deps.iter().any(|f| f.0 == 2), "{deps:?}");
    // Save / reopen: v6, same document and volume.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(6));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert!((body_vol(&t, 2) - want(50.0, 8.0)).abs() < 1e-6);
}

#[test]
fn topology_change_breaks_the_link_until_unlinked() {
    let mut s = Session::new();
    holed_block(&mut s);
    tray::ok(
        &mut s,
        &json!({"cmd": "project", "sketch": 5, "source": top_face(), "construction": true}),
    );
    // A second hole through the top face (added before the sketch with the
    // timeline marker) changes the face's loops: refused by name.
    tray::ok(&mut s, &json!({"cmd": "set_rollback", "at": 4}));
    tray::ok(
        &mut s,
        &json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
                "diameter": 4.0, "center": [-10.0, 0.0]}),
    );
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 6, "op": "cut", "target": 1, "through_all": true}),
    );
    assert!(r.ok, "{:?}", r.error);
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(&mut s, &json!({"cmd": "set_rollback", "at": null}));
    assert!(
        !r.ok,
        "rolling forward over a changed projection source was accepted"
    );
    let e = format!("{:?}", r.error);
    assert!(
        e.contains("BrokenReference") && e.contains("projection 1"),
        "{e}"
    );
    assert_eq!(s.model().unwrap().document(), &before);
    // Unlink (marker still before the sketch: the link is removed from the
    // definition, the geometry stays), then roll forward.
    tray::ok(
        &mut s,
        &json!({"cmd": "unlink_projection", "sketch": 5, "link": 0}),
    );
    tray::ok(&mut s, &json!({"cmd": "set_rollback", "at": null}));
    assert!(s.summary().features.iter().all(|f| !f.rolled_back));
}

#[test]
fn sketch_projection_matches_by_entity() {
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
                   "diameter": 10.0, "center": [5.0, 5.0]}),
            json!({"cmd": "create_sketch",
                   "plane": {"Offset": {"base": "WorldXY", "distance": 15.0}},
                   "template": "polygon", "vertices": [[30.0, 30.0], [31.0, 30.0], [30.0, 31.0]]}),
            json!({"cmd": "project", "sketch": 2, "source": {"Sketch": 1}}),
        ],
    );
    let sk = s.summary().features[1].sketch.clone().unwrap();
    let circle = sk
        .regions()
        .unwrap()
        .into_iter()
        .max_by(|a, b| a.area.total_cmp(&b.area))
        .unwrap()
        .id;
    tray::ok(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 2, "distance": 3.0, "region": circle}),
    );
    assert!((body_vol(&s, 1) - PI * 25.0 * 3.0).abs() < 1e-6);
    let b = s.summary().bodies[0].bounds.unwrap();
    assert!((b.min[2] - 15.0).abs() < 1e-9 && (b.max[0] - 10.0).abs() < 1e-9);
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 1, "name": "diameter", "value": 12.0}),
    );
    assert!((body_vol(&s, 1) - PI * 36.0 * 3.0).abs() < 1e-6);
}

#[test]
fn cycles_and_oblique_sources_are_refused() {
    let mut s = Session::new();
    holed_block(&mut s);
    tray::ok(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 5, "distance": 2.0}),
    );
    let before = s.model().unwrap().document().clone();
    // F5 projecting the face of F6, which is built from F5: a cycle.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "project", "sketch": 5,
                "source": {"Face": {"feature": 6, "role": "ExtrudeEndCap"}}}),
    );
    assert!(!r.ok);
    // Itself.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "project", "sketch": 5, "source": {"Sketch": 5}}),
    );
    assert!(!r.ok);
    // A side face (normal ±Y / ±X) is not parallel to the XY-parallel sketch.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "project", "sketch": 5,
                "source": {"Face": {"feature": 2, "role": {"ExtrudeSide": {"entity": 1}}}}}),
    );
    assert!(!r.ok);
    assert!(
        format!("{:?}", r.error).contains("parallel"),
        "{:?}",
        r.error
    );
    assert_eq!(s.model().unwrap().document(), &before);
}
