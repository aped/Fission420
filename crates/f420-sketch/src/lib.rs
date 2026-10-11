//! `f420_sketch`: 2D sketch model, constraint solver, and profile extraction.
//!
//! Units: all lengths are millimetres (f64). Points' `x`/`y` and circle radii are
//! solver unknowns; the stored values act as the initial guess and are replaced
//! by the solution only when a solve succeeds.
//!
//! # Tolerance policy (numerical, sketch solver)
//! * Convergence: an equation system is satisfied when every equation residual
//!   satisfies `|r_i| <= solve_tolerance(scale)` where
//!   `solve_tolerance(scale) = max(ABS_SOLVE_TOL, REL_SOLVE_TOL * scale)` and
//!   `scale` is the largest magnitude of any coordinate, radius or dimension
//!   value in the sketch (at least 1 mm). See [`ABS_SOLVE_TOL`], [`REL_SOLVE_TOL`].
//! * Rank: a singular value `s` of the Jacobian counts toward the rank when
//!   `s > RANK_REL_TOL * s_max`. All Jacobian entries are dimensionless
//!   (mm per mm), so the relative threshold is unit independent. See [`RANK_REL_TOL`].
//!
//! Geometric/profile tolerances reuse the solve tolerance (a length in mm).

mod arc;
mod edit;
mod profile;
mod solver;

pub use arc::{ArcGeometry, MAX_ARC_RADIUS_RATIO, arc_through_3_points};
pub use edit::{DragOutcome, DragResult, fmt_mm};

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub use profile::{LoopSegment, Profile};

/// Absolute floor of the convergence tolerance, mm.
pub const ABS_SOLVE_TOL: f64 = 1e-9;
/// Convergence tolerance relative to the sketch scale (dimensionless).
pub const REL_SOLVE_TOL: f64 = 1e-12;
/// Relative singular-value threshold for numerical rank (dimensionless).
pub const RANK_REL_TOL: f64 = 1e-9;
/// Upper bound on solver unknowns; the solver uses dense linear algebra.
pub const MAX_UNKNOWNS: usize = 2048;
/// Above this many constraints the per-constraint conflict diagnosis is skipped
/// (it costs one rank computation / solve per constraint).
pub const MAX_DIAGNOSED_CONSTRAINTS: usize = 128;

/// Convergence tolerance (mm) for a sketch of the given characteristic size (mm).
pub fn solve_tolerance(scale: f64) -> f64 {
    ABS_SOLVE_TOL.max(REL_SOLVE_TOL * scale)
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PointId(pub u32);

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntityId(pub u32);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SketchPoint {
    pub id: PointId,
    pub x: f64,
    pub y: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Entity {
    Line {
        id: EntityId,
        a: PointId,
        b: PointId,
    },
    /// `radius` is a solver unknown (initial guess), mm.
    Circle {
        id: EntityId,
        center: PointId,
        radius: f64,
    },
    /// Circular arc traversed counter-clockwise from `start` to `end` about
    /// `center` (sweep in (0, 2π)). All three are ordinary solver points; the
    /// entity implies the equation `|start − center| = |end − center|`, which
    /// is always part of the system and never reported as a user constraint.
    Arc {
        id: EntityId,
        center: PointId,
        start: PointId,
        end: PointId,
    },
}

impl Entity {
    pub fn id(&self) -> EntityId {
        match self {
            Entity::Line { id, .. } | Entity::Circle { id, .. } | Entity::Arc { id, .. } => *id,
        }
    }

    /// Every point the entity uses (endpoints, centre).
    pub fn points(&self) -> Vec<PointId> {
        match self {
            Entity::Line { a, b, .. } => vec![*a, *b],
            Entity::Circle { center, .. } => vec![*center],
            Entity::Arc {
                center, start, end, ..
            } => vec![*center, *start, *end],
        }
    }
}

/// A named driving dimension. `value` is in mm and must be finite and > 0.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Dimension {
    pub name: String,
    pub value: f64,
}

impl Dimension {
    pub fn new(name: impl Into<String>, value: f64) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Constraint {
    /// `p == (x, y)`; 2 equations.
    FixedPoint { p: PointId, x: f64, y: f64 },
    /// Line endpoints share y; 1 equation.
    Horizontal { line: EntityId },
    /// Line endpoints share x; 1 equation.
    Vertical { line: EntityId },
    /// `x_b - x_a = dim.value`; 1 equation.
    HorizontalDistance {
        a: PointId,
        b: PointId,
        dim: Dimension,
    },
    /// `y_b - y_a = dim.value`; 1 equation.
    VerticalDistance {
        a: PointId,
        b: PointId,
        dim: Dimension,
    },
    /// `midpoint(a, b) == center`; 2 equations.
    SymmetricAboutPoint {
        a: PointId,
        b: PointId,
        center: PointId,
    },
    /// `2 * radius = dim.value`; 1 equation.
    Diameter { circle: EntityId, dim: Dimension },
    /// Arc: `|start − center| = dim.value`; circle: `radius = dim.value`; 1 equation.
    Radius { entity: EntityId, dim: Dimension },
    /// The line is tangent to the arc at the endpoint they share (same
    /// `PointId`): the radius vector at that point is perpendicular to the
    /// line. 1 equation, `(P − C)·(Q − P)/|Q − P| = 0` (P shared point, C arc
    /// centre, Q the line's other end). The line and the arc must share exactly
    /// one endpoint.
    Tangent { line: EntityId, arc: EntityId },
    /// Two arcs/circles have equal radii (arc radius = |start − centre|);
    /// 1 equation, no dimension.
    EqualRadius { a: EntityId, b: EntityId },
    /// `a == b`; 2 equations.
    Coincident { a: PointId, b: PointId },
    /// Euclidean `|b - a| = dim.value` (aligned length); 1 nonlinear equation.
    Distance {
        a: PointId,
        b: PointId,
        dim: Dimension,
    },
}

impl Constraint {
    pub fn dimension(&self) -> Option<&Dimension> {
        match self {
            Constraint::HorizontalDistance { dim, .. }
            | Constraint::VerticalDistance { dim, .. }
            | Constraint::Distance { dim, .. }
            | Constraint::Diameter { dim, .. }
            | Constraint::Radius { dim, .. } => Some(dim),
            _ => None,
        }
    }

    pub fn dimension_mut(&mut self) -> Option<&mut Dimension> {
        match self {
            Constraint::HorizontalDistance { dim, .. }
            | Constraint::VerticalDistance { dim, .. }
            | Constraint::Distance { dim, .. }
            | Constraint::Diameter { dim, .. }
            | Constraint::Radius { dim, .. } => Some(dim),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolveStatus {
    /// Residual within tolerance, DOF 0, no redundant equations.
    FullyConstrained,
    /// Residual within tolerance, DOF > 0, no redundant equations.
    UnderConstrained,
    /// Residual within tolerance but some equations are redundant
    /// (consistent over-definition). `conflicting` lists redundant constraints.
    OverConstrained,
    /// The least-squares minimum has a residual above tolerance: the constraints
    /// contradict each other. `conflicting` lists candidate culprits.
    Inconsistent,
    /// The iteration limit was reached while still making progress.
    DidNotConverge,
    /// The sketch failed structural validation (dangling ids, bad values...).
    /// Use [`Sketch::validate`] for the structured error. (Addition to the base API.)
    Invalid,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SolveReport {
    pub status: SolveStatus,
    /// Max absolute equation residual, mm.
    pub residual: f64,
    /// Unknowns minus Jacobian rank at the final iterate.
    pub dof: usize,
    pub iterations: usize,
    /// Constraint indices (into `Sketch::constraints`).
    pub conflicting: Vec<usize>,
}

impl SolveReport {
    /// True when the solved coordinates were written back to the sketch.
    pub fn is_success(&self) -> bool {
        matches!(
            self.status,
            SolveStatus::FullyConstrained
                | SolveStatus::UnderConstrained
                | SolveStatus::OverConstrained
        )
    }
}

#[derive(thiserror::Error, Debug, Clone, PartialEq)]
pub enum SketchError {
    #[error("dimension '{name}' has invalid value {value}: must be finite and > 0 mm")]
    InvalidDimension { name: String, value: f64 },
    #[error("unknown dimension '{0}'")]
    UnknownDimension(String),
    #[error("duplicate dimension name '{0}'")]
    DuplicateDimension(String),
    #[error("duplicate point id {0:?}")]
    DuplicatePoint(PointId),
    #[error("duplicate entity id {0:?}")]
    DuplicateEntity(EntityId),
    #[error("reference to missing point {0:?}")]
    MissingPoint(PointId),
    #[error("reference to missing entity {0:?}")]
    MissingEntity(EntityId),
    #[error("constraint {index}: entity {entity:?} is not a {expected}")]
    WrongEntityKind {
        index: usize,
        entity: EntityId,
        expected: &'static str,
    },
    #[error("non-finite or invalid value: {0}")]
    InvalidValue(String),
    #[error("sketch too large: {unknowns} unknowns (max {max})")]
    TooLarge { unknowns: usize, max: usize },
    #[error("sketch is not in a solved state (max residual {residual} mm > {tolerance} mm)")]
    NotSolved { residual: f64, tolerance: f64 },
    #[error("unsupported profile: {0}")]
    UnsupportedProfile(String),
    #[error("degenerate profile: {0}")]
    DegenerateProfile(String),
    /// A three-point arc that cannot be built (collinear/coincident points or a
    /// radius beyond [`MAX_ARC_RADIUS_RATIO`] × chord).
    #[error("degenerate arc: {0}")]
    DegenerateArc(String),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Sketch {
    pub points: Vec<SketchPoint>,
    pub entities: Vec<Entity>,
    pub constraints: Vec<Constraint>,
    /// The sketch-origin point (fixed at (0, 0) by a `FixedPoint` constraint) of
    /// editor-created sketches. Templates have none: their single fixed point is a
    /// movable anchor (`center_x` / `center_y`). Absent in format-v1 files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<PointId>,
}

fn check_dim_value(name: &str, value: f64) -> Result<(), SketchError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(SketchError::InvalidDimension {
            name: name.to_owned(),
            value,
        });
    }
    Ok(())
}

/// For a `Tangent` constraint: `(shared point, arc centre, line's other end)`.
/// The line and the arc must share exactly one endpoint `PointId`.
pub(crate) fn tangent_shared_point(
    line: &Entity,
    arc: &Entity,
) -> Result<(PointId, PointId, PointId), &'static str> {
    let (
        Entity::Line { a, b, .. },
        Entity::Arc {
            center, start, end, ..
        },
    ) = (line, arc)
    else {
        return Err("tangent needs a line and an arc");
    };
    let shared = |p: &PointId| p == start || p == end;
    match (shared(a), shared(b)) {
        (true, false) => Ok((*a, *center, *b)),
        (false, true) => Ok((*b, *center, *a)),
        (true, true) => Err("tangent line and arc share both endpoints"),
        (false, false) => Err("tangent line and arc must share an endpoint"),
    }
}

impl Sketch {
    /// Rectangle centred on a fixed origin point. Points: 0 origin, 1..=4 corners
    /// (BL, BR, TR, TL). Lines 1..=4 (bottom, right, top, left). Exactly fully
    /// constrained: 10 unknowns, 10 independent equations.
    pub fn centered_rectangle(width: f64, height: f64) -> Result<Sketch, SketchError> {
        check_dim_value("width", width)?;
        check_dim_value("height", height)?;
        let (hw, hh) = (width / 2.0, height / 2.0);
        let p = |id: u32, x: f64, y: f64| SketchPoint {
            id: PointId(id),
            x,
            y,
        };
        let line = |id: u32, a: u32, b: u32| Entity::Line {
            id: EntityId(id),
            a: PointId(a),
            b: PointId(b),
        };
        let s = Sketch {
            origin: None,
            points: vec![
                p(0, 0.0, 0.0),
                p(1, -hw, -hh),
                p(2, hw, -hh),
                p(3, hw, hh),
                p(4, -hw, hh),
            ],
            entities: vec![line(1, 1, 2), line(2, 2, 3), line(3, 3, 4), line(4, 4, 1)],
            constraints: vec![
                Constraint::FixedPoint {
                    p: PointId(0),
                    x: 0.0,
                    y: 0.0,
                },
                Constraint::Horizontal { line: EntityId(1) },
                Constraint::Horizontal { line: EntityId(3) },
                Constraint::Vertical { line: EntityId(2) },
                Constraint::Vertical { line: EntityId(4) },
                Constraint::HorizontalDistance {
                    a: PointId(1),
                    b: PointId(2),
                    dim: Dimension::new("width", width),
                },
                Constraint::VerticalDistance {
                    a: PointId(1),
                    b: PointId(4),
                    dim: Dimension::new("height", height),
                },
                Constraint::SymmetricAboutPoint {
                    a: PointId(1),
                    b: PointId(3),
                    center: PointId(0),
                },
            ],
        };
        s.validate()?;
        Ok(s)
    }

    /// Circle centred on a fixed origin point. Points: 0 origin, 1 centre.
    pub fn centered_circle(diameter: f64) -> Result<Sketch, SketchError> {
        check_dim_value("diameter", diameter)?;
        let s = Sketch {
            origin: None,
            points: vec![
                SketchPoint {
                    id: PointId(0),
                    x: 0.0,
                    y: 0.0,
                },
                SketchPoint {
                    id: PointId(1),
                    x: 0.0,
                    y: 0.0,
                },
            ],
            entities: vec![Entity::Circle {
                id: EntityId(1),
                center: PointId(1),
                radius: diameter / 2.0,
            }],
            constraints: vec![
                Constraint::FixedPoint {
                    p: PointId(0),
                    x: 0.0,
                    y: 0.0,
                },
                Constraint::Coincident {
                    a: PointId(0),
                    b: PointId(1),
                },
                Constraint::Diameter {
                    circle: EntityId(1),
                    dim: Dimension::new("diameter", diameter),
                },
            ],
        };
        s.validate()?;
        Ok(s)
    }

    /// Named driving dimensions in constraint order.
    pub fn dimensions(&self) -> Vec<(String, f64)> {
        self.constraints
            .iter()
            .filter_map(Constraint::dimension)
            .map(|d| (d.name.clone(), d.value))
            .collect()
    }

    /// Change a named dimension's value (does not solve). Rejects non-finite,
    /// non-positive values and unknown names; on error the sketch is unchanged.
    pub fn set_dimension(&mut self, name: &str, value: f64) -> Result<(), SketchError> {
        check_dim_value(name, value)?;
        let dim = self
            .constraints
            .iter_mut()
            .filter_map(Constraint::dimension_mut)
            .find(|d| d.name == name)
            .ok_or_else(|| SketchError::UnknownDimension(name.to_owned()))?;
        dim.value = value;
        Ok(())
    }

    /// Structural validation: unique ids, existing references, matching entity
    /// kinds, finite values, valid dimensions, size limit.
    pub fn validate(&self) -> Result<(), SketchError> {
        let mut pids = HashSet::new();
        for p in &self.points {
            if !pids.insert(p.id) {
                return Err(SketchError::DuplicatePoint(p.id));
            }
            if !p.x.is_finite() || !p.y.is_finite() {
                return Err(SketchError::InvalidValue(format!(
                    "point {:?} has non-finite coordinates",
                    p.id
                )));
            }
        }
        let need_point = |id: &PointId| {
            if pids.contains(id) {
                Ok(())
            } else {
                Err(SketchError::MissingPoint(*id))
            }
        };
        let mut kinds: HashMap<EntityId, &Entity> = HashMap::new();
        for e in &self.entities {
            if kinds.insert(e.id(), e).is_some() {
                return Err(SketchError::DuplicateEntity(e.id()));
            }
            match e {
                Entity::Line { id, a, b } => {
                    need_point(a)?;
                    need_point(b)?;
                    if a == b {
                        return Err(SketchError::InvalidValue(format!(
                            "line {id:?} uses the same point for both ends"
                        )));
                    }
                }
                Entity::Circle { id, center, radius } => {
                    need_point(center)?;
                    if !radius.is_finite() || *radius <= 0.0 {
                        return Err(SketchError::InvalidValue(format!(
                            "circle {id:?} radius {radius} must be finite and > 0"
                        )));
                    }
                }
                Entity::Arc {
                    id,
                    center,
                    start,
                    end,
                } => {
                    need_point(center)?;
                    need_point(start)?;
                    need_point(end)?;
                    if start == end || start == center || end == center {
                        return Err(SketchError::InvalidValue(format!(
                            "arc {id:?} must use three distinct points (centre, start, end)"
                        )));
                    }
                }
            }
        }
        let need_entity = |index: usize, id: &EntityId, expected: &'static str| {
            let e = kinds.get(id).ok_or(SketchError::MissingEntity(*id))?;
            let ok = match expected {
                "line" => matches!(e, Entity::Line { .. }),
                "circle" => matches!(e, Entity::Circle { .. }),
                "arc" => matches!(e, Entity::Arc { .. }),
                _ => matches!(e, Entity::Circle { .. } | Entity::Arc { .. }),
            };
            if ok {
                Ok(*e)
            } else {
                Err(SketchError::WrongEntityKind {
                    index,
                    entity: *id,
                    expected,
                })
            }
        };
        let mut names = HashSet::new();
        for (i, c) in self.constraints.iter().enumerate() {
            match c {
                Constraint::FixedPoint { p, x, y } => {
                    need_point(p)?;
                    if !x.is_finite() || !y.is_finite() {
                        return Err(SketchError::InvalidValue(format!(
                            "constraint {i}: fixed point target is non-finite"
                        )));
                    }
                }
                Constraint::Horizontal { line } | Constraint::Vertical { line } => {
                    need_entity(i, line, "line")?;
                }
                Constraint::HorizontalDistance { a, b, .. }
                | Constraint::VerticalDistance { a, b, .. }
                | Constraint::Distance { a, b, .. }
                | Constraint::Coincident { a, b } => {
                    need_point(a)?;
                    need_point(b)?;
                }
                Constraint::SymmetricAboutPoint { a, b, center } => {
                    need_point(a)?;
                    need_point(b)?;
                    need_point(center)?;
                }
                Constraint::Diameter { circle, .. } => {
                    need_entity(i, circle, "circle")?;
                }
                Constraint::Radius { entity, .. } => {
                    need_entity(i, entity, "arc or circle")?;
                }
                Constraint::EqualRadius { a, b } => {
                    need_entity(i, a, "arc or circle")?;
                    need_entity(i, b, "arc or circle")?;
                    if a == b {
                        return Err(SketchError::InvalidValue(format!(
                            "constraint {i}: equal radius needs two different entities"
                        )));
                    }
                }
                Constraint::Tangent { line, arc } => {
                    let l = need_entity(i, line, "line")?;
                    let a = need_entity(i, arc, "arc")?;
                    tangent_shared_point(l, a).map_err(|why| {
                        SketchError::InvalidValue(format!("constraint {i}: {why}"))
                    })?;
                }
            }
            if let Some(d) = c.dimension() {
                check_dim_value(&d.name, d.value)?;
                if !d.name.is_empty() && !names.insert(d.name.as_str()) {
                    return Err(SketchError::DuplicateDimension(d.name.clone()));
                }
            }
        }
        if let Some(o) = &self.origin {
            need_point(o)?;
        }
        let unknowns = self.unknown_count();
        if unknowns > MAX_UNKNOWNS {
            return Err(SketchError::TooLarge {
                unknowns,
                max: MAX_UNKNOWNS,
            });
        }
        Ok(())
    }

    fn unknown_count(&self) -> usize {
        let circles = self
            .entities
            .iter()
            .filter(|e| matches!(e, Entity::Circle { .. }))
            .count();
        self.points.len().saturating_mul(2).saturating_add(circles)
    }

    /// Characteristic size (mm) used for scale-aware tolerances.
    pub fn scale(&self) -> f64 {
        let mut s: f64 = 1.0;
        for p in &self.points {
            s = s.max(p.x.abs()).max(p.y.abs());
        }
        for e in &self.entities {
            if let Entity::Circle { radius, .. } = e {
                s = s.max(radius.abs());
            }
        }
        for c in &self.constraints {
            if let Some(d) = c.dimension() {
                s = s.max(d.value.abs());
            }
            if let Constraint::FixedPoint { x, y, .. } = c {
                s = s.max(x.abs()).max(y.abs());
            }
        }
        if s.is_finite() { s } else { 1.0 }
    }

    /// Solve the constraint system. Coordinates/radii are written back only when
    /// the status is FullyConstrained, UnderConstrained or OverConstrained
    /// (consistent redundancy); otherwise the sketch is left unchanged.
    pub fn solve(&mut self) -> SolveReport {
        solver::solve(self)
    }

    /// Max absolute equation residual at the current values, mm.
    pub fn residual(&self) -> Result<f64, SketchError> {
        self.validate()?;
        let sys = solver::System::build(self);
        Ok(sys.max_residual(&sys.initial))
    }

    /// Extract a closed profile from the current (solved) geometry.
    pub fn profile(&self) -> Result<Profile, SketchError> {
        profile::extract(self)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_edit;

#[cfg(test)]
mod tests_arc;
