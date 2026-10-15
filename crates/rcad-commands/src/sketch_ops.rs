//! Granular headless sketch operations (`Command::SketchOp`): the same code
//! the sketch editor runs, so every editor action is scriptable and testable
//! without a window. Each op is one undoable step on a sketch feature.
use rcad_document::{DocError, FeatureId, Model};
use rcad_sketch::Sketch;
use serde::{Deserialize, Serialize};

/// One edit of a sketch. Points are sketch-plane coordinates in mm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SketchOp {
    /// Trim the curve under `at`: remove the span between its nearest
    /// intersections with other curves (or to its free ends).
    Trim { entity: u32, at: [f64; 2] },
    /// Extend the open end of `entity` nearest `at` to the next curve.
    Extend { entity: u32, at: [f64; 2] },
    /// Split `entity` into two at the point nearest `at`.
    BreakAt { entity: u32, at: [f64; 2] },
}

/// Result of a sketch op (ids of entities it created, for scripts).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SketchOpOut {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<u32>,
}

/// Apply `op` to the sketch of feature `feature` in one transaction.
pub fn apply(
    model: &mut Model,
    feature: FeatureId,
    op: &SketchOp,
) -> Result<SketchOpOut, DocError> {
    let _ = (model, feature, op, std::marker::PhantomData::<Sketch>);
    Err(DocError::ProductLimit(
        "sketch operations are not implemented yet".into(),
    ))
}
