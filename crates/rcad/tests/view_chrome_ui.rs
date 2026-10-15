//! View cube and navigation bar over the 3D view: a face of the cube gives
//! that standard view, a corner an isometric view from that corner; the
//! navigation bar's Orbit / Pan / Zoom set what a left drag does, Fit and
//! Look At act through the registry, Display style switches edges.
use std::time::{Duration, Instant};

use eframe::egui::{self, PointerButton, Pos2};
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
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

fn yaw_pitch(h: &Harness<'_, RcadApp>) -> (f32, f32) {
    let (c, _) = h.state().view_camera();
    (c.yaw, c.pitch)
}

fn close(a: f32, b: f32) -> bool {
    let d = (a - b).rem_euclid(std::f32::consts::TAU);
    d.min(std::f32::consts::TAU - d) < 1e-3
}

#[test]
fn view_cube_faces_and_corners_orient_the_view() {
    let mut h = harness();
    click(&mut h, "New");
    // Top face of the cube: looking straight down.
    click(&mut h, "ViewCube Top");
    let (yaw, pitch) = yaw_pitch(&h);
    let (ty, tp) = rcad_viewport::ViewPreset::Top.yaw_pitch();
    assert!(close(yaw, ty) && (pitch - tp).abs() < 1e-3, "{yaw} {pitch}");
    // A corner visible from the top: +x −y +z, an isometric view from there.
    click(&mut h, "ViewCube corner +x-y+z");
    let (yaw, pitch) = yaw_pitch(&h);
    assert!(close(yaw, -std::f32::consts::FRAC_PI_4), "{yaw}");
    assert!(
        (pitch - (1.0f32 / 3.0f32.sqrt()).asin()).abs() < 1e-4,
        "{pitch}"
    );
    // From there the Front face is visible.
    click(&mut h, "ViewCube Front");
    let (fy, _) = rcad_viewport::ViewPreset::Front.yaw_pitch();
    let (yaw, pitch) = yaw_pitch(&h);
    assert!(close(yaw, fy) && pitch.abs() < 1e-3, "{yaw} {pitch}");
    // An edge visible from the front: −y +z, 45° down from the front.
    click(&mut h, "ViewCube edge -y+z");
    let (yaw, pitch) = yaw_pitch(&h);
    assert!(
        close(yaw, -std::f32::consts::FRAC_PI_2)
            && (pitch - std::f32::consts::FRAC_PI_4).abs() < 1e-4,
        "{yaw} {pitch}"
    );
    // Faces hidden from the current direction are not offered.
    assert!(h.query_by_label("ViewCube Back").is_none());
}

#[test]
fn navigation_bar_modes_fit_look_at_and_display_style() {
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
    settle(&mut h);
    // Look At is refused without a planar face (reason in the registry).
    assert!(h.get_by_label("Look At").accesskit_node().is_disabled());
    let top = h
        .state()
        .faces()
        .iter()
        .find(|f| {
            f.plane
                .is_some_and(|p| p.normal[2] > 0.99 && p.point[2] > 1.0)
        })
        .map(|f| f.group)
        .expect("top face");
    h.state_mut().select_face(Some(top));
    settle(&mut h);
    click(&mut h, "ViewCube corner +x-y+z");
    click(&mut h, "Look At");
    let (_, pitch) = yaw_pitch(&h);
    assert!(pitch > 1.5, "Look At the top face looks down: {pitch}");
    // Pan mode: a left drag moves the camera target, not its direction.
    click(&mut h, "Pan");
    let before = h.state().view_camera().0;
    let view = h.state().view_rect().unwrap();
    let a = view.center() + egui::vec2(-150.0, 120.0);
    let b = a + egui::vec2(60.0, 0.0);
    drag(&mut h, a, b);
    let after = h.state().view_camera().0;
    assert!((after.yaw - before.yaw).abs() < 1e-6 && (after.pitch - before.pitch).abs() < 1e-6);
    assert!(after.target != before.target, "pan moved the target");
    // Zoom mode: a left drag up brings the camera closer.
    click(&mut h, "Zoom");
    let d0 = h.state().view_camera().0.distance;
    drag(&mut h, a, a + egui::vec2(0.0, -80.0));
    assert!(h.state().view_camera().0.distance < d0);
    click(&mut h, "Orbit");
    // Display style menu.
    click(&mut h, "Display style");
    click(&mut h, "Shaded");
    click(&mut h, "Display style");
    h.get_by_label("Shaded with edges");
    let _ = PointerButton::Primary;
}

fn drag(h: &mut Harness<'_, RcadApp>, a: Pos2, b: Pos2) {
    h.event(egui::Event::PointerMoved(a));
    h.step();
    h.event(egui::Event::PointerButton {
        pos: a,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    h.step();
    for k in 1..=8 {
        h.event(egui::Event::PointerMoved(a + (b - a) * (k as f32 / 8.0)));
        h.step();
    }
    h.event(egui::Event::PointerButton {
        pos: b,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    settle(h);
}

/// In sketch mode at 1280 × 800 the sketch status line and the tool hint
/// (long ones: Line, Dimension, Spline, Text) stay clear of the view cube,
/// wrapping in the narrower area instead of running under it.
#[test]
fn sketch_banners_stay_clear_of_the_view_cube() {
    let mut h = Harness::builder()
        .with_size([1280.0, 800.0])
        .with_max_steps(64)
        .build_ui_state(
            |ui, app: &mut RcadApp| app.ui(ui),
            RcadApp::new(ExecMode::Sync, None),
        );
    click(&mut h, "New");
    click(&mut h, "Create Sketch");
    for tool in ["Line", "Dimension", "Spline", "Text"] {
        click(&mut h, tool);
        let (chips, cube) = h.state().view_chrome_rects();
        let cube = cube.expect("view cube drawn");
        assert!(chips.len() >= 2, "status and hint lines: {chips:?}");
        for c in &chips {
            assert!(
                !c.intersects(cube),
                "{tool}: banner {c:?} runs under the view cube {cube:?}"
            );
        }
    }
}
