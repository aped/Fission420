//! Mesh repair in the body inspector (run 7, Run-8 pull-forward ME01;
//! docs/MESH.md): the diagnosis of a mesh body, repair options with an
//! explicit weld tolerance, **Preview** (counts only, nothing changes) and
//! **Repair as new body** (one undo step; the source is kept, hidden).
use crate::app::RcadApp;
use crate::probe::Probe as _;
use eframe::egui::{self, Color32, RichText};
use rcad_commands::mesh_repair::RepairMeshArgs;
use rcad_commands::{Command, DocSummary, Response};
use rcad_document::BodyId;
use rcad_exchange::mesh_repair::{RepairOptions, RepairReport};

#[derive(Debug, Clone, Default)]
pub struct MeshRepairUi {
    pub options: RepairOptions,
    /// Weld tolerance as typed (mm).
    pub tolerance: String,
    /// Hole size limit as typed (edges; 0 = no filling).
    pub max_edges: String,
    /// (doc_id, revision, body, preview?, report) of the last preview or repair.
    pub report: Option<(u64, u64, BodyId, bool, RepairReport)>,
    pending: Option<(BodyId, bool)>,
}

impl MeshRepairUi {
    pub(crate) fn on_reply(&mut self, response: &Response, summary: &DocSummary) {
        if let (Some(r), Some((b, preview))) = (&response.mesh_repair, self.pending.take()) {
            self.report = Some((summary.doc_id, summary.revision, b, preview, r.clone()));
        }
    }
}

impl RcadApp {
    pub(crate) fn mesh_repair_ui(&mut self, ui: &mut egui::Ui, body: BodyId) {
        let (doc, rev) = (self.summary.doc_id, self.summary.revision);
        let diag = self
            .summary
            .bodies
            .iter()
            .find(|b| b.id == body)
            .and_then(|b| b.mesh.clone());
        let mut send = None;
        let header = egui::CollapsingHeader::new("Mesh check / repair")
            .id_salt(("mesh_repair", body.0))
            .show(ui, |ui| {
                if let Some(d) = &diag {
                    ui.label(format!(
                        "{} boundary · {} non-manifold · {} misoriented edges · {} degenerate \
                         triangles · {} components",
                        d.boundary_edges,
                        d.non_manifold_edges,
                        d.inconsistent_orientation_edges,
                        d.degenerate_triangles,
                        d.components
                    ))
                    .probe("Mesh diagnosis");
                }
                let st = &mut self.mesh_repair;
                if st.tolerance.is_empty() {
                    st.tolerance = "0".into();
                    st.max_edges = "0".into();
                }
                ui.horizontal(|ui| {
                    ui.label("Weld tolerance (mm)");
                    ui.add(egui::TextEdit::singleline(&mut st.tolerance).desired_width(60.0))
                        .named("Mesh weld tolerance")
                        .on_hover_text(
                            "Vertices closer than this merge (0: exact duplicates only). \
                             Changes geometry by up to this amount.",
                        );
                });
                ui.horizontal(|ui| {
                    ui.label("Fill holes up to (edges)");
                    ui.add(egui::TextEdit::singleline(&mut st.max_edges).desired_width(60.0))
                        .named("Mesh hole fill limit")
                        .on_hover_text("Planar boundary loops only; 0 = do not fill");
                });
                ui.checkbox(
                    &mut st.options.remove_degenerate,
                    "Remove degenerate triangles",
                )
                .named("Remove degenerate triangles");
                ui.checkbox(
                    &mut st.options.remove_duplicates,
                    "Remove duplicate triangles",
                )
                .named("Remove duplicate triangles");
                ui.checkbox(
                    &mut st.options.orient,
                    "Make orientation consistent (outward)",
                )
                .named("Orient triangles");
                let tol = st.tolerance.trim().parse::<f64>();
                let edges = st.max_edges.trim().parse::<u32>();
                let opts = match (tol, edges) {
                    (Ok(t), Ok(e)) => Some(RepairOptions {
                        weld_tolerance: t,
                        fill_holes_max_edges: e,
                        ..st.options
                    }),
                    _ => None,
                };
                ui.horizontal(|ui| {
                    for (label, preview) in
                        [("Preview repair", true), ("Repair as new body", false)]
                    {
                        if ui
                            .add_enabled(opts.is_some(), egui::Button::new(label))
                            .named(label)
                            .clicked()
                            && let Some(o) = opts
                        {
                            st.pending = Some((body, preview));
                            send = Some(Command::RepairMesh(RepairMeshArgs {
                                body,
                                options: o,
                                preview,
                                body_name: None,
                                hide_source: !preview,
                            }));
                        }
                    }
                });
                // Review run 7 #29: only for this body at this revision (an
                // applied repair is one revision older than the new body's
                // document state, so it is shown on the source until edited).
                if let Some((d, r, b, preview, rep)) = &st.report
                    && *d == doc
                    && *b == body
                    && *r == rev
                {
                    let t = rcad_commands::mesh_repair::summary(rep, *preview);
                    let colour = if rep.after.closed {
                        Color32::from_rgb(120, 200, 120)
                    } else {
                        Color32::from_rgb(255, 200, 120)
                    };
                    ui.label(RichText::new(t).small().color(colour))
                        .probe("Mesh repair report");
                    for n in &rep.notes {
                        ui.small(n);
                    }
                }
                ui.small(
                    "Repair copies the mesh; it does not remove self-intersections. A closed \
                     mesh is not by itself a valid printable solid.",
                );
            });
        header.header_response.probe("Mesh check / repair");
        if let Some(c) = send {
            self.request(c);
        }
    }
}
