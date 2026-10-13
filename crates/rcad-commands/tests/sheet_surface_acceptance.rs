//! SESSION_7 acceptance projects 6 (sheet-metal U-channel) and 7 (open-shell
//! repair) replayed from the committed workflows
//! `examples/sheet_u_channel_workflow.json` and
//! `examples/open_box_repair_workflow.json`, checked in closed form, with
//! their exports written for external validation:
//! `RCAD_EVIDENCE_DIR=DIR cargo test -p rcad-commands --test sheet_surface_acceptance`
//! then `tools/validate_sheet_surface.sh DIR` (FreeCAD reads the STEP/DXF).
//! With `RCAD_WRITE_EXAMPLES=1` the native examples are (re)written.
mod support;
use rcad_commands::{Command, Session};
use rcad_document::BodyId;
use rcad_kernel::VolumeResult;
use serde_json::{Value, json};
use std::f64::consts::PI;
use std::path::PathBuf;
use support::tray;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn replay(name: &str) -> Session {
    let fixtures = root().join("crates/rcad-kernel/tests/fixtures/step");
    let text = std::fs::read_to_string(root().join("examples").join(name))
        .unwrap()
        .replace("${STEP_FIXTURES}", &fixtures.display().to_string());
    let steps: Vec<Value> = serde_json::from_str(&text).unwrap();
    let mut s = Session::new();
    for v in steps {
        let c: Command = serde_json::from_value(v.clone()).unwrap();
        let r = s.execute(&c);
        assert!(r.ok, "{v}: {:?}", r.error);
    }
    s
}

fn vol(s: &Session, b: u32) -> Option<f64> {
    let b = s.summary().bodies.into_iter().find(|x| x.id == BodyId(b))?;
    match b.measurements?.volume {
        VolumeResult::Value { volume, .. } => Some(volume),
        _ => None,
    }
}

fn out_dir() -> (PathBuf, Option<tempfile::TempDir>) {
    match std::env::var_os("RCAD_EVIDENCE_DIR") {
        Some(d) => {
            let d = PathBuf::from(d);
            std::fs::create_dir_all(&d).unwrap();
            (d, None)
        }
        None => {
            let t = tempfile::tempdir().unwrap();
            (t.path().to_path_buf(), Some(t))
        }
    }
}

fn write_example(s: &mut Session, name: &str) {
    if std::env::var_os("RCAD_WRITE_EXAMPLES").is_some() {
        tray::ok(
            s,
            &json!({"cmd": "save_copy", "path": root().join("examples").join(name)}),
        );
    }
}

#[test]
fn acceptance_6_u_channel() {
    let (dir, _keep) = out_dir();
    let mut s = replay("sheet_u_channel_workflow.json");
    // Convention: straights between tangent points 60 + 20 + 20.
    let folded =
        |t: f64, r: f64| 40.0 * t * 100.0 + 40.0 * 2.0 * (PI / 4.0) * ((r + t).powi(2) - r * r);
    let flat = |t: f64, r: f64| 100.0 + 2.0 * (PI / 2.0) * (r + 0.4 * t);
    assert!((flat(1.0, 2.0) - 107.539822).abs() < 1e-6);
    assert!((vol(&s, 1).unwrap() - folded(1.0, 2.0)).abs() < 1e-6);
    write_example(&mut s, "sheet_u_channel.rcad.json");
    for (tag, t, r) in [("t1_r2", 1.0, 2.0), ("t2_r3", 2.0, 3.0)] {
        tray::ok(
            &mut s,
            &json!({"cmd": "edit_sheet_rule", "base": 2, "thickness": t, "radius": r}),
        );
        assert!((vol(&s, 1).unwrap() - folded(t, r)).abs() < 1e-6, "{tag}");
        let rr = tray::ok(
            &mut s,
            &json!({"cmd": "export_flat_pattern", "feature": 2,
                    "path": dir.join(format!("u_channel_{tag}_flat.dxf"))}),
        );
        assert!(
            rr.message
                .unwrap()
                .contains(&format!("{:.6} x 40.000000", flat(t, r)))
        );
        tray::ok(
            &mut s,
            &json!({"cmd": "export_step", "path": dir.join(format!("u_channel_{tag}_folded.step")),
                    "bodies": [1]}),
        );
        tray::ok(
            &mut s,
            &json!({"cmd": "sheet_flat_state", "base": 2, "flat": true}),
        );
        let fb = s.summary().bodies.iter().find(|b| b.visible).unwrap().id;
        assert!((vol(&s, fb.0).unwrap() - flat(t, r) * 40.0 * t).abs() < 1e-6);
        tray::ok(
            &mut s,
            &json!({"cmd": "export_step", "path": dir.join(format!("u_channel_{tag}_flat.step")),
                    "bodies": [fb.0]}),
        );
        tray::ok(
            &mut s,
            &json!({"cmd": "sheet_flat_state", "base": 2, "flat": false}),
        );
    }
    // A 90° flange on the top edge is valid (the corners stay open); undo it.
    let n = s.summary().features.len();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "sheet_flange", "base": 2, "edge": 3, "angle_deg": 90, "length": 20,
                "start_offset": 0, "end_offset": 0, "down": false}),
    );
    assert!(r.ok, "{:?}", r.error);
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    // Invalid self-overlapping flanges: 150° flanges on the bottom (line 1)
    // and top (line 3) edges lean over the base toward each other and cross;
    // the second is refused and nothing changes.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "sheet_flange", "base": 2, "edge": 1, "angle_deg": 150, "length": 60}),
    );
    assert!(r.ok, "leaning flange alone is valid: {:?}", r.error);
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "sheet_flange", "base": 2, "edge": 3, "angle_deg": 150, "length": 60}),
    );
    assert!(!r.ok, "self-overlapping flange pair accepted");
    assert!(format!("{:?}", r.error).contains("overlaps flange"));
    assert_eq!(s.summary().features.len(), n + 1);
}

#[test]
fn acceptance_7_open_box_repair_and_thicken() {
    let (dir, _keep) = out_dir();
    let mut s = replay("open_box_repair_workflow.json");
    assert_eq!(vol(&s, 1), None, "imported open shell has no volume");
    assert_eq!(vol(&s, 2), None, "stitched but not filled: still open");
    let v = vol(&s, 3).unwrap();
    assert!((v - 24000.0).abs() <= 0.02 * 600.0 + 1e-6, "{v}");
    assert_eq!(vol(&s, 4), None);
    let t = vol(&s, 5).unwrap();
    assert!((t - 0.5 * PI * (144.0 - 100.0) * 30.0).abs() < 1e-6, "{t}");
    // Excessive thickness toward the axis: refused, nothing added.
    let n = s.summary().bodies.len();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "thicken_surface", "body": 4, "distance": 12, "reverse": true}),
    );
    assert!(!r.ok && format!("{:?}", r.error).contains("collapses"));
    assert_eq!(s.summary().bodies.len(), n);
    write_example(&mut s, "open_box_repair.rcad.json");
    tray::ok(
        &mut s,
        &json!({"cmd": "export_step", "path": dir.join("gap_box_filled.step"), "bodies": [3]}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "export_step", "path": dir.join("half_tube_thick.step"), "bodies": [5]}),
    );
    // The exact open box (no gap) fills to exactly 24000.
    let mut e = Session::new();
    tray::ok(&mut e, &json!({"cmd": "new_document"}));
    let p = root().join("crates/rcad-kernel/tests/fixtures/step/open_box_mm.step");
    tray::ok(&mut e, &json!({"cmd": "import_surfaces", "path": p}));
    tray::ok(&mut e, &json!({"cmd": "fill_surface", "body": 1}));
    assert!((vol(&e, 2).unwrap() - 24000.0).abs() < 1e-6);
    tray::ok(
        &mut e,
        &json!({"cmd": "export_step", "path": dir.join("open_box_filled.step"), "bodies": [2]}),
    );
}

#[test]
fn committed_native_examples_reopen() {
    for (name, body, want) in [
        ("sheet_u_channel.rcad.json", 1, 4000.0 + 100.0 * PI),
        ("open_box_repair.rcad.json", 5, 0.5 * PI * 44.0 * 30.0),
    ] {
        let p = root().join("examples").join(name);
        if !p.exists() {
            panic!("{name} missing: run with RCAD_WRITE_EXAMPLES=1");
        }
        let mut s = Session::new();
        tray::ok(&mut s, &json!({"cmd": "open", "path": p}));
        assert!((vol(&s, body).unwrap() - want).abs() < 1e-6, "{name}");
    }
}
