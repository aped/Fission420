//! Typed, serializable command API shared by the UI, the CLI, and tests.
//!
//! Every modeling action available in the app is a [`Command`]; [`Session::execute`]
//! returns a [`Response`] with feature ids, measurements, and structured errors.
//! See docs/COMMANDS.md.

pub mod acceptance;
pub mod scenarios;
pub mod sketches;

use f420_document::{
    BodyGeometry, BodyId, BodySource, DocError, Document, Extent, FaceCandidate, FaceRef, FaceRole,
    FeatureId, FeatureKind, Model, Operation, SketchPlane, load_file, save_file, write_atomic,
};
use f420_kernel::{self as k, Frame, Measurements, MeshCheck};
use f420_sketch::{Sketch, SolveStatus};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "template", rename_all = "snake_case")]
pub enum SketchTemplate {
    /// Dimensioned rectangle centred on `center` (default: sketch origin).
    /// Parameters: "width", "height", "center_x", "center_y".
    CenteredRectangle {
        width: f64,
        height: f64,
        #[serde(default)]
        center: Option<[f64; 2]>,
    },
    /// Circle centred on `center` (default: sketch origin).
    /// Parameters: "diameter", "center_x", "center_y".
    CenteredCircle {
        diameter: f64,
        #[serde(default)]
        center: Option<[f64; 2]>,
    },
    /// Closed polygon of lines through `vertices` (sketch-plane mm), sharing
    /// endpoints, with no other constraints (vertices stay editable).
    Polygon { vertices: Vec<[f64; 2]> },
    /// A complete sketch as produced by the interactive editor.
    Custom { sketch: Sketch },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    NewDocument,
    CreateSketch {
        #[serde(default)]
        name: Option<String>,
        plane: SketchPlane,
        #[serde(flatten)]
        template: SketchTemplate,
    },
    /// Extrude a sketch profile (ADR 0003): a new body (default), or joined to /
    /// cut from `target` (default: the only body, else required). `through_all`
    /// (Cut only) ignores `distance`. `reverse` extrudes against the sketch normal.
    Extrude {
        #[serde(default)]
        name: Option<String>,
        sketch: FeatureId,
        #[serde(default)]
        distance: f64,
        #[serde(default, skip_serializing_if = "ExtrudeOp::is_new_body")]
        op: ExtrudeOp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<BodyId>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        through_all: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reverse: bool,
        /// New body: its name (default "BodyN").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body_name: Option<String>,
    },
    /// New body: revolve a sketch about its local Y axis (WorldXZ sketch -> world Z axis).
    Revolve {
        #[serde(default)]
        name: Option<String>,
        sketch: FeatureId,
        angle_deg: f64,
    },
    CutThroughAll {
        #[serde(default)]
        name: Option<String>,
        sketch: FeatureId,
    },
    /// Blind circular pocket from the extrusion cap the sketch is on, `depth` mm
    /// into the material (0 < depth < thickness; parameter "depth").
    Pocket {
        #[serde(default)]
        name: Option<String>,
        sketch: FeatureId,
        depth: f64,
    },
    /// Repeat an Extrude Cut `count - 1` more times at `spacing` mm along
    /// `direction` (the seed sketch's local 2D axes; e.g. `[1, 0]` = sketch x),
    /// on the seed's target body. Parameters "count", "spacing".
    LinearPattern {
        #[serde(default)]
        name: Option<String>,
        feature: FeatureId,
        count: u32,
        spacing: f64,
        direction: [f64; 2],
    },
    /// Repeat an Extrude Join/Cut `count - 1` more times, each rotated a further
    /// `angle_deg` counter-clockwise about the seed sketch's normal through
    /// `center` (seed sketch 2D coordinates). Parameters "count", "angle_deg".
    CircularPattern {
        #[serde(default)]
        name: Option<String>,
        feature: FeatureId,
        count: u32,
        angle_deg: f64,
        center: [f64; 2],
    },
    /// Add the mirror image of an Extrude Join/Cut's tool across `plane` (world
    /// plane through the origin, or a planar face) to the seed's target body.
    Mirror {
        #[serde(default)]
        name: Option<String>,
        feature: FeatureId,
        plane: SketchPlane,
    },
    /// Chamfer the straight convex edge between two planar faces (semantic
    /// references, e.g. from `FeatureSummary.faces`), `distance` mm along each.
    Chamfer {
        #[serde(default)]
        name: Option<String>,
        a: FaceRef,
        b: FaceRef,
        distance: f64,
    },
    SetParameter {
        feature: FeatureId,
        name: String,
        value: f64,
    },
    /// Replace a sketch feature's definition (one undoable step; the editor's
    /// Finish Sketch). Plane and downstream references are kept.
    EditSketch {
        feature: FeatureId,
        sketch: Sketch,
    },
    RenameFeature {
        feature: FeatureId,
        name: String,
    },
    RenameBody {
        body: BodyId,
        name: String,
    },
    SetBodyVisible {
        body: BodyId,
        visible: bool,
    },
    /// Re-attach a sketch to another plane or face (repair of a broken face
    /// reference; one undo step). The sketch's local geometry is kept.
    SetSketchPlane {
        feature: FeatureId,
        plane: SketchPlane,
    },
    /// Import exact solids from a STEP file as new bodies (one per solid). The
    /// file is embedded in the document. `units_to_mm` is used only when the
    /// file declares no length unit.
    ImportStep {
        path: PathBuf,
        #[serde(default)]
        units_to_mm: Option<f64>,
    },
    /// Import an STL file as a mesh body (display/export only). STL is unitless:
    /// `units_to_mm` (mm per file unit) is required.
    ImportStl {
        path: PathBuf,
        #[serde(default)]
        units_to_mm: Option<f64>,
    },
    /// Import the 2D geometry of a DXF file as a new (unconstrained) sketch on
    /// `plane`. `units_to_mm` is used only when the file declares no unit.
    ImportDxf {
        path: PathBuf,
        plane: SketchPlane,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        units_to_mm: Option<f64>,
        /// Skip unsupported entities (reported) instead of refusing the file.
        #[serde(default)]
        allow_partial: bool,
        #[serde(default)]
        layers: Option<Vec<String>>,
    },
    /// Import every build item of a 3MF package as a mesh body (units from the
    /// file; source embedded). Display/export only, like STL.
    #[serde(rename = "import_3mf")]
    Import3mf {
        path: PathBuf,
    },
    /// Write `bodies` (default: all visible) as one 3MF object each: B-rep
    /// bodies tessellated at `chord` (watertightness checked), mesh bodies as
    /// imported; `unit` (default millimeter) is the file's declared unit.
    #[serde(rename = "export_3mf")]
    Export3mf {
        path: PathBuf,
        #[serde(default)]
        chord: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bodies: Option<Vec<BodyId>>,
        #[serde(default)]
        unit: Option<f420_exchange::threemf::ThreeMfUnit>,
    },
    /// Import the planar paths of an SVG file (subset: docs/formats/svg.md) as a
    /// new unconstrained sketch on `plane`. `units_to_mm` (mm per user unit) is
    /// used only when the file declares no physical size.
    ImportSvg {
        path: PathBuf,
        plane: SketchPlane,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        units_to_mm: Option<f64>,
        #[serde(default)]
        allow_partial: bool,
    },
    /// Write a sketch's lines/arcs/circles as SVG (local 2D coordinates, mm, y
    /// negated for SVG's y-down axis).
    ExportSvg {
        path: PathBuf,
        sketch: FeatureId,
    },
    /// Write a sketch's lines/arcs/circles as an R12 ASCII DXF (mm).
    ExportDxf {
        path: PathBuf,
        sketch: FeatureId,
    },
    /// Remove a feature that nothing depends on (one undoable step). Refused
    /// with the dependent features named otherwise.
    DeleteFeature {
        feature: FeatureId,
    },
    Undo,
    Redo,
    Save {
        path: PathBuf,
    },
    Open {
        path: PathBuf,
    },
    Close,
    /// Binary STL of `bodies` (default: all visible bodies). B-rep bodies are
    /// tessellated (watertightness checked); mesh bodies are written as imported.
    ExportStl {
        path: PathBuf,
        #[serde(default)]
        chord: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bodies: Option<Vec<BodyId>>,
    },
    /// STEP (AP214 via truck-stepio) of the exact B-rep `bodies` (default: all
    /// visible B-rep bodies; visible mesh bodies are named in the message as not
    /// written), one solid per body. An explicit selection with a mesh is refused.
    ExportStep {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bodies: Option<Vec<BodyId>>,
    },
    Measure,
    Inspect,
}

impl Command {
    /// A new-body distance extrusion (the common case).
    pub fn extrude(name: Option<String>, sketch: FeatureId, distance: f64) -> Self {
        Command::Extrude {
            name,
            sketch,
            distance,
            op: ExtrudeOp::NewBody,
            target: None,
            through_all: false,
            reverse: false,
            body_name: None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Command::NewDocument => "new_document",
            Command::CreateSketch { .. } => "create_sketch",
            Command::Extrude { .. } => "extrude",
            Command::CutThroughAll { .. } => "cut_through_all",
            Command::Pocket { .. } => "pocket",
            Command::Revolve { .. } => "revolve",
            Command::LinearPattern { .. } => "linear_pattern",
            Command::Mirror { .. } => "mirror",
            Command::CircularPattern { .. } => "circular_pattern",
            Command::Chamfer { .. } => "chamfer",
            Command::SetParameter { .. } => "set_parameter",
            Command::EditSketch { .. } => "edit_sketch",
            Command::RenameFeature { .. } => "rename_feature",
            Command::RenameBody { .. } => "rename_body",
            Command::SetBodyVisible { .. } => "set_body_visible",
            Command::SetSketchPlane { .. } => "set_sketch_plane",
            Command::ImportStep { .. } => "import_step",
            Command::ImportStl { .. } => "import_stl",
            Command::ImportDxf { .. } => "import_dxf",
            Command::ExportDxf { .. } => "export_dxf",
            Command::ImportSvg { .. } => "import_svg",
            Command::ExportSvg { .. } => "export_svg",
            Command::Import3mf { .. } => "import_3mf",
            Command::Export3mf { .. } => "export_3mf",
            Command::DeleteFeature { .. } => "delete_feature",
            Command::Undo => "undo",
            Command::Redo => "redo",
            Command::Save { .. } => "save",
            Command::Open { .. } => "open",
            Command::Close => "close",
            Command::ExportStl { .. } => "export_stl",
            Command::ExportStep { .. } => "export_step",
            Command::Measure => "measure",
            Command::Inspect => "inspect",
        }
    }
}

/// What an extrusion does with its tool body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExtrudeOp {
    #[default]
    NewBody,
    Join,
    Cut,
}

impl ExtrudeOp {
    fn is_new_body(&self) -> bool {
        *self == ExtrudeOp::NewBody
    }
}

/// Inspectable per-body state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodySummary {
    pub id: BodyId,
    pub name: String,
    pub visible: bool,
    /// "brep" (exact solid) or "mesh" (triangle mesh, STL import).
    pub kind: String,
    pub source: BodySource,
    pub created_by: FeatureId,
    pub last_feature: FeatureId,
    /// Exact B-rep measurements (None: deferred or a mesh body).
    #[serde(default)]
    pub measurements: Option<Measurements>,
    /// Mesh bodies: diagnostics (closed, boundary edges, volume if closed...).
    #[serde(default)]
    pub mesh: Option<f420_document::import::MeshDiagnostics>,
}

/// Inspectable per-feature state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureSummary {
    pub id: FeatureId,
    pub name: String,
    pub kind: String,
    pub parameters: Vec<(String, f64)>,
    pub depends_on: Vec<FeatureId>,
    pub plane: Option<SketchPlane>,
    pub solve_status: Option<SolveStatus>,
    pub solve_dof: Option<usize>,
    pub solve_residual: Option<f64>,
    pub frame: Option<Frame>,
    /// Sketches: labels of conflicting/redundant constraints from the last solve.
    #[serde(default)]
    pub solve_conflicts: Vec<String>,
    /// Sketches: solved geometry (for display and as the editor's seed).
    #[serde(default)]
    pub sketch: Option<Sketch>,
    /// Sketches: why this is not a usable closed profile, if it is not.
    #[serde(default)]
    pub profile_error: Option<String>,
    /// Extrusions/imports: semantic faces that can host a sketch, with their frames.
    #[serde(default)]
    pub faces: Vec<FaceSummary>,
    /// Extrusions: operation, extent, direction.
    #[serde(default)]
    pub operation: Option<Operation>,
    #[serde(default)]
    pub extent: Option<Extent>,
    #[serde(default)]
    pub reverse: bool,
    /// Body created or modified by this feature.
    #[serde(default)]
    pub body: Option<BodyId>,
    /// Bodies created by this feature.
    #[serde(default)]
    pub creates: Vec<BodyId>,
    /// Imports: units, counts, warnings.
    #[serde(default)]
    pub notes: Vec<String>,
}

/// A semantically referenceable face and its current placement frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceSummary {
    pub reference: FaceRef,
    pub frame: Frame,
    #[serde(default)]
    pub body: BodyId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocSummary {
    /// Identity of the open document; changes on New/Open/Close so clients can
    /// discard state that belongs to a previous document.
    #[serde(default)]
    pub doc_id: u64,
    pub open: bool,
    /// The body's measurements were deferred and are not available yet
    /// (interactive clients only; see `Session::set_defer_measurements`).
    #[serde(default)]
    pub measuring: bool,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub revision: u64,
    pub can_undo: Option<String>,
    pub can_redo: Option<String>,
    pub features: Vec<FeatureSummary>,
    /// Bodies in creation order.
    #[serde(default)]
    pub bodies: Vec<BodySummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Response {
    pub ok: bool,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature: Option<FeatureId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measurements: Option<Measurements>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mesh_check: Option<MeshCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<DocSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DocError>,
    /// Set by a queueing front end (the app engine) when this command was not
    /// executed because a newer edit of the same parameter was already queued
    /// behind it. `ok` is true and nothing changed. [`Session`] never sets it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub superseded: bool,
    /// Wall time (ms) the front end spent on this command, including any work
    /// it did after [`Session::execute`] (e.g. display tessellation). [`Session`]
    /// leaves it `None`; profiling only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<f64>,
}

/// `(doc_id, revision, [(body, deep copy)])` awaiting measurement.
pub type PendingMeasurement = (u64, u64, Vec<(BodyId, f420_kernel::Body)>);

/// An application session: at most one open document.
pub struct Session {
    model: Option<Model>,
    path: Option<PathBuf>,
    /// Bumped whenever the open document is replaced or closed.
    doc_id: u64,
    /// Skip B-rep measurement during edits (see `set_defer_measurements`).
    defer_measurements: bool,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Self {
            model: None,
            path: None,
            doc_id: 0,
            defer_measurements: false,
        }
    }

    pub fn model(&self) -> Option<&Model> {
        self.model.as_ref()
    }

    /// Interactive clients: skip the expensive B-rep measurements while editing
    /// and supply them later with [`Session::adopt_measurements`]. Responses then
    /// carry `measurements: None` and `summary.measuring: true` until adopted.
    /// `save` and `measure` always measure synchronously first. Off by default
    /// (CLI, tests and the acceptance runner get measurements in every response).
    pub fn set_defer_measurements(&mut self, on: bool) {
        self.defer_measurements = on;
        if let Some(m) = &mut self.model {
            m.set_defer_measurements(on);
        }
    }

    /// Bodies awaiting deferred measurement: `(doc_id, revision, bodies)`. The
    /// bodies are deep copies (Truck bodies share geometry behind mutexes, so a
    /// shallow clone measured on another thread races with this session's own use).
    pub fn pending_measurement(&self) -> Option<PendingMeasurement> {
        let m = self.model.as_ref()?;
        if !m.measurements_pending() {
            return None;
        }
        let bodies = m
            .evaluation()
            .unmeasured()
            .into_iter()
            .map(|(id, b)| b.deep_copy().map(|b| (id, b)))
            .collect::<Result<Vec<_>, _>>()
            .ok()?;
        Some((self.doc_id, m.revision, bodies))
    }

    /// The deferred measurement of `revision` failed: stop reporting it as
    /// pending. Returns true when it applied to the current document revision.
    pub fn fail_measurements(&mut self, doc_id: u64, revision: u64) -> bool {
        doc_id == self.doc_id
            && self
                .model
                .as_mut()
                .is_some_and(|model| model.fail_measurements(revision))
    }

    /// Install out-of-band measurements if they still describe the current
    /// document revision. Returns true when adopted.
    pub fn adopt_measurements(
        &mut self,
        doc_id: u64,
        revision: u64,
        m: Vec<(BodyId, Measurements)>,
    ) -> bool {
        doc_id == self.doc_id
            && self
                .model
                .as_mut()
                .is_some_and(|model| model.adopt_measurements(revision, m))
    }

    fn fresh_model_settings(&mut self) {
        let defer = self.defer_measurements;
        if let Some(m) = &mut self.model {
            m.set_defer_measurements(defer);
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn model_mut(&mut self) -> Result<&mut Model, DocError> {
        self.model
            .as_mut()
            .ok_or(DocError::Validation("no document is open".into()))
    }

    /// Execute a command. Never panics on user input; failures leave state unchanged.
    pub fn execute(&mut self, cmd: &Command) -> Response {
        let mut r = Response {
            command: cmd.name().into(),
            ..Default::default()
        };
        match self.run(cmd, &mut r) {
            Ok(()) => r.ok = true,
            Err(e) => {
                r.ok = false;
                r.error = Some(e);
            }
        }
        if r.ok
            && !matches!(
                cmd,
                Command::Inspect | Command::Measure | Command::ExportStl { .. }
            )
        {
            r.measurements = self
                .model
                .as_ref()
                .and_then(|m| m.evaluation().measurements.clone());
        }
        r
    }

    fn run(&mut self, cmd: &Command, r: &mut Response) -> Result<(), DocError> {
        match cmd {
            Command::NewDocument => {
                self.model = Some(Model::new());
                self.path = None;
                self.doc_id += 1;
                self.fresh_model_settings();
            }
            Command::CreateSketch {
                name,
                plane,
                template,
            } => {
                let v = |r: Result<Sketch, f420_sketch::SketchError>| {
                    r.map_err(|e| DocError::Validation(e.to_string()))
                };
                let (sketch, center) = match template {
                    SketchTemplate::CenteredRectangle {
                        width,
                        height,
                        center,
                    } => (v(Sketch::centered_rectangle(*width, *height))?, *center),
                    SketchTemplate::CenteredCircle { diameter, center } => {
                        (v(Sketch::centered_circle(*diameter))?, *center)
                    }
                    SketchTemplate::Polygon { vertices } => {
                        (solved(v(polygon_sketch(vertices))?)?, None)
                    }
                    SketchTemplate::Custom { sketch } => (solved(sketch.clone())?, None),
                };
                let mut sketch = sketch;
                if let Some([cx, cy]) = center {
                    if !(cx.is_finite() && cy.is_finite()) {
                        return Err(DocError::Validation(format!(
                            "center must be finite, got [{cx}, {cy}]"
                        )));
                    }
                    for c in &mut sketch.constraints {
                        if let f420_sketch::Constraint::FixedPoint { x, y, .. } = c {
                            (*x, *y) = (cx, cy);
                        }
                    }
                }
                let (name, plane) = (name.clone(), *plane);
                let id = self.model_mut()?.transact("Create sketch", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Sketch"));
                    Ok(d.push(n, FeatureKind::Sketch { plane, sketch }))
                })?;
                r.feature = Some(id);
            }
            Command::Extrude {
                name,
                sketch,
                distance,
                op,
                target,
                through_all,
                reverse,
                body_name,
            } => {
                let (name, sketch, distance, reverse) =
                    (name.clone(), *sketch, *distance, *reverse);
                let extent = if *through_all {
                    if *op != ExtrudeOp::Cut {
                        return Err(DocError::Validation(
                            "'through all' is only available for Cut".into(),
                        ));
                    }
                    Extent::ThroughAll
                } else {
                    if !distance.is_finite() || distance <= 0.0 {
                        return Err(DocError::Validation(format!(
                            "extrusion distance must be > 0 mm, got {distance}"
                        )));
                    }
                    Extent::Distance
                };
                let m = self.model_mut()?;
                let target = match op {
                    ExtrudeOp::NewBody => None,
                    _ => Some(match target {
                        Some(t) => *t,
                        None => default_target(m, sketch)?,
                    }),
                };
                let body_name = body_name.clone();
                let id = m.transact("Extrude", |d| {
                    let operation = match (op, target) {
                        (ExtrudeOp::Join, Some(target)) => Operation::Join { target },
                        (ExtrudeOp::Cut, Some(target)) => Operation::Cut { target },
                        _ => {
                            let n = match &body_name {
                                Some(n) => valid_name(n, "body")?,
                                None => d.default_body_name(),
                            };
                            if d.bodies.iter().any(|b| b.name == n) {
                                return Err(DocError::Validation(format!(
                                    "another body is already named '{n}'"
                                )));
                            }
                            Operation::NewBody {
                                body: d.add_body(n),
                            }
                        }
                    };
                    let n = name.unwrap_or_else(|| d.default_name("Extrude"));
                    Ok(d.push(
                        n,
                        FeatureKind::Extrude {
                            sketch,
                            distance: if extent == Extent::ThroughAll {
                                0.0
                            } else {
                                distance
                            },
                            operation,
                            extent,
                            reverse,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::Revolve {
                name,
                sketch,
                angle_deg,
            } => {
                let (name, sketch, a) = (name.clone(), *sketch, *angle_deg);
                if !a.is_finite() || a <= 0.0 || a > 360.0 {
                    return Err(DocError::Validation(format!(
                        "revolve angle must be in (0, 360] degrees, got {a}"
                    )));
                }
                let id = self.model_mut()?.transact("Revolve", |d| {
                    let body = d.add_body(d.default_body_name());
                    let n = name.unwrap_or_else(|| d.default_name("Revolve"));
                    Ok(d.push(
                        n,
                        FeatureKind::Revolve {
                            sketch,
                            angle: a.to_radians(),
                            body,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::CutThroughAll { name, sketch } => {
                let (name, sketch) = (name.clone(), *sketch);
                let m = self.model_mut()?;
                let target = default_target(m, sketch)?;
                let id = m.transact("Cut", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Cut"));
                    Ok(d.push(
                        n,
                        FeatureKind::Cut {
                            sketch,
                            extent: f420_document::CutExtent::ThroughAll,
                            target,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::Pocket {
                name,
                sketch,
                depth,
            } => {
                let (name, sketch, depth) = (name.clone(), *sketch, *depth);
                if !depth.is_finite() || depth <= 0.0 {
                    return Err(DocError::Validation(format!(
                        "pocket depth must be > 0 mm, got {depth}"
                    )));
                }
                let m = self.model_mut()?;
                let target = default_target(m, sketch)?;
                let id = m.transact("Pocket", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Pocket"));
                    Ok(d.push(
                        n,
                        FeatureKind::Pocket {
                            sketch,
                            depth,
                            target,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::LinearPattern {
                name,
                feature,
                count,
                spacing,
                direction,
            } => {
                let (name, seed, count, spacing, direction) =
                    (name.clone(), *feature, *count, *spacing, *direction);
                let id = self.model_mut()?.transact("Pattern", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Pattern"));
                    Ok(d.push(
                        n,
                        FeatureKind::LinearPattern {
                            seed,
                            count,
                            spacing,
                            direction,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::CircularPattern {
                name,
                feature,
                count,
                angle_deg,
                center,
            } => {
                let (name, seed, count, a, center) =
                    (name.clone(), *feature, *count, *angle_deg, *center);
                if !a.is_finite() || a <= 0.0 || a >= 360.0 {
                    return Err(DocError::Validation(format!(
                        "angle between instances must be in (0, 360) degrees, got {a}"
                    )));
                }
                let id = self.model_mut()?.transact("Circular pattern", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Circular pattern"));
                    Ok(d.push(
                        n,
                        FeatureKind::CircularPattern {
                            seed,
                            count,
                            angle: a.to_radians(),
                            center,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::Mirror {
                name,
                feature,
                plane,
            } => {
                let (name, seed, plane) = (name.clone(), *feature, *plane);
                let id = self.model_mut()?.transact("Mirror", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Mirror"));
                    Ok(d.push(n, FeatureKind::Mirror { seed, plane }))
                })?;
                r.feature = Some(id);
            }
            Command::Chamfer {
                name,
                a,
                b,
                distance,
            } => {
                let (name, a, b, distance) = (name.clone(), *a, *b, *distance);
                if !distance.is_finite() || distance <= 0.0 {
                    return Err(DocError::Validation(format!(
                        "chamfer distance must be > 0 mm, got {distance}"
                    )));
                }
                let id = self.model_mut()?.transact("Chamfer", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Chamfer"));
                    Ok(d.push(n, FeatureKind::Chamfer { a, b, distance }))
                })?;
                r.feature = Some(id);
            }
            Command::SetParameter {
                feature,
                name,
                value,
            } => {
                let c = self.model_mut()?.set_parameter(*feature, name, *value)?;
                r.feature = Some(*feature);
                r.message = Some(format!("{}.{}: {} -> {}", c.feature, c.name, c.old, c.new));
            }
            Command::EditSketch { feature, sketch } => {
                let (feature, new) = (*feature, solved(sketch.clone())?);
                let m = self.model_mut()?;
                let name = m.document().feature(feature)?.name.clone();
                m.transact(&format!("Edit sketch {name}"), |d| {
                    match &mut d.feature_mut(feature)?.kind {
                        FeatureKind::Sketch { sketch, .. } => {
                            *sketch = keep_entity_ids(sketch, new)?;
                            Ok(())
                        }
                        _ => Err(DocError::Validation(format!("{feature} is not a sketch"))),
                    }
                })?;
                r.feature = Some(feature);
            }
            Command::DeleteFeature { feature } => {
                let feature = *feature;
                let m = self.model_mut()?;
                let doc = m.document();
                let name = doc.feature(feature)?.name.clone();
                // Explicit references plus Join/Cut targets of bodies it creates.
                let dependents: Vec<String> = doc
                    .dependents_of(feature)
                    .into_iter()
                    .filter_map(|id| doc.feature(id).ok())
                    .map(|f| format!("{} {}", f.id, f.name))
                    .collect();
                if !dependents.is_empty() {
                    return Err(DocError::Validation(format!(
                        "{name} is used by {}; delete those first",
                        dependents.join(", ")
                    )));
                }
                m.transact(&format!("Delete {name}"), |d| {
                    d.features.retain(|f| f.id != feature);
                    d.prune_bodies();
                    d.prune_blobs();
                    Ok(())
                })?;
                r.message = Some(format!("deleted {feature} {name}"));
            }
            Command::RenameBody { body, name } => {
                let (body, name) = (*body, valid_name(name, "body")?);
                let m = self.model_mut()?;
                if m.document()
                    .bodies
                    .iter()
                    .any(|b| b.id != body && b.name == name)
                {
                    return Err(DocError::Validation(format!(
                        "another body is already named '{name}'"
                    )));
                }
                let old = m.document().body(body)?.name.clone();
                m.transact(&format!("Rename {old} to {name}"), |d| {
                    d.body_mut(body)?.name = name.clone();
                    Ok(())
                })?;
            }
            Command::SetBodyVisible { body, visible } => {
                let (body, visible) = (*body, *visible);
                let m = self.model_mut()?;
                let name = m.document().body(body)?.name.clone();
                m.transact(
                    &format!("{} {name}", if visible { "Show" } else { "Hide" }),
                    |d| {
                        d.body_mut(body)?.visible = visible;
                        Ok(())
                    },
                )?;
            }
            Command::SetSketchPlane { feature, plane } => {
                let (feature, plane) = (*feature, *plane);
                let m = self.model_mut()?;
                let name = m.document().feature(feature)?.name.clone();
                m.transact(&format!("Re-attach {name}"), |d| {
                    match &mut d.feature_mut(feature)?.kind {
                        FeatureKind::Sketch { plane: p, .. } => {
                            *p = plane;
                            Ok(())
                        }
                        _ => Err(DocError::Validation(format!("{feature} is not a sketch"))),
                    }
                })?;
                r.feature = Some(feature);
            }
            Command::ImportStep { path, units_to_mm } => {
                let bytes = read_import(path)?;
                let unit = *units_to_mm;
                // Parse once up front for body names and an early, precise error.
                let imp = f420_document::import::step(&bytes, unit)?;
                if imp.solids.is_empty() {
                    return Err(DocError::Validation(format!(
                        "{}: no solids found",
                        path.display()
                    )));
                }
                let names: Vec<Option<String>> =
                    imp.solids.iter().map(|s| s.name.clone()).collect();
                let file = file_name(path);
                let stem = file_stem(path);
                let notes = imp.notes;
                let id = self.model_mut()?.transact(&format!("Import {file}"), |d| {
                    let blob = d.add_blob(&file, &bytes);
                    let mut bodies = Vec::new();
                    for (i, n) in names.iter().enumerate() {
                        let base = n
                            .as_deref()
                            .map(str::trim)
                            .filter(|n| !n.is_empty())
                            .map(str::to_string)
                            .unwrap_or_else(|| {
                                if names.len() == 1 {
                                    stem.clone()
                                } else {
                                    format!("{stem}-{}", i + 1)
                                }
                            });
                        let name = unique_body_name(d, &base);
                        bodies.push(d.add_body(name));
                    }
                    let n = d.default_name("Import");
                    Ok(d.push(
                        n,
                        FeatureKind::ImportStep {
                            blob,
                            unit_override_to_mm: unit,
                            bodies,
                        },
                    ))
                })?;
                r.feature = Some(id);
                r.message = Some(notes.join("; "));
            }
            Command::ImportStl { path, units_to_mm } => {
                let to_mm = units_to_mm.ok_or_else(|| {
                    DocError::Validation(
                        "STL files have no units: choose the file unit (units_to_mm, e.g. 1 for \
                         mm, 25.4 for inches)"
                            .into(),
                    )
                })?;
                if !to_mm.is_finite() || to_mm <= 0.0 {
                    return Err(DocError::Validation(format!(
                        "units_to_mm must be > 0, got {to_mm}"
                    )));
                }
                let bytes = read_import(path)?;
                let (_, notes) = f420_document::import::stl(&bytes, to_mm)?;
                let file = file_name(path);
                let stem = file_stem(path);
                let id = self.model_mut()?.transact(&format!("Import {file}"), |d| {
                    let blob = d.add_blob(&file, &bytes);
                    let body = d.add_body(unique_body_name(d, &stem));
                    let n = d.default_name("Import");
                    Ok(d.push(n, FeatureKind::ImportMesh { blob, to_mm, body }))
                })?;
                r.feature = Some(id);
                r.message = Some(notes.join("; "));
            }
            Command::ImportDxf {
                path,
                plane,
                name,
                units_to_mm,
                allow_partial,
                layers,
            } => {
                let bytes = read_import(path)?;
                let opts = f420_exchange::dxf::DxfImportOptions {
                    layers: layers.clone(),
                    allow_partial: *allow_partial,
                    unit_override_to_mm: *units_to_mm,
                    ..Default::default()
                };
                let imp = f420_exchange::dxf::import(&bytes, &opts)
                    .map_err(|e| DocError::Validation(e.to_string()))?;
                let mut notes = vec![format!(
                    "units: {} ({} mm per unit, {})",
                    imp.units.name,
                    imp.units.to_mm,
                    if imp.units.declared {
                        "declared in file"
                    } else {
                        "chosen at import"
                    }
                )];
                notes.push(format!(
                    "imported {}",
                    imp.imported
                        .iter()
                        .map(|c| format!("{} x{}", c.entity_type, c.count))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                for sk in &imp.skipped {
                    notes.push(format!(
                        "skipped {} x{} ({})",
                        sk.entity_type, sk.count, sk.reason
                    ));
                }
                // Degenerate entities are listed in both `skipped` and `warnings`.
                notes.extend(
                    imp.warnings
                        .iter()
                        .filter(|w| !w.contains(": dropped "))
                        .map(|w| format!("warning: {w}")),
                );
                let (plane, name, sketch) = (*plane, name.clone(), imp.sketch);
                let stem = file_stem(path);
                let kept = notes.clone();
                let id = self
                    .model_mut()?
                    .transact(&format!("Import {stem}.dxf"), |d| {
                        let n = name.unwrap_or_else(|| d.default_name(&format!("{stem}-")));
                        let id = d.push(n, FeatureKind::Sketch { plane, sketch });
                        d.feature_mut(id)?.notes = kept;
                        Ok(id)
                    })?;
                r.feature = Some(id);
                r.message = Some(notes.join("; "));
            }
            Command::ImportSvg {
                path,
                plane,
                name,
                units_to_mm,
                allow_partial,
            } => {
                let bytes = read_import(path)?;
                let opts = f420_exchange::svg::SvgImportOptions {
                    allow_partial: *allow_partial,
                    unit_override_to_mm: *units_to_mm,
                    ..Default::default()
                };
                let imp = f420_exchange::svg::import(&bytes, &opts)
                    .map_err(|e| DocError::Validation(e.to_string()))?;
                let mut notes = vec![format!(
                    "units: {} ({} mm per user unit, {})",
                    imp.units.name,
                    imp.units.to_mm,
                    if imp.units.declared {
                        "declared in file"
                    } else {
                        "chosen at import"
                    }
                )];
                notes.push(format!(
                    "imported {}",
                    imp.imported
                        .iter()
                        .map(|c| format!("{} x{}", c.entity_type, c.count))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                for sk in &imp.skipped {
                    notes.push(format!(
                        "skipped {} x{} ({})",
                        sk.entity_type, sk.count, sk.reason
                    ));
                }
                // Degenerate entities are listed in both `skipped` and `warnings`.
                notes.extend(
                    imp.warnings
                        .iter()
                        .filter(|w| !w.contains(": dropped "))
                        .map(|w| format!("warning: {w}")),
                );
                let (plane, name, sketch) = (*plane, name.clone(), imp.sketch);
                let stem = file_stem(path);
                let kept = notes.clone();
                let id = self
                    .model_mut()?
                    .transact(&format!("Import {stem}.svg"), |d| {
                        let n = name.unwrap_or_else(|| d.default_name(&format!("{stem}-")));
                        let id = d.push(n, FeatureKind::Sketch { plane, sketch });
                        d.feature_mut(id)?.notes = kept;
                        Ok(id)
                    })?;
                r.feature = Some(id);
                r.message = Some(notes.join("; "));
            }
            Command::ExportSvg { path, sketch } => {
                let m = self.model_mut()?;
                let solved = m
                    .evaluation()
                    .features
                    .iter()
                    .find(|f| f.id == *sketch)
                    .and_then(|f| f.solved_sketch.clone())
                    .ok_or_else(|| DocError::Validation(format!("{sketch} is not a sketch")))?;
                let text = f420_exchange::svg::export(&solved)
                    .map_err(|e| DocError::Validation(e.to_string()))?;
                write_atomic(path, text.as_bytes())?;
                r.message = Some(format!(
                    "wrote {} ({} bytes, SVG, mm, local sketch coordinates with y negated)",
                    path.display(),
                    text.len()
                ));
            }
            Command::ExportDxf { path, sketch } => {
                let m = self.model_mut()?;
                let solved = m
                    .evaluation()
                    .features
                    .iter()
                    .find(|f| f.id == *sketch)
                    .and_then(|f| f.solved_sketch.clone())
                    .ok_or_else(|| DocError::Validation(format!("{sketch} is not a sketch")))?;
                let text = f420_exchange::dxf::export(&solved, &Default::default())
                    .map_err(|e| DocError::Validation(e.to_string()))?;
                write_atomic(path, text.as_bytes())?;
                r.message = Some(format!(
                    "wrote {} ({} bytes, R12 ASCII, mm)",
                    path.display(),
                    text.len()
                ));
            }
            Command::RenameFeature { feature, name } => {
                let (feature, name) = (*feature, name.trim().to_string());
                if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
                    return Err(DocError::Validation(
                        "feature name must be 1-128 printable characters".into(),
                    ));
                }
                let m = self.model_mut()?;
                if m.document()
                    .features
                    .iter()
                    .any(|f| f.id != feature && f.name == name)
                {
                    return Err(DocError::Validation(format!(
                        "another feature is already named '{name}'"
                    )));
                }
                let old = m.document().feature(feature)?.name.clone();
                m.transact(&format!("Rename {old} to {name}"), |d| {
                    d.feature_mut(feature)?.name = name.clone();
                    Ok(())
                })?;
                r.feature = Some(feature);
            }
            Command::Undo => r.message = Some(format!("undid: {}", self.model_mut()?.undo()?)),
            Command::Redo => r.message = Some(format!("redid: {}", self.model_mut()?.redo()?)),
            Command::Save { path } => {
                let m = self.model_mut()?;
                m.complete_measurements()?;
                save_file(path, m.document(), Some(m.evaluation()))?;
                m.dirty = false;
                self.path = Some(path.clone());
                r.message = Some(format!("saved {}", path.display()));
            }
            Command::Open { path } => {
                let nf = load_file(path)?;
                let model = Model::from_document(nf.document)?;
                let mut msg = format!("opened {}", path.display());
                if let (Some(c), Some(m)) = (&nf.cache, &model.evaluation().measurements)
                    && let (Some(v0), k::VolumeResult::Value { volume, .. }) = (c.volume, &m.volume)
                {
                    msg += &format!("; regenerated volume {volume:.9} vs saved cache {v0:.9}");
                }
                self.model = Some(model);
                self.path = Some(path.clone());
                self.doc_id += 1;
                self.fresh_model_settings();
                r.message = Some(msg);
            }
            Command::Close => {
                self.model_mut()?;
                self.model = None;
                self.path = None;
                self.doc_id += 1;
            }
            Command::ExportStl {
                path,
                chord,
                bodies,
            } => {
                let chord = chord.unwrap_or(k::tol::MESH_CHORD);
                let m = self.model_mut()?;
                let ids = export_selection(m, bodies.as_deref())?;
                let meshes = export_meshes(m, &ids, chord, r)?;
                let mut all = k::TriMesh {
                    positions: vec![],
                    normals: vec![],
                    triangles: vec![],
                    tri_face: vec![],
                    chord_tolerance: chord,
                };
                for (_, mesh) in &meshes {
                    let base = all.positions.len() as u32;
                    all.positions.extend(&mesh.positions);
                    all.normals.extend(&mesh.normals);
                    all.triangles.extend(
                        mesh.triangles
                            .iter()
                            .map(|t| [t[0] + base, t[1] + base, t[2] + base]),
                    );
                    all.tri_face.extend(&mesh.tri_face);
                }
                let bytes = k::write_stl_binary(&all, "Fission420");
                write_atomic(path, &bytes)?;
                r.measurements = m.evaluation().measurements.clone();
                r.message = Some(format!(
                    "wrote {} ({} triangles, {} bod{}, chord {chord} mm) from committed revision {}",
                    path.display(),
                    all.triangles.len(),
                    ids.len(),
                    if ids.len() == 1 { "y" } else { "ies" },
                    m.revision
                ));
            }
            Command::Export3mf {
                path,
                chord,
                bodies,
                unit,
            } => {
                use f420_exchange::threemf;
                let chord = chord.unwrap_or(k::tol::MESH_CHORD);
                let unit = unit.unwrap_or(threemf::ThreeMfUnit::Millimeter);
                let m = self.model_mut()?;
                let ids = export_selection(m, bodies.as_deref())?;
                let meshes = export_meshes(m, &ids, chord, r)?;
                let objects: Vec<threemf::MeshObject> = meshes
                    .into_iter()
                    .map(|(name, mesh)| threemf::MeshObject {
                        name: Some(name),
                        positions: mesh.positions,
                        triangles: mesh.triangles,
                        color: None,
                    })
                    .collect();
                let bytes = threemf::write(&objects, unit)
                    .map_err(|e| DocError::Validation(e.to_string()))?;
                write_atomic(path, &bytes)?;
                r.message = Some(format!(
                    "wrote {} ({} object{}, unit {}, chord {chord} mm) from committed revision {}",
                    path.display(),
                    objects.len(),
                    if objects.len() == 1 { "" } else { "s" },
                    unit.name(),
                    m.revision
                ));
            }
            Command::Import3mf { path } => {
                let bytes = read_import(path)?;
                let (objects, notes) = f420_document::import::threemf(&bytes)?;
                if objects.is_empty() {
                    return Err(DocError::Validation(format!(
                        "{}: no build items",
                        path.display()
                    )));
                }
                let names: Vec<Option<String>> = objects.iter().map(|o| o.name.clone()).collect();
                let file = file_name(path);
                let stem = file_stem(path);
                let id = self.model_mut()?.transact(&format!("Import {file}"), |d| {
                    let blob = d.add_blob(&file, &bytes);
                    let mut bodies = Vec::new();
                    for (i, n) in names.iter().enumerate() {
                        let base = n
                            .as_deref()
                            .map(str::trim)
                            .filter(|n| !n.is_empty())
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("{stem}-{}", i + 1));
                        let name = unique_body_name(d, &base);
                        bodies.push(d.add_body(name));
                    }
                    let n = d.default_name("Import");
                    Ok(d.push(n, FeatureKind::Import3mf { blob, bodies }))
                })?;
                r.feature = Some(id);
                r.message = Some(notes.join("; "));
            }
            Command::ExportStep { path, bodies } => {
                let m = self.model_mut()?;
                let ev = m.evaluation();
                let mut ids = export_selection(m, bodies.as_deref())?;
                // Default selection: visible B-rep bodies; meshes are named, not
                // silently dropped. An explicit selection with a mesh is refused.
                let mut skipped = Vec::new();
                if bodies.is_none() {
                    ids.retain(|id| match ev.body_by_id(*id) {
                        Some(b) if b.geometry.mesh().is_some() => {
                            skipped.push(b.name.clone());
                            false
                        }
                        _ => true,
                    });
                    if ids.is_empty() {
                        return Err(DocError::Validation(format!(
                            "no visible B-rep body to export (mesh bodies cannot be written \
                             to STEP: {})",
                            skipped.join(", ")
                        )));
                    }
                }
                let mut named = Vec::new();
                for id in &ids {
                    let b = ev.body_by_id(*id).expect("selection checked");
                    let body = b.geometry.brep().ok_or_else(|| {
                        DocError::Validation(format!(
                            "{} is a mesh (STL) body; STEP export needs exact B-rep bodies",
                            b.name
                        ))
                    })?;
                    named.push((b.name.as_str(), body));
                }
                let text = k::export_step_bodies(&named, "Fission420")
                    .map_err(|e| DocError::Validation(e.to_string()))?;
                write_atomic(path, text.as_bytes())?;
                r.message = Some(format!(
                    "wrote {} ({} bytes, {} solid{}) from committed revision {}{}",
                    path.display(),
                    text.len(),
                    ids.len(),
                    if ids.len() == 1 { "" } else { "s" },
                    m.revision,
                    if skipped.is_empty() {
                        String::new()
                    } else {
                        format!("; mesh bodies not written: {}", skipped.join(", "))
                    }
                ));
            }
            Command::Measure => {
                let m = self.model_mut()?;
                m.complete_measurements()?;
                r.measurements = m.evaluation().measurements.clone();
            }
            Command::Inspect => {}
        }
        r.summary = Some(self.summary());
        Ok(())
    }

    pub fn summary(&self) -> DocSummary {
        let Some(m) = &self.model else {
            return DocSummary {
                doc_id: self.doc_id,
                open: false,
                measuring: false,
                path: None,
                dirty: false,
                revision: 0,
                can_undo: None,
                can_redo: None,
                features: vec![],
                bodies: vec![],
            };
        };
        DocSummary {
            doc_id: self.doc_id,
            open: true,
            measuring: m.measurements_pending(),
            path: self.path.clone(),
            dirty: m.dirty,
            revision: m.revision,
            can_undo: m.undo_label().map(Into::into),
            can_redo: m.redo_label().map(Into::into),
            features: summarize(m.document(), m),
            bodies: m
                .evaluation()
                .bodies
                .iter()
                .map(|b| BodySummary {
                    id: b.id,
                    name: b.name.clone(),
                    visible: b.visible,
                    kind: match b.geometry {
                        BodyGeometry::Brep(_) => "brep",
                        BodyGeometry::Mesh(_) => "mesh",
                    }
                    .into(),
                    source: b.source.clone(),
                    created_by: b.created_by,
                    last_feature: b.last_feature,
                    measurements: b.measurements.clone(),
                    mesh: b.geometry.mesh().map(|mb| mb.diagnostics.clone()),
                })
                .collect(),
        }
    }
}

fn summarize(doc: &Document, m: &Model) -> Vec<FeatureSummary> {
    let ev = m.evaluation();
    doc.features
        .iter()
        .map(|f| {
            let res = ev.features.iter().find(|r| r.id == f.id);
            let (kind, plane) = match &f.kind {
                FeatureKind::Sketch { plane, .. } => ("sketch", Some(*plane)),
                FeatureKind::Extrude { .. } => ("extrude", None),
                FeatureKind::Revolve { .. } => ("revolve", None),
                FeatureKind::Cut { .. } => ("cut_through_all", None),
                FeatureKind::Pocket { .. } => ("pocket", None),
                FeatureKind::ImportStep { .. } => ("import_step", None),
                FeatureKind::ImportMesh { .. } => ("import_mesh", None),
                FeatureKind::Import3mf { .. } => ("import_3mf", None),
                FeatureKind::LinearPattern { .. } => ("linear_pattern", None),
                FeatureKind::CircularPattern { .. } => ("circular_pattern", None),
                FeatureKind::Mirror { plane, .. } => ("mirror", Some(*plane)),
                FeatureKind::Chamfer { .. } => ("chamfer", None),
            };
            let (operation, extent, reverse) = match &f.kind {
                FeatureKind::Extrude {
                    operation,
                    extent,
                    reverse,
                    ..
                } => (Some(*operation), Some(*extent), *reverse),
                _ => (None, None, false),
            };
            FeatureSummary {
                id: f.id,
                name: f.name.clone(),
                kind: kind.into(),
                parameters: f.parameters(),
                depends_on: doc.dependencies_of(f),
                plane,
                solve_status: res.and_then(|r| r.solve.as_ref()).map(|s| s.status),
                solve_dof: res.and_then(|r| r.solve.as_ref()).map(|s| s.dof),
                solve_residual: res.and_then(|r| r.solve.as_ref()).map(|s| s.residual),
                frame: res.and_then(|r| r.frame),
                solve_conflicts: match (&f.kind, res.and_then(|r| r.solve.as_ref())) {
                    (FeatureKind::Sketch { sketch, .. }, Some(rep)) => rep
                        .conflicting
                        .iter()
                        .map(|&i| sketch.constraint_label(i))
                        .collect(),
                    _ => vec![],
                },
                sketch: res.and_then(|r| r.solved_sketch.clone()),
                profile_error: res.and_then(|r| r.profile_error.clone()),
                faces: ev
                    .faces
                    .iter()
                    .filter(|c| c.reference.feature == f.id)
                    .map(face_summary)
                    .collect(),
                operation,
                extent,
                reverse,
                body: res.and_then(|r| r.body),
                creates: f.created_bodies(),
                notes: f
                    .notes
                    .iter()
                    .cloned()
                    .chain(res.map(|r| r.notes.clone()).unwrap_or_default())
                    .collect(),
            }
        })
        .collect()
}

fn face_summary(c: &FaceCandidate) -> FaceSummary {
    FaceSummary {
        reference: c.reference,
        frame: c.frame,
        body: c.body,
    }
}

/// Target body for a Join/Cut when none was given: the body owning the face
/// the sketch is on, else the only B-rep body. Never guesses among several.
fn default_target(m: &Model, sketch: FeatureId) -> Result<BodyId, DocError> {
    let ev = m.evaluation();
    if let Some(rf) = ev
        .features
        .iter()
        .find(|r| r.id == sketch)
        .and_then(|r| r.resolved_face.as_ref())
    {
        return Ok(rf.body);
    }
    let breps: Vec<_> = ev
        .bodies
        .iter()
        .filter(|b| b.geometry.brep().is_some())
        .collect();
    match breps.as_slice() {
        [one] => Ok(one.id),
        [] => Err(DocError::Validation(
            "there is no solid body to join to or cut".into(),
        )),
        many => Err(DocError::Validation(format!(
            "choose a target body ({})",
            many.iter()
                .map(|b| format!("{} {}", b.id, b.name))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Triangle meshes of bodies `ids` for mesh exports: B-rep bodies tessellated at
/// `chord` and required watertight (the failing check is put on `r`), mesh
/// bodies as imported. Returns (body name, mesh) in `ids` order.
fn export_meshes(
    m: &Model,
    ids: &[BodyId],
    chord: f64,
    r: &mut Response,
) -> Result<Vec<(String, k::TriMesh)>, DocError> {
    let ev = m.evaluation();
    let mut out = Vec::with_capacity(ids.len());
    let mut checks = Vec::new();
    for id in ids {
        let b = ev
            .body_by_id(*id)
            .ok_or_else(|| DocError::Validation(format!("unknown body {id}")))?;
        let mesh = match &b.geometry {
            BodyGeometry::Brep(body) => {
                let mesh =
                    k::tessellate(body, chord).map_err(|e| DocError::Validation(e.to_string()))?;
                let check = k::check_mesh(&mesh);
                if !check.watertight {
                    r.mesh_check = Some(check);
                    return Err(DocError::Validation(format!(
                        "tessellation of {} is not watertight; nothing written",
                        b.name
                    )));
                }
                checks.push(check);
                mesh
            }
            BodyGeometry::Mesh(mb) => mb.mesh.clone(),
        };
        out.push((b.name.clone(), mesh));
    }
    if ids.len() == 1 {
        r.mesh_check = checks.pop();
    }
    Ok(out)
}

/// Bodies to export: the given ids (must exist), else every visible body.
fn export_selection(m: &Model, ids: Option<&[BodyId]>) -> Result<Vec<BodyId>, DocError> {
    let ev = m.evaluation();
    let ids: Vec<BodyId> = match ids {
        Some(ids) => {
            for id in ids {
                if ev.body_by_id(*id).is_none() {
                    return Err(DocError::Validation(format!("unknown body {id}")));
                }
            }
            ids.to_vec()
        }
        None => ev
            .bodies
            .iter()
            .filter(|b| b.visible)
            .map(|b| b.id)
            .collect(),
    };
    if ids.is_empty() {
        return Err(DocError::Validation(
            "document has no (visible) body to export".into(),
        ));
    }
    Ok(ids)
}

/// 1-128 printable characters, trimmed.
fn valid_name(name: &str, what: &str) -> Result<String, DocError> {
    let name = name.trim().to_string();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(DocError::Validation(format!(
            "{what} name must be 1-128 printable characters"
        )));
    }
    Ok(name)
}

fn unique_body_name(d: &Document, base: &str) -> String {
    let base: String = base.chars().filter(|c| !c.is_control()).take(100).collect();
    let base = if base.trim().is_empty() {
        "Body".to_string()
    } else {
        base
    };
    if !d.bodies.iter().any(|b| b.name == base) {
        return base;
    }
    (2..)
        .map(|i| format!("{base} ({i})"))
        .find(|n| !d.bodies.iter().any(|b| &b.name == n))
        .unwrap_or(base)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "import".into())
}

fn file_stem(p: &Path) -> String {
    p.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "import".into())
}

/// Read an import source, bounded by the native file limit (it is embedded).
fn read_import(path: &Path) -> Result<Vec<u8>, DocError> {
    let ferr = |e: std::io::Error| DocError::File(format!("open {}: {e}", path.display()));
    let len = std::fs::metadata(path).map_err(ferr)?.len();
    let max = f420_document::MAX_FILE_BYTES / 2;
    if len > max {
        return Err(DocError::File(format!(
            "{}: {len} bytes exceeds the import limit of {max} bytes",
            path.display()
        )));
    }
    std::fs::read(path).map_err(ferr)
}

/// Closed loop of lines through `vertices` with shared endpoints.
pub fn polygon_sketch(vertices: &[[f64; 2]]) -> Result<Sketch, f420_sketch::SketchError> {
    if vertices.len() < 3 {
        return Err(f420_sketch::SketchError::UnsupportedProfile(format!(
            "a polygon needs at least 3 vertices, got {}",
            vertices.len()
        )));
    }
    let mut s = Sketch::default();
    s.ensure_origin();
    let ids: Vec<_> = vertices.iter().map(|v| s.add_point(v[0], v[1])).collect();
    for i in 0..ids.len() {
        s.add_line(ids[i], ids[(i + 1) % ids.len()]);
    }
    s.validate()?;
    Ok(s)
}

/// Entity ids are persistent names (face references use them, ADR 0003): an
/// edit may keep, remove or add entities, but must not give an existing or
/// removed id to different geometry. Ids removed by the edit are retired.
fn keep_entity_ids(old: &Sketch, mut new: Sketch) -> Result<Sketch, DocError> {
    use f420_sketch::Entity;
    // An entity keeps its identity when its endpoints change (the editor can
    // reconnect a line), but not when its kind changes.
    let shape = |e: &Entity| match e {
        Entity::Line { .. } => 0,
        Entity::Circle { .. } => 1,
        _ => 2,
    };
    for e in &new.entities {
        let id = e.id();
        let reused = match old.entity(id) {
            Some(o) => shape(o) != shape(e),
            None => id.0 < old.retired_entity_ids,
        };
        if reused {
            return Err(DocError::Validation(format!(
                "sketch entity id {} is reused for different geometry; new entities need fresh \
                 ids (sketch-entity ids are persistent face names)",
                id.0
            )));
        }
    }
    let removed = old
        .entities
        .iter()
        .map(|e| e.id())
        .filter(|id| new.entity(*id).is_none())
        .map(|id| id.0.saturating_add(1))
        .max()
        .unwrap_or(0);
    new.retired_entity_ids = new
        .retired_entity_ids
        .max(old.retired_entity_ids)
        .max(removed);
    Ok(new)
}

/// Validate and solve a sketch, returning the solved definition. Refuses
/// sketches whose constraints conflict, naming the culprits.
pub fn solved(mut s: Sketch) -> Result<Sketch, DocError> {
    s.validate()
        .map_err(|e| DocError::Validation(format!("invalid sketch: {e}")))?;
    let original = s.clone();
    let rep = s.solve();
    if !rep.is_success() {
        let culprits: Vec<String> = rep
            .conflicting
            .iter()
            .map(|&i| original.constraint_label(i))
            .collect();
        return Err(DocError::Validation(format!(
            "sketch does not solve ({:?}); conflicting: {}",
            rep.status,
            if culprits.is_empty() {
                "not identified".into()
            } else {
                culprits.join("; ")
            }
        )));
    }
    Ok(s)
}

/// Convenience: the semantic reference to an extrusion's end cap ("top face").
pub fn end_cap_of(extrude: FeatureId) -> SketchPlane {
    SketchPlane::Face(FaceRef {
        feature: extrude,
        role: FaceRole::ExtrudeEndCap,
    })
}
