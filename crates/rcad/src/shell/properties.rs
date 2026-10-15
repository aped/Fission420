//! Read-only Properties panel (the right column's Properties tab): what is
//! selected and its measured values, without any action buttons. Commands
//! live in the ribbon, the dialogs and the context menus; the Inspector tab
//! keeps the older per-selection forms.

use eframe::egui::{self, RichText};
use rcad_kernel::VolumeResult;

use crate::app::RcadApp;
use crate::probe::Probe as _;

fn row(ui: &mut egui::Ui, k: &str, v: impl Into<String>) {
    ui.label(RichText::new(k).weak());
    ui.label(v.into());
    ui.end_row();
}

fn mm(v: f64) -> String {
    crate::params_ui::fmt_value(v)
}

impl RcadApp {
    /// The tab row at the top of the right column.
    pub(crate) fn right_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (props, l) in [(true, "Properties"), (false, "Inspector")] {
                if ui
                    .selectable_label(self.shell.properties == props, RichText::new(l).heading())
                    .named(&format!("{l} tab"))
                    .clicked()
                {
                    self.shell.properties = props;
                }
            }
        });
    }

    pub(crate) fn properties_ui(&mut self, ui: &mut egui::Ui) {
        if !self.summary.open {
            ui.weak("No document open.");
            return;
        }
        egui::Grid::new("properties_grid")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                if let Some(face) = self.selected_face().cloned() {
                    row(ui, "Face", face.description.clone());
                    if let Some(p) = face.plane {
                        row(
                            ui,
                            "Plane normal",
                            format!(
                                "({}, {}, {})",
                                mm(p.normal[0]),
                                mm(p.normal[1]),
                                mm(p.normal[2])
                            ),
                        );
                    }
                }
                if let Some(f) = self
                    .selected
                    .and_then(|id| self.summary.features.iter().find(|f| f.id == id))
                    .cloned()
                {
                    row(ui, "Feature", format!("{} ({})", f.name, f.kind));
                    row(ui, "Health", format!("{:?}", f.health.state));
                    if let Some(m) = &f.health.message {
                        row(ui, "Reason", m.clone());
                    }
                    for (n, v) in &f.parameters {
                        let v = match f.expressions.get(n) {
                            Some(e) => format!("{} = {e}", mm(*v)),
                            None => mm(*v),
                        };
                        row(ui, n, v);
                    }
                    if !f.depends_on.is_empty() {
                        row(
                            ui,
                            "Uses",
                            f.depends_on
                                .iter()
                                .map(|d| self.feature_name(*d))
                                .collect::<Vec<_>>()
                                .join(", "),
                        );
                    }
                }
                let body = self
                    .selected_body
                    .and_then(|b| self.summary.bodies.iter().find(|x| x.id == b))
                    .cloned();
                if let Some(b) = body {
                    row(ui, "Body", format!("{} ({})", b.name, b.kind));
                    if let Some(m) = &b.measurements {
                        if let VolumeResult::Value { volume, .. } = m.volume {
                            row(ui, "Volume (mm³)", mm(volume));
                        }
                        row(
                            ui,
                            "Size (mm)",
                            format!(
                                "{} × {} × {}",
                                mm(m.bounds.max[0] - m.bounds.min[0]),
                                mm(m.bounds.max[1] - m.bounds.min[1]),
                                mm(m.bounds.max[2] - m.bounds.min[2])
                            ),
                        );
                        row(
                            ui,
                            "Topology",
                            format!(
                                "{} faces, {} edges, {} vertices",
                                m.face_count, m.edge_count, m.vertex_count
                            ),
                        );
                    }
                    if let Some(mat) = &b.material {
                        row(ui, "Material", format!("{mat:?}"));
                    }
                }
                if self.selected.is_none()
                    && self.selected_face.is_none()
                    && self.selected_body.is_none()
                {
                    row(ui, "Document", self.window_title());
                    row(ui, "Features", self.summary.features.len().to_string());
                    row(ui, "Bodies", self.summary.bodies.len().to_string());
                    if let Some(v) = self.volume() {
                        row(ui, "Volume (mm³)", mm(v));
                    }
                    row(ui, "Revision", self.summary.revision.to_string());
                }
            })
            .response
            .probe("Properties grid");
        ui.add_space(6.0);
        ui.weak("Read only. Commands are in the ribbon and context menus; the Inspector tab has the per-selection forms.");
    }
}
