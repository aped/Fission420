//! Run 7 surfaces and open-shell repair through the command API
//! (docs/SURFACES.md; acceptance project 7). STEP fixtures were made by
//! FreeCAD (tools/fixtures/step/open_shells.py); expected values are closed
//! form: the open box is [0,40]×[0,30]×[0,20] without its top (V = 24000).
mod support;
use rcad_commands::Session;
use rcad_document::BodyId;
use rcad_kernel::VolumeResult;
use serde_json::json;
use std::f64::consts::PI;
use support::tray;

fn fixture(name: &str) -> String {
    format!(
        "{}/../rcad-kernel/tests/fixtures/step/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn body(s: &Session, id: u32) -> rcad_commands::BodySummary {
    s.summary()
        .bodies
        .into_iter()
        .find(|b| b.id == BodyId(id))
        .unwrap_or_else(|| panic!("body {id}"))
}

fn volume(s: &Session, id: u32) -> Option<f64> {
    match body(s, id).measurements.expect("measured").volume {
        VolumeResult::Value { volume, .. } => Some(volume),
        VolumeResult::Unavailable { .. } => None,
    }
}

fn import(s: &mut Session, name: &str) {
    tray::ok(s, &json!({"cmd": "new_document"}));
    tray::ok(s, &json!({"cmd": "import_surfaces", "path": fixture(name)}));
}

#[test]
fn open_box_stays_open_then_fills_to_a_solid() {
    let mut s = Session::new();
    import(&mut s, "open_box_mm.step");
    let b = body(&s, 1);
    let m = b.measurements.clone().unwrap();
    assert!(
        !m.all_shells_closed,
        "an open shell must not be reported closed"
    );
    assert_eq!(volume(&s, 1), None, "an open shell has no volume");
    // Solid-only operations refuse the surface body.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "shell_face", "face": {"feature": 1, "role": {"Imported": {"body": 1, "face": 0}}}, "thickness": 1.0}),
    );
    assert!(!r.ok);
    // A plain STEP import points at import_surfaces.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "import_step", "path": fixture("open_box_mm.step")}),
    );
    assert!(!r.ok);
    assert!(format!("{:?}", r.error).contains("import_surfaces"));
    // Analysis before repair.
    let r = tray::ok(&mut s, &json!({"cmd": "analyze_surface", "body": 1}));
    let a = r.surface_analysis.unwrap();
    assert!(!a.closed);
    assert_eq!(
        (a.free_edges.len(), a.gaps.len(), a.boundary_loops.len()),
        (4, 0, 1)
    );
    assert!(a.orientation_conflicts.is_empty());
    // Fill (new body; the source is kept, hidden).
    let r = tray::ok(&mut s, &json!({"cmd": "fill_surface", "body": 1}));
    assert!(r.message.unwrap().contains("closed solid"));
    assert!((volume(&s, 2).unwrap() - 24000.0).abs() < 1e-6);
    assert!(!body(&s, 1).visible);
    assert_eq!(volume(&s, 1), None);
    // Undo / redo / save / reopen.
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert_eq!(s.summary().bodies.len(), 1);
    assert!(body(&s, 1).visible);
    tray::ok(&mut s, &json!({"cmd": "redo"}));
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("box.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": p}));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": p}));
    assert!((volume(&t, 2).unwrap() - 24000.0).abs() < 1e-6);
    assert_eq!(volume(&t, 1), None);
    // The open source body is not exported as a (malformed) solid.
    let r = tray::exec(
        &mut t,
        &json!({"cmd": "export_step", "path": dir.path().join("open.step"), "bodies": [1]}),
    );
    assert!(!r.ok);
    // The repaired solid exports as a STEP solid.
    let out = dir.path().join("box.step");
    tray::ok(
        &mut t,
        &json!({"cmd": "export_step", "path": out, "bodies": [2]}),
    );
    assert!(
        std::fs::read_to_string(&out)
            .unwrap()
            .contains("MANIFOLD_SOLID_BREP")
    );
}

#[test]
fn loose_faces_gap_report_stitch_fill() {
    let mut s = Session::new();
    import(&mut s, "loose_faces_mm.step");
    let a = tray::ok(
        &mut s,
        &json!({"cmd": "analyze_surface", "body": 1, "search": 1e-6}),
    )
    .surface_analysis
    .unwrap();
    assert_eq!((a.shells, a.free_edges.len(), a.gaps.len()), (5, 20, 8));
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "stitch_surface", "body": 1, "tolerance": 1e-6}),
    );
    assert!(r.message.unwrap().contains("open shell, 4 free edge(s)"));
    tray::ok(&mut s, &json!({"cmd": "fill_surface", "body": 2}));
    assert!((volume(&s, 3).unwrap() - 24000.0).abs() < 1e-6);
}

#[test]
fn gap_box_heals_within_the_displayed_tolerance_and_refuses_below_it() {
    let mut s = Session::new();
    import(&mut s, "gap_box_mm.step");
    let a = tray::ok(
        &mut s,
        &json!({"cmd": "analyze_surface", "body": 1, "search": 0.1}),
    )
    .surface_analysis
    .unwrap();
    let biggest = a.gaps.iter().map(|g| g.size).fold(0.0, f64::max);
    assert!((biggest - 0.02).abs() < 1e-9);
    // Stitch at 0.05 mm (shown in the notes), then fill.
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "stitch_surface", "body": 1, "tolerance": 0.05}),
    );
    let msg = r.message.unwrap();
    assert!(msg.contains("healing tolerance 0.05 mm"), "{msg}");
    tray::ok(
        &mut s,
        &json!({"cmd": "fill_surface", "body": 2, "tolerance": 0.05}),
    );
    let v = volume(&s, 3).unwrap();
    assert!((v - 24000.0).abs() <= 0.02 * 600.0 + 1e-6, "{v}");
    // Upstream edit: a tolerance below the gap leaves the stitch open and the
    // fill impossible; the edit is refused and the model is unchanged.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "tolerance", "value": 0.01}),
    );
    assert!(!r.ok);
    assert!((volume(&s, 3).unwrap() - v).abs() < 1e-12);
}

#[test]
fn thicken_plate_and_half_tube_with_collapse_refusal() {
    let mut s = Session::new();
    import(&mut s, "plate_mm.step");
    tray::ok(
        &mut s,
        &json!({"cmd": "thicken_surface", "body": 1, "distance": 2.0}),
    );
    assert!((volume(&s, 2).unwrap() - 2000.0).abs() < 1e-6);
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "distance", "value": 3.0}),
    );
    assert!((volume(&s, 2).unwrap() - 3000.0).abs() < 1e-6);
    let mut t = Session::new();
    import(&mut t, "half_tube_mm.step");
    tray::ok(
        &mut t,
        &json!({"cmd": "thicken_surface", "body": 1, "distance": 2.0}),
    );
    let out = volume(&t, 2).unwrap();
    // FreeCAD's face normal points away from the axis: outward thicken.
    let want_out = 0.5 * PI * (144.0 - 100.0) * 30.0;
    assert!((out - want_out).abs() < 1e-6, "{out}");
    let b = body(&t, 2).measurements.unwrap().bounds;
    assert!((b.max[0] - 12.0).abs() < 1e-9 && (b.min[1] + 12.0).abs() < 1e-9);
    // Toward the axis by >= the radius: offset collapse, refused.
    let r = tray::exec(
        &mut t,
        &json!({"cmd": "thicken_surface", "body": 1, "distance": 10.0, "reverse": true}),
    );
    assert!(!r.ok);
    assert!(format!("{:?}", r.error).contains("collapses"));
    let r = tray::ok(
        &mut t,
        &json!({"cmd": "thicken_surface", "body": 1, "distance": 4.0, "reverse": true}),
    );
    assert!(r.ok);
    let inn = volume(&t, 3).unwrap();
    assert!(
        (inn - 0.5 * PI * (100.0 - 36.0) * 30.0).abs() < 1e-6,
        "{inn}"
    );
}
