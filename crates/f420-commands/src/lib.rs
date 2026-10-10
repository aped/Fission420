//! Typed, serializable command API shared by the UI, the CLI, and tests.
//!
//! Every modeling action available in the app is a [`Command`]; [`Session::execute`]
//! returns a [`Response`] with feature ids, measurements, and structured errors.
//! See docs/COMMANDS.md.

pub mod acceptance;

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
    /// Dimensioned rectangle centred on the sketch origin (dimensions "width", "height").
    CenteredRectangle { width: f64, height: f64 },
    /// Circle centred on the sketch origin (dimension "diameter").
    CenteredCircle { diameter: f64 },
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
            Command::SetParameter { .. } => "set_parameter",
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocSummary {
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
            }
            Command::CreateSketch {
                name,
                plane,
                template,
            } => {
                let sketch = match template {
                    SketchTemplate::CenteredRectangle { width, height } => {
                        Sketch::centered_rectangle(*width, *height)
                    }
                    SketchTemplate::CenteredCircle { diameter } => {
                        Sketch::centered_circle(*diameter)
                    }
                }
                .map_err(|e| DocError::Validation(e.to_string()))?;
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
                r.message = Some(msg);
            }
            Command::Close => {
                self.model_mut()?;
                self.model = None;
                self.path = None;
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
                    "wrote {} ({} triangles, chord {chord} mm)",
                    path.display(),
                    mesh.triangles.len()
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
                r.message = Some(format!("wrote {} ({} bytes)", path.display(), text.len()));
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
            }
        })
        .collect()
}

/// Convenience: the semantic reference to an extrusion's end cap ("top face").
pub fn end_cap_of(extrude: FeatureId) -> SketchPlane {
    SketchPlane::Face(FaceRef {
        feature: extrude,
        role: FaceRole::ExtrudeEndCap,
    })
}
