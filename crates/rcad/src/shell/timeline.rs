//! Bottom timeline strip (Fusion's timeline): one marker per feature in
//! history order, tinted by health, rolled-back features dimmed after the
//! history marker. Drag the history marker between features to roll back
//! (`SetRollback`), double-click a feature to edit it, right-click for Edit
//! Feature / Roll back here. Accessible names: "Timeline <feature>" and
//! "Timeline marker".

use eframe::egui::{self, Color32, Sense, Stroke, vec2};
use rcad_commands::{Command, FeatureSummary};
use rcad_document::{FeatureId, HealthState};

use crate::app::RcadApp;
use crate::probe::Probe as _;

const CHIP: f32 = 22.0;

fn glyph(kind: &str) -> &'static str {
    match kind {
        "sketch" => "✏",
        "extrude" => "⬆",
        "revolve" => "⟳",
        "hole" | "cut_through_all" | "pocket" => "○",
        "fillet" | "fillet_loop" => "◔",
        "chamfer" | "chamfer_loop" => "⬊",
        "shell" => "☐",
        "linear_pattern" | "circular_pattern" => "⊞",
        "mirror" => "↔",
        "thread" => "≈",
        "loft" | "sweep" => "⤴",
        k if k.starts_with("import") => "⬇",
        k if k.starts_with("sheet") => "⊟",
        k if k.starts_with("surface") || k == "thicken" => "◊",
        k if k.starts_with("mesh") => "⏶",
        _ => "■",
    }
}

fn tint(f: &FeatureSummary) -> Color32 {
    match f.health.state {
        HealthState::Error => Color32::from_rgb(200, 70, 70),
        HealthState::Warning => Color32::from_rgb(210, 160, 60),
        _ if f.suppressed || f.rolled_back => Color32::from_gray(70),
        _ => Color32::from_rgb(70, 110, 160),
    }
}

impl RcadApp {
    pub(crate) fn timeline_ui(&mut self, ui: &mut egui::Ui) {
        let feats = self.summary.features.clone();
        if !self.summary.open || feats.is_empty() {
            ui.weak("Timeline: empty");
            return;
        }
        // Index of the first rolled-back feature (marker sits before it).
        let marker_at = feats
            .iter()
            .position(|f| f.rolled_back)
            .unwrap_or(feats.len());
        let mut edit: Option<FeatureId> = None;
        let mut select: Option<FeatureId> = None;
        let mut roll: Option<Option<FeatureId>> = None;
        let mut gaps: Vec<(f32, usize)> = vec![];
        let mut marker_drag: Option<egui::Response> = None;
        egui::ScrollArea::horizontal()
            .id_salt("timeline_scroll")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (i, f) in feats.iter().enumerate() {
                        if i == marker_at {
                            marker_drag = Some(self.marker(ui));
                        }
                        gaps.push((ui.cursor().min.x, i));
                        let (rect, r) = ui.allocate_exact_size(vec2(CHIP, CHIP), Sense::click());
                        let fill = tint(f);
                        let sel = self.selected == Some(f.id);
                        ui.painter().rect(
                            rect,
                            3.0,
                            fill,
                            Stroke::new(
                                if sel { 2.0 } else { 1.0 },
                                if sel {
                                    Color32::WHITE
                                } else {
                                    Color32::from_gray(30)
                                },
                            ),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            glyph(&f.kind),
                            egui::FontId::proportional(13.0),
                            Color32::WHITE,
                        );
                        let name = format!("Timeline {}", f.name);
                        let r = r.named(&name);
                        let mut tip = f.name.clone();
                        if let Some(m) = &f.health.message {
                            tip.push_str(&format!("\n{m}"));
                        }
                        let r = r.on_hover_text(tip);
                        if r.double_clicked() {
                            edit = Some(f.id);
                        } else if r.clicked() {
                            select = Some(f.id);
                        }
                        r.context_menu(|ui| {
                            let label = if f.kind == "sketch" {
                                "Edit Sketch"
                            } else {
                                "Edit Feature"
                            };
                            if ui.button(label).clicked() {
                                edit = Some(f.id);
                                ui.close();
                            }
                            if ui.button("Roll History Marker Here").clicked() {
                                roll = Some(Some(f.id));
                                ui.close();
                            }
                        });
                    }
                    gaps.push((ui.cursor().min.x, feats.len()));
                    if marker_at == feats.len() {
                        marker_drag = Some(self.marker(ui));
                    }
                    if ui
                        .small_button("⏭")
                        .named("Timeline to end")
                        .on_hover_text("Roll the marker to the end (evaluate every feature)")
                        .clicked()
                    {
                        roll = Some(None);
                    }
                });
            });
        // Dropping the marker: the nearest gap; marker before feature i =
        // roll back to feature i-1 (None at the end).
        if let Some(m) = marker_drag
            && m.drag_stopped()
            && let Some(p) = m.interact_pointer_pos().or(ui.ctx().pointer_latest_pos())
            && let Some(&(_, i)) = gaps
                .iter()
                .min_by(|a, b| (a.0 - p.x).abs().total_cmp(&(b.0 - p.x).abs()))
            && i != marker_at
        {
            roll = Some(if i >= feats.len() {
                None
            } else if i == 0 {
                // Nothing before the first feature: keep it (refused otherwise).
                Some(feats[0].id)
            } else {
                Some(feats[i - 1].id)
            });
        }
        if let Some(id) = select {
            self.select(Some(id));
        }
        if let Some(at) = roll {
            self.request(Command::SetRollback { at });
        }
        if let Some(id) = edit {
            self.edit_feature(id);
        }
    }

    /// The draggable history marker.
    fn marker(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let (rect, r) = ui.allocate_exact_size(vec2(8.0, CHIP), Sense::drag());
        let c = Color32::from_rgb(255, 200, 90);
        ui.painter()
            .rect_filled(rect.shrink2(vec2(2.0, 0.0)), 1.0, c);
        if r.dragged()
            && let Some(p) = ui.ctx().pointer_latest_pos()
        {
            ui.painter()
                .vline(p.x, rect.y_range(), Stroke::new(2.0, c.gamma_multiply(0.6)));
        }
        r.named("Timeline marker")
            .on_hover_text("History marker: drag it between features to roll back")
            .probe("Timeline marker")
    }
}
