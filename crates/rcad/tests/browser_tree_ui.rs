//! Browser nodes in Fusion's order: Document Settings (units), Named Views,
//! Origin, then Bodies, Sketches, Construction; Origin planes start sketches,
//! Named Views set the camera, Sketches rows select and edit.
use std::time::{Duration, Instant};

use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::{FeatureId, PlaneBase, SketchPlane};

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
fn browser_nodes_follow_fusion_order_and_act() {
    let mut h = harness();
    click(&mut h, "New");
    h.state_mut().request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 20.0,
            height: 10.0,
            center: None,
        },
    });
    h.state_mut()
        .request(Command::extrude(None, FeatureId(1), 5.0));
    h.state_mut().request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::Offset {
            base: PlaneBase::WorldXY,
            distance: 12.0,
            angle: 0.0,
        },
        template: SketchTemplate::CenteredCircle {
            diameter: 4.0,
            center: None,
        },
    });
    settle(&mut h);
    // Node order, top to bottom (then left to right on the first row).
    let pos = |h: &Harness<'_, RcadApp>, l: &str| {
        let r = h.get_by_label(l).rect();
        (r.min.y, r.min.x)
    };
    let order = [
        "Document Settings",
        "Named Views",
        "Origin",
        "Sketches (2)",
        "Construction (1)",
    ];
    for w in order.windows(2) {
        let (a, b) = (pos(&h, w[0]), pos(&h, w[1]));
        assert!(
            a.0 < b.0 - 1.0 || ((a.0 - b.0).abs() < 1.0 && a.1 < b.1),
            "{} is not before {}: {a:?} {b:?}",
            w[0],
            w[1]
        );
    }
    let bodies = h.get_by_label_contains("Bodies (1)").rect().min.y;
    assert!(pos(&h, "Origin").0 < bodies && bodies < pos(&h, "Sketches (2)").0);
    // Document Settings ▸ Units.
    click(&mut h, "Document Settings");
    h.get_by_label_contains("millimetre");
    h.key_press(eframe::egui::Key::Escape);
    settle(&mut h);
    // Sketches node rows select (History keeps the bare names).
    click(&mut h, "Sketches (2)");
    click(&mut h, "Browser Sketch2");
    assert_eq!(h.state().selected().map(|f| f.0), Some(3));
    // Origin ▸ XZ starts a sketch on world XZ.
    click(&mut h, "Origin");
    click(&mut h, "Origin XZ Plane");
    let s = h.state().sketch_session().expect("sketch");
    assert!(matches!(
        s.target,
        rcad::SketchTarget::New {
            plane: SketchPlane::WorldXZ
        }
    ));
}
