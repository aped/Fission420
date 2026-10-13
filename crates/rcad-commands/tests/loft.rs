//! Exact loft as a feature (run 5, ADR 0008): new body between two sketches
//! on parallel planes. Expected values: prismatoid V = h/6 (A1 + 4 Am + A2)
//! (exact for straight-edged lofts between parallel planes) and the cone
//! frustum π h/3 (R² + R r + r²).
mod support;
use rcad_commands::Session;
use rcad_document::DocError;
use serde_json::json;
use std::f64::consts::PI;
use support::tray;

fn rect_on(z: f64, w: f64, h: f64) -> serde_json::Value {
    let plane = if z == 0.0 {
        json!("WorldXY")
    } else {
        json!({"Offset": {"base": "WorldXY", "distance": z}})
    };
    json!({"cmd": "create_sketch", "plane": plane, "template": "centered_rectangle",
           "width": w, "height": h, "center": [0.0, 0.0]})
}

fn prismatoid(h: f64, a: (f64, f64), b: (f64, f64)) -> f64 {
    let m = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    h / 6.0 * (a.0 * a.1 + 4.0 * m.0 * m.1 + b.0 * b.1)
}

#[test]
fn tapered_container_blank_regenerates_and_takes_a_pocket() {
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        rect_on(0.0, 60.0, 40.0),
        rect_on(30.0, 70.0, 50.0),
        json!({"cmd": "loft", "a": 1, "b": 2, "body_name": "Tapered container"}),
    ] {
        tray::ok(&mut s, &v);
    }
    let want = prismatoid(30.0, (60.0, 40.0), (70.0, 50.0));
    assert!((want - 88000.0).abs() < 1e-9);
    assert!(
        (tray::volume(&s) - want).abs() < 1e-6,
        "{}",
        tray::volume(&s)
    );
    // Upstream edit of the top sketch regenerates the loft.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "width", "value": 75.0}),
    );
    assert!((tray::volume(&s) - prismatoid(30.0, (60.0, 40.0), (75.0, 50.0))).abs() < 1e-6);
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    // A pocket 50 x 30, 25 deep from the top plane: a tapered container.
    for v in [
        rect_on(30.0, 50.0, 30.0),
        json!({"cmd": "extrude", "sketch": 4, "distance": 25.0, "op": "cut", "target": 1}),
    ] {
        tray::ok(&mut s, &v);
    }
    assert!((tray::volume(&s) - (88000.0 - 50.0 * 30.0 * 25.0)).abs() < 1e-6);
    let p = tray::probe(&s, &[[0.0, 0.0, 10.0], [0.0, 0.0, 3.0], [29.0, 0.0, 15.0]]);
    assert_eq!(p, vec![Some(false), Some(true), Some(true)]);
    // Save: lofts are format v5.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("l.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(5));
    let mut s2 = Session::new();
    tray::ok(&mut s2, &json!({"cmd": "open", "path": path}));
    assert_eq!(
        s2.model().unwrap().document(),
        s.model().unwrap().document()
    );
}

#[test]
fn circle_frustum_and_refusals() {
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
               "diameter": 40.0, "center": [0.0, 0.0]}),
        json!({"cmd": "create_sketch", "plane": {"Offset": {"base": "WorldXY", "distance": 30.0}},
               "template": "centered_circle", "diameter": 30.0, "center": [0.0, 0.0]}),
        json!({"cmd": "loft", "a": 1, "b": 2}),
    ] {
        tray::ok(&mut s, &v);
    }
    let want = PI * 30.0 / 3.0 * (400.0 + 300.0 + 225.0);
    // Cone volume: our measure is a tessellation estimate (ADR 0008).
    let v = tray::volume(&s);
    assert!((v - want).abs() < 1e-3 * want, "{v} vs {want}");
    // A rectangle against a triangle: refused by name, unchanged.
    for v in [
        rect_on(0.0, 20.0, 20.0),
        json!({"cmd": "create_sketch", "plane": {"Offset": {"base": "WorldXY", "distance": 10.0}},
               "template": "polygon", "vertices": [[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]]}),
    ] {
        tray::ok(&mut s, &v);
    }
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(&mut s, &json!({"cmd": "loft", "a": 4, "b": 5}));
    assert!(!r.ok);
    match r.error {
        Some(DocError::ProductLimit(m)) => assert!(m.contains("loft"), "{m}"),
        e => panic!("{e:?}"),
    }
    assert_eq!(s.model().unwrap().document(), &before);
}
