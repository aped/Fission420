//! Mesh inspection and mesh edits in the body inspector (docs/MESH.md).
//!
//! * "Mesh inspection": the stored diagnosis (duplicates, components with
//!   bounds/volume) and **Check self-intersections** (bounded; read only).
//! * "Mesh edit": transform (with inch → mm and m → mm presets), plane cut
//!   (axis-aligned plane, keep below/above/both, cap) and decimation (target
//!   triangles, maximum deviation) with **Preview** (counts only, nothing
//!   changes) and **Apply as new body** (one undo step; the source is kept,
//!   hidden).
use crate::app::RcadApp;
use crate::probe::Probe as _;
use eframe::egui::{self, Color32, RichText};
use rcad_commands::mesh_edit::{EditMeshArgs, InspectMeshArgs, MeshInspection};
use rcad_commands::{Command, DocSummary, Response};
use rcad_document::BodyId;
use rcad_exchange::mesh_edit::{
    CutKeep, Decimate, MeshEditOp, MeshEditReport, MeshTransform, PlaneCut,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditKind {
    #[default]
    Transform,
    PlaneCut,
    Decimate,
}

#[derive(Debug, Clone)]
pub struct MeshEditUi {
    pub kind: EditKind,
    /// Transform fields as typed: sx, sy, sz, angle (deg), tx, ty, tz.
    pub t: [String; 7],
    /// Rotation axis / cut normal: 0 X, 1 Y, 2 Z.
    pub axis: usize,
    pub cut_axis: usize,
    pub cut_offset: String,
    pub keep: CutKeep,
    pub cap: bool,
    pub target: String,
    pub max_dev: String,
    /// (doc_id, revision, body, preview?, report) of the last edit.
    pub report: Option<(u64, u64, BodyId, bool, MeshEditReport)>,
    /// (doc_id, revision, inspection) of the last self-intersection check.
    pub inspection: Option<(u64, u64, MeshInspection)>,
    pending: Option<(BodyId, bool)>,
}

impl Default for MeshEditUi {
    fn default() -> Self {
        MeshEditUi {
            kind: EditKind::Transform,
            t: ["1", "1", "1", "0", "0", "0", "0"].map(String::from),
            axis: 2,
            cut_axis: 2,
            cut_offset: "0".into(),
            keep: CutKeep::Below,
            cap: true,
            target: "0".into(),
            max_dev: "0.05".into(),
            report: None,
            inspection: None,
            pending: None,
        }
    }
}

const AXES: [&str; 3] = ["X", "Y", "Z"];

fn unit(k: usize) -> [f64; 3] {
    let mut v = [0.0; 3];
    v[k] = 1.0;
    v
}

impl MeshEditUi {
    pub(crate) fn on_reply(&mut self, response: &Response, summary: &DocSummary) {
        if let Some(i) = &response.mesh_inspection {
            self.inspection = Some((summary.doc_id, summary.revision, i.clone()));
        }
        if let (Some(r), Some((b, preview))) = (&response.mesh_edit, self.pending.take()) {
            self.report = Some((summary.doc_id, summary.revision, b, preview, r.clone()));
        }
    }

    /// The operation described by the fields, or the first field that does not parse.
    pub fn op(&self) -> Result<MeshEditOp, String> {
        let num = |s: &str, what: &str| {
            s.trim()
                .parse::<f64>()
                .map_err(|_| format!("{what}: not a number"))
        };
        let op = match self.kind {
            EditKind::Transform => {
                let names = [
                    "scale X", "scale Y", "scale Z", "angle", "move X", "move Y", "move Z",
                ];
                let mut v = [0.0; 7];
                for k in 0..7 {
                    v[k] = num(&self.t[k], names[k])?;
                }
                MeshEditOp::Transform(MeshTransform {
                    scale: [v[0], v[1], v[2]],
                    axis: unit(self.axis),
                    angle_deg: v[3],
                    translate: [v[4], v[5], v[6]],
                })
            }
            EditKind::PlaneCut => MeshEditOp::PlaneCut(PlaneCut {
                origin: [0.0; 3],
                normal: unit(self.cut_axis),
                offset: num(&self.cut_offset, "plane offset")?,
                keep: self.keep,
                cap: self.cap,
            }),
            EditKind::Decimate => {
                let t = self
                    .target
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| "target triangles: a whole number".to_string())?;
                MeshEditOp::Decimate(Decimate {
                    target_triangles: t,
                    max_deviation: num(&self.max_dev, "max deviation")?,
                })
            }
        };
        op.validate()?;
        Ok(op)
    }
}

/// One line per component (at most `n`).
fn components(d: &rcad_exchange::mesh::MeshDiagnostics, n: usize) -> Vec<String> {
    d.component_details
        .iter()
        .take(n)
        .enumerate()
        .map(|(i, c)| {
            let (lo, hi) = c.bounds;
            format!(
                "#{}: {} triangles, size {:.3} × {:.3} × {:.3} mm, {}",
                i + 1,
                c.triangles,
                hi[0] - lo[0],
                hi[1] - lo[1],
                hi[2] - lo[2],
                match c.signed_volume {
                    Some(v) => format!("closed, volume {v:.3} mm³"),
                    None => "open (no volume)".into(),
                }
            )
        })
        .collect()
}

impl RcadApp {
    pub(crate) fn mesh_inspect_ui(&mut self, ui: &mut egui::Ui, body: BodyId) {
        let (doc, rev) = (self.summary.doc_id, self.summary.revision);
        let diag = self
            .summary
            .bodies
            .iter()
            .find(|b| b.id == body)
            .and_then(|b| b.mesh.clone());
        let mut send = None;
        let header = egui::CollapsingHeader::new("Mesh inspection")
            .id_salt(("mesh_inspect", body.0))
            .show(ui, |ui| {
                if let Some(d) = &diag {
                    ui.label(format!(
                        "{} triangles · {} duplicate · {} degenerate · {} components{}",
                        d.triangles,
                        d.duplicate_triangles,
                        d.degenerate_triangles,
                        d.components,
                        if d.components > d.component_details.len() {
                            format!(" ({} listed)", d.component_details.len())
                        } else {
                            String::new()
                        }
                    ))
                    .probe("Mesh inspection counts");
                    for line in components(d, 12) {
                        ui.small(line);
                    }
                    if d.component_details.len() > 12 {
                        ui.small(format!("… {} more", d.component_details.len() - 12));
                    }
                }
                if ui
                    .button("Check self-intersections")
                    .named("Check self-intersections")
                    .on_hover_text(
                        "Triangle-pair test on a uniform grid (bounded). Neighbouring \
                         triangles (sharing a vertex) are not tested; a partial check is \
                         labelled. Read only.",
                    )
                    .clicked()
                {
                    send = Some(Command::InspectMesh(InspectMeshArgs { body, limits: None }));
                }
                if let Some((d, r, ins)) = &self.mesh_edit.inspection
                    && *d == doc
                    && *r == rev
                    && ins.body == body
                {
                    let si = &ins.self_intersection;
                    let colour = if si.intersecting_pairs == 0 && si.complete {
                        Color32::from_rgb(120, 200, 120)
                    } else {
                        Color32::from_rgb(255, 200, 120)
                    };
                    ui.label(
                        RichText::new(rcad_commands::mesh_edit::inspection_summary(si))
                            .small()
                            .color(colour),
                    )
                    .probe("Self-intersection result");
                    if !si.examples.is_empty() {
                        ui.small(format!(
                            "pairs (triangle indices): {}",
                            si.examples
                                .iter()
                                .map(|(a, b)| format!("{a}–{b}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    ui.small(&si.method);
                }
            });
        header.header_response.probe("Mesh inspection");
        if let Some(c) = send {
            self.request(c);
        }
    }

    pub(crate) fn mesh_edit_ui(&mut self, ui: &mut egui::Ui, body: BodyId) {
        let (doc, rev) = (self.summary.doc_id, self.summary.revision);
        let mut send = None;
        let header = egui::CollapsingHeader::new("Mesh edit")
            .id_salt(("mesh_edit", body.0))
            .show(ui, |ui| {
                let st = &mut self.mesh_edit;
                ui.horizontal_wrapped(|ui| {
                    for (k, label) in [
                        (EditKind::Transform, "Transform"),
                        (EditKind::PlaneCut, "Plane cut"),
                        (EditKind::Decimate, "Decimate"),
                    ] {
                        ui.selectable_value(&mut st.kind, k, label).named(label);
                    }
                });
                let field = |ui: &mut egui::Ui, label: &str, v: &mut String| {
                    ui.label(label);
                    ui.add(egui::TextEdit::singleline(v).desired_width(56.0))
                        .named(&format!("Mesh {label}"));
                };
                match st.kind {
                    EditKind::Transform => {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Unit presets:");
                            for (label, s) in [("inch → mm (×25.4)", "25.4"), ("m → mm (×1000)", "1000")] {
                                if ui.button(label).named(label).clicked() {
                                    for k in 0..3 {
                                        st.t[k] = s.into();
                                    }
                                }
                            }
                        });
                        let [sx, sy, sz, ang, tx, ty, tz] = &mut st.t;
                        ui.horizontal_wrapped(|ui| {
                            field(ui, "scale X", sx);
                            field(ui, "scale Y", sy);
                            field(ui, "scale Z", sz);
                        });
                        ui.horizontal_wrapped(|ui| {
                            field(ui, "rotate (deg)", ang);
                            ui.label("about");
                            for (k, a) in AXES.iter().enumerate() {
                                ui.radio_value(&mut st.axis, k, *a);
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            field(ui, "move X", tx);
                            field(ui, "move Y", ty);
                            field(ui, "move Z", tz);
                        });
                        ui.small("Order: scale, rotate about the origin, then move (mm). Scale factors must be > 0 (no mirroring).");
                    }
                    EditKind::PlaneCut => {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Plane normal");
                            for (k, a) in AXES.iter().enumerate() {
                                ui.radio_value(&mut st.cut_axis, k, *a);
                            }
                            field(ui, "at (mm)", &mut st.cut_offset);
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Keep");
                            ui.radio_value(&mut st.keep, CutKeep::Below, "below").named("Keep below");
                            ui.radio_value(&mut st.keep, CutKeep::Above, "above").named("Keep above");
                            ui.radio_value(&mut st.keep, CutKeep::Both, "both (two bodies)").named("Keep both");
                        });
                        ui.checkbox(&mut st.cap, "Cap the cut (closed meshes only)")
                            .named("Cap the cut");
                    }
                    EditKind::Decimate => {
                        ui.horizontal_wrapped(|ui| {
                            field(ui, "target triangles (0 = none)", &mut st.target);
                            field(ui, "max deviation (mm)", &mut st.max_dev);
                        });
                        ui.small(
                            "Quadric edge collapse; boundaries and topology are kept. The \
                             deviation from the source is measured (sampled) and reported.",
                        );
                    }
                }
                let op = st.op();
                if let Err(e) = &op {
                    ui.colored_label(Color32::from_rgb(255, 160, 120), e);
                }
                ui.horizontal_wrapped(|ui| {
                    for (label, preview) in [("Preview edit", true), ("Apply as new body", false)] {
                        if ui
                            .add_enabled(op.is_ok(), egui::Button::new(label))
                            .named(label)
                            .clicked()
                            && let Ok(o) = &op
                        {
                            st.pending = Some((body, preview));
                            send = Some(Command::EditMesh(EditMeshArgs {
                                body,
                                op: *o,
                                preview,
                                body_name: None,
                                hide_source: !preview,
                            }));
                        }
                    }
                });
                if let Some((d, r, b, preview, rep)) = &st.report
                    && *d == doc
                    && *b == body
                    && *r == rev
                {
                    let t = rcad_document::mesh_edit::summary(rep);
                    let closed = rep.after.iter().all(|a| a.closed);
                    let colour = if closed {
                        Color32::from_rgb(120, 200, 120)
                    } else {
                        Color32::from_rgb(255, 200, 120)
                    };
                    ui.label(
                        RichText::new(format!("{}{t}", if *preview { "preview: " } else { "" }))
                            .small()
                            .color(colour),
                    )
                    .probe("Mesh edit report");
                    for n in &rep.notes {
                        ui.small(n);
                    }
                    if let Some(dv) = &rep.deviation {
                        ui.small(&dv.method);
                    }
                }
            });
        header.header_response.probe("Mesh edit");
        if let Some(c) = send {
            self.request(c);
        }
    }
}
