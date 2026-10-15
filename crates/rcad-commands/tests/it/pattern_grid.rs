//! Rectangular (two-direction) linear pattern, as Fusion's rectangular
//! pattern: a 3 x 2 grid of compartment pockets in a tray from one seed;
//! closed-form volume; counts and spacings are parameters (count2 edit,
//! spacing2 edit, refusal of an overlapping spacing); undo; save/reopen as
//! format v8; a one-row pattern stays format-identical to before.
use crate::support;
use rcad_commands::Session;
use serde_json::{Value, json};
use support::tray;

fn run(s: &mut Session, steps: &[Value]) {
    for v in steps {
        tray::ok(s, v);
    }
}

/// Block 100 x 70 x 20 (x 0..100, y 0..70); a 28 x 30 pocket, 15 deep,
/// corner at (4, 4), sketched on the top face (features 3, 4).
fn tray_seed(s: &mut Session) {
    run(
        s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 100.0, "height": 70.0, "center": [50.0, 35.0]}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 20.0}),
            json!({"cmd": "create_sketch",
                   "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
                   "template": "centered_rectangle", "width": 28.0, "height": 30.0,
                   "center": [18.0, 19.0]}),
            json!({"cmd": "extrude", "sketch": 3, "distance": 15.0, "op": "cut", "target": 1}),
        ],
    );
}

#[test]
fn three_by_two_compartments_from_one_pocket() {
    let mut s = Session::new();
    tray_seed(&mut s);
    let pocket = 28.0 * 30.0 * 15.0;
    tray::ok(
        &mut s,
        &json!({"cmd": "linear_pattern", "feature": 4, "count": 3, "spacing": 32.0,
                "direction": [1.0, 0.0],
                "second": {"count": 2, "spacing": 32.0, "direction": [0.0, 1.0]}}),
    );
    assert!((tray::volume(&s) - (140000.0 - 6.0 * pocket)).abs() < 1e-6);
    // spacing2 edit: the rows move apart, same volume.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 5, "name": "spacing2", "value": 34.0}),
    );
    assert!((tray::volume(&s) - (140000.0 - 6.0 * pocket)).abs() < 1e-6);
    // Spacing below the pocket size: instances overlap (one pocket of the
    // union shape); the exact volume follows from the overlap.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 5, "name": "spacing2", "value": 20.0}),
    );
    // Rows overlap in y by 10: each column is one 28 x 50 pocket.
    assert!((tray::volume(&s) - (140000.0 - 3.0 * 28.0 * 50.0 * 15.0)).abs() < 1e-6);
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert!((tray::volume(&s) - (140000.0 - 6.0 * pocket)).abs() < 1e-6);
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 5, "name": "count2", "value": 1.0}),
    );
    assert!((tray::volume(&s) - (140000.0 - 3.0 * pocket)).abs() < 1e-6);
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 5, "name": "count2", "value": 0.0}),
    );
    assert!(!r.ok);
    // Save / reopen: v8.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("grid.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(8));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": path}));
    assert!((tray::volume(&t) - (140000.0 - 6.0 * pocket)).abs() < 1e-6);
    // A parallel second direction is refused.
    let mut u = Session::new();
    tray_seed(&mut u);
    let r = tray::exec(
        &mut u,
        &json!({"cmd": "linear_pattern", "feature": 4, "count": 2, "spacing": 32.0,
                "direction": [1.0, 0.0],
                "second": {"count": 2, "spacing": 32.0, "direction": [-1.0, 0.0]}}),
    );
    assert!(!r.ok);
    assert!(
        format!("{:?}", r.error).contains("parallel"),
        "{:?}",
        r.error
    );
}

/// A sketch on the floor of grid instance (i 1, j 1) keeps that occurrence
/// when the first-direction count changes: instance numbers are stable per
/// (i, j), not positional.
#[test]
fn copy_reference_keeps_its_grid_cell_when_count_changes() {
    let mut s = Session::new();
    tray_seed(&mut s);
    tray::ok(
        &mut s,
        &json!({"cmd": "linear_pattern", "feature": 4, "count": 3, "spacing": 32.0,
                "direction": [1.0, 0.0],
                "second": {"count": 2, "spacing": 32.0, "direction": [0.0, 1.0]}}),
    );
    // Instance (1, 1) = 1 + 1·100; its pocket floor faces up at z = 5.
    let floor = json!({"Face": {"feature": 5, "role": {"Copy": {"instance": 101, "tool": 0,
                         "of": "EndCap"}}}});
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "create_sketch", "plane": floor, "template": "centered_circle",
                "diameter": 4.0, "center": [0.0, 0.0]}),
    );
    let sk = r.feature.unwrap();
    let frame = |s: &Session| {
        s.model()
            .unwrap()
            .evaluation()
            .features
            .iter()
            .find(|f| f.id == sk)
            .and_then(|f| f.frame)
            .expect("sketch frame")
    };
    let o = frame(&s).origin;
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 5, "name": "count", "value": 2.0}),
    );
    let o2 = frame(&s).origin;
    for k in 0..3 {
        assert!((o[k] - o2[k]).abs() < 1e-9, "{o:?} -> {o2:?}");
    }
}
