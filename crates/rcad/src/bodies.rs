//! Bodies in the browser and the body inspector.
//!
//! The inspector also sets a body's physical material (catalog or a
//! user density, `SetBodyMaterial`) and shows its mass properties
//! (`MassProperties`: volume, mass, centre of mass, inertia; world frame,
//! mm / g / g·mm²; exact or a labelled estimate).
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
use rcad_commands::mass::MassReport;
use rcad_commands::{BodySummary, Command, DocSummary};
use rcad_document::material::{CATALOG, catalog_entry};
use rcad_document::{BodyId, BodySource, DensityUnit};
use rcad_kernel::VolumeResult;

use crate::app::RcadApp;
use crate::probe::Probe as _;

/// Edit buffers of the body Move / Copy form.
#[derive(Debug, Clone, PartialEq)]
pub struct MoveForm {
    pub t: [String; 3],
    pub angle: String,
    /// Rotation axis through the origin: 0 = X, 1 = Y, 2 = Z.
    pub axis: usize,
    pub copy: bool,
}

impl Default for MoveForm {
    fn default() -> Self {
        Self {
            t: ["0".into(), "0".into(), "0".into()],
            angle: "0".into(),
            axis: 2,
            copy: false,
        }
    }
}

/// Material form and the last mass-properties report.
#[derive(Debug, Clone, Default)]
pub struct BodyProps {
    /// Body the form was filled for (reset when another body is shown).
    pub body: Option<BodyId>,
    /// Catalog key, `CUSTOM` or `NONE`.
    pub choice: String,
    pub density: String,
    pub name: String,
    pub unit: DensityUnit,
    /// (doc_id, revision, report) of the last `MassProperties` reply.
    pub report: Option<(u64, u64, MassReport)>,
    /// Combine form: tool body, operation (0 join, 1 cut, 2 intersect),
    /// keep the tool.
    pub combine_tool: Option<BodyId>,
    pub combine_op: usize,
    pub keep_tools: bool,
}

const CUSTOM: &str = "custom";
const NONE: &str = "none";

impl BodyProps {
    /// Keep the report of a `MassProperties` reply (engine hook in `handle_reply`).
    pub(crate) fn on_reply(&mut self, response: &rcad_commands::Response, summary: &DocSummary) {
        if let Some(r) = &response.mass_properties {
            self.report = Some((summary.doc_id, summary.revision, r.clone()));
        }
    }

    fn reset_for(&mut self, b: &BodySummary) {
        self.body = Some(b.id);
        self.unit = DensityUnit::GPerCm3;
        match &b.material {
            Some(m) if m.catalog.is_some() => {
                self.choice = m.catalog.clone().unwrap_or_default();
                self.density = m.density.to_string();
                self.name.clear();
            }
            Some(m) => {
                self.choice = CUSTOM.into();
                self.density = m.density.to_string();
                self.name = m.name.clone();
            }
            None => {
                self.choice = NONE.into();
                self.density.clear();
                self.name.clear();
            }
        }
    }
}

fn choice_text(choice: &str) -> String {
    match choice {
        NONE => "None".into(),
        CUSTOM => "Custom density".into(),
        k => catalog_entry(k).map_or_else(
            || k.to_string(),
            |e| format!("{} ({} g/cm³)", e.name, e.density),
        ),
    }
}

/// A readout with a precision suited to the magnitude.
fn num(x: f64) -> String {
    if x != 0.0 && (x.abs() >= 1e6 || x.abs() < 1e-3) {
        format!("{x:.6e}")
    } else {
        format!("{x:.4}")
    }
}

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
        BodySource::RepairedMesh { source } => {
            format!("Repaired copy of mesh {source} — triangle mesh (mesh repair feature)")
        }
        BodySource::EditedMesh { source, operation } => {
            format!("Mesh {operation} of {source} — triangle mesh (mesh edit feature)")
        }
    }
}

impl RcadApp {
    /// "Bodies" group of the browser.
    pub(crate) fn bodies_browser(&mut self, ui: &mut egui::Ui) {
        let bodies = self.summary.bodies.clone();
        ui.label(RichText::new(format!("Bodies ({})", bodies.len())).strong());
        if bodies.is_empty() {
            ui.weak("No bodies yet. Extrude a sketch or File › Import….");
            // Empty components and external components can be made
            // in an empty document (an assembly file).
            self.components_browser(ui);
            return;
        }
        let mut select = None;
        let mut toggle = None;
        let mut start_rename = None;
        let mut commit_rename = None;
        let mut cancel_rename = false;
        // Some(Some(b)): isolate b; Some(None): show all again.
        let mut isolate: Option<Option<rcad_document::BodyId>> = None;
        if let Some(iso) = self.isolated
            && let Some(name) = bodies.iter().find(|b| b.id == iso).map(|b| b.name.clone())
        {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Isolated: {name}")).strong());
                if ui
                    .small_button("Show all")
                    .on_hover_text("End isolation: draw every visible body again")
                    .named("Show all bodies")
                    .clicked()
                {
                    isolate = Some(None);
                }
            });
        }
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
                        crate::shell::ribbon::menu_min_width(ui);
                        if ui.button("Rename body").probe("Rename body").clicked() {
                            start_rename = Some(b.id);
                            ui.close();
                        }
                        let t = if b.visible { "Hide body" } else { "Show body" };
                        if ui.button(t).clicked() {
                            toggle = Some((b.id, !b.visible));
                            ui.close();
                        }
                        if b.visible
                            && ui
                                .button("Isolate")
                                .on_hover_text("Show only this body (view only; not saved, not an undo step)")
                                .named("Isolate body")
                                .clicked()
                        {
                            isolate = Some(Some(b.id));
                            ui.close();
                        }
                    });
                }
                let (badge, col) = match (&b.source, b.kind.as_str()) {
                    (_, "mesh") => ("mesh", Color32::from_rgb(220, 190, 120)),
                    (_, "surface") => ("open surface", Color32::from_rgb(255, 200, 120)),
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
        if let Some(b) = isolate {
            self.set_isolation(b);
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
        self.components_browser(ui);
    }

    /// Inspector content for the selected body.
    pub(crate) fn body_inspector(&mut self, ui: &mut egui::Ui, id: BodyId) {
        let Some(b) = self.summary.bodies.iter().find(|b| b.id == id).cloned() else {
            return;
        };
        // Wrapping row (see the feature header in panels.rs).
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("{} (body)", b.name))
                    .strong()
                    .size(16.0),
            );
            if labelled(ui.small_button("Rename…"), "Rename body…".into())
                .probe("Rename body…")
                .clicked()
            {
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
        if b.kind != "mesh" {
            self.move_ui(ui, b.id);
            self.split_ui(ui, b.id);
            self.combine_ui(ui, b.id);
        }
        if b.kind == "mesh" {
            self.mesh_inspect_ui(ui, b.id);
            self.mesh_repair_ui(ui, b.id);
            self.mesh_edit_ui(ui, b.id);
            ui.colored_label(
                Color32::from_rgb(255, 200, 120),
                "mesh: exact operations unavailable (no sketching on its faces, no join/cut, no STEP export). Display, hide/show, mesh repair/edit, save and STL/3MF export only.",
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
                    VolumeResult::Value {
                        volume,
                        method: rcad_kernel::VolumeMethod::TessellationEstimate,
                        error_estimate,
                    } => {
                        ui.label(format!(
                            "Volume ≈ {volume:.3} mm³ (tessellation estimate, ±{error_estimate:.3})"
                        ));
                    }
                    VolumeResult::Value { volume, .. } => {
                        ui.label(format!("Volume {volume:.4} mm³"));
                    }
                    VolumeResult::Unavailable { reason } => {
                        ui.label(format!("Volume unavailable: {reason}"));
                    }
                }
                let (a, c) = (m.bounds.min, m.bounds.max);
                ui.label(format!(
                    "Overall size (bounding box) {:.3} × {:.3} × {:.3} mm",
                    c[0] - a[0],
                    c[1] - a[1],
                    c[2] - a[2]
                ))
                .on_hover_text("Extent along X, Y, Z. Not a wall or floor thickness: select two parallel faces to measure the distance between them.");
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
            (None, None) if b.measure_error.is_some() => {
                ui.colored_label(
                    egui::Color32::from_rgb(255, 170, 90),
                    format!(
                        "Measurement failed for this geometry (not retried until the body changes): {}",
                        b.measure_error.as_deref().unwrap_or_default()
                    ),
                );
                if let Some(bb) = b.bounds {
                    ui.small(format!(
                        "Overall size (bounding box) {:.3} × {:.3} × {:.3} mm",
                        bb.max[0] - bb.min[0],
                        bb.max[1] - bb.min[1],
                        bb.max[2] - bb.min[2]
                    ));
                }
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
        ui.separator();
        self.material_ui(ui, &b);
    }

    /// Material assignment and mass properties of body `b`.
    fn material_ui(&mut self, ui: &mut egui::Ui, b: &BodySummary) {
        if self.body_props.body != Some(b.id) {
            self.body_props.reset_for(b);
        }
        let estimate = matches!(
            b.measurements.as_ref().map(|m| &m.volume),
            Some(VolumeResult::Value {
                method: rcad_kernel::VolumeMethod::TessellationEstimate,
                ..
            })
        );
        match &b.material {
            Some(m) => {
                ui.label(format!("Material: {}", m.describe()))
                    .on_hover_text("Physical material for mass properties (not the display colour). Catalog densities are typical nominal values of the solid material; a printed part with infill weighs less.");
            }
            None => {
                ui.label("Material: none (no mass)");
            }
        }
        if let Some(mass) = b.mass {
            let t = if estimate {
                format!("Mass ≈ {} g (volume is an estimate)", num(mass))
            } else if b.kind == "mesh" {
                format!("Mass {} g (of the triangle mesh)", num(mass))
            } else {
                format!("Mass {} g", num(mass))
            };
            ui.label(t);
        }
        let mut send = None;
        ui.collapsing("Set material", |ui| {
            let f = &mut self.body_props;
            crate::form::combo(ui, "Material", choice_text(&f.choice), |ui| {
                ui.selectable_value(&mut f.choice, NONE.to_string(), choice_text(NONE));
                ui.selectable_value(&mut f.choice, CUSTOM.to_string(), choice_text(CUSTOM));
                for e in CATALOG {
                    ui.selectable_value(&mut f.choice, e.key.to_string(), choice_text(e.key));
                }
            });
            let custom = f.choice == CUSTOM;
            if custom {
                crate::panels::text_field(ui, "Material name", &mut f.name);
                ui.horizontal(|ui| {
                    crate::panels::num_field(ui, "Density", &mut f.density);
                    ui.radio_value(&mut f.unit, DensityUnit::GPerCm3, "g/cm³");
                    ui.radio_value(&mut f.unit, DensityUnit::KgPerM3, "kg/m³");
                });
            } else if let Some(e) = catalog_entry(&f.choice) {
                ui.small(format!(
                    "{} g/cm³, typical nominal (published values {}–{} g/cm³)",
                    e.density, e.range.0, e.range.1
                ));
            }
            let density = f
                .density
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|d| d.is_finite());
            let ready = !custom || density.is_some();
            let r = ui
                .add_enabled(ready, egui::Button::new("Apply material"))
                .probe("Apply material");
            if r.clicked() {
                send = Some(Command::SetBodyMaterial {
                    body: b.id,
                    material: (!custom && f.choice != NONE).then(|| f.choice.clone()),
                    density: if custom { density } else { None },
                    density_unit: f.unit,
                    name: (custom && !f.name.trim().is_empty()).then(|| f.name.clone()),
                });
            }
        });
        if let Some(c) = send {
            // Refill the form from the reply's state on a later frame.
            self.body_props.body = None;
            self.request(c);
        }
        if ui
            .button("Mass properties")
            .on_hover_text("Volume, mass, centre of mass and inertia of this body (world frame)")
            .probe("Mass properties")
            .clicked()
        {
            self.request(Command::MassProperties {
                occurrences: None,
                bodies: Some(vec![b.id]),
            });
        }
        let current = self
            .body_props
            .report
            .as_ref()
            .filter(|(d, r, _)| *d == self.summary.doc_id && *r == self.summary.revision);
        let Some(bm) = current.and_then(|(_, _, rep)| rep.bodies.iter().find(|x| x.body == b.id))
        else {
            return;
        };
        let Some(p) = &bm.properties else {
            ui.colored_label(
                Color32::from_rgb(255, 170, 90),
                format!(
                    "Mass properties unavailable: {}",
                    bm.error.as_deref().unwrap_or("unknown")
                ),
            );
            return;
        };
        ui.label(RichText::new("Mass properties (world frame; mm, g, g·mm²)").strong());
        if bm.exact {
            ui.small(format!("Exact boundary integral ({:?})", p.method));
        } else {
            ui.colored_label(
                Color32::from_rgb(255, 200, 120),
                format!(
                    "Estimate ({:?}): volume ±{} mm³, centre ±{} mm",
                    p.method,
                    num(p.volume_error),
                    num(p.centroid_error)
                ),
            );
        }
        ui.label(format!("Volume {} mm³", num(p.volume)));
        match bm.mass {
            Some(m) => ui.label(format!("Mass {} g", num(m))),
            None => ui.label("Mass: assign a material"),
        };
        let c = p.centroid;
        ui.label(format!(
            "Centre of mass ({}, {}, {}) mm",
            num(c[0]),
            num(c[1]),
            num(c[2])
        ));
        if let Some(i) = bm.inertia {
            ui.small("Inertia about the centre of mass, world axes (g·mm²):");
            for row in i {
                ui.small(format!(
                    "[{}  {}  {}]",
                    num(row[0]),
                    num(row[1]),
                    num(row[2])
                ));
            }
        }
    }
}

impl RcadApp {
    /// Combine this body with another one (`Command::Combine`).
    fn combine_ui(&mut self, ui: &mut egui::Ui, target: BodyId) {
        let others: Vec<(BodyId, String)> = self
            .summary
            .bodies
            .iter()
            .filter(|o| o.id != target && o.kind != "mesh")
            .map(|o| (o.id, o.name.clone()))
            .collect();
        let mut send = None;
        egui::CollapsingHeader::new("Combine with…")
            .open(self.shell.take_focus(crate::shell::Focus::Combine))
            .show(ui, |ui| {
                if others.is_empty() {
                    ui.weak("No other solid body to combine with.");
                    return;
                }
                let f = &mut self.body_props;
                if !f
                    .combine_tool
                    .is_some_and(|t| others.iter().any(|o| o.0 == t))
                {
                    f.combine_tool = Some(others[0].0);
                }
                let name = |id: Option<BodyId>| {
                    others
                        .iter()
                        .find(|o| Some(o.0) == id)
                        .map_or(String::new(), |o| o.1.clone())
                };
                crate::form::combo(ui, "Tool body", name(f.combine_tool), |ui| {
                    for (id, n) in &others {
                        ui.selectable_value(&mut f.combine_tool, Some(*id), n);
                    }
                });
                ui.horizontal(|ui| {
                    ui.radio_value(&mut f.combine_op, 0, "Join");
                    ui.radio_value(&mut f.combine_op, 1, "Cut");
                    ui.radio_value(&mut f.combine_op, 2, "Intersect");
                });
                ui.checkbox(&mut f.keep_tools, "Keep tool body")
                .on_hover_text(
                    "Off: the tool body is consumed (it disappears; its features stay in History)",
                );
                if ui.button("Combine").probe("Combine").clicked()
                    && let Some(t) = f.combine_tool
                {
                    send = Some(Command::Combine {
                        name: None,
                        target,
                        tools: vec![t],
                        op: match f.combine_op {
                            0 => rcad_kernel::CombineOp::Join,
                            1 => rcad_kernel::CombineOp::Cut,
                            _ => rcad_kernel::CombineOp::Intersect,
                        },
                        keep_tools: f.keep_tools,
                    });
                }
                ui.small(
                    "Checked independently against both bodies; refused (nothing changes) when \
                 Truck fails or the check disagrees. Bodies that only touch usually fail.",
                );
            });
        if let Some(c) = send {
            self.request(c);
        }
    }

    /// Move / Copy a body: rotation about a world axis through the
    /// origin, then a translation; one undo step; a feature in History whose
    /// values stay editable parameters (tx, ty, tz, angle_deg).
    fn move_ui(&mut self, ui: &mut egui::Ui, body: rcad_document::BodyId) {
        let mut send = None;
        let header = egui::CollapsingHeader::new("Move / Copy")
            .open(self.shell.take_focus(crate::shell::Focus::MoveCopy))
            .show(ui, |ui| {
                let f = &mut self.move_form;
                ui.horizontal(|ui| {
                    for (i, l) in ["ΔX", "ΔY", "ΔZ"].iter().enumerate() {
                        crate::panels::num_field(ui, &format!("{l} (mm)"), &mut f.t[i]);
                    }
                });
                ui.horizontal(|ui| {
                    crate::panels::num_field(ui, "Rotate (deg)", &mut f.angle);
                    ui.label("about");
                    ui.radio_value(&mut f.axis, 0, "X");
                    ui.radio_value(&mut f.axis, 1, "Y");
                    ui.radio_value(&mut f.axis, 2, "Z");
                });
                ui.checkbox(
                    &mut f.copy,
                    "Copy (keep the original; the copy is independent)",
                )
                .probe("Copy body");
                let parse = |t: &str| t.trim().parse::<f64>().ok().filter(|v| v.is_finite());
                let vals = (
                    parse(&f.t[0]),
                    parse(&f.t[1]),
                    parse(&f.t[2]),
                    parse(&f.angle),
                );
                let label = if f.copy { "Copy body" } else { "Move body" };
                if let (Some(x), Some(y), Some(z), Some(a)) = vals {
                    if ui.button(label).probe(label).clicked() {
                        let mut axis = [0.0; 3];
                        axis[f.axis] = 1.0;
                        send = Some(Command::MoveBody {
                            name: None,
                            body,
                            translate: [x, y, z],
                            origin: [0.0; 3],
                            axis: Some(axis),
                            angle_deg: a,
                            copy: f.copy,
                            body_name: None,
                        });
                    }
                } else {
                    ui.add_enabled(false, egui::Button::new(label));
                }
                ui.small(
                    "Faces made before the move keep their references (sketches, fillets, holes \
                 follow the body). Patterns/mirrors of features made before the move are refused.",
                );
            });
        header.header_response.probe("Move / Copy");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.scale_factor).desired_width(50.0))
                .named("Scale factor");
            if ui.button("Scale body").probe("Scale body").clicked()
                && let Ok(f) = self.scale_factor.trim().parse::<f64>()
            {
                send = Some(Command::ScaleBody {
                    name: None,
                    body,
                    center: [0.0; 3],
                    factor: f,
                });
            }
        })
        .response
        .on_hover_text("Uniform scale about the world origin, in place (one Undo step; the factor stays a parameter)");
        if let Some(c) = send {
            self.request(c);
        }
    }
}

impl RcadApp {
    /// Split a body by a world plane moved along its normal. The body
    /// keeps the part behind the plane; a new body gets the part in front.
    fn split_ui(&mut self, ui: &mut egui::Ui, body: rcad_document::BodyId) {
        let mut send = None;
        let header = egui::CollapsingHeader::new("Split by plane")
            .id_salt(("split_ui", body.0))
            .open(self.shell.take_focus(crate::shell::Focus::Split))
            .show(ui, |ui| {
                let (base, offset) = &mut self.split_form;
                ui.horizontal(|ui| {
                    ui.radio_value(base, 0, "World XY").probe("Split XY");
                    ui.radio_value(base, 1, "World XZ").probe("Split XZ");
                    ui.radio_value(base, 2, "World YZ").probe("Split YZ");
                });
                crate::panels::num_field(ui, "Split plane offset (mm)", offset);
                let off = offset.trim().parse::<f64>().ok().filter(|v| v.is_finite());
                if ui
                    .add_enabled(off.is_some(), egui::Button::new("Split body"))
                    .probe("Split body")
                    .clicked()
                    && let Some(d) = off
                {
                    let base = match *base {
                        0 => rcad_document::PlaneBase::WorldXY,
                        1 => rcad_document::PlaneBase::WorldXZ,
                        _ => rcad_document::PlaneBase::WorldYZ,
                    };
                    send = Some(Command::SplitBody {
                        name: None,
                        body,
                        plane: rcad_document::SketchPlane::Offset {
                            base,
                            distance: d,
                            angle: 0.0,
                        },
                        body_name: None,
                    });
                }
                ui.small(
                    "Offset along the plane's normal (+Z, −Y, +X). This body keeps the part \
                     behind the plane; a new body gets the part in front. The offset stays a \
                     parameter (plane_offset).",
                );
            });
        header.header_response.probe("Split by plane");
        if let Some(c) = send {
            self.request(c);
        }
    }
}
