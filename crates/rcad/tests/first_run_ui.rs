//! What a first-time user meets: refusals of key presses and menu actions
//! are shown over the view (not only in the collapsed log), the window
//! title and status bar name the document and its unsaved state, and
//! About shows the version, license and source.
use std::time::{Duration, Instant};

use eframe::egui::Key;
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::SketchPlane;

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

/// E (Extrude) in an empty document says why nothing happened, over the
/// view; × dismisses it, and the next command clears a later one.
#[test]
fn refused_key_press_shows_a_dismissible_notice() {
    let mut h = harness();
    click(&mut h, "New");
    assert_eq!(h.state().notice(), None);
    h.key_press(Key::E);
    settle(&mut h);
    let n = h.state().notice().expect("a notice").to_string();
    assert!(n.starts_with("Extrude"), "{n}");
    assert!(n.len() > "Extrude…: ".len() + 10, "says why: {n}");
    click(&mut h, "Dismiss notice");
    assert_eq!(h.state().notice(), None);
    assert!(h.query_by_label("Dismiss notice").is_none());
    // Again, then a command replaces it.
    h.key_press(Key::E);
    settle(&mut h);
    assert!(h.state().notice().is_some());
    h.state_mut().request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 10.0,
            height: 10.0,
            center: None,
        },
    });
    settle(&mut h);
    assert_eq!(h.state().notice(), None);
}

fn rectangle() -> Command {
    Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 10.0,
            height: 10.0,
            center: None,
        },
    }
}

/// The window title and the status bar agree on the document name and
/// its save state: a new document is "Untitled" and not saved yet (never
/// "saved … unsaved"); an edit marks it; Save names it and clears the mark.
#[test]
fn title_and_status_bar_show_the_name_and_unsaved_state() {
    let mut h = harness();
    assert_eq!(h.state().window_title(), "R-CAD");
    click(&mut h, "New");
    assert_eq!(h.state().window_title(), "Untitled — R-CAD");
    h.get_by_label("not saved yet");
    assert!(h.query_by_label("saved").is_none());
    h.state_mut().request(rectangle());
    settle(&mut h);
    assert_eq!(h.state().window_title(), "● Untitled — R-CAD");
    h.get_by_label("● unsaved changes");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bracket.rcad.json");
    h.state_mut().request(Command::Save { path: path.clone() });
    settle(&mut h);
    assert_eq!(h.state().window_title(), "bracket.rcad.json — R-CAD");
    h.get_by_label("saved");
    h.get_by_label(&path.display().to_string());
    // The next edit marks it again.
    h.state_mut().request(rectangle());
    settle(&mut h);
    assert!(h.state().window_title().starts_with("● "));
}

/// File ▸ About R-CAD shows this build's version, license and source.
#[test]
fn about_shows_version_license_and_source() {
    let mut h = harness();
    click(&mut h, "File menu");
    click(&mut h, "About R-CAD");
    h.get_by_label(&format!("Version {}", env!("CARGO_PKG_VERSION")));
    h.get_by_label(&format!("License: {}", env!("CARGO_PKG_LICENSE")));
    assert!(h.query_by_label_contains("github.com").is_some());
}
