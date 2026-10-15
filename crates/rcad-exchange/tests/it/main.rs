//! The integration tests of `rcad-exchange` in one test binary (one link instead of one per
//! file). Each file under tests/it/ is a module; run one with
//! `cargo test -p rcad-exchange --test it <module>::`.
//!
//! Tests that set process-wide state (the cancellation flag, environment variables,
//! the file-dialog backend) stay separate binaries in tests/, so plain `cargo test`,
//! which runs the tests of one binary as threads of one process, cannot disturb them.

mod curve_import;
mod dxf_fixtures;
mod dxf_scale;
mod mesh_edit;
mod stl_fixtures;
mod svg_fixtures;
mod threemf_fixtures;
