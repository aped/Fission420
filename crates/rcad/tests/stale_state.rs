//! SESSION_4 §2: stale asynchronous state. Import replies carry a request
//! token, so a reply from a cancelled or replaced import form never resolves a
//! newer one; Undo/Redo close dialogs holding ids that may now name other
//! objects; a dialog whose ids vanish is closed instead of acting on them.
//! Runs on the threaded engine with slow regenerations queued first, so the
//! imports really are pending when the forms are cancelled/reopened.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, ExtrudeOp, SketchTemplate};
use rcad_document::{BodyId, FaceRef, FaceRole, FeatureId, SketchPlane};

fn harness(mode: ExecMode) -> Harness<'static, RcadApp> {
    Harness::builder()
        .with_size([1400.0, 900.0])
        .with_max_steps(64)
        .build_ui_state(|ui, app: &mut RcadApp| app.ui(ui), RcadApp::new(mode, None))
}

fn settle(h: &mut Harness<'_, RcadApp>) {
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

fn click(h: &mut Harness<'_, RcadApp>, label: &str) {
    h.get_by_label(label).click();
    settle(h);
}

/// Click without waiting for the engine (work stays queued).
fn click_now(h: &mut Harness<'_, RcadApp>, label: &str) {
    h.get_by_label(label).click();
    let _ = h.try_run();
}

fn type_into(h: &mut Harness<'_, RcadApp>, label: &str, text: &str) {
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

const STEP_BOX: &str = "rcad-kernel/tests/fixtures/step/box_40x30x10_mm.step";
const STEP_TWO: &str = "rcad-kernel/tests/fixtures/step/two_boxes_compound_mm.step";

/// Queue a plate with a long hole pattern (seconds of boolean work) without
/// waiting, so commands submitted next stay pending.
fn queue_slow_work(h: &mut Harness<'_, RcadApp>) {
    h.state_mut().request(Command::NewDocument);
    queue_slow_edits(h);
}

/// The slow part of [`queue_slow_work`] into the open (empty) document.
fn queue_slow_edits(h: &mut Harness<'_, RcadApp>) {
    let app = h.state_mut();
    app.request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 300.0,
            height: 60.0,
            center: Some([150.0, 30.0]),
        },
    });
    app.request(Command::extrude(None, FeatureId(1), 5.0));
    app.request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::Face(FaceRef {
            feature: FeatureId(2),
            role: FaceRole::ExtrudeEndCap,
        }),
        template: SketchTemplate::CenteredCircle {
            diameter: 3.0,
            center: Some([5.0, 30.0]),
        },
    });
    app.request(Command::Extrude {
        name: None,
        sketch: FeatureId(3),
        distance: 0.0,
        op: ExtrudeOp::Cut,
        target: Some(BodyId(1)),
        through_all: true,
        reverse: false,
        body_name: None,
        region: None,
    });
    app.request(Command::LinearPattern {
        name: None,
        feature: FeatureId(4),
        count: 40,
        spacing: 7.0,
        direction: [1.0, 0.0],
    });
    let _ = h.try_run();
}

/// Run-3 app finding #14: cancel a pending import, reopen Import, choose a
/// different file and press OK before the first reply arrives. Before the
/// fix the first reply resolved the second form (its file name with the
/// first import's bodies) and the second reply was dropped.
#[test]
fn import_reply_after_cancel_does_not_resolve_the_reopened_form() {
    let mut h = harness(ExecMode::Threaded);
    queue_slow_work(&mut h);
    click_now(&mut h, "Import…");
    type_into(&mut h, "Import file path", &fixture(STEP_BOX));
    click_now(&mut h, "OK");
    assert!(h.state().busy(), "the first import must still be pending");
    click_now(&mut h, "Cancel");
    // Reopen, same format, different file.
    click_now(&mut h, "Import…");
    type_into(&mut h, "Import file path", &fixture(STEP_TWO));
    click_now(&mut h, "OK");
    assert!(h.state().busy(), "the second import must still be pending");
    settle(&mut h);
    let summary = h
        .state()
        .import_summary()
        .expect("second import summarized");
    assert_eq!(summary.file, "two_boxes_compound_mm.step");
    assert_eq!(summary.bodies.len(), 2, "{:?}", summary.bodies);
    // Both imports ran (Cancel closes the form only): plate + 1 + 2 bodies.
    let s = h.state().summary();
    assert_eq!(s.bodies.len(), 4, "{:?}", s.bodies);
    let created = summary.feature.unwrap();
    let f = s.features.iter().find(|f| f.id == created).unwrap();
    assert_eq!(
        f.creates.len(),
        2,
        "summary names the second import's feature"
    );
}

/// Cancel, then a different format: only the DXF form is resolved.
#[test]
fn import_cancel_then_other_format() {
    let mut h = harness(ExecMode::Threaded);
    queue_slow_work(&mut h);
    click_now(&mut h, "Import…");
    type_into(&mut h, "Import file path", &fixture(STEP_TWO));
    click_now(&mut h, "OK");
    assert!(h.state().busy());
    click_now(&mut h, "Cancel");
    click_now(&mut h, "Import…");
    type_into(
        &mut h,
        "Import file path",
        &fixture("rcad-exchange/tests/fixtures/dxf/slot_freecad_r14.dxf"),
    );
    click_now(&mut h, "OK");
    assert!(h.state().busy());
    settle(&mut h);
    let summary = h.state().import_summary().expect("DXF import summarized");
    assert_eq!(summary.file, "slot_freecad_r14.dxf");
    assert!(summary.bodies.is_empty(), "a DXF import makes a sketch");
}

/// New requested while an import is pending: the import completes in the
/// old document (queue order), its own form is resolved, and the switch then
/// clears the summary; the new document is empty.
#[test]
fn new_document_while_import_pending() {
    let mut h = harness(ExecMode::Threaded);
    click(&mut h, "New");
    queue_slow_edits(&mut h);
    click_now(&mut h, "Import…");
    type_into(&mut h, "Import file path", &fixture(STEP_BOX));
    click_now(&mut h, "OK");
    assert!(h.state().busy());
    let doc_before = h.state().summary().doc_id;
    h.state_mut().request(Command::NewDocument);
    settle(&mut h);
    let s = h.state().summary();
    assert_ne!(s.doc_id, doc_before);
    assert!(s.features.is_empty() && s.bodies.is_empty(), "{s:?}");
    assert!(h.state().import_summary().is_none());
    assert!(h.query_by_label("Import file path").is_none());
}

/// A failed import followed by a document switch: the error lands on its own
/// form, then the switch closes it; the new document is unchanged (empty).
#[test]
fn failed_import_then_document_switch() {
    let mut h = harness(ExecMode::Threaded);
    click(&mut h, "New");
    queue_slow_edits(&mut h);
    click_now(&mut h, "Import…");
    type_into(
        &mut h,
        "Import file path",
        &fixture("rcad-kernel/tests/fixtures/step/garbage.step"),
    );
    click_now(&mut h, "OK");
    h.state_mut().request(Command::NewDocument);
    settle(&mut h);
    let s = h.state().summary();
    assert!(s.features.is_empty() && s.revision == 0, "{s:?}");
    assert!(h.state().import_summary().is_none());
    assert!(
        h.state()
            .diagnostics()
            .iter()
            .any(|d| d.error && d.text.contains("import_step rejected")),
        "{:?}",
        h.state().diagnostics()
    );
}

/// Run-3 app finding #9 (Undo part): an Extrude dialog holding a sketch id is
/// closed by Undo, so a later object reusing that id cannot be extruded by it.
#[test]
fn undo_closes_dialogs_holding_ids() {
    let mut h = harness(ExecMode::Sync);
    click(&mut h, "New");
    let app = h.state_mut();
    app.request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 20.0,
            height: 10.0,
            center: None,
        },
    });
    settle(&mut h);
    click(&mut h, "Extrude…");
    assert!(h.query_by_label("OK").is_some(), "extrude dialog open");
    click(&mut h, "Undo");
    assert!(
        h.query_by_label("OK").is_none(),
        "Undo closed the dialog that held the sketch id"
    );
    assert!(
        h.state()
            .diagnostics()
            .iter()
            .any(|d| d.text.contains("Undo changed the document")),
        "{:?}",
        h.state().diagnostics()
    );
    // A new sketch now reuses id F1; nothing was extruded by the old dialog.
    assert!(h.state().summary().bodies.is_empty());
}
