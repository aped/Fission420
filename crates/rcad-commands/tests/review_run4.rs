//! Run-4 review findings fixed in run 5 (the run-4 review findings list):
//! #15 — a file whose expressions no longer evaluate opens with its stored
//! values and a warning, and can be repaired; the stored geometry is used,
//! edits are refused until the expression is fixed.
mod support;
use rcad_commands::Session;
use serde_json::json;
use support::tray::{self, Tray};

#[test]
fn a_file_with_a_broken_expression_opens_and_can_be_repaired() {
    let t = Tray::BASELINE;
    let mut s = tray::build(&t, false);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tray.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    // Hand edit: a parameter now names one that does not exist.
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let params = v
        .pointer_mut("/document/parameters")
        .and_then(|p| p.as_array_mut())
        .expect("parameters in the file");
    let cw = params
        .iter_mut()
        .find(|p| p["name"] == "cavity_width")
        .unwrap();
    cw["expression"] = json!("tray_widht - 2 * wall");
    std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();

    let mut s2 = Session::new();
    let r = tray::ok(&mut s2, &json!({"cmd": "open", "path": path}));
    let msg = r.message.unwrap_or_default();
    assert!(
        msg.contains("WARNING") && msg.contains("tray_widht"),
        "{msg}"
    );
    // Stored values: the same tray.
    assert!((tray::volume(&s2) - t.plain_volume()).abs() < 1e-6);
    // Edits re-derive every binding: refused until the expression is fixed.
    let r = tray::exec(
        &mut s2,
        &json!({"cmd": "set_user_parameter", "name": "divider_count", "expression": "3",
                "unit": "unitless"}),
    );
    assert!(!r.ok);
    tray::ok(
        &mut s2,
        &json!({"cmd": "set_user_parameter", "name": "cavity_width",
                "expression": "tray_width - 2 * wall", "unit": "mm"}),
    );
    tray::ok(
        &mut s2,
        &json!({"cmd": "set_user_parameter", "name": "divider_count", "expression": "3",
                "unit": "unitless"}),
    );
    let t3 = Tray { n: 3, ..t };
    assert!((tray::volume(&s2) - t3.plain_volume()).abs() < 1e-6);
}

/// #9: the hole's material side comes from the faces under the hole, not from
/// any coplanar face. Plane z = 10 holds the base's top (x 0..30, +Z) and an
/// overhang's underside (x 40..60, −Z); a hole at (15, 15) on the base top
/// was refused as ambiguous, and now drills down through the base.
#[test]
fn hole_side_comes_from_the_faces_under_the_hole() {
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 40.0, "height": 30.0, "center": [20.0, 15.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
        json!({"cmd": "create_sketch", "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
               "template": "centered_rectangle", "width": 30.0, "height": 30.0,
               "center": [45.0, 15.0]}),
        json!({"cmd": "extrude", "sketch": 3, "distance": 10.0, "op": "join", "target": 1}),
    ] {
        tray::ok(&mut s, &v);
    }
    let v0 = 12000.0 + 30.0 * 30.0 * 10.0;
    assert!((tray::volume(&s) - v0).abs() < 1e-6);
    tray::ok(
        &mut s,
        &json!({"cmd": "hole", "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
                "center": [15.0, 15.0], "diameter": 5.5}),
    );
    let want = v0 - std::f64::consts::PI * 2.75 * 2.75 * 10.0;
    let v = tray::volume(&s);
    assert!((v - want).abs() < 1e-3, "{v} vs {want}");
    // Drilled into the base (below the face), nothing above it.
    let p = tray::probe(
        &s,
        &[[15.0, 15.0, 5.0], [15.0, 15.0, 15.0], [10.0, 15.0, 5.0]],
    );
    assert_eq!(p, vec![Some(false), Some(false), Some(true)]);
}
