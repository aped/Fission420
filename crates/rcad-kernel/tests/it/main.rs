//! The integration tests of `rcad-kernel` in one test binary (one link instead of one per
//! file). Each file under tests/it/ is a module; run one with
//! `cargo test -p rcad-kernel --test it <module>::`.

mod arcs;
mod combine;
mod ellipse_spline;
mod mass_properties;
mod mesh_checks;
mod plate;
mod pocket;
mod revolved_booleans;
mod section;
mod step_curved_roundtrip;
mod step_export_assembly;
mod step_export_multi;
mod step_import;
mod step_import_report;
mod surfaces;
mod volume_general;
mod volume_revolution;
