//! UI interaction tests: drive the real widgets (buttons by label, typed text)
//! through egui_kittest's AccessKit tree, then check the document state that
//! the engine (the authoritative Session) reports back.

use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use rcad::{ExecMode, RcadApp};

const V_100X50: f64 = 49214.60183660255; // 50000 - 250*pi
const V_80X50: f64 = 39214.60183660255; // 40000 - 250*pi
const VOL_TOL: f64 = 0.005;

fn harness(mode: ExecMode) -> Harness<'static, RcadApp> {
    Harness::builder()
        .with_size([1400.0, 900.0])
        .with_max_steps(64)
        .build_ui_state(|ui, app: &mut RcadApp| app.ui(ui), RcadApp::new(mode, None))
}

/// Step frames until the engine has answered everything (worker or sync).
fn settle(h: &mut Harness<'_, RcadApp>) {
    let start = Instant::now();
    loop {
        let _ = h.try_run();
        if h.state().idle() {
            let _ = h.try_run();
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "engine did not become idle"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Open the Templates menu and click one of its items.
fn template(h: &mut Harness<'_, RcadApp>, item: &str) {
    h.get_by_label("Templates").click();
    let _ = h.try_run();
    h.get_by_label(item).click();
    settle(h);
}

fn click(h: &mut Harness<'_, RcadApp>, label: &str) {
    h.get_by_label(label).click();
    settle(h);
}

/// Focus the text field labelled `label`, select all, type `text`.
fn type_into(h: &mut Harness<'_, RcadApp>, label: &str, text: &str) {
    h.get_by_label(label).focus();
    let _ = h.try_run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    let _ = h.try_run();
    h.get_by_label(label).type_text(text);
    let _ = h.try_run();
    assert_eq!(
        h.get_by_label(label).value().as_deref(),
        Some(text),
        "field {label}"
    );
}

fn field_value(h: &Harness<'_, RcadApp>, label: &str) -> Option<String> {
    h.get_by_label(label).value()
}

fn volume(h: &Harness<'_, RcadApp>) -> f64 {
    h.state().volume().expect("analytic volume available")
}

fn assert_volume(h: &Harness<'_, RcadApp>, expected: f64) {
    let v = volume(h);
    assert!(
        (v - expected).abs() <= VOL_TOL,
        "volume {v} vs expected {expected}"
    );
}

/// Steps 1-4 up to (not including) the cut, entirely through toolbar/dialog widgets.
fn build_plate_and_hole_sketch(h: &mut Harness<'_, RcadApp>) {
    click(h, "New");
    assert!(h.state().summary().open);

    template(h, "Rectangle on XY…");
    type_into(h, "Rectangle width (mm)", "80");
    type_into(h, "Rectangle height (mm)", "50");
    click(h, "OK");
    let s = &h.state().summary().features;
    assert_eq!(s.len(), 1);
    assert_eq!(
        s[0].parameters,
        vec![
            ("width".to_string(), 80.0),
            ("height".to_string(), 50.0),
            ("center_x".to_string(), 0.0),
            ("center_y".to_string(), 0.0)
        ]
    );
    // The inspector shows the actual constraint state (symbol + words).
    h.get_by_label("✔ Fully constrained");

    click(h, "Extrude…");
    type_into(h, "Extrude distance (mm)", "10");
    click(h, "OK");
    assert_eq!(h.state().summary().features.len(), 2);
    assert!((volume(h) - 40000.0).abs() <= VOL_TOL);

    template(h, "Circle on face…");
    // Pick the extrusion from the combo explicitly.
    h.get_by_role_and_label(Role::ComboBox, "End cap of extrusion")
        .click();
    let _ = h.try_run();
    let extrude = h.state().summary().features[1].clone();
    let option = format!("{}: {}", extrude.id, extrude.name);
    h.get_by_label(&option).click();
    let _ = h.try_run();
    type_into(h, "Circle diameter (mm)", "10");
    click(h, "OK");
    let circle = h.state().summary().features[2].clone();
    // Semantic face reference (feature + role), not a face index.
    assert_eq!(circle.plane, Some(rcad_commands::end_cap_of(extrude.id)));
    h.get_by_label_contains(&format!("on {}.EndCap (semantic", extrude.name));

    click(h, "Cut through all…");
}

fn check_cut_result(h: &Harness<'_, RcadApp>) {
    assert_eq!(h.state().summary().features.len(), 4);
    assert_volume(h, V_80X50);
    let m = h.state().measurements().expect("measurements");
    assert_eq!(m.shell_count, 1);
    assert!(m.all_shells_closed);
}

/// Steps 1-4 of the acceptance workflow.
fn build_plate(h: &mut Harness<'_, RcadApp>) {
    build_plate_and_hole_sketch(h);
    click(h, "OK");
    check_cut_result(h);
}

/// Browser rows are labelled by the (unique) feature name.
fn feature_row_label(h: &Harness<'_, RcadApp>, idx: usize) -> String {
    h.state().summary().features[idx].name.clone()
}

#[test]
fn full_workflow_through_widgets() {
    let mut h = harness(ExecMode::Sync);
    build_plate(&mut h);
    assert!(h.state().mesh_triangles() > 0, "display mesh produced");

    // Step 5: select the base sketch, edit width in the inspector (preview), Apply.
    let rev0 = h.state().summary().revision;
    let row = feature_row_label(&h, 0);
    click(&mut h, &row);
    assert_eq!(field_value(&h, "width (mm)").as_deref(), Some("80"));
    type_into(&mut h, "width (mm)", "100");
    let _ = h.try_run();
    assert_eq!(
        h.state().summary().revision,
        rev0,
        "an un-applied inspector edit is only a preview"
    );
    click(&mut h, "Apply");
    assert_volume(&h, V_100X50);
    let b = h.state().measurements().unwrap().bounds;
    assert!((b.min[0] + 50.0).abs() < 1e-6 && (b.max[0] - 50.0).abs() < 1e-6);
    assert_eq!(field_value(&h, "width (mm)").as_deref(), Some("100"));

    // Step 7: undo/redo via toolbar buttons.
    click(&mut h, "Undo");
    assert_volume(&h, V_80X50);
    assert_eq!(field_value(&h, "width (mm)").as_deref(), Some("80"));
    click(&mut h, "Redo");
    assert_volume(&h, V_100X50);

    // Step 6: depth 12 through the inspector; the hole sketch follows the top face.
    let row = feature_row_label(&h, 1);
    click(&mut h, &row);
    type_into(&mut h, "distance (mm)", "12");
    click(&mut h, "Apply");
    assert_volume(&h, 60000.0 - 300.0 * std::f64::consts::PI);
    let hole = &h.state().summary().features[2];
    assert_eq!(hole.frame.map(|f| f.origin[2]), Some(12.0), "{hole:?}");
    type_into(&mut h, "distance (mm)", "10");
    click(&mut h, "Apply");
    assert_volume(&h, V_100X50);

    // Step 10b: oversize hole through the inspector is rejected (product limit).
    let before = h.state().summary().clone();
    let row = feature_row_label(&h, 2);
    click(&mut h, &row);
    type_into(&mut h, "diameter (mm)", "60");
    click(&mut h, "Apply");
    let d = h.state().diagnostics().last().expect("diagnostic").clone();
    assert!(d.error && d.kind == "ProductLimit", "{d:?}");
    assert_eq!(h.state().summary(), &before);
    assert_eq!(field_value(&h, "diameter (mm)").as_deref(), Some("10"));

    // Step 10: a negative thickness is rejected, shown, reverted; doc unchanged.
    let before = h.state().summary().clone();
    let row = feature_row_label(&h, 1);
    click(&mut h, &row);
    type_into(&mut h, "distance (mm)", "-5");
    click(&mut h, "Apply");
    let d = h.state().diagnostics().last().expect("diagnostic").clone();
    assert!(d.error, "{d:?}");
    assert!(d.text.contains("rejected"), "{d:?}");
    h.get_by_label_contains("Last error [");
    assert_eq!(h.state().summary(), &before, "document unchanged");
    assert_eq!(field_value(&h, "distance (mm)").as_deref(), Some("10"));
    assert_volume(&h, V_100X50);
}

#[test]
fn cancel_creates_nothing_and_reverts_preview() {
    let mut h = harness(ExecMode::Sync);
    click(&mut h, "New");
    template(&mut h, "Rectangle on XY…");
    type_into(&mut h, "Rectangle width (mm)", "30");
    click(&mut h, "Cancel");
    assert!(h.state().summary().features.is_empty());
    assert!(h.query_by_label("Rectangle width (mm)").is_none());

    template(&mut h, "Rectangle on XY…");
    click(&mut h, "OK");
    let rev = h.state().summary().revision;
    let row = feature_row_label(&h, 0);
    click(&mut h, &row);
    type_into(&mut h, "height (mm)", "12");
    click(&mut h, "Cancel edits");
    assert_eq!(field_value(&h, "height (mm)").as_deref(), Some("50"));
    assert_eq!(h.state().summary().revision, rev);
}

#[test]
fn non_numeric_input_is_reported() {
    let mut h = harness(ExecMode::Sync);
    click(&mut h, "New");
    template(&mut h, "Rectangle on XY…");
    type_into(&mut h, "Rectangle width (mm)", "abc");
    click(&mut h, "OK");
    assert!(h.state().summary().features.is_empty());
    let d = h.state().diagnostics().last().unwrap();
    assert_eq!(d.kind, "Input");
    click(&mut h, "Cancel");
}

#[test]
fn save_export_reopen_through_dialogs() {
    let dir = std::env::temp_dir().join(format!("rcad-ui-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let doc = dir.join("plate.rcad.json");
    let stl = dir.join("plate.stl");
    let mut h = harness(ExecMode::Sync);
    build_plate(&mut h);

    click(&mut h, "Save");
    type_into(&mut h, "File path", doc.to_str().unwrap());
    click(&mut h, "OK");
    assert!(doc.exists());
    assert!(!h.state().summary().dirty);

    h.get_by_label("Export").click();
    let _ = h.try_run();
    click(&mut h, "Export STL…");
    type_into(&mut h, "STL path", stl.to_str().unwrap());
    type_into(&mut h, "Chord tolerance (mm)", "0.01");
    click(&mut h, "OK");
    let mc = h.state().last_export().expect("mesh check").clone();
    assert!(mc.watertight);
    assert!((mc.mesh_volume - V_80X50).abs() / V_80X50 <= 1e-3);
    // The dialog's per-body report (run 4 print export) shows the mesh checks.
    h.get_by_label_contains("valid solid mesh: closed yes");
    click(&mut h, "Cancel");

    let before = h.state().summary().features.clone();
    click(&mut h, "Close");
    assert!(!h.state().summary().open);
    click(&mut h, "Open…");
    type_into(&mut h, "File path", doc.to_str().unwrap());
    click(&mut h, "OK");
    assert_eq!(h.state().summary().features, before);
    assert_volume(&h, V_80X50);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn threaded_worker_shows_busy_state() {
    let mut h = harness(ExecMode::Threaded);
    build_plate_and_hole_sketch(&mut h);
    // Submit the cut; the boolean regenerates on the worker thread while the UI
    // keeps rendering with a busy indicator. Run 2: commands are queued (FIFO
    // worker) instead of disabled, so New stays available while busy.
    h.get_by_label("OK").click();
    let _ = h.try_run();
    let _ = h.try_run();
    assert!(h.state().busy(), "cut should still be regenerating");
    h.get_by_label("Regenerating…");
    assert!(!h.get_by_label("New").accesskit_node().is_disabled());
    settle(&mut h);
    assert!(h.query_by_label("Regenerating…").is_none());
    assert!(!h.get_by_label("New").accesskit_node().is_disabled());
    check_cut_result(&h);
}

/// ~15 boolean regenerations: minutes in an unoptimized build (Truck booleans are
/// ~35 s each at opt-level 0). Runs in `cargo test -p rcad --release`.
#[test]
#[cfg_attr(debug_assertions, ignore = "slow in debug; run with --release")]
fn demo_script_reaches_final_plate() {
    let mut h = harness(ExecMode::Sync);
    let dir = std::env::temp_dir().join(format!("rcad-ui-demo-{}", std::process::id()));
    h.state_mut().start_demo(dir.clone());
    settle(&mut h);
    let s = h.state().summary();
    assert_eq!(s.features.len(), 4);
    assert_volume(&h, V_100X50);
    // The scripted negative thickness was rejected and shown.
    assert!(h.state().diagnostics().iter().any(|d| d.error));
    assert_eq!(field_value(&h, "width (mm)").as_deref(), Some("100"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn revolve_through_widgets() {
    let mut h = harness(ExecMode::Sync);
    click(&mut h, "New");
    template(&mut h, "Rectangle on XZ…");
    click(&mut h, "OK");
    click(&mut h, "Revolve…");
    type_into(&mut h, "Revolve angle (deg)", "90");
    click(&mut h, "OK");
    // Washer quarter: A = 20*10, x_c = 30 -> V = 200 * pi/2 * 30 (Pappus).
    assert_volume(&h, 200.0 * std::f64::consts::FRAC_PI_2 * 30.0);
    let row = feature_row_label(&h, 1);
    click(&mut h, &row);
    assert_eq!(field_value(&h, "angle_deg (deg)").as_deref(), Some("90"));
    type_into(&mut h, "angle_deg (deg)", "360");
    click(&mut h, "Apply");
    assert_volume(&h, 200.0 * 2.0 * std::f64::consts::PI * 30.0);
}
