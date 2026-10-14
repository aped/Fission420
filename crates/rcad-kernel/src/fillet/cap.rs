//! Exact constant-radius fillet, or equal-distance chamfer, of the whole
//! outer edge loop of a planar cap face ("round / chamfer the top edge" of an
//! extruded plate, box or rounded rectangle).
//!
//! The chamfer is the same construction with a straight 45° section of legs
//! d: planes along lines, cones about the arcs' axes, and straight miter
//! lines at every junction. A convex arc must have R > d, because R = d would
//! end the cone in an apex. Its removed section has A = d²/2 and M = d³/6.
//!
//! Scope. Two kinds of body are accepted.
//! * A straight prism under the cap: one closed shell whose faces are the
//!   cap, one opposite cap parallel to it at depth `H`, and walls parallel to
//!   the cap normal everywhere (planes, cylinders, any extruded line/arc).
//!   This is checked on the B-rep (face classification, exact volume = cap
//!   area × H). Such a body is rebuilt whole, and its holes become through walls.
//! * Any other closed body whose blend region is clean (an enclosure with a
//!   cavity, a plate pocketed from the other side). Each outer-loop edge must
//!   bound a four-sided wall: the cap edge, two vertical lines shared with
//!   the neighbouring walls, and a far edge that is the cap edge translated
//!   to a common depth `H`. No other face may reach into the band within r
//!   of both the cap and the loop; this is checked on dense edge samples
//!   with a sampling-step margin. Only the cap face and these walls are
//!   rebuilt; every other face, including the far edges and the cap's hole
//!   loops, is kept as the same B-rep object. Each kept wall is trimmed on
//!   its own surface, so one wall may span several loop segments.
//! * Optionally (`*_cap_face_edges`) the face's hole loops are blended too,
//!   always through the second path; the hole walls must satisfy the same
//!   conditions, and the bands of different loops must stay 2r apart.
//!
//! The cap's outer loop is made of straight lines and circular arcs. Each
//! junction between consecutive edges is either tangent (G1), or a sharp
//! corner between two straight edges (convex or concave). Anything else is
//! refused by name: a sharp corner touching an arc, a convex arc of radius
//! `R < r` (the rolling ball does not fit), an arc with `0 < R − r <
//! MIN_LENGTH`, a straight edge too short for the setbacks of its corners,
//! walls closer than `r` to the inset loop (the fillet bands would overlap),
//! or a hole in the cap closer than `r` to the outer loop.
//!
//! Construction (direct B-rep, no boolean; see ADR 0014). In the cap's own
//! frame (cap at w = 0, outward normal +w, bottom at w = −H) every outer
//! segment gets a wall from −H to −r and a fillet face whose section is the
//! quarter circle of radius r centred at depth −r and inset r from the wall:
//! * straight segment: an exact cylinder (rational quadratic arc × line);
//! * arc of radius R about c: the torus of revolution about c's axis, major
//!   radius R − r (convex) or R + r (concave), minor radius r (a rational
//!   revolved surface);
//! * convex arc with R = r: the torus degenerates to the sphere of radius r
//!   about (c, −r); the face is a three-sided patch. It is parametrised by a
//!   revolution about a horizontal axis that misses the patch, so no surface
//!   pole lies on the face.
//!
//! Section edges between consecutive fillet faces lie in the plane bisecting
//! the junction: at a tangent junction the plane normal to the loop (a
//! quarter circle); at a sharp line–line corner the bisector plane, where the
//! two equal-radius cylinders (axes meeting at the inset corner) intersect in
//! the exact quarter ellipse with semi-axes r/cos(α/2) and r (α = turning
//! angle). The top face is the planar region bounded by the loop inset by r
//! (plus the kept holes). All curves are exact rational quadratics or lines.
//!
//! Volume self-check: the result must equal the input volume (cap area × H
//! for a prism) minus the closed-form removed volume. Oracle (independent of
//! the construction; docs/FILLETS.md): the
//! removed section is the spandrel of area A = r²(1 − π/4) whose centroid
//! lies d̄ = r(10 − 3π)/(12 − 3π) from each wall. Swept along the loop
//! (Pappus for arcs, the miter setbacks at sharp corners) it removes
//! A·(P − d̄·(Σ signed arc sweeps + 2·Σ tan(α_j/2))), P the loop length.
use crate::{Body, Frame, KernelError, Result, guarded, tol};
use std::f64::consts::{FRAC_PI_2, PI};
use truck_modeling::*;

type V2 = [f64; 2];

/// Angle (rad) below which a junction between two loop edges counts as
/// tangent. Sketch-solved tangencies are far tighter (1e-9 relative).
pub(crate) const TANGENT_ANGLE: f64 = 1.0e-6;
/// Largest |R − r| (mm) treated as the sphere case R = r (Truck's linear
/// tolerance; a parametric R = r is exact).
pub(crate) const SPHERE_MATCH: f64 = tol::KERNEL_LINEAR;
/// Panels per edge of the exact volume self-check (and twice that).
pub(crate) const VOLUME_PANELS: usize = 4;
/// Relative agreement required between the result's boundary-integral volume
/// and the closed form.
pub(crate) const VOLUME_REL: f64 = 1e-9;
/// Largest arc sweep kept as one loop segment (one rational quadratic
/// span; Truck's extruded circles are 120° arcs). Longer arcs are split.
const MAX_ARC_SWEEP: f64 = 2.0 * PI / 3.0;
/// Samples per segment for the clearance checks (inset loop, holes).
const FIT_SAMPLES: usize = 256;
/// Rotation (rad) of a revolved fillet surface's generatrix before the face
/// start, so the face never touches the surface's v = 0 seam.
const SEAM_OFFSET: f64 = 0.25;

/// Prefix of every message of this module, renamed per operation by
/// [`named`].
const PREFIX: &str = "edge loop: ";

fn bad(msg: impl std::fmt::Display) -> KernelError {
    KernelError::InvalidInput(format!("{PREFIX}{msg}"))
}

/// Which blend the loop gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Blend {
    /// Quarter-circle section of radius r.
    Fillet,
    /// Straight 45° section with both legs d (equal-distance chamfer).
    Chamfer,
}

impl Blend {
    fn word(self) -> &'static str {
        match self {
            Blend::Fillet => "fillet",
            Blend::Chamfer => "chamfer",
        }
    }
    fn size(self) -> &'static str {
        match self {
            Blend::Fillet => "radius",
            Blend::Chamfer => "distance",
        }
    }
    /// Area A and first moment M (about either wall) of the removed section.
    fn section(self, r: f64) -> (f64, f64) {
        match self {
            Blend::Fillet => (r * r * (1.0 - PI / 4.0), r.powi(3) * (5.0 / 6.0 - PI / 4.0)),
            Blend::Chamfer => (0.5 * r * r, r.powi(3) / 6.0),
        }
    }
}

/// Rewrite the module's generic message prefix for the operation.
fn named(kind: Blend, e: KernelError) -> KernelError {
    let fix = |m: String| match m.strip_prefix(PREFIX) {
        Some(rest) => format!("{} loop: {rest}", kind.word()),
        None => m,
    };
    match e {
        KernelError::InvalidInput(m) => KernelError::InvalidInput(fix(m)),
        KernelError::InvalidResult(m) => KernelError::InvalidResult(fix(m)),
        KernelError::Internal(m) => KernelError::Internal(fix(m)),
        other => other,
    }
}

fn sub(a: V2, b: V2) -> V2 {
    [a[0] - b[0], a[1] - b[1]]
}
fn add(a: V2, b: V2) -> V2 {
    [a[0] + b[0], a[1] + b[1]]
}
fn mul(a: V2, s: f64) -> V2 {
    [a[0] * s, a[1] * s]
}
fn len(a: V2) -> f64 {
    a[0].hypot(a[1])
}
fn unit(a: V2) -> V2 {
    mul(a, 1.0 / len(a))
}
fn cross(a: V2, b: V2) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn dot(a: V2, b: V2) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
/// Left normal (the material side of a loop traversed with material on the left).
fn left(t: V2) -> V2 {
    [-t[1], t[0]]
}

/// One edge of a cap loop in the cap frame, traversed with material on the left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Seg {
    Line {
        a: V2,
        b: V2,
    },
    /// Arc about `c` of radius `rad` from `a` to `b`, sweep ≤ π/2.
    Arc {
        a: V2,
        b: V2,
        c: V2,
        rad: f64,
        ccw: bool,
    },
}

impl Seg {
    fn a(&self) -> V2 {
        match *self {
            Seg::Line { a, .. } | Seg::Arc { a, .. } => a,
        }
    }
    fn b(&self) -> V2 {
        match *self {
            Seg::Line { b, .. } | Seg::Arc { b, .. } => b,
        }
    }
    /// Unit tangent at the start (`end = false`) or end.
    fn tangent(&self, end: bool) -> V2 {
        match *self {
            Seg::Line { a, b } => unit(sub(b, a)),
            Seg::Arc { a, b, c, ccw, .. } => {
                let radial = unit(sub(if end { b } else { a }, c));
                if ccw {
                    left(radial)
                } else {
                    mul(left(radial), -1.0)
                }
            }
        }
    }
    /// Signed sweep (rad), 0 for a line.
    fn sweep(&self) -> f64 {
        match *self {
            Seg::Line { .. } => 0.0,
            Seg::Arc { a, b, c, ccw, .. } => crate::arc_signed_sweep(a, b, c, ccw),
        }
    }
    fn length(&self) -> f64 {
        match *self {
            Seg::Line { a, b } => len(sub(b, a)),
            Seg::Arc { rad, .. } => rad * self.sweep().abs(),
        }
    }
    /// Point at fraction `s` of the segment.
    fn at(&self, s: f64) -> V2 {
        match *self {
            Seg::Line { a, b } => add(a, mul(sub(b, a), s)),
            Seg::Arc { a, c, rad, .. } => {
                let a0 = (a[1] - c[1]).atan2(a[0] - c[0]);
                let t = a0 + self.sweep() * s;
                [c[0] + rad * t.cos(), c[1] + rad * t.sin()]
            }
        }
    }
    /// Exact distance from `p` to the segment.
    fn dist(&self, p: V2) -> f64 {
        match *self {
            Seg::Line { a, b } => {
                let d = sub(b, a);
                let s = (dot(sub(p, a), d) / dot(d, d)).clamp(0.0, 1.0);
                len(sub(p, add(a, mul(d, s))))
            }
            Seg::Arc { a, b, c, rad, ccw } => {
                let q = sub(p, c);
                let ends = len(sub(p, a)).min(len(sub(p, b)));
                if len(q) == 0.0 {
                    return rad;
                }
                // The radial projection is the nearest point when it lies on the arc.
                let foot = add(c, mul(unit(q), rad));
                let sw = crate::arc_signed_sweep(a, b, c, ccw).abs();
                let at = crate::arc_signed_sweep(a, foot, c, ccw).abs();
                if at <= sw { (len(q) - rad).abs() } else { ends }
            }
        }
    }
}

/// The cap face's loops in the cap frame: the outer loop (counter-clockwise)
/// and the holes (clockwise), both with material on the left.
pub(crate) struct CapLoops {
    pub outer: Vec<Seg>,
    pub holes: Vec<Vec<Seg>>,
}

/// Right-handed cap frame: origin and in-plane axes of `cap`, normal x × y.
fn cap_frame(cap: &Frame) -> Result<(Point3, Vector3, Vector3, Vector3)> {
    let o = Point3::new(cap.origin[0], cap.origin[1], cap.origin[2]);
    let z = Vector3::new(cap.z[0], cap.z[1], cap.z[2]);
    let x = Vector3::new(cap.x[0], cap.x[1], cap.x[2]);
    let y = z.cross(x);
    Ok((o, x, y, z))
}

/// Recognise one cap-boundary edge (oriented) as a line or circular arc in the
/// cap plane; arcs over a quarter turn are split into equal pieces ≤ π/2.
fn edge_segments(e: &Edge, to2: &dyn Fn(Point3) -> (V2, f64)) -> Result<Vec<Seg>> {
    let c = e.oriented_curve();
    let c = match c {
        Curve::IntersectionCurve(ic) => {
            crate::exact::exact_edge(&ic, e.front().point(), e.back().point()).map_err(|x| {
                bad(format!(
                    "a cap edge is an implicit intersection curve ({x})"
                ))
            })?
        }
        other => other,
    };
    let (t0, t1) = c.range_tuple();
    let pt = |s: f64| to2(c.subs(t0 + (t1 - t0) * s));
    let (a, wa) = to2(e.front().point());
    let (b, wb) = to2(e.back().point());
    let samples: Vec<(V2, f64)> = (0..=16).map(|k| pt(k as f64 / 16.0)).collect();
    let scale = 1.0 + len(sub(b, a));
    if samples
        .iter()
        .chain([(a, wa), (b, wb)].iter())
        .any(|(_, w)| w.abs() > 1e-7 * scale)
    {
        return Err(bad("a cap edge leaves the cap plane"));
    }
    let chord = sub(b, a);
    if len(chord) < tol::MIN_LENGTH {
        return Err(bad("the cap loop has a closed or degenerate edge"));
    }
    let line_dev = samples
        .iter()
        .map(|(p, _)| cross(chord, sub(*p, a)).abs() / len(chord))
        .fold(0.0, f64::max);
    if line_dev <= 1e-9 * scale {
        return Ok(vec![Seg::Line { a, b }]);
    }
    // Circle through a, the middle sample and b.
    let m = samples[8].0;
    let (ab, am) = (sub(b, a), sub(m, a));
    let d = 2.0 * cross(ab, am);
    if d.abs() < 1e-12 {
        return Err(bad("a cap edge is neither straight nor a circular arc"));
    }
    let (lb, lm) = (dot(ab, ab), dot(am, am));
    let c0 = add(
        a,
        [(am[1] * lb - ab[1] * lm) / d, (ab[0] * lm - am[0] * lb) / d],
    );
    let rad = len(sub(a, c0));
    if samples
        .iter()
        .any(|(p, _)| (len(sub(*p, c0)) - rad).abs() > 1e-7 * (1.0 + rad))
    {
        return Err(bad(
            "a cap edge is curved but not a circular arc (only lines and arcs are supported)",
        ));
    }
    // Counter-clockwise when the middle sample comes before b going ccw from a.
    let ccw = crate::arc_signed_sweep(a, m, c0, true) < crate::arc_signed_sweep(a, b, c0, true);
    let sweep = crate::arc_signed_sweep(a, b, c0, ccw);
    let k = ((sweep.abs() / MAX_ARC_SWEEP - 1e-9).ceil() as usize).max(1);
    let a0 = (a[1] - c0[1]).atan2(a[0] - c0[0]);
    let on = |t: f64| [c0[0] + rad * t.cos(), c0[1] + rad * t.sin()];
    Ok((0..k)
        .map(|j| Seg::Arc {
            a: if j == 0 {
                a
            } else {
                on(a0 + sweep * j as f64 / k as f64)
            },
            b: if j + 1 == k {
                b
            } else {
                on(a0 + sweep * (j + 1) as f64 / k as f64)
            },
            c: c0,
            rad,
            ccw,
        })
        .collect())
}

fn loop_area(segs: &[Seg]) -> f64 {
    segs.iter()
        .map(|s| {
            let (p, q) = (s.a(), s.b());
            let mut a = 0.5 * cross(p, q);
            if let Seg::Arc { rad, .. } = s {
                let t = s.sweep();
                a += 0.5 * rad * rad * (t - t.sin());
            }
            a
        })
        .sum()
}

/// The single planar face of `body` in the cap plane with outward normal
/// `z`, and every other face classified: the one opposite cap (depth H) and
/// walls ruled along z. Returns the cap loops and H.
fn prism_under_cap(body: &Body, cap: &Frame) -> Result<(CapLoops, f64)> {
    let (o, x, y, z) = cap_frame(cap)?;
    let to2 = |p: Point3| {
        let d = p - o;
        ([d.dot(x), d.dot(y)], d.dot(z))
    };
    let shells = body.solid.boundaries();
    if shells.len() != 1 {
        return Err(bad("the body must be one closed shell (no voids)"));
    }
    let bounds = crate::measure::raw_bounds(&body.solid)?;
    let diag = (0..3)
        .map(|i| (bounds.max[i] - bounds.min[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    let ptol = 1e-9 * diag.max(1.0);
    let mut caps = Vec::new();
    let mut bottoms = Vec::new();
    for (i, f) in body.solid.face_iter().enumerate() {
        let s = f.surface();
        if let Surface::Plane(pl) = &s {
            let mut n = pl.normal();
            if !f.orientation() {
                n = -n;
            }
            let w = (pl.origin() - o).dot(z);
            if (n - z).magnitude() < tol::FACE_MATCH {
                if w.abs() <= ptol {
                    caps.push(f.clone());
                    continue;
                }
                return Err(bad(format!(
                    "face {i} is parallel to the cap at another height: the body is not a \
                     straight prism under the cap face"
                )));
            }
            if (n + z).magnitude() < tol::FACE_MATCH {
                bottoms.push((f.clone(), -w));
                continue;
            }
            if n.dot(z).abs() < tol::FACE_MATCH {
                continue; // planar wall
            }
            return Err(bad(format!(
                "face {i} is inclined to the cap: the body is not a straight prism under the \
                 cap face"
            )));
        }
        // A curved face must be a wall: normal perpendicular to z everywhere.
        let ((u0, u1), (v0, v1)) = match s.parameter_range() {
            (
                (std::ops::Bound::Included(a), std::ops::Bound::Included(b)),
                (std::ops::Bound::Included(c), std::ops::Bound::Included(d)),
            ) => ((a, b), (c, d)),
            ((std::ops::Bound::Included(a), std::ops::Bound::Included(b)), (_, _)) => {
                ((a, b), (0.0, 2.0 * PI))
            }
            _ => {
                return Err(bad(format!(
                    "face {i} has an unbounded curved surface; the body is not a straight \
                     prism under the cap face"
                )));
            }
        };
        let ruled = (0..=6).all(|a| {
            (0..=6).all(|b| {
                let n = s.normal(
                    u0 + (u1 - u0) * a as f64 / 6.0,
                    v0 + (v1 - v0) * b as f64 / 6.0,
                );
                n.dot(z).abs() < 1e-9
            })
        });
        if !ruled {
            return Err(bad(format!(
                "face {i} is curved but not a wall parallel to the cap normal (an existing \
                 fillet or blend touches the body): the body is not a straight prism under the \
                 cap face"
            )));
        }
    }
    if caps.len() != 1 {
        return Err(bad(format!(
            "expected exactly one face in the cap plane, found {}",
            caps.len()
        )));
    }
    if bottoms.len() != 1 {
        return Err(bad(format!(
            "expected exactly one opposite cap parallel to the face, found {}",
            bottoms.len()
        )));
    }
    let h = bottoms[0].1;
    let mut outer = Vec::new();
    let mut holes = Vec::new();
    for w in caps[0].boundaries() {
        let mut segs = Vec::new();
        for e in w.edge_iter() {
            segs.extend(edge_segments(e, &to2)?);
        }
        if loop_area(&segs) > 0.0 {
            if !outer.is_empty() {
                return Err(bad("the cap face has two outer loops"));
            }
            outer = segs;
        } else {
            holes.push(segs);
        }
    }
    if outer.len() < 2 {
        return Err(bad("the cap face has no outer loop"));
    }
    // The body is the prism of the cap region: exact volume = area × H.
    let area = loop_area(&outer) + holes.iter().map(|h| loop_area(h)).sum::<f64>();
    match crate::measure(body)?.volume {
        crate::VolumeResult::Value {
            volume,
            error_estimate,
            method,
        } if method != crate::VolumeMethod::TessellationEstimate => {
            let want = area * h;
            if (volume - want).abs() > (1e-9 * want.abs()).max(1e-9) + 3.0 * error_estimate {
                return Err(bad(format!(
                    "the body is not a straight prism under the cap face (volume {volume:.6} \
                     mm^3, cap area × height {want:.6} mm^3)"
                )));
            }
        }
        _ => {
            return Err(bad(
                "cannot verify that the body is a straight prism under the cap face (no exact \
                 volume)",
            ));
        }
    }
    Ok((CapLoops { outer, holes }, h))
}

/// Junction analysis and inset loop.
struct Plan {
    segs: Vec<Seg>,
    /// Turning angle at the start of each segment (signed, left positive).
    alpha: Vec<f64>,
    /// Inset (top) vertex at the start of each segment.
    q: Vec<V2>,
    /// Segment's inset curve degenerates to a point (convex arc with R = r).
    sphere: Vec<bool>,
}

fn plan(segs: &[Seg], holes: &[Vec<Seg>], r: f64, kind: Blend) -> Result<Plan> {
    let (word, size) = (kind.word(), kind.size());
    let n = segs.len();
    let mut alpha = vec![0.0; n];
    let mut q = vec![[0.0; 2]; n];
    for j in 0..n {
        let prev = &segs[(j + n - 1) % n];
        let next = &segs[j];
        let (ti, to) = (prev.tangent(true), next.tangent(false));
        let a = cross(ti, to).atan2(dot(ti, to));
        let p = next.a();
        // Tangent only when treating the corner as tangent moves the inset
        // geometry by no more than Truck's linear tolerance (r·|α|).
        if a.abs() <= TANGENT_ANGLE && a.abs() * r <= tol::KERNEL_LINEAR {
            alpha[j] = 0.0;
            q[j] = add(p, mul(left(to), r));
        } else {
            if !matches!(prev, Seg::Line { .. }) || !matches!(next, Seg::Line { .. }) {
                return Err(bad(format!(
                    "the cap loop has a sharp corner at ({:.4}, {:.4}) next to an arc; only \
                     tangent junctions or sharp corners between two straight edges can be \
                     blended (round the corner first)",
                    p[0], p[1]
                )));
            }
            if a.abs() > PI - 1e-3 {
                return Err(bad("the cap loop folds back on itself at a corner"));
            }
            alpha[j] = a;
            let bis = unit(add(left(ti), left(to)));
            q[j] = add(p, mul(bis, r / (a / 2.0).cos()));
        }
    }
    let mut sphere = vec![false; n];
    for (i, s) in segs.iter().enumerate() {
        match *s {
            Seg::Line { a, b } => {
                let qa = q[i];
                let qb = q[(i + 1) % n];
                let d = sub(qb, qa);
                if dot(d, sub(b, a)) <= 0.0 || len(d) < tol::MIN_LENGTH {
                    return Err(bad(format!(
                        "{size} {r} mm does not fit: the straight edge of length {:.4} mm is too \
                         short for its corner setbacks",
                        len(sub(b, a))
                    )));
                }
            }
            Seg::Arc { rad, ccw, .. } => {
                if ccw {
                    let rho = rad - r;
                    if rho.abs() <= SPHERE_MATCH && kind == Blend::Fillet {
                        sphere[i] = true;
                    } else if rho.abs() <= SPHERE_MATCH {
                        return Err(bad(format!(
                            "distance {r} mm equals the convex arc radius {rad:.6} mm: the \
                             corner cone would end in an apex (not supported; use a smaller \
                             distance)"
                        )));
                    } else if rho < 0.0 {
                        return Err(bad(format!(
                            "{size} {r} mm is larger than the convex arc radius {rad:.6} mm of \
                             the cap loop (corner blends of a smaller vertical radius are not \
                             supported; use a {size} <= R)"
                        )));
                    } else if rho < tol::MIN_LENGTH {
                        return Err(bad(format!(
                            "{size} {r} mm is within {rho:.2e} mm of the arc radius {rad:.6} mm: \
                             make them equal or differ by at least {} mm",
                            tol::MIN_LENGTH
                        )));
                    }
                }
            }
        }
    }
    for i in 0..n {
        if sphere[i] && sphere[(i + 1) % n] {
            return Err(bad(
                "two consecutive arcs of radius equal to the fillet radius would collapse the \
                 top face (the cap is too narrow for this radius)",
            ));
        }
    }
    // Inset loop: every inset point keeps distance >= r from the whole loop
    // (otherwise fillet bands of different edges overlap), and it has area.
    let inset_at = |i: usize, s: f64| -> V2 {
        match segs[i] {
            Seg::Line { .. } => add(q[i], mul(sub(q[(i + 1) % n], q[i]), s)),
            Seg::Arc { c, .. } => {
                let p = segs[i].at(s);
                let rad = len(sub(p, c));
                let rho = len(sub(q[i], c));
                add(c, mul(sub(p, c), rho / rad))
            }
        }
    };
    let min_dist = |p: V2| segs.iter().map(|s| s.dist(p)).fold(f64::INFINITY, f64::min);
    let scale = segs.iter().map(|s| s.length()).sum::<f64>();
    for (i, _) in sphere.iter().enumerate().filter(|(_, s)| !**s) {
        for k in 0..=FIT_SAMPLES {
            let p = inset_at(i, k as f64 / FIT_SAMPLES as f64);
            let d = min_dist(p);
            if d < r - 1e-9 * scale.max(1.0) {
                return Err(bad(format!(
                    "{size} {r} mm does not fit: the loop's walls come within {:.4} mm of the \
                     {word}'s inset edge at ({:.4}, {:.4}) (the {word} bands would overlap)",
                    d, p[0], p[1]
                )));
            }
        }
    }
    for hole in holes {
        for s in hole {
            let step = s.length() / FIT_SAMPLES as f64;
            for k in 0..=FIT_SAMPLES {
                let p = s.at(k as f64 / FIT_SAMPLES as f64);
                if min_dist(p) <= r + 0.5 * step + tol::KERNEL_LINEAR {
                    return Err(bad(format!(
                        "a hole in the cap face at ({:.4}, {:.4}) lies within the {word} {size} \
                         {r} mm of the outer edge loop",
                        p[0], p[1]
                    )));
                }
            }
        }
    }
    Ok(Plan {
        segs: segs.to_vec(),
        alpha,
        q,
        sphere,
    })
}

/// Closed-form volume removed by the blend (see the module docs).
pub(crate) fn removed_volume(segs: &[Seg], alpha: &[f64], r: f64, kind: Blend) -> f64 {
    let (area, moment) = kind.section(r);
    let perim: f64 = segs.iter().map(|s| s.length()).sum();
    let turn: f64 = segs.iter().map(|s| s.sweep()).sum();
    let corners: f64 = alpha.iter().map(|a| 2.0 * (a / 2.0).tan()).sum();
    area * perim - moment * (turn + corners)
}

/// Geometry builder in the cap frame.
struct Geo {
    o: Point3,
    x: Vector3,
    y: Vector3,
    z: Vector3,
}

impl Geo {
    fn w(&self, p: V2, h: f64) -> Point3 {
        self.o + self.x * p[0] + self.y * p[1] + self.z * h
    }
    fn dir(&self, d: V2) -> Vector3 {
        self.x * d[0] + self.y * d[1]
    }
    /// Arc about `c` from `a` to `b` with signed sweep `sweep` (|sweep| < π)
    /// at height `h`, as one rational quadratic span through the exact end
    /// points.
    fn arc(&self, a: V2, b: V2, c: V2, sweep: f64, h: f64) -> NurbsCurve<Vector4> {
        let half = 0.5 * sweep;
        let rad = len(sub(a, c));
        let a0 = (a[1] - c[1]).atan2(a[0] - c[0]) + half;
        let apex = add(c, mul([a0.cos(), a0.sin()], rad / half.cos()));
        conic(self.w(a, h), self.w(apex, h), self.w(b, h), half.cos())
    }
    /// Curve of a loop segment at height `h` (a line or one rational arc span).
    fn seg_curve(&self, s: &Seg, h: f64) -> Result<Curve> {
        Ok(match *s {
            Seg::Line { a, b } => Curve::Line(Line(self.w(a, h), self.w(b, h))),
            Seg::Arc { a, b, c, .. } => Curve::NurbsCurve(self.arc(a, b, c, s.sweep(), h)),
        })
    }
}

fn hom(p: Point3, w: f64) -> Vector4 {
    Vector4::new(p.x * w, p.y * w, p.z * w, w)
}

/// Rational quadratic Bezier (p0, p1, p2) with middle weight `w`.
fn conic(p0: Point3, p1: Point3, p2: Point3, w: f64) -> NurbsCurve<Vector4> {
    NurbsCurve::new(BSplineCurve::new(
        KnotVec::bezier_knot(2),
        vec![hom(p0, 1.0), hom(p1, w), hom(p2, 1.0)],
    ))
}

/// Circular arc about `c` in the plane (e1, e2), radius `r`, angles b0..b1
/// (|b1 − b0| < π), as one rational quadratic span.
fn circle_arc(
    c: Point3,
    e1: Vector3,
    e2: Vector3,
    r: f64,
    b0: f64,
    b1: f64,
) -> NurbsCurve<Vector4> {
    let half = 0.5 * (b1 - b0);
    let at = |t: f64, rr: f64| c + (e1 * t.cos() + e2 * t.sin()) * rr;
    conic(
        at(b0, r),
        at(b0 + half, r / half.cos()),
        at(b1, r),
        half.cos(),
    )
}

/// Face bounded by `wire` (counter-clockwise seen from outside) on `surface`;
/// `outward` is the expected outward direction at parameter `uv`.
fn oriented_face(wire: Wire, surface: Surface, uv: (f64, f64), outward: Vector3) -> Result<Face> {
    let n = surface.normal(uv.0, uv.1);
    let face = if n.dot(outward) >= 0.0 {
        Face::try_new(vec![wire], surface)
    } else {
        Face::try_new(vec![wire.inverse()], surface).map(|f| f.inverse())
    };
    face.map_err(|e| KernelError::InvalidResult(format!("{PREFIX}face: {e:?}")))
}

fn plane_face(wires: Vec<Wire>, o: Point3, u: Vector3, v: Vector3) -> Result<Face> {
    Face::try_new(wires, Surface::Plane(Plane::new(o, o + u, o + v)))
        .map_err(|e| KernelError::InvalidResult(format!("{PREFIX}planar face: {e:?}")))
}

/// Wall faces of a loop (material on the left) between heights `lo` and
/// `hi`, given its edges at both heights and its vertical edges.
fn wall_faces(
    g: &Geo,
    segs: &[Seg],
    lo: &[Edge],
    hi: &[Edge],
    up: &[Edge],
    zlo: f64,
    zhi: f64,
) -> Result<Vec<Face>> {
    let n = segs.len();
    let mut out = Vec::with_capacity(n);
    for (i, s) in segs.iter().enumerate() {
        let wire: Wire = vec![
            lo[i].clone(),
            up[(i + 1) % n].clone(),
            hi[i].inverse(),
            up[i].inverse(),
        ]
        .into();
        let t = s.tangent(false);
        let right = g.dir(mul(left(t), -1.0));
        match *s {
            Seg::Line { a, b } => {
                let p = g.w(a, zlo);
                out.push(plane_face(
                    vec![wire],
                    p,
                    g.w(b, zlo) - p,
                    g.z * (zhi - zlo),
                )?);
            }
            Seg::Arc { .. } => {
                let Curve::NurbsCurve(c) = g.seg_curve(s, zlo)? else {
                    unreachable!("arc segment curve")
                };
                let ctrl = c.control_points();
                let lift = g.z * (zhi - zlo);
                let grid: Vec<Vec<Vector4>> = ctrl
                    .iter()
                    .map(|v| vec![*v, *v + lift.extend(0.0) * v.w])
                    .collect();
                let surf = NurbsSurface::new(BSplineSurface::new(
                    (KnotVec::bezier_knot(2), KnotVec::bezier_knot(1)),
                    grid,
                ));
                // Outward at the start of the arc (u = 0) for a short sweep.
                out.push(oriented_face(
                    wire,
                    Surface::NurbsSurface(surf),
                    (0.0, 0.5),
                    right,
                )?);
            }
        }
    }
    Ok(out)
}

/// One kept wall of a general body: the cap edge it rises from covers the
/// loop segments `first .. first + count`; `far` is its far edge (oriented
/// along the loop) and `surface` its own surface, reused for the trimmed wall.
#[derive(Clone)]
struct Group {
    first: usize,
    count: usize,
    far: Edge,
    surface: Surface,
}

/// How the walls under a blended loop are rebuilt.
enum Walls<'a> {
    /// One new wall per segment, from the lower edges `eb` / vertices `vb`.
    PerSeg { vb: &'a [Vertex], eb: &'a [Edge] },
    /// One trimmed wall per kept wall face.
    Groups(&'a [Group]),
}

/// What lies below the blended walls.
enum Lower {
    /// A straight prism: rebuild the walls' lower parts, the hole walls and
    /// the opposite cap (holes given as loops).
    Prism { holes: Vec<Vec<Seg>> },
    /// Keep the rest of the body: each wall's far edge (oriented along its
    /// segment, at depth h), the hole wires of the cap face, and every face
    /// not touched by the blend.
    Keep {
        groups: Vec<Group>,
        hole_wires: Vec<Wire>,
        others: Vec<Face>,
        /// Further blended loops (holes of the cap face): plan, walls and
        /// wall depth of each.
        extra: Vec<(Plan, Vec<Group>, f64)>,
    },
}

/// The kept walls trimmed to depth `r`: each keeps its surface and far edge;
/// its top is the chain of the new mid edges `em` of its segments.
fn group_walls(
    g: &Geo,
    segs: &[Seg],
    groups: &[Group],
    vm: &[Vertex],
    em: &[Edge],
    h: f64,
    r: f64,
) -> Result<Vec<Face>> {
    let n = segs.len();
    let k = groups.len();
    let ev: Vec<Edge> = groups
        .iter()
        .map(|gr| builder::line(gr.far.front(), &vm[gr.first]))
        .collect();
    let mut out = Vec::with_capacity(k);
    for (gi, gr) in groups.iter().enumerate() {
        let mut edges = vec![gr.far.clone(), ev[(gi + 1) % k].clone()];
        for j in (gr.first..gr.first + gr.count).rev() {
            edges.push(em[j % n].inverse());
        }
        edges.push(ev[gi].inverse());
        let wire: Wire = edges.into();
        // Outward (right of travel) at the middle of the group's first segment.
        let s = &segs[gr.first % n];
        let mid = g.w(s.at(0.5), -0.5 * (h + r));
        let right = g.dir(mul(left(s.tangent(false)), -1.0));
        let right = match *s {
            Seg::Line { .. } => right,
            Seg::Arc { c, ccw, .. } => {
                let radial = g.dir(unit(sub(s.at(0.5), c)));
                if ccw { radial } else { -radial }
            }
        };
        let uv = gr
            .surface
            .search_parameter(mid, None, 100)
            .ok_or_else(|| bad("a kept wall's surface does not contain its trimmed part"))?;
        out.push(oriented_face(wire, gr.surface.clone(), uv, right)?);
    }
    Ok(out)
}

/// Walls (from the lower edges `eb` / vertices `vb` at depth `h` up to
/// depth `r`) and blend faces of one loop; returns them and the loop's inset
/// wire on the cap plane.
fn strip(
    g: &Geo,
    p: &Plan,
    walls: Walls,
    h: f64,
    r: f64,
    kind: Blend,
) -> Result<(Vec<Face>, Wire)> {
    let segs = &p.segs;
    let n = segs.len();
    let s2 = std::f64::consts::FRAC_1_SQRT_2;
    let vm: Vec<Vertex> = segs
        .iter()
        .map(|s| builder::vertex(g.w(s.a(), -r)))
        .collect();
    // Top vertices; the two ends of a sphere segment share one.
    let start = (0..n)
        .find(|&j| !p.sphere[(j + n - 1) % n])
        .ok_or_else(|| bad("every edge collapses"))?;
    let mut vt: Vec<Option<Vertex>> = vec![None; n];
    for k in 0..n {
        let j = (start + k) % n;
        let prev = (j + n - 1) % n;
        vt[j] = Some(if p.sphere[prev] && k > 0 {
            vt[prev].clone().expect("previous top vertex")
        } else {
            builder::vertex(g.w(p.q[j], 0.0))
        });
    }
    let vt: Vec<Vertex> = vt.into_iter().map(|v| v.expect("top vertex")).collect();
    let mut em = Vec::with_capacity(n);
    for (i, s) in segs.iter().enumerate() {
        em.push(Edge::new(&vm[i], &vm[(i + 1) % n], g.seg_curve(s, -r)?));
    }
    // Section edges: quarter circle / ellipse from (p, −r) over the corner
    // control point (p, 0) to the inset vertex (q, 0).
    let eq: Vec<Edge> = (0..n)
        .map(|j| {
            let pj = segs[j].a();
            match kind {
                Blend::Fillet => {
                    let c = conic(g.w(pj, -r), g.w(pj, 0.0), g.w(p.q[j], 0.0), s2);
                    Edge::new(&vm[j], &vt[j], Curve::NurbsCurve(c))
                }
                // The two 45° planes (or cones) meet in a straight line.
                Blend::Chamfer => builder::line(&vm[j], &vt[j]),
            }
        })
        .collect();
    // Top (inset) edges.
    let mut et: Vec<Option<Edge>> = Vec::with_capacity(n);
    for (i, s) in segs.iter().enumerate() {
        let j = (i + 1) % n;
        if p.sphere[i] {
            et.push(None);
            continue;
        }
        let c = match *s {
            Seg::Line { .. } => Curve::Line(Line(g.w(p.q[i], 0.0), g.w(p.q[j], 0.0))),
            Seg::Arc { c, .. } => Curve::NurbsCurve(g.arc(p.q[i], p.q[j], c, s.sweep(), 0.0)),
        };
        et.push(Some(Edge::new(&vt[i], &vt[j], c)));
    }
    let mut faces = match walls {
        Walls::PerSeg { vb, eb } => {
            let ev: Vec<Edge> = (0..n).map(|j| builder::line(&vb[j], &vm[j])).collect();
            wall_faces(g, segs, eb, &em, &ev, -h, -r)?
        }
        Walls::Groups(groups) => group_walls(g, segs, groups, &vm, &em, h, r)?,
    };
    // Fillet faces.
    for (i, s) in segs.iter().enumerate() {
        let j = (i + 1) % n;
        let mut edges = vec![em[i].clone(), eq[j].clone()];
        if let Some(e) = &et[i] {
            edges.push(e.inverse());
        }
        edges.push(eq[i].inverse());
        let wire: Wire = edges.into();
        let face = match *s {
            Seg::Line { a, b } if kind == Blend::Chamfer => {
                // Plane through the edge at depth r and the inset line: normal
                // t × (r n + r z) = r (z + right), outward.
                let o = g.w(a, -r);
                let nin = left(unit(sub(b, a)));
                plane_face(
                    vec![wire],
                    o,
                    g.w(b, -r) - o,
                    g.w(add(a, mul(nin, r)), 0.0) - o,
                )?
            }
            Seg::Line { a, b } => {
                let t = unit(sub(b, a));
                let nin = left(t);
                let l = len(sub(b, a));
                let m = r * (1.0 + (p.alpha[i] / 2.0).tan().abs() + (p.alpha[j] / 2.0).tan().abs());
                let sec = [
                    (g.w(a, -r), 1.0),
                    (g.w(a, 0.0), s2),
                    (g.w(add(a, mul(nin, r)), 0.0), 1.0),
                ];
                let (d0, d1) = (g.dir(t) * -m, g.dir(t) * (l + m));
                let grid: Vec<Vec<Vector4>> = sec
                    .iter()
                    .map(|(pt, w)| vec![hom(*pt + d0, *w), hom(*pt + d1, *w)])
                    .collect();
                let surf = NurbsSurface::new(BSplineSurface::new(
                    (KnotVec::bezier_knot(2), KnotVec::bezier_knot(1)),
                    grid,
                ));
                oriented_face(
                    wire,
                    Surface::NurbsSurface(surf),
                    (0.5, 0.5),
                    g.z - g.dir(nin),
                )?
            }
            Seg::Arc { a, b, c, .. } if p.sphere[i] => {
                // Sphere about (c, −r): revolve a great-circle arc about the
                // horizontal axis a' = (n_e − n_s)/|n_e − n_s| (outward
                // normals at the arc ends), which misses the patch.
                let (ns, ne) = (g.dir(unit(sub(a, c))), g.dir(unit(sub(b, c))));
                let axis = (ne - ns).normalize();
                let bis = (ne + ns).normalize();
                let phi = s.sweep().abs();
                let center = g.w(c, -r);
                // Generatrix rotated back by SEAM_OFFSET about the axis:
                // right-hand rotation about `axis` takes z to axis × z = bis.
                let e2 = g.z * SEAM_OFFSET.cos() - bis * SEAM_OFFSET.sin();
                let margin = 0.1;
                let generatrix = circle_arc(
                    center,
                    axis,
                    e2,
                    r,
                    FRAC_PI_2 - phi / 2.0 - margin,
                    FRAC_PI_2 + phi / 2.0 + margin,
                );
                let surf = Processor::new(RevolutedCurve::by_revolution(
                    Curve::NurbsCurve(generatrix),
                    center,
                    axis,
                ));
                oriented_face(
                    wire,
                    Surface::RevolutedCurve(surf),
                    (0.5, SEAM_OFFSET + PI / 4.0),
                    g.z + bis,
                )?
            }
            Seg::Arc { a, c, ccw, .. } => {
                // Torus about the arc's axis; positive rotation follows the arc.
                let axis = if ccw { g.z } else { -g.z };
                let t0 = s.tangent(false);
                let nin = left(t0);
                // Generatrix: the section at the arc start rotated back by SEAM_OFFSET.
                let rot = Matrix4::from_translation(g.w(c, 0.0).to_vec())
                    * Matrix4::from_axis_angle(axis, Rad(-SEAM_OFFSET))
                    * Matrix4::from_translation(-g.w(c, 0.0).to_vec());
                let (p0, p2) = (
                    rot.transform_point(g.w(a, -r)),
                    rot.transform_point(g.w(add(a, mul(nin, r)), 0.0)),
                );
                // Fillet: torus (quarter-circle section); chamfer: cone
                // (straight section).
                let sec = match kind {
                    Blend::Fillet => {
                        Curve::NurbsCurve(conic(p0, rot.transform_point(g.w(a, 0.0)), p2, s2))
                    }
                    Blend::Chamfer => Curve::Line(Line(p0, p2)),
                };
                let surf = Processor::new(RevolutedCurve::by_revolution(sec, g.w(c, 0.0), axis));
                // Outward at the arc middle: up, and away from the axis for a
                // convex arc (towards it for a concave one).
                let radial = g.dir(unit(sub(segs[i].at(0.5), c)));
                oriented_face(
                    wire,
                    Surface::RevolutedCurve(surf),
                    (0.5, SEAM_OFFSET + 0.5 * s.sweep().abs()),
                    g.z + if ccw { radial } else { -radial },
                )?
            }
        };
        faces.push(face);
    }
    let top_loop: Wire = et.iter().flatten().cloned().collect();
    Ok((faces, top_loop))
}

/// Build the blended solid.
fn construct(g: &Geo, p: &Plan, lower: &Lower, h: f64, r: f64, kind: Blend) -> Result<Solid> {
    let segs = &p.segs;
    let n = segs.len();
    let (vb, eb): (Vec<Vertex>, Vec<Edge>) = match lower {
        Lower::Prism { .. } => {
            let vb: Vec<Vertex> = segs
                .iter()
                .map(|s| builder::vertex(g.w(s.a(), -h)))
                .collect();
            let mut eb = Vec::with_capacity(n);
            for (i, s) in segs.iter().enumerate() {
                eb.push(Edge::new(&vb[i], &vb[(i + 1) % n], g.seg_curve(s, -h)?));
            }
            (vb, eb)
        }
        Lower::Keep { .. } => (Vec::new(), Vec::new()),
    };
    let walls = match lower {
        Lower::Prism { .. } => Walls::PerSeg { vb: &vb, eb: &eb },
        Lower::Keep { groups, .. } => Walls::Groups(groups),
    };
    let (mut faces, top_loop) = strip(g, p, walls, h, r, kind)?;
    match lower {
        Lower::Prism { holes } => {
            // Holes: walls from −H to 0.
            let mut hole_top = Vec::new();
            let mut hole_bottom = Vec::new();
            for hs in holes {
                let m = hs.len();
                let hb: Vec<Vertex> = hs.iter().map(|s| builder::vertex(g.w(s.a(), -h))).collect();
                let ht: Vec<Vertex> = hs
                    .iter()
                    .map(|s| builder::vertex(g.w(s.a(), 0.0)))
                    .collect();
                let mut lo = Vec::with_capacity(m);
                let mut hi = Vec::with_capacity(m);
                for (i, s) in hs.iter().enumerate() {
                    let j = (i + 1) % m;
                    lo.push(Edge::new(&hb[i], &hb[j], g.seg_curve(s, -h)?));
                    hi.push(Edge::new(&ht[i], &ht[j], g.seg_curve(s, 0.0)?));
                }
                let up: Vec<Edge> = (0..m).map(|j| builder::line(&hb[j], &ht[j])).collect();
                faces.extend(wall_faces(g, hs, &lo, &hi, &up, -h, 0.0)?);
                hole_top.push(hi.iter().cloned().collect::<Wire>());
                hole_bottom.push(lo.iter().rev().map(|e| e.inverse()).collect::<Wire>());
            }
            // Top face: inset loop (counter-clockwise) and the holes (clockwise).
            let mut top = vec![top_loop];
            top.extend(hole_top);
            faces.push(plane_face(top, g.w([0.0, 0.0], 0.0), g.x, g.y)?);
            // Bottom face, outward −z.
            let mut bottom: Vec<Wire> = vec![eb.iter().rev().map(|e| e.inverse()).collect()];
            bottom.extend(hole_bottom);
            faces.push(plane_face(bottom, g.w([0.0, 0.0], -h), g.y, g.x)?);
        }
        Lower::Keep {
            hole_wires,
            others,
            extra,
            ..
        } => {
            let mut top = vec![top_loop];
            for (q, groups, hq) in extra {
                let (f, w) = strip(g, q, Walls::Groups(groups), *hq, r, kind)?;
                faces.extend(f);
                top.push(w);
            }
            top.extend(hole_wires.iter().cloned());
            faces.push(plane_face(top, g.w([0.0, 0.0], 0.0), g.x, g.y)?);
            faces.extend(others.iter().cloned());
        }
    }
    let shell: Shell = faces.into();
    Solid::try_new(vec![shell])
        .map_err(|e| KernelError::InvalidResult(format!("{PREFIX}solid: {e:?}")))
}

/// Fillet every edge of the outer boundary loop of the planar cap face in
/// `cap`'s plane (outward normal `cap.z`) with radius `r` mm. See the module
/// docs for the scope; refusals are `InvalidInput` with the reason.
pub fn fillet_cap_loop(body: &Body, cap: &Frame, r: f64) -> Result<Body> {
    cap_loop(body, cap, r, Blend::Fillet, false).map_err(|e| named(Blend::Fillet, e))
}

/// [`fillet_cap_loop`] of every edge of the face: the outer loop and the
/// loop of every hole in it (a hole's rim is rounded into the hole, its
/// corners as tori of radius R + r about the hole's arcs). The walls of the
/// holes must run straight down like the outer walls.
pub fn fillet_cap_face_edges(body: &Body, cap: &Frame, r: f64) -> Result<Body> {
    cap_loop(body, cap, r, Blend::Fillet, true).map_err(|e| named(Blend::Fillet, e))
}

/// [`chamfer_cap_loop`] of every edge of the face, the holes' rims included
/// (a countersink-like cone at a round hole).
pub fn chamfer_cap_face_edges(body: &Body, cap: &Frame, d: f64) -> Result<Body> {
    cap_loop(body, cap, d, Blend::Chamfer, true).map_err(|e| named(Blend::Chamfer, e))
}

/// Equal-distance chamfer (`d` mm along the wall and along the cap) of every
/// edge of the outer boundary loop of the planar cap face in `cap`'s plane:
/// 45° planes along straight edges, cones along arcs, straight miter lines
/// at sharp corners. Same scope as [`fillet_cap_loop`]; a convex arc must
/// have a radius larger than `d`.
pub fn chamfer_cap_loop(body: &Body, cap: &Frame, d: f64) -> Result<Body> {
    cap_loop(body, cap, d, Blend::Chamfer, false).map_err(|e| named(Blend::Chamfer, e))
}

/// Volume and error of a body: the exact parametric integral when it
/// applies, else the general measurement.
fn volume_of(body: &Body) -> Result<(f64, f64)> {
    if let Ok(Ok(v)) = guarded("edge loop volume", || {
        crate::measure::parametric_volume(&body.solid, VOLUME_PANELS)
    }) && v.1 <= VOLUME_REL * v.0.abs()
    {
        return Ok(v);
    }
    match crate::measure(body)?.volume {
        crate::VolumeResult::Value {
            volume,
            error_estimate,
            ..
        } => Ok((volume, error_estimate)),
        crate::VolumeResult::Unavailable { reason } => Err(KernelError::InvalidResult(format!(
            "{PREFIX}cannot verify the result volume ({reason})"
        ))),
    }
}

/// The blend region of a general body: the cap face, one wall face per
/// outer-loop edge (ruled along the cap normal, bounded by its cap edge,
/// two vertical lines shared with its neighbours and a far edge that is the
/// cap edge translated to depth `h`, the same for all walls), and everything
/// else kept as it is. Returns the loop, the kept part, the cap face's hole
/// loops (for the clearance checks) and `h`.
/// The outer loop, the kept part, the cap's unblended hole loops, the wall
/// depth, and the blended hole loops (segments, far edges, depth).
type Band = (Vec<Seg>, Lower, Vec<Vec<Seg>>, f64, Vec<HoleBand>);
type HoleBand = (Vec<Seg>, Vec<Group>, f64);

fn band_under_cap(body: &Body, cap: &Frame, include_holes: bool) -> Result<Band> {
    let (o, x, y, z) = cap_frame(cap)?;
    let to2 = |p: Point3| {
        let d = p - o;
        ([d.dot(x), d.dot(y)], d.dot(z))
    };
    if body.solid.boundaries().len() != 1 {
        return Err(bad("the body must be one closed shell (no voids)"));
    }
    let bounds = crate::measure::raw_bounds(&body.solid)?;
    let diag = (0..3)
        .map(|i| (bounds.max[i] - bounds.min[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    let ptol = 1e-9 * diag.max(1.0);
    let faces: Vec<Face> = body.solid.face_iter().cloned().collect();
    let caps: Vec<usize> = faces
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            let Surface::Plane(pl) = f.surface() else {
                return false;
            };
            let n = if f.orientation() {
                pl.normal()
            } else {
                -pl.normal()
            };
            (n - z).magnitude() < tol::FACE_MATCH && (pl.origin() - o).dot(z).abs() <= ptol
        })
        .map(|(i, _)| i)
        .collect();
    if caps.len() != 1 {
        return Err(bad(format!(
            "expected exactly one face in the cap plane, found {}",
            caps.len()
        )));
    }
    let ci = caps[0];
    type LoopEdges = (Vec<Seg>, Vec<Edge>, Vec<usize>);
    let mut outer: Option<LoopEdges> = None;
    let mut holes = Vec::new();
    let mut hole_edges: Vec<LoopEdges> = Vec::new();
    let mut hole_wires = Vec::new();
    for w in faces[ci].boundaries() {
        let mut segs = Vec::new();
        let mut edges = Vec::new();
        let mut counts = Vec::new();
        for e in w.edge_iter() {
            let piece = edge_segments(e, &to2)?;
            edges.push(e.clone());
            counts.push(piece.len());
            segs.extend(piece);
        }
        if loop_area(&segs) > 0.0 {
            if outer.is_some() {
                return Err(bad("the cap face has two outer loops"));
            }
            outer = Some((segs, edges, counts));
        } else {
            holes.push(segs.clone());
            hole_edges.push((segs, edges, counts));
            hole_wires.push(w.clone());
        }
    }
    let (segs, cap_edges, counts) = outer.ok_or_else(|| bad("the cap face has no outer loop"))?;
    let (groups, wall_of, h) =
        walls_of(&faces, ci, &segs, &cap_edges, &counts, &to2, ptol, diag, z)?;
    let mut walls: Vec<usize> = wall_of.clone();
    let mut extra = Vec::new();
    if include_holes {
        for (hs, he, hc) in &hole_edges {
            let (hg, hwall, hh) = walls_of(&faces, ci, hs, he, hc, &to2, ptol, diag, z)?;
            walls.extend(hwall);
            extra.push((hs.clone(), hg, hh));
        }
        holes.clear();
        hole_wires.clear();
    }
    let others: Vec<Face> = faces
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != ci && !walls.contains(i))
        .map(|(_, f)| f.clone())
        .collect();
    Ok((
        segs,
        Lower::Keep {
            groups,
            hole_wires,
            others,
            extra: Vec::new(),
        },
        holes,
        h,
        extra,
    ))
}

/// The wall under each edge of one cap loop (`cap_edges` as the cap face
/// traverses them, edge `i` covering `counts[i]` consecutive segments of
/// `segs`): a four-sided face ruled down from the cap edge along the cap
/// normal, two vertical lines shared with the neighbouring walls, and a far
/// edge that is the cap edge translated to a common depth. Returns the walls
/// (as [`Group`]s), their face indices and the depth.
#[allow(clippy::too_many_arguments)]
fn walls_of(
    faces: &[Face],
    ci: usize,
    segs: &[Seg],
    cap_edges: &[Edge],
    counts: &[usize],
    to2: &dyn Fn(Point3) -> (V2, f64),
    ptol: f64,
    diag: f64,
    z: Vector3,
) -> Result<(Vec<Group>, Vec<usize>, f64)> {
    let n = segs.len();
    let mut wall_of = Vec::with_capacity(cap_edges.len());
    for e in cap_edges {
        let owners: Vec<usize> = faces
            .iter()
            .enumerate()
            .filter(|(i, f)| {
                *i != ci
                    && f.boundaries()
                        .iter()
                        .any(|w| w.edge_iter().any(|x| x.id() == e.id()))
            })
            .map(|(i, _)| i)
            .collect();
        if owners.len() != 1 {
            return Err(bad("a cap edge is not shared by exactly one other face"));
        }
        wall_of.push(owners[0]);
    }
    let mut seen = wall_of.clone();
    seen.sort_unstable();
    seen.dedup();
    if seen.len() != wall_of.len() {
        return Err(bad("one wall face spans several cap edges"));
    }
    let vertical = |e: &Edge| match e.oriented_curve() {
        Curve::Line(Line(a, b)) => {
            let d = b - a;
            d.magnitude() > ptol && d.normalize().cross(z).magnitude() < 1e-9
        }
        _ => false,
    };
    let mut groups: Vec<Group> = Vec::with_capacity(cap_edges.len());
    let mut depth: Option<f64> = None;
    let mut first = 0;
    for (i, &wi) in wall_of.iter().enumerate() {
        let count = counts[i];
        let gsegs = &segs[first..first + count];
        let f = &faces[wi];
        let wires = f.boundaries();
        if wires.len() != 1 || wires[0].len() != 4 {
            return Err(bad(format!(
                "the wall under cap edge {i} is not a four-sided face (cap edge, two vertical \
                 edges, far edge): another feature cuts into it"
            )));
        }
        // The wall surface is ruled along the cap normal (normals ⟂ z on a
        // grid over the face's parameter box around its boundary).
        let surf = f.surface();
        let uvs: Vec<(f64, f64)> = wires[0]
            .edge_iter()
            .flat_map(|e| {
                let c = e.oriented_curve();
                let (t0, t1) = c.range_tuple();
                (0..=4).map(move |k| c.subs(t0 + (t1 - t0) * k as f64 / 4.0))
            })
            .filter_map(|p| surf.search_parameter(p, None, 100))
            .collect();
        if uvs.len() < 8 {
            return Err(bad(format!(
                "the wall under cap edge {i} could not be sampled"
            )));
        }
        let (umin, umax, vmin, vmax) = uvs.iter().fold(
            (
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
            ),
            |(a, b, c, d), &(u, v)| (a.min(u), b.max(u), c.min(v), d.max(v)),
        );
        for a in 0..=4 {
            for b in 0..=4 {
                let nrm = surf.normal(
                    umin + (umax - umin) * a as f64 / 4.0,
                    vmin + (vmax - vmin) * b as f64 / 4.0,
                );
                if nrm.dot(z).abs() > 1e-9 {
                    return Err(bad(format!(
                        "the wall under cap edge {i} is not parallel to the cap normal"
                    )));
                }
            }
        }
        let rest: Vec<Edge> = wires[0]
            .edge_iter()
            .filter(|e| e.id() != cap_edges[i].id())
            .cloned()
            .collect();
        let others: Vec<&Edge> = rest.iter().filter(|e| !vertical(e)).collect();
        if rest.len() != 3 || others.len() != 1 {
            return Err(bad(format!(
                "the wall under cap edge {i} does not run straight down from the cap edge"
            )));
        }
        // Orient the far edge along the loop.
        let (sa, sb) = (gsegs[0].a(), gsegs[count - 1].b());
        let fe = others[0];
        let (a2, _) = to2(fe.front().point());
        let fe = if len(sub(a2, sa)) <= ptol.max(1e-7) {
            fe.clone()
        } else {
            fe.inverse()
        };
        let (fa, fha) = to2(fe.front().point());
        let (fb, fhb) = to2(fe.back().point());
        let hh = -fha;
        if len(sub(fa, sa)) > 1e-7 || len(sub(fb, sb)) > 1e-7 || (fhb - fha).abs() > 1e-7 {
            return Err(bad(format!(
                "the wall under cap edge {i} does not end in a copy of the cap edge"
            )));
        }
        // The far curve is the cap edge translated (samples).
        let c = fe.oriented_curve();
        let (t0, t1) = c.range_tuple();
        for k in 1..16 {
            let (q, hq) = to2(c.subs(t0 + (t1 - t0) * k as f64 / 16.0));
            let on = gsegs
                .iter()
                .map(|s| s.dist(q))
                .fold(f64::INFINITY, f64::min);
            if on > 1e-7 * (1.0 + diag) || (hq + hh).abs() > 1e-7 * (1.0 + diag) {
                return Err(bad(format!(
                    "the wall under cap edge {i} is not a straight wall over the cap edge"
                )));
            }
        }
        match depth {
            None => depth = Some(hh),
            Some(d) if (d - hh).abs() <= 1e-7 * (1.0 + diag) => {}
            Some(_) => {
                return Err(bad(
                    "the walls under the cap loop end at different depths (a step or cut \
                     interrupts them)",
                ));
            }
        }
        groups.push(Group {
            first,
            count,
            far: fe,
            surface: surf,
        });
        first += count;
    }
    if first != n {
        return Err(KernelError::Internal(
            "edge loop: segment count mismatch".into(),
        ));
    }
    // Consecutive far edges share their vertices (the walls share their
    // vertical edges).
    let k = groups.len();
    for i in 0..k {
        if groups[i].far.back().id() != groups[(i + 1) % k].far.front().id() {
            return Err(bad("adjacent walls do not share a vertical edge"));
        }
    }
    Ok((groups, wall_of, depth.unwrap_or(0.0)))
}

/// Refuse when any edge of the kept faces reaches into the blend band
/// (depth below the cap < r and distance from the loop < r).
fn band_is_clear(segs: &[Seg], lower: &Lower, g: &Geo, r: f64) -> Result<()> {
    let Lower::Keep {
        others, hole_wires, ..
    } = lower
    else {
        return Ok(());
    };
    let skip: std::collections::HashSet<_> = hole_wires
        .iter()
        .flat_map(|w| w.edge_iter().map(|e| e.id()))
        .collect();
    let to2 = |p: Point3| {
        let d = p - g.o;
        ([d.dot(g.x), d.dot(g.y)], d.dot(g.z))
    };
    for f in others {
        for w in f.boundaries() {
            for e in w.edge_iter() {
                if skip.contains(&e.id()) {
                    continue;
                }
                let c = e.oriented_curve();
                let (t0, t1) = c.range_tuple();
                let pts: Vec<Point3> = (0..=FIT_SAMPLES)
                    .filter_map(|k| {
                        let t = t0 + (t1 - t0) * k as f64 / FIT_SAMPLES as f64;
                        match &c {
                            Curve::IntersectionCurve(ic) => ic.search_triple(t).map(|x| x.0),
                            other => Some(other.subs(t)),
                        }
                    })
                    .collect();
                let step = pts
                    .windows(2)
                    .map(|p| (p[1] - p[0]).magnitude())
                    .fold(0.0, f64::max);
                for p in pts {
                    let (q, w) = to2(p);
                    let near = segs.iter().map(|s| s.dist(q)).fold(f64::INFINITY, f64::min);
                    if -w < r + step && near < r + step {
                        return Err(bad(format!(
                            "another feature reaches into the blend band at ({:.4}, {:.4}, \
                             {:.4}) (within the {} of both the cap and the loop)",
                            p.x, p.y, p.z, r
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Refuse when two blended loops come within `2 r` of each other (their
/// bands would meet).
fn loops_apart(a: &[Seg], b: &[Seg], r: f64) -> Result<()> {
    for s in a {
        let step = s.length() / FIT_SAMPLES as f64;
        for k in 0..=FIT_SAMPLES {
            let p = s.at(k as f64 / FIT_SAMPLES as f64);
            let d = b.iter().map(|t| t.dist(p)).fold(f64::INFINITY, f64::min);
            if d <= 2.0 * r + 0.5 * step + tol::KERNEL_LINEAR {
                return Err(bad(format!(
                    "two edge loops of the face come within {d:.4} mm of each other at \
                     ({:.4}, {:.4}): their blends of {r} mm would meet",
                    p[0], p[1]
                )));
            }
        }
    }
    Ok(())
}

fn cap_loop(body: &Body, cap: &Frame, r: f64, kind: Blend, include_holes: bool) -> Result<Body> {
    cap.validate()?;
    let size = kind.size();
    if !r.is_finite() || r < tol::MIN_LENGTH {
        return Err(bad(format!(
            "{size} must be >= {} mm, got {r}",
            tol::MIN_LENGTH
        )));
    }
    let (o, x, y, z) = cap_frame(cap)?;
    let g = Geo { o, x, y, z };
    // A straight prism is rebuilt whole (arcs of any sweep, closed-form
    // volume) when only its outer loop is blended; otherwise the blend
    // replaces only the cap face and the walls of the blended loops.
    let prism = guarded("edge loop", || prism_under_cap(body, cap))?;
    let prism = match prism {
        Ok((loops, _)) if include_holes && !loops.holes.is_empty() => {
            Err(bad("holes are blended too"))
        }
        other => other,
    };
    let (segs, mut lower, holes, h, base, extra) = match prism {
        Ok((loops, h)) => {
            let area =
                loop_area(&loops.outer) + loops.holes.iter().map(|h| loop_area(h)).sum::<f64>();
            (
                loops.outer,
                Lower::Prism {
                    holes: loops.holes.clone(),
                },
                loops.holes,
                h,
                (area * h, 0.0),
                Vec::new(),
            )
        }
        Err(prism_err) => {
            match guarded("edge loop", || band_under_cap(body, cap, include_holes))? {
                Ok((segs, lower, holes, h, extra)) => {
                    (segs, lower, holes, h, volume_of(body)?, extra)
                }
                Err(band_err) => {
                    return Err(bad(format!(
                        "{}; {}",
                        prism_err
                            .to_string()
                            .replace(&format!("invalid input: {PREFIX}"), ""),
                        band_err
                            .to_string()
                            .replace(&format!("invalid input: {PREFIX}"), "")
                    )));
                }
            }
        }
    };
    for depth in std::iter::once(h).chain(extra.iter().map(|e| e.2)) {
        if r >= depth - tol::MIN_LENGTH {
            return Err(bad(format!(
                "{size} {r} mm does not fit: the walls are only {depth:.6} mm deep under the \
                 cap face"
            )));
        }
    }
    let p = plan(&segs, &holes, r, kind)?;
    let mut removed = removed_volume(&p.segs, &p.alpha, r, kind);
    let mut all_segs = segs.clone();
    let mut plans = Vec::new();
    for (i, (hs, hg, hh)) in extra.iter().enumerate() {
        loops_apart(&segs, hs, r)?;
        for (other, _, _) in extra.iter().skip(i + 1) {
            loops_apart(hs, other, r)?;
        }
        let q = plan(hs, &holes, r, kind)?;
        removed += removed_volume(&q.segs, &q.alpha, r, kind);
        all_segs.extend(hs.iter().copied());
        plans.push((q, hg.clone(), *hh));
    }
    if let Lower::Keep { extra: e, .. } = &mut lower {
        *e = plans;
    }
    band_is_clear(&all_segs, &lower, &g, r)?;
    let solid = guarded("edge loop", || construct(&g, &p, &lower, h, r, kind))??;
    let out = Body { solid };
    crate::build::check_single_closed(&out, "edge loop")?;
    // Self-check against the closed form, with the exact boundary integral
    // over each face's own parametrisation (measure/parametric.rs) when it
    // applies.
    let want = base.0 - removed;
    let (volume, err) = volume_of(&out)?;
    if (volume - want).abs() > VOLUME_REL * want.abs() + 3.0 * (err + base.1) {
        return Err(KernelError::InvalidResult(format!(
            "{PREFIX}the result's volume {volume:.9} mm^3 differs from the closed form \
             {want:.9} mm^3"
        )));
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
