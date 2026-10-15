//! Edit Feature: reopen a feature's command dialog prefilled from its stored
//! definition; OK replaces the definition in place with
//! `Command::RedefineFeature` (same id and history position, one undo step).
//! Cancel leaves the document untouched.

use std::collections::BTreeMap;

use rcad_commands::{Command, ExtrudeOp};
use rcad_document::{BodyId, Extent, FeatureId, FeatureKind, Operation};

use crate::app::{Dialog, RcadApp, SketchTarget};
use crate::extrude::{ExtentMode, ExtrudeForm, ExtrudeMore};
use crate::params_ui::fmt_value;

fn op_of(op: &Operation) -> (ExtrudeOp, Option<BodyId>) {
    match op {
        Operation::NewBody { .. } => (ExtrudeOp::NewBody, None),
        Operation::Join { target } => (ExtrudeOp::Join, Some(*target)),
        Operation::Cut { target } => (ExtrudeOp::Cut, Some(*target)),
        Operation::Intersect { target } => (ExtrudeOp::Intersect, Some(*target)),
    }
}

/// The operation for `op`/`target`, keeping the created body id when the
/// feature stays a New Body (0: "the body it created before, or a new one").
fn operation(op: ExtrudeOp, target: Option<BodyId>, old: &Operation) -> Result<Operation, String> {
    let need = || target.ok_or_else(|| "choose the target body".to_string());
    Ok(match op {
        ExtrudeOp::NewBody => match old {
            Operation::NewBody { body } => Operation::NewBody { body: *body },
            _ => Operation::NewBody { body: BodyId(0) },
        },
        ExtrudeOp::Join => Operation::Join { target: need()? },
        ExtrudeOp::Cut => Operation::Cut { target: need()? },
        ExtrudeOp::Intersect => Operation::Intersect { target: need()? },
    })
}

/// Extrude form buffers for a stored Extrude definition.
pub fn extrude_form(app: &RcadApp, def: &FeatureKind) -> Option<ExtrudeForm> {
    let FeatureKind::Extrude {
        sketch,
        distance,
        operation,
        extent,
        reverse,
        region,
        options,
        ..
    } = def
    else {
        return None;
    };
    let mut form = ExtrudeForm::new(app, Some(*sketch));
    let (op, target) = op_of(operation);
    form.op = op;
    form.target = target.or(form.target);
    form.through_all = *extent == Extent::ThroughAll;
    form.reverse = *reverse;
    form.region = region.clone();
    form.distance = fmt_value(*distance);
    let mut more = ExtrudeMore {
        to_face: None,
        ..ExtrudeMore::default()
    };
    more.mode = if options.symmetric {
        ExtentMode::Symmetric
    } else if options.distance2 != 0.0 {
        ExtentMode::TwoSides
    } else if options.to.is_some() {
        ExtentMode::ToPlane
    } else {
        ExtentMode::OneSide
    };
    more.distance2 = fmt_value(if options.distance2 != 0.0 {
        options.distance2
    } else {
        5.0
    });
    more.start_offset = fmt_value(options.start_offset);
    if let Some(t) = options.thin {
        more.thin = true;
        more.thin_t = fmt_value(t);
    }
    more.taper_deg = fmt_value(options.taper.to_degrees());
    if let Some(p) = &options.to {
        match p {
            rcad_document::SketchPlane::Offset { base, distance, .. } => {
                more.to_base = *base;
                more.to_offset = fmt_value(*distance);
            }
            rcad_document::SketchPlane::WorldXY => more.to_base = rcad_document::PlaneBase::WorldXY,
            rcad_document::SketchPlane::WorldXZ => more.to_base = rcad_document::PlaneBase::WorldXZ,
            rcad_document::SketchPlane::WorldYZ => more.to_base = rcad_document::PlaneBase::WorldYZ,
            rcad_document::SketchPlane::Face(f) => {
                more.to_base = rcad_document::PlaneBase::Face(*f)
            }
        }
    }
    if !options.regions.is_empty() {
        more.all_outer = Some(
            region
                .iter()
                .cloned()
                .chain(options.regions.iter().cloned())
                .collect(),
        );
    }
    form.more = more;
    Some(form)
}

/// The new definition from the dialog, starting from the stored one (fields
/// the dialog does not show, such as split-piece bodies, are kept).
pub(crate) fn redefined(dlg: &Dialog, old: &FeatureKind) -> Result<FeatureKind, String> {
    match (dlg, old) {
        (
            Dialog::Extrude(form),
            FeatureKind::Extrude {
                operation: old_op,
                pieces,
                ..
            },
        ) => {
            let Command::Extrude {
                sketch,
                distance,
                op,
                target,
                through_all,
                reverse,
                region,
                options,
                ..
            } = form.command()?
            else {
                return Err("not an extrusion".into());
            };
            let operation = operation(op, target, old_op)?;
            Ok(FeatureKind::Extrude {
                sketch,
                distance,
                extent: if through_all {
                    Extent::ThroughAll
                } else {
                    Extent::Distance
                },
                reverse,
                region,
                options,
                pieces: if matches!(operation, Operation::Cut { .. }) {
                    pieces.clone()
                } else {
                    vec![]
                },
                operation,
            })
        }
        (
            Dialog::Revolve {
                sketch,
                angle,
                axis,
                reverse,
                op,
                target,
            },
            FeatureKind::Revolve { body, .. },
        ) => {
            let merge = match op {
                ExtrudeOp::NewBody => None,
                _ => Some(operation(
                    *op,
                    *target,
                    &Operation::NewBody { body: *body },
                )?),
            };
            Ok(FeatureKind::Revolve {
                sketch: sketch.ok_or("choose the profile sketch")?,
                angle: crate::app::parse_num("angle (deg)", angle)?.to_radians(),
                body: *body,
                axis: *axis,
                reverse: *reverse,
                merge,
            })
        }
        _ => Err("this dialog cannot redefine that feature".into()),
    }
}

/// The feature's parameter expressions after the dialog: a field holding
/// an expression binds it; a plain number unbinds that parameter; other
/// bindings are kept.
pub(crate) fn typed_expressions(
    dlg: &Dialog,
    old: &BTreeMap<String, String>,
) -> Option<BTreeMap<String, String>> {
    let mut m = old.clone();
    let mut put = |name: &str, text: &str| {
        let t = text.trim();
        if !t.is_empty() && t.parse::<f64>().is_err() {
            m.insert(name.to_string(), t.to_string());
        } else {
            m.remove(name);
        }
    };
    match dlg {
        Dialog::Extrude(f) => put("distance", &f.distance),
        Dialog::Revolve { angle, .. } => put("angle_deg", angle),
        _ => {}
    }
    (!m.is_empty() || !old.is_empty()).then_some(m)
}

impl RcadApp {
    /// Edit Feature: sketches reopen the sketch editor; extrusions and
    /// revolves reopen their dialog prefilled; other kinds say where their
    /// values are edited.
    pub fn edit_feature(&mut self, id: FeatureId) {
        let Some(f) = self.summary.features.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        self.select(Some(id));
        if f.kind == "sketch" {
            self.begin_sketch(SketchTarget::Existing(id));
            return;
        }
        let dialog = match &f.definition {
            Some(def @ FeatureKind::Extrude { .. }) => extrude_form(self, def).map(Dialog::Extrude),
            Some(FeatureKind::Revolve {
                sketch,
                angle,
                axis,
                reverse,
                merge,
                ..
            }) => {
                let (op, target) = merge.as_ref().map_or((ExtrudeOp::NewBody, None), op_of);
                Some(Dialog::Revolve {
                    sketch: Some(*sketch),
                    angle: f
                        .expressions
                        .get("angle_deg")
                        .cloned()
                        .unwrap_or_else(|| fmt_value(angle.to_degrees())),
                    axis: *axis,
                    reverse: *reverse,
                    op,
                    target,
                })
            }
            _ => None,
        };
        match dialog {
            Some(mut d) => {
                if let Dialog::Extrude(form) = &mut d
                    && let Some(e) = f.expressions.get("distance")
                {
                    form.distance = e.clone();
                }
                self.dialog = Some(d);
                self.shell.editing = Some(id);
                self.last_dialog = None;
            }
            None => self.diag(
                false,
                "info",
                format!(
                    "Edit Feature: {} has no edit dialog yet; change its values in the Inspector",
                    f.name
                ),
            ),
        }
    }

    /// OK in an Edit Feature dialog: the RedefineFeature command.
    pub(crate) fn redefine_command(&self, dlg: &Dialog) -> Option<Result<Command, String>> {
        let id = self.shell.editing?;
        let f = self.summary.features.iter().find(|f| f.id == id)?;
        let Some(old) = &f.definition else {
            return Some(Err(format!("{} cannot be redefined", f.name)));
        };
        Some(redefined(dlg, old).map(|kind| Command::RedefineFeature {
            feature: id,
            kind: Box::new(kind),
            name: None,
            expressions: typed_expressions(dlg, &f.expressions),
        }))
    }
}
