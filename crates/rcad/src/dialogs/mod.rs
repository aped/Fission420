//! Command-dialog building blocks shared by the docked command dialogs.

use crate::probe::Probe as _;
use eframe::egui;
use rcad_commands::{BodySummary, ExtrudeOp};
use rcad_document::BodyId;

/// Operation radio (New Body / Join / Cut / Intersect) and, for the
/// combining operations, the target body. `what` prefixes the probe keys
/// ("Revolve Join"). Returns false while a combining operation has no
/// usable target (OK stays disabled).
pub fn operation_ui(
    ui: &mut egui::Ui,
    what: &str,
    op: &mut ExtrudeOp,
    target: &mut Option<BodyId>,
    bodies: &[BodySummary],
) -> bool {
    ui.horizontal(|ui| {
        ui.label("Operation");
        for (o, l) in [
            (ExtrudeOp::NewBody, "New Body"),
            (ExtrudeOp::Join, "Join"),
            (ExtrudeOp::Cut, "Cut"),
            (ExtrudeOp::Intersect, "Intersect"),
        ] {
            ui.radio_value(op, o, l).probe(&format!("{what} {l}"));
        }
    });
    if *op == ExtrudeOp::NewBody {
        return true;
    }
    let solids: Vec<&BodySummary> = bodies.iter().filter(|b| b.kind == "brep").collect();
    if target.is_none_or(|t| !solids.iter().any(|b| b.id == t)) {
        *target = solids.last().map(|b| b.id);
    }
    let name = |id: Option<BodyId>| {
        id.and_then(|i| solids.iter().find(|b| b.id == i))
            .map_or("(none)".to_string(), |b| b.name.clone())
    };
    egui::ComboBox::from_label(format!("{what} target body"))
        .selected_text(name(*target))
        .show_ui(ui, |ui| {
            for b in &solids {
                ui.selectable_value(target, Some(b.id), &b.name)
                    .probe(&format!("{what} target {}", b.name));
            }
        })
        .response
        .probe(&format!("{what} target body"));
    if solids.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(255, 110, 110),
            "No solid body to combine with: use New Body",
        );
        return false;
    }
    target.is_some()
}
