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
use rcad_commands::print::{
    BedPreset, BedSpec, BodyPlacement, ExportReport, Layout, Orient, Placement, PrintOptions,
};
use rcad_commands::{Command, Response};
use rcad_document::{BodyId, FaceRef, FeatureId, SketchPlane};

use crate::app::{RcadApp, nonempty_path};
use crate::probe::Probe as _;

/// Interchange format, from the file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Step,
    Stl,
    Dxf,
    ThreeMf,
    Svg,
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
            "3mf" => Some(Format::ThreeMf),
            "svg" => Some(Format::Svg),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::Step => "STEP: exact solids as B-rep bodies",
            Format::Stl => "STL: triangle mesh as a mesh body",
            Format::Dxf => "DXF: 2D lines/arcs/circles/polylines as an unconstrained sketch",
            Format::ThreeMf => "3MF: mesh objects as mesh bodies (units declared in the file)",
            Format::Svg => "SVG: planar paths as an unconstrained sketch",
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
    /// STEP: import the file's surface models (open shells) as one
    /// surface body instead of its solids.
    pub surfaces: bool,
    /// Submitted: the request token (sequence number) of the import command
    /// whose reply this form waits for. A reply with another token (an import
    /// from a cancelled or replaced form) never touches this form.
    pub pending: Option<u64>,
    /// Last failure (the document is unchanged).
    pub error: Option<String>,
}

impl ImportForm {
    pub fn command(&self) -> Result<Command, String> {
        let path = nonempty_path(&self.path)?;
        let fmt = Format::of(&self.path).ok_or_else(|| {
            format!(
                "'{}': unsupported file type; import supports .step/.stp, .stl, .3mf, .dxf and .svg",
                self.path.trim()
            )
        })?;
        let unit = UNITS.get(self.unit).map_or(1.0, |u| u.1);
        let plane = match self.plane {
            DxfPlane::WorldXY => SketchPlane::WorldXY,
            DxfPlane::WorldXZ => SketchPlane::WorldXZ,
            DxfPlane::WorldYZ => SketchPlane::WorldYZ,
            DxfPlane::Face(r) => SketchPlane::Face(r),
        };
        Ok(match fmt {
            Format::ThreeMf => Command::Import3mf { path },
            Format::Svg => Command::ImportSvg {
                path,
                plane,
                name: None,
                units_to_mm: self.needs_units.then_some(unit),
                allow_partial: self.allow_partial,
            },
            Format::Step if self.surfaces => {
                Command::ImportSurfaces(rcad_commands::sheet_surface::ImportSurfacesArgs {
                    path,
                    units_to_mm: self.needs_units.then_some(unit),
                })
            }
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
                plane,
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
        Command::ImportStep { .. }
            | Command::ImportSurfaces(_)
            | Command::ImportStl { .. }
            | Command::ImportDxf { .. }
            | Command::Import3mf { .. }
            | Command::ImportSvg { .. }
    )
}

impl RcadApp {
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
            surfaces: false,
            pending: None,
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
    /// `token` is the reply's request sequence number: only the form that
    /// submitted that request is resolved.
    pub(crate) fn import_reply(&mut self, cmd: &Command, response: &Response, token: u64) {
        if !is_import(cmd) {
            return;
        }
        let form = match &mut self.dialog {
            Some(crate::app::Dialog::Import(f)) if f.pending == Some(token) => f,
            _ => return,
        };
        form.pending = None;
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
        let Some(bb) = rcad_viewport::Aabb::from_points(pts) else {
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
        crate::panels::path_field(
            ui,
            "Import file path",
            &mut form.path,
            crate::file_pick::PickMode::Open,
            crate::file_pick::IMPORT,
        );
        let fmt = Format::of(&form.path);
        match fmt {
            Some(f) => {
                ui.label(format!("Format: {}", f.label()));
            }
            None if form.path.trim().is_empty() => {
                ui.weak("Supported: .step / .stp, .stl, .3mf, .dxf, .svg");
            }
            None => {
                ui.colored_label(
                    Color32::from_rgb(255, 200, 120),
                    "Unsupported extension: import supports .step / .stp, .stl, .3mf, .dxf and .svg",
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
        if matches!(fmt, Some(Format::Dxf | Format::Svg)) {
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
            crate::sheet_ui::import_surfaces_toggle(ui, &mut form.surfaces);
        }
        if form.pending.is_some() {
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
        form.pending.is_none() && fmt.is_some()
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
        // STEP of all visible bodies skips mesh bodies (named in the result);
        // an explicit selection containing a mesh body is refused.
        let all = matches!(scope, ExportScope::AllVisible);
        let skipped: Vec<&str> = if step && all {
            chosen
                .iter()
                .filter(|b| b.kind == "mesh")
                .map(|b| b.name.as_str())
                .collect()
        } else {
            vec![]
        };
        let chosen: Vec<_> = if step && all {
            chosen.into_iter().filter(|b| b.kind != "mesh").collect()
        } else {
            chosen
        };
        if !skipped.is_empty() {
            ui.small(format!(
                "Mesh bodies are skipped for STEP (no exact geometry): {}",
                skipped.join(", ")
            ));
        }
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
        // A surface import is not a solid import.
        let surfaces = self
            .summary
            .features
            .iter()
            .any(|f| Some(f.id) == s.feature && f.kind == "import_surfaces");
        egui::Window::new("Import summary")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::RIGHT_BOTTOM, [-310.0, -40.0])
            .show(ctx, |ui| {
                ui.label(RichText::new(&s.file).strong());
                if surfaces {
                    ui.label("Format: STEP surface models: one open-shell surface body (no volume)");
                } else {
                    ui.label(format!("Format: {}", s.format.label()));
                }
                if !s.bodies.is_empty() {
                    ui.label(format!("Bodies ({}): {}", s.bodies.len(), s.bodies.join(", ")));
                }
                let has_skipped = s.notes.iter().any(|n| n.starts_with("skipped"));
                for n in s
                    .notes
                    .iter()
                    .filter(|n| !(has_skipped && n.starts_with("warning: skipped")))
                {
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
                    Format::Step if surfaces => "Exact surface body without feature history: analyse it, stitch it, fill planar holes or thicken a single face (each makes a new body). Solid operations and STEP export refuse it until it is a closed solid.",
                    Format::Step => "Exact B-rep without feature history: select/hide/rename, sketch on planar faces, Extrude Join/Cut with it as target, export STEP/STL. Its original features are not recovered.",
                    Format::Stl => "Triangle mesh: display, hide/show, rename, save, STL export. Exact operations (sketch on faces, join/cut, STEP export) are unavailable.",
                    Format::Dxf | Format::Svg => "Unconstrained sketch: edit and dimension it in the sketch editor, extrude it, export DXF/SVG. Constraints were not inferred.",
                    Format::ThreeMf => "Mesh objects: display, hide/show, rename, save, STL/3MF export. Exact operations (sketch on faces, join/cut, STEP export) are unavailable. Units came from the file.",
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

    /// Body of the Export DXF / SVG (sketch) dialog. Returns true when OK may be pressed.
    pub(crate) fn export_dxf_ui(
        &self,
        ui: &mut egui::Ui,
        path: &mut String,
        sketch: &mut Option<FeatureId>,
        svg: bool,
    ) -> bool {
        let sketches: Vec<_> = self
            .summary
            .features
            .iter()
            .filter(|f| f.kind == "sketch")
            .collect();
        crate::panels::feature_combo(ui, "Sketch to export", sketch, &sketches);
        if svg {
            crate::panels::path_field(
                ui,
                "SVG path",
                path,
                crate::file_pick::PickMode::Save,
                crate::file_pick::SVG,
            );
            ui.small("Writes the sketch's lines, arcs and circles in its local 2D sketch coordinates, mm (SVG y axis points down, so y is negated). Construction geometry and constraints are not written.");
        } else {
            crate::panels::path_field(
                ui,
                "DXF path",
                path,
                crate::file_pick::PickMode::Save,
                crate::file_pick::DXF,
            );
            ui.small("Writes the sketch's lines, arcs and circles in its local 2D sketch coordinates, mm, as R12 ASCII DXF. Construction geometry and constraints are not written.");
        }
        sketch.is_some()
    }
}

// ------------------------------------------------------------ print export

/// Printable volume choice of the STL/3MF export dialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BedChoice {
    /// No fit report.
    Off,
    /// Bambu Lab P1S (values from the installed Bambu Studio profile).
    P1s,
    Manual,
}

/// STL / 3MF export dialog state (docs/PRINT_EXPORT.md): body selection,
/// tessellation, units, export-only placement, bed fit, and the per-body
/// report of the last export from this dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct PrintForm {
    /// STL (else 3MF).
    pub stl: bool,
    pub path: String,
    pub chord: String,
    /// Max facet angle (degrees); empty = chord only.
    pub angle: String,
    pub scope: ExportScope,
    /// 3MF file unit.
    pub unit: rcad_exchange::threemf::ThreeMfUnit,
    /// Export-only placement on the bed (off: document coordinates).
    pub place: bool,
    /// Per-body orientation (bodies not listed: as modeled).
    pub orient: Vec<(BodyId, Orient)>,
    pub layout: bool,
    pub gap: String,
    pub bed: BedChoice,
    pub bed_size: [String; 3],
    /// STL only: one file per body.
    pub separate_files: bool,
    pub allow_open_meshes: bool,
    /// Request token of the export this form waits for.
    pub pending: Option<u64>,
    /// Report of the last successful export from this dialog.
    pub report: Option<ExportReport>,
    /// Last failure (nothing was written).
    pub error: Option<String>,
}

impl PrintForm {
    pub fn new(stl: bool, path: String, scope: ExportScope) -> Self {
        Self {
            stl,
            path,
            chord: format!("{}", rcad_kernel::tol::MESH_CHORD),
            angle: String::new(),
            scope,
            unit: rcad_exchange::threemf::ThreeMfUnit::Millimeter,
            place: false,
            orient: vec![],
            layout: false,
            gap: "10".into(),
            bed: BedChoice::Off,
            bed_size: ["256".into(), "256".into(), "250".into()],
            separate_files: false,
            allow_open_meshes: false,
            pending: None,
            report: None,
            error: None,
        }
    }

    pub fn orientation(&self, body: BodyId) -> Orient {
        self.orient
            .iter()
            .find(|(b, _)| *b == body)
            .map_or(Orient::AsModeled, |(_, o)| *o)
    }

    pub fn set_orientation(&mut self, body: BodyId, o: Orient) {
        self.orient.retain(|(b, _)| *b != body);
        if o != Orient::AsModeled {
            self.orient.push((body, o));
        }
    }

    /// Features referenced by face-down choices (for dialog revalidation).
    pub fn face_features(&self) -> Vec<FeatureId> {
        self.orient
            .iter()
            .filter_map(|(_, o)| match o {
                Orient::FaceDown(r) => Some(r.feature),
                Orient::AsModeled => None,
            })
            .collect()
    }

    pub fn options(&self) -> Result<PrintOptions, String> {
        use crate::app::parse_num;
        let angle = match self.angle.trim() {
            "" => None,
            a => Some(parse_num("max facet angle", a)?),
        };
        let bed = match self.bed {
            BedChoice::Off => None,
            BedChoice::P1s => Some(BedSpec::Preset {
                preset: BedPreset::BambuP1s,
            }),
            BedChoice::Manual => Some(BedSpec::Manual {
                width: parse_num("bed width", &self.bed_size[0])?,
                depth: parse_num("bed depth", &self.bed_size[1])?,
                height: parse_num("bed height", &self.bed_size[2])?,
            }),
        };
        let placement = if self.place {
            Some(Placement {
                bodies: self
                    .orient
                    .iter()
                    .map(|(b, o)| BodyPlacement {
                        body: *b,
                        orient: *o,
                    })
                    .collect(),
                layout: if self.layout {
                    Some(Layout {
                        gap: parse_num("layout gap", &self.gap)?,
                    })
                } else {
                    None
                },
            })
        } else {
            None
        };
        Ok(PrintOptions {
            angle_deg: angle,
            placement,
            bed,
            separate_files: self.stl && self.separate_files,
            allow_open_meshes: self.allow_open_meshes,
        })
    }

    pub fn command(&self) -> Result<Command, String> {
        let path = nonempty_path(&self.path)?;
        let chord = Some(crate::app::parse_num("chord", &self.chord)?);
        let bodies = self.scope.bodies();
        // Face-down choices only for exported bodies (the selection may have changed).
        let mut form = self.clone();
        if let Some(ids) = &bodies {
            form.orient.retain(|(b, _)| ids.contains(b));
        }
        let print = form.options()?;
        Ok(if self.stl {
            Command::ExportStl {
                path,
                chord,
                bodies,
                print,
            }
        } else {
            Command::Export3mf {
                path,
                chord,
                bodies,
                unit: Some(self.unit),
                print,
            }
        })
    }
}

fn fmt3(v: [f64; 3]) -> String {
    format!("{:.3} × {:.3} × {:.3}", v[0], v[1], v[2])
}

impl RcadApp {
    pub(crate) fn open_print_export_dialog(&mut self, stl: bool) {
        let form = PrintForm::new(
            stl,
            self.default_path(if stl { "stl" } else { "3mf" }),
            self.default_export_scope(),
        );
        self.dialog = Some(if stl {
            crate::app::Dialog::ExportStl(form)
        } else {
            crate::app::Dialog::Export3mf(form)
        });
    }

    /// Engine reply to an STL/3MF export: store the report (or the error) in
    /// the form that submitted it (matched by request token).
    pub(crate) fn print_export_reply(&mut self, cmd: &Command, response: &Response, token: u64) {
        if !matches!(cmd, Command::ExportStl { .. } | Command::Export3mf { .. }) {
            return;
        }
        let form = match &mut self.dialog {
            Some(crate::app::Dialog::ExportStl(f) | crate::app::Dialog::Export3mf(f))
                if f.pending == Some(token) =>
            {
                f
            }
            _ => return,
        };
        form.pending = None;
        if response.ok {
            form.report = response.export.clone();
            form.error = None;
        } else {
            form.report = None;
            form.error = Some(
                response
                    .error
                    .as_ref()
                    .map_or("export failed".to_string(), |e| e.to_string()),
            );
        }
    }

    /// Body of the Export STL / Export 3MF dialog. Returns true when OK may be pressed.
    pub(crate) fn print_export_ui(&self, ui: &mut egui::Ui, form: &mut PrintForm) -> bool {
        use rcad_exchange::threemf::ThreeMfUnit as U;
        let warn = Color32::from_rgb(255, 200, 120);
        let mut ok = true;
        if form.stl {
            crate::panels::path_field(
                ui,
                "STL path",
                &mut form.path,
                crate::file_pick::PickMode::Save,
                crate::file_pick::STL,
            );
            ui.small("Units: coordinates in mm. STL has no unit field, so the slicer must interpret the file as millimetres.");
        } else {
            crate::panels::path_field(
                ui,
                "3MF path",
                &mut form.path,
                crate::file_pick::PickMode::Save,
                crate::file_pick::THREEMF,
            );
            egui::ComboBox::from_label("3MF file unit")
                .selected_text(format!("{:?}", form.unit))
                .show_ui(ui, |ui| {
                    for u in [
                        U::Millimeter,
                        U::Centimeter,
                        U::Meter,
                        U::Inch,
                        U::Foot,
                        U::Micron,
                    ] {
                        ui.selectable_value(&mut form.unit, u, format!("{u:?}"));
                    }
                });
            ui.small(format!(
                "Units: declared in the file as {}; sizes below are mm. Geometry only: one named object per body, no printer, process or filament settings.",
                form.unit.name()
            ));
        }
        crate::panels::num_field(ui, "Chord tolerance (mm)", &mut form.chord);
        crate::panels::num_field(ui, "Max facet angle (deg)", &mut form.angle);
        ui.small(format!(
            "Default chord {} mm (max distance of the mesh from curved surfaces). Facet angle empty = chord only; otherwise the chord is refined until adjacent facets of a curved face differ by at most this angle.",
            rcad_kernel::tol::MESH_CHORD
        ));
        ok &= self.export_scope_ui(ui, &mut form.scope, false).is_none();
        let bodies = &self.summary.bodies;
        let hidden: Vec<&str> = bodies
            .iter()
            .filter(|b| !b.visible)
            .map(|b| b.name.as_str())
            .collect();
        let chosen: Vec<_> = match &form.scope {
            ExportScope::AllVisible => bodies.iter().filter(|b| b.visible).collect(),
            ExportScope::Selected(ids) => bodies.iter().filter(|b| ids.contains(&b.id)).collect(),
        };
        ui.small(match (&form.scope, hidden.is_empty()) {
            (_, true) => "Hidden bodies: none.".to_string(),
            (ExportScope::AllVisible, false) => {
                format!("Hidden bodies are not exported: {}.", hidden.join(", "))
            }
            (ExportScope::Selected(_), false) => format!(
                "Hidden bodies are exported only when ticked: {}.",
                hidden.join(", ")
            ),
        });
        ui.checkbox(&mut form.place, "Place on bed (export only)")
            .probe("Place on bed (export only)");
        let face = self
            .selected_face()
            .and_then(|f| f.reference.map(|r| (r, f.body)));
        egui::Grid::new("print_bodies")
            .striped(true)
            .show(ui, |ui| {
                ui.strong("Body");
                ui.strong("Kind");
                ui.strong("Modeled size (mm)");
                ui.strong("Orientation");
                ui.end_row();
                for b in &chosen {
                    ui.label(format!(
                        "{}{}",
                        b.name,
                        if b.visible { "" } else { " (hidden)" }
                    ));
                    ui.label(match (&b.mesh, b.kind.as_str()) {
                        (Some(d), _) if d.closed => "mesh (closed)",
                        (Some(_), _) => "mesh (OPEN)",
                        (None, "mesh") => "mesh",
                        _ => "solid",
                    });
                    let size = b
                        .measurements
                        .as_ref()
                        .map(|m| {
                            let bb = &m.bounds;
                            fmt3(std::array::from_fn(|i| bb.max[i] - bb.min[i]))
                        })
                        .or_else(|| {
                            b.mesh
                                .as_ref()
                                .and_then(|d| d.bounds)
                                .map(|(lo, hi)| fmt3(std::array::from_fn(|i| hi[i] - lo[i])))
                        })
                        .unwrap_or_else(|| "measuring…".into());
                    ui.label(size);
                    ui.add_enabled_ui(form.place, |ui| {
                        ui.horizontal(|ui| {
                            let cur = form.orientation(b.id);
                            if ui
                                .radio(cur == Orient::AsModeled, "As modeled")
                                .probe(&format!("{} as modeled", b.name))
                                .clicked()
                            {
                                form.set_orientation(b.id, Orient::AsModeled);
                            }
                            let here = face.filter(|(_, fb)| *fb == Some(b.id)).map(|(r, _)| r);
                            let resp = ui
                                .add_enabled(
                                    here.is_some(),
                                    egui::RadioButton::new(
                                        matches!(cur, Orient::FaceDown(_)),
                                        "Selected face down",
                                    ),
                                )
                                .probe(&format!("{} selected face down", b.name))
                                .on_disabled_hover_text(
                                    "Select a planar face of this body in the viewport first",
                                );
                            if resp.clicked()
                                && let Some(r) = here
                            {
                                form.set_orientation(b.id, Orient::FaceDown(r));
                            }
                            if let Orient::FaceDown(r) = cur {
                                ui.small(format!(
                                    "on bed: {} {}",
                                    self.feature_name(r.feature),
                                    crate::face_role_label(&r.role)
                                ));
                            }
                        });
                    });
                    ui.end_row();
                }
            });
        if form.place {
            ui.checkbox(&mut form.layout, "Lay out side by side along X")
                .probe("Lay out side by side along X");
            if form.layout {
                crate::panels::num_field(ui, "Layout gap (mm)", &mut form.gap);
            }
            ui.small("Each body is rotated (proper rotation, never mirrored) so its chosen face lies on the bed, then moved so its lowest point is at z = 0. The document is not changed.");
        } else {
            ui.small("Coordinates are written as modeled (document coordinates).");
        }
        ui.horizontal(|ui| {
            ui.label("Bed");
            ui.radio_value(&mut form.bed, BedChoice::Off, "None")
                .probe("Bed none");
            ui.radio_value(&mut form.bed, BedChoice::P1s, "Bambu Lab P1S")
                .probe("Bambu Lab P1S");
            ui.radio_value(&mut form.bed, BedChoice::Manual, "Manual")
                .probe("Bed manual");
        });
        match form.bed {
            BedChoice::P1s => {
                ui.small("256 × 256 mm, height 250 mm, front-left 18 × 28 mm excluded (Bambu Studio 2.8.2.61 P1S 0.4 nozzle profile). Fit is reported, never forced.");
            }
            BedChoice::Manual => {
                let [w, d, h] = &mut form.bed_size;
                crate::panels::num_field(ui, "Bed width (mm)", w);
                crate::panels::num_field(ui, "Bed depth (mm)", d);
                crate::panels::num_field(ui, "Bed height (mm)", h);
            }
            BedChoice::Off => {}
        }
        if form.stl {
            ui.checkbox(&mut form.separate_files, "One STL file per body")
                .probe("One STL file per body");
        }
        ui.checkbox(&mut form.allow_open_meshes, "Allow open reference meshes")
            .probe("Allow open reference meshes");
        ui.small("Off: every body must export as a closed, manifold, outward solid mesh, or nothing is written. Self-intersection is not checked.");
        if !form.allow_open_meshes
            && chosen
                .iter()
                .any(|b| b.mesh.as_ref().is_some_and(|d| !d.closed))
        {
            ui.colored_label(warn, "An open mesh body is chosen: the export will be refused unless open reference meshes are allowed.");
        }
        if let Err(e) = form.options() {
            ui.colored_label(warn, e);
            ok = false;
        }
        ui.small("Exports the last committed model revision (queued after pending edits).");
        if form.pending.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Exporting…");
            });
        }
        if let Some(e) = &form.error {
            ui.colored_label(
                Color32::from_rgb(255, 110, 110),
                format!("Export failed, nothing written: {e}"),
            );
        }
        if let Some(r) = &form.report {
            print_report_ui(ui, r);
        }
        ok && form.pending.is_none()
    }
}

/// Per-body report of a finished export.
fn print_report_ui(ui: &mut egui::Ui, r: &ExportReport) {
    ui.separator();
    ui.label(
        RichText::new(format!(
            "Exported revision {} as {} ({})",
            r.revision, r.format, r.placement
        ))
        .strong(),
    );
    ui.small(format!("{}; units {}", r.selection, r.units));
    egui::Grid::new("print_report")
        .striped(true)
        .show(ui, |ui| {
            for h in ["Body", "Triangles", "Size (mm)", "Mesh checks", "Fit"] {
                ui.strong(h);
            }
            ui.end_row();
            for b in &r.bodies {
                ui.label(&b.name);
                ui.label(b.triangles.to_string());
                ui.label(fmt3(b.dims));
                let c = &b.mesh_check;
                ui.label(format!(
                    "{}: closed {} · manifold {} · outward {} · V {:.3} mm³ · self-intersection {}",
                    b.label,
                    yes(c.closed_edges),
                    yes(c.nonmanifold_vertices == 0 && c.nonmanifold_edges == 0),
                    yes(c.orientation_consistent && c.outward),
                    c.mesh_volume,
                    c.self_intersection
                ));
                ui.label(b.fit.as_ref().map_or("-", |f| f.detail.as_str()));
                ui.end_row();
            }
        });
    for f in &r.files {
        ui.small(format!("{} ({} bytes)", f.path.display(), f.bytes));
    }
    for w in &r.warnings {
        ui.colored_label(Color32::from_rgb(255, 200, 120), w);
    }
}

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "NO" }
}
