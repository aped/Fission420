//! Ellipses and cubic B-splines: exact Bézier decomposition, evaluation and
//! the bounded polyline approximation the profile checks use.
//!
//! # Exact representation
//! Profile loops carry these curves as [`LoopSegment::Cubic`] spans: rational
//! cubic Bézier curves with control points `start, c1, c2, end` and weights
//! `1, w[0], w[1], 1` (`end` is the next segment's start).
//!
//! * An ellipse with centre `C`, unit axis `u` (towards the axis end point),
//!   `v = u` rotated +90°, semi-axes `a` (along `u`) and `b` is
//!   `E(θ) = C + a cos θ u + b sin θ v`, traversed counter-clockwise. Each
//!   quarter `[jπ/2, (j+1)π/2]` is exactly the affine image of the circular
//!   quarter arc, a rational quadratic Bézier with weights `(1, √2/2, 1)`,
//!   elevated to degree 3 without changing the curve (Piegl & Tiller, *The
//!   NURBS Book*, §5.5).
//! * A uniform cubic B-spline (open: clamped knots `0,0,0,0,1,…,n−3 ×4`;
//!   closed: periodic, integer knots, the first three control points
//!   repeated at the end) is split into one polynomial Bézier span per knot
//!   interval through its blossom (polar form): span `[s, t]` has control
//!   points `f(s,s,s), f(s,s,t), f(s,t,t), f(t,t,t)` (de Boor's algorithm with
//!   a different parameter per stage). The decomposition is exact.
//!
//! # Polyline approximation (checks only)
//! Self-intersection, loop-contact and containment tests on these curves use a
//! polyline within a stated distance `δ` of the exact curve: a span is split by
//! de Casteljau subdivision (in homogeneous coordinates for rational spans)
//! until every control point lies within `δ` of the chord segment. With
//! positive weights the span lies in the convex hull of its control points,
//! and that hull lies within `δ` of the chord, so every point of the curve is
//! within `δ` of the polyline (and the polyline's vertices are on the curve).
//! `δ` = [`FLATTEN_REL`] × the loop's own size (at least [`FLATTEN_ABS`],
//! [`loop_flatten_tolerance`]); the checks widen their contact tolerance by `δ`, so they may refuse curves closer than
//! `δ` but never accept curves that touch.

use crate::profile::LoopSegment;
use crate::{Entity, EntityId, PointId, Sketch, SketchError};

/// Relative polyline tolerance of the curve checks (× sketch scale).
pub const FLATTEN_REL: f64 = 1e-5;
/// Absolute floor of the polyline tolerance, mm.
pub const FLATTEN_ABS: f64 = 1e-6;
/// Most control points a spline may have.
pub const MAX_SPLINE_POINTS: usize = 256;
/// Subdivision depth cap of [`flatten`] (2^16 pieces per span at most).
const MAX_FLATTEN_DEPTH: u32 = 16;

/// Polyline tolerance (mm) for curves of characteristic size `scale`.
pub fn flatten_tolerance(scale: f64) -> f64 {
    FLATTEN_ABS.max(FLATTEN_REL * scale)
}

/// Polyline tolerance of one loop: [`flatten_tolerance`] of the diagonal of
/// its control points' box (the loop's own size, not the sketch's: a small
/// thin ellipse far from the origin keeps a tight bound).
pub fn loop_flatten_tolerance(segs: &[LoopSegment]) -> f64 {
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for s in segs {
        let pts: Vec<[f64; 2]> = match *s {
            LoopSegment::Cubic { start, c1, c2, .. } => vec![start, c1, c2],
            ref other => vec![other.start()],
        };
        for p in pts {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    let d = (hi[0] - lo[0]).hypot(hi[1] - lo[1]);
    flatten_tolerance(if d.is_finite() { d } else { 0.0 })
}

/// Structural rules of a spline's control point list.
pub(crate) fn check_spline(
    id: EntityId,
    points: &[PointId],
    closed: bool,
) -> Result<(), SketchError> {
    let min = if closed { 3 } else { 4 };
    if points.len() < min || points.len() > MAX_SPLINE_POINTS {
        return Err(SketchError::InvalidValue(format!(
            "spline {id:?} needs {min} to {MAX_SPLINE_POINTS} control points ({} given; \
             {} spline)",
            points.len(),
            if closed { "closed" } else { "open" }
        )));
    }
    let mut seen = std::collections::HashSet::new();
    if let Some(p) = points.iter().find(|p| !seen.insert(**p)) {
        return Err(SketchError::InvalidValue(format!(
            "spline {id:?} uses control point {p:?} twice"
        )));
    }
    Ok(())
}

fn lerp(a: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// Blossom `f(t1, t2, t3)` of a degree-3 B-spline on knot span `k`
/// (`knots[k] <= knots[k+1]`, control points `ctrl[k-3..=k]`).
fn blossom(ctrl: &[[f64; 2]], knots: &[f64], k: usize, t: [f64; 3]) -> [f64; 2] {
    let mut d = [ctrl[k - 3], ctrl[k - 2], ctrl[k - 1], ctrl[k]];
    for (r, &tr) in (1..=3).zip(t.iter()) {
        for j in (r..=3).rev() {
            let i = k - 3 + j; // global index of d[j]
            let (lo, hi) = (knots[i], knots[i + 4 - r]);
            let a = if hi > lo { (tr - lo) / (hi - lo) } else { 0.0 };
            d[j] = lerp(d[j - 1], d[j], a);
        }
    }
    d[3]
}

/// Exact Bézier spans `[P0, P1, P2, P3]` of a uniform cubic B-spline (see the
/// module docs). Open: `ctrl.len() >= 4`, `n − 3` spans from `ctrl[0]` to the
/// last control point. Closed: `ctrl.len() >= 3`, `n` spans.
pub fn spline_beziers(ctrl: &[[f64; 2]], closed: bool) -> Vec<[[f64; 2]; 4]> {
    let n = ctrl.len();
    let (pts, knots): (Vec<[f64; 2]>, Vec<f64>) = if closed {
        if n < 3 {
            return vec![];
        }
        let pts: Vec<_> = (0..n + 3).map(|i| ctrl[i % n]).collect();
        (pts, (0..n + 7).map(|i| i as f64).collect())
    } else {
        if n < 4 {
            return vec![];
        }
        let last = (n - 3) as f64;
        let knots = (0..n + 4)
            .map(|i| (i as f64 - 3.0).clamp(0.0, last))
            .collect();
        (ctrl.to_vec(), knots)
    };
    let spans = if closed { 3..n + 3 } else { 3..n };
    spans
        .map(|k| {
            let (s, t) = (knots[k], knots[k + 1]);
            [
                blossom(&pts, &knots, k, [s, s, s]),
                blossom(&pts, &knots, k, [s, s, t]),
                blossom(&pts, &knots, k, [s, t, t]),
                blossom(&pts, &knots, k, [t, t, t]),
            ]
        })
        .collect()
}

/// Exact counter-clockwise rational cubic spans of an ellipse, starting at the
/// axis end `major` (see the module docs): `(P0, P1, P2, weights of P1, P2)`.
/// The four spans join end to end; the last ends at the first's start.
pub fn ellipse_beziers(center: [f64; 2], major: [f64; 2], minor: f64) -> Vec<[[f64; 2]; 3]> {
    ellipse_spans(center, major, minor)
        .into_iter()
        .map(|(p, _)| p)
        .collect()
}

/// Ellipse spans with their weights: `([P0, P1, P2], [w1, w2])`.
pub(crate) fn ellipse_spans(
    center: [f64; 2],
    major: [f64; 2],
    minor: f64,
) -> Vec<([[f64; 2]; 3], [f64; 2])> {
    let (dx, dy) = (major[0] - center[0], major[1] - center[1]);
    let a = dx.hypot(dy);
    if a <= 0.0 || !a.is_finite() {
        return vec![];
    }
    let u = [dx / a, dy / a];
    let v = [-u[1], u[0]];
    let at = |cu: f64, cv: f64| {
        [
            center[0] + a * cu * u[0] + minor * cv * v[0],
            center[1] + a * cu * u[1] + minor * cv * v[1],
        ]
    };
    // Quarter points at θ = 0, π/2, π, 3π/2 (exact unit cosines and sines).
    let q = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)];
    let w = std::f64::consts::FRAC_1_SQRT_2;
    let we = (1.0 + 2.0 * w) / 3.0;
    (0..4)
        .map(|j| {
            let (c0, s0) = q[j];
            let (c1, s1) = q[(j + 1) % 4];
            let p0 = if j == 0 { major } else { at(c0, s0) };
            let p2 = if j == 3 { major } else { at(c1, s1) };
            let m = at(c0 + c1, s0 + s1);
            // Degree elevation of (p0, m, p2; 1, w, 1).
            let e1 = [
                (p0[0] + 2.0 * w * m[0]) / (1.0 + 2.0 * w),
                (p0[1] + 2.0 * w * m[1]) / (1.0 + 2.0 * w),
            ];
            let e2 = [
                (2.0 * w * m[0] + p2[0]) / (1.0 + 2.0 * w),
                (2.0 * w * m[1] + p2[1]) / (1.0 + 2.0 * w),
            ];
            ([p0, e1, e2], [we, we])
        })
        .collect()
}

/// Point of the rational cubic `p0, c1, c2, p3` (weights `1, w0, w1, 1`) at
/// `t ∈ [0, 1]`.
pub fn cubic_point(
    p0: [f64; 2],
    c1: [f64; 2],
    c2: [f64; 2],
    p3: [f64; 2],
    w: [f64; 2],
    t: f64,
) -> [f64; 2] {
    let s = 1.0 - t;
    let b = [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t];
    let ww = [1.0, w[0], w[1], 1.0];
    let pts = [p0, c1, c2, p3];
    let mut num = [0.0; 2];
    let mut den = 0.0;
    for i in 0..4 {
        let k = b[i] * ww[i];
        num[0] += k * pts[i][0];
        num[1] += k * pts[i][1];
        den += k;
    }
    [num[0] / den, num[1] / den]
}

/// Derivative of [`cubic_point`] with respect to `t`.
fn cubic_deriv(
    p0: [f64; 2],
    c1: [f64; 2],
    c2: [f64; 2],
    p3: [f64; 2],
    w: [f64; 2],
    t: f64,
) -> [f64; 2] {
    let s = 1.0 - t;
    let b = [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t];
    let db = [
        -3.0 * s * s,
        3.0 * s * s - 6.0 * s * t,
        6.0 * s * t - 3.0 * t * t,
        3.0 * t * t,
    ];
    let ww = [1.0, w[0], w[1], 1.0];
    let pts = [p0, c1, c2, p3];
    let (mut n, mut dn, mut d, mut dd) = ([0.0; 2], [0.0; 2], 0.0, 0.0);
    for i in 0..4 {
        for k in 0..2 {
            n[k] += b[i] * ww[i] * pts[i][k];
            dn[k] += db[i] * ww[i] * pts[i][k];
        }
        d += b[i] * ww[i];
        dd += db[i] * ww[i];
    }
    [
        (dn[0] * d - n[0] * dd) / (d * d),
        (dn[1] * d - n[1] * dd) / (d * d),
    ]
}

/// 16-point Gauss–Legendre nodes and weights on [−1, 1] (positive half).
const GL16: [(f64, f64); 8] = [
    (0.095_012_509_837_637_44, 0.189_450_610_455_068_5),
    (0.281_603_550_779_258_9, 0.182_603_415_044_923_6),
    (0.458_016_777_657_227_4, 0.169_156_519_395_002_5),
    (0.617_876_244_402_643_7, 0.149_595_988_816_576_7),
    (0.755_404_408_355_003, 0.124_628_971_255_533_9),
    (0.865_631_202_387_831_7, 0.095_158_511_682_492_78),
    (0.944_575_023_073_232_6, 0.062_253_523_938_647_89),
    (0.989_400_934_991_649_9, 0.027_152_459_411_754_09),
];

/// `∫ (x dy − y dx) / 2` along the rational cubic span (signed area swept
/// relative to the origin). Composite 16-point Gauss–Legendre over 8 equal
/// pieces: for the polynomial spans of a B-spline the integrand is a
/// polynomial of degree 5 and the rule is exact; for the ellipse spans the
/// integrand is smooth and rational with weights in (0.8, 1], and the rule's
/// error is far below the solve tolerance.
pub fn cubic_area_term(p0: [f64; 2], c1: [f64; 2], c2: [f64; 2], p3: [f64; 2], w: [f64; 2]) -> f64 {
    const PIECES: usize = 8;
    let mut sum = 0.0;
    for k in 0..PIECES {
        let (a, b) = (k as f64 / PIECES as f64, (k + 1) as f64 / PIECES as f64);
        let (m, h) = ((a + b) / 2.0, (b - a) / 2.0);
        for &(x, wt) in &GL16 {
            for t in [m - h * x, m + h * x] {
                let p = cubic_point(p0, c1, c2, p3, w, t);
                let d = cubic_deriv(p0, c1, c2, p3, w, t);
                sum += wt * h * 0.5 * (p[0] * d[1] - p[1] * d[0]);
            }
        }
    }
    sum
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
}

/// Polyline through points of the rational cubic span, from `p0` to `p3`
/// inclusive, within `delta` of the exact curve (see the module docs).
pub fn flatten(
    p0: [f64; 2],
    c1: [f64; 2],
    c2: [f64; 2],
    p3: [f64; 2],
    w: [f64; 2],
    delta: f64,
) -> Vec<[f64; 2]> {
    // Homogeneous control points (x w, y w, w).
    let h = |p: [f64; 2], w: f64| [p[0] * w, p[1] * w, w];
    let ctrl = [h(p0, 1.0), h(c1, w[0]), h(c2, w[1]), h(p3, 1.0)];
    let mut out = vec![p0];
    flatten_rec(ctrl, delta, 0, &mut out);
    if let Some(last) = out.last_mut() {
        *last = p3;
    }
    out
}

fn flatten_rec(c: [[f64; 3]; 4], delta: f64, depth: u32, out: &mut Vec<[f64; 2]>) {
    let pt = |h: [f64; 3]| [h[0] / h[2], h[1] / h[2]];
    let (a, b) = (pt(c[0]), pt(c[3]));
    let flat = seg_dist(pt(c[1]), a, b) <= delta && seg_dist(pt(c[2]), a, b) <= delta;
    if flat || depth >= MAX_FLATTEN_DEPTH {
        out.push(b);
        return;
    }
    let mid = |x: [f64; 3], y: [f64; 3]| {
        [
            (x[0] + y[0]) / 2.0,
            (x[1] + y[1]) / 2.0,
            (x[2] + y[2]) / 2.0,
        ]
    };
    let (p01, p12, p23) = (mid(c[0], c[1]), mid(c[1], c[2]), mid(c[2], c[3]));
    let (p012, p123) = (mid(p01, p12), mid(p12, p23));
    let m = mid(p012, p123);
    flatten_rec([c[0], p01, p012, m], delta, depth + 1, out);
    flatten_rec([m, p123, p23, c[3]], delta, depth + 1, out);
}

/// Profile spans of a closed curve entity (ellipse or closed spline), in the
/// curve's own direction, starting at its first joint.
pub(crate) fn closed_segments(s: &Sketch, e: &Entity) -> Vec<LoopSegment> {
    let xy = |id: &PointId| s.point_xy(*id).unwrap_or([f64::NAN; 2]);
    match e {
        Entity::Ellipse {
            center,
            major,
            minor,
            ..
        } => ellipse_spans(xy(center), xy(major), *minor)
            .into_iter()
            .map(|([p0, c1, c2], w)| LoopSegment::Cubic {
                start: p0,
                c1,
                c2,
                w,
            })
            .collect(),
        Entity::Spline {
            points,
            closed: true,
            ..
        } => {
            let ctrl: Vec<[f64; 2]> = points.iter().map(xy).collect();
            spline_beziers(&ctrl, true)
                .into_iter()
                .map(|[p0, c1, c2, _]| LoopSegment::Cubic {
                    start: p0,
                    c1,
                    c2,
                    w: [1.0, 1.0],
                })
                .collect()
        }
        _ => vec![],
    }
}

/// Bézier spans of an open spline from its first to its last control point.
pub(crate) fn open_spline_spans(s: &Sketch, points: &[PointId]) -> Vec<[[f64; 2]; 4]> {
    let ctrl: Vec<[f64; 2]> = points
        .iter()
        .map(|id| s.point_xy(*id).unwrap_or([f64::NAN; 2]))
        .collect();
    spline_beziers(&ctrl, false)
}

impl Sketch {
    /// Add a full ellipse: centre point, axis end point (new points at the
    /// given coordinates) and the other semi-axis `minor` (mm, > 0). Returns
    /// the entity and its two points. Does not solve.
    pub fn add_ellipse(
        &mut self,
        center: [f64; 2],
        major: [f64; 2],
        minor: f64,
    ) -> Result<(EntityId, PointId, PointId), SketchError> {
        let a = (major[0] - center[0]).hypot(major[1] - center[1]);
        if !(minor.is_finite() && minor > 0.0 && a.is_finite() && a > 0.0)
            || center.iter().chain(&major).any(|v| !v.is_finite())
        {
            return Err(SketchError::InvalidValue(format!(
                "ellipse needs a centre distinct from its axis end and a minor radius > 0 \
                 (got axis {a} mm, minor {minor} mm)"
            )));
        }
        let c = self.add_point(center[0], center[1]);
        let m = self.add_point(major[0], major[1]);
        let id = self.next_entity_id();
        self.entities.push(Entity::Ellipse {
            id,
            center: c,
            major: m,
            minor,
        });
        Ok((id, c, m))
    }

    /// Add a cubic B-spline through new control points at `ctrl` (open:
    /// at least 4; closed: at least 3). Returns the entity and its points.
    /// Does not solve.
    pub fn add_spline(
        &mut self,
        ctrl: &[[f64; 2]],
        closed: bool,
    ) -> Result<(EntityId, Vec<PointId>), SketchError> {
        if ctrl.iter().flatten().any(|v| !v.is_finite()) {
            return Err(SketchError::InvalidValue(
                "spline control point is not finite".into(),
            ));
        }
        let id = self.next_entity_id();
        let fake: Vec<PointId> = (0..ctrl.len() as u32).map(PointId).collect();
        check_spline(id, &fake, closed)?;
        let points: Vec<PointId> = ctrl.iter().map(|p| self.add_point(p[0], p[1])).collect();
        self.entities.push(Entity::Spline {
            id,
            points: points.clone(),
            closed,
        });
        Ok((id, points))
    }
}

impl Sketch {
    /// Ellipse template: centre on a fixed anchor point, major axis along
    /// x (a horizontal construction line from the centre to the axis end),
    /// semi-axes `major_radius` (along x) and `minor_radius` (dimensions
    /// "major_radius", "minor_radius"). Points: 0 anchor, 1 centre, 2 axis
    /// end; entities: 1 ellipse, 2 axis line. Fully constrained (7 unknowns,
    /// 7 independent equations).
    pub fn centered_ellipse(major_radius: f64, minor_radius: f64) -> Result<Sketch, SketchError> {
        crate::check_dim_value("major_radius", major_radius)?;
        crate::check_dim_value("minor_radius", minor_radius)?;
        let mut s = Sketch::default();
        let o = s.add_point(0.0, 0.0);
        let (e, c, m) = s.add_ellipse([0.0, 0.0], [major_radius, 0.0], minor_radius)?;
        let axis = s.add_line(c, m);
        s.set_construction(axis, true);
        s.constraints = vec![
            crate::Constraint::FixedPoint {
                p: o,
                x: 0.0,
                y: 0.0,
            },
            crate::Constraint::Coincident { a: o, b: c },
            crate::Constraint::Horizontal { line: axis },
            crate::Constraint::EllipseRadius {
                ellipse: e,
                major: true,
                dim: crate::Dimension::new("major_radius", major_radius),
            },
            crate::Constraint::EllipseRadius {
                ellipse: e,
                major: false,
                dim: crate::Dimension::new("minor_radius", minor_radius),
            },
        ];
        s.validate()?;
        Ok(s)
    }
}

/// Display / picking polyline of an ellipse or spline: `per_span` equal
/// parameter steps per exact span, closed curves repeating their first point.
/// `None` for other entities or missing points. An approximation for drawing
/// only; profiles use the exact spans.
pub fn entity_polyline(s: &Sketch, e: &Entity, per_span: usize) -> Option<Vec<[f64; 2]>> {
    let per_span = per_span.max(1);
    let spans: Vec<([[f64; 2]; 4], [f64; 2])> = match e {
        Entity::Ellipse {
            center,
            major,
            minor,
            ..
        } => ellipse_spans(s.point_xy(*center)?, s.point_xy(*major)?, *minor)
            .into_iter()
            .map(|([p0, c1, c2], w)| ([p0, c1, c2, p0], w))
            .collect(),
        Entity::Spline { points, closed, .. } => {
            let ctrl = points
                .iter()
                .map(|p| s.point_xy(*p))
                .collect::<Option<Vec<_>>>()?;
            spline_beziers(&ctrl, *closed)
                .into_iter()
                .map(|b| (b, [1.0, 1.0]))
                .collect()
        }
        _ => return None,
    };
    if spans.is_empty() {
        return None;
    }
    // Ellipse spans carry no end point: it is the next span's start.
    let k = spans.len();
    let ends: Vec<[f64; 2]> = (0..k)
        .map(|i| match e {
            Entity::Ellipse { .. } => spans[(i + 1) % k].0[0],
            _ => spans[i].0[3],
        })
        .collect();
    let mut out = Vec::with_capacity(k * per_span + 1);
    for (i, (p, w)) in spans.iter().enumerate() {
        for j in 0..per_span {
            out.push(cubic_point(
                p[0],
                p[1],
                p[2],
                ends[i],
                *w,
                j as f64 / per_span as f64,
            ));
        }
    }
    out.push(ends[k - 1]);
    Some(out)
}
