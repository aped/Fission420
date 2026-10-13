//! SKETCH corner fillet through the real UI: draw a 40 x 20 rectangle, Fillet
//! tool from the toolbar, click a corner, type 5, Enter; refused radius keeps
//! the sketch; extruded volume (800 − 25(1 − π/4))·10.

use std::f64::consts::PI;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers, PointerButton, Pos2};
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

fn arcs(h: &Harness<'_, RcadApp>) -> usize {
    h.state()
        .sketch_session()
        .unwrap()
        .editor
        .sketch
        .entities
        .iter()
        .filter(|e| matches!(e, Entity::Arc { .. }))
        .count()
}

#[test]
fn fillet_tool_rounds_a_corner() {
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Create Sketch");
    click(&mut h, "2-Point Rect");
    sk_click(&mut h, [0.0, 0.0]);
    sk_click(&mut h, [40.0, 20.0]);
    click(&mut h, "Fillet");
    sk_click(&mut h, [40.0, 20.0]);
    let label = "Fillet radius (mm)";
    assert!(h.query_by_label(label).is_some(), "inline entry opened");
    // Too large for the 20 mm side: refused, entry stays, nothing added.
    type_into(&mut h, label, "25");
    h.key_press(Key::Enter);
    let _ = h.try_run();
    assert_eq!(arcs(&h), 0);
    let fb = h.state().sketch_session().unwrap().editor.feedback.clone();
    assert!(
        fb.as_ref()
            .is_some_and(|f| f.error && f.text.contains("does not fit")),
        "{fb:?}"
    );
    type_into(&mut h, label, "5");
    let pv = h.state().sketch_session().unwrap().editor.fillet_preview();
    assert!(matches!(pv, Some(Ok(_))), "{pv:?}");
    h.key_press(Key::Enter);
    let _ = h.try_run();
    let _ = h.try_run();
    assert_eq!(arcs(&h), 1);
    click(&mut h, "Finish Sketch");
    let f = h.state().summary().features[0].clone();
    assert!(f.profile_error.is_none(), "{:?}", f.profile_error);
    assert!(f.parameters.contains(&("fillet".to_string(), 5.0)));
    click(&mut h, "Extrude…");
    click(&mut h, "OK");
    let v = h.state().volume().expect("volume");
    let want = (800.0 - 25.0 * (1.0 - PI / 4.0)) * 10.0;
    assert!((v - want).abs() < 1e-3, "{v} vs {want}");
}
