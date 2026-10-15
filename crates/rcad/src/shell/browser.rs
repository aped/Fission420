//! Browser tree nodes in Fusion's order: Document Settings (units), Named
//! Views, Origin (the world planes), then the document's sections, Bodies
//! (with Components), Sketches and Construction. The first three are node
//! menus on the Browser heading's row (shown as Settings / Views / Origin) so
//! the column stays short in a 800 px window; Sketches and Construction are
//! collapsible nodes, drawn after the History list so its rows keep their
//! place. Accessible names: "Document Settings",
//! "Named Views", "Origin", "Sketches (N)", "Construction (N)", and
//! "Browser <sketch>" for sketch rows (History keeps the bare names).

use eframe::egui::{self, RichText};
use rcad_document::SketchPlane;

use crate::app::{RcadApp, SketchTarget};
use crate::probe::Probe as _;

impl RcadApp {
    /// Document Settings ▸ Units, Named Views, Origin.
    pub(crate) fn browser_top_nodes(&mut self, ui: &mut egui::Ui) {
        {
            ui.menu_button("⚙ Settings", |ui| {
                ui.label(RichText::new("Units").strong());
                ui.label("Length: millimetre (mm)").on_hover_text(
                    "Model units are millimetres and degrees; parameters may use mm, cm, m or in",
                );
                ui.label("Angle: degree (°)");
            })
            .response
            .named("Document Settings");
            ui.menu_button("Views", |ui| {
                use crate::shell::{CmdId as C, registry};
                for id in [
                    C::ViewTop,
                    C::ViewFront,
                    C::ViewRight,
                    C::ViewLeft,
                    C::ViewBack,
                    C::ViewIso,
                ] {
                    let d = registry::def(id);
                    if ui
                        .button(d.label)
                        .named(&format!("Named view {}", d.label))
                        .clicked()
                    {
                        (d.run)(self);
                        ui.close();
                    }
                }
            })
            .response
            .named("Named Views");
            let open = self.summary.open;
            ui.menu_button("Origin", |ui| {
                for (label, plane) in [
                    ("XY Plane", SketchPlane::WorldXY),
                    ("XZ Plane", SketchPlane::WorldXZ),
                    ("YZ Plane", SketchPlane::WorldYZ),
                ] {
                    if ui
                        .add_enabled(open, egui::Button::new(format!("Sketch on {label}")))
                        .named(&format!("Origin {label}"))
                        .clicked()
                    {
                        self.begin_sketch(SketchTarget::New { plane });
                        ui.close();
                    }
                }
                ui.weak("Origin point and X / Y / Z axes are the world frame.");
            })
            .response
            .named("Origin");
        }
    }

    /// Sketches and Construction nodes (after Bodies; drawn below History).
    pub(crate) fn browser_sketch_nodes(&mut self, ui: &mut egui::Ui) {
        let sketches: Vec<_> = self
            .summary
            .features
            .iter()
            .filter(|f| f.kind == "sketch")
            .map(|f| (f.id, f.name.clone(), f.plane))
            .collect();
        let mut edit = None;
        let mut select = None;
        let header = egui::CollapsingHeader::new(format!("Sketches ({})", sketches.len()))
            .id_salt("browser_sketches")
            .show(ui, |ui| {
                for (id, name, _) in &sketches {
                    let r = ui
                        .selectable_label(self.selected == Some(*id), name.as_str())
                        .named(&format!("Browser {name}"));
                    if r.double_clicked() {
                        edit = Some(*id);
                    } else if r.clicked() {
                        select = Some(*id);
                    }
                }
                if sketches.is_empty() {
                    ui.weak("No sketches");
                }
            });
        header
            .header_response
            .named(&format!("Sketches ({})", sketches.len()));
        let construction: Vec<_> = sketches
            .iter()
            .filter(|(_, _, p)| matches!(p, Some(SketchPlane::Offset { .. })))
            .collect();
        let header = egui::CollapsingHeader::new(format!("Construction ({})", construction.len()))
            .id_salt("browser_construction")
            .show(ui, |ui| {
                for (_, name, p) in &construction {
                    let what = match p {
                        Some(SketchPlane::Offset {
                            distance, angle, ..
                        }) => format!(
                            "Offset plane {} mm{} (sketch {name})",
                            crate::params_ui::fmt_value(*distance),
                            if *angle != 0.0 {
                                format!(", {}°", crate::params_ui::fmt_value(angle.to_degrees()))
                            } else {
                                String::new()
                            }
                        ),
                        _ => name.clone(),
                    };
                    ui.label(what);
                }
                if construction.is_empty() {
                    ui.weak("No construction planes (CONSTRUCT ▸ Offset Plane)");
                }
            });
        header
            .header_response
            .named(&format!("Construction ({})", construction.len()));
        if self.sketch.is_none() {
            if let Some(id) = select {
                self.select(Some(id));
            }
            if let Some(id) = edit {
                self.select(Some(id));
                self.begin_sketch(SketchTarget::Existing(id));
            }
        }
    }
}
