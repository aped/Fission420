//! Rational cubic Bézier spans of profile loops ([`crate::LoopSegment::Cubic`]):
//! evaluation, the exact NURBS edge, Green's-theorem area, and point samples
//! for the 2D checks.
//!
//! A span has control points `start, c1, c2, end` with weights `1, w0, w1, 1`
//! (all positive). It is built as a single-span degree-3 NURBS curve
//! (Bézier knot vector), the same exact geometry, so ellipse quarters and
//! B-spline spans are never approximated in the B-rep.

use crate::{Frame, KernelError, Result};
use truck_modeling::*;

/// Point at `t ∈ [0, 1]` (rational de Casteljau via the Bernstein form).
pub(crate) fn point(p: [[f64; 2]; 4], w: [f64; 2], t: f64) -> [f64; 2] {
    let s = 1.0 - t;
    let b = [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t];
    let ww = [1.0, w[0], w[1], 1.0];
    let (mut x, mut y, mut d) = (0.0, 0.0, 0.0);
    for i in 0..4 {
        let k = b[i] * ww[i];
        x += k * p[i][0];
        y += k * p[i][1];
        d += k;
    }
    [x / d, y / d]
}

fn deriv(p: [[f64; 2]; 4], w: [f64; 2], t: f64) -> [f64; 2] {
    let s = 1.0 - t;
    let b = [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t];
    let db = [
        -3.0 * s * s,
        3.0 * s * s - 6.0 * s * t,
        6.0 * s * t - 3.0 * t * t,
        3.0 * t * t,
    ];
    let ww = [1.0, w[0], w[1], 1.0];
    let (mut n, mut dn, mut d, mut dd) = ([0.0; 2], [0.0; 2], 0.0, 0.0);
    for i in 0..4 {
        for k in 0..2 {
            n[k] += b[i] * ww[i] * p[i][k];
            dn[k] += db[i] * ww[i] * p[i][k];
        }
        d += b[i] * ww[i];
        dd += db[i] * ww[i];
    }
    [
        (dn[0] * d - n[0] * dd) / (d * d),
        (dn[1] * d - n[1] * dd) / (d * d),
    ]
}

/// 16-point Gauss–Legendre nodes / weights on [−1, 1] (positive half).
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

/// `∫ (x dy − y dx) / 2` along the span: composite 16-point Gauss–Legendre on
/// 8 pieces (exact for polynomial spans, whose integrand has degree 5; for
/// rational spans with weights near 1 the error is negligible).
pub(crate) fn area_term(p: [[f64; 2]; 4], w: [f64; 2]) -> f64 {
    const PIECES: usize = 8;
    let mut sum = 0.0;
    for k in 0..PIECES {
        let (a, b) = (k as f64 / PIECES as f64, (k + 1) as f64 / PIECES as f64);
        let (m, h) = ((a + b) / 2.0, (b - a) / 2.0);
        for &(x, wt) in &GL16 {
            for t in [m - h * x, m + h * x] {
                let q = point(p, w, t);
                let d = deriv(p, w, t);
                sum += wt * h * 0.5 * (q[0] * d[1] - q[1] * d[0]);
            }
        }
    }
    sum
}

/// `n` points at equal parameter steps from the start (end excluded).
pub(crate) fn samples(p: [[f64; 2]; 4], w: [f64; 2], n: usize) -> impl Iterator<Item = [f64; 2]> {
    (0..n).map(move |k| point(p, w, k as f64 / n as f64))
}

fn seg_dist(q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((q[0] - a[0]) * dx + (q[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (q[0] - a[0] - t * dx).hypot(q[1] - a[1] - t * dy)
}

/// Points of the span from the start (end excluded) forming, with the end,
/// a polyline within `delta` of the curve: de Casteljau subdivision (in
/// homogeneous coordinates) until both inner control points are within
/// `delta` of the chord. With positive weights the span lies in its control
/// points' convex hull, which then lies within `delta` of the chord.
pub(crate) fn flatten_into(out: &mut Vec<[f64; 2]>, p: [[f64; 2]; 4], w: [f64; 2], delta: f64) {
    let h = |q: [f64; 2], w: f64| [q[0] * w, q[1] * w, w];
    let c = [h(p[0], 1.0), h(p[1], w[0]), h(p[2], w[1]), h(p[3], 1.0)];
    rec(out, c, delta, 0);
}

/// Subdivision depth cap (2^18 pieces per span at most).
const MAX_DEPTH: u32 = 18;

fn rec(out: &mut Vec<[f64; 2]>, c: [[f64; 3]; 4], delta: f64, depth: u32) {
    let pt = |h: [f64; 3]| [h[0] / h[2], h[1] / h[2]];
    let (a, b) = (pt(c[0]), pt(c[3]));
    if depth >= MAX_DEPTH
        || (seg_dist(pt(c[1]), a, b) <= delta && seg_dist(pt(c[2]), a, b) <= delta)
    {
        out.push(a);
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
    rec(out, [c[0], p01, p012, m], delta, depth + 1);
    rec(out, [m, p123, p23, c[3]], delta, depth + 1);
}

/// Smallest and largest coordinate along unit 2D direction `d` over the
/// control points (bounds of the span by the convex-hull property).
pub(crate) fn hull_range(p: [[f64; 2]; 4], d: [f64; 2]) -> (f64, f64) {
    p.iter()
        .map(|q| q[0] * d[0] + q[1] * d[1])
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        })
}

/// The exact edge curve of the span in world coordinates (control points
/// mapped through the affine frame, exact for NURBS).
pub(crate) fn nurbs(p: [[f64; 2]; 4], w: [f64; 2], frame: &Frame) -> Result<NurbsCurve<Vector4>> {
    let ww = [1.0, w[0], w[1], 1.0];
    let ctrl: Vec<Vector4> = p
        .iter()
        .zip(ww)
        .map(|(q, w)| {
            let x = frame.to_world(*q);
            Vector4::new(x[0] * w, x[1] * w, x[2] * w, w)
        })
        .collect();
    let bs = BSplineCurve::try_new(KnotVec::bezier_knot(3), ctrl)
        .map_err(|e| KernelError::Internal(format!("cubic span construction failed: {e:?}")))?;
    Ok(NurbsCurve::new(bs))
}
