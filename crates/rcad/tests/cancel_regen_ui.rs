//! Cancel a slow regeneration from the toolbar (threaded engine): a plate
//! with a 40-hole pattern, its thickness edited, and "Cancel" pressed while
//! the worker regenerates the pattern. The edit is refused as cancelled and
//! the model keeps its previous thickness and revision. Own test binary: the
//! cancellation flag is process-wide.

use std::time::{Duration, Instant};

use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, ExtrudeOp, SketchTemplate};
use rcad_document::{BodyId, FaceRef, FaceRole, FeatureId, SketchPlane};

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
        if h.state().idle() {
            let _ = h.try_run();
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(600), "not idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn cancel_a_slow_regeneration() {
    let mut h = harness();
    let app = h.state_mut();
    app.request(Command::NewDocument);
    app.request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 300.0,
            height: 60.0,
            center: Some([150.0, 30.0]),
        },
    });
    app.request(Command::extrude(None, FeatureId(1), 5.0));
    app.request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::Face(FaceRef {
            feature: FeatureId(2),
            role: FaceRole::ExtrudeEndCap,
        }),
        template: SketchTemplate::CenteredCircle {
            diameter: 3.0,
            center: Some([5.0, 30.0]),
        },
    });
    app.request(Command::Extrude {
        name: None,
        sketch: FeatureId(3),
        distance: 0.0,
        op: ExtrudeOp::Cut,
        target: Some(BodyId(1)),
        through_all: true,
        reverse: false,
        body_name: None,
        region: None,
        options: Default::default(),
    });
    app.request(Command::LinearPattern {
        name: None,
        feature: FeatureId(4),
        count: 40,
        spacing: 7.0,
        direction: [1.0, 0.0],
        second: None,
    });
    settle(&mut h);
    let rev = h.state().summary().revision;
    let v0 = h.state().volume().unwrap();
    h.state_mut().request(Command::SetParameter {
        feature: FeatureId(2),
        name: "distance".into(),
        value: 6.0,
    });
    // Wait until the worker is busy, then press Cancel.
    let start = Instant::now();
    while !h.state().busy() {
        let _ = h.try_run();
        assert!(start.elapsed() < Duration::from_secs(10), "never busy");
    }
    let _ = h.try_run();
    h.get_by_label("Cancel regeneration").click();
    settle(&mut h);
    assert_eq!(h.state().summary().revision, rev, "the edit was committed");
    assert!((h.state().volume().unwrap() - v0).abs() < 1e-9);
    let d = h.state().diagnostics();
    assert!(
        d.iter().any(|m| m.kind == "Cancelled"),
        "no cancellation message: {:?}",
        d.iter().rev().take(3).map(|m| &m.text).collect::<Vec<_>>()
    );
}
