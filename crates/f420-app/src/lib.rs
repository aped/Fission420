//! Fission420 native application: egui UI over the shared command API.
//!
//! The authoritative state is the [`f420_commands::Session`] owned by
//! [`engine::Engine`] (on a worker thread in the real app). The UI in [`app`]
//! renders the last reported document summary and sends [`f420_commands::Command`]s.

pub mod app;
pub mod bodies;
pub mod engine;
pub mod exchange;
pub mod extrude;
pub mod panels;
pub mod params_ui;
pub mod place;
pub mod probe;
pub mod reattach;
pub mod recovery;
pub mod scenes;
pub mod section;
pub mod sketch_edit;
pub mod sketch_view;
pub mod view3d;

pub use app::{
    ConfirmAnswer, Diagnostic, F420App, Guarded, PrintTemplate, SketchSession, SketchTarget,
};
pub use engine::{ExecMode, Reply};

/// Human-readable name of a semantic face role.
pub(crate) fn face_role_label(role: &f420_document::FaceRole) -> String {
    use f420_document::FaceRole;
    match role {
        FaceRole::ExtrudeEndCap => "end cap".into(),
        FaceRole::ExtrudeStartCap => "start cap".into(),
        FaceRole::ExtrudeSide { entity } => format!("side face (sketch entity {entity})"),
        FaceRole::Imported { body, face } => format!("imported face {face} of {body}"),
        FaceRole::Copy { instance, tool, of } => {
            let of = match of {
                f420_document::CopiedRole::EndCap => "end cap".to_string(),
                f420_document::CopiedRole::StartCap => "start cap".to_string(),
                f420_document::CopiedRole::Side { entity } => {
                    format!("side face (sketch entity {entity})")
                }
            };
            let tool = if *tool > 0 {
                format!(", tool {}", tool + 1)
            } else {
                String::new()
            };
            format!("{of} of occurrence {}{tool}", instance + 1)
        }
        FaceRole::Chamfer => "chamfer face".into(),
    }
}
