//! Create Sketch on a world origin plane through the toolbar's plane menu,
//! also once a body exists and a feature is selected (the empty-document card
//! is gone then): XZ gives a sketch whose frame normal is -Y, YZ +X.

use std::time::{Duration, Instant};

use egui_kittest::{Harness, kittest::Queryable as _};
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
        assert!(start.elapsed() < Duration::from_secs(180), "not idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn click(h: &mut Harness<'_, RcadApp>, label: &str) {
    h.get_by_label(label).click();
    settle(h);
}

#[test]
fn sketch_on_world_planes_from_the_toolbar_menu() {
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
    settle(&mut h);
    let sk = h.state().summary().features[0].id;
    h.state_mut().request(Command::extrude(None, sk, 10.0));
    settle(&mut h);
    assert_eq!(h.state().summary().bodies.len(), 1);
    for (label, normal) in [
        ("Sketch on XZ", [0.0, -1.0, 0.0]),
        ("Sketch on YZ", [1.0, 0.0, 0.0]),
        ("Sketch on XY", [0.0, 0.0, 1.0]),
    ] {
        click(&mut h, "Sketch plane menu");
        click(&mut h, label);
        let frame = h.state().sketch_session().expect("sketch session").frame;
        for (z, n) in frame.z.iter().zip(normal) {
            assert!((z - n).abs() < 1e-12, "{label}: {:?}", frame.z);
        }
        // Leave the empty sketch without creating anything.
        click(&mut h, "Cancel Sketch");
        assert!(
            h.state().sketch_session().is_none(),
            "{label}: session left open"
        );
        assert_eq!(
            h.state().summary().features.len(),
            2,
            "{label}: nothing created"
        );
    }
}
