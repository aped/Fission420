//! Editing helpers used by the interactive sketch editor: id allocation,
//! geometry insertion/removal, human-readable constraint labels, and
//! constraint-respecting drags.
//!
//! Drags never bypass constraints. A drag first tries to pin the dragged
//! point (or circle radius) at the target with temporary equations; if the
//! constraints forbid that, it tries pinning only x or only y (sliding along a
//! constraint), and otherwise leaves the sketch unchanged and reports which
//! constraints block the motion.

use crate::solver::{self, Pin};
use crate::{
    Constraint, Dimension, Entity, EntityId, PointId, Sketch, SketchPoint, SolveReport, SolveStatus,
};
use std::collections::HashSet;

/// How far a drag got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragOutcome {
    /// The dragged item reached the target exactly.
    Reached,
    /// Constraints allowed motion along one axis only (closest reachable).
    Partial,
    /// Constraints forbid moving toward the target; sketch unchanged.
    Blocked,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DragResult {
    pub outcome: DragOutcome,
    /// Solve report of the sketch after the drag (without pins).
    pub report: SolveReport,
    /// Constraint indices that prevent reaching the target (Partial/Blocked).
    pub blocked_by: Vec<usize>,
}

impl Sketch {
    pub fn next_point_id(&self) -> PointId {
        PointId(
            self.points
                .iter()
                .map(|p| p.id.0.saturating_add(1))
                .max()
                .unwrap_or(0),
        )
    }

    pub fn next_entity_id(&self) -> EntityId {
        EntityId(
            self.entities
                .iter()
                .map(|e| e.id().0.saturating_add(1))
                .max()
                .unwrap_or(1)
                .max(1)
                .max(self.retired_entity_ids),
        )
    }

    pub fn point(&self, id: PointId) -> Option<&SketchPoint> {
        self.points.iter().find(|p| p.id == id)
    }

    pub fn point_xy(&self, id: PointId) -> Option<[f64; 2]> {
        self.point(id).map(|p| [p.x, p.y])
    }

    pub fn entity(&self, id: EntityId) -> Option<&Entity> {
        self.entities.iter().find(|e| e.id() == id)
    }

    pub fn add_point(&mut self, x: f64, y: f64) -> PointId {
        let id = self.next_point_id();
        self.points.push(SketchPoint { id, x, y });
        id
    }

    pub fn add_line(&mut self, a: PointId, b: PointId) -> EntityId {
        let id = self.next_entity_id();
        self.entities.push(Entity::Line { id, a, b });
        id
    }

    pub fn add_circle(&mut self, center: PointId, radius: f64) -> EntityId {
        let id = self.next_entity_id();
        self.entities.push(Entity::Circle { id, center, radius });
        id
    }

    /// Mark entity `id` as construction (`on`) or normal geometry. Construction
    /// entities are solved and constrained as usual but ignored by profile
    /// extraction. Returns false (sketch unchanged) if the entity does not exist.
    pub fn set_construction(&mut self, id: EntityId, on: bool) -> bool {
        if self.entity(id).is_none() {
            return false;
        }
        if on {
            self.construction.insert(id);
        } else {
            self.construction.remove(&id);
        }
        true
    }

    /// True when entity `id` is construction geometry.
    pub fn is_construction(&self, id: EntityId) -> bool {
        self.construction.contains(&id)
    }

    /// The sketch-origin point, created (fixed at (0, 0)) if absent.
    pub fn ensure_origin(&mut self) -> PointId {
        if let Some(o) = self.origin
            && self.point(o).is_some()
        {
            return o;
        }
        let id = self.add_point(0.0, 0.0);
        self.constraints.push(Constraint::FixedPoint {
            p: id,
            x: 0.0,
            y: 0.0,
        });
        self.origin = Some(id);
        id
    }

    /// True when some entity uses `id` as an endpoint or centre.
    pub fn point_used_by_entity(&self, id: PointId) -> bool {
        self.entities.iter().any(|e| e.points().contains(&id))
    }

    fn used_by_entity(&self, id: PointId) -> bool {
        self.point_used_by_entity(id)
    }

    /// Remove an entity and everything that only existed for it:
    /// * constraints on the entity itself (Horizontal/Vertical/Diameter/Radius/Tangent/EqualRadius/
    ///   Parallel/Perpendicular/EqualLength/PointOnLine/PointOnCircle) and its construction flag;
    /// * its endpoints/centre (an arc's centre included) that no other entity uses, with every constraint
    ///   that references them;
    /// * construction points (points used by no entity, such as a rectangle
    ///   centre) that were linked to the removed geometry through constraints
    ///   and are no longer linked by any surviving constraint to a point that an
    ///   entity uses. A link to the origin alone does not keep them alive.
    ///
    /// The origin and unrelated standalone points are never removed.
    /// Returns false if the entity does not exist (sketch unchanged).
    pub fn remove_entity(&mut self, id: EntityId) -> bool {
        let Some(pos) = self.entities.iter().position(|e| e.id() == id) else {
            return false;
        };
        let e = self.entities.remove(pos);
        self.construction.remove(&id);
        self.retired_entity_ids = self.retired_entity_ids.max(id.0.saturating_add(1));
        let ends: Vec<PointId> = e.points();
        self.constraints
            .retain(|c| !constraint_entities(c).contains(&id));
        let is_construction = |s: &Sketch, q: PointId| s.origin != Some(q) && !s.used_by_entity(q);

        // Construction points reachable from the removed geometry via constraints.
        let mut seen: HashSet<PointId> = ends.iter().copied().collect();
        let mut frontier = ends.clone();
        let mut candidates: HashSet<PointId> = HashSet::new();
        while let Some(p) = frontier.pop() {
            for c in &self.constraints {
                let pts = constraint_points(c);
                if !pts.contains(&p) {
                    continue;
                }
                for q in pts {
                    if seen.insert(q) && is_construction(self, q) {
                        candidates.insert(q);
                        frontier.push(q);
                    }
                }
            }
        }
        let mut dead: HashSet<PointId> = ends
            .iter()
            .copied()
            .filter(|&p| is_construction(self, p))
            .collect();
        // Fixpoint: a candidate survives if a constraint without dead points links
        // it to an entity point or to another surviving candidate.
        let mut alive: HashSet<PointId> = HashSet::new();
        loop {
            let newly: Vec<PointId> = candidates
                .iter()
                .copied()
                .filter(|q| !alive.contains(q))
                .filter(|&q| {
                    self.constraints.iter().any(|c| {
                        let pts = constraint_points(c);
                        pts.contains(&q)
                            && !pts.iter().any(|r| dead.contains(r))
                            && pts.iter().any(|&r| {
                                r != q
                                    && self.origin != Some(r)
                                    && (self.used_by_entity(r) || alive.contains(&r))
                            })
                    })
                })
                .collect();
            if newly.is_empty() {
                break;
            }
            alive.extend(newly);
        }
        dead.extend(candidates.difference(&alive).copied());
        self.constraints
            .retain(|c| !constraint_points(c).iter().any(|p| dead.contains(p)));
        self.points.retain(|q| !dead.contains(&q.id));
        true
    }

    pub fn remove_constraint(&mut self, index: usize) -> Option<Constraint> {
        (index < self.constraints.len()).then(|| self.constraints.remove(index))
    }

    /// `preferred` if unused, else `preferred2`, `preferred3`, ...
    pub fn unique_dimension_name(&self, preferred: &str) -> String {
        let used = |n: &str| self.dimensions().iter().any(|(m, _)| m == n);
        if !used(preferred) {
            return preferred.to_string();
        }
        (2..)
            .map(|i| format!("{preferred}{i}"))
            .find(|n| !used(n))
            .unwrap_or_else(|| preferred.to_string())
    }

    /// Human-readable description of constraint `index` (for diagnostics/UI).
    pub fn constraint_label(&self, index: usize) -> String {
        let Some(c) = self.constraints.get(index) else {
            return format!("constraint #{index}");
        };
        let p = |id: &PointId| {
            if self.origin == Some(*id) {
                "origin".to_string()
            } else {
                format!("P{}", id.0)
            }
        };
        let dim = |d: &Dimension| format!("{} = {} mm", d.name, fmt_mm(d.value));
        match c {
            Constraint::FixedPoint { p: q, x, y } => {
                format!("Fixed {} at ({}, {})", p(q), fmt_mm(*x), fmt_mm(*y))
            }
            Constraint::Horizontal { line } => format!("Horizontal L{}", line.0),
            Constraint::Vertical { line } => format!("Vertical L{}", line.0),
            Constraint::HorizontalDistance { a, b, dim: d } => {
                format!("{} (horizontal {} to {})", dim(d), p(a), p(b))
            }
            Constraint::VerticalDistance { a, b, dim: d } => {
                format!("{} (vertical {} to {})", dim(d), p(a), p(b))
            }
            Constraint::Distance { a, b, dim: d } => {
                format!("{} (distance {} to {})", dim(d), p(a), p(b))
            }
            Constraint::SymmetricAboutPoint { a, b, center } => {
                format!("Symmetric {} / {} about {}", p(a), p(b), p(center))
            }
            Constraint::Diameter { circle, dim: d } => {
                format!("{} (diameter of C{})", dim(d), circle.0)
            }
            Constraint::Coincident { a, b } => format!("Coincident {} · {}", p(a), p(b)),
            Constraint::Radius { entity, dim: d } => {
                let prefix = match self.entity(*entity) {
                    Some(Entity::Circle { .. }) => "C",
                    _ => "A",
                };
                format!("{} (radius of {prefix}{})", dim(d), entity.0)
            }
            Constraint::Tangent { line, arc } => format!("Tangent L{} · A{}", line.0, arc.0),
            Constraint::EqualRadius { a, b } => {
                let name = |id: &EntityId| match self.entity(*id) {
                    Some(Entity::Circle { .. }) => format!("C{}", id.0),
                    _ => format!("A{}", id.0),
                };
                format!("Equal radius {} · {}", name(a), name(b))
            }
            Constraint::Parallel { a, b } => format!("Parallel L{}, L{}", a.0, b.0),
            Constraint::Perpendicular { a, b } => format!("Perpendicular L{}, L{}", a.0, b.0),
            Constraint::EqualLength { a, b } => format!("Equal length L{}, L{}", a.0, b.0),
            Constraint::Angle { a, b, dim } => {
                format!(
                    "{} = {}° (L{} → L{})",
                    dim.name,
                    fmt_mm(dim.value),
                    a.0,
                    b.0
                )
            }
            Constraint::PointOnLine { point, line } => format!("{} on L{}", p(point), line.0),
            Constraint::PointOnCircle { point, circle } => {
                let kind = match self.entity(*circle) {
                    Some(Entity::Circle { .. }) => "C",
                    _ => "A",
                };
                format!("{} on {kind}{}", p(point), circle.0)
            }
        }
    }

    /// Drag point `p` toward `target` (sketch-plane mm) without violating any
    /// constraint. On `Blocked` the sketch is unchanged. A missing point or a
    /// non-finite target is `Blocked` with an empty `blocked_by`.
    pub fn drag_point(&mut self, p: PointId, target: [f64; 2]) -> DragResult {
        if self.point(p).is_none() || !target.iter().all(|v| v.is_finite()) {
            return self.blocked(vec![]);
        }
        let pins = |mask: [bool; 2]| -> Vec<Pin> {
            (0..2)
                .filter(|&axis| mask[axis])
                .map(|axis| Pin::Coord {
                    p,
                    axis,
                    value: target[axis],
                })
                .collect()
        };
        let mut moved = self.clone();
        if let Some(q) = moved.points.iter_mut().find(|q| q.id == p) {
            (q.x, q.y) = (target[0], target[1]);
        }
        self.drag_with(
            &moved,
            &pins([true, true]),
            &[pins([true, false]), pins([false, true])],
            |s| {
                s.point_xy(p)
                    .map(|q| (q[0] - target[0]).hypot(q[1] - target[1]))
                    .unwrap_or(f64::INFINITY)
            },
        )
    }

    /// Drag a circle's radius toward `radius` (mm, finite and > 0). A missing
    /// circle or invalid radius is `Blocked` with an empty `blocked_by`.
    pub fn drag_radius(&mut self, circle: EntityId, radius: f64) -> DragResult {
        let is_circle = matches!(self.entity(circle), Some(Entity::Circle { .. }));
        if !is_circle || !radius.is_finite() || radius <= 0.0 {
            return self.blocked(vec![]);
        }
        let pin = [Pin::Radius {
            circle,
            value: radius,
        }];
        let mut moved = self.clone();
        for e in &mut moved.entities {
            if let Entity::Circle { id, radius: r, .. } = e
                && *id == circle
            {
                *r = radius;
            }
        }
        self.drag_with(&moved, &pin, &[], |_| 0.0)
    }

    fn blocked(&self, blocked_by: Vec<usize>) -> DragResult {
        DragResult {
            outcome: DragOutcome::Blocked,
            report: self.clone().solve(),
            blocked_by,
        }
    }

    /// Largest change of any solver unknown between `self` and `other` (same
    /// topology), mm.
    fn max_change(&self, other: &Sketch) -> f64 {
        let pts = self
            .points
            .iter()
            .zip(&other.points)
            .map(|(a, b)| (a.x - b.x).abs().max((a.y - b.y).abs()));
        let radii = self
            .entities
            .iter()
            .zip(&other.entities)
            .map(|(a, b)| match (a, b) {
                (Entity::Circle { radius: r1, .. }, Entity::Circle { radius: r2, .. }) => {
                    (r1 - r2).abs()
                }
                _ => 0.0,
            });
        pts.chain(radii).fold(0.0, f64::max)
    }

    fn drag_with(
        &mut self,
        moved: &Sketch,
        full: &[Pin],
        partial: &[Vec<Pin>],
        miss: impl Fn(&Sketch) -> f64,
    ) -> DragResult {
        let ok = |r: &SolveReport| r.is_success();
        let mut first = moved.clone();
        let r1 = solver::solve_with(&mut first, full);
        if ok(&r1) {
            let report = first.solve();
            if report.is_success() {
                *self = first;
                return DragResult {
                    outcome: DragOutcome::Reached,
                    report,
                    blocked_by: vec![],
                };
            }
        }
        let mut blocked_by = r1.conflicting.clone();
        let mut best: Option<(f64, Sketch)> = None;
        for pins in partial {
            let mut s = moved.clone();
            let rp = solver::solve_with(&mut s, pins);
            if ok(&rp) {
                let d = miss(&s);
                if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                    best = Some((d, s));
                }
            } else if r1.conflicting.is_empty() {
                // No single constraint explains the full block (e.g. both axes
                // are dimensioned): report the per-axis culprits instead.
                blocked_by.extend(rp.conflicting);
            }
        }
        blocked_by.sort_unstable();
        blocked_by.dedup();
        let tol = crate::solve_tolerance(self.scale());
        if let Some((_, mut s)) = best {
            let report = s.solve();
            // A partial solve that does not actually move anything is a block.
            if report.is_success() && self.max_change(&s) > tol {
                *self = s;
                return DragResult {
                    outcome: DragOutcome::Partial,
                    report,
                    blocked_by,
                };
            }
        }
        self.blocked(blocked_by)
    }

    /// Solve status shorthand: true when the sketch is solvable and DOF = 0.
    pub fn is_fully_constrained(&self) -> bool {
        self.clone().solve().status == SolveStatus::FullyConstrained
    }
}

/// Points a constraint references (not the line entities' endpoints).
pub(crate) fn constraint_points(c: &Constraint) -> Vec<PointId> {
    match c {
        Constraint::FixedPoint { p, .. } => vec![*p],
        Constraint::HorizontalDistance { a, b, .. }
        | Constraint::VerticalDistance { a, b, .. }
        | Constraint::Distance { a, b, .. }
        | Constraint::Coincident { a, b } => vec![*a, *b],
        Constraint::SymmetricAboutPoint { a, b, center } => vec![*a, *b, *center],
        Constraint::PointOnLine { point, .. } | Constraint::PointOnCircle { point, .. } => {
            vec![*point]
        }
        Constraint::Horizontal { .. }
        | Constraint::Vertical { .. }
        | Constraint::Diameter { .. }
        | Constraint::Radius { .. }
        | Constraint::Tangent { .. }
        | Constraint::EqualRadius { .. }
        | Constraint::Parallel { .. }
        | Constraint::Perpendicular { .. }
        | Constraint::EqualLength { .. }
        | Constraint::Angle { .. } => vec![],
    }
}

/// Entities a constraint references directly.
pub(crate) fn constraint_entities(c: &Constraint) -> Vec<EntityId> {
    match c {
        Constraint::Horizontal { line } | Constraint::Vertical { line } => vec![*line],
        Constraint::Diameter { circle, .. } => vec![*circle],
        Constraint::Radius { entity, .. } => vec![*entity],
        Constraint::Tangent { line, arc } => vec![*line, *arc],
        Constraint::EqualRadius { a, b }
        | Constraint::Parallel { a, b }
        | Constraint::Perpendicular { a, b }
        | Constraint::EqualLength { a, b }
        | Constraint::Angle { a, b, .. } => vec![*a, *b],
        Constraint::PointOnLine { line, .. } => vec![*line],
        Constraint::PointOnCircle { circle, .. } => vec![*circle],
        _ => vec![],
    }
}

/// Compact mm formatting for labels (up to 4 decimals, trailing zeros trimmed).
pub fn fmt_mm(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}
