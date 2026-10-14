//! Trim on circles through the real sketch UI:
//! a rectangle with a circle centred on its right edge; trimming the edge
//! inside the circle holds the new ends on the circle, trimming the circle
//! inside the rectangle shares them, so the line/arc profile closes and
//! extrudes to the analytic volume; the arc keeps the circle's entity id.
//! Pointer events go through the fallback camera and the real view rect,
//! like tests/sketch_workflow.rs.

use std::f64::consts::PI;
use std::time::{Duration, Instant};

use eframe::egui::{self, Modifiers, PointerButton, Pos2};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_sketch::{Constraint, Entity};

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

fn button(h: &mut Harness<'_, RcadApp>, pos: Pos2, pressed: bool) {
    h.event(egui::Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
    let _ = h.try_run();
}

fn sk_click(h: &mut Harness<'_, RcadApp>, p: [f64; 2]) {
    let s = h.state().sketch_to_screen(p).expect("visible");
    h.event(egui::Event::PointerMoved(s));
    let _ = h.try_run();
    button(h, s, true);
    button(h, s, false);
    let _ = h.try_run();
}

fn sketch(h: &Harness<'_, RcadApp>) -> rcad_sketch::Sketch {
    h.state()
        .sketch_session()
        .expect("sketch session")
        .editor
        .sketch
        .clone()
}

#[test]
fn trim_against_a_circle_closes_the_profile_and_keeps_the_circle_id() {
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Create Sketch");
    click(&mut h, "2-Point Rect");
    sk_click(&mut h, [0.0, 0.0]);
    sk_click(&mut h, [40.0, 20.0]);
    click(&mut h, "Circle");
    sk_click(&mut h, [40.0, 10.0]);
    sk_click(&mut h, [46.0, 10.0]);
    let s = sketch(&h);
    let circle = s
        .entities
        .iter()
        .find(|e| matches!(e, Entity::Circle { .. }))
        .map(|e| e.id())
        .expect("circle drawn");
    // The drawn (snapped) radius and centre: the centre must sit on the right
    // edge so the kept arc is exactly a half circle.
    let Some(Entity::Circle { center, radius, .. }) = s.entity(circle).cloned() else {
        panic!()
    };
    let c = s.point_xy(center).unwrap();
    assert!(
        (c[0] - 40.0).abs() < 1e-9 && radius > 2.0 && radius < 10.0,
        "{c:?} {radius}"
    );
    click(&mut h, "Trim");
    sk_click(&mut h, [40.0, 10.0 + 0.5 * radius]);
    let s = sketch(&h);
    let held = s
        .constraints
        .iter()
        .filter(|c| matches!(c, Constraint::PointOnCircle { circle: c0, .. } if *c0 == circle))
        .count();
    assert_eq!(
        held,
        2,
        "{:?}",
        h.state().sketch_session().unwrap().editor.feedback
    );
    sk_click(&mut h, [40.0 - radius + 0.2, 10.0]);
    let s = sketch(&h);
    assert!(
        matches!(s.entity(circle), Some(Entity::Arc { .. })),
        "{:?}",
        h.state().sketch_session().unwrap().editor.feedback
    );
    click(&mut h, "Finish Sketch");
    let f = h.state().summary().features[0].clone();
    assert!(f.profile_error.is_none(), "{:?}", f.profile_error);
    click(&mut h, "Extrude…");
    click(&mut h, "OK");
    let v = h.state().volume().expect("volume");
    // Rectangle plus the half disc outside it, extruded 10 mm (default).
    let want = (800.0 + 0.5 * PI * radius * radius) * 10.0;
    assert!((v - want).abs() <= 0.005, "{v} vs {want}");
}
