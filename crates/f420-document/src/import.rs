//! Adapters from embedded source files to body geometry (ADR 0004). Pure
//! functions of the bytes and the recorded options, so regeneration is
//! deterministic and needs no external path.
use crate::regen::MeshBody;
use crate::{DocError, Result};
use f420_kernel::{Body, Frame};
use serde::{Deserialize, Serialize};

/// A planar face of an imported solid with its stable import index and the
/// frame recorded at import (ADR 0003).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportedPlanarFace {
    pub index: u32,
    pub frame: Frame,
}

/// One imported exact solid.
pub struct ImportedSolid {
    pub name: Option<String>,
    pub body: Body,
    pub planar_faces: Vec<ImportedPlanarFace>,
}

pub struct StepResult {
    pub solids: Vec<ImportedSolid>,
    /// Units, counts and warnings for the import summary.
    pub notes: Vec<String>,
}

/// Mesh diagnostics shown for mesh bodies (never an exact-solid claim).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MeshDiagnostics {
    pub triangles: usize,
    pub vertices: usize,
    pub bounds: Option<([f64; 3], [f64; 3])>,
    pub closed: bool,
    pub boundary_edges: usize,
    pub non_manifold_edges: usize,
    pub inconsistent_orientation_edges: usize,
    pub degenerate_triangles: usize,
    pub components: usize,
    /// Enclosed volume (mm^3), only for a closed, consistently oriented mesh.
    pub signed_volume: Option<f64>,
    pub surface_area: f64,
    pub encoding: String,
}

/// Parse an embedded STEP file into exact solids (mm).
pub fn step(_bytes: &[u8], _unit_override_to_mm: Option<f64>) -> Result<StepResult> {
    Err(DocError::ProductLimit(
        "STEP import is not available in this build".into(),
    ))
}

/// Parse an embedded STL file into a mesh body scaled by `to_mm`.
pub fn stl(_bytes: &[u8], _to_mm: f64) -> Result<(MeshBody, Vec<String>)> {
    Err(DocError::ProductLimit(
        "STL import is not available in this build".into(),
    ))
}
