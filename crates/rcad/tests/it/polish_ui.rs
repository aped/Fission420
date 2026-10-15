//! First-user polish: Esc closes the information windows, faces are named
//! in plain words, the status bar's volume line reads cleanly and the
//! workspace tabs explain themselves.
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::{FeatureId, SketchPlane};

fn harness() -> Harness<'static, RcadApp> {
    Harness::builder()
        .with_size([1280.0, 800.0])
        .with_max_steps(64)
        .build_ui_state(
            |ui, app: &mut RcadApp| app.ui(ui),
            RcadApp::new(ExecMode::Sync, None),
        )
}

fn settle(h: &mut Harness<'_, RcadApp>) {
    let start = Instant::now();
    loop {
        let _ = h.try_run();
        if h.state().idle() {
            let _ = h.try_run();
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(120), "not idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn click(h: &mut Harness<'_, RcadApp>, label: &str) {
    h.get_by_label(label).click();
    settle(h);
}

fn esc(h: &mut Harness<'_, RcadApp>) {
    h.key_press(Key::Escape);
    settle(h);
}

/// A 40 × 30 × 10 plate.
fn plate(h: &mut Harness<'_, RcadApp>) {
    click(h, "New");
    h.state_mut().request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 40.0,
            height: 30.0,
            center: Some([20.0, 15.0]),
        },
    });
    h.state_mut()
        .request(Command::extrude(None, FeatureId(1), 10.0));
    settle(h);
}

/// Esc closes About, then Help, then a refusal notice, one per press.
#[test]
fn escape_closes_information_windows_and_the_notice() {
    let mut h = harness();
    click(&mut h, "New");
    h.key_press(Key::E); // refused: no sketch
    settle(&mut h);
    assert!(h.state().notice().is_some());
    h.key_press(Key::F1);
    settle(&mut h);
    click(&mut h, "File menu");
    click(&mut h, "About R-CAD");
    assert!(h.query_by_label_contains("Version ").is_some());
    esc(&mut h);
    assert!(
        h.query_by_label_contains("Version ").is_none(),
        "About still open"
    );
    assert!(
        h.query_by_label("Keyboard layer:").is_some(),
        "Help closed too early"
    );
    esc(&mut h);
    assert!(
        h.query_by_label("Keyboard layer:").is_none(),
        "Help still open"
    );
    assert!(h.state().notice().is_some());
    esc(&mut h);
    assert_eq!(h.state().notice(), None);
}

/// A picked face is named in plain words (body, feature, face); the
/// internal reference is no longer part of the name.
#[test]
fn picked_faces_are_named_in_plain_words() {
    let mut h = harness();
    plate(&mut h);
    let top = h
        .state()
        .faces()
        .iter()
        .find(|f| {
            f.plane
                .is_some_and(|p| p.normal[2] > 0.99 && (p.point[2] - 10.0).abs() < 1e-9)
        })
        .cloned()
        .expect("top face");
    assert_eq!(top.description, "Body1: Extrude1 end cap");
    h.state_mut().select_face(Some(top.group));
    settle(&mut h);
    h.get_by_label("Body1: Extrude1 end cap");
}

/// The volume line has no "±0e0" noise and counts solids in words.
#[test]
fn status_bar_volume_line_reads_cleanly() {
    let mut h = harness();
    plate(&mut h);
    h.get_by_label_contains("Volume 12000.0000 mm³ · Bounds");
    h.get_by_label_contains("· 1 solid closed");
    assert!(h.query_by_label_contains("±").is_none());
}

/// Each workspace tab says what it holds on hover.
#[test]
fn workspace_tabs_have_tooltips() {
    let mut h = harness();
    click(&mut h, "New");
    h.get_by_label("MESH tab").hover();
    let _ = h.try_run();
    let _ = h.try_run();
    assert!(
        h.query_by_label_contains("repair and edit").is_some(),
        "no MESH tab tooltip"
    );
}

/// File ▸ Quit (Ctrl+Q) asks first when there is unsaved work and quits
/// straight away otherwise.
#[test]
fn quit_asks_about_unsaved_changes() {
    let mut h = harness();
    plate(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Q);
    settle(&mut h);
    h.get_by_label("Discard changes");
    click(&mut h, "Keep editing");
    assert!(!h.state().exit_confirmed());
    click(&mut h, "File menu");
    click(&mut h, "Quit");
    click(&mut h, "Discard changes");
    assert!(h.state().exit_confirmed());
    // A document without changes quits without asking.
    let mut h = harness();
    click(&mut h, "New");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Q);
    settle(&mut h);
    assert!(h.state().exit_confirmed());
}

fn pointer(h: &mut Harness<'_, RcadApp>, p: egui::Pos2, pressed: Option<bool>) {
    h.event(egui::Event::PointerMoved(p));
    if let Some(pressed) = pressed {
        h.event(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    let _ = h.try_run();
}

/// A browser width the user drags is saved and used at the next start;
/// opening a document (layout alone) saves nothing.
#[test]
fn dragged_browser_width_persists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r-cad").join("prefs.json");
    let mut h = harness();
    h.state_mut().load_prefs(path.clone());
    plate(&mut h);
    assert!(!path.exists(), "saved without a drag");
    let [w, _] = h.state().panel_widths().expect("widths");
    let y = 400.0;
    pointer(&mut h, egui::pos2(w + 1.0, y), Some(true));
    for dx in [10.0, 30.0, 60.0] {
        pointer(&mut h, egui::pos2(w + 1.0 + dx, y), None);
    }
    pointer(&mut h, egui::pos2(w + 61.0, y), Some(false));
    let _ = h.try_run();
    let saved = h
        .state()
        .prefs()
        .browser_width
        .expect("browser width saved");
    // Wider by about the drag (egui starts the drag after a few points).
    assert!(
        saved > w + 40.0 && saved <= w + 61.0,
        "{saved} after dragging {w} by 60"
    );
    assert!(path.exists());
    // Next start (preferences load before the first frame, as in main):
    // the column opens at the saved width.
    let mut app = RcadApp::new(ExecMode::Sync, None);
    app.load_prefs(path);
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .with_max_steps(64)
        .build_ui_state(|ui, app: &mut RcadApp| app.ui(ui), app);
    click(&mut h, "New");
    let [w2, _] = h.state().panel_widths().unwrap();
    assert!((w2 - saved).abs() <= 3.0, "{w2} vs {saved}");
}
