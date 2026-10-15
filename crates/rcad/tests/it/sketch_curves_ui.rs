//! Ellipse and Spline tools through the real sketch UI: an ellipse by three
//! clicks (centre, axis end, other radius) extrudes to πab × depth; a closed
//! spline by clicking back on its first control point is one region; an
//! open spline finished with Enter keeps its control points editable.

use std::f64::consts::PI;
use std::time::{Duration, Instant};

use eframe::egui::{self, Modifiers, PointerButton, Pos2};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_sketch::Entity;

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

fn sketch(h: &Harness<'_, RcadApp>) -> rcad_sketch::Sketch {
    h.state()
        .sketch_session()
        .expect("sketch session")
        .editor
        .sketch
        .clone()
}

fn feedback(h: &Harness<'_, RcadApp>) -> String {
    format!("{:?}", h.state().sketch_session().unwrap().editor.feedback)
}

#[test]
fn ellipse_tool_makes_an_extrudable_ellipse() {
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Create Sketch");
    click(&mut h, "Ellipse");
    sk_click(&mut h, [0.0, 0.0]);
    sk_click(&mut h, [20.0, 0.0]);
    sk_click(&mut h, [5.0, 10.0]);
    let s = sketch(&h);
    let Some(Entity::Ellipse {
        center,
        major,
        minor,
        ..
    }) = s.entities.first().cloned()
    else {
        panic!("no ellipse: {}", feedback(&h));
    };
    let (c, m) = (s.point_xy(center).unwrap(), s.point_xy(major).unwrap());
    let a = (m[0] - c[0]).hypot(m[1] - c[1]);
    // Grid snapping may round the clicks; the area must match the geometry.
    assert!(a > 15.0 && minor > 5.0, "{a} {minor}");
    let area = s.regions().unwrap()[0].area;
    assert!((area - PI * a * minor).abs() < 1e-8 * area, "{area}");
    click(&mut h, "Finish Sketch");
    click(&mut h, "Extrude…");
    click(&mut h, "OK");
    let v = h.state().volume().expect("volume");
    let want = PI * a * minor * 10.0;
    assert!((v - want).abs() < 1e-6 * want, "{v} vs {want}");
}

#[test]
fn spline_tool_closes_on_the_first_point_and_finishes_open_with_enter() {
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Create Sketch");
    click(&mut h, "Spline");
    for p in [[0.0, 0.0], [20.0, -5.0], [30.0, 15.0], [10.0, 25.0]] {
        sk_click(&mut h, p);
    }
    // Nothing is added until the spline is finished.
    assert!(sketch(&h).entities.is_empty());
    sk_click(&mut h, [0.0, 0.0]);
    let s = sketch(&h);
    let Some(Entity::Spline { points, closed, .. }) = s.entities.first().cloned() else {
        panic!("no spline: {}", feedback(&h));
    };
    assert!(closed);
    assert_eq!(points.len(), 4);
    assert_eq!(s.regions().unwrap().len(), 1);
    // An open spline: four points, then Enter.
    for p in [[40.0, 0.0], [45.0, 10.0], [55.0, 10.0], [60.0, 0.0]] {
        sk_click(&mut h, p);
    }
    h.key_press(egui::Key::Enter);
    settle(&mut h);
    let s = sketch(&h);
    let open: Vec<_> = s
        .entities
        .iter()
        .filter(|e| matches!(e, Entity::Spline { closed: false, .. }))
        .collect();
    assert_eq!(open.len(), 1, "{}", feedback(&h));
    // Trim refuses (not silently ignores) sketches with free-form curves.
    click(&mut h, "Trim");
    sk_click(&mut h, [45.0, 10.0]);
    assert!(
        feedback(&h).contains("ellipses or splines"),
        "{}",
        feedback(&h)
    );
    click(&mut h, "Finish Sketch");
}
