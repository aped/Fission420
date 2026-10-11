//! Re-attach (repair) a sketch's support plane.
//!
//! When an edit would leave a sketch's face reference broken or ambiguous the
//! document refuses it with `BrokenReference` and keeps the last valid model;
//! it never silently re-attaches. The banner then offers "Re-attach sketch…",
//! and every sketch's inspector has "Re-attach…". The window lists world
//! planes and every referenceable face of features earlier in the history
//! (plus the face picked in the view), previews the sketch's profile in the
//! chosen frame, and commits one `SetSketchPlane` (one undo step). The sketch's
//! local geometry is kept. The refused edit can then be repeated.

use eframe::egui::{self, Color32, RichText};
use f420_commands::Command;
use f420_document::{FeatureId, SketchPlane};

use crate::app::F420App;
use crate::place::PlaneFrame;
use crate::probe::Probe as _;

#[derive(Debug, Clone, PartialEq)]
pub struct Reattach {
    pub sketch: FeatureId,
    pub choice: Option<SketchPlane>,
}

impl F420App {
    /// Open the re-attach window for `sketch`.
    pub fn start_reattach(&mut self, sketch: FeatureId) {
        let picked = self
            .selected_face()
            .and_then(|f| f.reference)
            .map(SketchPlane::Face)
            // Only a face the window offers (not one of a later feature: cycle).
            .filter(|p| self.support_options(sketch).iter().any(|(o, _)| o == p));
        self.reattach = Some(Reattach {
            sketch,
            choice: picked,
        });
    }

    /// Supports a sketch may be moved to: world planes and faces of features
    /// before it in the history (later ones would create a cycle).
    fn support_options(&self, sketch: FeatureId) -> Vec<(SketchPlane, String)> {
        let mut v = vec![
            (SketchPlane::WorldXY, "World XY".to_string()),
            (SketchPlane::WorldXZ, "World XZ".to_string()),
            (SketchPlane::WorldYZ, "World YZ".to_string()),
        ];
        let fs = &self.summary.features;
        let pos = fs.iter().position(|f| f.id == sketch).unwrap_or(fs.len());
        for f in &fs[..pos] {
            for face in &f.faces {
                let body = self
                    .summary
                    .bodies
                    .iter()
                    .find(|b| b.id == face.body)
                    .map_or(String::new(), |b| format!(" ({})", b.name));
                v.push((
                    SketchPlane::Face(face.reference),
                    format!(
                        "{} {}{body}",
                        f.name,
                        crate::face_role_label(&face.reference.role)
                    ),
                ));
            }
        }
        v
    }

    /// Frame the chosen support would give, from the current summary.
    pub fn reattach_preview_frame(&self) -> Option<PlaneFrame> {
        let r = self.reattach.as_ref()?;
        self.plane_frame(r.choice.as_ref()?).ok()
    }

    pub(crate) fn reattach_ui(&mut self, ctx: &egui::Context) {
        let Some(mut r) = self.reattach.clone() else {
            return;
        };
        let Some(f) = self
            .summary
            .features
            .iter()
            .find(|f| f.id == r.sketch)
            .cloned()
        else {
            self.reattach = None;
            return;
        };
        let options = self.support_options(r.sketch);
        let mut done: Option<bool> = None;
        egui::Window::new("Re-attach sketch")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 70.0])
            .show(ctx, |ui| {
                ui.label(RichText::new(&f.name).strong());
                if let Some(p) = &f.plane {
                    ui.small(format!("Current support: {}", self.plane_text(p)));
                }
                let text = r
                    .choice
                    .and_then(|c| options.iter().find(|(p, _)| *p == c))
                    .map_or("(choose)".to_string(), |(_, l)| l.clone());
                egui::ComboBox::from_label("New support")
                    .selected_text(text)
                    .width(280.0)
                    .show_ui(ui, |ui| {
                        for (p, l) in &options {
                            ui.selectable_value(&mut r.choice, Some(*p), l);
                        }
                    });
                ui.small("Pick a face in the view before opening this window to preselect it.");
                match r.choice.as_ref().map(|c| self.plane_frame(c)) {
                    Some(Ok(fr)) => {
                        ui.small(format!(
                            "Preview frame: origin ({:.3}, {:.3}, {:.3}) normal ({:.3}, {:.3}, {:.3}); the sketch keeps its local coordinates (dashed outline in the view).",
                            fr.origin[0], fr.origin[1], fr.origin[2], fr.z[0], fr.z[1], fr.z[2]
                        ));
                    }
                    Some(Err(e)) => {
                        ui.colored_label(Color32::from_rgb(255, 110, 110), e);
                    }
                    None => {}
                }
                ui.small("One undoable step. Features using this sketch regenerate on the new support; a refused regeneration keeps the model.");
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(r.choice.is_some(), egui::Button::new("Re-attach"))
                        .probe("Re-attach")
                        .clicked()
                    {
                        done = Some(true);
                    }
                    if ui.button("Cancel re-attach").probe("Cancel re-attach").clicked() {
                        done = Some(false);
                    }
                });
            });
        match done {
            Some(true) => {
                self.reattach = None;
                if let Some(plane) = r.choice {
                    self.request(Command::SetSketchPlane {
                        feature: r.sketch,
                        plane,
                    });
                }
            }
            Some(false) => self.reattach = None,
            None => self.reattach = Some(r),
        }
    }

    /// Dashed outline of the sketch being re-attached, in the chosen frame.
    pub(crate) fn paint_reattach_preview(&self, painter: &egui::Painter, rect: egui::Rect) {
        let (Some(r), Some(fr)) = (&self.reattach, self.reattach_preview_frame()) else {
            return;
        };
        let Some(sk) = self
            .summary
            .features
            .iter()
            .find(|f| f.id == r.sketch)
            .and_then(|f| f.sketch.as_ref())
        else {
            return;
        };
        let Some(outline) = crate::extrude::profile_outline(sk) else {
            return;
        };
        let mut ov = f420_viewport::Overlay::default();
        ov.polylines.push(f420_viewport::OverlayPolyline {
            points: outline
                .iter()
                .map(|p| fr.to_world(*p).map(|v| v as f32))
                .collect(),
            closed: true,
            color: Color32::from_rgb(255, 200, 90),
            width: 2.0,
            dashed: true,
        });
        ov.labels.push(f420_viewport::OverlayLabel {
            at: fr.origin.map(|v| v as f32),
            text: "re-attach preview".into(),
            color: Color32::from_rgb(255, 200, 90),
        });
        let (cam, _) = self.view_camera();
        f420_viewport::paint_overlay(painter, &cam, rect, &ov);
    }
}
