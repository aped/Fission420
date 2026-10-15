//! Display isolation through the body list's context menu: Isolate draws
//! only that body (view state: the document and its revision are
//! unchanged), "Show all" restores every visible body.

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

#[test]
fn isolate_and_show_all() {
    let mut h = harness();
    h.get_by_label("New").click();
    settle(&mut h);
    for (i, c) in [[0.0, 0.0], [50.0, 0.0]].into_iter().enumerate() {
        h.state_mut().request(Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 20.0,
                height: 10.0,
                center: Some(c),
            },
        });
        settle(&mut h);
        let sk = h.state().summary().features[2 * i].id;
        h.state_mut().request(Command::extrude(None, sk, 5.0));
        settle(&mut h);
    }
    assert_eq!(h.state().scene().bodies.len(), 2);
    let rev = h.state().summary().revision;
    let doc = h.state().summary().clone();
    h.get_by_label("Body2").click_secondary();
    let _ = h.try_run();
    h.get_by_label("Isolate body").click();
    settle(&mut h);
    let s = h.state().scene();
    assert_eq!(s.bodies.len(), 1);
    assert_eq!(s.bodies[0].id.0, 2);
    // Only Body2's triangles, all at x > 30.
    assert!(s.mesh.positions.iter().all(|p| p[0] > 30.0));
    assert_eq!(
        h.state().summary().revision,
        rev,
        "isolation is not an edit"
    );
    assert_eq!(h.state().summary().bodies, doc.bodies);
    h.get_by_label("Show all bodies").click();
    settle(&mut h);
    assert_eq!(h.state().scene().bodies.len(), 2);
    assert!(h.state().isolated().is_none());
}
