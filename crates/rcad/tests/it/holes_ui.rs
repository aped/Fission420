//! Hole dialog through the real widgets: select a face, Hole…, the centre
//! defaults to the face's centroid, tick Counterbore, OK; exact volume.
use std::f64::consts::PI;
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::SketchPlane;

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
        assert!(start.elapsed() < Duration::from_secs(60), "not idle");
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

#[test]
fn counterbored_hole_from_the_face_dialog() {
    let mut h = harness();
    click(&mut h, "New");
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
        .request(Command::extrude(None, rcad_document::FeatureId(1), 10.0));
    settle(&mut h);
    let top = h
        .state()
        .faces()
        .iter()
        .find(|f| {
            f.plane
                .is_some_and(|p| p.normal[2] > 0.99 && (p.point[2] - 10.0).abs() < 1e-9)
        })
        .map(|f| f.group)
        .expect("top face");
    h.state_mut().select_face(Some(top));
    let _ = h.try_run();
    click(&mut h, "Hole…");
    // Default centre: the face's centroid (20, 15).
    let x = format!(
        "{:?}",
        h.get_by_label("Hole centre X (face mm)").accesskit_node()
    );
    assert!(x.contains("\"20\""), "{x}");
    h.get_by_label("Counterbore").click();
    let _ = h.try_run();
    type_into(&mut h, "Counterbore depth (mm)", "4");
    click(&mut h, "OK");
    let want = 12000.0 - PI * 2.75 * 2.75 * 10.0 - PI * (25.0 - 2.75 * 2.75) * 4.0;
    let v = h.state().volume().unwrap();
    assert!((v - want).abs() < 1e-6, "{v} vs {want}");
    let f = h.state().summary().features.last().unwrap().clone();
    assert_eq!(f.kind, "hole");
    let p: Vec<&str> = f.parameters.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        p,
        [
            "diameter",
            "cbore_diameter",
            "cbore_depth",
            "center_x",
            "center_y"
        ]
    );
}
