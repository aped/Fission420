//! Fix / Unfix, Concentric, Collinear, Symmetric and Driven / Driving
//! through the sketch inspector's buttons with real pointer selection.

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

fn button(h: &mut Harness<'_, RcadApp>, pos: Pos2, pressed: bool, modifiers: Modifiers) {
    h.event(egui::Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers,
    });
    let _ = h.try_run();
}

fn sk_click_mod(h: &mut Harness<'_, RcadApp>, p: [f64; 2], m: Modifiers) {
    let s = h.state().sketch_to_screen(p).expect("visible");
    h.event(egui::Event::PointerMoved(s));
    let _ = h.try_run();
    button(h, s, true, m);
    button(h, s, false, m);
    settle(h);
}

fn sk_click(h: &mut Harness<'_, RcadApp>, p: [f64; 2]) {
    sk_click_mod(h, p, Modifiers::NONE);
}

fn sketch(h: &Harness<'_, RcadApp>) -> rcad_sketch::Sketch {
    h.state().sketch_session().unwrap().editor.sketch.clone()
}

fn dof(h: &Harness<'_, RcadApp>) -> usize {
    h.state().sketch_session().unwrap().editor.report.dof
}

fn centres(h: &Harness<'_, RcadApp>) -> Vec<[f64; 2]> {
    let s = sketch(h);
    s.entities
        .iter()
        .filter_map(|e| match e {
            Entity::Circle { center, .. } => s.point_xy(*center),
            _ => None,
        })
        .collect()
}

#[test]
fn concentric_fix_collinear_and_symmetric_from_the_inspector() {
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Create Sketch");
    // Two circles, made concentric.
    click(&mut h, "Circle");
    sk_click(&mut h, [20.0, 20.0]);
    sk_click(&mut h, [30.0, 20.0]);
    sk_click(&mut h, [23.0, 21.0]);
    sk_click(&mut h, [27.0, 21.0]);
    click(&mut h, "Select");
    sk_click(&mut h, [30.0, 20.0]);
    sk_click_mod(&mut h, [27.0, 21.0], Modifiers::SHIFT);
    let before = dof(&h);
    click(&mut h, "Concentric");
    let c = centres(&h);
    assert!(
        (c[0][0] - c[1][0]).abs() < 1e-9 && (c[0][1] - c[1][1]).abs() < 1e-9,
        "{c:?}"
    );
    assert_eq!(dof(&h), before - 2);
    // Fix the outer circle (its centre point): 2 DOF less; again: unfixed.
    sk_click(&mut h, [c[0][0] + 10.0, c[0][1]]);
    let before = dof(&h);
    click(&mut h, "Fix / Unfix");
    assert_eq!(dof(&h), before - 2);
    sk_click(&mut h, [c[0][0] + 10.0, c[0][1]]);
    click(&mut h, "Fix / Unfix");
    assert_eq!(dof(&h), before);
    // Two horizontal-ish lines made collinear.
    click(&mut h, "Line");
    sk_click(&mut h, [-40.0, -20.0]);
    sk_click(&mut h, [-20.0, -21.0]);
    h.key_press(egui::Key::Escape);
    settle(&mut h);
    sk_click(&mut h, [-10.0, -24.0]);
    sk_click(&mut h, [10.0, -26.0]);
    h.key_press(egui::Key::Escape);
    settle(&mut h);
    click(&mut h, "Select");
    sk_click(&mut h, [-30.0, -20.5]);
    sk_click_mod(&mut h, [0.0, -25.0], Modifiers::SHIFT);
    click(&mut h, "Collinear");
    let s = sketch(&h);
    assert!(
        s.constraints
            .iter()
            .any(|c| matches!(c, Constraint::Collinear { .. })),
        "{:?}",
        h.state().sketch_session().unwrap().editor.feedback
    );
    // Symmetric: the ends of a line across a vertical mirror line.
    click(&mut h, "Line");
    sk_click(&mut h, [50.0, -40.0]);
    sk_click(&mut h, [50.0, 40.0]);
    h.key_press(egui::Key::Escape);
    settle(&mut h);
    sk_click(&mut h, [40.0, 10.0]);
    sk_click(&mut h, [62.0, 12.0]);
    h.key_press(egui::Key::Escape);
    settle(&mut h);
    let s = sketch(&h);
    let pid = |x: f64, y: f64| {
        s.points
            .iter()
            .find(|p| (p.x - x).abs() < 0.6 && (p.y - y).abs() < 0.6)
            .map(|p| p.id)
            .unwrap()
    };
    let (p1, p2) = (pid(40.0, 10.0), pid(62.0, 12.0));
    click(&mut h, "Select");
    sk_click(&mut h, [40.0, 10.0]);
    sk_click_mod(&mut h, [62.0, 12.0], Modifiers::SHIFT);
    sk_click_mod(&mut h, [50.0, 0.0], Modifiers::SHIFT);
    click(&mut h, "Symmetric");
    let s = sketch(&h);
    let (a, b) = (s.point_xy(p1).unwrap(), s.point_xy(p2).unwrap());
    let x = s
        .entities
        .iter()
        .find_map(|e| match e {
            Entity::Line { a, b, .. } => {
                let (u, v) = (s.point_xy(*a)?, s.point_xy(*b)?);
                ((u[0] - v[0]).abs() < 1e-6 && (u[1] - v[1]).abs() > 50.0).then_some(u[0])
            }
            _ => None,
        })
        .expect("mirror line");
    assert!(
        ((a[0] + b[0]) / 2.0 - x).abs() < 1e-8 && (a[1] - b[1]).abs() < 1e-8,
        "{a:?} {b:?} about x = {x}: {:?}",
        h.state().sketch_session().unwrap().editor.feedback
    );
}

/// Two circles selected, then the mirror line: Symmetric mirrors the
/// centres and makes the radii equal.
#[test]
fn symmetric_circles_about_a_line_from_the_inspector() {
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Create Sketch");
    click(&mut h, "Line");
    sk_click(&mut h, [20.0, -30.0]);
    sk_click(&mut h, [20.0, 30.0]);
    h.key_press(egui::Key::Escape);
    settle(&mut h);
    click(&mut h, "Circle");
    sk_click(&mut h, [5.0, 10.0]);
    sk_click(&mut h, [8.0, 10.0]);
    sk_click(&mut h, [33.0, 12.0]);
    sk_click(&mut h, [37.0, 12.0]);
    click(&mut h, "Select");
    sk_click(&mut h, [8.0, 10.0]);
    sk_click_mod(&mut h, [37.0, 12.0], Modifiers::SHIFT);
    sk_click_mod(&mut h, [20.0, -20.0], Modifiers::SHIFT);
    click(&mut h, "Symmetric");
    let s = sketch(&h);
    let circles: Vec<([f64; 2], f64)> = s
        .entities
        .iter()
        .filter_map(|e| match e {
            Entity::Circle { center, radius, .. } => Some((s.point_xy(*center)?, *radius)),
            _ => None,
        })
        .collect();
    let [(c1, r1), (c2, r2)] = circles.as_slice() else {
        panic!("{circles:?}")
    };
    // The mirror line is free too: reflect c1 about where it ended up.
    let (p, q) = s
        .entities
        .iter()
        .find_map(|e| match e {
            Entity::Line { a, b, .. } => Some((s.point_xy(*a)?, s.point_xy(*b)?)),
            _ => None,
        })
        .unwrap();
    let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
    let t = ((c1[0] - p[0]) * dx + (c1[1] - p[1]) * dy) / (dx * dx + dy * dy);
    let foot = [p[0] + t * dx, p[1] + t * dy];
    let image = [2.0 * foot[0] - c1[0], 2.0 * foot[1] - c1[1]];
    assert!(
        (image[0] - c2[0]).abs() < 1e-7
            && (image[1] - c2[1]).abs() < 1e-7
            && (r1 - r2).abs() < 1e-8,
        "{circles:?}: {:?}",
        h.state().sketch_session().unwrap().editor.feedback
    );
}
