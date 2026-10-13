//! Offset construction plane through the real UI: select a box's top face,
//! "Sketch on offset plane…", 5 mm, OK: the sketch session's plane is 5 mm
//! above the face; a circle drawn there extrudes to a body at z 30..40.

use std::f64::consts::PI;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::{FaceRef, FaceRole, PlaneBase, SketchPlane};

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

fn sk_click(h: &mut Harness<'_, RcadApp>, p: [f64; 2]) {
    let s: Pos2 = h.state().sketch_to_screen(p).expect("visible");
    h.event(egui::Event::PointerMoved(s));
    let _ = h.try_run();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: s,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        let _ = h.try_run();
    }
    let _ = h.try_run();
}

#[test]
fn sketch_on_a_plane_offset_from_the_selected_face() {
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
    let ext = h.state().summary().features[1].id;
    let top = FaceRef {
        feature: ext,
        role: FaceRole::ExtrudeEndCap,
    };
    let group = h
        .state()
        .faces()
        .iter()
        .find(|f| f.reference == Some(top))
        .map(|f| f.group)
        .expect("top face");
    h.state_mut().select_face(Some(group));
    let _ = h.try_run();
    click(&mut h, "Sketch on offset plane…");
    // The base defaults to the selected face (checked by the stored plane below).
    assert!(h.query_all_by_label("Selected face").count() >= 1);
    type_into(&mut h, "Plane offset (mm)", "5");
    click(&mut h, "OK");
    let frame = h.state().sketch_session().expect("sketch session").frame;
    assert!((frame.origin[2] - 30.0).abs() < 1e-9, "{frame:?}");
    click(&mut h, "Circle");
    sk_click(&mut h, [0.0, 0.0]);
    sk_click(&mut h, [5.0, 0.0]);
    click(&mut h, "Finish Sketch");
    let f = h.state().summary().features[2].clone();
    assert_eq!(
        f.plane,
        Some(SketchPlane::Offset {
            base: PlaneBase::Face(top),
            distance: 5.0
        })
    );
    assert!(f.parameters.contains(&("plane_offset".to_string(), 5.0)));
    // A separate body on the offset plane.
    h.state_mut().request(Command::extrude(None, f.id, 10.0));
    settle(&mut h);
    let b = h.state().summary().bodies[1].clone();
    let m = b.measurements.expect("measured");
    assert!((m.bounds.min[2] - 30.0).abs() < 1e-9 && (m.bounds.max[2] - 40.0).abs() < 1e-9);
    if let rcad_kernel::VolumeResult::Value { volume, .. } = m.volume {
        assert!((volume - PI * 25.0 * 10.0).abs() < 1e-3, "{volume}");
    }
}
