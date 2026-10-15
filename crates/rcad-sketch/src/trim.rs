//! Trim, Extend and Break: curve edits that cut lines, arcs and circles at
//! their intersections with the other curves of the sketch.
//!
//! These change topology only; they do not solve. The caller solves the
//! result in a transaction (the editor's `try_change`, the command layer's
//! `SketchOp`) and keeps the previous sketch when the solve fails.
//!
//! Identity rules (entity ids are persistent face names):
//! * the edited curve keeps its id; a trim of a circle turns it into an arc
//!   with the same id and centre point;
//! * pieces split off get fresh ids;
//! * cut ends reuse an existing point at the crossing when one is used by a
//!   curve (so a profile closes), else get a new point held on the cutting
//!   curve(s) by `PointOnLine` / `PointOnCircle`.
//!
//! Constraints whose meaning depended on the removed span are dropped and
//! counted (equal length, a length dimension spanning a split line, a tangency
//! no piece still touches, constraints on a removed free end).
//!
//! Intersections are computed for lines, arcs and circles. A sketch containing
//! an ellipse or spline is refused (its intersections are not computed, and
//! ignoring them would give a plausible but wrong result).

use crate::{ArcGeometry, Constraint, Entity, EntityId, PointId, Sketch, SketchError};
use std::f64::consts::TAU;

/// What a trim, extend or break did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurveEdit {
    /// Entities created (split-off pieces).
    pub created: Vec<EntityId>,
    /// Entities removed (a curve with no crossing is deleted by Trim).
    pub removed: Vec<EntityId>,
    /// Constraints removed because they no longer applied.
    pub dropped: usize,
    /// New ends that lie on no other curve (not held by any constraint).
    pub free_ends: usize,
    /// Length dimensions removed because the line they measured was split.
    pub split_dimensions: Vec<String>,
}

impl CurveEdit {
    /// Status-line text: `verb` is "Trimmed", "Extended" or "Broken".
    pub fn message(&self, verb: &str) -> String {
        let base = match (self.dropped, verb) {
            (0, _) => format!("{verb}."),
            (n, "Extended") => format!("{verb}; {n} constraint(s) on the old end were removed."),
            (n, _) => format!("{verb}; {n} constraint(s) that no longer applied were removed."),
        };
        let dims = if self.split_dimensions.is_empty() {
            String::new()
        } else {
            format!(
                " The split line's length dimension {} was removed (it spanned both pieces).",
                self.split_dimensions
                    .iter()
                    .map(|n| format!("'{n}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let free = if self.free_ends == 0 {
            String::new()
        } else {
            format!(
                " {} new end(s) lie on no other curve and are not held.",
                self.free_ends
            )
        };
        format!("{base}{dims}{free}")
    }
}

fn refused(why: impl Into<String>) -> SketchError {
    SketchError::Refused(why.into())
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Distance from `p` to segment `a`-`b`.
pub fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    if l2 <= 0.0 {
        return dist(p, a);
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0);
    dist(p, [a[0] + t * dx, a[1] + t * dy])
}

/// Distance from `p` to an arc (to the nearest end outside its angular span).
pub fn arc_distance(g: &ArcGeometry, p: [f64; 2]) -> f64 {
    let ang = (p[1] - g.center[1]).atan2(p[0] - g.center[0]);
    let rel = (ang - g.start_angle).rem_euclid(TAU);
    if rel <= g.sweep {
        return (dist(p, g.center) - g.radius).abs();
    }
    let at = |a: f64| {
        [
            g.center[0] + g.radius * a.cos(),
            g.center[1] + g.radius * a.sin(),
        ]
    };
    dist(p, at(g.start_angle)).min(dist(p, at(g.start_angle + g.sweep)))
}

/// Does constraint `c` reference point `p` directly?
pub fn constraint_mentions(c: &Constraint, p: PointId) -> bool {
    match c {
        Constraint::FixedPoint { p: q, .. }
        | Constraint::FixedX { p: q, .. }
        | Constraint::FixedY { p: q, .. } => *q == p,
        Constraint::HorizontalDistance { a, b, .. }
        | Constraint::VerticalDistance { a, b, .. }
        | Constraint::Distance { a, b, .. }
        | Constraint::Coincident { a, b } => *a == p || *b == p,
        Constraint::SymmetricAboutPoint { a, b, center } => *a == p || *b == p || *center == p,
        Constraint::Symmetric { a, b, .. } => *a == p || *b == p,
        Constraint::PointOnLine { point, .. } | Constraint::PointOnCircle { point, .. } => {
            *point == p
        }
        _ => false,
    }
}

/// How a trim/extend end was joined to the curve it was cut at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CutEnd {
    /// An existing point already lies there: its id is shared, so profiles close.
    Shared,
    /// New point held on the cutting curve(s): `PointOnLine` for a line,
    /// `PointOnCircle` for a circle or arc (both at a line/circle crossing).
    Held,
    /// New point on no other curve (nothing to hold it on).
    Free,
}

/// End point for a cut at `x`. Reuses an existing point there (other than
/// `own`, the edited curve's points) when a curve uses it, else adds one and
/// keeps it on the cutting curves (not in `skip`) through `x`: a line whose
/// segment holds `x` (`PointOnLine`) and/or a circle or arc through `x`
/// (`PointOnCircle`).
pub fn cut_point(
    sk: &mut Sketch,
    x: [f64; 2],
    own: &[PointId],
    skip: &[EntityId],
) -> (PointId, CutEnd) {
    let tol = crate::solve_tolerance(sk.scale());
    let used = |sk: &Sketch, p: PointId| {
        sk.origin == Some(p)
            || sk
                .entities
                .iter()
                .any(|e| !skip.contains(&e.id()) && e.points().contains(&p))
    };
    if let Some(p) = sk
        .points
        .iter()
        .filter(|q| !own.contains(&q.id) && dist([q.x, q.y], x) <= tol)
        .map(|q| q.id)
        .find(|&q| used(sk, q))
    {
        return (p, CutEnd::Shared);
    }
    let line = sk.entities.iter().find_map(|e| match e {
        Entity::Line { id, a, b } if !skip.contains(id) => {
            let (pa, pb) = (sk.point_xy(*a)?, sk.point_xy(*b)?);
            (seg_dist(x, pa, pb) <= tol && dist(x, pa) > tol && dist(x, pb) > tol).then_some(*id)
        }
        _ => None,
    });
    let round = sk.entities.iter().find_map(|e| match e {
        Entity::Circle { id, center, radius } if !skip.contains(id) => {
            let c = sk.point_xy(*center)?;
            ((dist(x, c) - radius).abs() <= tol).then_some(*id)
        }
        Entity::Arc { id, .. } if !skip.contains(id) => {
            let g = sk.arc_geometry(*id)?;
            ((dist(x, g.center) - g.radius).abs() <= tol && arc_distance(&g, x) <= tol)
                .then_some(*id)
        }
        _ => None,
    });
    let n = sk.add_point(x[0], x[1]);
    if let Some(line) = line {
        sk.constraints
            .push(Constraint::PointOnLine { point: n, line });
    }
    if let Some(circle) = round {
        sk.constraints
            .push(Constraint::PointOnCircle { point: n, circle });
    }
    if line.is_some() || round.is_some() {
        (n, CutEnd::Held)
    } else {
        (n, CutEnd::Free)
    }
}

/// Drop `PointOnLine` / `PointOnCircle` constraints whose point has become an
/// end (or the centre) of that curve: implied, and refused by validation.
pub(crate) fn drop_implied_on_curve(sk: &mut Sketch) {
    let ends = |sk: &Sketch, l: EntityId| sk.entity(l).map(|x| x.points()).unwrap_or_default();
    let keep: Vec<bool> = sk
        .constraints
        .iter()
        .map(|c| match c {
            Constraint::PointOnLine { point, line } => !ends(sk, *line).contains(point),
            Constraint::PointOnCircle { point, circle } => !ends(sk, *circle).contains(point),
            _ => true,
        })
        .collect();
    let mut it = keep.into_iter();
    sk.constraints.retain(|_| it.next().unwrap_or(true));
}

/// After `e` was shortened/split into `e` + `pieces`: a tangency naming `e`
/// stays with whichever curve still ends at the tangent point, else it is
/// dropped. Returns the number dropped.
pub fn retarget_tangents(sk: &mut Sketch, e: EntityId, pieces: &[EntityId]) -> usize {
    // Endpoints only (an arc's centre is not where tangency holds).
    let ends = |sk: &Sketch, id: EntityId| match sk.entity(id) {
        Some(Entity::Line { a, b, .. }) => vec![*a, *b],
        Some(Entity::Arc { start, end, .. }) => vec![*start, *end],
        _ => vec![],
    };
    let shares = |sk: &Sketch, u: EntityId, v: EntityId| {
        let pv = ends(sk, v);
        ends(sk, u).iter().any(|p| pv.contains(p))
    };
    let before = sk.constraints.len();
    let old = std::mem::take(&mut sk.constraints);
    let mut keep = Vec::with_capacity(old.len());
    for c in old {
        let (x, y) = match c {
            Constraint::Tangent { line, arc } => (line, arc),
            Constraint::TangentArcs { a, b } => (a, b),
            _ => {
                keep.push(c);
                continue;
            }
        };
        if x != e && y != e {
            keep.push(c);
            continue;
        }
        let other = if x == e { y } else { x };
        let swap = |id: EntityId| {
            let (nx, ny) = if x == e { (id, y) } else { (x, id) };
            match c {
                Constraint::Tangent { .. } => Constraint::Tangent { line: nx, arc: ny },
                _ => Constraint::TangentArcs { a: nx, b: ny },
            }
        };
        if shares(sk, e, other) {
            keep.push(c);
        } else if let Some(pc) = pieces.iter().find(|pc| shares(sk, **pc, other)) {
            keep.push(swap(*pc));
        }
    }
    sk.constraints = keep;
    before - sk.constraints.len()
}

/// Remove ends detached by an edit that no curve uses any more, with their
/// constraints (the origin stays). Returns the number of constraints removed.
fn drop_freed(sk: &mut Sketch, freed: &[PointId]) -> usize {
    let mut dropped = 0;
    for &pt in freed {
        if Some(pt) != sk.origin && !sk.entities.iter().any(|x| x.points().contains(&pt)) {
            let before = sk.constraints.len();
            sk.constraints.retain(|c| !constraint_mentions(c, pt));
            dropped += before - sk.constraints.len();
            sk.points.retain(|x| x.id != pt);
        }
    }
    dropped
}

impl Sketch {
    /// Curves the current geometry has collapsed: lines whose ends coincide
    /// and arcs whose radius vanished (within the solve tolerance). A solve
    /// that converges to such a state (e.g. Horizontal plus Vertical on one
    /// line) is not a valid sketch edit.
    pub fn collapsed_curves(&self) -> Vec<EntityId> {
        let tol = crate::solve_tolerance(self.scale());
        self.entities
            .iter()
            .filter(|e| match e {
                Entity::Line { a, b, .. } => match (self.point_xy(*a), self.point_xy(*b)) {
                    (Some(p), Some(q)) => dist(p, q) <= tol,
                    _ => false,
                },
                Entity::Arc { center, start, .. } => {
                    match (self.point_xy(*center), self.point_xy(*start)) {
                        (Some(c), Some(p)) => dist(c, p) <= tol,
                        _ => false,
                    }
                }
                Entity::Circle { radius, .. } => *radius <= tol,
                _ => false,
            })
            .map(Entity::id)
            .collect()
    }

    /// Trim and Extend compute intersections with lines, arcs and circles
    /// only: in a sketch with an ellipse or spline they refuse rather than
    /// ignore those curves.
    pub fn free_form_refusal(&self, tool: &str) -> Result<(), SketchError> {
        if self
            .entities
            .iter()
            .any(|e| matches!(e, Entity::Ellipse { .. } | Entity::Spline { .. }))
        {
            return Err(refused(format!(
                "{tool} does not support sketches with ellipses or splines (intersections \
                 with them are not computed); delete and redraw the lines instead."
            )));
        }
        Ok(())
    }

    /// Parameters t along the line through `p -> q` (t = 0 at p, 1 at q) where
    /// other curves cross it: inside the segment, or (`beyond`) outside it.
    /// Sorted ascending.
    pub fn line_hits(&self, line: EntityId, p: [f64; 2], q: [f64; 2], beyond: bool) -> Vec<f64> {
        let d = [q[0] - p[0], q[1] - p[1]];
        let len2 = d[0] * d[0] + d[1] * d[1];
        let tol = crate::solve_tolerance(self.scale());
        let eps = tol / len2.sqrt().max(1e-12);
        let inside = |t: f64| {
            if beyond {
                t < -eps || t > 1.0 + eps
            } else {
                t > eps && t < 1.0 - eps
            }
        };
        let mut ts = Vec::new();
        let circle_hits = |c: [f64; 2], r: f64| -> Vec<f64> {
            // |p + t d - c|^2 = r^2
            let f = [p[0] - c[0], p[1] - c[1]];
            let (a, b, cc) = (
                len2,
                2.0 * (f[0] * d[0] + f[1] * d[1]),
                f[0] * f[0] + f[1] * f[1] - r * r,
            );
            let disc = b * b - 4.0 * a * cc;
            if disc < 0.0 || a <= 0.0 {
                return vec![];
            }
            let s = disc.sqrt();
            vec![(-b - s) / (2.0 * a), (-b + s) / (2.0 * a)]
        };
        for e in &self.entities {
            if e.id() == line {
                continue;
            }
            match e {
                Entity::Line { a, b, .. } => {
                    let (Some(c), Some(g)) = (self.point_xy(*a), self.point_xy(*b)) else {
                        continue;
                    };
                    let k = [g[0] - c[0], g[1] - c[1]];
                    let den = d[0] * k[1] - d[1] * k[0];
                    if den.abs() < 1e-12 * len2.sqrt() * k[0].hypot(k[1]) {
                        continue; // parallel
                    }
                    let w = [c[0] - p[0], c[1] - p[1]];
                    let t = (w[0] * k[1] - w[1] * k[0]) / den;
                    let u = (w[0] * d[1] - w[1] * d[0]) / den;
                    if inside(t) && (-1e-9..=1.0 + 1e-9).contains(&u) {
                        ts.push(t);
                    }
                }
                // Refused up front (see `free_form_refusal`).
                Entity::Ellipse { .. } | Entity::Spline { .. } => {}
                Entity::Circle { center, radius, .. } => {
                    if let Some(c) = self.point_xy(*center) {
                        ts.extend(circle_hits(c, *radius).into_iter().filter(|t| inside(*t)));
                    }
                }
                Entity::Arc { id, .. } => {
                    let Some(g) = self.arc_geometry(*id) else {
                        continue;
                    };
                    for t in circle_hits(g.center, g.radius) {
                        let x = [p[0] + t * d[0], p[1] + t * d[1]];
                        let ang = (x[1] - g.center[1]).atan2(x[0] - g.center[0]);
                        let rel = (ang - g.start_angle).rem_euclid(TAU);
                        // Tolerant at both ends (rel just below 0 wraps to ~2π).
                        let ae = tol / g.radius;
                        if inside(t) && (rel <= g.sweep + ae || rel >= TAU - ae) {
                            ts.push(t);
                        }
                    }
                }
            }
        }
        ts.sort_by(f64::total_cmp);
        ts
    }

    /// Polar angles (about `c`) where other curves cross the circle (c, r).
    pub fn circle_cuts(&self, round: EntityId, c: [f64; 2], r: f64) -> Vec<f64> {
        let tol = crate::solve_tolerance(self.scale());
        let ang = |x: [f64; 2]| (x[1] - c[1]).atan2(x[0] - c[0]);
        let mut out = Vec::new();
        let on_arc = |g: &ArcGeometry, x: [f64; 2]| {
            let rel =
                ((x[1] - g.center[1]).atan2(x[0] - g.center[0]) - g.start_angle).rem_euclid(TAU);
            let ae = tol / g.radius;
            rel <= g.sweep + ae || rel >= TAU - ae
        };
        let circle_circle = |c2: [f64; 2], r2: f64| -> Vec<[f64; 2]> {
            let dx = [c2[0] - c[0], c2[1] - c[1]];
            let dd = dx[0].hypot(dx[1]);
            if dd <= tol || dd > r + r2 + tol || dd < (r - r2).abs() - tol {
                return vec![];
            }
            let u = [dx[0] / dd, dx[1] / dd];
            let a = (r * r - r2 * r2 + dd * dd) / (2.0 * dd);
            let h = (r * r - a * a).max(0.0).sqrt();
            let m = [c[0] + a * u[0], c[1] + a * u[1]];
            vec![
                [m[0] - h * u[1], m[1] + h * u[0]],
                [m[0] + h * u[1], m[1] - h * u[0]],
            ]
        };
        for e in &self.entities {
            if e.id() == round {
                continue;
            }
            match e {
                Entity::Line { a, b, .. } => {
                    let (Some(p), Some(q)) = (self.point_xy(*a), self.point_xy(*b)) else {
                        continue;
                    };
                    let d = [q[0] - p[0], q[1] - p[1]];
                    let f = [p[0] - c[0], p[1] - c[1]];
                    let (aa, bb, cc) = (
                        d[0] * d[0] + d[1] * d[1],
                        2.0 * (f[0] * d[0] + f[1] * d[1]),
                        f[0] * f[0] + f[1] * f[1] - r * r,
                    );
                    let disc = bb * bb - 4.0 * aa * cc;
                    if disc < 0.0 || aa == 0.0 {
                        continue;
                    }
                    for t in [
                        (-bb - disc.sqrt()) / (2.0 * aa),
                        (-bb + disc.sqrt()) / (2.0 * aa),
                    ] {
                        if (-1e-9..=1.0 + 1e-9).contains(&t) {
                            out.push(ang([p[0] + t * d[0], p[1] + t * d[1]]));
                        }
                    }
                }
                // Refused up front (see `free_form_refusal`).
                Entity::Ellipse { .. } | Entity::Spline { .. } => {}
                Entity::Circle { center, radius, .. } => {
                    if let Some(c2) = self.point_xy(*center) {
                        out.extend(circle_circle(c2, *radius).into_iter().map(ang));
                    }
                }
                Entity::Arc { id, .. } => {
                    if let Some(g) = self.arc_geometry(*id) {
                        out.extend(
                            circle_circle(g.center, g.radius)
                                .into_iter()
                                .filter(|x| on_arc(&g, *x))
                                .map(ang),
                        );
                    }
                }
            }
        }
        out
    }

    /// Trim curve `e` at `at` (a point on or near the span to remove): the span
    /// between the nearest crossings either side of `at` (or a free end) is
    /// removed. A curve with no crossing is deleted. A line cut in the middle
    /// splits into two lines (the second one new, with the first's
    /// horizontal/vertical constraint); a circle becomes an arc of the same id
    /// and centre; an arc cut in the middle splits into two arcs with an
    /// equal-radius constraint. Does not solve.
    pub fn trim(&mut self, e: EntityId, at: [f64; 2]) -> Result<CurveEdit, SketchError> {
        self.free_form_refusal("Trim")?;
        match self.entity(e).cloned() {
            None => Err(SketchError::MissingEntity(e)),
            Some(Entity::Line { a, b, .. }) => self.trim_line(e, a, b, at),
            Some(Entity::Circle { .. } | Entity::Arc { .. }) => self.trim_round(e, at),
            Some(_) => Err(refused("Trim works on lines, arcs and circles.")),
        }
    }

    fn trim_line(
        &mut self,
        e: EntityId,
        a: PointId,
        b: PointId,
        at: [f64; 2],
    ) -> Result<CurveEdit, SketchError> {
        let (Some(p), Some(q)) = (self.point_xy(a), self.point_xy(b)) else {
            return Err(SketchError::MissingPoint(a));
        };
        let d = [q[0] - p[0], q[1] - p[1]];
        let len2 = d[0] * d[0] + d[1] * d[1];
        let tc = (((at[0] - p[0]) * d[0] + (at[1] - p[1]) * d[1]) / len2).clamp(0.0, 1.0);
        let cuts = self.line_hits(e, p, q, false);
        let lo = cuts.iter().copied().filter(|t| *t < tc).fold(0.0, f64::max);
        let hi = cuts.iter().copied().filter(|t| *t > tc).fold(1.0, f64::min);
        let pt = |t: f64| [p[0] + t * d[0], p[1] + t * d[1]];
        let mut out = CurveEdit::default();
        if lo <= 0.0 && hi >= 1.0 {
            self.remove_entity(e);
            out.removed.push(e);
            return Ok(out);
        }
        // Constraints whose meaning depends on this line's extent.
        let before = self.constraints.len();
        self.constraints
            .retain(|c| !matches!(c, Constraint::EqualLength { a: x, b: y } if *x == e || *y == e));
        out.dropped += before - self.constraints.len();
        let point = |sk: &mut Sketch, t: f64, out: &mut CurveEdit| {
            let (n, how) = cut_point(sk, pt(t), &[a, b], &[e]);
            out.free_ends += usize::from(how == CutEnd::Free);
            n
        };
        let set_ends = |sk: &mut Sketch, na: PointId, nb: PointId| {
            if let Some(Entity::Line { a, b, .. }) = sk.entities.iter_mut().find(|x| x.id() == e) {
                (*a, *b) = (na, nb);
            }
        };
        let mut freed = Vec::new();
        if lo <= 0.0 {
            let n = point(self, hi, &mut out);
            set_ends(self, n, b);
            freed.push(a);
        } else if hi >= 1.0 {
            let n = point(self, lo, &mut out);
            set_ends(self, a, n);
            freed.push(b);
        } else {
            let n0 = point(self, lo, &mut out);
            let n1 = point(self, hi, &mut out);
            set_ends(self, a, n0);
            out.split_dimensions = self.drop_span_dimensions(a, b);
            let l2 = self.add_line(n1, b);
            if self.is_construction(e) {
                self.construction.insert(l2);
            }
            out.created.push(l2);
            let hv: Vec<Constraint> = self
                .constraints
                .iter()
                .filter_map(|c| match c {
                    Constraint::Horizontal { line } if *line == e => {
                        Some(Constraint::Horizontal { line: l2 })
                    }
                    Constraint::Vertical { line } if *line == e => {
                        Some(Constraint::Vertical { line: l2 })
                    }
                    _ => None,
                })
                .collect();
            self.constraints.extend(hv);
        }
        out.dropped += retarget_tangents(self, e, &out.created);
        drop_implied_on_curve(self);
        out.dropped += drop_freed(self, &freed);
        Ok(out)
    }

    /// A dimension between a split line's original ends spans two separate
    /// lines and no longer measures a length; no piece keeps both measured
    /// points, so it is removed. Returns the removed names.
    fn drop_span_dimensions(&mut self, a: PointId, b: PointId) -> Vec<String> {
        let mut names = Vec::new();
        self.constraints.retain(|c| match c {
            Constraint::Distance { a: x, b: y, dim }
            | Constraint::HorizontalDistance { a: x, b: y, dim }
            | Constraint::VerticalDistance { a: x, b: y, dim }
                if (*x == a && *y == b) || (*x == b && *y == a) =>
            {
                names.push(dim.name.clone());
                false
            }
            _ => true,
        });
        names
    }

    /// Circle or arc data for a cut: (centre, radius, start angle, sweep,
    /// centre point, is circle).
    fn round_frame(&self, e: EntityId) -> Option<([f64; 2], f64, f64, f64, PointId, bool)> {
        match self.entity(e)? {
            Entity::Circle { center, radius, .. } => {
                Some((self.point_xy(*center)?, *radius, 0.0, TAU, *center, true))
            }
            Entity::Arc { id, center, .. } => {
                let g = self.arc_geometry(*id)?;
                Some((g.center, g.radius, g.start_angle, g.sweep, *center, false))
            }
            _ => None,
        }
    }

    /// Crossings of a circle/arc as angles relative to its start, sorted,
    /// deduplicated; for an arc only those strictly inside its span.
    fn round_cuts(&self, e: EntityId) -> Option<(Vec<f64>, f64)> {
        let (c, r, start, sweep, _, is_circle) = self.round_frame(e)?;
        let rel = |a: f64| (a - start).rem_euclid(TAU);
        let eps = crate::solve_tolerance(self.scale()) / r;
        let mut cuts: Vec<f64> = self
            .circle_cuts(e, c, r)
            .into_iter()
            .map(rel)
            .filter(|t| is_circle || (*t > eps && *t < sweep - eps))
            .collect();
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|x, y| (*x - *y).abs() <= eps);
        if is_circle && cuts.len() > 1 && cuts[0] + TAU - cuts[cuts.len() - 1] <= eps {
            cuts.pop(); // the same crossing either side of angle 0
        }
        Some((cuts, eps))
    }

    /// Replace circle `e` by an arc of the same id and centre running CCW from
    /// angle `from` to `to` (relative to angle 0); its diameter dimension
    /// becomes a radius dimension, constraints valid only for a full circle
    /// are dropped. Returns the number dropped.
    fn circle_to_arc(&mut self, e: EntityId, s_id: PointId, e_id: PointId) -> usize {
        let Some(Entity::Circle { center, .. }) = self.entity(e).cloned() else {
            return 0;
        };
        if let Some(slot) = self.entities.iter_mut().find(|x| x.id() == e) {
            *slot = Entity::Arc {
                id: e,
                center,
                start: s_id,
                end: e_id,
            };
        }
        let before = self.constraints.len();
        let mut keep = Vec::new();
        for k in std::mem::take(&mut self.constraints) {
            match k {
                Constraint::Diameter { circle, mut dim } if circle == e => {
                    dim.value /= 2.0;
                    keep.push(Constraint::Radius { entity: e, dim });
                }
                // Valid for an arc with the same id as they were for the circle.
                k @ (Constraint::Radius { .. }
                | Constraint::EqualRadius { .. }
                | Constraint::PointOnCircle { .. }) => keep.push(k),
                k if k.entities().contains(&e) => {}
                k => keep.push(k),
            }
        }
        self.constraints = keep;
        before.saturating_sub(self.constraints.len())
    }

    /// Trim a circle (to an arc) or an arc at the nearest crossings around `at`.
    fn trim_round(&mut self, e: EntityId, at: [f64; 2]) -> Result<CurveEdit, SketchError> {
        let (c, r, start, sweep, center_pt, is_circle) =
            self.round_frame(e).ok_or(SketchError::MissingEntity(e))?;
        let (cuts, _) = self.round_cuts(e).ok_or(SketchError::MissingEntity(e))?;
        let rel = |a: f64| (a - start).rem_euclid(TAU);
        let tc = rel((at[1] - c[1]).atan2(at[0] - c[0]));
        let on = |t: f64| [c[0] + r * (start + t).cos(), c[1] + r * (start + t).sin()];
        let mut out = CurveEdit::default();
        if is_circle {
            if cuts.is_empty() {
                self.remove_entity(e);
                out.removed.push(e);
                return Ok(out);
            }
            if cuts.len() < 2 {
                return Err(refused("A circle needs two crossings to trim."));
            }
            // Removed interval (lo, hi) around the click, cyclically.
            let hi = cuts
                .iter()
                .copied()
                .find(|t| *t > tc)
                .unwrap_or(cuts[0] + TAU);
            let lo = cuts
                .iter()
                .copied()
                .rev()
                .find(|t| *t < tc)
                .unwrap_or(cuts[cuts.len() - 1] - TAU);
            // Keep the arc CCW from hi to lo (+ 2π).
            let (s_id, h0) = cut_point(self, on(hi), &[], &[e]);
            let (e_id, h1) = cut_point(self, on(lo), &[], &[e]);
            out.free_ends += usize::from(h0 == CutEnd::Free) + usize::from(h1 == CutEnd::Free);
            out.dropped += self.circle_to_arc(e, s_id, e_id);
            drop_implied_on_curve(self);
            return Ok(out);
        }
        let lo = cuts.iter().copied().filter(|t| *t < tc).fold(0.0, f64::max);
        let hi = cuts
            .iter()
            .copied()
            .filter(|t| *t > tc)
            .fold(sweep, f64::min);
        let Some(Entity::Arc {
            start: sp, end: ep, ..
        }) = self.entity(e).cloned()
        else {
            return Err(SketchError::MissingEntity(e));
        };
        if lo <= 0.0 && hi >= sweep {
            self.remove_entity(e);
            out.removed.push(e);
            return Ok(out);
        }
        let set = |sk: &mut Sketch, s: PointId, t: PointId| {
            if let Some(Entity::Arc { start, end, .. }) =
                sk.entities.iter_mut().find(|x| x.id() == e)
            {
                (*start, *end) = (s, t);
            }
        };
        let own = [sp, ep];
        let point = |sk: &mut Sketch, t: f64, out: &mut CurveEdit| {
            let (n, how) = cut_point(sk, on(t), &own, &[e]);
            out.free_ends += usize::from(how == CutEnd::Free);
            n
        };
        let mut freed = Vec::new();
        if lo <= 0.0 {
            let n = point(self, hi, &mut out);
            set(self, n, ep);
            freed.push(sp);
        } else if hi >= sweep {
            let n = point(self, lo, &mut out);
            set(self, sp, n);
            freed.push(ep);
        } else {
            let n0 = point(self, lo, &mut out);
            let n1 = point(self, hi, &mut out);
            set(self, sp, n0);
            let id = self.push_arc_piece(e, center_pt, n1, ep);
            out.created.push(id);
        }
        out.dropped += retarget_tangents(self, e, &out.created);
        drop_implied_on_curve(self);
        out.dropped += drop_freed(self, &freed);
        Ok(out)
    }

    /// A new arc piece of arc/circle `e` (same centre point, equal radius,
    /// same construction flag).
    fn push_arc_piece(
        &mut self,
        e: EntityId,
        center: PointId,
        start: PointId,
        end: PointId,
    ) -> EntityId {
        let id = self.next_entity_id();
        self.entities.push(Entity::Arc {
            id,
            center,
            start,
            end,
        });
        if self.is_construction(e) {
            self.construction.insert(id);
        }
        self.constraints
            .push(Constraint::EqualRadius { a: e, b: id });
        id
    }

    /// Extend the free end of line `e` nearest `at` (or `end`, when given) along
    /// the line to the nearest curve its extension crosses. The end must be
    /// used by no other curve. Constraints on the old end are dropped (the
    /// origin is never moved). Does not solve.
    pub fn extend(
        &mut self,
        e: EntityId,
        at: [f64; 2],
        end: Option<PointId>,
    ) -> Result<CurveEdit, SketchError> {
        self.free_form_refusal("Extend")?;
        let Some(Entity::Line { a, b, .. }) = self.entity(e).cloned() else {
            return match self.entity(e) {
                None => Err(SketchError::MissingEntity(e)),
                Some(_) => Err(refused("Extend works on lines only.")),
            };
        };
        let (Some(p), Some(q)) = (self.point_xy(a), self.point_xy(b)) else {
            return Err(SketchError::MissingPoint(a));
        };
        let d = [q[0] - p[0], q[1] - p[1]];
        let len2 = d[0] * d[0] + d[1] * d[1];
        let tc = ((at[0] - p[0]) * d[0] + (at[1] - p[1]) * d[1]) / len2;
        let at_b = end.map_or(tc >= 0.5, |pt| pt == b);
        let end = if at_b { b } else { a };
        if self
            .entities
            .iter()
            .any(|x| x.id() != e && x.points().contains(&end))
        {
            return Err(refused(
                "That end is connected to another curve; only a free end can be extended.",
            ));
        }
        let hits = self.line_hits(e, p, q, true);
        let t = if at_b {
            hits.iter()
                .copied()
                .filter(|t| *t > 1.0)
                .fold(f64::INFINITY, f64::min)
        } else {
            hits.iter()
                .copied()
                .filter(|t| *t < 0.0)
                .fold(f64::NEG_INFINITY, f64::max)
        };
        if !t.is_finite() {
            return Err(refused(
                "Nothing to extend to: no curve crosses the line's extension.",
            ));
        }
        let x = [p[0] + t * d[0], p[1] + t * d[1]];
        let origin = self.origin;
        let mut out = CurveEdit::default();
        let before = self.constraints.len();
        self.constraints.retain(|c| {
            !matches!(c, Constraint::EqualLength { a: u, b: v } if *u == e || *v == e)
                && !(Some(end) != origin && constraint_mentions(c, end))
        });
        out.dropped += before - self.constraints.len();
        let (n, how) = cut_point(self, x, &[a, b], &[e]);
        out.free_ends += usize::from(how == CutEnd::Free);
        if let Some(Entity::Line { a, b, .. }) = self.entities.iter_mut().find(|x| x.id() == e) {
            if at_b {
                *b = n;
            } else {
                *a = n;
            }
        }
        if Some(end) != origin {
            self.points.retain(|x| x.id != end);
        }
        drop_implied_on_curve(self);
        Ok(out)
    }

    /// Break curve `e` at the crossings either side of `at`: the span under
    /// `at` becomes its own curve and nothing is removed. A line keeps its id
    /// for its first piece; the other pieces are new lines held collinear with
    /// it (`PointOnLine` of their far ends). An arc's pieces share its centre
    /// and get an equal-radius constraint. A circle needs at least two
    /// crossings and becomes two arcs (the clicked span is the new one).
    /// Refused when the span under `at` has no crossing to break at. Does not
    /// solve.
    pub fn break_at(&mut self, e: EntityId, at: [f64; 2]) -> Result<CurveEdit, SketchError> {
        self.free_form_refusal("Break")?;
        let nothing = || refused("Nothing to break at: no other curve crosses this span.");
        match self.entity(e).cloned() {
            None => Err(SketchError::MissingEntity(e)),
            Some(Entity::Line { a, b, .. }) => {
                let (Some(p), Some(q)) = (self.point_xy(a), self.point_xy(b)) else {
                    return Err(SketchError::MissingPoint(a));
                };
                let d = [q[0] - p[0], q[1] - p[1]];
                let len2 = d[0] * d[0] + d[1] * d[1];
                let tc = (((at[0] - p[0]) * d[0] + (at[1] - p[1]) * d[1]) / len2).clamp(0.0, 1.0);
                let cuts = self.line_hits(e, p, q, false);
                let lo = cuts.iter().copied().filter(|t| *t < tc).fold(0.0, f64::max);
                let hi = cuts.iter().copied().filter(|t| *t > tc).fold(1.0, f64::min);
                if lo <= 0.0 && hi >= 1.0 {
                    return Err(nothing());
                }
                let pt = |t: f64| [p[0] + t * d[0], p[1] + t * d[1]];
                let mut out = CurveEdit::default();
                let before = self.constraints.len();
                self.constraints.retain(
                    |c| !matches!(c, Constraint::EqualLength { a: x, b: y } if *x == e || *y == e),
                );
                out.dropped += before - self.constraints.len();
                out.split_dimensions = self.drop_span_dimensions(a, b);
                // Cut parameters strictly inside the line, in order.
                let ts: Vec<f64> = [lo, hi]
                    .into_iter()
                    .filter(|t| *t > 0.0 && *t < 1.0)
                    .collect();
                let mut ends = vec![a];
                for t in &ts {
                    let (n, how) = cut_point(self, pt(*t), &[a, b], &[e]);
                    out.free_ends += usize::from(how == CutEnd::Free);
                    ends.push(n);
                }
                ends.push(b);
                if let Some(Entity::Line { b: eb, .. }) =
                    self.entities.iter_mut().find(|x| x.id() == e)
                {
                    *eb = ends[1];
                }
                for w in ends[1..].windows(2) {
                    let l = self.add_line(w[0], w[1]);
                    if self.is_construction(e) {
                        self.construction.insert(l);
                    }
                    out.created.push(l);
                    // Collinear with the first piece: its far end on the line.
                    self.constraints.push(Constraint::PointOnLine {
                        point: w[1],
                        line: e,
                    });
                }
                out.dropped += retarget_tangents(self, e, &out.created);
                drop_implied_on_curve(self);
                Ok(out)
            }
            Some(Entity::Circle { .. } | Entity::Arc { .. }) => {
                let (c, r, start, sweep, center_pt, is_circle) =
                    self.round_frame(e).ok_or(SketchError::MissingEntity(e))?;
                let (cuts, _) = self.round_cuts(e).ok_or(SketchError::MissingEntity(e))?;
                let rel = |a: f64| (a - start).rem_euclid(TAU);
                let tc = rel((at[1] - c[1]).atan2(at[0] - c[0]));
                let on = |t: f64| [c[0] + r * (start + t).cos(), c[1] + r * (start + t).sin()];
                let mut out = CurveEdit::default();
                if is_circle {
                    if cuts.len() < 2 {
                        return Err(refused(
                            "A circle needs two crossings to break (it becomes two arcs).",
                        ));
                    }
                    let hi = cuts
                        .iter()
                        .copied()
                        .find(|t| *t > tc)
                        .unwrap_or(cuts[0] + TAU);
                    let lo = cuts
                        .iter()
                        .copied()
                        .rev()
                        .find(|t| *t < tc)
                        .unwrap_or(cuts[cuts.len() - 1] - TAU);
                    let (s_id, h0) = cut_point(self, on(hi), &[], &[e]);
                    let (e_id, h1) = cut_point(self, on(lo), &[], &[e]);
                    out.free_ends +=
                        usize::from(h0 == CutEnd::Free) + usize::from(h1 == CutEnd::Free);
                    // The circle keeps its id as the rest (hi -> lo); the
                    // clicked span (lo -> hi) is the new arc.
                    out.dropped += self.circle_to_arc(e, s_id, e_id);
                    let id = self.push_arc_piece(e, center_pt, e_id, s_id);
                    out.created.push(id);
                    drop_implied_on_curve(self);
                    return Ok(out);
                }
                let lo = cuts.iter().copied().filter(|t| *t < tc).fold(0.0, f64::max);
                let hi = cuts
                    .iter()
                    .copied()
                    .filter(|t| *t > tc)
                    .fold(sweep, f64::min);
                if lo <= 0.0 && hi >= sweep {
                    return Err(nothing());
                }
                let Some(Entity::Arc {
                    start: sp, end: ep, ..
                }) = self.entity(e).cloned()
                else {
                    return Err(SketchError::MissingEntity(e));
                };
                let mut ends = vec![sp];
                for t in [lo, hi].into_iter().filter(|t| *t > 0.0 && *t < sweep) {
                    let (n, how) = cut_point(self, on(t), &[sp, ep], &[e]);
                    out.free_ends += usize::from(how == CutEnd::Free);
                    ends.push(n);
                }
                ends.push(ep);
                if let Some(Entity::Arc { end, .. }) =
                    self.entities.iter_mut().find(|x| x.id() == e)
                {
                    *end = ends[1];
                }
                for w in ends[1..].windows(2) {
                    let id = self.push_arc_piece(e, center_pt, w[0], w[1]);
                    out.created.push(id);
                }
                out.dropped += retarget_tangents(self, e, &out.created);
                drop_implied_on_curve(self);
                Ok(out)
            }
            Some(_) => Err(refused("Break works on lines, arcs and circles.")),
        }
    }
}
