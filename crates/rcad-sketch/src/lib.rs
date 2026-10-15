//! `rcad_sketch`: 2D sketch model, constraint solver, and profile extraction.
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
pub mod curves;
mod edit;
pub mod fillet;
pub mod font;
pub mod guard;
pub mod offset;
pub mod pattern;
mod profile;
mod region;
pub mod shapes;
mod solver;
pub mod text;
pub mod trim;

pub use arc::{ArcGeometry, MAX_ARC_RADIUS_RATIO, arc_through_3_points};
pub use edit::{DragOutcome, DragResult, fmt_mm};

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};

pub use fillet::FilletResult;
pub use offset::{OffsetPair, OffsetResult, offset_profile};
pub use profile::{LoopSegment, Profile, loop_area};
pub use region::{Region, RegionRef, SketchRegion};
pub use shapes::{PolygonFit, PolygonResult, SlotResult};
pub use text::{FontSpec, HAlign, SketchText, TextLoop, TextSpec, VAlign};
pub use trim::{CurveEdit, arc_distance, constraint_mentions, seg_dist};

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
    /// Full ellipse ([`curves`]): centre point, the end point of one axis
    /// (`major`: its distance from the centre is that semi-axis, its direction
    /// the ellipse's orientation) and the other semi-axis `minor` (mm), a solver
    /// unknown like a circle's radius. 5 degrees of freedom. Serde-additive.
    Ellipse {
        id: EntityId,
        center: PointId,
        major: PointId,
        minor: f64,
    },
    /// Uniform cubic B-spline ([`curves`]) whose control points are ordinary
    /// sketch points (2 degrees of freedom each). Open (`closed == false`):
    /// clamped, at least 4 control points, passing through the first and last.
    /// Closed: periodic, at least 3 control points. Serde-additive.
    Spline {
        id: EntityId,
        points: Vec<PointId>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        closed: bool,
    },
}

impl Entity {
    pub fn id(&self) -> EntityId {
        match self {
            Entity::Line { id, .. }
            | Entity::Circle { id, .. }
            | Entity::Arc { id, .. }
            | Entity::Ellipse { id, .. }
            | Entity::Spline { id, .. } => *id,
        }
    }

    /// Every point the entity uses (endpoints, centre, control points).
    pub fn points(&self) -> Vec<PointId> {
        match self {
            Entity::Line { a, b, .. } => vec![*a, *b],
            Entity::Circle { center, .. } => vec![*center],
            Entity::Arc {
                center, start, end, ..
            } => vec![*center, *start, *end],
            Entity::Ellipse { center, major, .. } => vec![*center, *major],
            Entity::Spline { points, .. } => points.clone(),
        }
    }

    /// The end points a curve joins other curves at in a profile loop (an
    /// open spline's first and last control points); `None` for closed curves.
    pub fn ends(&self) -> Option<(PointId, PointId)> {
        match self {
            Entity::Line { a, b, .. } => Some((*a, *b)),
            Entity::Arc { start, end, .. } => Some((*start, *end)),
            Entity::Spline {
                points,
                closed: false,
                ..
            } => Some((*points.first()?, *points.last()?)),
            Entity::Circle { .. } | Entity::Ellipse { .. } | Entity::Spline { .. } => None,
        }
    }
}

/// A named dimension. `value` is in mm (degrees for angles) and must be
/// finite and > 0 for a driving dimension.
///
/// A **driven** dimension (`driven == true`) adds no equation: it measures
/// the solved sketch (its value is refreshed after every successful solve,
/// signed for horizontal/vertical distances) and cannot be set. A redundant
/// new dimension becomes driven instead of being refused (shown in
/// parentheses). Distances, diameters, radii, ellipse radii and angles may
/// be driven. Serde-additive (absent = driving).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Dimension {
    pub name: String,
    pub value: f64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub driven: bool,
}

impl Dimension {
    pub fn new(name: impl Into<String>, value: f64) -> Self {
        Self {
            name: name.into(),
            value,
            driven: false,
        }
    }

    /// A driven (measuring) dimension.
    pub fn driven(name: impl Into<String>, value: f64) -> Self {
        Self {
            driven: true,
            ..Self::new(name, value)
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Constraint {
    /// `p == (x, y)`; 2 equations.
    FixedPoint { p: PointId, x: f64, y: f64 },
    /// `p.x == x`; 1 equation (pins one coordinate, e.g. the centre of
    /// a projected arc whose ends are fixed, without a redundant equation).
    FixedX { p: PointId, x: f64 },
    /// `p.y == y`; 1 equation.
    FixedY { p: PointId, y: f64 },
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
    /// Ellipse semi-axis: `major` → `|major point − centre| = dim.value`,
    /// otherwise `minor = dim.value`; 1 equation (serde-additive).
    EllipseRadius {
        ellipse: EntityId,
        major: bool,
        dim: Dimension,
    },
    /// The line is tangent to the arc at the endpoint they share (same
    /// `PointId`): the radius vector at that point is perpendicular to the
    /// line. 1 equation, `(P − C)·(Q − P)/|Q − P| = 0` (P shared point, C arc
    /// centre, Q the line's other end). The line and the arc must share exactly
    /// one endpoint.
    Tangent { line: EntityId, arc: EntityId },
    /// Two arcs share exactly one endpoint and are tangent there (internally
    /// or externally): the radius vectors at the shared point P are collinear.
    /// 1 equation (mm), `(P − C_a) × (P − C_b) / |P − C_b|`. Used by
    /// line–arc sketch fillets (serde-additive).
    TangentArcs { a: EntityId, b: EntityId },
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
    /// Two distinct lines are parallel (or anti-parallel). 1 equation,
    /// `S · (u_a × u_b) = 0` with `u` the unit directions and `S` the sketch
    /// scale (mm, constant during a solve) so the residual is in mm.
    Parallel { a: EntityId, b: EntityId },
    /// Two distinct lines are perpendicular. 1 equation, `S · (u_a · u_b) = 0`
    /// (same scaling as [`Constraint::Parallel`]).
    Perpendicular { a: EntityId, b: EntityId },
    /// Two distinct lines have equal length. 1 equation, `|a| − |b| = 0` (mm).
    EqualLength { a: EntityId, b: EntityId },
    /// `point` lies on the infinite line through `line`'s endpoints. 1
    /// equation, the signed distance `u × (p − a) = 0` (mm). The point may not
    /// be one of the line's own endpoints.
    PointOnLine { point: PointId, line: EntityId },
    /// `point` lies on the full circle of `circle` (a circle, or an arc's
    /// supporting circle; the arc's angular range is not enforced). 1 equation
    /// (mm): circle `|p − c| − r`, arc `|p − c| − |start − c|`. The point may not
    /// be the centre, nor an arc's own start/end (they lie on it by definition).
    /// Serde-additive: older sketches never contain it.
    PointOnCircle { point: PointId, circle: EntityId },
    /// The `copy` curves are the `source` loop offset by `dim.value` mm (see
    /// [`offset`] and [`Sketch::offset_loop`]): two equations per copy vertex,
    /// one for a copy circle. `pairs` are in loop order
    /// (serde-additive).
    Offset {
        pairs: Vec<OffsetPair>,
        dim: Dimension,
    },
    /// Signed counter-clockwise angle from line `a`'s direction (its first to
    /// its second point) to line `b`'s, `dim.value` degrees in (0, 180). 1
    /// equation, `S · wrap(θ − θ0)` with θ = atan2(u_a × u_b, u_a · u_b) and
    /// `S` the sketch scale (residual in mm, like [`Constraint::Parallel`]); the
    /// wrap to (−π, π] makes the solution unique (no θ0 + 180° twin).
    Angle {
        a: EntityId,
        b: EntityId,
        dim: Dimension,
    },
    /// Linear sketch pattern copy ([`pattern`]): `to − from = k (b − a)`,
    /// the step vector being the (construction) line `a -> b`. 2 equations, or 1
    /// (only coordinate `axis`, 0 = x, 1 = y) for the one point of a copied arc
    /// whose second equation would duplicate the arc's implied radius equation.
    PatternStep {
        from: PointId,
        to: PointId,
        a: PointId,
        b: PointId,
        k: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        axis: Option<u8>,
    },
    /// Circles/arcs `a` and `b` (different centre points) share their centre
    /// position. 2 equations (serde-additive).
    Concentric { a: EntityId, b: EntityId },
    /// Line `b` lies on the infinite line through line `a`: both ends of `b`
    /// on `a`. 2 equations (serde-additive).
    Collinear { a: EntityId, b: EntityId },
    /// Points `a` and `b` are mirror images about the infinite line through
    /// `line`: their midpoint lies on it and `b − a` is perpendicular to it.
    /// 2 equations (serde-additive).
    Symmetric {
        a: PointId,
        b: PointId,
        line: EntityId,
    },
    /// Circular sketch pattern copy: `to = c + R(k θ) (from − c)`, θ =
    /// `dim.value` degrees counter-clockwise (0 < θ < 360). 2 equations, or 1
    /// (`axis`) as for [`Constraint::PatternStep`].
    PatternRotate {
        from: PointId,
        to: PointId,
        center: PointId,
        k: u32,
        dim: Dimension,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        axis: Option<u8>,
    },
}

impl Constraint {
    /// Entities this constraint references directly.
    pub fn entities(&self) -> Vec<EntityId> {
        edit::constraint_entities(self)
    }

    pub fn dimension(&self) -> Option<&Dimension> {
        match self {
            Constraint::HorizontalDistance { dim, .. }
            | Constraint::VerticalDistance { dim, .. }
            | Constraint::Distance { dim, .. }
            | Constraint::Diameter { dim, .. }
            | Constraint::Radius { dim, .. }
            | Constraint::EllipseRadius { dim, .. }
            | Constraint::Angle { dim, .. }
            | Constraint::PatternRotate { dim, .. }
            | Constraint::Offset { dim, .. } => Some(dim),
            _ => None,
        }
    }

    pub fn dimension_mut(&mut self) -> Option<&mut Dimension> {
        match self {
            Constraint::HorizontalDistance { dim, .. }
            | Constraint::VerticalDistance { dim, .. }
            | Constraint::Distance { dim, .. }
            | Constraint::Diameter { dim, .. }
            | Constraint::Radius { dim, .. }
            | Constraint::EllipseRadius { dim, .. }
            | Constraint::Angle { dim, .. }
            | Constraint::PatternRotate { dim, .. }
            | Constraint::Offset { dim, .. } => Some(dim),
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
    /// Sketch text: the requested font is not installed or cannot be read
    /// Stored text keeps its outlines; only re-resolving needs it.
    #[error("font not found: {0}")]
    FontNotFound(String),
    /// Sketch text: invalid string/height or an unusable font.
    #[error("text: {0}")]
    Text(String),
    /// A sketch edit (trim, extend, break, ...) that cannot be applied here;
    /// the text says why. The sketch is unchanged.
    #[error("{0}")]
    Refused(String),
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
    /// Construction (reference) entities: solved and constrained like any other
    /// entity but ignored by profile extraction. Absent when empty.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub construction: BTreeSet<EntityId>,
    /// Entity ids below this were used by removed entities and are never
    /// reallocated (face references name sketch entities by id, ADR 0003).
    /// Only raised by [`Sketch::remove_entity`]; absent (0) otherwise.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub retired_entity_ids: u32,
    /// Text items ([`text`]): their glyph outlines are profile loops.
    /// Absent when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texts: Vec<SketchText>,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// Structure of an `Offset` constraint: matching kinds, shared centres, one
/// circle pair or a loop whose consecutive sources and copies share points.
fn offset_valid(s: &Sketch, pairs: &[OffsetPair]) -> Result<(), String> {
    if pairs.is_empty() {
        return Err("no curves".into());
    }
    let mut seen = HashSet::new();
    for p in pairs {
        let (a, b) = (
            s.entity(p.source)
                .ok_or(format!("missing entity {:?}", p.source))?,
            s.entity(p.copy)
                .ok_or(format!("missing entity {:?}", p.copy))?,
        );
        if p.source == p.copy || !seen.insert(p.copy) || !seen.insert(p.source) {
            return Err("each curve may appear once".into());
        }
        if p.sign != 1 && p.sign != -1 {
            return Err(format!("sign must be +1 or -1, got {}", p.sign));
        }
        match (a, b) {
            (Entity::Line { .. }, Entity::Line { .. }) => {}
            (Entity::Arc { center: c0, .. }, Entity::Arc { center: c1, .. })
            | (Entity::Circle { center: c0, .. }, Entity::Circle { center: c1, .. })
                if c0 == c1 => {}
            _ => {
                return Err(format!(
                    "{:?} -> {:?}: copies must be of the same kind (arcs and circles \
                     sharing the source centre point)",
                    p.source, p.copy
                ));
            }
        }
    }
    let circles = pairs
        .iter()
        .filter(|p| matches!(s.entity(p.source), Some(Entity::Circle { .. })))
        .count();
    if circles > 0 {
        return if pairs.len() == 1 {
            Ok(())
        } else {
            Err("a circle is offset on its own".into())
        };
    }
    if pairs.len() < 2 {
        return Err("a loop needs at least two curves".into());
    }
    for i in 0..pairs.len() {
        let (p, q) = (&pairs[i], &pairs[(i + 1) % pairs.len()]);
        if offset::shared_point(s, p.source, q.source).is_none()
            || offset::shared_point(s, p.copy, q.copy).is_none()
        {
            return Err(format!(
                "{:?} and {:?} (and their copies) must share exactly one end point",
                p.source, q.source
            ));
        }
    }
    Ok(())
}

pub(crate) fn check_dim_value(name: &str, value: f64) -> Result<(), SketchError> {
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

/// For a `TangentArcs` constraint: `(shared point, centre of a, centre of b)`.
/// The arcs must share exactly one endpoint `PointId`.
pub(crate) fn arcs_shared_point(
    a: &Entity,
    b: &Entity,
) -> Result<(PointId, PointId, PointId), &'static str> {
    let (
        Entity::Arc {
            center: ca,
            start: sa,
            end: ea,
            ..
        },
        Entity::Arc {
            center: cb,
            start: sb,
            end: eb,
            ..
        },
    ) = (a, b)
    else {
        return Err("arc tangency needs two arcs");
    };
    let shared: Vec<PointId> = [*sa, *ea]
        .into_iter()
        .filter(|p| p == sb || p == eb)
        .collect();
    match shared.as_slice() {
        [p] => Ok((*p, *ca, *cb)),
        [] => Err("tangent arcs must share an endpoint"),
        _ => Err("tangent arcs share both endpoints"),
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
            construction: BTreeSet::new(),
            retired_entity_ids: 0,
            texts: vec![],
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
            construction: BTreeSet::new(),
            retired_entity_ids: 0,
            texts: vec![],
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
    /// Named dimensions in constraint order. The copies of one circular
    /// pattern share their `pattern_angle` dimension (listed once).
    pub fn dimensions(&self) -> Vec<(String, f64)> {
        let mut seen = HashSet::new();
        self.constraints
            .iter()
            .filter(|c| {
                !matches!(c, Constraint::PatternRotate { dim, .. } if !dim.name.is_empty() && !seen.insert(dim.name.clone()))
            })
            // An unnamed ellipse radius pins a projected ellipse (driven by
            // its projection link, not a user dimension).
            .filter(|c| !matches!(c, Constraint::EllipseRadius { dim, .. } if dim.name.is_empty()))
            .filter_map(Constraint::dimension)
            .map(|d| (d.name.clone(), d.value))
            .collect()
    }

    /// Change a named dimension's value (does not solve). Rejects non-finite,
    /// non-positive values and unknown names; on error the sketch is unchanged.
    pub fn set_dimension(&mut self, name: &str, value: f64) -> Result<(), SketchError> {
        check_dim_value(name, value)?;
        if self
            .constraints
            .iter()
            .filter_map(Constraint::dimension)
            .any(|d| d.name == name && d.driven)
        {
            return Err(SketchError::Refused(format!(
                "'{name}' is a driven dimension: it measures the sketch and cannot be set \
                 (make it driving first, after removing a constraint it is redundant with)"
            )));
        }
        let is_angle = self
            .constraints
            .iter()
            .any(|c| matches!(c, Constraint::Angle { dim, .. } if dim.name == name));
        if is_angle && value >= 180.0 {
            return Err(SketchError::InvalidDimension {
                name: name.to_owned(),
                value,
            });
        }
        let is_pattern_angle = self
            .constraints
            .iter()
            .any(|c| matches!(c, Constraint::PatternRotate { dim, .. } if dim.name == name));
        if is_pattern_angle && value >= 360.0 {
            return Err(SketchError::InvalidDimension {
                name: name.to_owned(),
                value,
            });
        }
        let mut found = false;
        // Every copy of a circular pattern carries the shared angle.
        for d in self
            .constraints
            .iter_mut()
            .filter_map(Constraint::dimension_mut)
            .filter(|d| d.name == name)
        {
            d.value = value;
            found = true;
        }
        if !found {
            return Err(SketchError::UnknownDimension(name.to_owned()));
        }
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
                Entity::Ellipse {
                    id,
                    center,
                    major,
                    minor,
                } => {
                    need_point(center)?;
                    need_point(major)?;
                    if center == major {
                        return Err(SketchError::InvalidValue(format!(
                            "ellipse {id:?} must use two distinct points (centre, axis end)"
                        )));
                    }
                    if !minor.is_finite() || *minor <= 0.0 {
                        return Err(SketchError::InvalidValue(format!(
                            "ellipse {id:?} minor radius {minor} must be finite and > 0"
                        )));
                    }
                }
                Entity::Spline { id, points, closed } => {
                    for p in points {
                        need_point(p)?;
                    }
                    curves::check_spline(*id, points, *closed)?;
                }
            }
        }
        let need_entity = |index: usize, id: &EntityId, expected: &'static str| {
            let e = kinds.get(id).ok_or(SketchError::MissingEntity(*id))?;
            let ok = match expected {
                "line" => matches!(e, Entity::Line { .. }),
                "circle" => matches!(e, Entity::Circle { .. }),
                "arc" => matches!(e, Entity::Arc { .. }),
                "ellipse" => matches!(e, Entity::Ellipse { .. }),
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
                Constraint::FixedX { p, x: v } | Constraint::FixedY { p, y: v } => {
                    need_point(p)?;
                    if !v.is_finite() {
                        return Err(SketchError::InvalidValue(format!(
                            "constraint {i}: fixed coordinate target is non-finite"
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
                Constraint::EllipseRadius { ellipse, .. } => {
                    need_entity(i, ellipse, "ellipse")?;
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
                Constraint::TangentArcs { a, b } => {
                    let ea = need_entity(i, a, "arc")?;
                    let eb = need_entity(i, b, "arc")?;
                    arcs_shared_point(ea, eb).map_err(|why| {
                        SketchError::InvalidValue(format!("constraint {i}: {why}"))
                    })?;
                }
                Constraint::Parallel { a, b }
                | Constraint::Perpendicular { a, b }
                | Constraint::EqualLength { a, b }
                | Constraint::Angle { a, b, .. } => {
                    need_entity(i, a, "line")?;
                    need_entity(i, b, "line")?;
                    if a == b {
                        return Err(SketchError::InvalidValue(format!(
                            "constraint {i}: needs two different lines"
                        )));
                    }
                    if let Constraint::Angle { dim, .. } = c
                        && !(dim.value > 0.0 && dim.value < 180.0)
                    {
                        return Err(SketchError::InvalidDimension {
                            name: dim.name.clone(),
                            value: dim.value,
                        });
                    }
                }
                Constraint::Offset { pairs, .. } => {
                    offset_valid(self, pairs).map_err(|why| {
                        SketchError::InvalidValue(format!("constraint {i}: offset: {why}"))
                    })?;
                }
                Constraint::PointOnCircle { point, circle } => {
                    need_point(point)?;
                    let own = match need_entity(i, circle, "arc or circle")? {
                        Entity::Circle { center, .. } => vec![*center],
                        Entity::Arc {
                            center, start, end, ..
                        } => vec![*center, *start, *end],
                        _ => vec![],
                    };
                    if own.contains(point) {
                        return Err(SketchError::InvalidValue(format!(
                            "constraint {i}: point {point:?} is the centre or an end of \
                             {circle:?} (point-on-circle needs a separate point)"
                        )));
                    }
                }
                Constraint::PatternStep {
                    from,
                    to,
                    a,
                    b,
                    k,
                    axis,
                } => {
                    for p in [from, to, a, b] {
                        need_point(p)?;
                    }
                    if *k == 0 || from == to || a == b || axis.is_some_and(|x| x > 1) {
                        return Err(SketchError::InvalidValue(format!(
                            "constraint {i}: pattern step needs k >= 1, distinct points and \
                             axis 0 or 1"
                        )));
                    }
                }
                Constraint::PatternRotate {
                    from,
                    to,
                    center,
                    k,
                    dim,
                    axis,
                } => {
                    for p in [from, to, center] {
                        need_point(p)?;
                    }
                    if *k == 0 || from == to || axis.is_some_and(|x| x > 1) {
                        return Err(SketchError::InvalidValue(format!(
                            "constraint {i}: pattern rotation needs k >= 1, distinct points and \
                             axis 0 or 1"
                        )));
                    }
                    if !(dim.value > 0.0 && dim.value < 360.0) {
                        return Err(SketchError::InvalidDimension {
                            name: dim.name.clone(),
                            value: dim.value,
                        });
                    }
                }
                Constraint::Concentric { a, b } => {
                    let ea = need_entity(i, a, "arc or circle")?;
                    let eb = need_entity(i, b, "arc or circle")?;
                    let centre = |e: &Entity| match e {
                        Entity::Circle { center, .. } | Entity::Arc { center, .. } => Some(*center),
                        _ => None,
                    };
                    if a == b || centre(ea) == centre(eb) {
                        return Err(SketchError::InvalidValue(format!(
                            "constraint {i}: concentric needs two curves with different centre \
                             points"
                        )));
                    }
                }
                Constraint::Collinear { a, b } => {
                    need_entity(i, a, "line")?;
                    let eb = need_entity(i, b, "line")?;
                    let ea = kinds.get(a).copied();
                    // The equations put b's ends on a; an end shared with a
                    // would be a vacuous row.
                    if a == b
                        || ea.is_some_and(|ea| ea.points().iter().any(|p| eb.points().contains(p)))
                    {
                        return Err(SketchError::InvalidValue(format!(
                            "constraint {i}: collinear needs two lines that share no end point \
                             (lines joined end to end: use Parallel)"
                        )));
                    }
                }
                Constraint::Symmetric { a, b, line } => {
                    need_point(a)?;
                    need_point(b)?;
                    need_entity(i, line, "line")?;
                    if a == b {
                        return Err(SketchError::InvalidValue(format!(
                            "constraint {i}: symmetry needs two different points"
                        )));
                    }
                }
                Constraint::PointOnLine { point, line } => {
                    need_point(point)?;
                    if let Entity::Line { a, b, .. } = need_entity(i, line, "line")?
                        && (point == a || point == b)
                    {
                        return Err(SketchError::InvalidValue(format!(
                            "constraint {i}: point {point:?} is an endpoint of line {line:?} \
                             (point-on-line needs a separate point)"
                        )));
                    }
                }
            }
            if let Some(d) = c.dimension() {
                if d.driven {
                    let drivable = matches!(
                        c,
                        Constraint::HorizontalDistance { .. }
                            | Constraint::VerticalDistance { .. }
                            | Constraint::Distance { .. }
                            | Constraint::Diameter { .. }
                            | Constraint::Radius { .. }
                            | Constraint::EllipseRadius { .. }
                            | Constraint::Angle { .. }
                    );
                    if !drivable || !d.value.is_finite() {
                        return Err(SketchError::InvalidValue(format!(
                            "constraint {i}: only a finite distance, diameter, radius or angle \
                             can be a driven dimension"
                        )));
                    }
                } else {
                    check_dim_value(&d.name, d.value)?;
                }
                // One circular pattern's copies share their angle dimension
                // (same name and value, all PatternRotate).
                let shared = |name: &str| {
                    let mut it = self
                        .constraints
                        .iter()
                        .filter_map(|c| c.dimension().map(|d| (c, d)))
                        .filter(|(_, d)| d.name == name);
                    it.clone().all(|(c, x)| {
                        matches!(c, Constraint::PatternRotate { .. }) && x.value == d.value
                    }) && it.next().is_some()
                };
                if !d.name.is_empty()
                    && !names.insert(d.name.as_str())
                    && !(matches!(c, Constraint::PatternRotate { .. }) && shared(&d.name))
                {
                    return Err(SketchError::DuplicateDimension(d.name.clone()));
                }
            }
        }
        if let Some(o) = &self.origin {
            need_point(o)?;
        }
        if let Some(c) = self.construction.iter().find(|c| !kinds.contains_key(c)) {
            return Err(SketchError::MissingEntity(*c));
        }
        text::validate_texts(self)?;
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
            .filter(|e| matches!(e, Entity::Circle { .. } | Entity::Ellipse { .. }))
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
            if let Entity::Circle { radius, .. } | Entity::Ellipse { minor: radius, .. } = e {
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
        for t in &self.texts {
            let p = t.spec.position;
            s = s.max(p[0].abs()).max(p[1].abs()).max(t.spec.height.abs());
        }
        if s.is_finite() { s } else { 1.0 }
    }

    /// Solve the constraint system. Coordinates/radii are written back only when
    /// the status is FullyConstrained, UnderConstrained or OverConstrained
    /// (consistent redundancy); otherwise the sketch is left unchanged.
    pub fn solve(&mut self) -> SolveReport {
        let r = solver::solve(self);
        if r.is_success() {
            self.refresh_driven();
        }
        r
    }

    /// The value constraint `c`'s dimension measures in the current geometry
    /// (mm; degrees for angles; signed for horizontal/vertical distances).
    pub fn measure(&self, c: &Constraint) -> Option<f64> {
        let p = |id: &PointId| self.point_xy(*id);
        let radius = |e: &EntityId| match self.entity(*e)? {
            Entity::Circle { radius, .. } => Some(*radius),
            Entity::Arc { id, .. } => self.arc_geometry(*id).map(|g| g.radius),
            _ => None,
        };
        let dir = |l: &EntityId| match self.entity(*l)? {
            Entity::Line { a, b, .. } => {
                let (pa, pb) = (p(a)?, p(b)?);
                Some([pb[0] - pa[0], pb[1] - pa[1]])
            }
            _ => None,
        };
        match c {
            Constraint::HorizontalDistance { a, b, .. } => Some(p(b)?[0] - p(a)?[0]),
            Constraint::VerticalDistance { a, b, .. } => Some(p(b)?[1] - p(a)?[1]),
            Constraint::Distance { a, b, .. } => {
                let (u, v) = (p(a)?, p(b)?);
                Some((v[0] - u[0]).hypot(v[1] - u[1]))
            }
            Constraint::Diameter { circle, .. } => radius(circle).map(|r| 2.0 * r),
            Constraint::Radius { entity, .. } => radius(entity),
            Constraint::EllipseRadius { ellipse, major, .. } => match self.entity(*ellipse)? {
                Entity::Ellipse {
                    center,
                    major: m,
                    minor,
                    ..
                } => {
                    if *major {
                        let (c0, m0) = (p(center)?, p(m)?);
                        Some((m0[0] - c0[0]).hypot(m0[1] - c0[1]))
                    } else {
                        Some(*minor)
                    }
                }
                _ => None,
            },
            Constraint::Angle { a, b, .. } => {
                let (u, v) = (dir(a)?, dir(b)?);
                let cross = u[0] * v[1] - u[1] * v[0];
                let dot = u[0] * v[0] + u[1] * v[1];
                Some(cross.atan2(dot).to_degrees())
            }
            _ => None,
        }
    }

    /// Would adding dimension constraint `c` (at its value) make the sketch
    /// consistently over-defined? Such a dimension is added driven instead
    /// of refused (a conflicting one, whose value disagrees, stays refused).
    pub fn dimension_is_redundant(&self, c: &Constraint) -> bool {
        if c.dimension().is_none_or(|d| d.driven) {
            return false;
        }
        let mut t = self.clone();
        t.constraints.push(c.clone());
        let r = t.solve();
        if r.status != SolveStatus::OverConstrained {
            return false;
        }
        // Named among the redundant rows; or, when diagnosis was skipped
        // (large sketch), the sketch was not over-defined before.
        r.conflicting.contains(&(t.constraints.len() - 1))
            || (r.conflicting.is_empty()
                && self.clone().solve().status != SolveStatus::OverConstrained)
    }

    /// Refresh every driven dimension to the value it measures.
    pub fn refresh_driven(&mut self) {
        let values: Vec<Option<f64>> = self
            .constraints
            .iter()
            .map(|c| {
                c.dimension()
                    .filter(|d| d.driven)
                    .and_then(|_| self.measure(c))
            })
            .collect();
        for (c, v) in self.constraints.iter_mut().zip(values) {
            if let (Some(d), Some(v)) = (c.dimension_mut(), v)
                && v.is_finite()
            {
                d.value = v;
            }
        }
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

    /// [`Sketch::profile`] plus the entity generating each segment (polygon edge
    /// `i` runs from vertex `i` to vertex `i + 1`; a circle profile has one entry).
    /// Entity ids are stable, so they name generated side faces persistently.
    pub fn profile_with_entities(&self) -> Result<(Profile, Vec<EntityId>), SketchError> {
        profile::extract_with_entities(self)
    }

    /// The profile region: one outer loop plus inner loops (holes), each with its
    /// generating entities. A single-loop sketch gives a region without holes;
    /// see the `region` module docs for the nesting rules.
    pub fn profile_region(&self) -> Result<Region, SketchError> {
        region::extract_region(self)
    }

    /// Every bounded region of the sketch: each closed loop minus the loops
    /// directly inside it (islands at any depth, several outer loops allowed),
    /// with its persistent [`RegionRef`]. Ordered by (depth, identity).
    pub fn regions(&self) -> Result<Vec<SketchRegion>, SketchError> {
        region::regions(self)
    }

    /// The region with identity `id`, `None` if no region has it any more.
    pub fn region_by_ref(&self, id: &RegionRef) -> Result<Option<Region>, SketchError> {
        region::region_by_ref(self, id)
    }

    /// The region containing sketch point `p` (`None` outside every loop or on
    /// a boundary).
    pub fn region_at(&self, p: [f64; 2]) -> Result<Option<RegionRef>, SketchError> {
        region::region_at(self, p)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_edit;

#[cfg(test)]
mod tests_arc;

#[cfg(test)]
mod tests_angle;
#[cfg(test)]
mod tests_constraints;
#[cfg(test)]
mod tests_curves;
#[cfg(test)]
mod tests_driven;
#[cfg(test)]
mod tests_fillet;
#[cfg(test)]
mod tests_guard;
#[cfg(test)]
mod tests_lines;
#[cfg(test)]
mod tests_offset;
#[cfg(test)]
mod tests_on_circle;
#[cfg(test)]
mod tests_region;
#[cfg(test)]
mod tests_regions_pick;
#[cfg(test)]
mod tests_trim;
