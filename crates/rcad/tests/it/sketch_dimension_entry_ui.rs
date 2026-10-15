//! Parametric dimension entry in the sketch editor's inline field, through
//! real widgets and pointer events: `name = expr` creates a user parameter
//! and binds the dimension to it, an expression over parameters binds, unit
//! suffixes convert, invalid input keeps the field open with the reason and
//! leaves the sketch unchanged, and Finish commits sketch, parameter and
//! bindings in one undo step so a later parameter edit drives the sketch.

use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};
use rcad_commands::Command;

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
    settle(h);
}

fn feedback(h: &Harness<'_, RcadApp>) -> String {
    h.state()
        .sketch_session()
        .and_then(|s| s.editor.feedback.as_ref())
        .map(|f| f.text.clone())
        .unwrap_or_default()
}

fn dims(h: &Harness<'_, RcadApp>) -> Vec<(String, f64)> {
    h.state()
        .sketch_session()
        .unwrap()
        .editor
        .sketch
        .dimensions()
}

/// Type into the open inline editor for `name` and press Enter.
fn type_dim(h: &mut Harness<'_, RcadApp>, name: &str, text: &str) {
    let label = format!("Edit {name} (mm)");
    let _ = h.try_run();
    assert!(
        h.query_by_label(&label).is_some(),
        "no inline editor {label}: {}",
        feedback(h)
    );
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    let _ = h.try_run();
    h.get_by_label(&label).type_text(text);
    let _ = h.try_run();
    h.key_press(Key::Enter);
    let _ = h.try_run();
    settle(h);
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Centre rectangle with its corner near (30, 20), Dimension tool active.
fn rectangle(h: &mut Harness<'_, RcadApp>) {
    click(h, "New");
    click(h, "Create Sketch");
    click(h, "Center Rect");
    sk_click(h, [0.2, 0.1]);
    sk_click(h, [30.0, 20.0]);
    click(h, "Dimension");
}

#[test]
fn name_equals_creates_a_parameter_and_expressions_bind_dimensions() {
    let mut h = harness();
    rectangle(&mut h);
    sk_click(&mut h, [0.0, -20.0]);
    type_dim(&mut h, "width", "w = 40");
    sk_click(&mut h, [20.0, 0.0]);
    type_dim(&mut h, "height", "w/2");
    let d = dims(&h);
    assert!(close(d[0].1, 40.0) && close(d[1].1, 20.0), "{d:?}");
    assert!(feedback(&h).contains("w/2"), "{}", feedback(&h));
    let undo_before = h.state().summary().can_undo.clone();
    click(&mut h, "Finish Sketch");
    assert!(h.state().sketch_session().is_none(), "finished");
    let s = h.state().summary();
    let w = s
        .parameters
        .iter()
        .find(|p| p.name == "w")
        .expect("w created");
    assert_eq!((w.expression.as_str(), w.value), ("40", Some(40.0)));
    let f = &s.features[0];
    assert_eq!(f.expressions.get("width").map(String::as_str), Some("w"));
    assert_eq!(f.expressions.get("height").map(String::as_str), Some("w/2"));
    assert_ne!(s.can_undo, undo_before);
    // The parameter now drives the sketch.
    h.state_mut().request(Command::SetUserParameter {
        name: "w".into(),
        expression: "60".into(),
        unit: None,
        comment: None,
    });
    settle(&mut h);
    let f = &h.state().summary().features[0];
    let get = |n: &str| f.parameters.iter().find(|(m, _)| m == n).unwrap().1;
    assert!(close(get("width"), 60.0) && close(get("height"), 30.0));
}

#[test]
fn unit_suffixes_convert_and_invalid_input_keeps_the_field_open() {
    let mut h = harness();
    rectangle(&mut h);
    sk_click(&mut h, [0.0, -20.0]);
    type_dim(&mut h, "width", "1 in");
    assert!(close(dims(&h)[0].1, 25.4), "{:?}", dims(&h));
    sk_click(&mut h, [12.7, 0.0]);
    type_dim(&mut h, "height", "2 cm");
    assert!(close(dims(&h)[1].1, 20.0), "{:?}", dims(&h));
    // Edit the width: unknown parameter, then a non-positive result.
    let before = h.state().sketch_session().unwrap().editor.sketch.clone();
    let label = h
        .state()
        .dimension_label("width")
        .expect("width label drawn");
    click(&mut h, "Select");
    for p in [label.center(), label.center()] {
        h.event(egui::Event::PointerMoved(p));
        button(&mut h, p, true);
        button(&mut h, p, false);
    }
    settle(&mut h);
    for bad in ["q / 2", "10 - 30", "w = "] {
        type_dim(&mut h, "width", bad);
        assert!(
            h.query_by_label("Edit width (mm)").is_some(),
            "{bad}: the field stays open"
        );
        assert!(
            h.state()
                .sketch_session()
                .unwrap()
                .editor
                .feedback
                .as_ref()
                .is_some_and(|f| f.error),
            "{bad}: {}",
            feedback(&h)
        );
        assert_eq!(
            h.state().sketch_session().unwrap().editor.sketch,
            before,
            "{bad}: sketch unchanged"
        );
    }
    assert!(
        feedback(&h).contains("not a valid dimension"),
        "{}",
        feedback(&h)
    );
    h.key_press(Key::Escape);
    settle(&mut h);
    click(&mut h, "Finish Sketch");
    assert!(
        h.state().summary().features[0].expressions.is_empty(),
        "numbers bind nothing"
    );
}

fn editor<'a>(h: &'a Harness<'_, RcadApp>) -> &'a rcad::sketch_edit::SketchEditor {
    &h.state().sketch_session().expect("sketch session").editor
}

/// Type `text` into the heads-up field `label`.
fn hud_type(h: &mut Harness<'_, RcadApp>, label: &str, text: &str) {
    assert!(
        h.query_by_label(label).is_some(),
        "no heads-up field {label}: {:?}",
        editor(h).hud
    );
    h.get_by_label(label).type_text(text);
    let _ = h.try_run();
}

#[test]
fn heads_up_typed_values_place_points_and_dimension_them() {
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Create Sketch");
    // Line: start at (10, 5), Tab, length 50, Tab, angle 30, Enter.
    click(&mut h, "Line");
    sk_click(&mut h, [10.0, 5.0]);
    h.key_press(Key::Tab);
    settle(&mut h);
    hud_type(&mut h, "Length (mm)", "50");
    h.key_press(Key::Tab);
    let _ = h.try_run();
    assert_eq!(
        editor(&h).hud.as_ref().map(|x| x.focus),
        Some(1),
        "Tab cycles"
    );
    hud_type(&mut h, "Angle (deg)", "30");
    h.key_press(Key::Enter);
    settle(&mut h);
    let sk = editor(&h).sketch.clone();
    let end = sk
        .entities
        .iter()
        .find_map(|e| match e {
            rcad_sketch::Entity::Line { b, .. } => sk.point_xy(*b),
            _ => None,
        })
        .expect("line drawn");
    let want = [
        10.0 + 50.0 * 30f64.to_radians().cos(),
        5.0 + 50.0 * 30f64.to_radians().sin(),
    ];
    assert!(
        (end[0] - want[0]).abs() < 1e-9 && (end[1] - want[1]).abs() < 1e-9,
        "{end:?} vs {want:?}: {}",
        feedback(&h)
    );
    assert!(
        sk.dimensions().iter().any(|(_, v)| (v - 50.0).abs() < 1e-9),
        "typed length dimensioned: {:?}",
        sk.dimensions()
    );
    h.key_press(Key::Escape);
    settle(&mut h);
    // Centre rectangle 40 x 20 about (0, -40).
    click(&mut h, "Center Rect");
    sk_click(&mut h, [0.0, -40.0]);
    sk_click_move(&mut h, [15.0, -30.0]);
    h.key_press(Key::Tab);
    settle(&mut h);
    hud_type(&mut h, "Width (mm)", "40");
    h.key_press(Key::Tab);
    let _ = h.try_run();
    hud_type(&mut h, "Height (mm)", "20");
    h.key_press(Key::Enter);
    settle(&mut h);
    let d = editor(&h).sketch.dimensions();
    assert!(
        d.iter()
            .any(|(n, v)| n.starts_with("width") && (v - 40.0).abs() < 1e-9)
            && d.iter()
                .any(|(n, v)| n.starts_with("height") && (v - 20.0).abs() < 1e-9),
        "{d:?}: {}",
        feedback(&h)
    );
    // Invalid typed input keeps the fields open and changes nothing.
    click(&mut h, "Circle");
    sk_click(&mut h, [60.0, 0.0]);
    let before = editor(&h).sketch.clone();
    h.key_press(Key::Tab);
    settle(&mut h);
    hud_type(&mut h, "Diameter (mm)", "abc");
    h.key_press(Key::Enter);
    settle(&mut h);
    assert!(editor(&h).hud.is_some(), "fields stay open");
    assert!(
        editor(&h).feedback.as_ref().is_some_and(|f| f.error),
        "{}",
        feedback(&h)
    );
    assert_eq!(editor(&h).sketch, before);
}

fn sk_click_move(h: &mut Harness<'_, RcadApp>, p: [f64; 2]) {
    let s = h.state().sketch_to_screen(p).expect("visible");
    h.event(egui::Event::PointerMoved(s));
    let _ = h.try_run();
}
