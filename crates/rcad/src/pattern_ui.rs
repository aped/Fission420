//! Sketch pattern dialog (docs/behavior/sketching.md "Sketch patterns"):
//! rectangular (one or two directions) or circular constrained copies of the
//! selected curves. Apply goes through [`SketchEditor::apply_pattern`] (one
//! local undo step); the document changes at Finish Sketch.

use crate::sketch_edit::SketchEditor;
use eframe::egui;

fn field(ui: &mut egui::Ui, label: &str, buf: &mut String) {
    let l = ui.label(label);
    ui.add(egui::TextEdit::singleline(buf).desired_width(70.0))
        .labelled_by(l.id);
    ui.end_row();
}

/// Show the dialog while a pattern is being set up.
pub fn pattern_window(ctx: &egui::Context, ed: &mut SketchEditor) {
    let Some(mut p) = ed.pattern_edit.clone() else {
        return;
    };
    let (mut apply, mut cancel) = (false, false);
    egui::Window::new("Sketch pattern")
        .id(egui::Id::new("sketch_pattern_dialog"))
        .collapsible(false)
        .resizable(false)
        .default_pos(egui::pos2(260.0, 120.0))
        .show(ctx, |ui| {
            ui.label(format!("{} curve(s) selected", p.entities.len()));
            ui.horizontal(|ui| {
                ui.selectable_value(&mut p.circular, false, "Rectangular");
                ui.selectable_value(&mut p.circular, true, "Circular");
            });
            egui::Grid::new("sketch_pattern_grid")
                .num_columns(2)
                .show(ui, |ui| {
                    if p.circular {
                        field(ui, "Instances", &mut p.ccount);
                        field(ui, "Angle between (°)", &mut p.angle);
                        field(ui, "Centre X (mm)", &mut p.cx);
                        field(ui, "Centre Y (mm)", &mut p.cy);
                    } else {
                        field(ui, "Instances", &mut p.count);
                        field(ui, "Step x (mm)", &mut p.dx);
                        field(ui, "Step y (mm)", &mut p.dy);
                        ui.label("");
                        ui.checkbox(&mut p.two_directions, "Second direction");
                        ui.end_row();
                        if p.two_directions {
                            field(ui, "Instances 2", &mut p.count2);
                            field(ui, "Step 2 x (mm)", &mut p.dx2);
                            field(ui, "Step 2 y (mm)", &mut p.dy2);
                        }
                    }
                });
            ui.small(
                "Copies are constrained to their sources; the spacing / angle become \
                 editable dimensions.",
            );
            if let Some(fb) = ed.feedback.as_ref().filter(|f| f.error) {
                ui.colored_label(egui::Color32::from_rgb(210, 60, 60), &fb.text);
            }
            ui.horizontal(|ui| {
                apply = ui.button("Apply pattern").clicked();
                cancel = ui.button("Cancel pattern").clicked();
            });
        });
    if cancel {
        ed.pattern_edit = None;
        return;
    }
    ed.pattern_edit = Some(p);
    if apply {
        ed.apply_pattern();
    }
}
