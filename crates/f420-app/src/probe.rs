//! Opt-in widget registry for driving the real window from outside
//! (`F420_PROBE`, see `tools/live_input.py`). Widgets that scripts need to
//! target note their screen rectangle under a stable key each frame; the shell
//! writes them to the probe file together with the camera and document state.
//! Disabled (zero cost beyond a thread-local check) unless enabled.

use std::cell::RefCell;

use eframe::egui;

thread_local! {
    static RECTS: RefCell<Option<Vec<(String, egui::Rect)>>> = const { RefCell::new(None) };
}

/// Turn the registry on (once, by the shell when `F420_PROBE` is set).
pub fn enable() {
    RECTS.with(|r| *r.borrow_mut() = Some(Vec::new()));
}

/// Clear last frame's rectangles (start of each frame).
pub fn begin_frame() {
    RECTS.with(|r| {
        if let Some(v) = r.borrow_mut().as_mut() {
            v.clear();
        }
    });
}

/// Record `rect` under `key` if the registry is enabled.
pub fn note(key: impl Into<String>, rect: egui::Rect) {
    RECTS.with(|r| {
        if let Some(v) = r.borrow_mut().as_mut() {
            v.push((key.into(), rect));
        }
    });
}

/// This frame's rectangles (empty when disabled).
pub fn rects() -> Vec<(String, egui::Rect)> {
    RECTS.with(|r| r.borrow().clone().unwrap_or_default())
}

/// `response.probe("Finish Sketch")` notes the widget's rectangle.
pub trait Probe {
    fn probe(self, key: &str) -> Self;
}

impl Probe for egui::Response {
    fn probe(self, key: &str) -> Self {
        note(key, self.rect);
        self
    }
}
