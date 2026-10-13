//! Named configurations (run-6 pull-forward of SESSION_7 §3 / coverage PA03):
//! two product variants (80 and 100 mm) with different part numbers and an
//! optional feature suppressed in one; a shared dimension edited once
//! regenerates both; batch exports named by variant leave the document as
//! it was; refusals unchanged; save/reopen (format v6). Volumes closed-form.
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

/// Open tray L x 60 x 20, wall w, floor 2; optional Ø6 drain hole in the floor.
fn vol(l: f64, w: f64, hole: bool) -> f64 {
    l * 60.0 * 20.0
        - (l - 2.0 * w) * (60.0 - 2.0 * w) * 18.0
        - if hole { 9.0 * PI * 2.0 } else { 0.0 }
}

fn build(s: &mut Session) {
    run(
        s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "set_user_parameter", "name": "length", "expression": "100", "unit": "mm"}),
            json!({"cmd": "set_user_parameter", "name": "wall", "expression": "2", "unit": "mm"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 100.0, "height": 60.0}),
            json!({"cmd": "set_expression", "feature": 1, "name": "width", "expression": "length"}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 20.0, "body_name": "Tray"}),
            json!({"cmd": "create_sketch",
                   "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
                   "template": "centered_rectangle", "width": 96.0, "height": 56.0}),
            json!({"cmd": "set_expression", "feature": 3, "name": "width",
                   "expression": "length - 2 * wall"}),
            json!({"cmd": "set_expression", "feature": 3, "name": "height",
                   "expression": "60 - 2 * wall"}),
            json!({"cmd": "extrude", "sketch": 3, "distance": 18.0, "op": "cut", "target": 1}),
            // F5/F6: the optional drain hole.
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
                   "diameter": 6.0}),
            json!({"cmd": "extrude", "sketch": 5, "op": "cut", "target": 1, "through_all": true}),
            json!({"cmd": "set_configuration", "name": "Small", "part_number": "ENC-80",
                   "values": {"length": "80"}, "suppressed": [5, 6]}),
            json!({"cmd": "set_configuration", "name": "Large", "part_number": "ENC-100",
                   "values": {"length": "100"}}),
        ],
    );
}

#[test]
fn two_variants_follow_a_shared_edit_and_export_by_name() {
    let mut s = Session::new();
    build(&mut s);
    assert!((tray::volume(&s) - vol(100.0, 2.0, true)).abs() < 1e-6);
    tray::ok(
        &mut s,
        &json!({"cmd": "activate_configuration", "name": "Small"}),
    );
    assert!((tray::volume(&s) - vol(80.0, 2.0, false)).abs() < 1e-6);
    let sum = s.summary();
    assert_eq!(sum.active_configuration.as_deref(), Some("Small"));
    assert!(sum.features.iter().filter(|f| f.suppressed).count() == 2);
    tray::ok(
        &mut s,
        &json!({"cmd": "activate_configuration", "name": "Large"}),
    );
    assert!((tray::volume(&s) - vol(100.0, 2.0, true)).abs() < 1e-6);
    // Shared dimension: wall 3, edited once, applies to both variants.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_user_parameter", "name": "wall", "expression": "3", "unit": "mm"}),
    );
    assert!((tray::volume(&s) - vol(100.0, 3.0, true)).abs() < 1e-6);
    tray::ok(
        &mut s,
        &json!({"cmd": "activate_configuration", "name": "Small"}),
    );
    assert!((tray::volume(&s) - vol(80.0, 3.0, false)).abs() < 1e-6);
    // Undo the activation: back to Large.
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert!((tray::volume(&s) - vol(100.0, 3.0, true)).abs() < 1e-6);
    assert_eq!(s.summary().active_configuration.as_deref(), Some("Large"));
    // Batch export: names carry the variant and part number; the document is
    // restored afterwards.
    let before = s.model().unwrap().document().clone();
    let dir = tempfile::tempdir().unwrap();
    tray::ok(
        &mut s,
        &json!({"cmd": "export_configurations", "dir": dir.path(), "stem": "tray",
                "step": true, "stl": true}),
    );
    assert_eq!(s.model().unwrap().document(), &before);
    for f in [
        "tray-Small-ENC-80.step",
        "tray-Small-ENC-80.stl",
        "tray-Large-ENC-100.step",
        "tray-Large-ENC-100.stl",
    ] {
        assert!(dir.path().join(f).is_file(), "{f} missing");
    }
    // The STEP of each variant re-imports at that variant's volume.
    for (f, v) in [
        ("tray-Small-ENC-80.step", vol(80.0, 3.0, false)),
        ("tray-Large-ENC-100.step", vol(100.0, 3.0, true)),
    ] {
        let mut t = Session::new();
        run(&mut t, &[json!({"cmd": "new_document"})]);
        tray::ok(
            &mut t,
            &json!({"cmd": "import_step", "path": dir.path().join(f)}),
        );
        let got: f64 = t
            .summary()
            .bodies
            .iter()
            .map(|b| match b.measurements.as_ref().unwrap().volume {
                rcad_kernel::VolumeResult::Value { volume, .. } => volume,
                ref x => panic!("{x:?}"),
            })
            .sum();
        assert!((got - v).abs() < 1e-6 * v, "{f}: {got} vs {v}");
    }
    // Existing files are not overwritten silently.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "export_configurations", "dir": dir.path(), "stem": "tray",
                "step": true}),
    );
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().document(), &before);
    // Save / reopen: v6 with both configurations.
    let path = dir.path().join("c.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(6));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": path}));
    assert_eq!(t.summary().configurations.len(), 2);
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
}

#[test]
fn configuration_refusals_leave_the_document_unchanged() {
    let mut s = Session::new();
    build(&mut s);
    let before = s.model().unwrap().document().clone();
    for c in [
        // Unknown parameter, bad expression, unknown feature, bad name.
        json!({"cmd": "set_configuration", "name": "X", "values": {"nope": "1"}}),
        json!({"cmd": "set_configuration", "name": "X", "values": {"length": "2 *"}}),
        json!({"cmd": "set_configuration", "name": "X", "suppressed": [99]}),
        json!({"cmd": "set_configuration", "name": "a/b"}),
        // Suppressing the cut's sketch but not the cut leaves an evaluated
        // feature depending on a suppressed one.
        json!({"cmd": "set_configuration", "name": "Bad", "suppressed": [5]}),
        // A length that leaves no material between the walls.
        json!({"cmd": "set_configuration", "name": "Tiny", "values": {"length": "4"}}),
        json!({"cmd": "activate_configuration", "name": "Nope"}),
    ] {
        let r = tray::exec(&mut s, &c);
        if c["name"] == json!("Bad") || c["name"] == json!("Tiny") {
            // Stored, but activation is refused.
            if r.ok {
                let a = tray::exec(
                    &mut s,
                    &json!({"cmd": "activate_configuration", "name": c["name"]}),
                );
                assert!(!a.ok, "{c} activated");
                tray::ok(&mut s, &json!({"cmd": "undo"}));
            }
            assert_eq!(s.model().unwrap().document(), &before, "{c}");
            continue;
        }
        assert!(!r.ok, "{c} accepted");
        assert_eq!(s.model().unwrap().document(), &before, "{c}");
    }
}
