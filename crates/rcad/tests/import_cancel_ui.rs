//! Cancel a long import from the Import dialog (threaded engine): a
//! generated 2 000 000-triangle binary STL is imported, the dialog's Cancel
//! is pressed while the worker reads it, and the import fails as cancelled:
//! no feature, no body, revision unchanged. A second import of a small file
//! afterwards works (the cancellation applied to that one command only).
//! Own test binary: the cancellation flag is process-wide.
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};

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

/// Binary STL of `n` separate small triangles.
fn big_stl(path: &std::path::Path, n: u32) {
    let mut b = Vec::with_capacity(84 + 50 * n as usize);
    b.extend_from_slice(&[b' '; 80]);
    b.extend_from_slice(&n.to_le_bytes());
    for i in 0..n {
        let x = (i % 1000) as f32;
        let y = (i / 1000) as f32;
        for v in [
            [0.0f32, 0.0, 1.0],
            [x, y, 0.0],
            [x + 1.0, y, 0.0],
            [x, y + 1.0, 0.0],
        ] {
            for c in v {
                b.extend_from_slice(&c.to_le_bytes());
            }
        }
        b.extend_from_slice(&[0, 0]);
    }
    std::fs::write(path, b).unwrap();
}

#[test]
fn cancel_a_long_stl_import_from_the_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let big = dir.path().join("big.stl");
    big_stl(&big, 2_000_000);
    let mut h = harness(ExecMode::Threaded);
    click(&mut h, "New");
    let rev = h.state().summary().revision;
    click_now(&mut h, "Import…");
    type_into(&mut h, "Import file path", &big.display().to_string());
    click_now(&mut h, "OK");
    assert!(h.state().busy(), "the import must be running");
    // Let the worker get into the read, then cancel from the dialog.
    std::thread::sleep(Duration::from_millis(150));
    let _ = h.try_run();
    assert!(h.state().busy(), "the import finished before Cancel");
    let t = Instant::now();
    click_now(&mut h, "Cancel");
    settle(&mut h);
    let s = h.state().summary();
    assert!(
        s.bodies.is_empty() && s.features.is_empty(),
        "{:?}",
        s.bodies
    );
    assert_eq!(s.revision, rev, "nothing committed");
    assert!(
        t.elapsed() < Duration::from_secs(60),
        "cancel took {:?}",
        t.elapsed()
    );
    let last = h
        .state()
        .diagnostics()
        .last()
        .cloned()
        .expect("a diagnostic");
    assert!(last.text.contains("cancelled"), "{last:?}");
    // Stopped while reading, not answered as still queued.
    assert!(!last.text.contains("before it started"), "{last:?}");
    // The next import is not affected.
    click_now(&mut h, "Import…");
    type_into(&mut h, "Import file path", &fixture(STEP_BOX));
    click(&mut h, "OK");
    assert_eq!(h.state().summary().bodies.len(), 1);
}

const STEP_BOX: &str = "rcad-kernel/tests/fixtures/step/box_40x30x10_mm.step";
