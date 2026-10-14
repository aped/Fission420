//! Offset of a closed loop (lines, arcs, or a circle) by a signed distance,
//! kept by one named, editable dimension (`Constraint::Offset`).
//!
//! Geometry (exact, sketch plane): lines move parallel; arcs stay concentric
//! with the source arc (the copy shares the source centre point) and change
//! radius by the distance; a circle becomes a concentric circle. Joints:
//! * **tangent** joints (G1 in the source, e.g. a rounded corner) offset along
//!   the common normal, so the copy stays tangent;
//! * **sharp** joints become the intersection of the two offset curves:
//!   convex corners of an outward offset are **extended to a sharp (mitred)
//!   corner** (no rounding is added), concave corners are trimmed to the
//!   intersection.
//!
//! Refusals (nothing is added): a distance of 0 or non-finite; an arc whose
//! radius would reach zero; an edge or arc that would vanish or reverse; a
//! sharp joint whose offset curves no longer meet; a copy that self-intersects
//! or touches another loop of the sketch. A collapsed or self-intersecting copy
//! is never emitted.
//!
//! Solver rows (see `solver.rs`, `Constraint::Offset`): every copy vertex gets
//! exactly two equations — tangent joint: `p' = p + s·d·n` (n the source
//! line's left normal, or the source arc's radial direction); sharp joint: one
//! row from each incident curve (signed distance from the source line = `s·d`,
//! or distance from the arc centre = source radius + `s·d`). The copy arcs'
//! implied equal-radius rows are dropped (implied by the vertex rows), so the
//! copy is exactly determined by the source and `d`: no added DOF and no
//! redundancy.

use crate::{
    Constraint, Dimension, Entity, EntityId, PointId, Sketch, SketchError, solve_tolerance,
};
use serde::{Deserialize, Serialize};
use std::f64::consts::TAU;

/// One source/copy pair of an [`Constraint::Offset`], in loop order.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OffsetPair {
    pub source: EntityId,
    pub copy: EntityId,
    /// Line: +1 when the copy lies left of the source line's direction (a → b),
    /// −1 right. Arc/circle: copy radius = source radius + `sign · d`.
    pub sign: i8,
    /// The joint from this pair to the next one (cyclically) is tangent (G1).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tangent_next: bool,
}

/// Result of [`Sketch::offset_loop`].
#[derive(Clone, Debug, PartialEq)]
pub struct OffsetResult {
    /// Copy entities, in loop order.
    pub entities: Vec<EntityId>,
    /// Name of the distance dimension (editable, bindable to a parameter).
    pub dimension: String,
}

#[derive(Clone, Copy, Debug)]
enum Seg {
    Line {
        e: EntityId,
        /// Traversal start/end point ids.
        s: PointId,
        t: PointId,
    },
    Arc {
        e: EntityId,
        s: PointId,
        t: PointId,
        c: PointId,
        /// Traversed counter-clockwise (the arc entity is CCW from its start to
        /// its end; traversal is reversed when `s` is the entity's end).
        ccw: bool,
    },
}

impl Seg {
    fn ends(&self) -> (PointId, PointId) {
        match *self {
            Seg::Line { s, t, .. } | Seg::Arc { s, t, .. } => (s, t),
        }
    }
    fn entity(&self) -> EntityId {
        match *self {
            Seg::Line { e, .. } | Seg::Arc { e, .. } => e,
        }
    }
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}
fn scale(a: [f64; 2], k: f64) -> [f64; 2] {
    [a[0] * k, a[1] * k]
}
fn norm(a: [f64; 2]) -> f64 {
    a[0].hypot(a[1])
}
fn unit(a: [f64; 2]) -> [f64; 2] {
    let l = norm(a);
    [a[0] / l, a[1] / l]
}
fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    norm(sub(a, b))
}

/// The closed loop through `entity`: segments in traversal order,
/// counter-clockwise overall. Joints must share point ids.
fn walk(s: &Sketch, entity: EntityId) -> Result<Vec<Seg>, SketchError> {
    let bad = |m: &str| SketchError::UnsupportedProfile(format!("offset: {m}"));
    let live = |e: &Entity| !s.is_construction(e.id());
    let ends = |e: &Entity| match e {
        Entity::Line { a, b, .. } => Some((*a, *b)),
        Entity::Arc { start, end, .. } => Some((*start, *end)),
        _ => None,
    };
    let first = s.entity(entity).ok_or(SketchError::MissingEntity(entity))?;
    if s.is_construction(entity) {
        return Err(bad("construction geometry cannot be offset"));
    }
    if let Some(e) = s.entities.iter().find(|e| {
        live(e)
            && matches!(e, Entity::Ellipse { .. } | Entity::Spline { .. })
            && e.points().iter().any(|p| first.points().contains(p))
    }) {
        return Err(bad(&format!(
            "ellipses and splines cannot be offset ({:?})",
            e.id()
        )));
    }
    let (p0, _) = ends(first).ok_or_else(|| bad("circles are offset as a whole"))?;
    let mut segs = Vec::new();
    let mut cur = first.clone();
    let mut at = p0; // traversal start of `cur`
    loop {
        let (a, b) = ends(&cur).expect("line or arc");
        let (sp, tp) = if at == a { (a, b) } else { (b, a) };
        segs.push(match cur {
            Entity::Line { id, .. } => Seg::Line {
                e: id,
                s: sp,
                t: tp,
            },
            Entity::Arc {
                id, center, start, ..
            } => Seg::Arc {
                e: id,
                s: sp,
                t: tp,
                c: center,
                ccw: sp == start,
            },
            _ => unreachable!("ends() is Some for lines and arcs only"),
        });
        let next: Vec<&Entity> = s
            .entities
            .iter()
            .filter(|x| live(x) && x.id() != cur.id())
            .filter(|x| ends(x).is_some_and(|(u, v)| u == tp || v == tp))
            .collect();
        if next.len() != 1 {
            return Err(bad(&format!(
                "the loop through entity {} is not closed through shared points \
                 (a vertex joins {} other curves)",
                entity.0,
                next.len()
            )));
        }
        if next[0].id() == entity {
            break;
        }
        if segs.len() > s.entities.len() {
            return Err(bad("the loop does not close"));
        }
        cur = next[0].clone();
        at = tp;
    }
    if segs.len() < 2 {
        return Err(bad("a closed loop needs at least two curves"));
    }
    // Orientation: signed area of the chords plus arc segments.
    let xy = |p: PointId| s.point_xy(p).unwrap_or([0.0, 0.0]);
    let mut area = 0.0;
    for g in &segs {
        let (a, b) = g.ends();
        let (pa, pb) = (xy(a), xy(b));
        area += 0.5 * cross(pa, pb);
        if let Seg::Arc { c, ccw, .. } = *g {
            let pc = xy(c);
            let r = dist(pa, pc);
            let a0 = (pa[1] - pc[1]).atan2(pa[0] - pc[0]);
            let a1 = (pb[1] - pc[1]).atan2(pb[0] - pc[0]);
            let sw = if ccw {
                (a1 - a0).rem_euclid(TAU)
            } else {
                -(a0 - a1).rem_euclid(TAU)
            };
            area += 0.5 * r * r * (sw - sw.sin());
        }
    }
    if area < 0.0 {
        segs.reverse();
        for g in &mut segs {
            *g = match *g {
                Seg::Line { e, s, t } => Seg::Line { e, s: t, t: s },
                Seg::Arc { e, s, t, c, ccw } => Seg::Arc {
                    e,
                    s: t,
                    t: s,
                    c,
                    ccw: !ccw,
                },
            };
        }
    }
    Ok(segs)
}

/// Unit travel tangent of `g` at its start (`at_end == false`) or end.
fn tangent(s: &Sketch, g: &Seg, at_end: bool) -> [f64; 2] {
    let xy = |p: PointId| s.point_xy(p).unwrap_or([0.0, 0.0]);
    match *g {
        Seg::Line { s: a, t: b, .. } => unit(sub(xy(b), xy(a))),
        Seg::Arc {
            s: a, t: b, c, ccw, ..
        } => {
            let q = xy(if at_end { b } else { a });
            let r = sub(q, xy(c));
            let t = unit([-r[1], r[0]]);
            if ccw { t } else { scale(t, -1.0) }
        }
    }
}

/// Outward normal (right of travel for a CCW loop).
fn out_normal(t: [f64; 2]) -> [f64; 2] {
    [t[1], -t[0]]
}

/// Offset curve of a segment: a line through a point with direction, or a
/// circle (centre, radius).
#[derive(Clone, Copy)]
enum Curve {
    Line { p: [f64; 2], u: [f64; 2] },
    Circle { c: [f64; 2], r: f64 },
}

fn intersections(x: Curve, y: Curve) -> Vec<[f64; 2]> {
    match (x, y) {
        (Curve::Line { p, u }, Curve::Line { p: q, u: v }) => {
            let den = cross(u, v);
            if den.abs() < 1e-12 {
                return vec![];
            }
            let t = cross(sub(q, p), v) / den;
            vec![add(p, scale(u, t))]
        }
        (Curve::Line { p, u }, Curve::Circle { c, r })
        | (Curve::Circle { c, r }, Curve::Line { p, u }) => {
            let f = sub(p, c);
            let b = dot(f, u);
            let disc = b * b - (dot(f, f) - r * r);
            if disc < 0.0 {
                return vec![];
            }
            let s = disc.sqrt();
            vec![add(p, scale(u, -b - s)), add(p, scale(u, -b + s))]
        }
        (Curve::Circle { c, r }, Curve::Circle { c: c2, r: r2 }) => {
            let d = dist(c, c2);
            if d < 1e-12 || d > r + r2 || d < (r - r2).abs() {
                return vec![];
            }
            let a = (r * r - r2 * r2 + d * d) / (2.0 * d);
            let h = (r * r - a * a).max(0.0).sqrt();
            let u = unit(sub(c2, c));
            let m = add(c, scale(u, a));
            vec![add(m, [-h * u[1], h * u[0]]), add(m, [h * u[1], -h * u[0]])]
        }
    }
}

impl Sketch {
    /// Offset the closed loop through `entity` (a line, arc or circle of the
    /// loop) by `distance` mm: positive = outward (away from the enclosed area),
    /// negative = inward. Adds the copy (new points, lines and arcs; arcs share
    /// the source centre points) and one `Constraint::Offset` whose dimension
    /// `name` (default `offset`, made unique) holds `|distance|`; editing it, or
    /// the source, moves the copy. See the module docs for joints and refusals.
    /// On error the sketch is unchanged.
    pub fn offset_loop(
        &mut self,
        entity: EntityId,
        distance: f64,
        name: Option<&str>,
    ) -> Result<OffsetResult, SketchError> {
        if !distance.is_finite() || distance == 0.0 {
            return Err(SketchError::InvalidValue(format!(
                "offset distance must be a non-zero finite number, got {distance}"
            )));
        }
        let mut work = self.clone();
        let tol = solve_tolerance(work.scale().max(distance.abs()));
        let base = name.unwrap_or("offset").trim();
        let base = if base.is_empty() { "offset" } else { base };
        let taken = |n: &str| {
            self.constraints
                .iter()
                .any(|c| c.dimension().is_some_and(|d| d.name == n))
        };
        let dim_name = if !taken(base) {
            base.to_string()
        } else {
            (2..)
                .map(|i| format!("{base}{i}"))
                .find(|n| !taken(n))
                .expect("a free name")
        };
        let v = distance.abs();
        let pair = match work.entity(entity).cloned() {
            Some(Entity::Circle { id, center, radius }) => {
                if work.is_construction(id) {
                    return Err(SketchError::UnsupportedProfile(
                        "offset: construction geometry cannot be offset".into(),
                    ));
                }
                let r = radius + distance;
                if r <= tol {
                    return Err(SketchError::UnsupportedProfile(format!(
                        "offset: the circle (radius {radius} mm) collapses at {distance} mm"
                    )));
                }
                let copy = work.add_circle(center, r);
                vec![OffsetPair {
                    source: id,
                    copy,
                    sign: if distance > 0.0 { 1 } else { -1 },
                    tangent_next: false,
                }]
            }
            Some(_) => offset_segments(&mut work, entity, distance, tol)?,
            None => return Err(SketchError::MissingEntity(entity)),
        };
        let copies: Vec<EntityId> = pair.iter().map(|p| p.copy).collect();
        work.constraints.push(Constraint::Offset {
            pairs: pair,
            dim: Dimension::new(dim_name.clone(), v),
        });
        work.validate()?;
        // The copy must be a simple loop disjoint from every other loop.
        work.regions().map_err(|e| {
            SketchError::UnsupportedProfile(format!(
                "offset by {distance} mm gives a loop that self-intersects or touches \
                 another loop ({e})"
            ))
        })?;
        let was_redundant = self.clone().solve().status == crate::SolveStatus::OverConstrained;
        let before = work.clone();
        let rep = work.solve();
        if !rep.is_success()
            || (rep.status == crate::SolveStatus::OverConstrained && !was_redundant)
        {
            return Err(SketchError::UnsupportedProfile(format!(
                "offset: the sketch does not solve with the offset ({:?})",
                rep.status
            )));
        }
        // The rows must hold the constructed geometry (no jump).
        for (p, q) in before.points.iter().zip(&work.points) {
            if dist([p.x, p.y], [q.x, q.y]) > 1e3 * tol {
                return Err(SketchError::UnsupportedProfile(
                    "offset: the solved copy differs from the constructed one".into(),
                ));
            }
        }
        *self = work;
        Ok(OffsetResult {
            entities: copies,
            dimension: dim_name,
        })
    }
}

/// Copy of a line/arc loop; returns the pairs (in loop order).
fn offset_segments(
    s: &mut Sketch,
    entity: EntityId,
    d: f64,
    tol: f64,
) -> Result<Vec<OffsetPair>, SketchError> {
    let refuse = |m: String| SketchError::UnsupportedProfile(format!("offset by {d} mm: {m}"));
    let segs = walk(s, entity)?;
    let n = segs.len();
    let xy = |s: &Sketch, p: PointId| s.point_xy(p).ok_or(SketchError::MissingPoint(p));
    // Offset curve of each segment.
    let mut curves = Vec::with_capacity(n);
    for g in &segs {
        curves.push(match *g {
            Seg::Line { s: a, t: b, .. } => {
                let (pa, pb) = (xy(s, a)?, xy(s, b)?);
                let u = unit(sub(pb, pa));
                Curve::Line {
                    p: add(pa, scale(out_normal(u), d)),
                    u,
                }
            }
            Seg::Arc {
                s: a, c, ccw, e, ..
            } => {
                let (pa, pc) = (xy(s, a)?, xy(s, c)?);
                let r = dist(pa, pc);
                let r2 = if ccw { r + d } else { r - d };
                if r2 <= tol {
                    return Err(refuse(format!(
                        "arc {} (radius {r:.4} mm) would collapse",
                        e.0
                    )));
                }
                Curve::Circle { c: pc, r: r2 }
            }
        });
    }
    // Copy vertices: vertex i is the start of segment i (end of i - 1).
    let mut verts = Vec::with_capacity(n);
    let mut tangent_next = vec![false; n];
    for i in 0..n {
        let prev = (i + n - 1) % n;
        let v = xy(s, segs[i].ends().0)?;
        let t_in = tangent(s, &segs[prev], true);
        let t_out = tangent(s, &segs[i], false);
        let naive = add(v, scale(out_normal(t_in), d));
        if cross(t_in, t_out).abs() < 1e-9 && dot(t_in, t_out) > 0.0 {
            tangent_next[prev] = true;
            verts.push(naive);
            continue;
        }
        let naive2 = add(v, scale(out_normal(t_out), d));
        let aim = scale(add(naive, naive2), 0.5);
        let x = intersections(curves[prev], curves[i])
            .into_iter()
            .min_by(|a, b| dist(*a, aim).total_cmp(&dist(*b, aim)))
            .ok_or_else(|| {
                refuse(format!(
                    "the curves at the joint of entities {} and {} no longer meet",
                    segs[prev].entity().0,
                    segs[i].entity().0
                ))
            })?;
        verts.push(x);
    }
    // Each copy segment keeps its direction and does not vanish.
    for i in 0..n {
        let (p, q) = (verts[i], verts[(i + 1) % n]);
        match segs[i] {
            Seg::Line { s: a, t: b, e } => {
                let dir = sub(xy(s, b)?, xy(s, a)?);
                if dist(p, q) <= tol || dot(sub(q, p), dir) <= 0.0 {
                    return Err(refuse(format!(
                        "edge {} would vanish or reverse (distance too large for this feature)",
                        e.0
                    )));
                }
            }
            Seg::Arc {
                s: a,
                t: b,
                c,
                ccw,
                e,
            } => {
                let pc = xy(s, c)?;
                let ang = |x: [f64; 2]| (x[1] - pc[1]).atan2(x[0] - pc[0]);
                let sweep = |u: [f64; 2], w: [f64; 2]| {
                    if ccw {
                        (ang(w) - ang(u)).rem_euclid(TAU)
                    } else {
                        (ang(u) - ang(w)).rem_euclid(TAU)
                    }
                };
                let old = sweep(xy(s, a)?, xy(s, b)?);
                let new = sweep(p, q);
                if dist(p, q) <= tol || new <= 1e-9 || (new - old).abs() >= std::f64::consts::PI {
                    return Err(refuse(format!(
                        "arc {} would vanish or reverse (distance too large for this feature)",
                        e.0
                    )));
                }
            }
        }
    }
    // Add the copy: one new point per vertex, entities in the source
    // entities' own orientation.
    let ids: Vec<PointId> = verts.iter().map(|v| s.add_point(v[0], v[1])).collect();
    let mut pairs = Vec::with_capacity(n);
    for i in 0..n {
        let (vs, vt) = (ids[i], ids[(i + 1) % n]);
        let pair = match segs[i] {
            Seg::Line { e, s: a, .. } => {
                let Some(Entity::Line { a: ea, .. }) = s.entity(e).cloned() else {
                    unreachable!()
                };
                let forward = ea == a;
                let (na, nb) = if forward { (vs, vt) } else { (vt, vs) };
                let copy = s.add_line(na, nb);
                // Copy at d along the outward normal = right of travel; left of
                // the entity direction when the entity runs against travel.
                let left = if forward { -d } else { d };
                OffsetPair {
                    source: e,
                    copy,
                    sign: if left > 0.0 { 1 } else { -1 },
                    tangent_next: tangent_next[i],
                }
            }
            Seg::Arc { e, c, ccw, .. } => {
                let (start, end) = if ccw { (vs, vt) } else { (vt, vs) };
                let copy = s.next_entity_id();
                s.entities.push(Entity::Arc {
                    id: copy,
                    center: c,
                    start,
                    end,
                });
                let dr = if ccw { d } else { -d };
                OffsetPair {
                    source: e,
                    copy,
                    sign: if dr > 0.0 { 1 } else { -1 },
                    tangent_next: tangent_next[i],
                }
            }
        };
        pairs.push(pair);
    }
    Ok(pairs)
}

/// Shared endpoint of two line/arc entities (exactly one).
pub(crate) fn shared_point(s: &Sketch, a: EntityId, b: EntityId) -> Option<PointId> {
    let ends = |id: EntityId| match s.entity(id)? {
        Entity::Line { a, b, .. } => Some([*a, *b]),
        Entity::Arc { start, end, .. } => Some([*start, *end]),
        _ => None,
    };
    let (x, y) = (ends(a)?, ends(b)?);
    let shared: Vec<PointId> = x.iter().copied().filter(|p| y.contains(p)).collect();
    (shared.len() == 1).then(|| shared[0])
}

/// Offset a closed profile (polygon, line/arc loop or circle) by `distance` mm
/// (+ outward, − inward) with the same geometry and refusals as
/// [`Sketch::offset_loop`]; the result is a new counter-clockwise profile. Used
/// by features (e.g. a scoped shell) that need the offset geometry only.
pub fn offset_profile(
    profile: &crate::Profile,
    distance: f64,
) -> Result<crate::Profile, SketchError> {
    use crate::{LoopSegment, Profile};
    let mut s = Sketch::default();
    let first = match profile {
        Profile::Circle { center, radius } => {
            let c = s.add_point(center[0], center[1]);
            s.add_circle(c, *radius)
        }
        Profile::Polygon(v) => {
            let p: Vec<PointId> = v.iter().map(|q| s.add_point(q[0], q[1])).collect();
            let n = p.len();
            let ids: Vec<EntityId> = (0..n).map(|i| s.add_line(p[i], p[(i + 1) % n])).collect();
            ids[0]
        }
        Profile::Loop(segs) => {
            let p: Vec<PointId> = segs
                .iter()
                .map(|g| {
                    let q = g.start();
                    s.add_point(q[0], q[1])
                })
                .collect();
            let n = p.len();
            let mut ids = Vec::with_capacity(n);
            for (i, g) in segs.iter().enumerate() {
                let (a, b) = (p[i], p[(i + 1) % n]);
                ids.push(match g {
                    LoopSegment::Line { .. } => s.add_line(a, b),
                    LoopSegment::Arc { center, ccw, .. } => {
                        let c = s.add_point(center[0], center[1]);
                        let id = s.next_entity_id();
                        let (start, end) = if *ccw { (a, b) } else { (b, a) };
                        s.entities.push(Entity::Arc {
                            id,
                            center: c,
                            start,
                            end,
                        });
                        id
                    }
                    LoopSegment::Cubic { .. } => {
                        return Err(SketchError::UnsupportedProfile(
                            "offset: profiles with ellipse or spline spans cannot be offset".into(),
                        ));
                    }
                });
            }
            ids[0]
        }
    };
    let source: Vec<EntityId> = s.entities.iter().map(Entity::id).collect();
    s.offset_loop(first, distance, None)?;
    s.construction = source.into_iter().collect();
    s.profile()
}
