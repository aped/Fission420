//! Read-only Properties tab of the right column: document totals without a
//! selection, a feature's parameters (with bound expressions) and health, a
//! body's measured size; no action buttons.
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

#[test]
fn properties_tab_is_read_only_and_follows_the_selection() {
    let mut h = harness();
    click(&mut h, "New");
    h.state_mut().request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 40.0,
            height: 30.0,
            center: None,
        },
    });
    h.state_mut()
        .request(Command::extrude(None, FeatureId(1), 10.0));
    h.state_mut().request(Command::SetUserParameter {
        name: "t".into(),
        expression: "10".into(),
        unit: None,
        comment: None,
    });
    h.state_mut().request(Command::SetExpression {
        feature: FeatureId(2),
        name: "distance".into(),
        expression: "t".into(),
    });
    settle(&mut h);
    click(&mut h, "Properties tab");
    // A feature: its parameters with the bound expression, and health.
    click(&mut h, "Extrude1");
    h.get_by_label("Extrude1 (extrude)");
    h.get_by_label("10 = t");
    h.get_by_label("Healthy");
    // No inspector actions on this tab.
    assert!(h.query_by_label("Shell…").is_none());
    assert!(h.query_by_label("Delete feature").is_none());
    // No selection (Esc): document totals.
    h.key_press(eframe::egui::Key::Escape);
    settle(&mut h);
    h.get_by_label("12000");
    // A body: its size.
    let b = h.state().summary().bodies[0].id;
    h.state_mut().select_body(Some(b));
    settle(&mut h);
    h.get_by_label("40 × 30 × 10");
    // Back to the Inspector tab: its forms are there again.
    click(&mut h, "Inspector tab");
    assert!(h.query_by_label("Properties grid").is_none());
}
