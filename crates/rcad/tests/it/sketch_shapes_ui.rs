//! Slot and Polygon tools through the real sketch UI: a slot by
//! three clicks (centres, then width) extrudes to L·W + π(W/2)² × depth; a
//! hexagon by two clicks keeps 3 DOF and its diameter dimension edits the
//! size (area (3√3/2)R²).

use std::f64::consts::PI;
use std::time::{Duration, Instant};

use eframe::egui::{self, Modifiers, PointerButton, Pos2};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::sketch_edit::SketchTool;
use rcad::{ExecMode, RcadApp};

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
fn slot_tool_makes_an_extrudable_tangent_slot() {
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Create Sketch");
    click(&mut h, "Slot");
    sk_click(&mut h, [-15.0, 3.0]);
    sk_click(&mut h, [15.0, 3.0]);
    // Width point 4 mm beside the centre line: W = 8.
    sk_click(&mut h, [0.0, 7.0]);
    let s = sketch(&h);
    assert_eq!(s.entities.len(), 4, "{}", feedback(&h));
    let dims = s.dimensions();
    let get = |n: &str| dims.iter().find(|(m, _)| m == n).map(|(_, v)| *v);
    let (l, w) = (get("slot_length").unwrap(), get("slot_width").unwrap());
    // Grid snapping may round the clicks; the area must match the dimensions.
    let area = s.regions().unwrap()[0].area;
    let expect = l * w + PI * (w / 2.0).powi(2);
    assert!((area - expect).abs() < 1e-6 * expect, "{area} vs {expect}");
    click(&mut h, "Finish Sketch");
    click(&mut h, "Extrude…");
    click(&mut h, "OK");
    let v = h.state().volume().expect("volume");
    assert!(
        (v - expect * 10.0).abs() < 1e-6 * v,
        "{v} vs {}",
        expect * 10.0
    );
}

#[test]
fn polygon_tool_makes_an_editable_hexagon() {
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Create Sketch");
    click(&mut h, "Polygon");
    assert_eq!(
        h.state().sketch_session().unwrap().editor.tool,
        SketchTool::Polygon
    );
    assert!(h.query_by_label("Sides").is_some(), "sides control shown");
    sk_click(&mut h, [0.0, 0.0]);
    sk_click(&mut h, [10.0, 0.0]);
    let s = sketch(&h);
    let lines = s
        .entities
        .iter()
        .filter(|e| matches!(e, rcad_sketch::Entity::Line { .. }))
        .count();
    assert_eq!(lines, 6, "{}", feedback(&h));
    let d = s
        .dimensions()
        .into_iter()
        .find(|(n, _)| n == "polygon_d")
        .expect("diameter dimension")
        .1;
    let r = d / 2.0;
    let area = s.regions().unwrap()[0].area;
    let expect = 1.5 * 3f64.sqrt() * r * r;
    assert!((area - expect).abs() < 1e-6 * expect, "{area} vs {expect}");
    click(&mut h, "Finish Sketch");
    let f = h.state().summary().features[0].clone();
    assert!(f.profile_error.is_none(), "{:?}", f.profile_error);
}
