//! Editing a configured parameter while a configuration is active stores
//! the edit in that configuration's row (as Fusion edits the active
//! configuration), so switching away and back does not silently revert it;
//! an unconfigured parameter stays shared. Volumes closed-form.
use crate::support;
use rcad_commands::Session;
use serde_json::json;
use support::tray;

/// Block length x 20 x 10 with `length` configured: A = 40, B = 60.
fn build(s: &mut Session) {
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "set_user_parameter", "name": "length", "expression": "40", "unit": "mm"}),
        json!({"cmd": "set_user_parameter", "name": "depth", "expression": "10", "unit": "mm"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 40.0, "height": 20.0}),
        json!({"cmd": "set_expression", "feature": 1, "name": "width", "expression": "length"}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
        json!({"cmd": "set_expression", "feature": 2, "name": "distance", "expression": "depth"}),
        json!({"cmd": "set_configuration", "name": "A", "values": {"length": "40"}}),
        json!({"cmd": "set_configuration", "name": "B", "values": {"length": "60"}}),
        json!({"cmd": "activate_configuration", "name": "A"}),
    ] {
        tray::ok(s, &v);
    }
}

#[test]
fn an_edit_under_an_active_configuration_survives_switching() {
    let mut s = Session::new();
    build(&mut s);
    assert!((tray::volume(&s) - 40.0 * 200.0).abs() < 1e-6);
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "set_user_parameter", "name": "length", "expression": "50"}),
    );
    assert!(
        r.message
            .as_deref()
            .unwrap_or("")
            .contains("active configuration 'A'"),
        "{:?}",
        r.message
    );
    assert!((tray::volume(&s) - 50.0 * 200.0).abs() < 1e-6);
    tray::ok(
        &mut s,
        &json!({"cmd": "activate_configuration", "name": "B"}),
    );
    assert!((tray::volume(&s) - 60.0 * 200.0).abs() < 1e-6);
    tray::ok(
        &mut s,
        &json!({"cmd": "activate_configuration", "name": "A"}),
    );
    assert!((tray::volume(&s) - 50.0 * 200.0).abs() < 1e-6, "edit lost");
    // A parameter no configuration sets stays shared by both.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_user_parameters", "parameters": [{"name": "depth", "expression": "5"}]}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "activate_configuration", "name": "B"}),
    );
    assert!((tray::volume(&s) - 60.0 * 100.0).abs() < 1e-6);
    // Undo restores the row together with the value.
    tray::ok(
        &mut s,
        &json!({"cmd": "activate_configuration", "name": "A"}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "set_user_parameter", "name": "length", "expression": "70"}),
    );
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    tray::ok(
        &mut s,
        &json!({"cmd": "activate_configuration", "name": "B"}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "activate_configuration", "name": "A"}),
    );
    assert!((tray::volume(&s) - 50.0 * 100.0).abs() < 1e-6);
}
