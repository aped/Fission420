//! Sketch Pattern dialog through the real UI (run 6): select a circle, Pattern…,
//! Circular 6 × 60° about the origin, Apply: six holes constrained to the
//! source; Cancel adds nothing; a rectangular 3 × 2 grid of a rectangle gives
//! six regions; Finish keeps the pattern dimensions as feature parameters.

use std::f64::consts::PI;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers, PointerButton, Pos2};
use egui_kittest::{Harness, kittest::Queryable as _};
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

fn feedback(h: &Harness<'_, RcadApp>) -> String {
    format!("{:?}", h.state().sketch_session().unwrap().editor.feedback)
}

fn circles(s: &rcad_sketch::Sketch) -> usize {
    s.entities
        .iter()
        .filter(|e| matches!(e, rcad_sketch::Entity::Circle { .. }))
        .count()
}

/// Select every curve (selection by clicking is covered by sketch_workflow.rs).
fn select_all(h: &mut Harness<'_, RcadApp>) {
    let ed = &mut h.state_mut().sketch_session_mut().unwrap().editor;
    ed.selection = ed
        .sketch
        .entities
        .iter()
        .map(|e| rcad::sketch_edit::Item::Entity(e.id()))
        .collect();
    let _ = h.try_run();
}

#[test]
fn circular_pattern_of_a_hole_from_the_dialog() {
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Create Sketch");
    click(&mut h, "Circle");
    sk_click(&mut h, [20.0, 0.0]);
    sk_click(&mut h, [23.0, 0.0]);
    assert_eq!(circles(&sketch(&h)), 1);
    click(&mut h, "Select");
    select_all(&mut h);
    assert_eq!(
        h.state()
            .sketch_session()
            .unwrap()
            .editor
            .selected_entities()
            .len(),
        1,
        "circle selected"
    );
    click(&mut h, "Pattern…");
    click(&mut h, "Cancel pattern");
    assert_eq!(circles(&sketch(&h)), 1);

    click(&mut h, "Pattern…");
    click(&mut h, "Circular");
    type_into(&mut h, "Instances", "6");
    type_into(&mut h, "Angle between (°)", "60");
    click(&mut h, "Apply pattern");
    let s = sketch(&h);
    assert_eq!(circles(&s), 6, "{}", feedback(&h));
    let r = s
        .entities
        .iter()
        .find_map(|e| match e {
            rcad_sketch::Entity::Circle { radius, .. } => Some(*radius),
            _ => None,
        })
        .unwrap();
    let rs = s.regions().unwrap();
    assert_eq!(rs.len(), 6);
    assert!(
        rs.iter()
            .all(|x| (x.area - PI * r * r).abs() < 1e-6 * x.area),
        "{rs:?}"
    );
    click(&mut h, "Finish Sketch");
    let f = h.state().summary().features[0].clone();
    assert!(
        f.parameters
            .iter()
            .any(|(n, v)| n == "pattern_angle" && *v == 60.0),
        "{:?}",
        f.parameters
    );
}

#[test]
fn rectangular_grid_from_the_dialog() {
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Create Sketch");
    click(&mut h, "2-Point Rect");
    sk_click(&mut h, [0.0, 0.0]);
    sk_click(&mut h, [6.0, 4.0]);
    click(&mut h, "Select");
    select_all(&mut h);
    assert_eq!(
        h.state()
            .sketch_session()
            .unwrap()
            .editor
            .selected_entities()
            .len(),
        4
    );
    click(&mut h, "Pattern…");
    type_into(&mut h, "Instances", "3");
    type_into(&mut h, "Step x (mm)", "10");
    type_into(&mut h, "Step y (mm)", "0");
    click(&mut h, "Second direction");
    type_into(&mut h, "Instances 2", "2");
    type_into(&mut h, "Step 2 x (mm)", "0");
    type_into(&mut h, "Step 2 y (mm)", "8");
    click(&mut h, "Apply pattern");
    let s = sketch(&h);
    let rs = s
        .regions()
        .unwrap_or_else(|e| panic!("{e}: {}", feedback(&h)));
    assert_eq!(rs.len(), 6, "{}", feedback(&h));
    // Grid snapping may round the drawn rectangle; every copy matches it and
    // sits at (10 i, 8 j) from it.
    let a0 = rs[0].area;
    assert!(rs.iter().all(|r| (r.area - a0).abs() < 1e-6), "{rs:?}");
    let low = |r: &rcad_sketch::SketchRegion| match &r.region.outer {
        rcad_sketch::Profile::Polygon(v) => v[0],
        _ => panic!(),
    };
    let o = low(&rs[0]);
    let mut offs: Vec<(i64, i64)> = rs
        .iter()
        .map(|r| {
            let p = low(r);
            ((p[0] - o[0]).round() as i64, (p[1] - o[1]).round() as i64)
        })
        .collect();
    offs.sort();
    assert_eq!(
        offs,
        vec![(0, 0), (0, 8), (10, 0), (10, 8), (20, 0), (20, 8)]
    );
}
