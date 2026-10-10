//! Fission420 native application: egui UI over the shared command API.
//!
//! The authoritative state is the [`f420_commands::Session`] owned by
//! [`engine::Engine`] (on a worker thread in the real app). The UI in [`app`]
//! renders the last reported document summary and sends [`f420_commands::Command`]s.

pub mod app;
pub mod engine;
pub mod place;
pub mod sketch_edit;

pub use app::{Diagnostic, F420App};
pub use engine::{ExecMode, Reply};
