//! Import and export dialogs (STEP, STL, DXF).
//!
//! Import: the format is chosen by file extension; the units choice is shown
//! when needed (STL always: it has no units; DXF/STEP only after the file was
//! found to declare none, the command then fails with "declares no length unit;
//! choose one" and the dialog asks). The command runs on the engine worker (off
//! the UI thread) as one undoable transaction; a failure leaves the document
//! unchanged and is shown in the dialog. On success the new bodies are fitted
//! and an import summary lists bodies, units, warnings and the supported edit
//! actions.
//!
//! Export: STL/STEP state their body selection policy explicitly (selected
//! bodies, or all visible bodies), write bodies as separate solids/shells (not
//! fused), and omit hidden bodies unless selected. STEP refuses mesh bodies.
//! DXF exports one sketch in its local 2D coordinates (mm, R12).

use std::path::Path;

use eframe::egui::{self, Color32, RichText};
use f420_commands::{Command, Response};
use f420_document::{BodyId, FaceRef, FeatureId, SketchPlane};

use crate::app::{F420App, nonempty_path};
use crate::probe::Probe as _;

/// Interchange format, from the file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Step,
    Stl,
    Dxf,
}

impl Format {
    pub fn of(path: &str) -> Option<Format> {
        let ext = Path::new(path.trim())
            .extension()?
            .to_string_lossy()
            .to_ascii_lowercase();
        match ext.as_str() {
            "step" | "stp" => Some(Format::Step),
            "stl" => Some(Format::Stl),
            "dxf" => Some(Format::Dxf),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Step => "STEP (exact solids → B-rep bodies)",
            Format::Stl => "STL (triangle mesh → mesh body)",
            Format::Dxf => "DXF (2D lines/arcs/circles/polylines → unconstrained sketch)",
        }
    }
}

/// Unit choices offered when a file has none (mm per file unit).
pub const UNITS: &[(&str, f64)] = &[("mm", 1.0), ("cm", 10.0), ("m", 1000.0), ("inch", 25.4)];

/// Target plane of a DXF import.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DxfPlane {
    WorldXY,
    WorldXZ,
    WorldYZ,
    Face(FaceRef),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportForm {
    pub path: String,
    /// Index into [`UNITS`].
    pub unit: usize,
    /// DXF/STEP: the file declared no unit; ask (set after the first attempt).
    pub needs_units: bool,
    pub plane: DxfPlane,
    /// Face selected when the dialog opened (offered as a DXF plane).
    pub face: Option<(FaceRef, String)>,
    pub allow_partial: bool,
    /// Submitted; waiting for the engine's reply.
    pub pending: bool,
    /// Last failure (the document is unchanged).
    pub error: Option<String>,
}

impl ImportForm {
    pub fn command(&self) -> Result<Command, String> {
        let path = nonempty_path(&self.path)?;
        let fmt = Format::of(&self.path).ok_or_else(|| {
            format!(
                "'{}': unsupported file type; import supports .step/.stp, .stl and .dxf",
                self.path.trim()
            )
        })?;
        let unit = UNITS.get(self.unit).map_or(1.0, |u| u.1);
        Ok(match fmt {
            Format::Step => Command::ImportStep {
                path,
                units_to_mm: self.needs_units.then_some(unit),
            },
            Format::Stl => Command::ImportStl {
                path,
                units_to_mm: Some(unit),
            },
            Format::Dxf => Command::ImportDxf {
                path,
                plane: match self.plane {
                    DxfPlane::WorldXY => SketchPlane::WorldXY,
                    DxfPlane::WorldXZ => SketchPlane::WorldXZ,
                    DxfPlane::WorldYZ => SketchPlane::WorldYZ,
                    DxfPlane::Face(r) => SketchPlane::Face(r),
                },
                name: None,
                units_to_mm: self.needs_units.then_some(unit),
                allow_partial: self.allow_partial,
                layers: None,
            },
        })
    }
}

/// Shown after a successful import.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportSummary {
    pub file: String,
    pub format: Format,
    pub feature: Option<FeatureId>,
    pub bodies: Vec<String>,
    pub notes: Vec<String>,
}

/// Body selection policy of an STL/STEP export.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportScope {
    AllVisible,
    Selected(Vec<BodyId>),
}

impl ExportScope {
    pub fn bodies(&self) -> Option<Vec<BodyId>> {
        match self {
            ExportScope::AllVisible => None,
            ExportScope::Selected(v) => Some(v.clone()),
        }
    }
}

fn is_import(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::ImportStep { .. } | Command::ImportStl { .. } | Command::ImportDxf { .. }
    )
}

impl F420App {
    pub(crate) fn open_import_dialog(&mut self) {
        let face = self
            .selected_face()
            .and_then(|f| f.reference.map(|r| (r, f.description.clone())));
        self.dialog = Some(crate::app::Dialog::Import(ImportForm {
            path: String::new(),
            unit: 0,
            needs_units: false,
            plane: DxfPlane::WorldXY,
            face,
            allow_partial: false,
            pending: false,
            error: None,
        }));
    }

    /// Default export scope: the selected body if there is one, else all visible.
    pub(crate) fn default_export_scope(&self) -> ExportScope {
        match self.selected_body {
            Some(b) => ExportScope::Selected(vec![b]),
            None => ExportScope::AllVisible,
        }
    }

    /// Engine reply to an import: close the dialog and summarize, or keep it
    /// open with the error (asking for units when the file has none).
    pub(crate) fn import_reply(&mut self, cmd: &Command, response: &Response) {
        if !is_import(cmd) {
            return;
        }
        let form = match &mut self.dialog {
            Some(crate::app::Dialog::Import(f)) if f.pending => f,
            _ => return,
        };
        form.pending = false;
        if !response.ok {
            let msg = response
                .error
                .as_ref()
                .map_or("import failed".to_string(), |e| e.to_string());
            if msg.contains("declares no length unit") {
                form.needs_units = true;
            }
            form.error = Some(msg);
            return;
        }
        let path = form.path.trim().to_string();
        let format = Format::of(&path).unwrap_or(Format::Step);
        self.dialog = None;
        self.last_dialog = None;
        let fid = response.feature;
        let f = fid.and_then(|id| self.summary.features.iter().find(|f| f.id == id));
        let created = f.map(|f| f.creates.clone()).unwrap_or_default();
        let bodies = self
            .summary
            .bodies
            .iter()
            .filter(|b| created.contains(&b.id))
            .map(|b| b.name.clone())
            .collect();
        let mut notes = f.map(|f| f.notes.clone()).unwrap_or_default();
        if notes.is_empty()
            && let Some(m) = &response.message
        {
            notes = m.split("; ").map(str::to_string).collect();
        }
        self.import_summary = Some(ImportSummary {
            file: Path::new(&path)
                .file_name()
                .map_or(path.clone(), |n| n.to_string_lossy().into_owned()),
            format,
            feature: fid,
            bodies,
            notes,
        });
        if created.is_empty() {
            if let Some(id) = fid {
                self.select(Some(id));
            }
        } else {
            self.fit_bodies(&created);
            self.select_body(created.first().copied());
        }
    }

    /// Frame the given (visible) bodies.
    pub(crate) fn fit_bodies(&mut self, ids: &[BodyId]) {
        let m = &self.scene.mesh;
        let groups: Vec<std::ops::Range<u32>> = self
            .scene
            .bodies
            .iter()
            .filter(|b| ids.contains(&b.id))
            .map(|b| b.groups.clone())
            .collect();
        let pts = m
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .enumerate()
            .filter(|(t, _)| {
                m.tri_group
                    .get(*t)
                    .is_some_and(|g| groups.iter().any(|r| r.contains(g)))
            })
            .flat_map(|(_, idx)| {
                idx.iter()
                    .filter_map(|&i| m.positions.get(i as usize).copied())
            });
        let Some(bb) = f420_viewport::Aabb::from_points(pts) else {
            return;
        };
        let [w, h] = self.view_camera().1;
        let aspect = if h > 0.0 { w / h } else { 1.5 };
        let (c, r) = (bb.center(), bb.radius().max(1.0));
        match self.viewport.as_mut() {
            Some(vp) => vp.fit_to(c, r),
            None => self.fallback_camera.fit_sphere(c, r, aspect),
        }
        self.fitted = true;
    }

    /// Body of the Import dialog. Returns true when OK may be pressed.
    pub(crate) fn import_form_ui(&self, ui: &mut egui::Ui, form: &mut ImportForm) -> bool {
        crate::panels::text_field(ui, "Import file path", &mut form.path);
        let fmt = Format::of(&form.path);
        match fmt {
            Some(f) => {
                ui.label(format!("Format: {}", f.label()));
            }
            None if form.path.trim().is_empty() => {
                ui.weak("Supported: .step / .stp, .stl, .dxf");
            }
            None => {
                ui.colored_label(
                    Color32::from_rgb(255, 200, 120),
                    "Unsupported extension: import supports .step / .stp, .stl and .dxf",
                );
            }
        }
        let show_units = fmt == Some(Format::Stl) || form.needs_units;
        if show_units {
            let current = UNITS.get(form.unit).map_or("mm", |u| u.0);
            egui::ComboBox::from_label("File units")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (i, (n, _)) in UNITS.iter().enumerate() {
                        ui.selectable_value(&mut form.unit, i, *n);
                    }
                });
            ui.small(if fmt == Some(Format::Stl) {
                "STL files carry no units: choose what one file unit means. The mesh is scaled to mm."
            } else {
                "This file declares no length unit: choose what one file unit means."
            });
        }
        if fmt == Some(Format::Dxf) {
            ui.horizontal_wrapped(|ui| {
                ui.label("Sketch plane");
                ui.radio_value(&mut form.plane, DxfPlane::WorldXY, "World XY");
                ui.radio_value(&mut form.plane, DxfPlane::WorldXZ, "World XZ");
                ui.radio_value(&mut form.plane, DxfPlane::WorldYZ, "World YZ");
                match form.face {
                    Some((r, _)) => {
                        ui.radio_value(&mut form.plane, DxfPlane::Face(r), "Selected face");
                    }
                    None => {
                        ui.add_enabled(false, egui::RadioButton::new(false, "Selected face"))
                            .on_disabled_hover_text(
                                "Select a planar face that can host a sketch before opening Import",
                            );
                    }
                }
            });
            if let (DxfPlane::Face(_), Some((_, d))) = (form.plane, &form.face) {
                ui.small(format!("Face: {d}"));
            }
            ui.checkbox(&mut form.allow_partial, "Allow partial import")
                .probe("Allow partial import");
            ui.small("Off: a file with unsupported entities (splines, blocks, text, 3D) is refused. On: those entities are skipped and listed in the import summary; nothing is skipped silently.");
        }
        if fmt == Some(Format::Step) {
            ui.small("Each solid becomes a separate exact body (no feature history). The file is embedded in the document.");
        }
        if form.pending {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Importing… (parsing and validating on the worker thread)");
            });
        }
        if let Some(e) = &form.error {
            ui.colored_label(
                Color32::from_rgb(255, 110, 110),
                format!("Import failed, the document is unchanged: {e}"),
            );
        }
        !form.pending && fmt.is_some()
    }

    /// Body selection controls shared by the STL and STEP export dialogs.
    pub(crate) fn export_scope_ui(
        &self,
        ui: &mut egui::Ui,
        scope: &mut ExportScope,
        step: bool,
    ) -> Option<String> {
        let bodies = &self.summary.bodies;
        let visible: Vec<_> = bodies.iter().filter(|b| b.visible).collect();
        let mut selected = matches!(scope, ExportScope::Selected(_));
        ui.label(RichText::new("Bodies to export").strong());
        if ui
            .radio(!selected, format!("All visible bodies ({})", visible.len()))
            .probe("All visible bodies")
            .clicked()
        {
            selected = false;
        }
        if ui
            .radio(selected, "Selected bodies")
            .probe("Selected bodies")
            .clicked()
        {
            selected = true;
        }
        match (selected, &*scope) {
            (false, ExportScope::Selected(_)) => *scope = ExportScope::AllVisible,
            (true, ExportScope::AllVisible) => {
                *scope = ExportScope::Selected(self.selected_body.into_iter().collect())
            }
            _ => {}
        }
        if let ExportScope::Selected(ids) = scope {
            ui.indent("export_bodies", |ui| {
                for b in bodies {
                    let mut on = ids.contains(&b.id);
                    let label = format!(
                        "{}{}{}",
                        b.name,
                        if b.kind == "mesh" { " (mesh)" } else { "" },
                        if b.visible { "" } else { " (hidden)" }
                    );
                    if ui.checkbox(&mut on, label).changed() {
                        if on {
                            ids.push(b.id);
                        } else {
                            ids.retain(|x| *x != b.id);
                        }
                    }
                }
            });
        }
        ui.small("Bodies are written as separate solids/shells, not fused. Hidden bodies are omitted unless selected.");
        let chosen: Vec<_> = match &*scope {
            ExportScope::AllVisible => visible.clone(),
            ExportScope::Selected(ids) => bodies.iter().filter(|b| ids.contains(&b.id)).collect(),
        };
        let blocker = if chosen.is_empty() {
            Some("No body chosen for export".to_string())
        } else if step && let Some(m) = chosen.iter().find(|b| b.kind == "mesh") {
            Some(format!(
                "STEP needs exact B-rep bodies; {} is a mesh (STL) body. Deselect it or export STL.",
                m.name
            ))
        } else {
            None
        };
        ui.label(format!(
            "Will write {} bod{}: {}",
            chosen.len(),
            if chosen.len() == 1 { "y" } else { "ies" },
            chosen
                .iter()
                .map(|b| b.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        if let Some(b) = &blocker {
            ui.colored_label(Color32::from_rgb(255, 200, 120), b);
        }
        blocker
    }

    /// Import summary window (after a successful import).
    pub(crate) fn import_summary_ui(&mut self, ctx: &egui::Context) {
        let Some(s) = self.import_summary.clone() else {
            return;
        };
        let mut close = false;
        egui::Window::new("Import summary")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::RIGHT_TOP, [-310.0, 60.0])
            .show(ctx, |ui| {
                ui.label(RichText::new(&s.file).strong());
                ui.label(format!("Format: {}", s.format.label()));
                if !s.bodies.is_empty() {
                    ui.label(format!("Bodies ({}): {}", s.bodies.len(), s.bodies.join(", ")));
                }
                for n in &s.notes {
                    let warn = n.starts_with("warning") || n.starts_with("skipped");
                    if warn {
                        ui.colored_label(Color32::from_rgb(255, 200, 120), n);
                    } else {
                        ui.small(n);
                    }
                }
                ui.separator();
                ui.label(RichText::new("Supported edit actions").strong());
                ui.small(match s.format {
                    Format::Step => "Exact B-rep without feature history: select/hide/rename, sketch on planar faces, Extrude Join/Cut with it as target, export STEP/STL. Its original features are not recovered.",
                    Format::Stl => "Triangle mesh: display, hide/show, rename, save, STL export. Exact operations (sketch on faces, join/cut, STEP export) are unavailable.",
                    Format::Dxf => "Unconstrained sketch: edit and dimension it in the sketch editor, extrude it, export DXF. Constraints were not inferred.",
                });
                ui.small("Undo removes the whole import in one step.");
                if ui.button("Close summary").probe("Close summary").clicked() {
                    close = true;
                }
            });
        if close {
            self.import_summary = None;
        }
    }

    /// Body of the Export DXF dialog. Returns true when OK may be pressed.
    pub(crate) fn export_dxf_ui(
        &self,
        ui: &mut egui::Ui,
        path: &mut String,
        sketch: &mut Option<FeatureId>,
    ) -> bool {
        let sketches: Vec<_> = self
            .summary
            .features
            .iter()
            .filter(|f| f.kind == "sketch")
            .collect();
        crate::panels::feature_combo(ui, "Sketch to export", sketch, &sketches);
        crate::panels::text_field(ui, "DXF path", path);
        ui.small("Writes the sketch's lines, arcs and circles in its local 2D sketch coordinates, mm, as R12 ASCII DXF. Construction geometry and constraints are not written.");
        sketch.is_some()
    }
}
