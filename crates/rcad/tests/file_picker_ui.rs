//! **Browse…** buttons on path fields, driven through the real widgets with a scripted dialog
//! backend in place of the platform file dialog (which a headless test cannot show): the chosen
//! path lands in the field it was opened for and the dialog's OK then uses it (import a STEP
//! fixture, save the document), a cancelled dialog leaves the field unchanged, and
//! the dialog receives the mode, filters and start folder for the field. One test function: the
//! dialog hub is process-wide.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::file_pick::{self, PickBackend, PickMode, PickRequest};
use rcad::{ExecMode, RcadApp};

/// Answers each dialog with the next scripted path (`None` = cancelled) and records requests.
#[derive(Default)]
struct Scripted {
    answers: Mutex<Vec<Option<PathBuf>>>,
    seen: Mutex<Vec<PickRequest>>,
}

impl PickBackend for Scripted {
    fn pick(&self, req: &PickRequest) -> Option<PathBuf> {
        self.seen.lock().unwrap().push(req.clone());
        self.answers.lock().unwrap().remove(0)
    }
}

fn harness() -> Harness<'static, RcadApp> {
    Harness::builder()
        .with_size([1400.0, 900.0])
        .with_max_steps(64)
        .build_ui_state(
            |ui, app: &mut RcadApp| app.ui(ui),
            RcadApp::new(ExecMode::Threaded, None),
        )
}

fn settle(h: &mut Harness<'_, RcadApp>) {
    let start = Instant::now();
    loop {
        let _ = h.try_run();
        if h.state().idle() && !file_pick::busy() {
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

fn type_into(h: &mut Harness<'_, RcadApp>, label: &str, text: &str) {
    h.get_by_label(label).focus();
    let _ = h.try_run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    let _ = h.try_run();
    h.get_by_label(label).type_text(text);
    let _ = h.try_run();
}

/// Click "Browse <field>" and run frames until the chosen path is in the field.
fn browse(h: &mut Harness<'_, RcadApp>, field: &str) {
    click(h, &format!("Browse {field}"));
    // The result is taken by the field on the frame after the dialog thread finishes.
    for _ in 0..4 {
        let _ = h.try_run();
    }
}

fn value(h: &Harness<'_, RcadApp>, label: &str) -> String {
    h.get_by_label(label).value().unwrap_or_default()
}

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(rel)
        .canonicalize()
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn browse_fills_path_fields() {
    let dir = std::env::temp_dir().join(format!("rcad-pick-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let step = fixture("rcad-kernel/tests/fixtures/step/box_40x30x10_mm.step");
    let doc = dir.join("picked.rcad.json");
    let backend = Arc::new(Scripted::default());
    *backend.answers.lock().unwrap() = vec![None, Some(step.clone()), Some(doc.clone())];
    file_pick::set_backend(backend.clone());

    let mut h = harness();
    click(&mut h, "New");

    // Import: a cancelled dialog keeps what was typed; a chosen file replaces it and imports.
    click(&mut h, "Import…");
    type_into(&mut h, "Import file path", "typed.step");
    browse(&mut h, "Import file path");
    assert_eq!(value(&h, "Import file path"), "typed.step");
    browse(&mut h, "Import file path");
    assert_eq!(value(&h, "Import file path"), step.to_string_lossy());
    click(&mut h, "OK");
    let s = h.state().summary().clone();
    assert_eq!(s.bodies.len(), 1, "the picked STEP file was imported");

    // Save As: the picked path is where the document is written.
    click(&mut h, "Save As…");
    browse(&mut h, "File path");
    assert_eq!(value(&h, "File path"), doc.to_string_lossy());
    click(&mut h, "OK");
    assert!(doc.is_file(), "document saved at the picked path");

    // The dialog was asked for the right kind of choice, filters and start folder.
    let seen = backend.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[0].mode, PickMode::Open);
    assert!(seen[0].filters.iter().any(|(_, e)| e.contains(&"step")));
    assert_eq!(seen[2].mode, PickMode::Save);
    assert!(seen[2].filters.iter().any(|(_, e)| e.contains(&"json")));

    // A start folder comes from what the field holds (the Browser's External document field).
    let other = dir.join("other.rcad.json");
    std::fs::copy(&doc, &other).unwrap();
    *backend.answers.lock().unwrap() = vec![Some(other.clone())];
    type_into(
        &mut h,
        "External document",
        &dir.join("old.rcad.json").to_string_lossy(),
    );
    browse(&mut h, "External document");
    assert_eq!(value(&h, "External document"), other.to_string_lossy());
    let last = backend.seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.mode, PickMode::Open);
    assert_eq!(last.start_dir.as_deref(), Some(dir.as_path()));
    assert_eq!(last.file_name, None, "no suggested name when opening");

    let _ = std::fs::remove_dir_all(&dir);
}
