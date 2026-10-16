//! Two-column command-dialog forms: every row's label sits in one
//! left-hand column and its widget starts at the same x on the right.
//!
//! [`form`] wraps a dialog body. Inside it, [`label`] (used by the shared
//! field helpers in [`crate::panels`]) draws into the label column, whose
//! width is the widest label of the previous frame (capped at
//! [`LABEL_MAX_W`]; a longer label pushes only its own row). Outside a form
//! (the Browser and Inspector columns) labels stay inline, so narrow
//! columns are not widened.

use std::cell::Cell;

use eframe::egui::{self, Response, Ui};

/// Widest label column (points).
pub const LABEL_MAX_W: f32 = 180.0;

#[derive(Clone, Copy)]
struct Columns {
    /// Label column width this frame (0 until measured).
    width: f32,
    /// Widest label drawn this frame.
    seen: f32,
}

thread_local! {
    static COLUMNS: Cell<Option<Columns>> = const { Cell::new(None) };
}

/// Lay out `add` as a two-column form. `id_salt` names the form (each dialog
/// keeps its own column width).
pub fn form<R>(ui: &mut Ui, id_salt: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let key = egui::Id::new(("form_label_w", id_salt));
    let width = ui.data(|d| d.get_temp::<f32>(key)).unwrap_or(0.0);
    let outer = COLUMNS.with(|c| c.replace(Some(Columns { width, seen: 0.0 })));
    let r = add(ui);
    let cols = COLUMNS.with(|c| c.replace(outer));
    if let Some(cols) = cols
        && (cols.seen - width).abs() > 0.5
    {
        ui.data_mut(|d| d.insert_temp(key, cols.seen));
        ui.ctx().request_repaint();
    }
    r
}

/// Whether a form is being laid out.
pub fn active() -> bool {
    COLUMNS.with(|c| c.get().is_some())
}

/// A row label: in a form, drawn in the label column; otherwise inline.
pub fn label(ui: &mut Ui, text: impl Into<egui::WidgetText>) -> Response {
    let Some(cols) = COLUMNS.with(|c| c.get()) else {
        return ui.label(text);
    };
    let h = ui.spacing().interact_size.y;
    let r = ui
        .allocate_ui_with_layout(
            egui::vec2(cols.width, h),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(cols.width);
                ui.label(text)
            },
        )
        .inner;
    let w = r.rect.width().min(LABEL_MAX_W);
    COLUMNS.with(|c| {
        c.set(Some(Columns {
            seen: cols.seen.max(w),
            ..cols
        }))
    });
    r
}

/// An empty label cell, so a checkbox or a button row lines up with the
/// widgets of labelled rows.
pub fn skip(ui: &mut Ui) {
    if let Some(cols) = COLUMNS.with(|c| c.get())
        && cols.width > 0.0
    {
        ui.add_space(cols.width + ui.spacing().item_spacing.x);
    }
}

/// A labelled row: the label, then `add` in the widget column (wrapping
/// within that column, e.g. a long radio group).
pub fn row<R>(ui: &mut Ui, text: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        label(ui, text);
        ui.horizontal_wrapped(add).inner
    })
    .inner
}

/// A row with no label (checkbox, toggle): its widget starts in the widget
/// column.
pub fn unlabelled<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        skip(ui);
        add(ui)
    })
    .inner
}

/// A combo box with its label in the label column (accessible name: the
/// label). `.response` is the combo button.
pub fn combo<R>(
    ui: &mut Ui,
    text: &str,
    selected: impl Into<egui::WidgetText>,
    items: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<()> {
    let response = ui
        .horizontal(|ui| {
            let l = label(ui, text);
            egui::ComboBox::from_id_salt(text)
                .selected_text(selected)
                .show_ui(ui, items)
                .response
                .labelled_by(l.id)
        })
        .inner;
    egui::InnerResponse::new((), response)
}

/// A checkbox in the widget column.
pub fn check(ui: &mut Ui, value: &mut bool, text: impl Into<egui::WidgetText>) -> Response {
    unlabelled(ui, |ui| ui.checkbox(value, text))
}
