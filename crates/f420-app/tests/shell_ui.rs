//! Scoped shell through the real UI: select a New Body extrusion, Shell… in
//! the inspector, choose the open face and thickness, OK; then edit the
//! thickness parameter from the inspector. Volumes are analytic.

use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};
use f420_app::{ExecMode, F420App};
use f420_commands::{Command, SketchTemplate};
use f420_document::SketchPlane;

fn harness() -> Harness<'static, F420App> {
    Harness::builder()
        .with_size([1400.0, 900.0])
        .with_max_steps(64)
        .build_ui_state(
            |ui, app: &mut F420App| app.ui(ui),
            F420App::new(ExecMode::Sync, None),
        )
}

fn settle(h: &mut Harness<'_, F420App>) {
    let start = Instant::now();
    loop {
        let _ = h.try_run();
        if h.state().idle() {
            let _ = h.try_run();
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(180), "not idle");
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

fn enclosure(l: f64, w: f64, h: f64, t: f64) -> f64 {
    l * w * h - (l - 2.0 * t) * (w - 2.0 * t) * (h - t)
}

#[test]
fn shell_from_the_inspector_and_edit_its_thickness() {
    let mut h = harness();
    click(&mut h, "New");
    h.state_mut().request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 80.0,
            height: 50.0,
            center: None,
        },
    });
    settle(&mut h);
    let sk = h.state().summary().features[0].id;
    h.state_mut().request(Command::extrude(None, sk, 25.0));
    settle(&mut h);
    let ext = h.state().summary().features[1].clone();
    click(&mut h, &ext.name);
    click(&mut h, "Shell…");
    assert!(h.query_by_label("End cap").is_some() && h.query_by_label("Start cap").is_some());
    type_into(&mut h, "Wall thickness (mm)", "2");
    click(&mut h, "OK");
    let v = h.state().volume().expect("volume");
    assert!((v - 19592.0).abs() < 1e-6, "{v}");
    let shell = h.state().summary().features[2].clone();
    assert_eq!(shell.kind, "shell");
    // Edit the thickness from the inspector.
    click(&mut h, &shell.name);
    type_into(&mut h, "thickness (mm)", "3");
    click(&mut h, "Apply");
    let v = h.state().volume().expect("volume");
    assert!((v - enclosure(80.0, 50.0, 25.0, 3.0)).abs() < 1e-6, "{v}");
}
