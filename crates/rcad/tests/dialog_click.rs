//! Regression probe: the first pointer click after dismissing the unsaved-changes
//! dialog must act (observed once in a live session during run 2).

use std::time::{Duration, Instant};

use eframe::egui::{self, Modifiers, PointerButton, Pos2};
use egui_kittest::{Harness, kittest::Queryable as _};
use rcad::{ExecMode, RcadApp};

fn harness() -> Harness<'static, RcadApp> {
    Harness::builder()
        .with_size([1280.0, 768.0])
        .with_max_steps(64)
        .build_ui_state(
            |ui, app: &mut RcadApp| app.ui(ui),
            RcadApp::new(ExecMode::Threaded, None),
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
        assert!(start.elapsed() < Duration::from_secs(120));
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn pointer_click(h: &mut Harness<'_, RcadApp>, p: Pos2) {
    h.event(egui::Event::PointerMoved(p));
    let _ = h.try_run();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    let _ = h.try_run();
    let _ = h.try_run();
}

fn click_label(h: &mut Harness<'_, RcadApp>, label: &str) {
    let c = h.get_by_label(label).rect().center();
    pointer_click(h, c);
    settle(h);
}

#[test]
fn first_click_after_discard_dialog_acts() {
    let mut h = harness();
    click_label(&mut h, "New");
    h.get_by_label("Templates").click();
    let _ = h.try_run();
    click_label(&mut h, "Rectangle on XY…");
    click_label(&mut h, "OK");
    assert!(h.state().summary().dirty);
    click_label(&mut h, "New");
    assert!(h.state().pending_confirmation().is_some());
    click_label(&mut h, "Discard changes");
    assert!(h.state().pending_confirmation().is_none());
    assert!(h.state().summary().features.is_empty(), "new document");
    click_label(&mut h, "Create Sketch");
    assert!(
        h.state().sketch_session().is_some(),
        "the first click after the dialog started a sketch"
    );
}
