//! Three-point circular arcs: construction, geometry queries.
//!
//! An [`Entity::Arc`] is stored counter-clockwise from `start` to `end` about
//! `center`. [`Sketch::add_arc_3pt`] builds one through a third ("via") point:
//! the centre is the circumcentre of the three points, and if `via` lies on the
//! clockwise side of start→end the endpoints are stored swapped so that the
//! stored CCW arc still passes through `via`.
//!
//! # Tolerances
//! * Degeneracy: the three points are rejected when any two are within the
//!   solve tolerance `tol = solve_tolerance(scale)` of each other, or when the
//!   height of the triangle over its longest side is `<= tol` (collinear at the
//!   solver's resolution).
//! * Radius limit: `radius <= MAX_ARC_RADIUS_RATIO × longest side`. Flatter
//!   arcs are numerically near-lines (sagitta below ~1/80000 of the chord) and
//!   badly conditioned for the solver; draw a line instead.

use crate::{Entity, EntityId, PointId, Sketch, SketchError, solve_tolerance};
use std::f64::consts::TAU;

/// Largest accepted ratio of arc radius to the longest side of the triangle
/// (start, end, via). Dimensionless.
pub const MAX_ARC_RADIUS_RATIO: f64 = 1.0e4;

/// Derived geometry of an [`Entity::Arc`] (mm, radians).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArcGeometry {
    pub center: [f64; 2],
    /// Distance from the centre to the start point.
    pub radius: f64,
    /// Polar angle of the start point about the centre, in (-π, π].
    pub start_angle: f64,
    /// Counter-clockwise sweep from start to end, in (0, 2π]. Exactly 2π only
    /// when start and end lie in the same direction (degenerate; profiles
    /// reject it).
    pub sweep: f64,
}

/// Circle through three points, or why there is none, at tolerance `tol` (mm).
/// Returns `(center, radius, ccw_from_a_to_b_passes_via)`.
pub(crate) fn circle_3pt(
    a: [f64; 2],
    b: [f64; 2],
    via: [f64; 2],
    tol: f64,
) -> Result<([f64; 2], f64, bool), String> {
    if a.iter().chain(&b).chain(&via).any(|v| !v.is_finite()) {
        return Err("non-finite point".into());
    }
    let d = |p: [f64; 2], q: [f64; 2]| (q[0] - p[0]).hypot(q[1] - p[1]);
    let (dab, dav, dbv) = (d(a, b), d(a, via), d(b, via));
    if dab.min(dav).min(dbv) <= tol {
        return Err(format!(
            "two of the three points coincide (closest pair {:.3e} mm apart)",
            dab.min(dav).min(dbv)
        ));
    }
    let (bx, by) = (b[0] - a[0], b[1] - a[1]);
    let (cx, cy) = (via[0] - a[0], via[1] - a[1]);
    // Twice the signed triangle area; < 0 when via is right of a->b.
    let cross = bx * cy - by * cx;
    let longest = dab.max(dav).max(dbv);
    let height = cross.abs() / longest;
    if height <= tol {
        return Err(format!(
            "the three points are collinear (height {height:.3e} mm <= {tol:.3e} mm)"
        ));
    }
    let (b2, c2) = (bx * bx + by * by, cx * cx + cy * cy);
    let den = 2.0 * cross;
    let ux = (cy * b2 - by * c2) / den;
    let uy = (bx * c2 - cx * b2) / den;
    let radius = ux.hypot(uy);
    if !radius.is_finite() || radius > MAX_ARC_RADIUS_RATIO * longest {
        return Err(format!(
            "radius {radius:.6e} mm exceeds {MAX_ARC_RADIUS_RATIO:e} x the {longest:.6} mm span \
             (nearly straight; use a line)"
        ));
    }
    Ok(([a[0] + ux, a[1] + uy], radius, cross < 0.0))
}

/// Circle through `a`, `b` and `via`: `(center, radius, ccw)` where `ccw` is
/// true when the counter-clockwise arc from `a` to `b` passes through `via`.
/// `None` for (near-)coincident or collinear points or a radius beyond
/// [`MAX_ARC_RADIUS_RATIO`] (tolerance: `solve_tolerance` of the points' scale).
/// Intended for editor previews; [`Sketch::add_arc_3pt`] applies the same rules.
pub fn arc_through_3_points(
    a: [f64; 2],
    b: [f64; 2],
    via: [f64; 2],
) -> Option<([f64; 2], f64, bool)> {
    let scale = a
        .iter()
        .chain(&b)
        .chain(&via)
        .fold(1.0_f64, |m, v| m.max(v.abs()));
    circle_3pt(a, b, via, solve_tolerance(scale)).ok()
}

/// Counter-clockwise angle from direction `p - c` to direction `q - c`, in [0, 2π).
pub(crate) fn ccw_angle(c: [f64; 2], p: [f64; 2], q: [f64; 2]) -> f64 {
    let a0 = (p[1] - c[1]).atan2(p[0] - c[0]);
    let a1 = (q[1] - c[1]).atan2(q[0] - c[0]);
    let s = (a1 - a0).rem_euclid(TAU);
    // rem_euclid can round to exactly TAU for tiny negative inputs.
    if s >= TAU { 0.0 } else { s }
}

impl Sketch {
    /// Add a circular arc from existing point `start` to existing point `end`
    /// passing through `via` (sketch mm). A new centre point is created at the
    /// circumcentre; the stored arc is CCW (endpoints swapped when `via` lies on
    /// the clockwise side). No constraints are added. On error the sketch is
    /// unchanged.
    pub fn add_arc_3pt(
        &mut self,
        start: PointId,
        end: PointId,
        via: [f64; 2],
    ) -> Result<EntityId, SketchError> {
        let a = self
            .point_xy(start)
            .ok_or(SketchError::MissingPoint(start))?;
        let b = self.point_xy(end).ok_or(SketchError::MissingPoint(end))?;
        if start == end {
            return Err(SketchError::DegenerateArc(
                "start and end are the same point".into(),
            ));
        }
        let scale = via.iter().fold(self.scale(), |m, v| m.max(v.abs()));
        let (center, _, ccw) =
            circle_3pt(a, b, via, solve_tolerance(scale)).map_err(SketchError::DegenerateArc)?;
        let (start, end) = if ccw { (start, end) } else { (end, start) };
        let c = self.add_point(center[0], center[1]);
        let id = self.next_entity_id();
        self.entities.push(Entity::Arc {
            id,
            center: c,
            start,
            end,
        });
        Ok(id)
    }

    /// Geometry of arc `id` at the current coordinates; `None` if `id` is not an
    /// arc, a point is missing, or the start coincides with the centre.
    pub fn arc_geometry(&self, id: EntityId) -> Option<ArcGeometry> {
        let Some(Entity::Arc {
            center, start, end, ..
        }) = self.entity(id)
        else {
            return None;
        };
        let (c, p, q) = (
            self.point_xy(*center)?,
            self.point_xy(*start)?,
            self.point_xy(*end)?,
        );
        let radius = (p[0] - c[0]).hypot(p[1] - c[1]);
        if !radius.is_finite() || radius <= 0.0 {
            return None;
        }
        let sweep = match ccw_angle(c, p, q) {
            s if s > 0.0 => s,
            _ => TAU,
        };
        Some(ArcGeometry {
            center: c,
            radius,
            start_angle: (p[1] - c[1]).atan2(p[0] - c[0]),
            sweep,
        })
    }
}
