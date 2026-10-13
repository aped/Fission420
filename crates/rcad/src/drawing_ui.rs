//! Browser "Drawings" section (run 6, docs/DRAWINGS.md): create a drawing
//! from the selected body (Front base view + the views from above and from
//! the right), add an iso view, switch the projection convention, export to
//! PDF/SVG and read the last export's report (flagged views and dangling
//! callouts in red). Everything goes through the shared commands.
//! Run 7: assembly drawings (placed occurrences, BOM table, balloons),
//! driven dimensions between semantic references with tolerances, notes.
use crate::app::RcadApp;
use crate::probe::Probe as _;
use eframe::egui::{self, Color32, RichText};
use rcad_commands::drawings::{DrawingReport, ViewSpec};
use rcad_commands::{Command, DocSummary, Response};
use rcad_document::drawing::{
    Annotation, DimRef, Drawing, DrawingId, Orientation, Projection, Tolerance, ViewId, ViewKind,
};

/// Drawings section state.
#[derive(Debug, Clone)]
pub struct DrawingUi {
    /// Export path typed by the user (format from the extension).
    pub path: String,
    /// (doc_id, revision, report) of the last export.
    pub report: Option<(u64, u64, DrawingReport)>,
    /// Run 7 annotation editor: target view, references, tolerance, note text.
    pub dim_view: Option<ViewId>,
    pub dim_a: Option<DimRef>,
    pub dim_b: Option<DimRef>,
    /// 0 none, 1 ±, 2 deviation, 3 limits.
    pub tol_kind: usize,
    pub tol_upper: f64,
    pub tol_lower: f64,
    pub note: String,
}

impl Default for DrawingUi {
    fn default() -> Self {
        Self {
            path: "drawing.pdf".into(),
            report: None,
            dim_view: None,
            dim_a: None,
            dim_b: None,
            tol_kind: 0,
            tol_upper: 0.1,
            tol_lower: -0.1,
            note: String::new(),
        }
    }
}

impl DrawingUi {
    /// The tolerance the editor describes.
    pub fn tolerance(&self) -> Tolerance {
        match self.tol_kind {
            1 => Tolerance::Symmetric {
                value: self.tol_upper,
            },
            2 => Tolerance::Deviation {
                upper: self.tol_upper,
                lower: self.tol_lower,
            },
            3 => Tolerance::Limits {
                upper: self.tol_upper,
                lower: self.tol_lower,
            },
            _ => Tolerance::None,
        }
    }
}

const TOL_KINDS: [&str; 4] = ["No tolerance", "± symmetric", "+/- deviation", "Limits"];

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
                        assembly: false,
                    });
                }
                let occurrences = !self.summary.assembly.occurrences.is_empty();
                if ui
                    .add_enabled(occurrences, egui::Button::new("New assembly drawing"))
                    .named("New assembly drawing")
                    .on_hover_text(
                        "Every placed component occurrence in a Front view with the views from \
                         above and from the right, balloons and a BOM table (flattened \
                         quantities)",
                    )
                    .clicked()
                {
                    send.push(Command::CreateDrawing {
                        name: None,
                        sheet: None,
                        projection: None,
                        title: None,
                        part_number: None,
                        material: None,
                        bodies: vec![],
                        standard_views: None,
                        assembly: true,
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
                                    assembly: false,
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
                            ViewKind::Detail { label, scale, .. } => format!(
                                "{} detail {label} {}",
                                v.id,
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
                        for (i, a) in v.annotations.iter().enumerate() {
                            let t = annotation_label(a, &self.summary);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!("  {t}")).small());
                                if ui
                                    .small_button("×")
                                    .named(&format!("Remove annotation {i} of {} {}", d.name, v.id))
                                    .clicked()
                                {
                                    send.push(Command::RemoveDrawingAnnotation {
                                        drawing: d.id,
                                        view: v.id,
                                        index: i,
                                    });
                                }
                            });
                        }
                    }
                    self.annotate_ui(ui, d, &mut send);
                    self.report_ui(ui, d.id);
                }
            });
        header.header_response.probe("Drawings header");
        for c in send {
            self.request(c);
        }
    }

    /// BOM/balloon toggles, the dimension editor and notes of drawing `d`.
    fn annotate_ui(&mut self, ui: &mut egui::Ui, d: &Drawing, send: &mut Vec<Command>) {
        let first_base = d
            .views
            .iter()
            .find(|v| matches!(v.kind, ViewKind::Base { .. }))
            .map(|v| (v.id, v.annotations.contains(&Annotation::Balloons)));
        let assembly = d
            .views
            .iter()
            .any(|v| matches!(v.kind, ViewKind::Base { assembly: true, .. }));
        ui.horizontal_wrapped(|ui| {
            let mut bom = d.bom.is_some();
            if ui
                .checkbox(&mut bom, "BOM table")
                .named(&format!("BOM table of {}", d.name))
                .on_hover_text("Flattened quantities of the evaluated assembly")
                .changed()
            {
                send.push(Command::SetBomTable {
                    drawing: d.id,
                    show: bom,
                    position: None,
                });
            }
            if let Some((vid, has)) = first_base
                && assembly
            {
                let mut b = has;
                if ui
                    .checkbox(&mut b, "Balloons")
                    .named(&format!("Balloons of {}", d.name))
                    .changed()
                {
                    send.push(Command::SetBalloons {
                        drawing: d.id,
                        view: vid,
                        show: b,
                    });
                }
            }
        });
        egui::CollapsingHeader::new("Dimension / note")
            .id_salt(("drawing_annotate", d.id.0))
            .show(ui, |ui| {
                let refs = references(&self.summary);
                let st = &mut self.drawing_ui;
                let views: Vec<ViewId> = d
                    .views
                    .iter()
                    .filter(|v| !matches!(v.kind, ViewKind::Detail { .. }))
                    .map(|v| v.id)
                    .collect();
                if st.dim_view.is_none_or(|v| !views.contains(&v)) {
                    st.dim_view = views.first().copied();
                }
                egui::ComboBox::from_label("Dimension view")
                    .selected_text(st.dim_view.map(|v| v.to_string()).unwrap_or_default())
                    .show_ui(ui, |ui| {
                        for v in &views {
                            ui.selectable_value(&mut st.dim_view, Some(*v), v.to_string())
                                .probe(&format!("Dimension view option {v}"));
                        }
                    })
                    .response
                    .probe("Dimension view");
                let name = |r: &Option<DimRef>| {
                    r.and_then(|r| refs.iter().find(|(x, _)| *x == r))
                        .map(|(_, n)| n.clone())
                        .unwrap_or_else(|| "(choose)".into())
                };
                egui::ComboBox::from_label("Dimension from")
                    .selected_text(name(&st.dim_a))
                    .show_ui(ui, |ui| {
                        for (r, n) in &refs {
                            ui.selectable_value(&mut st.dim_a, Some(*r), n)
                                .probe(&format!("Dimension from option {n}"));
                        }
                    })
                    .response
                    .probe("Dimension from");
                let b_text = if st.dim_b.is_none() {
                    "(none: hole diameter)".to_string()
                } else {
                    name(&st.dim_b)
                };
                egui::ComboBox::from_label("Dimension to")
                    .selected_text(b_text)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut st.dim_b, None, "(none: hole diameter)")
                            .probe("Dimension to option none");
                        for (r, n) in &refs {
                            ui.selectable_value(&mut st.dim_b, Some(*r), n)
                                .probe(&format!("Dimension to option {n}"));
                        }
                    })
                    .response
                    .probe("Dimension to");
                egui::ComboBox::from_label("Dimension tolerance")
                    .selected_text(TOL_KINDS[st.tol_kind.min(3)])
                    .show_ui(ui, |ui| {
                        for (i, k) in TOL_KINDS.iter().enumerate() {
                            ui.selectable_value(&mut st.tol_kind, i, *k)
                                .probe(&format!("Dimension tolerance option {k}"));
                        }
                    })
                    .response
                    .probe("Dimension tolerance");
                if st.tol_kind > 0 {
                    ui.horizontal(|ui| {
                        ui.label(if st.tol_kind == 1 { "±" } else { "Upper" });
                        ui.add(
                            egui::DragValue::new(&mut st.tol_upper)
                                .speed(0.01)
                                .max_decimals(3),
                        )
                        .named("Tolerance upper");
                        if st.tol_kind > 1 {
                            ui.label("Lower");
                            ui.add(
                                egui::DragValue::new(&mut st.tol_lower)
                                    .speed(0.01)
                                    .max_decimals(3),
                            )
                            .named("Tolerance lower");
                        }
                    });
                }
                if ui
                    .add_enabled(
                        st.dim_view.is_some() && st.dim_a.is_some(),
                        egui::Button::new("Add dimension"),
                    )
                    .named(&format!("Add dimension to {}", d.name))
                    .on_hover_text(
                        "Driven by the model: measured at every export; shown as dangling \
                         (red, no value) when a reference stops resolving",
                    )
                    .clicked()
                    && let (Some(view), Some(a)) = (st.dim_view, st.dim_a)
                {
                    send.push(Command::AddDrawingDimension {
                        drawing: d.id,
                        view,
                        a,
                        b: st.dim_b,
                        tolerance: st.tolerance(),
                    });
                }
                ui.separator();
                ui.add(
                    egui::TextEdit::multiline(&mut st.note)
                        .desired_rows(2)
                        .desired_width(220.0),
                )
                .named("Drawing note text");
                if ui
                    .add_enabled(!st.note.trim().is_empty(), egui::Button::new("Add note"))
                    .named(&format!("Add note to {}", d.name))
                    .on_hover_text("Placed at the lower left of the sheet, above the border")
                    .clicked()
                {
                    let y = 20.0 + 12.0 * d.notes.len() as f64;
                    send.push(Command::AddDrawingNote {
                        drawing: d.id,
                        text: std::mem::take(&mut st.note),
                        position: [15.0, y.min(d.sheet.mm().1 - 15.0)],
                    });
                }
                for (i, n) in d.notes.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(n.text.replace('\n', " / ")).small());
                        if ui
                            .small_button("×")
                            .named(&format!("Remove note {i} of {}", d.name))
                            .clicked()
                        {
                            send.push(Command::RemoveDrawingNote {
                                drawing: d.id,
                                index: i,
                            });
                        }
                    });
                }
            });
    }

    /// Issue count of the last drawing export if it belongs to the current
    /// document revision (live-input probe).
    pub fn last_drawing_issues(&self) -> Option<usize> {
        self.drawing_ui
            .report
            .as_ref()
            .filter(|(d, r, _)| *d == self.summary.doc_id && *r == self.summary.revision)
            .map(|(_, _, r)| r.issues.len())
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

/// Dimension references offered by the editor: every referenceable planar
/// face of the model and every hole axis.
fn references(summary: &DocSummary) -> Vec<(DimRef, String)> {
    let mut out = Vec::new();
    for f in &summary.features {
        for fs in &f.faces {
            out.push((
                DimRef::Face(fs.reference),
                format!(
                    "{} {}: {}",
                    f.id,
                    f.name,
                    crate::face_role_label(&fs.reference.role)
                ),
            ));
        }
        if f.kind == "hole" {
            out.push((
                DimRef::Hole(f.id),
                format!("{} {}: hole axis", f.id, f.name),
            ));
        }
    }
    out
}

fn ref_label(r: &DimRef, summary: &DocSummary) -> String {
    match r {
        DimRef::Face(fr) => format!("{} {}", fr.feature, crate::face_role_label(&fr.role)),
        DimRef::Hole(f) => {
            let n = summary
                .features
                .iter()
                .find(|x| x.id == *f)
                .map(|x| x.name.clone())
                .unwrap_or_else(|| "missing".into());
            format!("{f} {n} axis")
        }
    }
}

/// One line describing an annotation in the browser.
fn annotation_label(a: &Annotation, summary: &DocSummary) -> String {
    match a {
        Annotation::HoleCallout { feature } => format!("callout of {feature}"),
        Annotation::Balloons => "balloons".into(),
        Annotation::Dimension { a, b, tolerance } => {
            let tol = match *tolerance {
                Tolerance::None => String::new(),
                Tolerance::Symmetric { value } => format!(" [±{value}]"),
                Tolerance::Deviation { upper, lower } => format!(" [{upper:+}/{lower:+}]"),
                Tolerance::Limits { upper, lower } => format!(" [limits {upper:+}/{lower:+}]"),
            };
            match b {
                Some(b) => format!(
                    "distance {} – {}{tol}",
                    ref_label(a, summary),
                    ref_label(b, summary)
                ),
                None => format!("diameter of {}{tol}", ref_label(a, summary)),
            }
        }
    }
}
