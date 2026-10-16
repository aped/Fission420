//! Command-dialog building blocks shared by the docked command dialogs.

pub mod edit_feature;

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
    crate::form::row(ui, "Operation", |ui| {
        for (o, l) in [
            (ExtrudeOp::NewBody, "New Body"),
            (ExtrudeOp::Join, "Join"),
            (ExtrudeOp::Cut, "Cut"),
            (ExtrudeOp::Intersect, "Intersect"),
        ] {
            ui.radio_value(op, o, l).named(&format!("{what} {l}"));
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
    crate::form::combo(ui, &format!("{what} target body"), name(*target), |ui| {
        for b in &solids {
            ui.selectable_value(target, Some(b.id), &b.name)
                .named(&format!("{what} target {}", b.name));
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

// ----- value inputs -----

use std::cell::RefCell;
use std::collections::HashMap;

use rcad_document::SlotKind;
use rcad_document::params::Quantity;

thread_local! {
    /// The document's evaluated parameters for this frame's dialog fields.
    static ENV: RefCell<HashMap<String, Quantity>> = RefCell::new(HashMap::new());
    /// Invalid value fields drawn this frame (label: reason).
    static INVALID: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Invalid fields the user just left (reported once as input errors).
    static LEFT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Make the document's parameters available to value fields.
pub fn set_expression_env(params: &[rcad_commands::ParameterSummary]) {
    let env: HashMap<String, Quantity> = params
        .iter()
        .filter_map(|p| {
            Some((
                p.name.clone(),
                Quantity {
                    value: p.value?,
                    dim: Some(p.unit.dim()),
                },
            ))
        })
        .collect();
    ENV.with(|e| *e.borrow_mut() = env);
}

/// Start collecting this frame's invalid fields.
pub fn begin_fields() {
    INVALID.with(|v| v.borrow_mut().clear());
}

/// The quantity a field label asks for: angles ("deg", "°", "angle",
/// "taper"), plain numbers (counts, instances, factors, scale, sides),
/// otherwise a length in mm.
pub fn slot_of(label: &str) -> SlotKind {
    let l = label.to_lowercase();
    if l.contains("deg") || l.contains('°') || l.contains("angle") || l.contains("taper") {
        SlotKind::Angle
    } else if ["count", "instances", "factor", "scale", "sides", "number"]
        .iter()
        .any(|w| l.contains(w))
    {
        SlotKind::Count
    } else {
        SlotKind::Length
    }
}

/// Evaluate a value field: a plain number, or an expression with units and
/// the document's parameter names (`w/2`, `10 mm + t`, `1 in`). Lengths in
/// mm, angles in degrees.
pub fn check_value(label: &str, src: &str) -> Result<f64, String> {
    let s = src.trim();
    if s.is_empty() {
        return Err("is empty".into());
    }
    if let Ok(v) = s.parse::<f64>() {
        return if v.is_finite() {
            Ok(v)
        } else {
            Err("is not finite".into())
        };
    }
    let kind = slot_of(label);
    let v = ENV.with(|e| rcad_document::params::evaluate_for_slot(s, &e.borrow(), kind))?;
    if v.is_finite() {
        Ok(v)
    } else {
        Err("does not evaluate to a finite number".into())
    }
}

/// Record an invalid field (`left`: focus just moved away from it).
pub fn note_invalid(label: &str, why: &str, left: bool) {
    let msg = format!("{label}: {why}");
    if left {
        LEFT.with(|v| v.borrow_mut().push(msg.clone()));
    }
    INVALID.with(|v| v.borrow_mut().push(msg));
}

/// The first invalid field of this frame (OK stays disabled).
pub fn invalid_fields() -> Option<String> {
    INVALID.with(|v| v.borrow().first().cloned())
}

/// Invalid fields the user left this frame.
pub fn take_left_invalid() -> Vec<String> {
    LEFT.with(|v| std::mem::take(&mut *v.borrow_mut()))
}
