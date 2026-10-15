//! Mesh inspection and mesh edits in the body inspector (docs/MESH.md).
//!
//! * "Mesh inspection": the stored diagnosis (duplicates, components with
//!   bounds/volume) and **Check self-intersections** (bounded; read only).
//! * "Mesh edit": transform (with inch → mm and m → mm presets), plane cut
//!   (axis-aligned plane, keep below/above/both, cap) and decimation (target
//!   triangles, maximum deviation) with **Preview** (counts only, nothing
//!   changes) and **Apply as new body** (one undo step; the source is kept,
//!   hidden).
//! * Viewport preview ([`RcadApp::mesh_edit_preview`]): while the panel is shown, the
//!   plane of a plane cut is drawn across the body (keep side shaded), and after
//!   **Preview** the bounds of each result are drawn as boxes.
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
    pub(crate) pending: Option<(BodyId, bool)>,
    /// (body, egui pass) where the panel was last shown: the viewport preview
    /// follows the open panel only.
    shown: Option<(BodyId, u64)>,
    /// Latest egui pass seen by the viewport painter.
    pass: std::cell::Cell<u64>,
    /// Open the inspection and edit panels on their next draw (scripted scenes).
    pub open_panels: bool,
}

/// World-space viewport preview of a mesh edit (mm).
#[derive(Debug, Clone, PartialEq)]
pub struct MeshEditPreview {
    pub body: BodyId,
    /// Plane cut: the plane clipped to the body's bounds (+5 %), 4 corners.
    pub plane: Option<[[f64; 3]; 4]>,
    /// Plane cut: unit vector towards the kept side (None: both kept).
    pub keep_dir: Option<[f64; 3]>,
    /// After **Preview**: bounds `(min, max)` of each result.
    pub result_bounds: Vec<([f64; 3], [f64; 3])>,
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
            shown: None,
            pass: std::cell::Cell::new(0),
            open_panels: false,
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
            .open(self.mesh_edit.open_panels.then_some(true))
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
        let force = std::mem::take(&mut self.mesh_edit.open_panels);
        let header = egui::CollapsingHeader::new("Mesh edit")
            .id_salt(("mesh_edit", body.0))
            .open(
                force
                    .then_some(true)
                    .or(self.shell.take_focus(crate::shell::Focus::MeshEdit)),
            )
            .show(ui, |ui| {
                let st = &mut self.mesh_edit;
                st.shown = Some((body, ui.ctx().cumulative_pass_nr()));
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
                            for (label, s) in [("inch to mm (×25.4)", "25.4"), ("m to mm (×1000)", "1000")] {
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
                // A preview describes the fields it was made from: once they
                // change (or no longer parse) it is not shown.
                if let Some((d, r, b, preview, rep)) = &st.report
                    && *d == doc
                    && *b == body
                    && *r == rev
                    && (!*preview || op.as_ref().ok() == Some(&rep.op))
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

impl RcadApp {
    /// The viewport preview of the mesh edit panel (None when it is not shown).
    pub fn mesh_edit_preview(&self) -> Option<MeshEditPreview> {
        let st = &self.mesh_edit;
        let (body, pass) = st.shown?;
        if self.ctx_pass().saturating_sub(pass) > 2 {
            return None;
        }
        let (lo, hi) = self
            .summary
            .bodies
            .iter()
            .find(|b| b.id == body)?
            .mesh
            .as_ref()?
            .bounds?;
        let mut pv = MeshEditPreview {
            body,
            plane: None,
            keep_dir: None,
            result_bounds: vec![],
        };
        if st.kind == EditKind::PlaneCut
            && let Ok(MeshEditOp::PlaneCut(c)) = st.op()
        {
            let k = st.cut_axis;
            let (u, v) = ((k + 1) % 3, (k + 2) % 3);
            let pad = |a: f64, b: f64| (a - 0.05 * (b - a).max(1.0), b + 0.05 * (b - a).max(1.0));
            let (u0, u1) = pad(lo[u], hi[u]);
            let (v0, v1) = pad(lo[v], hi[v]);
            let corner = |a: f64, b: f64| {
                let mut p = [0.0; 3];
                p[k] = c.offset;
                p[u] = a;
                p[v] = b;
                p
            };
            pv.plane = Some([
                corner(u0, v0),
                corner(u1, v0),
                corner(u1, v1),
                corner(u0, v1),
            ]);
            pv.keep_dir = match c.keep {
                CutKeep::Below => Some(unit(k).map(|x| -x)),
                CutKeep::Above => Some(unit(k)),
                CutKeep::Both => None,
            };
        }
        if let Some((d, r, b, true, rep)) = &st.report
            && *d == self.summary.doc_id
            && *r == self.summary.revision
            && *b == body
            && st.op().ok() == Some(rep.op)
        {
            pv.result_bounds = rep.after.iter().filter_map(|a| a.bounds).collect();
        }
        Some(pv)
    }

    fn ctx_pass(&self) -> u64 {
        self.mesh_edit.pass.get()
    }

    /// Draw [`RcadApp::mesh_edit_preview`] over the 3D view.
    pub(crate) fn paint_mesh_edit_preview(&self, painter: &egui::Painter) {
        self.mesh_edit.pass.set(painter.ctx().cumulative_pass_nr());
        let Some(pv) = self.mesh_edit_preview() else {
            return;
        };
        let to = |p: [f64; 3]| self.world_to_screen(p);
        if let Some(q) = pv.plane
            && let Some(pts) = q.iter().map(|p| to(*p)).collect::<Option<Vec<_>>>()
        {
            painter.add(egui::Shape::convex_polygon(
                pts.clone(),
                Color32::from_rgba_unmultiplied(255, 170, 60, 50),
                egui::Stroke::new(1.5, Color32::from_rgb(255, 170, 60)),
            ));
            if let Some(dir) = pv.keep_dir {
                // Arrow from the plane centre towards the kept side.
                let c: [f64; 3] =
                    std::array::from_fn(|i| q.iter().map(|p| p[i]).sum::<f64>() / 4.0);
                let span = (0..3)
                    .map(|i| (q[2][i] - q[0][i]).abs())
                    .fold(0.0, f64::max);
                let tip: [f64; 3] = std::array::from_fn(|i| c[i] + dir[i] * 0.25 * span);
                if let (Some(a), Some(b)) = (to(c), to(tip)) {
                    painter.arrow(
                        a,
                        b - a,
                        egui::Stroke::new(2.0, Color32::from_rgb(255, 170, 60)),
                    );
                }
            }
            let r = egui::Rect::from_points(&pts);
            crate::probe::note("Mesh cut plane preview", r);
        }
        for (lo, hi) in &pv.result_bounds {
            let c = |i: usize| {
                [
                    if i & 1 == 0 { lo[0] } else { hi[0] },
                    if i & 2 == 0 { lo[1] } else { hi[1] },
                    if i & 4 == 0 { lo[2] } else { hi[2] },
                ]
            };
            for (a, b) in [
                (0, 1),
                (2, 3),
                (4, 5),
                (6, 7),
                (0, 2),
                (1, 3),
                (4, 6),
                (5, 7),
                (0, 4),
                (1, 5),
                (2, 6),
                (3, 7),
            ] {
                if let (Some(p), Some(q)) = (to(c(a)), to(c(b))) {
                    painter.line_segment(
                        [p, q],
                        egui::Stroke::new(1.5, Color32::from_rgb(120, 200, 255)),
                    );
                }
            }
        }
    }
}
