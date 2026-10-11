//! Bodies in the browser and the body inspector.
//!
//! The browser lists every body of the document above the feature history:
//! select (highlights all of its faces in the view), inline rename
//! (double-click or context menu; Enter commits, Esc cancels), show/hide.
//! Every change is a document command (`RenameBody`, `SetBodyVisible`), so it
//! is one undoable step and the engine's reply is the only source of truth.
//!
//! The inspector shows what kind of geometry a body is and where it came from:
//! modeled bodies have feature history; imported STEP bodies are exact B-rep
//! without recovered history; STL bodies are triangle meshes on which exact
//! operations are unavailable.

use eframe::egui::{self, Color32, RichText};
use f420_commands::{BodySummary, Command};
use f420_document::{BodyId, BodySource};
use f420_kernel::VolumeResult;

use crate::app::F420App;
use crate::probe::Probe as _;

/// Inline rename buffer of a body row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BodyRename {
    pub body: BodyId,
    pub buffer: String,
    /// First frame: focus the field and select its text.
    pub focus: bool,
}

/// Give a widget an accessible label different from its visible text (the
/// visible text of row buttons repeats; tests and screen readers need a name).
fn labelled(r: egui::Response, label: String) -> egui::Response {
    labelled_as(r, egui::WidgetType::Button, label)
}

fn labelled_as(r: egui::Response, ty: egui::WidgetType, label: String) -> egui::Response {
    let enabled = r.enabled();
    r.widget_info(|| egui::WidgetInfo::labeled(ty, enabled, &label));
    r
}

/// One-line provenance of a body.
pub fn provenance(b: &BodySummary) -> String {
    match &b.source {
        BodySource::Modeled => "Modeled (feature history)".into(),
        BodySource::ImportedStep {
            file_name,
            solid_name,
        } => format!(
            "Imported from {file_name}{} — exact B-rep, no feature history",
            solid_name
                .as_deref()
                .map_or(String::new(), |n| format!(" (solid '{n}')"))
        ),
        BodySource::ImportedMesh { file_name } => {
            format!("Imported from {file_name} — triangle mesh, no feature history")
        }
    }
}

impl F420App {
    /// "Bodies" group of the browser.
    pub(crate) fn bodies_browser(&mut self, ui: &mut egui::Ui) {
        let bodies = self.summary.bodies.clone();
        ui.label(RichText::new(format!("Bodies ({})", bodies.len())).strong());
        if bodies.is_empty() {
            ui.weak("No bodies yet. Extrude a sketch or File › Import….");
            return;
        }
        let mut select = None;
        let mut toggle = None;
        let mut start_rename = None;
        let mut commit_rename = None;
        let mut cancel_rename = false;
        for b in &bodies {
            let sel = self.selected_body == Some(b.id);
            ui.horizontal(|ui| {
                let eye = if b.visible { "Hide" } else { "Show" };
                let r = ui
                    .small_button(eye)
                    .on_hover_text(if b.visible {
                        "Hide this body (it stays in the document; hidden bodies are not drawn, picked, fitted or exported by default)"
                    } else {
                        "Show this body"
                    });
                let r = labelled(r, format!("{eye} {}", b.name)).probe(&format!("{eye} {}", b.name));
                if r.clicked() {
                    toggle = Some((b.id, !b.visible));
                }
                let renaming = self.body_rename.as_ref().filter(|r| r.body == b.id).cloned();
                if let Some(mut rn) = renaming {
                    let id = egui::Id::new(("body_rename", b.id.0));
                    let mut out = egui::TextEdit::singleline(&mut rn.buffer)
                        .id(id)
                        .desired_width(140.0)
                        .show(ui);
                    let r = labelled_as(
                        egui::Response::clone(&out.response),
                        egui::WidgetType::TextEdit,
                        "Body name".into(),
                    )
                    .probe("Body name");
                    if rn.focus {
                        r.request_focus();
                        let n = rn.buffer.chars().count();
                        out.state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(n),
                            )));
                        out.state.store(ui.ctx(), id);
                        rn.focus = false;
                    }
                    let (enter, esc) = ui.input(|i| {
                        (
                            i.key_pressed(egui::Key::Enter),
                            i.key_pressed(egui::Key::Escape),
                        )
                    });
                    if esc {
                        cancel_rename = true;
                    } else if enter && (r.lost_focus() || r.has_focus()) {
                        commit_rename = Some((b.id, rn.buffer.clone()));
                    } else {
                        self.body_rename = Some(rn);
                    }
                } else {
                    let mut text = RichText::new(&b.name);
                    if !b.visible {
                        text = text.weak().italics();
                    }
                    let r = ui.selectable_label(sel, text).probe(&b.name);
                    if r.clicked() {
                        select = Some(b.id);
                    }
                    if r.double_clicked() {
                        start_rename = Some(b.id);
                    }
                    r.context_menu(|ui| {
                        if ui.button("Rename body").probe("Rename body").clicked() {
                            start_rename = Some(b.id);
                            ui.close();
                        }
                        let t = if b.visible { "Hide body" } else { "Show body" };
                        if ui.button(t).clicked() {
                            toggle = Some((b.id, !b.visible));
                            ui.close();
                        }
                    });
                }
                let (badge, col) = match (&b.source, b.kind.as_str()) {
                    (_, "mesh") => ("mesh", Color32::from_rgb(220, 190, 120)),
                    (BodySource::ImportedStep { .. }, _) => ("STEP", Color32::from_rgb(150, 200, 255)),
                    _ => ("solid", Color32::GRAY),
                };
                ui.label(RichText::new(badge).small().color(col))
                    .on_hover_text(provenance(b));
                if !b.visible {
                    ui.label(RichText::new("hidden").small().weak());
                }
            });
        }
        if let Some(id) = select
            && self.sketch.is_none()
        {
            self.select_body(Some(id));
        }
        if let Some((body, visible)) = toggle {
            self.request(Command::SetBodyVisible { body, visible });
        }
        if cancel_rename {
            self.body_rename = None;
        }
        if let Some((body, name)) = commit_rename {
            self.body_rename = None;
            let old = bodies.iter().find(|b| b.id == body).map(|b| b.name.clone());
            if old.as_deref() != Some(name.trim()) {
                self.request(Command::RenameBody { body, name });
            }
        }
        if let Some(id) = start_rename {
            let name = bodies
                .iter()
                .find(|b| b.id == id)
                .map_or(String::new(), |b| b.name.clone());
            self.body_rename = Some(BodyRename {
                body: id,
                buffer: name,
                focus: true,
            });
        }
    }

    /// Inspector content for the selected body.
    pub(crate) fn body_inspector(&mut self, ui: &mut egui::Ui, id: BodyId) {
        let Some(b) = self.summary.bodies.iter().find(|b| b.id == id).cloned() else {
            return;
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} (body)", b.name))
                    .strong()
                    .size(16.0),
            );
            if ui.small_button("Rename…").probe("Rename body…").clicked() {
                self.body_rename = Some(BodyRename {
                    body: b.id,
                    buffer: b.name.clone(),
                    focus: true,
                });
            }
            let t = if b.visible { "Hide" } else { "Show" };
            let r = labelled(ui.small_button(t), format!("{t} body"));
            if r.clicked() {
                self.request(Command::SetBodyVisible {
                    body: b.id,
                    visible: !b.visible,
                });
            }
        });
        let kind = if b.kind == "mesh" {
            "Triangle mesh (STL)"
        } else {
            "Exact solid (B-rep)"
        };
        ui.label(format!("Kind: {kind}"));
        ui.label(provenance(&b));
        if b.kind == "mesh" {
            ui.colored_label(
                Color32::from_rgb(255, 200, 120),
                "mesh: exact operations unavailable (no sketching on its faces, no join/cut, no STEP export). Display, hide/show, save and STL export only.",
            );
        } else if !matches!(b.source, BodySource::Modeled) {
            ui.small("Supported edits: sketch on its planar faces, then Extrude New Body / Join / Cut with this body as the target; export STEP/STL.");
        }
        if !b.visible {
            ui.weak("Hidden: not drawn, picked, fitted, or exported unless explicitly selected.");
        }
        ui.small(format!(
            "Created by {} · last modified by {}",
            self.feature_name(b.created_by),
            self.feature_name(b.last_feature)
        ));
        ui.separator();
        match (&b.measurements, &b.mesh) {
            (Some(m), _) => {
                match &m.volume {
                    VolumeResult::Value { volume, .. } => {
                        ui.label(format!("Volume {volume:.4} mm³"));
                    }
                    VolumeResult::Unavailable { reason } => {
                        ui.label(format!("Volume unavailable: {reason}"));
                    }
                }
                let (a, c) = (m.bounds.min, m.bounds.max);
                ui.small(format!(
                    "Bounds X[{:.3}, {:.3}] Y[{:.3}, {:.3}] Z[{:.3}, {:.3}] mm",
                    a[0], c[0], a[1], c[1], a[2], c[2]
                ));
                ui.small(format!(
                    "{} shell{} ({}) · {} faces · {} edges · {} vertices",
                    m.shell_count,
                    if m.shell_count == 1 { "" } else { "s" },
                    if m.all_shells_closed {
                        "closed"
                    } else {
                        "OPEN"
                    },
                    m.face_count,
                    m.edge_count,
                    m.vertex_count
                ));
            }
            (None, Some(d)) => {
                ui.label(format!(
                    "{} triangles · {} components · {}",
                    d.triangles,
                    d.components,
                    if d.closed { "closed" } else { "NOT closed" }
                ));
                if let Some((a, c)) = d.bounds {
                    ui.small(format!(
                        "Bounds X[{:.3}, {:.3}] Y[{:.3}, {:.3}] Z[{:.3}, {:.3}] mm",
                        a[0], c[0], a[1], c[1], a[2], c[2]
                    ));
                }
                match d.signed_volume {
                    Some(v) => {
                        ui.small(format!("Mesh volume {v:.4} mm³ (triangle mesh, not exact)"))
                    }
                    None => ui.small("No volume: the mesh is not closed"),
                };
                ui.small(format!(
                    "boundary edges {} · non-manifold edges {} · inconsistent orientation {} · degenerate triangles {}",
                    d.boundary_edges,
                    d.non_manifold_edges,
                    d.inconsistent_orientation_edges,
                    d.degenerate_triangles
                ));
            }
            (None, None) if self.summary.measuring => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("Measuring…");
                });
            }
            (None, None) => {
                ui.weak("No measurements available.");
            }
        }
    }
}
