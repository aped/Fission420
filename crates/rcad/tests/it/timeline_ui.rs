//! Bottom timeline strip: one marker per feature, health tint, the history
//! marker (roll back here / to the end, by context menu and by dragging the
//! marker), Edit Feature from a feature's marker.
use std::time::{Duration, Instant};

use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::{FeatureId, SketchPlane};

fn harness() -> Harness<'static, RcadApp> {
    Harness::builder()
        .with_size([1400.0, 900.0])
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

fn two_plates(h: &mut Harness<'_, RcadApp>) {
    click(h, "New");
    for (i, x) in [0.0, 60.0].into_iter().enumerate() {
        h.state_mut().request(Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 20.0,
                height: 10.0,
                center: Some([x, 0.0]),
            },
        });
        h.state_mut()
            .request(Command::extrude(None, FeatureId(2 * i as u32 + 1), 5.0));
    }
    settle(h);
    assert_eq!(h.state().summary().bodies.len(), 2);
}

#[test]
fn timeline_markers_roll_back_and_edit() {
    let mut h = harness();
    two_plates(&mut h);
    for f in ["Sketch1", "Extrude1", "Sketch2", "Extrude2"] {
        h.get_by_label(&format!("Timeline {f}"));
    }
    h.get_by_label("Timeline marker");
    // Context menu on Extrude1 ▸ Roll History Marker Here.
    h.get_by_label("Timeline Extrude1").click_secondary();
    settle(&mut h);
    click(&mut h, "Roll History Marker Here");
    let rolled: Vec<String> = h
        .state()
        .summary()
        .features
        .iter()
        .filter(|f| f.rolled_back)
        .map(|f| f.name.clone())
        .collect();
    assert_eq!(rolled, ["Sketch2", "Extrude2"]);
    assert_eq!(h.state().summary().bodies.len(), 1);
    // Drag the marker to the end of the strip: everything evaluates again.
    let from = h.get_by_label("Timeline marker").rect().center();
    let to =
        h.get_by_label("Timeline Extrude2").rect().right_center() + eframe::egui::vec2(4.0, 0.0);
    h.event(eframe::egui::Event::PointerMoved(from));
    h.step();
    h.event(eframe::egui::Event::PointerButton {
        pos: from,
        button: eframe::egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    h.step();
    for k in 1..=10 {
        h.event(eframe::egui::Event::PointerMoved(
            from + (to - from) * (k as f32 / 10.0),
        ));
        h.step();
    }
    h.event(eframe::egui::Event::PointerButton {
        pos: to,
        button: eframe::egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    settle(&mut h);
    assert!(
        h.state().summary().features.iter().all(|f| !f.rolled_back),
        "{:?}",
        h.state().diagnostics().last()
    );
    assert_eq!(h.state().summary().bodies.len(), 2);
    // Edit Feature from the timeline.
    h.get_by_label("Timeline Extrude2").click_secondary();
    settle(&mut h);
    click(&mut h, "Edit Feature");
    h.get_by_label("Edit Extrude2");
}
