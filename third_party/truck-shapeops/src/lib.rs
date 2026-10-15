//! Crate for operationg shapes. Provides boolean operations to Solid, and shape healing for importing shapes from other CAD systems.

// R-CAD fork: `deny(warnings)` and `deny(clippy::all)` removed so newer compilers do not fail the build
// (registry builds cap lints; a path dependency does not).
#![allow(dead_code)]
#![deny(rust_2018_idioms)]
#![warn(
    missing_docs,
    missing_debug_implementations,
    trivial_casts,
    trivial_numeric_casts,
    unsafe_code,
    unstable_features,
    unused_import_braces,
    unused_qualifications
)]

mod healing;
pub use healing::{RobustSplitClosedEdgesAndFaces, SplitClosedEdgesAndFaces};
mod transversal;
pub use transversal::{and, connected_components, or, ShapeOpsCurve, ShapeOpsSurface};
mod alternative;
