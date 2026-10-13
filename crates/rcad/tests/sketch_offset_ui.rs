//! Offset tool through the real sketch UI (SESSION_4 §7 item 3): draw a
//! 40 x 20 rectangle, press O, click its bottom edge, type −2 in the inline
//! entry (dashed preview), Enter: the inner copy and its `offset` dimension
//! are added; the finished sketch's ring extrudes to the analytic volume, and
//! editing the `offset` parameter from the inspector moves the wall.

use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
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

fn sketch(h: &Harness<'_, RcadApp>) -> rcad_sketch::Sketch {
    h.state()
        .sketch_session()
        .expect("sketch session")
        .editor
        .sketch
        .clone()
}

#[test]
fn offset_tool_adds_an_editable_wall() {
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Create Sketch");
    click(&mut h, "2-Point Rect");
    sk_click(&mut h, [0.0, 0.0]);
    sk_click(&mut h, [40.0, 20.0]);
    assert_eq!(sketch(&h).entities.len(), 4);
    h.key_press(Key::O);
    let _ = h.try_run();
    assert_eq!(
        h.state().sketch_session().unwrap().editor.tool,
        rcad::sketch_edit::SketchTool::Offset
    );
    sk_click(&mut h, [20.0, 0.0]);
    let label = "Offset distance (mm, + out / − in)";
    assert!(h.query_by_label(label).is_some(), "inline entry opened");
    type_into(&mut h, label, "-2");
    // Live preview of the copy before committing.
    let pv = h.state().sketch_session().unwrap().editor.offset_preview();
    assert!(matches!(pv, Some(Ok(_))), "{pv:?}");
    assert_eq!(sketch(&h).entities.len(), 4, "nothing added before Enter");
    h.key_press(Key::Enter);
    let _ = h.try_run();
    let _ = h.try_run();
    let s = sketch(&h);
    let lines = s
        .entities
        .iter()
        .filter(|e| matches!(e, Entity::Line { .. }))
        .count();
    assert_eq!(
        lines,
        8,
        "{:?}",
        h.state().sketch_session().unwrap().editor.feedback
    );
    assert!(s.constraints.iter().any(
        |c| matches!(c, Constraint::Offset { dim, .. } if dim.name == "offset" && dim.value == 2.0)
    ));
    click(&mut h, "Finish Sketch");
    let f = h.state().summary().features[0].clone();
    assert!(f.profile_error.is_none(), "{:?}", f.profile_error);
    assert!(f.parameters.contains(&("offset".to_string(), 2.0)));
    click(&mut h, "Extrude…");
    click(&mut h, "OK");
    let v = h.state().volume().expect("volume");
    assert!((v - (800.0 - 36.0 * 16.0) * 10.0).abs() < 1e-3, "{v}");
    // Edit the wall from the inspector: the copy follows.
    click(&mut h, &f.name);
    type_into(&mut h, "offset (mm)", "3");
    click(&mut h, "Apply");
    // The drawn rectangle has no dimensions of its own, so the least-squares
    // solve may also move it; the copy must stay exactly 3 mm inside whatever
    // the source now is: ring = W·H − (W − 6)(H − 6) for the current W × H.
    let s = h.state().summary().features[0].sketch.clone().unwrap();
    let xs: Vec<f64> = s.points.iter().map(|p| p.x).collect();
    let ys: Vec<f64> = s.points.iter().map(|p| p.y).collect();
    let w =
        xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min);
    let hh =
        ys.iter().cloned().fold(f64::MIN, f64::max) - ys.iter().cloned().fold(f64::MAX, f64::min);
    let want = (w * hh - (w - 6.0) * (hh - 6.0)) * 10.0;
    let v = h.state().volume().expect("volume");
    assert!((v - want).abs() < 1e-3, "{v} vs {want} ({w} x {hh})");
    assert!(s.dimensions().contains(&("offset".to_string(), 3.0)));
    let _ = h.get_by_label("Undo").accesskit_node();
}
