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
pub mod health;
mod hole;
pub mod import;
mod io;
pub mod material;
pub mod mesh_edit;
mod model;
pub mod params;
mod regen;
mod session;
pub mod sheet;
pub mod surface;

pub use direct::ImportedFaceIds;
pub use health::{FeatureHealth, HealthState};
pub use io::{
    FORMAT_NAME, FORMAT_VERSION, LEGACY_FORMAT_NAMES, MAX_FILE_BYTES, MIN_FORMAT_VERSION,
    NativeFile, load_file, migrate_to_v3, parse_native, read_bounded, required_version, save_file,
    v9_feature, write_atomic,
};
pub use material::{DensityUnit, Material};
pub use model::*;
pub use params::{ParamUnit, Parameter, SlotKind};
pub use regen::{
    BodyGeometry, BodySource, EvalBody, Evaluation, FRAME_YZ, FaceCandidate, FeatureResult,
    MeshBody, RegenCache, ResolvedFace, SourceCurves, UNPROJECTABLE, panic_text, projection_curves,
    regenerate, regenerate_cached, regenerate_cached_opts, regenerate_partial,
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
    /// A cut splits its target into `pieces` separate solids, but only
    /// `assigned` extra body ids were given to the feature. Recoverable: the
    /// command that commits the cut assigns the missing ids and regenerates.
    #[error(
        "feature {feature}: the cut splits its target into {pieces} separate bodies, but only \
         {} were assigned when it was committed; edit the cut (or commit it again) to create \
         the new bodies",
        assigned + 1
    )]
    NeedsPieces {
        feature: FeatureId,
        pieces: usize,
        assigned: usize,
    },
    /// The regeneration was cancelled ([`request_cancel`]) between features;
    /// the edit is not committed and the model is unchanged.
    #[error("cancelled: the regeneration was stopped before {0}; the model is unchanged")]
    Cancelled(FeatureId),
    /// A file import was cancelled while it was read ([`request_cancel`]);
    /// nothing was imported and the model is unchanged.
    #[error("cancelled: the import of {0} was stopped; nothing was imported")]
    ImportCancelled(String),
    #[error("file error: {0}")]
    File(String),
    /// A defect inside the application (a caught panic) stopped the command.
    /// The document is left at the last committed revision.
    #[error("internal error in '{command}': {detail}")]
    Internal { command: String, detail: String },
}

pub type Result<T> = std::result::Result<T, DocError>;

/// Cooperative cancellation of the command in progress (`rcad_cancel`; the
/// flag is process-wide, one document worker per process): the UI thread sets
/// it, regeneration checks it before each feature (and between pattern
/// instances) and fails with [`DocError::Cancelled`], and the file readers poll
/// it in their loops (an import then fails with [`DocError::ImportCancelled`]),
/// so the transaction is not committed. The command engine clears it before
/// each command. A single kernel operation without a poll is not interrupted.
pub fn request_cancel() {
    rcad_cancel::request();
}

/// Clear a cancellation request (before starting a new command).
pub fn clear_cancel() {
    rcad_cancel::clear();
}

/// Whether cancellation was requested.
pub fn cancel_requested() -> bool {
    rcad_cancel::requested()
}

/// [`DocError::Cancelled`] if cancellation was requested.
pub(crate) fn check_cancel(next: FeatureId) -> Result<()> {
    if rcad_cancel::requested() {
        Err(DocError::Cancelled(next))
    } else {
        Ok(())
    }
}
