//! Typed, serializable command API shared by the UI, the CLI, and tests.
//!
//! Every modeling action available in the app is a [`Command`]; [`Session::execute`]
//! returns a [`Response`] with feature ids, measurements, and structured errors.
//! See docs/COMMANDS.md.

pub mod acceptance;
pub mod scenarios;
pub mod sketches;

use f420_document::{
    DocError, Document, FaceRef, FaceRole, FeatureId, FeatureKind, Model, SketchPlane, load_file,
    save_file, write_atomic,
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
    Extrude {
        #[serde(default)]
        name: Option<String>,
        sketch: FeatureId,
        distance: f64,
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
    Undo,
    Redo,
    Save {
        path: PathBuf,
    },
    Open {
        path: PathBuf,
    },
    Close,
    ExportStl {
        path: PathBuf,
        #[serde(default)]
        chord: Option<f64>,
    },
    /// STEP (AP214 via truck-stepio) of the exact B-rep. Refused for geometry the
    /// exporter cannot write exactly (see f420_kernel::export_step).
    ExportStep {
        path: PathBuf,
    },
    Measure,
    Inspect,
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Command::NewDocument => "new_document",
            Command::CreateSketch { .. } => "create_sketch",
            Command::Extrude { .. } => "extrude",
            Command::CutThroughAll { .. } => "cut_through_all",
            Command::Revolve { .. } => "revolve",
            Command::SetParameter { .. } => "set_parameter",
            Command::EditSketch { .. } => "edit_sketch",
            Command::RenameFeature { .. } => "rename_feature",
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
    /// Extrusions: semantic faces that can host a sketch, with their frames.
    #[serde(default)]
    pub faces: Vec<FaceSummary>,
}

/// A semantically referenceable face and its current placement frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceSummary {
    pub reference: FaceRef,
    pub frame: Frame,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocSummary {
    /// Identity of the open document; changes on New/Open/Close so clients can
    /// discard state that belongs to a previous document.
    #[serde(default)]
    pub doc_id: u64,
    pub open: bool,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub revision: u64,
    pub can_undo: Option<String>,
    pub can_redo: Option<String>,
    pub features: Vec<FeatureSummary>,
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
}

/// An application session: at most one open document.
pub struct Session {
    model: Option<Model>,
    path: Option<PathBuf>,
    /// Bumped whenever the open document is replaced or closed.
    doc_id: u64,
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
        }
    }

    pub fn model(&self) -> Option<&Model> {
        self.model.as_ref()
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
            } => {
                let (name, sketch, distance) = (name.clone(), *sketch, *distance);
                if !distance.is_finite() || distance <= 0.0 {
                    return Err(DocError::Validation(format!(
                        "extrusion thickness must be > 0 mm, got {distance}"
                    )));
                }
                let id = self.model_mut()?.transact("Extrude", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Extrude"));
                    Ok(d.push(n, FeatureKind::Extrude { sketch, distance }))
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
                    let n = name.unwrap_or_else(|| d.default_name("Revolve"));
                    Ok(d.push(
                        n,
                        FeatureKind::Revolve {
                            sketch,
                            angle: a.to_radians(),
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::CutThroughAll { name, sketch } => {
                let (name, sketch) = (name.clone(), *sketch);
                let id = self.model_mut()?.transact("Cut", |d| {
                    let n = name.unwrap_or_else(|| d.default_name("Cut"));
                    Ok(d.push(
                        n,
                        FeatureKind::Cut {
                            sketch,
                            extent: f420_document::CutExtent::ThroughAll,
                        },
                    ))
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
                            *sketch = new;
                            Ok(())
                        }
                        _ => Err(DocError::Validation(format!("{feature} is not a sketch"))),
                    }
                })?;
                r.feature = Some(feature);
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
                r.message = Some(msg);
            }
            Command::Close => {
                self.model_mut()?;
                self.model = None;
                self.path = None;
                self.doc_id += 1;
            }
            Command::ExportStl { path, chord } => {
                let chord = chord.unwrap_or(k::tol::MESH_CHORD);
                let m = self.model_mut()?;
                let body = m.evaluation().body.as_ref().ok_or(DocError::Validation(
                    "document has no solid body to export".into(),
                ))?;
                let mesh =
                    k::tessellate(body, chord).map_err(|e| DocError::Validation(e.to_string()))?;
                let check = k::check_mesh(&mesh);
                if !check.watertight {
                    r.mesh_check = Some(check);
                    return Err(DocError::Validation(
                        "tessellation is not watertight; STL not written".into(),
                    ));
                }
                let bytes = k::write_stl_binary(&mesh, "body");
                write_atomic(path, &bytes)?;
                r.mesh_check = Some(check);
                r.measurements = m.evaluation().measurements.clone();
                r.message = Some(format!(
                    "wrote {} ({} triangles, chord {chord} mm) from committed revision {}",
                    path.display(),
                    mesh.triangles.len(),
                    m.revision
                ));
            }
            Command::ExportStep { path } => {
                let m = self.model_mut()?;
                let body = m.evaluation().body.as_ref().ok_or(DocError::Validation(
                    "document has no solid body to export".into(),
                ))?;
                let text = k::export_step(body, "Fission420")
                    .map_err(|e| DocError::Validation(e.to_string()))?;
                write_atomic(path, text.as_bytes())?;
                r.message = Some(format!(
                    "wrote {} ({} bytes) from committed revision {}",
                    path.display(),
                    text.len(),
                    m.revision
                ));
            }
            Command::Measure => {
                let m = self.model_mut()?;
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
                path: None,
                dirty: false,
                revision: 0,
                can_undo: None,
                can_redo: None,
                features: vec![],
            };
        };
        DocSummary {
            doc_id: self.doc_id,
            open: true,
            path: self.path.clone(),
            dirty: m.dirty,
            revision: m.revision,
            can_undo: m.undo_label().map(Into::into),
            can_redo: m.redo_label().map(Into::into),
            features: summarize(m.document(), m),
        }
    }
}

fn summarize(doc: &Document, m: &Model) -> Vec<FeatureSummary> {
    doc.features
        .iter()
        .map(|f| {
            let res = m.evaluation().features.iter().find(|r| r.id == f.id);
            let (kind, plane) = match &f.kind {
                FeatureKind::Sketch { plane, .. } => ("sketch", Some(*plane)),
                FeatureKind::Extrude { .. } => ("extrude", None),
                FeatureKind::Revolve { .. } => ("revolve", None),
                FeatureKind::Cut { .. } => ("cut_through_all", None),
            };
            FeatureSummary {
                id: f.id,
                name: f.name.clone(),
                kind: kind.into(),
                parameters: f.parameters(),
                depends_on: f.dependencies(),
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
                faces: extrude_faces(doc, m, f),
            }
        })
        .collect()
}

/// End/start cap frames of an evaluated extrusion (same derivation as the
/// regenerator's semantic face resolution: profile frame offset by distance).
fn extrude_faces(doc: &Document, m: &Model, f: &f420_document::Feature) -> Vec<FaceSummary> {
    let FeatureKind::Extrude { sketch, distance } = &f.kind else {
        return vec![];
    };
    let Some(start) = m
        .evaluation()
        .features
        .iter()
        .find(|r| r.id == *sketch)
        .and_then(|r| r.frame)
    else {
        return vec![];
    };
    if doc.feature(*sketch).is_err() {
        return vec![];
    }
    let flip = Frame {
        origin: start.origin,
        x: start.x,
        y: start.y.map(|c| -c),
        z: start.z.map(|c| -c),
    };
    vec![
        FaceSummary {
            reference: FaceRef {
                feature: f.id,
                role: FaceRole::ExtrudeEndCap,
            },
            frame: start.offset(*distance),
        },
        FaceSummary {
            reference: FaceRef {
                feature: f.id,
                role: FaceRole::ExtrudeStartCap,
            },
            frame: flip,
        },
    ]
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
