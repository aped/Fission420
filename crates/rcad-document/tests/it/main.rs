//! The integration tests of `rcad-document` in one test binary (one link instead of one per
//! file). Each file under tests/it/ is a module; run one with
//! `cargo test -p rcad-document --test it <module>::`.

mod assembly_bodies;
mod atomic_save;
mod document;
mod format_name;
mod format_v9;
mod migration;
mod native_fuzz;
mod regen_review;
mod sketch_edit;
