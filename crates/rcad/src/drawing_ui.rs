//! Browser "Drawings" section (run 6, docs/DRAWINGS.md): create a drawing
//! from the selected body (Front base view + the views from above and from
//! the right), add an iso view, switch the projection convention, export to
//! PDF/SVG and read the last export's report (flagged views and dangling
//! callouts in red). Everything goes through the shared commands.
use crate::app::RcadApp;
use crate::probe::Probe as _;
use eframe::egui::{self, Color32, RichText};
use rcad_commands::drawings::{DrawingReport, ViewSpec};
use rcad_commands::{Command, DocSummary, Response};
use rcad_document::drawing::{DrawingId, Orientation, Projection, ViewKind};

/// Drawings section state.
#[derive(Debug, Clone)]
pub struct DrawingUi {
    /// Export path typed by the user (format from the extension).
    pub path: String,
    /// (doc_id, revision, report) of the last export.
    pub report: Option<(u64, u64, DrawingReport)>,
}

impl Default for DrawingUi {
    fn default() -> Self {
        Self {
            path: "drawing.pdf".into(),
            report: None,
        }
    }
}

impl DrawingUi {
    /// Keep the report of an `ExportDrawing` reply (engine hook in `handle_reply`).
    pub(crate) fn on_reply(&mut self, response: &Response, summary: &DocSummary) {
        if let Some(r) = &response.drawing_report {
            self.report = Some((summary.doc_id, summary.revision, r.clone()));
        }
    }
}

impl RcadApp {
    /// The Browser's Drawings section.
    pub(crate) fn drawings_panel(&mut self, ui: &mut egui::Ui) {
        let drawings = self.summary.drawings.clone();
        let mut send: Vec<Command> = Vec::new();
        let header = egui::CollapsingHeader::new(format!("Drawings ({})", drawings.len()))
            .id_salt("drawings_panel")
            .show(ui, |ui| {
                let body = self
                    .selected_body
                    .and_then(|b| self.summary.bodies.iter().find(|x| x.id == b))
                    .filter(|b| b.kind == "brep")
                    .map(|b| (b.id, b.name.clone()));
                let label = match &body {
                    Some((_, n)) => format!("New drawing of {n}"),
                    None => "New drawing (select a body)".into(),
                };
                if ui
                    .add_enabled(body.is_some(), egui::Button::new(label))
                    .named("New drawing of selected body")
                    .on_hover_text(
                        "Front view at a standard scale that fits, plus the views from above \
                         and from the right (third angle)",
                    )
                    .clicked()
                    && let Some((b, _)) = body
                {
                    send.push(Command::CreateDrawing {
                        name: None,
                        sheet: None,
                        projection: None,
                        title: None,
                        part_number: None,
                        material: None,
                        bodies: vec![b],
                        standard_views: None,
                    });
                }
                ui.horizontal(|ui| {
                    ui.label("Export to");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.drawing_ui.path).desired_width(160.0),
                    )
                    .named("Drawing export path");
                });
                for d in &drawings {
                    ui.separator();
                    ui.label(RichText::new(&d.name).strong())
                        .on_hover_text(format!("{}; {} view(s)", d.id, d.views.len()));
                    ui.horizontal_wrapped(|ui| {
                        let first = d.projection == Projection::FirstAngle;
                        let toggle = if first { "First angle" } else { "Third angle" };
                        if ui
                            .small_button(toggle)
                            .named(&format!("Projection of {}", d.name))
                            .on_hover_text("Switch first/third angle projection")
                            .clicked()
                        {
                            send.push(edit_projection(
                                d.id,
                                if first {
                                    Projection::ThirdAngle
                                } else {
                                    Projection::FirstAngle
                                },
                            ));
                        }
                        let base = d.views.iter().find_map(|v| match &v.kind {
                            ViewKind::Base { bodies, .. } => Some(bodies.clone()),
                            _ => None,
                        });
                        let has_iso = d.views.iter().any(|v| {
                            matches!(
                                v.kind,
                                ViewKind::Base {
                                    orientation: Orientation::Iso,
                                    ..
                                }
                            )
                        });
                        if let Some(bodies) = base
                            && !has_iso
                            && ui
                                .small_button("+ Iso")
                                .named(&format!("Add iso view to {}", d.name))
                                .clicked()
                        {
                            let (w, h) = d.sheet.mm();
                            send.push(Command::AddDrawingView {
                                drawing: d.id,
                                view: ViewSpec::Base {
                                    bodies,
                                    orientation: Orientation::Iso,
                                    scale: None,
                                    position: Some([w * 0.75, h * 0.68]),
                                },
                            });
                        }
                        // Section through the middle of the first base view
                        // (vertical cutting line), seen from the right.
                        let first_base = d.views.iter().find_map(|v| match &v.kind {
                            ViewKind::Base {
                                bodies,
                                orientation,
                                ..
                            } => Some((v.id, bodies.clone(), *orientation)),
                            _ => None,
                        });
                        if let Some((vid, bodies, o)) = first_base
                            && o != Orientation::Iso
                            && ui
                                .small_button("+ Section")
                                .named(&format!("Add section to {}", d.name))
                                .on_hover_text(
                                    "Full section through the middle of the base view, \
                                     seen from the right",
                                )
                                .clicked()
                        {
                            let right = o.frame().right;
                            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
                            for b in self
                                .summary
                                .bodies
                                .iter()
                                .filter(|b| bodies.contains(&b.id))
                            {
                                if let Some(bb) = &b.bounds {
                                    for i in 0..8 {
                                        let p = [
                                            if i & 1 == 0 { bb.min[0] } else { bb.max[0] },
                                            if i & 2 == 0 { bb.min[1] } else { bb.max[1] },
                                            if i & 4 == 0 { bb.min[2] } else { bb.max[2] },
                                        ];
                                        let t = p[0] * right[0] + p[1] * right[1] + p[2] * right[2];
                                        lo = lo.min(t);
                                        hi = hi.max(t);
                                    }
                                }
                            }
                            let used: Vec<String> = d
                                .views
                                .iter()
                                .filter_map(|v| match &v.kind {
                                    ViewKind::Section { label, .. } => Some(label.clone()),
                                    _ => None,
                                })
                                .collect();
                            let label = ('A'..='Z')
                                .map(|c| c.to_string())
                                .find(|c| !used.contains(c))
                                .unwrap_or_else(|| "S".into());
                            if lo.is_finite() {
                                send.push(Command::AddDrawingView {
                                    drawing: d.id,
                                    view: ViewSpec::Section {
                                        parent: vid,
                                        from: rcad_document::drawing::Side::Right,
                                        offset: (lo + hi) / 2.0,
                                        label,
                                    },
                                });
                            }
                        }
                        if ui
                            .small_button("Export")
                            .named(&format!("Export {}", d.name))
                            .clicked()
                        {
                            send.push(Command::ExportDrawing {
                                drawing: d.id,
                                path: self.drawing_ui.path.clone().into(),
                                format: None,
                            });
                        }
                        if ui
                            .small_button("Delete")
                            .named(&format!("Delete drawing {}", d.name))
                            .clicked()
                        {
                            send.push(Command::DeleteDrawing { drawing: d.id });
                        }
                    });
                    for v in &d.views {
                        let text = match &v.kind {
                            ViewKind::Base {
                                orientation, scale, ..
                            } => format!(
                                "{} {} {}",
                                v.id,
                                orientation.label(),
                                rcad_document::drawing::fmt_scale(*scale)
                            ),
                            ViewKind::Section { label, .. } => {
                                format!("{} section {label}-{label}", v.id)
                            }
                            ViewKind::Projected { parent, from } => {
                                format!("{} {parent} {}", v.id, from.label())
                            }
                        };
                        ui.label(RichText::new(text).small());
                    }
                    self.report_ui(ui, d.id);
                }
            });
        header.header_response.probe("Drawings header");
        for c in send {
            self.request(c);
        }
    }

    /// The last export report of drawing `id`, if it is for the current revision.
    fn report_ui(&self, ui: &mut egui::Ui, id: DrawingId) {
        let Some((doc, rev, r)) = &self.drawing_ui.report else {
            return;
        };
        if *doc != self.summary.doc_id || *rev != self.summary.revision || r.drawing != id {
            return;
        }
        if r.issues.is_empty() {
            ui.label(
                RichText::new(format!("Exported {} bytes; all references valid", r.bytes)).small(),
            )
            .probe("Drawing export ok");
        }
        for i in &r.issues {
            ui.label(
                RichText::new(i)
                    .small()
                    .color(Color32::from_rgb(200, 40, 40)),
            )
            .probe("Drawing issue");
        }
    }
}

fn edit_projection(drawing: DrawingId, p: Projection) -> Command {
    Command::EditDrawing {
        drawing,
        name: None,
        sheet: None,
        projection: Some(p),
        title: None,
        part_number: None,
        material: None,
        auto_dimensions: None,
    }
}
