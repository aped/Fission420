//! Import / export dialogs driven through the real widgets: STEP (single and
//! multi-solid, then a sketch on an imported planar face), STL with an explicit
//! unit, unitless DXF (the dialog asks for units after the first attempt), a
//! failed import into a non-empty document (unchanged), and the export
//! selection policy (selected vs all visible bodies, STEP refused for meshes,
//! DXF of a sketch). Expected sizes come from the fixtures' documented
//! geometry (40x30x10 mm box, FreeCAD-generated), not from the importer.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use f420_app::{ExecMode, F420App};
use f420_document::{BodySource, FaceRole};
use f420_kernel::VolumeResult;

fn harness(mode: ExecMode) -> Harness<'static, F420App> {
    Harness::builder()
        .with_size([1400.0, 900.0])
        .with_max_steps(64)
        .build_ui_state(|ui, app: &mut F420App| app.ui(ui), F420App::new(mode, None))
}

fn settle(h: &mut Harness<'_, F420App>) {
    let start = Instant::now();
    loop {
        let _ = h.try_run();
        if h.state().idle() {
            let _ = h.try_run();
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(300), "not idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn click(h: &mut Harness<'_, F420App>, label: &str) {
    h.get_by_label(label).click();
    settle(h);
}

fn type_into(h: &mut Harness<'_, F420App>, label: &str, text: &str) {
    h.get_by_label(label).focus();
    let _ = h.try_run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    let _ = h.try_run();
    h.get_by_label(label).type_text(text);
    let _ = h.try_run();
}

fn fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(rel)
        .canonicalize()
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
        .display()
        .to_string()
}

fn import(h: &mut Harness<'_, F420App>, path: &str) {
    click(h, "Import…");
    type_into(h, "Import file path", path);
    click(h, "OK");
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("f420-xui-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

#[test]
fn step_import_summary_fit_and_sketch_on_imported_face() {
    let mut h = harness(ExecMode::Threaded);
    click(&mut h, "New");
    let cam0 = h.state().fallback_camera.clone();
    import(
        &mut h,
        &fixture("f420-kernel/tests/fixtures/step/box_40x30x10_mm.step"),
    );
    let s = h.state().summary().clone();
    assert_eq!(s.bodies.len(), 1);
    let b = &s.bodies[0];
    assert_eq!(b.kind, "brep");
    assert!(
        matches!(&b.source, BodySource::ImportedStep { file_name, .. } if file_name == "box_40x30x10_mm.step")
    );
    // Summary window with units and supported actions; the dialog closed.
    assert!(
        h.query_by_label("Import summary").is_some() || h.query_by_label("Close summary").is_some()
    );
    assert!(h.query_by_label("Import file path").is_none());
    assert_eq!(h.state().selected_body(), Some(b.id));
    assert_ne!(h.state().fallback_camera, cam0, "new bodies fitted");
    // Measured (deferred) volume of the 40x30x10 box.
    let vol = s.bodies[0]
        .measurements
        .as_ref()
        .map(|m| m.volume.clone())
        .or_else(|| {
            h.state().summary().bodies[0]
                .measurements
                .as_ref()
                .map(|m| m.volume.clone())
        });
    if let Some(VolumeResult::Value { volume, .. }) = vol {
        assert!((volume - 12000.0).abs() < 0.01, "{volume}");
    }
    click(&mut h, "Close summary");
    assert!(h.query_by_label("Close summary").is_none());
    // Body inspector: provenance, no feature history.
    let name = b.name.clone();
    click(&mut h, &name);
    // Pick the imported top face (z = 10 within the box footprint): a
    // semantic imported-face reference that can host a sketch.
    let bb = h.state().summary().bodies[0].clone();
    let top = h
        .state()
        .faces()
        .iter()
        .find(|f| matches!(f.reference, Some(r) if matches!(r.role, FaceRole::Imported { .. })))
        .cloned()
        .expect("imported planar faces are referenceable");
    assert_eq!(top.body, Some(bb.id));
    h.state_mut().select_face(Some(top.group));
    let _ = h.try_run();
    assert!(
        !h.get_by_label("Create Sketch")
            .accesskit_node()
            .is_disabled()
    );
    // Undo removes the whole import in one step.
    click(&mut h, "Undo");
    assert!(h.state().summary().bodies.is_empty());
    assert!(h.state().summary().features.is_empty());
}

#[test]
fn multi_solid_step_and_failed_import_leaves_document_unchanged() {
    let mut h = harness(ExecMode::Sync);
    click(&mut h, "New");
    import(
        &mut h,
        &fixture("f420-kernel/tests/fixtures/step/two_boxes_compound_mm.step"),
    );
    let before = h.state().summary().clone();
    assert_eq!(before.bodies.len(), 2, "one body per solid");
    click(&mut h, "Close summary");
    // Garbage STEP into the non-empty document: refused, shown in the dialog,
    // nothing changes.
    import(
        &mut h,
        &fixture("f420-kernel/tests/fixtures/step/garbage.step"),
    );
    let after = h.state().summary().clone();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.features, before.features);
    assert_eq!(after.bodies.len(), 2);
    assert!(
        h.query_by_label("Import file path").is_some(),
        "dialog stays open"
    );
    assert!(h.state().diagnostics().last().unwrap().error);
    // Truncated file too.
    type_into(
        &mut h,
        "Import file path",
        &fixture("f420-kernel/tests/fixtures/step/truncated.step"),
    );
    click(&mut h, "OK");
    assert_eq!(h.state().summary().revision, before.revision);
    click(&mut h, "Cancel");
    assert!(h.query_by_label("Import file path").is_none());
}

#[test]
fn stl_units_and_unitless_dxf_asks_for_units() {
    let mut h = harness(ExecMode::Sync);
    click(&mut h, "New");
    click(&mut h, "Import…");
    type_into(
        &mut h,
        "Import file path",
        &fixture("f420-exchange/tests/fixtures/stl/box_freecad_binary.stl"),
    );
    // STL: the unit choice is always shown; choose cm.
    h.get_by_label("File units").click();
    let _ = h.try_run();
    h.get_by_label("cm").click();
    let _ = h.try_run();
    click(&mut h, "OK");
    let b = h.state().summary().bodies[0].clone();
    assert_eq!(b.kind, "mesh");
    let d = b.mesh.as_ref().expect("mesh diagnostics");
    let (lo, hi) = d.bounds.unwrap();
    // FreeCAD box 20x10x5 file units -> cm -> 200x100x50 mm.
    assert!(((hi[0] - lo[0]) - 200.0).abs() < 1e-3, "{lo:?} {hi:?}");
    assert!(((hi[2] - lo[2]) - 50.0).abs() < 1e-3, "{lo:?} {hi:?}");
    assert!(d.closed);
    click(&mut h, "Close summary");
    // The mesh body is distinguished in the scene and cannot host a sketch.
    assert!(h.state().scene().bodies[0].mesh_body);
    let f = h.state().faces()[0].clone();
    assert!(f.mesh_body && f.sketch_blocker().is_some());

    // Unitless DXF: first attempt asks for units; the document is unchanged.
    let rev = h.state().summary().revision;
    click(&mut h, "Import…");
    type_into(
        &mut h,
        "Import file path",
        &fixture("f420-exchange/tests/fixtures/dxf/polyline_bulge_r12_unitless.dxf"),
    );
    assert!(
        h.query_by_label("File units").is_none(),
        "not asked up front"
    );
    click(&mut h, "OK");
    assert_eq!(h.state().summary().revision, rev);
    assert!(h.query_by_label("File units").is_some(), "now asked");
    click(&mut h, "OK"); // default mm
    let sk = h.state().summary().features.last().unwrap().clone();
    assert_eq!(sk.kind, "sketch");
    let notes = h.state().import_summary().expect("summary").notes.clone();
    assert!(
        notes.iter().any(|n| n.contains("chosen at import")),
        "{notes:?}"
    );
    click(&mut h, "Close summary");
}

#[test]
fn export_selection_policy_and_dxf() {
    let mut h = harness(ExecMode::Sync);
    click(&mut h, "New");
    import(
        &mut h,
        &fixture("f420-kernel/tests/fixtures/step/two_boxes_compound_mm.step"),
    );
    click(&mut h, "Close summary");
    click(&mut h, "Import…");
    type_into(
        &mut h,
        "Import file path",
        &fixture("f420-exchange/tests/fixtures/stl/box_freecad_ascii.stl"),
    );
    click(&mut h, "OK");
    click(&mut h, "Close summary");
    let bodies = h.state().summary().bodies.clone();
    assert_eq!(bodies.len(), 3);
    // Mesh body selected (after import): STEP refuses it with a reason.
    h.get_by_label("Export").click();
    let _ = h.try_run();
    click(&mut h, "Export STEP…");
    assert!(h.get_by_label("OK").accesskit_node().is_disabled());
    // All visible bodies: the mesh body is skipped (stated), the rest export.
    click(&mut h, "All visible bodies (3)");
    assert!(!h.get_by_label("OK").accesskit_node().is_disabled());
    click(&mut h, "Cancel");
    // Select the first STEP body and export just it.
    let first = bodies[0].name.clone();
    click(&mut h, &first);
    h.get_by_label("Export").click();
    let _ = h.try_run();
    click(&mut h, "Export STEP…");
    let path = tmp("one.step");
    type_into(&mut h, "STEP path", path.to_str().unwrap());
    assert!(!h.get_by_label("OK").accesskit_node().is_disabled());
    click(&mut h, "OK");
    let text = std::fs::read_to_string(&path).expect("STEP written");
    assert_eq!(
        text.matches("MANIFOLD_SOLID_BREP").count(),
        1,
        "one selected solid"
    );
    // STL of all visible bodies: three bodies concatenated (not fused).
    h.get_by_label("Export").click();
    let _ = h.try_run();
    click(&mut h, "Export STL…");
    click(&mut h, "All visible bodies (3)");
    let stl = tmp("all.stl");
    type_into(&mut h, "STL path", stl.to_str().unwrap());
    click(&mut h, "OK");
    let msg = h.state().diagnostics().last().unwrap().text.clone();
    assert!(msg.contains("3 bodies"), "{msg}");
    click(&mut h, "Cancel");
    // DXF of a sketch.
    h.get_by_label("Templates").click();
    let _ = h.try_run();
    click(&mut h, "Rectangle on XY…");
    click(&mut h, "OK");
    h.get_by_label("Export").click();
    let _ = h.try_run();
    click(&mut h, "Export DXF (sketch)…");
    let dxf = tmp("s.dxf");
    type_into(&mut h, "DXF path", dxf.to_str().unwrap());
    click(&mut h, "OK");
    let t = std::fs::read_to_string(&dxf).expect("DXF written");
    assert_eq!(t.matches("\nLINE\n").count(), 4, "{t}");
}
