//! Sketch text dialog of the Text tool (run 6, docs/behavior/sketching.md
//! "Sketch text"). Edits [`SketchEditor::text_edit`]; Apply adds or changes the
//! text through [`SketchEditor::apply_text`] (one local undo step), so the
//! document changes only at Finish Sketch like every other editor tool.

use crate::sketch_edit::SketchEditor;
use eframe::egui;
use rcad_sketch::{HAlign, VAlign};

/// Installed font families (sorted, unique), for the family picker.
fn families() -> &'static [String] {
    static F: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    F.get_or_init(|| {
        let mut v: Vec<String> = rcad_sketch::font::installed_fonts()
            .iter()
            .map(|f| f.family.clone())
            .collect();
        v.sort_by_key(|s| s.to_lowercase());
        v.dedup();
        v
    })
}

fn field(ui: &mut egui::Ui, label: &str, buf: &mut String, width: f32) {
    let l = ui.label(label);
    ui.add(egui::TextEdit::singleline(buf).desired_width(width))
        .labelled_by(l.id);
    ui.end_row();
}

/// Show the dialog while a text is being placed or edited.
pub fn text_window(ctx: &egui::Context, ed: &mut SketchEditor) {
    let Some(mut t) = ed.text_edit.clone() else {
        return;
    };
    let (mut apply, mut cancel, mut delete) = (false, false, false);
    let existing = t.id.and_then(|id| ed.sketch.text_item(id)).cloned();
    egui::Window::new(if t.id.is_some() {
        "Edit sketch text"
    } else {
        "New sketch text"
    })
    .id(egui::Id::new("sketch_text_dialog"))
    .collapsible(false)
    .resizable(false)
    .default_pos(egui::pos2(260.0, 120.0))
    .show(ctx, |ui| {
        egui::Grid::new("sketch_text_grid")
            .num_columns(2)
            .show(ui, |ui| {
                field(ui, "Text string", &mut t.text, 220.0);
                let l = ui.label("Font family");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut t.family).desired_width(150.0))
                        .labelled_by(l.id);
                    egui::ComboBox::from_id_salt("sketch_text_family")
                        .selected_text("Installed…")
                        .height(300.0)
                        .show_ui(ui, |ui| {
                            for f in families() {
                                ui.selectable_value(&mut t.family, f.clone(), f);
                            }
                        });
                });
                ui.end_row();
                ui.label("Style");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut t.bold, "Bold");
                    ui.checkbox(&mut t.italic, "Italic");
                });
                ui.end_row();
                field(ui, "Height (mm, cap height)", &mut t.height, 80.0);
                field(ui, "Rotation (°)", &mut t.rotation, 80.0);
                ui.label("Horizontal");
                ui.horizontal(|ui| {
                    for (v, n) in [
                        (HAlign::Left, "Left"),
                        (HAlign::Center, "Center"),
                        (HAlign::Right, "Right"),
                    ] {
                        ui.selectable_value(&mut t.halign, v, n);
                    }
                });
                ui.end_row();
                ui.label("Vertical");
                ui.horizontal(|ui| {
                    for (v, n) in [
                        (VAlign::Baseline, "Baseline"),
                        (VAlign::Middle, "Middle"),
                        (VAlign::Top, "Top"),
                    ] {
                        ui.selectable_value(&mut t.valign, v, n);
                    }
                });
                ui.end_row();
            });
        if t.anchor.is_some() {
            ui.label("Anchored to a sketch point (the text follows it).");
        }
        if let Some(x) = &existing {
            let f = &x.resolved.font;
            ui.small(format!(
                "Outlines from {} {} ({}, {}).",
                f.family, f.style, f.file, f.hash
            ));
            if rcad_sketch::font::find_font(&x.spec.font).is_err() {
                ui.colored_label(
                    egui::Color32::from_rgb(200, 120, 0),
                    "This font is not installed here: placement edits work, changing the \
                     string or font is refused.",
                );
            }
        }
        if let Some(fb) = ed.feedback.as_ref().filter(|f| f.error) {
            ui.colored_label(egui::Color32::from_rgb(210, 60, 60), &fb.text);
        }
        ui.horizontal(|ui| {
            apply = ui.button("Apply text").clicked();
            cancel = ui.button("Cancel text").clicked();
            if t.id.is_some() {
                delete = ui.button("Delete text").clicked();
            }
        });
    });
    if cancel {
        ed.text_edit = None;
        return;
    }
    ed.text_edit = Some(t);
    if delete {
        ed.delete_text();
    } else if apply {
        ed.apply_text();
    }
}
