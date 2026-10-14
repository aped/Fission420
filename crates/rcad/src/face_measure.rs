//! Exact face measurement in the face inspector: cylinder
//! radius/diameter, axis-to-axis, plane-to-axis and plane-to-plane distance or
//! angle from the B-rep surfaces (`Command::MeasureFaces`). World frame; mm and
//! degrees. The readout is shown only while it describes the current document
//! revision, so it always follows regeneration.
use eframe::egui::{self, RichText};
use rcad_commands::measure_faces::{FaceMeasureReport, FacePick};
use rcad_commands::{Command, DocSummary, Response};
use rcad_kernel::{DirectFace, FacePairMeasure};

use crate::app::RcadApp;
use crate::probe::Probe as _;

/// "Measure from" face and the last report.
#[derive(Debug, Clone, Default)]
pub struct ExactMeasure {
    pub from: Option<(FacePick, String)>,
    /// (doc_id, revision, a, b, report).
    pub report: Option<(u64, u64, FacePick, Option<FacePick>, FaceMeasureReport)>,
    /// Picks of the request in flight (the reply carries only the report).
    pending: Option<(FacePick, Option<FacePick>)>,
}

impl ExactMeasure {
    /// Engine hook in `handle_reply`.
    pub(crate) fn on_reply(&mut self, response: &Response, summary: &DocSummary) {
        if response.command != "measure_faces" {
            return;
        }
        let pending = self.pending.take();
        if let (Some(r), Some((a, b))) = (&response.face_measure, pending) {
            self.report = Some((summary.doc_id, summary.revision, a, b, r.clone()));
        }
    }
}

fn mm(x: f64) -> String {
    crate::params_ui::fmt_value(x)
}

/// One-line text of a face's exact geometry.
pub fn describe(f: &DirectFace) -> String {
    match f {
        DirectFace::Plane { normal: n, .. } => {
            format!("Plane, normal ({:.4}, {:.4}, {:.4})", n[0], n[1], n[2])
        }
        DirectFace::Cylinder {
            radius,
            hole,
            axis: a,
            ..
        } => format!(
            "Cylinder ({}): radius {} mm, diameter {} mm, axis ({:.4}, {:.4}, {:.4})",
            if *hole { "hole" } else { "boss" },
            mm(*radius),
            mm(2.0 * radius),
            a[0],
            a[1],
            a[2]
        ),
        DirectFace::Other => "Not a plane or circular cylinder".into(),
    }
}

/// One-line text of a pair measurement.
pub fn pair_text(p: &FacePairMeasure) -> String {
    match p {
        FacePairMeasure::PlaneDistance { distance } => {
            format!("Distance between parallel planes: {} mm", mm(*distance))
        }
        FacePairMeasure::PlaneAngle { degrees } => {
            format!("Angle between the planes' normals: {}°", mm(*degrees))
        }
        FacePairMeasure::AxisDistance { distance } => {
            format!("Distance between parallel axes: {} mm", mm(*distance))
        }
        FacePairMeasure::AxisAngle { degrees, distance } => format!(
            "Angle between the axes: {}°, closest distance {} mm",
            mm(*degrees),
            mm(*distance)
        ),
        FacePairMeasure::PlaneAxisDistance { distance } => {
            format!("Distance from the plane to the axis: {} mm", mm(*distance))
        }
        FacePairMeasure::PlaneAxisAngle { degrees } => {
            format!("Angle between the axis and the plane: {}°", mm(*degrees))
        }
    }
}

impl RcadApp {
    fn face_pick(&self, group: u32) -> Option<FacePick> {
        let db = self.scene.body_of(group)?;
        if db.mesh_body {
            return None;
        }
        Some(FacePick {
            body: db.id,
            face: group - db.groups.start,
        })
    }

    fn measure_faces(&mut self, a: FacePick, b: Option<FacePick>) {
        self.exact_measure.pending = Some((a, b));
        self.request(Command::MeasureFaces { a, b });
    }

    /// Exact measurement section of the face inspector.
    pub(crate) fn exact_face_ui(&mut self, ui: &mut egui::Ui, info: &crate::view3d::FaceInfo) {
        let Some(pick) = self.face_pick(info.group) else {
            return;
        };
        ui.separator();
        ui.label(RichText::new("Exact face measurement (world; mm, °)").strong());
        let mut send = None;
        ui.horizontal(|ui| {
            if ui
                .small_button("Exact geometry")
                .on_hover_text("Plane or cylinder of this face from the B-rep (radius, diameter, axis)")
                .probe("Exact geometry")
                .clicked()
            {
                send = Some((pick, None));
            }
            match self.exact_measure.from.clone() {
                Some((from, _)) if from != pick => {
                    if ui
                        .small_button("Measure to this face")
                        .probe("Measure to this face")
                        .clicked()
                    {
                        send = Some((from, Some(pick)));
                    }
                }
                Some(_) => {}
                None => {
                    if ui
                        .small_button("Exact measure from here")
                        .on_hover_text("Then select a second plane or cylinder: distance between planes, axes, plane and axis, or their angle")
                        .probe("Exact measure from here")
                        .clicked()
                    {
                        self.exact_measure.from = Some((pick, info.description.clone()));
                    }
                }
            }
            if self.exact_measure.from.is_some()
                && ui.small_button("Clear").probe("Clear exact measure").clicked()
            {
                self.exact_measure.from = None;
            }
        });
        if let Some((_, d)) = &self.exact_measure.from {
            ui.small(format!("from: {d}"));
        }
        if let Some((a, b)) = send {
            self.measure_faces(a, b);
        }
        let current = self
            .exact_measure
            .report
            .as_ref()
            .filter(|(d, r, a, b, _)| {
                *d == self.summary.doc_id
                    && *r == self.summary.revision
                    && (*a == pick || *b == Some(pick))
            });
        if let Some((_, _, _, _, rep)) = current {
            ui.label(describe(&rep.a));
            if let Some(a) = rep.area {
                ui.label(format!("Area {} mm² (exact)", mm(a)))
                    .on_hover_text(
                        "Planar face area from the B-rep boundary (lines and arcs exact)",
                    );
            }
            if let Some(b) = &rep.b {
                ui.label(describe(b));
            }
            if let Some(p) = &rep.pair {
                ui.label(RichText::new(pair_text(p)).strong());
            }
        }
    }
}
