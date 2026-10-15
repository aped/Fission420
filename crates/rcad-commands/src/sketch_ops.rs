//! Granular headless sketch operations (`Command::SketchOp`): the same code
//! the sketch editor runs, so every editor action is scriptable and testable
//! without a window. Each op is one undoable step on a sketch feature: the
//! stored sketch is edited, solved, and committed only if it solves (a refusal
//! leaves the document revision unchanged).
//!
//! Projected geometry (curves driven by a projection link) is read-only: an op
//! that would delete, trim, extend or break it is refused; unlink it first
//! (`Command::UnlinkProjection`).
use rcad_document::{DocError, FeatureId, FeatureKind, Model};
use rcad_sketch::{Constraint, Entity, EntityId, PointId, Sketch, SolveStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A point operand: an existing point id, or new coordinates (mm).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PointRef {
    Id(u32),
    At([f64; 2]),
}

/// One edit of a sketch. Points are sketch-plane coordinates in mm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SketchOp {
    /// A standalone point.
    AddPoint { at: [f64; 2] },
    /// A line between two points (ids are shared, coordinates add points).
    AddLine {
        a: PointRef,
        b: PointRef,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        construction: bool,
    },
    /// A circle of `radius` mm about `center`.
    AddCircle {
        center: PointRef,
        radius: f64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        construction: bool,
    },
    /// A counter-clockwise arc from `start` to `end` about `center` (the
    /// arc's equal-radius equation moves `end` onto the circle when solving).
    AddArc {
        center: PointRef,
        start: PointRef,
        end: PointRef,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        construction: bool,
    },
    /// A geometric constraint (the model's `Constraint`, e.g.
    /// `{"Horizontal":{"line":1}}`). Refused if it conflicts or is redundant.
    AddConstraint { constraint: Constraint },
    /// A dimension (a `Constraint` carrying a `dim`; an empty name gets a
    /// fresh one), optionally bound to `expression` (user parameters, units,
    /// arithmetic: docs/PARAMETERS.md). Refused if it conflicts or is
    /// redundant.
    AddDimension {
        constraint: Constraint,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expression: Option<String>,
    },
    /// Delete an entity with everything that only existed for it
    /// (`Sketch::remove_entity`).
    DeleteEntity { entity: u32 },
    /// Delete constraint number `index` (0-based, `Sketch::constraints`).
    DeleteConstraint { index: usize },
    /// Trim the curve under `at`: remove the span between its nearest
    /// intersections with other curves (or to its free ends).
    Trim { entity: u32, at: [f64; 2] },
    /// Extend the open end of `entity` nearest `at` to the next curve.
    Extend { entity: u32, at: [f64; 2] },
    /// Break `entity` at the intersections either side of `at`: the span
    /// under `at` becomes its own curve, nothing is removed.
    BreakAt { entity: u32, at: [f64; 2] },
    /// Fix (`fix: true`) the points where they are, or remove their fixes
    /// (the origin's fix stays; projected points are refused).
    Fix {
        points: Vec<u32>,
        #[serde(default = "yes")]
        fix: bool,
    },
    /// Make dimension `name` driven (it measures) or driving (refused while
    /// it is redundant).
    SetDriven { name: String, driven: bool },
    /// Replace the whole sketch definition, creating or redefining the user
    /// `parameters` and setting dimension `expressions` (dimension name ->
    /// expression; an empty expression unbinds) in the same step: the sketch
    /// editor's Finish when dimensions were typed as expressions. A bound
    /// dimension whose value the sketch changes without rebinding it is
    /// refused (its expression would silently win).
    Replace {
        sketch: Sketch,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        parameters: Vec<NewParameter>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        expressions: BTreeMap<String, String>,
    },
}

fn yes() -> bool {
    true
}

/// A user parameter created (or redefined) by [`SketchOp::Replace`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewParameter {
    pub name: String,
    pub expression: String,
    /// Declared unit (default mm; `deg` for a parameter typed into an angle).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<rcad_document::ParamUnit>,
}

/// Result of a sketch op (ids for scripts, and the solve state).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SketchOpOut {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<u32>,
    /// Points the op added.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<u32>,
    /// Name of the dimension an `AddDimension` created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimension: Option<String>,
    /// Remaining degrees of freedom after the op.
    pub dof: usize,
    /// Status-line text (what was done, constraints dropped...).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl SketchOpOut {
    /// One-line summary for the command response.
    pub fn summary(&self) -> String {
        let mut s = String::from("sketch op:");
        if !self.created.is_empty() {
            s += &format!(" created entities {:?};", self.created);
        }
        if !self.removed.is_empty() {
            s += &format!(" removed entities {:?};", self.removed);
        }
        if !self.points.is_empty() {
            s += &format!(" added points {:?};", self.points);
        }
        if let Some(d) = &self.dimension {
            s += &format!(" dimension '{d}';");
        }
        s += &format!(" {} DOF left", self.dof);
        if !self.note.is_empty() {
            s += &format!("; {}", self.note);
        }
        s
    }
}

fn refused(why: impl Into<String>) -> DocError {
    DocError::Validation(why.into())
}

fn sketch_err(e: rcad_sketch::SketchError) -> DocError {
    DocError::Validation(e.to_string())
}

/// Entities driven by a projection link of the sketch feature.
fn projected(model: &Model, feature: FeatureId) -> Result<Vec<EntityId>, DocError> {
    match &model.document().feature(feature)?.kind {
        FeatureKind::Sketch { links, .. } => Ok(links
            .iter()
            .flat_map(|l| l.loops.iter().flatten())
            .map(|c| c.entity)
            .collect()),
        _ => Err(refused(format!("{feature} is not a sketch"))),
    }
}

fn point(sk: &mut Sketch, p: PointRef, out: &mut SketchOpOut) -> Result<PointId, DocError> {
    match p {
        PointRef::Id(id) => {
            let id = PointId(id);
            sk.point(id)
                .map(|_| id)
                .ok_or_else(|| refused(format!("the sketch has no point P{}", id.0)))
        }
        PointRef::At([x, y]) => {
            if !x.is_finite() || !y.is_finite() {
                return Err(refused("point coordinates must be finite"));
            }
            let id = sk.add_point(x, y);
            out.points.push(id.0);
            Ok(id)
        }
    }
}

fn mark(sk: &mut Sketch, id: EntityId, construction: bool, out: &mut SketchOpOut) {
    if construction {
        sk.set_construction(id, true);
    }
    out.created.push(id.0);
}

/// Edit `sk` by `op` (no solve). Returns whether the op adds a constraint
/// that must not be redundant.
fn edit(
    sk: &mut Sketch,
    op: &SketchOp,
    locked: &[EntityId],
    out: &mut SketchOpOut,
) -> Result<bool, DocError> {
    let unlocked = |e: u32| {
        let id = EntityId(e);
        if locked.contains(&id) {
            Err(refused(format!(
                "entity {e} is projected geometry (read-only); unlink its projection first"
            )))
        } else {
            Ok(id)
        }
    };
    let curve = |sk: &mut Sketch,
                 r: Result<rcad_sketch::CurveEdit, rcad_sketch::SketchError>,
                 verb: &str,
                 out: &mut SketchOpOut| {
        let _ = sk;
        let c = r.map_err(sketch_err)?;
        out.created.extend(c.created.iter().map(|e| e.0));
        out.removed.extend(c.removed.iter().map(|e| e.0));
        out.note = c.message(verb);
        Ok::<bool, DocError>(false)
    };
    match op {
        SketchOp::AddPoint { at } => {
            point(sk, PointRef::At(*at), out)?;
            Ok(false)
        }
        SketchOp::AddLine { a, b, construction } => {
            let (pa, pb) = (point(sk, *a, out)?, point(sk, *b, out)?);
            if pa == pb {
                return Err(refused("a line needs two different points"));
            }
            let id = sk.add_line(pa, pb);
            mark(sk, id, *construction, out);
            Ok(false)
        }
        SketchOp::AddCircle {
            center,
            radius,
            construction,
        } => {
            if !radius.is_finite() || *radius <= 0.0 {
                return Err(refused(format!(
                    "circle radius {radius} must be finite and > 0 mm"
                )));
            }
            let c = point(sk, *center, out)?;
            let id = sk.add_circle(c, *radius);
            mark(sk, id, *construction, out);
            Ok(false)
        }
        SketchOp::AddArc {
            center,
            start,
            end,
            construction,
        } => {
            let c = point(sk, *center, out)?;
            let (s, e) = (point(sk, *start, out)?, point(sk, *end, out)?);
            let id = sk.next_entity_id();
            sk.entities.push(Entity::Arc {
                id,
                center: c,
                start: s,
                end: e,
            });
            mark(sk, id, *construction, out);
            Ok(false)
        }
        SketchOp::AddConstraint { constraint } => {
            if constraint.dimension().is_some() {
                return Err(refused(
                    "this constraint carries a dimension: use add_dimension",
                ));
            }
            sk.constraints.push(constraint.clone());
            Ok(true)
        }
        SketchOp::AddDimension { constraint, .. } => {
            let mut c = constraint.clone();
            let Some(d) = c.dimension_mut() else {
                return Err(refused(
                    "add_dimension needs a dimension constraint (distance, radius, angle...)",
                ));
            };
            if d.name.trim().is_empty() {
                d.name = sk.unique_dimension_name("d");
            }
            out.dimension = Some(d.name.clone());
            if sk.dimension_is_redundant(&c) {
                // Redundant: added driven (measuring), as in the editor.
                if let Some(d) = c.dimension_mut() {
                    d.driven = true;
                }
                out.note = "added as a driven dimension (the sketch already fixes it)".into();
                sk.constraints.push(c);
                return Ok(false);
            }
            sk.constraints.push(c);
            Ok(true)
        }
        SketchOp::DeleteEntity { entity } => {
            let id = unlocked(*entity)?;
            if !sk.remove_entity(id) {
                return Err(sketch_err(rcad_sketch::SketchError::MissingEntity(id)));
            }
            out.removed.push(id.0);
            Ok(false)
        }
        SketchOp::DeleteConstraint { index } => {
            let c = sk
                .constraints
                .get(*index)
                .ok_or_else(|| refused(format!("the sketch has no constraint {index}")))?;
            if matches!(c, Constraint::FixedPoint { p, .. } if Some(*p) == sk.origin) {
                return Err(refused("the origin's fix cannot be deleted"));
            }
            let label = sk.constraint_label(*index);
            // Pins of projected curves follow their link.
            let pins_projected = locked.iter().any(|e| {
                sk.entity(*e)
                    .is_some_and(|x| x.points().iter().any(|p| constraint_pins(c, *p)))
            });
            if pins_projected || c.entities().iter().any(|e| locked.contains(e)) {
                return Err(refused(format!(
                    "'{label}' holds projected geometry; unlink its projection first"
                )));
            }
            sk.remove_constraint(*index);
            out.note = format!("deleted {label}");
            Ok(false)
        }
        SketchOp::Trim { entity, at } => {
            let id = unlocked(*entity)?;
            let r = sk.trim(id, *at);
            curve(sk, r, "Trimmed", out)
        }
        SketchOp::Extend { entity, at } => {
            let id = unlocked(*entity)?;
            let r = sk.extend(id, *at, None);
            curve(sk, r, "Extended", out)
        }
        SketchOp::BreakAt { entity, at } => {
            let id = unlocked(*entity)?;
            let r = sk.break_at(id, *at);
            curve(sk, r, "Broken", out)
        }
        SketchOp::Fix { points, fix } => {
            let pts: Vec<PointId> = points.iter().map(|p| PointId(*p)).collect();
            for p in &pts {
                if sk.point(*p).is_none() {
                    return Err(refused(format!("the sketch has no point P{}", p.0)));
                }
                if locked
                    .iter()
                    .any(|e| sk.entity(*e).is_some_and(|x| x.points().contains(p)))
                {
                    return Err(refused(format!(
                        "P{} belongs to projected geometry (held by its link)",
                        p.0
                    )));
                }
            }
            let n = if *fix {
                sk.fix_points(&pts)
            } else {
                sk.unfix_points(&pts)
            };
            out.note = format!("{} {n} point(s)", if *fix { "fixed" } else { "unfixed" });
            Ok(*fix)
        }
        SketchOp::SetDriven { name, driven } => {
            let mut found = false;
            for c in &mut sk.constraints {
                if let Some(d) = c.dimension_mut()
                    && d.name == *name
                {
                    d.driven = *driven;
                    if !*driven {
                        d.value = d.value.abs();
                    }
                    found = true;
                }
            }
            if !found {
                return Err(refused(format!("the sketch has no dimension '{name}'")));
            }
            Ok(!*driven)
        }
        SketchOp::Replace { sketch, .. } => {
            // Projected curves keep their definition (their link drives them).
            for e in locked {
                if sketch.entity(*e) != sk.entity(*e) {
                    return Err(refused(format!(
                        "entity {} is projected geometry (read-only); unlink its projection first",
                        e.0
                    )));
                }
            }
            *sk = sketch.clone();
            Ok(false)
        }
    }
}

fn constraint_pins(c: &Constraint, p: PointId) -> bool {
    matches!(c, Constraint::FixedPoint { p: q, .. } | Constraint::FixedX { p: q, .. } | Constraint::FixedY { p: q, .. } if *q == p)
}

/// Apply `op` to the sketch of feature `feature` in one transaction.
pub fn apply(
    model: &mut Model,
    feature: FeatureId,
    op: &SketchOp,
) -> Result<SketchOpOut, DocError> {
    let (name, old) = {
        let f = model.document().feature(feature)?;
        match &f.kind {
            FeatureKind::Sketch { sketch, .. } => (f.name.clone(), sketch.clone()),
            _ => return Err(refused(format!("{feature} is not a sketch"))),
        }
    };
    let locked = projected(model, feature)?;
    let mut out = SketchOpOut::default();
    let mut sk = old.clone();
    let checked = edit(&mut sk, op, &locked, &mut out)?;
    sk.validate()
        .map_err(|e| refused(format!("invalid sketch: {e}")))?;
    let original = sk.clone();
    let rep = sk.solve();
    let labels = |idx: &[usize]| {
        if idx.is_empty() {
            "not identified".to_string()
        } else {
            idx.iter()
                .map(|&i| original.constraint_label(i))
                .collect::<Vec<_>>()
                .join("; ")
        }
    };
    if !rep.is_success() {
        return Err(refused(format!(
            "the sketch would not solve ({:?}); conflicts with: {}",
            rep.status,
            labels(&rep.conflicting)
        )));
    }
    let collapsed = sk.collapsed_curves();
    if !collapsed.is_empty() {
        return Err(refused(format!(
            "the sketch would collapse {} to zero size; change or delete a constraint first",
            collapsed
                .iter()
                .map(|e| format!("entity {}", e.0))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if checked && rep.status == SolveStatus::OverConstrained {
        return Err(refused(format!(
            "this would over-constrain the sketch (redundant with: {})",
            labels(&rep.conflicting)
        )));
    }
    out.dof = rep.dof;
    let new = crate::keep_entity_ids(&old, sk)?;
    let expression = match op {
        SketchOp::AddDimension {
            expression: Some(e),
            ..
        } if !e.trim().is_empty() => {
            let e = e.trim().to_string();
            if rcad_document::params::plain_number(&e).is_none() {
                rcad_document::params::check_syntax(&e).map_err(|why| DocError::Expression {
                    name: format!("{name}.{}", out.dimension.clone().unwrap_or_default()),
                    detail: format!("'{e}': {why}"),
                })?;
            }
            Some(e)
        }
        _ => None,
    };
    let dim = out.dimension.clone();
    let label = format!("{} in {name}", op_label(op));
    let (params, binds) = match op {
        SketchOp::Replace {
            parameters,
            expressions,
            ..
        } => (parameters.clone(), expressions.clone()),
        _ => (vec![], BTreeMap::new()),
    };
    for p in &params {
        rcad_document::params::valid_name(p.name.trim()).map_err(|e| DocError::Expression {
            name: p.name.clone(),
            detail: e,
        })?;
        rcad_document::params::check_syntax(&p.expression).map_err(|e| DocError::Expression {
            name: p.name.clone(),
            detail: format!("'{}': {e}", p.expression),
        })?;
    }
    for (n, e) in &binds {
        if !e.trim().is_empty() && rcad_document::params::plain_number(e).is_none() {
            rcad_document::params::check_syntax(e).map_err(|why| DocError::Expression {
                name: format!("{name}.{n}"),
                detail: format!("'{e}': {why}"),
            })?;
        }
    }
    // A bound dimension changed without rebinding: its expression would win.
    if let SketchOp::Replace { .. } = op {
        let f = model.document().feature(feature)?;
        let old_dims = old.dimensions();
        for (n, v) in new.dimensions() {
            let changed = old_dims
                .iter()
                .find(|(o, _)| *o == n)
                .is_some_and(|(_, ov)| (ov - v).abs() > 1e-9 * ov.abs().max(1.0));
            if changed
                && !binds.contains_key(&n)
                && let Some(e) = f.expressions.get(&n)
            {
                return Err(refused(format!(
                    "dimension '{n}' of {name} is bound to the expression '{e}'; type a new \
                     value or expression for it in the sketch, or change the expression"
                )));
            }
        }
    }
    model.transact(&label, |d| {
        for p in &params {
            let (pn, pe) = (p.name.trim().to_string(), p.expression.trim().to_string());
            match d.parameters.iter_mut().find(|x| x.name == pn) {
                Some(x) => {
                    x.expression = pe;
                    if let Some(u) = p.unit {
                        x.unit = u;
                    }
                }
                None => d.parameters.push(rcad_document::Parameter {
                    name: pn,
                    expression: pe,
                    unit: p.unit.unwrap_or_default(),
                    comment: String::new(),
                }),
            }
        }
        let f = d.feature_mut(feature)?;
        match &mut f.kind {
            FeatureKind::Sketch { sketch, .. } => *sketch = new,
            _ => return Err(refused(format!("{feature} is not a sketch"))),
        }
        for (n, e) in &binds {
            let e = e.trim();
            if e.is_empty() {
                f.expressions.remove(n);
            } else if let Some(v) = rcad_document::params::plain_number(e) {
                f.expressions.remove(n);
                f.set_parameter(n, v)?;
            } else {
                f.expressions.insert(n.clone(), e.to_string());
            }
        }
        if let (Some(e), Some(n)) = (&expression, &dim) {
            match rcad_document::params::plain_number(e) {
                Some(v) => f.set_parameter(n, v)?,
                None => {
                    f.expressions.insert(n.clone(), e.clone());
                }
            }
        }
        // A dimension removed by the op takes its expression binding with
        // it (as Edit Sketch does).
        let names: Vec<String> = f.parameters().into_iter().map(|(n, _)| n).collect();
        f.expressions.retain(|k, _| names.contains(k));
        Ok(())
    })?;
    Ok(out)
}

fn op_label(op: &SketchOp) -> &'static str {
    match op {
        SketchOp::AddPoint { .. } => "Add point",
        SketchOp::AddLine { .. } => "Add line",
        SketchOp::AddCircle { .. } => "Add circle",
        SketchOp::AddArc { .. } => "Add arc",
        SketchOp::AddConstraint { .. } => "Add constraint",
        SketchOp::AddDimension { .. } => "Add dimension",
        SketchOp::DeleteEntity { .. } => "Delete entity",
        SketchOp::DeleteConstraint { .. } => "Delete constraint",
        SketchOp::Trim { .. } => "Trim",
        SketchOp::Extend { .. } => "Extend",
        SketchOp::BreakAt { .. } => "Break",
        SketchOp::Replace { .. } => "Edit sketch",
        SketchOp::Fix { fix: true, .. } => "Fix",
        SketchOp::Fix { .. } => "Unfix",
        SketchOp::SetDriven { .. } => "Driven / driving",
    }
}
