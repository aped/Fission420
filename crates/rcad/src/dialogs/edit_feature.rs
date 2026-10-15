//! Edit Feature: reopen a feature's command dialog prefilled from its stored
//! definition; OK replaces the definition in place with
//! `Command::RedefineFeature` (same id and history position, one undo step).
//! Cancel leaves the document untouched.

use std::collections::BTreeMap;

use rcad_commands::{Command, ExtrudeOp};
use rcad_document::{BodyId, Extent, FeatureId, FeatureKind, Operation, PlaneBase, SketchPlane};

use crate::app::{Dialog, PatternKind, RcadApp, SketchTarget};
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
        (Dialog::Hole { .. }, FeatureKind::Hole { target, .. }) => {
            let Command::Hole {
                plane,
                center,
                diameter,
                depth,
                counterbore,
                ..
            } = crate::app::build_command(dlg)?
            else {
                return Err("not a hole".into());
            };
            Ok(FeatureKind::Hole {
                plane,
                center,
                diameter,
                depth,
                counterbore: counterbore
                    .map(|[diameter, depth]| rcad_document::Counterbore { diameter, depth }),
                target: *target,
            })
        }
        _ => kind_from_command(crate::app::build_command(dlg)?, old),
    }
}

/// The definition a dialog's command describes, for the dialog kinds whose
/// command maps one to one onto the stored feature (fillets, chamfers,
/// shells, patterns, mirror, loft, sweep, thread). Values the dialog does
/// not show (created bodies, the chord tolerance, a nut's clearance, a
/// mirror plane's tilt) are kept from `old`.
fn kind_from_command(cmd: Command, old: &FeatureKind) -> Result<FeatureKind, String> {
    use FeatureKind as K;
    Ok(match (cmd, old) {
        (Command::Chamfer { a, b, distance, .. }, K::Chamfer { .. }) => {
            K::Chamfer { a, b, distance }
        }
        (Command::Fillet { a, b, radius, .. }, K::Fillet { .. }) => K::Fillet { a, b, radius },
        (
            Command::FilletLoop {
                face,
                radius,
                holes,
                ..
            },
            K::FilletLoop { .. },
        ) => K::FilletLoop {
            face,
            radius,
            holes,
        },
        (
            Command::ChamferLoop {
                face,
                distance,
                holes,
                ..
            },
            K::ChamferLoop { .. },
        ) => K::ChamferLoop {
            face,
            distance,
            holes,
        },
        (
            Command::Shell {
                extrude,
                open,
                thickness,
                ..
            },
            K::Shell { .. },
        ) => K::Shell {
            extrude,
            open,
            thickness,
        },
        (
            Command::ShellFace {
                face, thickness, ..
            },
            K::ShellFace { .. },
        ) => K::ShellFace { face, thickness },
        (
            Command::LinearPattern {
                feature,
                count,
                spacing,
                direction,
                second,
                ..
            },
            K::LinearPattern { .. },
        ) => K::LinearPattern {
            seed: feature,
            count,
            spacing,
            direction,
            second,
        },
        (
            Command::CircularPattern {
                feature,
                count,
                angle_deg,
                center,
                ..
            },
            K::CircularPattern { .. },
        ) => K::CircularPattern {
            seed: feature,
            count,
            angle: angle_deg.to_radians(),
            center,
        },
        (
            Command::Mirror { feature, plane, .. },
            K::Mirror {
                plane: old_plane, ..
            },
        ) => K::Mirror {
            seed: feature,
            plane: keep_tilt(plane, old_plane),
        },
        (Command::Loft { a, b, via, .. }, K::Loft { body, .. }) => K::Loft {
            a,
            b,
            body: *body,
            via,
        },
        (Command::Sweep { profile, path, .. }, K::Sweep { body, .. }) => K::Sweep {
            profile,
            path,
            body: *body,
        },
        (
            Command::Thread {
                plane,
                center,
                diameter,
                pitch,
                length,
                left_hand,
                nut_width,
                ..
            },
            K::Thread {
                chord_tol,
                nut: old_nut,
                body,
                ..
            },
        ) => K::Thread {
            plane,
            center,
            diameter,
            pitch,
            length,
            left_hand,
            chord_tol: *chord_tol,
            nut: nut_width.map(|width| rcad_document::NutSpec {
                width,
                clearance: old_nut.map_or(0.1, |n| n.clearance),
            }),
            body: *body,
        },
        _ => return Err("this dialog cannot redefine that feature".into()),
    })
}

/// A mirror plane from the dialog (which has no tilt field) keeps the tilt
/// of the stored offset plane on the same base.
fn keep_tilt(new: SketchPlane, old: &SketchPlane) -> SketchPlane {
    match (new, old) {
        (
            SketchPlane::Offset { base, distance, .. },
            SketchPlane::Offset {
                base: old_base,
                angle,
                ..
            },
        ) if base == *old_base => SketchPlane::Offset {
            base,
            distance,
            angle: *angle,
        },
        (new, _) => new,
    }
}

/// The dialog's base plane and offset for a stored plane (an offset plane
/// is its base moved along the normal).
fn split_offset(plane: &SketchPlane) -> (SketchPlane, f64) {
    match plane {
        SketchPlane::Offset { base, distance, .. } => (
            match base {
                PlaneBase::WorldXY => SketchPlane::WorldXY,
                PlaneBase::WorldXZ => SketchPlane::WorldXZ,
                PlaneBase::WorldYZ => SketchPlane::WorldYZ,
                PlaneBase::Face(r) => SketchPlane::Face(*r),
            },
            *distance,
        ),
        p => (*p, 0.0),
    }
}

/// Whether `v` is zero at the tolerance of a normalized direction.
fn zero(v: f64) -> bool {
    v.abs() < 1e-12
}

/// The dialog for a stored fillet, chamfer, shell, pattern, mirror, loft,
/// sweep or thread, prefilled (a bound value shows its expression). None
/// for definitions the dialog cannot show (a pattern along a slanted
/// direction, a loft with several middle sections).
pub(crate) fn prefill(def: &FeatureKind, expressions: &BTreeMap<String, String>) -> Option<Dialog> {
    use FeatureKind as K;
    let e = |n: &str, v: f64| expressions.get(n).cloned().unwrap_or_else(|| fmt_value(v));
    let pattern = |seed: FeatureId, kind: PatternKind| Dialog::Pattern {
        feature: seed,
        kind,
        count: "3".into(),
        spacing: "10".into(),
        along_y: false,
        reverse: false,
        count2: "1".into(),
        spacing2: "10".into(),
        angle: "60".into(),
        center_x: "0".into(),
        center_y: "0".into(),
        mirror: SketchPlane::WorldYZ,
        mirror_face: None,
        mirror_offset: "0".into(),
    };
    Some(match def {
        K::Chamfer { a, b, distance } => Dialog::Chamfer {
            a: *a,
            b: Some(*b),
            distance: e("distance", *distance),
            fillet: false,
        },
        K::Fillet { a, b, radius } => Dialog::Chamfer {
            a: *a,
            b: Some(*b),
            distance: e("radius", *radius),
            fillet: true,
        },
        K::FilletLoop {
            face,
            radius,
            holes,
        } => Dialog::FilletLoop {
            face: *face,
            radius: e("radius", *radius),
            chamfer: false,
            holes: *holes,
        },
        K::ChamferLoop {
            face,
            distance,
            holes,
        } => Dialog::FilletLoop {
            face: *face,
            radius: e("distance", *distance),
            chamfer: true,
            holes: *holes,
        },
        K::Shell {
            extrude,
            open,
            thickness,
        } => Dialog::Shell {
            extrude: *extrude,
            open_end: *open == rcad_document::FaceRole::ExtrudeEndCap,
            thickness: e("thickness", *thickness),
        },
        K::ShellFace { face, thickness } => Dialog::ShellFace {
            face: *face,
            thickness: e("thickness", *thickness),
        },
        K::LinearPattern {
            seed,
            count,
            spacing,
            direction,
            second,
        } => {
            // The dialog offers ± sketch X or Y, with the second direction
            // along the other axis.
            let [dx, dy] = *direction;
            let (along_y, reverse) = match (zero(dx), zero(dy)) {
                (false, true) => (false, dx < 0.0),
                (true, false) => (true, dy < 0.0),
                _ => return None,
            };
            if let Some(s) = second {
                let [sx, sy] = s.direction;
                let fits = if along_y {
                    zero(sy) && sx > 0.0
                } else {
                    zero(sx) && sy > 0.0
                };
                if !fits {
                    return None;
                }
            }
            let mut d = pattern(*seed, PatternKind::Linear);
            if let Dialog::Pattern {
                count: c,
                spacing: sp,
                along_y: ay,
                reverse: rv,
                count2,
                spacing2,
                ..
            } = &mut d
            {
                *c = e("count", f64::from(*count));
                *sp = e("spacing", *spacing);
                *ay = along_y;
                *rv = reverse;
                if let Some(s) = second {
                    *count2 = e("count2", f64::from(s.count));
                    *spacing2 = e("spacing2", s.spacing);
                }
            }
            d
        }
        K::CircularPattern {
            seed,
            count,
            angle,
            center,
        } => {
            let mut d = pattern(*seed, PatternKind::Circular);
            if let Dialog::Pattern {
                count: c,
                angle: a,
                center_x,
                center_y,
                ..
            } = &mut d
            {
                *c = e("count", f64::from(*count));
                *a = e("angle_deg", angle.to_degrees());
                *center_x = fmt_value(center[0]);
                *center_y = fmt_value(center[1]);
            }
            d
        }
        K::Mirror { seed, plane } => {
            let (base, offset) = split_offset(plane);
            let mut d = pattern(*seed, PatternKind::Mirror);
            if let Dialog::Pattern {
                mirror,
                mirror_face,
                mirror_offset,
                ..
            } = &mut d
            {
                *mirror = base;
                *mirror_face = match base {
                    SketchPlane::Face(r) => Some(r),
                    _ => None,
                };
                *mirror_offset = e("plane_offset", offset);
            }
            d
        }
        K::Loft { a, b, via, .. } => {
            if via.len() > 1 {
                return None;
            }
            Dialog::LoftSweep {
                a: Some(*a),
                b: Some(*b),
                sweep: false,
                via: via.first().copied(),
            }
        }
        K::Sweep { profile, path, .. } => Dialog::LoftSweep {
            a: Some(*profile),
            b: Some(*path),
            sweep: true,
            via: None,
        },
        K::Thread {
            plane,
            center,
            diameter,
            pitch,
            length,
            left_hand,
            nut,
            ..
        } => Dialog::Thread {
            diameter: e("diameter", *diameter),
            pitch: e("pitch", *pitch),
            length: e("length", *length),
            nut: nut.is_some(),
            nut_width: e("nut_width", nut.map_or(30.0, |n| n.width)),
            left_hand: *left_hand,
            plane: *plane,
            cx: fmt_value(center[0]),
            cy: fmt_value(center[1]),
        },
        _ => return None,
    })
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
        Dialog::Hole {
            diameter, cx, cy, ..
        } => {
            put("diameter", diameter);
            put("center_x", cx);
            put("center_y", cy);
        }
        Dialog::Chamfer {
            distance, fillet, ..
        } => put(if *fillet { "radius" } else { "distance" }, distance),
        Dialog::FilletLoop {
            radius, chamfer, ..
        } => put(if *chamfer { "distance" } else { "radius" }, radius),
        Dialog::Shell { thickness, .. } | Dialog::ShellFace { thickness, .. } => {
            put("thickness", thickness)
        }
        Dialog::Pattern {
            kind,
            count,
            spacing,
            count2,
            spacing2,
            angle,
            mirror_offset,
            ..
        } => match kind {
            PatternKind::Linear => {
                put("count", count);
                put("spacing", spacing);
                put("count2", count2);
                put("spacing2", spacing2);
            }
            PatternKind::Circular => {
                put("count", count);
                put("angle_deg", angle);
            }
            PatternKind::Mirror => put("plane_offset", mirror_offset),
        },
        Dialog::Thread {
            diameter,
            pitch,
            length,
            nut,
            nut_width,
            ..
        } => {
            put("diameter", diameter);
            put("pitch", pitch);
            put("length", length);
            // Without a nut there is no nut_width parameter to bind.
            put("nut_width", if *nut { nut_width } else { "" });
        }
        _ => {}
    }
    (!m.is_empty() || !old.is_empty()).then_some(m)
}

impl RcadApp {
    /// Edit Feature: sketches reopen the sketch editor; extrusions,
    /// revolves, holes, fillets, chamfers, shells, patterns, mirrors,
    /// lofts, sweeps and threads reopen their dialog prefilled; other kinds
    /// say where their values are edited.
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
            Some(FeatureKind::Hole {
                plane,
                center,
                diameter,
                depth,
                counterbore,
                ..
            }) => {
                let (face, offset) = match plane {
                    rcad_document::SketchPlane::Face(r) => (Some(*r), 0.0),
                    rcad_document::SketchPlane::Offset {
                        base: rcad_document::PlaneBase::Face(r),
                        distance,
                        ..
                    } => (Some(*r), *distance),
                    _ => (None, 0.0),
                };
                let e = |n: &str, v: f64| {
                    f.expressions
                        .get(n)
                        .cloned()
                        .unwrap_or_else(|| fmt_value(v))
                };
                face.map(|face| Dialog::Hole {
                    face,
                    cx: e("center_x", center[0]),
                    cy: e("center_y", center[1]),
                    diameter: e("diameter", *diameter),
                    through: depth.is_none(),
                    depth: fmt_value(depth.unwrap_or(8.0)),
                    counterbore: counterbore.is_some(),
                    cb_diameter: fmt_value(counterbore.map_or(10.0, |c| c.diameter)),
                    cb_depth: fmt_value(counterbore.map_or(5.0, |c| c.depth)),
                    offset: fmt_value(offset),
                })
            }
            Some(def) => prefill(def, &f.expressions),
            None => None,
        };
        match dialog {
            Some(mut d) => {
                if let Dialog::Extrude(form) = &mut d
                    && let Some(e) = f.expressions.get("distance")
                {
                    form.distance = e.clone();
                }
                // The stored face references are valid at this revision; an
                // older face pick must not turn OK into a stale-pick refusal.
                if crate::app::built_from_pick(&d) {
                    self.shell.pick_revision = Some(self.summary.revision);
                }
                self.dialog = Some(d);
                self.shell.editing = Some(id);
                self.last_dialog = None;
            }
            None => self.notify(format!(
                "Edit Feature: {} has no edit dialog; change its values in the Inspector",
                f.name
            )),
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

#[cfg(test)]
mod tests {
    use super::*;
    use rcad_document::{FaceRef, FaceRole, NutSpec, PatternDirection};

    fn face(feature: u32, role: FaceRole) -> FaceRef {
        FaceRef {
            feature: FeatureId(feature),
            role,
        }
    }

    /// OK on an unchanged Edit Feature dialog gives back the stored
    /// definition, values the dialog does not show included.
    #[test]
    fn unchanged_dialogs_give_back_the_stored_definition() {
        let top = face(2, FaceRole::ExtrudeEndCap);
        let side = face(2, FaceRole::ExtrudeSide { entity: 3 });
        let defs = [
            FeatureKind::Chamfer {
                a: top,
                b: side,
                distance: 1.5,
            },
            FeatureKind::Fillet {
                a: top,
                b: side,
                radius: 2.0,
            },
            FeatureKind::FilletLoop {
                face: top,
                radius: 2.0,
                holes: true,
            },
            FeatureKind::ChamferLoop {
                face: top,
                distance: 0.5,
                holes: false,
            },
            FeatureKind::Shell {
                extrude: FeatureId(2),
                open: FaceRole::ExtrudeStartCap,
                thickness: 1.25,
            },
            FeatureKind::ShellFace {
                face: top,
                thickness: 3.0,
            },
            FeatureKind::LinearPattern {
                seed: FeatureId(4),
                count: 4,
                spacing: 12.5,
                direction: [0.0, -1.0],
                second: Some(PatternDirection {
                    count: 2,
                    spacing: 7.0,
                    direction: [1.0, 0.0],
                }),
            },
            FeatureKind::CircularPattern {
                seed: FeatureId(4),
                count: 6,
                angle: 60f64.to_radians(),
                center: [1.5, -2.0],
            },
            FeatureKind::Mirror {
                seed: FeatureId(4),
                plane: SketchPlane::Offset {
                    base: PlaneBase::Face(side),
                    distance: 4.0,
                    angle: 0.25,
                },
            },
            FeatureKind::Mirror {
                seed: FeatureId(4),
                plane: SketchPlane::WorldXZ,
            },
            FeatureKind::Loft {
                a: FeatureId(1),
                b: FeatureId(3),
                body: BodyId(7),
                via: vec![FeatureId(2)],
            },
            FeatureKind::Sweep {
                profile: FeatureId(1),
                path: FeatureId(3),
                body: BodyId(7),
            },
            FeatureKind::Thread {
                plane: SketchPlane::Face(top),
                center: [3.0, 4.0],
                diameter: 10.0,
                pitch: 1.5,
                length: 20.0,
                left_hand: true,
                chord_tol: 0.02,
                nut: Some(NutSpec {
                    width: 17.0,
                    clearance: 0.2,
                }),
                body: BodyId(5),
            },
        ];
        for def in defs {
            let dlg = prefill(&def, &BTreeMap::new()).expect("a dialog");
            assert_eq!(redefined(&dlg, &def).as_ref(), Ok(&def), "{dlg:?}");
        }
    }

    /// Definitions the dialog cannot show open no dialog (rather than one
    /// whose OK would silently change them).
    #[test]
    fn definitions_the_dialog_cannot_show_open_none() {
        let slanted = FeatureKind::LinearPattern {
            seed: FeatureId(4),
            count: 3,
            spacing: 5.0,
            direction: [0.6, 0.8],
            second: None,
        };
        assert!(prefill(&slanted, &BTreeMap::new()).is_none());
        let two_sections = FeatureKind::Loft {
            a: FeatureId(1),
            b: FeatureId(4),
            body: BodyId(7),
            via: vec![FeatureId(2), FeatureId(3)],
        };
        assert!(prefill(&two_sections, &BTreeMap::new()).is_none());
    }

    /// A bound value reopens as its expression.
    #[test]
    fn bound_values_prefill_their_expression() {
        let def = FeatureKind::ShellFace {
            face: face(2, FaceRole::ExtrudeEndCap),
            thickness: 2.0,
        };
        let e = BTreeMap::from([("thickness".to_string(), "wall".to_string())]);
        let Some(Dialog::ShellFace { thickness, .. }) = prefill(&def, &e) else {
            panic!("shell dialog")
        };
        assert_eq!(thickness, "wall");
    }
}
