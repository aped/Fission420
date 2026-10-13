//! `rcad_exchange`: pure-Rust interchange adapters (ADR 0004).
//!
//! * [`stl`]: ASCII and binary STL reader and writers ([`stl::StlMesh`]).
//! * [`mesh`]: topology/geometry diagnostics of an indexed triangle mesh.
//! * [`dxf`]: ASCII DXF import to an unconstrained [`rcad_sketch::Sketch`] and export of a
//!   sketch's lines, arcs and circles.
//! * [`svg`]: SVG planar paths (lines, rects, circles, circular arcs) to an unconstrained
//!   sketch with explicit units/transforms, and sketch export.
//! * [`threemf`]: 3MF core-subset multi-object mesh import (units, item/component
//!   transforms, object colours) and export, over a bounded ZIP reader/writer.
//!
//! Every reader is a pure function from bytes to a typed result, enforces byte and
//! entity/triangle limits, and reports malformed or unsupported input as an
//! [`ExchangeError`]; no input can make it panic. Lengths are millimetres after conversion;
//! the conversion factor is always reported back. Supported subsets, limits and known
//! losses: `docs/formats/stl.md`, `docs/formats/dxf.md`, `docs/formats/threemf.md`.

pub mod dxf;
pub mod mesh;
pub mod stl;
pub mod svg;
pub mod threemf;
mod zip;

use serde::{Deserialize, Serialize};

/// Entities of one type (and one reason) that were not imported.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct UnsupportedEntities {
    /// DXF entity type as written in the file (e.g. `SPLINE`), upper case.
    pub entity_type: String,
    pub count: usize,
    /// Why they are unsupported (e.g. "block references are not expanded").
    pub reason: String,
    /// 1-based line of the first such entity's `0` group code.
    pub first_line: usize,
}

/// Recoverable adapter error. Serializable so the command layer can return it verbatim.
#[derive(thiserror::Error, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ExchangeError {
    /// Input exceeds a configured limit (`what`: "bytes", "triangles", "entities", ...).
    #[error("{format}: too many {what}: {actual} exceeds the limit of {limit}")]
    LimitExceeded {
        format: String,
        what: String,
        limit: u64,
        actual: u64,
    },
    /// Input ends early or its length disagrees with its own header.
    #[error("{format}: truncated input: {message}")]
    Truncated { format: String, message: String },
    /// Syntax error. `line` is 1-based where the format is line oriented.
    #[error("{format}: malformed input{}: {message}", line.map(|l| format!(" at line {l}")).unwrap_or_default())]
    Malformed {
        format: String,
        line: Option<usize>,
        message: String,
    },
    /// A NaN or infinite coordinate/value.
    #[error("{format}: non-finite number{}: {message}", line.map(|l| format!(" at line {l}")).unwrap_or_default())]
    NonFinite {
        format: String,
        line: Option<usize>,
        message: String,
    },
    /// The file declares no length unit and no override was given: the caller must ask.
    /// `measurement` is the DXF `$MEASUREMENT` value if present (0 imperial, 1 metric).
    #[error("{format}: the file declares no length unit; choose one{}",
        measurement.map(|m| if m == 0 { " (the drawing says imperial)" } else { " (the drawing says metric)" }).unwrap_or(""))]
    UnitsRequired {
        format: String,
        measurement: Option<i32>,
    },
    /// The declared unit is not supported (e.g. astronomical units).
    #[error("{format}: unsupported length unit: {message}")]
    UnsupportedUnits { format: String, message: String },
    /// Unsupported content is present and partial import was not allowed.
    #[error("{format}: unsupported content (enable partial import to skip it): {}",
        entities.iter().map(|e| format!("{} x{} ({})", e.entity_type, e.count, e.reason)).collect::<Vec<_>>().join("; "))]
    Unsupported {
        format: String,
        entities: Vec<UnsupportedEntities>,
    },
    /// Nothing importable was found.
    #[error("{format}: no supported geometry: {message}")]
    Empty { format: String, message: String },
    /// An option value is invalid (non-finite scale, negative tolerance, ...).
    #[error("invalid option: {0}")]
    InvalidOption(String),
    /// The imported geometry could not be turned into a valid sketch (or the sketch to
    /// export is invalid).
    #[error("sketch: {0}")]
    Sketch(String),
}

pub type Result<T> = std::result::Result<T, ExchangeError>;

/// Finite and strictly positive.
pub(crate) fn check_scale(name: &str, v: f64) -> Result<()> {
    if v.is_finite() && v > 0.0 {
        Ok(())
    } else {
        Err(ExchangeError::InvalidOption(format!(
            "{name} must be finite and > 0 (got {v})"
        )))
    }
}
