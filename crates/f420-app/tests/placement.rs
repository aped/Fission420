//! Interactive placement: dialog centre fields, real pointer drags injected
//! over the (GPU-less) view rect, Esc, and the scripted `--demo-place` path.
//! Without wgpu the app uses `fallback_camera` with the real view rect, so
//! pointer → ray → plane runs through exactly the same code as the GUI.

use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers, PointerButton, accesskit::Role};
use egui_kittest::{Harness, kittest::Queryable as _};
use f420_app::{ExecMode, F420App, place::Tool};

fn harness() -> Harness<'static, F420App> {
    Harness::builder()
        .with_size([1400.0, 900.0])
        .with_max_steps(64)
        .build_ui_state(
            |ui, app: &mut F420App| app.ui(ui),
            F420App::new(ExecMode::Sync, None),
        )
}

fn settle(h: &mut Harness<'_, F420App>) {
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

fn click(h: &mut Harness<'_, F420App>, label: &str) {
    h.get_by_label(label).click();
    settle(h);
}

fn type_into(h: &mut Harness<'_, F420App>, label: &str, text: &str) {
    h.get_by_label(label).focus();
    let _ = h.try_run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    let _ = h.try_run();
    h.get_by_label(label).type_text(text);
    let _ = h.try_run();
}

fn value(h: &Harness<'_, F420App>, label: &str) -> Option<String> {
    h.get_by_label(label).value()
}

fn params(h: &Harness<'_, F420App>, idx: usize) -> Vec<(String, f64)> {
    h.state().summary().features[idx].parameters.clone()
}

fn p(n: &str, v: f64) -> (String, f64) {
    (n.to_string(), v)
}

/// New + 80x50 rectangle + extrude 10, via dialogs.
fn plate(h: &mut Harness<'_, F420App>) {
    click(h, "New");
    click(h, "Rectangle on XY…");
    click(h, "OK");
    click(h, "Extrude…");
    click(h, "OK");
    assert_eq!(h.state().summary().features.len(), 2);
}

/// Real pointer drag between the screen projections of two world points.
fn drag(h: &mut Harness<'_, F420App>, from: [f64; 3], to: [f64; 3]) {
    let a = h.state().world_to_screen(from).expect("from visible");
    let b = h.state().world_to_screen(to).expect("to visible");
    let rect = h.state().view_rect().expect("view laid out");
    assert!(rect.contains(a) && rect.contains(b));
    h.event(egui::Event::PointerMoved(a));
    let _ = h.try_run();
    h.event(egui::Event::PointerButton {
        pos: a,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    let _ = h.try_run();
    for k in 1..=6 {
        let t = k as f32 / 6.0;
        h.event(egui::Event::PointerMoved(a + (b - a) * t));
        let _ = h.try_run();
    }
    h.event(egui::Event::PointerButton {
        pos: b,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    let _ = h.try_run();
    let _ = h.try_run();
}

#[test]
fn dialogs_accept_centre_fields() {
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Rectangle on XY…");
    type_into(&mut h, "Rectangle width (mm)", "60");
    type_into(&mut h, "Rectangle height (mm)", "30");
    type_into(&mut h, "Rectangle center X (mm)", "10");
    type_into(&mut h, "Rectangle center Y (mm)", "-5");
    click(&mut h, "OK");
    assert_eq!(
        params(&h, 0),
        vec![
            p("width", 60.0),
            p("height", 30.0),
            p("center_x", 10.0),
            p("center_y", -5.0)
        ]
    );
    click(&mut h, "Extrude…");
    click(&mut h, "OK");
    click(&mut h, "Circle on face…");
    type_into(&mut h, "Circle diameter (mm)", "8");
    type_into(&mut h, "Circle center X (mm)", "12");
    type_into(&mut h, "Circle center Y (mm)", "-3");
    click(&mut h, "OK");
    assert_eq!(
        params(&h, 2),
        vec![p("diameter", 8.0), p("center_x", 12.0), p("center_y", -3.0)]
    );
    // Off-centre hole outside the base is refused with ProductLimit and the
    // document is unchanged.
    click(&mut h, "Circle on face…");
    type_into(&mut h, "Circle diameter (mm)", "8");
    type_into(&mut h, "Circle center X (mm)", "38");
    click(&mut h, "OK");
    click(&mut h, "Cut through all…");
    h.get_by_role_and_label(Role::ComboBox, "Cut profile sketch")
        .click();
    let _ = h.try_run();
    let bad = h.state().summary().features[3].clone();
    h.get_by_label(&format!("{}: {}", bad.id, bad.name)).click();
    let _ = h.try_run();
    let before = h.state().summary().revision;
    click(&mut h, "OK");
    assert_eq!(h.state().summary().revision, before);
    assert!(
        h.state()
            .diagnostics()
            .iter()
            .any(|d| d.error && d.kind == "ProductLimit"),
        "{:?}",
        h.state().diagnostics()
    );
}

#[test]
fn pointer_drag_places_rectangle_and_esc_cancels() {
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Place rectangle");
    assert_eq!(h.state().placement().map(|p| p.tool), Some(Tool::Rectangle));
    drag(&mut h, [-20.2, -10.1, 0.0], [30.3, 14.8, 0.0]);
    // Released: tool done, pre-filled dialog open, nothing created yet.
    assert!(h.state().placement().is_none());
    assert!(h.state().summary().features.is_empty());
    assert_eq!(value(&h, "Rectangle width (mm)").as_deref(), Some("50"));
    assert_eq!(value(&h, "Rectangle height (mm)").as_deref(), Some("25"));
    assert_eq!(value(&h, "Rectangle center X (mm)").as_deref(), Some("5"));
    assert_eq!(value(&h, "Rectangle center Y (mm)").as_deref(), Some("2.5"));
    click(&mut h, "OK");
    assert_eq!(
        params(&h, 0),
        vec![
            p("width", 50.0),
            p("height", 25.0),
            p("center_x", 5.0),
            p("center_y", 2.5)
        ]
    );
    // Esc cancels an active tool; a cancelled dialog creates nothing.
    click(&mut h, "Place rectangle");
    assert!(h.state().placement().is_some());
    h.key_press(Key::Escape);
    let _ = h.try_run();
    assert!(h.state().placement().is_none());
    click(&mut h, "Place rectangle");
    drag(&mut h, [0.0, 0.0, 0.0], [10.0, 10.0, 0.0]);
    click(&mut h, "Cancel");
    assert_eq!(h.state().summary().features.len(), 1);
}

#[test]
fn pointer_drag_places_off_centre_hole_on_end_cap() {
    let mut h = harness();
    plate(&mut h);
    click(&mut h, "Place hole");
    let ext = h.state().summary().features[1].id;
    assert_eq!(
        h.state().placement().map(|p| p.tool),
        Some(Tool::Hole { extrude: ext })
    );
    // Press at the centre on the end cap (z = 10), drag to the rim.
    drag(&mut h, [-15.2, 9.8, 10.0], [-11.4, 9.9, 10.0]);
    assert_eq!(value(&h, "Circle diameter (mm)").as_deref(), Some("7"));
    assert_eq!(value(&h, "Circle center X (mm)").as_deref(), Some("-15"));
    assert_eq!(value(&h, "Circle center Y (mm)").as_deref(), Some("10"));
    click(&mut h, "OK");
    let s = h.state().summary().features[2].clone();
    assert_eq!(s.plane, Some(f420_commands::end_cap_of(ext)));
    assert_eq!(
        s.parameters,
        vec![
            p("diameter", 7.0),
            p("center_x", -15.0),
            p("center_y", 10.0)
        ]
    );
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "boolean is slow in debug; run with --release"
)]
fn demo_place_script_cuts_off_centre_hole() {
    let mut h = harness();
    h.state_mut().start_demo_place(false);
    settle(&mut h);
    let fs = &h.state().summary().features;
    assert_eq!(fs.len(), 4, "{:?}", h.state().diagnostics());
    assert_eq!(
        fs[2].parameters,
        vec![p("diameter", 12.0), p("center_x", 20.0), p("center_y", 8.0)]
    );
    let v = h.state().volume().unwrap();
    let expected = 40000.0 - std::f64::consts::PI * 36.0 * 10.0;
    assert!((v - expected).abs() < 0.005, "{v} vs {expected}");
}
