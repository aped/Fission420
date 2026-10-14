//! Cap-loop fillet through the real UI: a box with its vertical edges
//! rounded R = 4, pick the top face, "Fillet edge loop…", radius 2, OK; then
//! set the radius to 4 (sphere corners) from the inspector. Volumes are
//! closed forms (spandrel area r²(1 − π/4), first moment r³(5/6 − π/4),
//! Pappus around the corners), matched to 1e-9 relative.

use std::f64::consts::PI;
use std::time::{Duration, Instant};

use eframe::egui::{Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::{Command, SketchTemplate};
use rcad_document::{FaceRef, FaceRole, FeatureId, SketchPlane};

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

fn request(h: &mut Harness<'_, RcadApp>, c: Command) -> FeatureId {
    h.state_mut().request(c);
    settle(h);
    h.state().summary().features.last().expect("feature").id
}

fn want(r: f64) -> f64 {
    let (a, m) = (r * r * (1.0 - PI / 4.0), r.powi(3) * (5.0 / 6.0 - PI / 4.0));
    (1200.0 - (4.0 - PI) * 16.0) * 10.0 - a * (2.0 * 32.0 + 2.0 * 22.0) - 2.0 * PI * (a * 4.0 - m)
}

fn close(v: f64, w: f64) {
    assert!((v - w).abs() < 1e-9 * w, "volume {v} vs closed form {w}");
}

#[test]
fn fillet_the_top_loop_from_a_picked_face_and_edit_its_radius() {
    let mut h = harness();
    click(&mut h, "New");
    let sk = request(
        &mut h,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 40.0,
                height: 30.0,
                center: Some([20.0, 15.0]),
            },
        },
    );
    let ext = request(&mut h, Command::extrude(None, sk, 10.0));
    let side = |e: u32| FaceRef {
        feature: ext,
        role: FaceRole::ExtrudeSide { entity: e },
    };
    for (a, b) in [(1, 2), (2, 3), (3, 4), (4, 1)] {
        request(
            &mut h,
            Command::Fillet {
                name: None,
                a: side(a),
                b: side(b),
                radius: 4.0,
            },
        );
    }
    let top = FaceRef {
        feature: ext,
        role: FaceRole::ExtrudeEndCap,
    };
    let g = h
        .state()
        .faces()
        .iter()
        .find(|f| f.reference == Some(top))
        .map(|f| f.group)
        .expect("top face displayed");
    h.state_mut().select_face(Some(g));
    let _ = h.try_run();
    click(&mut h, "Fillet edge loop…");
    type_into(&mut h, "Fillet radius (mm)", "2");
    click(&mut h, "OK");
    let last = h.state().summary().features.last().unwrap().clone();
    assert_eq!(
        last.kind,
        "fillet_loop",
        "{:?}",
        h.state().diagnostics().last()
    );
    assert!(last.parameters.contains(&("radius".to_string(), 2.0)));
    close(h.state().volume().unwrap(), want(2.0));
    // Edit the radius from the inspector: R = r gives sphere corners.
    click(&mut h, &last.name);
    type_into(&mut h, "radius (mm)", "4");
    click(&mut h, "Apply");
    close(h.state().volume().unwrap(), want(4.0));
    // A radius above the corner radius is refused; the model keeps r = 4.
    type_into(&mut h, "radius (mm)", "5");
    click(&mut h, "Apply");
    let f = h.state().summary().features.last().unwrap().clone();
    assert!(f.parameters.contains(&("radius".to_string(), 4.0)), "{f:?}");
    close(h.state().volume().unwrap(), want(4.0));
}
