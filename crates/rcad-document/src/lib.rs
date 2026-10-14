//! Parametric document: feature history, semantic references, regeneration,
//! transactions with undo/redo, and the versioned native file format.
//!
//! The [`Document`] (feature definitions + parameters) is authoritative. Geometry
//! ([`Evaluation`]) is a disposable cache produced by [`regenerate`].

pub mod assembly;
mod blob;
mod combine;
pub mod direct;
pub mod drawing;
mod hole;
pub mod import;
mod io;
pub mod material;
mod model;
pub mod params;
mod regen;
mod session;
pub mod sheet;
pub mod surface;

pub use direct::ImportedFaceIds;
pub use io::{
    FORMAT_NAME, FORMAT_VERSION, LEGACY_FORMAT_NAMES, MAX_FILE_BYTES, MIN_FORMAT_VERSION,
    NativeFile, load_file, migrate_to_v3, parse_native, read_bounded, save_file, write_atomic,
};
pub use material::{DensityUnit, Material};
pub use model::*;
pub use params::{ParamUnit, Parameter, SlotKind};
pub use regen::{
    BodyGeometry, BodySource, EvalBody, Evaluation, FRAME_YZ, FaceCandidate, FeatureResult,
    MeshBody, RegenCache, ResolvedFace, projection_curves, regenerate, regenerate_cached,
    regenerate_cached_opts,
};
pub use session::{LoadReport, Model, ModelSnapshot, ParamChange};

use serde::{Deserialize, Serialize};

/// Structured, serializable error. A failing operation never changes the document.
#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail")]
pub enum DocError {
    #[error("validation error: {0}")]
    Validation(String),
    #[error("unknown feature {0}")]
    UnknownFeature(FeatureId),
    #[error("unknown parameter '{name}' on feature {feature}")]
    UnknownParameter { feature: FeatureId, name: String },
    #[error("broken reference from feature {from}: {detail}")]
    BrokenReference { from: FeatureId, detail: String },
    #[error("dependency cycle involving features {0:?}")]
    Cycle(Vec<FeatureId>),
    #[error("sketch {feature} did not solve: {detail}")]
    SketchSolve { feature: FeatureId, detail: String },
    #[error("product limit: {0}")]
    ProductLimit(String),
    #[error("kernel error in feature {feature}: {detail}")]
    Kernel { feature: FeatureId, detail: String },
    /// The kernel returned a result that failed the independent check against
    /// the nominal operation (wrong geometry refused before commit).
    #[error("feature {feature}: geometry check failed: {detail}")]
    GeometryCheck { feature: FeatureId, detail: String },
    /// A named parameter or bound expression is invalid (unknown name, cycle,
    /// division by zero, unit mismatch, non-finite value, ...).
    #[error("parameter {name}: {detail}")]
    Expression { name: String, detail: String },
    #[error("nothing to {0}")]
    NothingTo(String),
    #[error("file error: {0}")]
    File(String),
}

pub type Result<T> = std::result::Result<T, DocError>;
