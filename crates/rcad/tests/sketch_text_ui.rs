//! Text tool through the real sketch UI (run 6, SK04): draw a 60 x 30
//! rectangle, Text tool, click inside, type "B8" at 10 mm, Apply: the glyphs
//! become regions inside the plate (2 glyphs with 2 counters each); a refused
//! font keeps the dialog open and the sketch unchanged; Cancel adds nothing;
//! clicking the text reopens it for editing (height 8); Finish Sketch stores
//! the text in the document.
//!
//! Needs the installed font "Liberation Sans"; `RCAD_SKIP_FONT_TESTS=1` skips.

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

#[test]
fn text_tool_places_edits_and_finishes_text() {
    if let Err(e) = rcad_sketch::font::find_font(&rcad_sketch::FontSpec::family("Liberation Sans"))
    {
        if std::env::var("RCAD_SKIP_FONT_TESTS").as_deref() == Ok("1") {
            eprintln!("SKIPPED (RCAD_SKIP_FONT_TESTS=1): {e}");
            return;
        }
        panic!("{e}; install Liberation Sans or set RCAD_SKIP_FONT_TESTS=1");
    }
    let mut h = harness();
    click(&mut h, "New");
    click(&mut h, "Create Sketch");
    click(&mut h, "2-Point Rect");
    sk_click(&mut h, [0.0, 0.0]);
    sk_click(&mut h, [60.0, 30.0]);
    assert_eq!(sketch(&h).entities.len(), 4);

    // Cancel adds nothing.
    click(&mut h, "Text");
    sk_click(&mut h, [10.0, 10.0]);
    assert!(h.query_by_label("Text string").is_some(), "dialog opened");
    click(&mut h, "Cancel text");
    assert!(h.query_by_label("Text string").is_none());
    assert!(sketch(&h).texts.is_empty());

    // A font that is not installed: refused, dialog stays, sketch unchanged.
    sk_click(&mut h, [10.0, 10.0]);
    type_into(&mut h, "Text string", "B8");
    type_into(&mut h, "Height (mm, cap height)", "10");
    type_into(&mut h, "Font family", "No Such Font Family");
    click(&mut h, "Apply text");
    assert!(sketch(&h).texts.is_empty());
    assert!(feedback(&h).contains("font not found"), "{}", feedback(&h));
    assert!(
        h.query_by_label("Text string").is_some(),
        "dialog kept open"
    );

    // The installed font: glyph regions with counters inside the plate.
    type_into(&mut h, "Font family", "Liberation Sans");
    click(&mut h, "Apply text");
    let s = sketch(&h);
    assert_eq!(s.texts.len(), 1, "{}", feedback(&h));
    assert!(h.query_by_label("Text string").is_none(), "dialog closed");
    let rs = s.regions().unwrap();
    let depth = |d| rs.iter().filter(|r| r.depth == d).count();
    assert_eq!((depth(0), depth(1), depth(2)), (1, 2, 4));
    assert!(
        rs.iter()
            .filter(|r| r.depth == 1)
            .all(|r| r.region.holes.len() == 2)
    );

    // Click the text with the Text tool: edit it (height 8).
    sk_click(&mut h, [15.0, 13.0]);
    assert!(
        h.query_by_label("Delete text").is_some(),
        "editing existing text"
    );
    type_into(&mut h, "Height (mm, cap height)", "8");
    click(&mut h, "Apply text");
    assert_eq!(sketch(&h).texts[0].spec.height, 8.0, "{}", feedback(&h));

    click(&mut h, "Finish Sketch");
    let f = h.state().summary().features[0].clone();
    let stored = f.sketch.expect("sketch");
    assert_eq!(stored.texts.len(), 1);
    assert_eq!(stored.texts[0].spec.text, "B8");
    assert_eq!(stored.regions().unwrap().len(), 7);
}
