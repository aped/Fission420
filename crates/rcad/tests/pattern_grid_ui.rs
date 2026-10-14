//! Rectangular pattern through the Pattern dialog: select the pocket in the
//! history, Pattern…, count 3 / spacing 32 along X and a second direction
//! count 2 / spacing 32: six pockets, closed-form volume.

use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, ExtrudeOp, SketchTemplate};
use rcad_document::{BodyId, FaceRef, FaceRole, SketchPlane};

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

fn type_into(h: &mut Harness<'_, RcadApp>, label: &str, text: &str) {
    h.get_by_label(label).focus();
    let _ = h.try_run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    let _ = h.try_run();
    h.get_by_label(label).type_text(text);
    let _ = h.try_run();
}

#[test]
fn rectangular_pattern_from_the_dialog() {
    let mut h = harness();
    click(&mut h, "New");
    let st = h.state_mut();
    st.request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 100.0,
            height: 70.0,
            center: Some([50.0, 35.0]),
        },
    });
    settle(&mut h);
    let sk = h.state().summary().features[0].id;
    h.state_mut().request(Command::extrude(None, sk, 20.0));
    settle(&mut h);
    let ext = h.state().summary().features[1].id;
    h.state_mut().request(Command::CreateSketch {
        name: None,
        plane: SketchPlane::Face(FaceRef {
            feature: ext,
            role: FaceRole::ExtrudeEndCap,
        }),
        template: SketchTemplate::CenteredRectangle {
            width: 28.0,
            height: 30.0,
            center: Some([18.0, 19.0]),
        },
    });
    settle(&mut h);
    let sk2 = h.state().summary().features[2].id;
    h.state_mut().request(Command::Extrude {
        name: Some("Pocket".into()),
        sketch: sk2,
        distance: 15.0,
        op: ExtrudeOp::Cut,
        target: Some(BodyId(1)),
        through_all: false,
        reverse: false,
        body_name: None,
        region: None,
        options: Default::default(),
    });
    settle(&mut h);
    click(&mut h, "Pocket");
    click(&mut h, "Pattern…");
    type_into(&mut h, "Pattern count", "3");
    type_into(&mut h, "Pattern spacing (mm)", "32");
    type_into(&mut h, "Second direction count", "2");
    type_into(&mut h, "Second direction spacing (mm)", "32");
    click(&mut h, "OK");
    let f = h.state().summary().features.last().unwrap().clone();
    assert_eq!(
        f.kind,
        "linear_pattern",
        "{:?}",
        h.state().diagnostics().last()
    );
    let v = h.state().volume().unwrap();
    let want = 140000.0 - 6.0 * 28.0 * 30.0 * 15.0;
    assert!((v - want).abs() < 1e-6, "{v} vs {want}");
}
