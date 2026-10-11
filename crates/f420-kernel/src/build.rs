//! Solid construction: extrusion and through-all cut.
use crate::{
    Body, Frame, KernelError, LoopSegment, Profile, Result, arc_signed_sweep, exact, guarded,
    measure, tol,
};
use truck_modeling::*;

fn p3(a: [f64; 3]) -> Point3 {
    Point3::new(a[0], a[1], a[2])
}
fn v3(a: [f64; 3]) -> Vector3 {
    Vector3::new(a[0], a[1], a[2])
}

/// Planar face for a profile, with normal along `frame.z`.
/// Circle seam (start-vertex) angles tried, in order, when a boolean fails. They give
/// the same exact geometry with a different arc partition; Truck 0.6 booleans are
/// sensitive to where arc vertices fall (see ADR 0001). Deterministic, fixed list.
const SEAM_ANGLES: [f64; 4] = [0.0, 0.41, 1.27, 2.03];
/// Boolean tolerances tried per seam (first is the policy value `tol::BOOLEAN`).
const BOOLEAN_TOLS: [f64; 3] = [tol::BOOLEAN, 0.02, 0.1];

fn profile_face(profile: &Profile, frame: &Frame) -> Result<Face> {
    profile_face_seam(profile, frame, 0.0)
}

fn profile_face_seam(profile: &Profile, frame: &Frame, seam: f64) -> Result<Face> {
    let wire: Wire = match profile {
        Profile::Polygon(pts) => {
            let verts: Vec<Vertex> = pts
                .iter()
                .map(|p| builder::vertex(p3(frame.to_world(*p))))
                .collect();
            let n = verts.len();
            (0..n)
                .map(|i| builder::line(&verts[i], &verts[(i + 1) % n]))
                .collect()
        }
        Profile::Loop(segs) => loop_wire(segs, frame)?,
        Profile::Circle { center, radius } => {
            let c = frame.to_world(*center);
            let start = frame.to_world([
                center[0] + radius * seam.cos(),
                center[1] + radius * seam.sin(),
            ]);
            let v = builder::vertex(p3(start));
            // A full turn (> 2*pi) closes the wire; Truck splits it into arcs.
            builder::rsweep(&v, p3(c), v3(frame.z), Rad(7.0))
        }
    };
    let face = builder::try_attach_plane(&[wire])
        .map_err(|e| KernelError::InvalidInput(format!("cannot attach plane to profile: {e:?}")))?;
    // Make the face normal agree with the frame normal so the sweep is outward.
    let n = face_normal(&face);
    let z = v3(frame.z);
    Ok(if n.dot(z) < 0.0 { face.inverse() } else { face })
}

/// Closed wire of a line/arc loop with shared vertices. Each arc becomes
/// `k = ceil(|sweep| / (pi/2))` edges (see [`arc_spans`]).
fn loop_wire(segs: &[LoopSegment], frame: &Frame) -> Result<Wire> {
    let verts: Vec<Vertex> = segs
        .iter()
        .map(|s| builder::vertex(p3(frame.to_world(s.start()))))
        .collect();
    let n = verts.len();
    let mut wire = Wire::new();
    for (i, seg) in segs.iter().enumerate() {
        let (v0, v1) = (&verts[i], &verts[(i + 1) % n]);
        match seg {
            LoopSegment::Line { .. } => wire.push_back(builder::line(v0, v1)),
            LoopSegment::Arc { start, center, ccw } => {
                let end = segs[(i + 1) % n].start();
                let spans = arc_spans(*start, end, *center, *ccw, frame)?;
                let k = spans.len();
                let mut prev = v0.clone();
                for (j, (curve, joint)) in spans.into_iter().enumerate() {
                    let next = if j + 1 == k {
                        v1.clone()
                    } else {
                        builder::vertex(p3(joint))
                    };
                    wire.push_back(Edge::new(&prev, &next, Curve::NurbsCurve(curve)));
                    prev = next;
                }
            }
        }
    }
    Ok(wire)
}

/// Exact circular arc as `k = ceil(|sweep| / (pi/2))` rational quadratic Bezier
/// spans (one per edge) with weights (1, cos(phi/2), 1), phi = sweep / k. A single
/// span is never used for sweeps >= pi (its middle weight would be <= 0), and spans
/// are separate edges rather than one C0 multi-span curve: OpenCascade's volume
/// integration of a C0 (double-knot) extruded B-spline was observed to be off by
/// 0.2 % (FreeCAD 1.1, half disc), while single-span patches are exact. Returns each
/// span with its end point (world). The first and last control points are exactly
/// the loop vertices. Control points are mapped through the (affine) frame, which is
/// exact for NURBS.
/// Relative slack (in units of quarter turns) when counting arc spans.
const ARC_SPAN_SLACK: f64 = 1e-9;

pub(crate) fn arc_spans(
    start: [f64; 2],
    end: [f64; 2],
    center: [f64; 2],
    ccw: bool,
    frame: &Frame,
) -> Result<Vec<(NurbsCurve<Vector4>, [f64; 3])>> {
    let r = (start[0] - center[0]).hypot(start[1] - center[1]);
    let sweep = arc_signed_sweep(start, end, center, ccw);
    let a0 = (start[1] - center[1]).atan2(start[0] - center[0]);
    // A quarter arc from solved sketch coordinates has a sweep of pi/2 only up
    // to rounding; the slack keeps it one span (a span of pi/2 * (1 + 1e-9) is
    // still far from the pi limit of a positive middle weight).
    let k = ((sweep.abs() / std::f64::consts::FRAC_PI_2 - ARC_SPAN_SLACK).ceil() as usize).max(1);
    let phi = sweep / k as f64;
    let w = (phi / 2.0).cos();
    let on = |t: f64, rr: f64| [center[0] + rr * t.cos(), center[1] + rr * t.sin()];
    let hom = |p: [f64; 2], w: f64| {
        let q = frame.to_world(p);
        Vector4::new(q[0] * w, q[1] * w, q[2] * w, w)
    };
    let mut out = Vec::with_capacity(k);
    let mut p0 = start;
    for j in 0..k {
        let t0 = a0 + phi * j as f64;
        let p2 = if j + 1 == k { end } else { on(t0 + phi, r) };
        let ctrl = vec![
            hom(p0, 1.0),
            hom(on(t0 + phi / 2.0, r / w), w),
            hom(p2, 1.0),
        ];
        let bs = BSplineCurve::try_new(KnotVec::bezier_knot(2), ctrl)
            .map_err(|e| KernelError::Internal(format!("arc curve construction failed: {e:?}")))?;
        out.push((NurbsCurve::new(bs), frame.to_world(p2)));
        p0 = p2;
    }
    Ok(out)
}

fn face_normal(face: &Face) -> Vector3 {
    match face.surface() {
        Surface::Plane(pl) => {
            let n = pl.normal();
            if face.orientation() { n } else { -n }
        }
        _ => Vector3::unit_z(),
    }
}

/// Extrude a closed profile along `frame.z` by `distance` (must be > 0).
pub fn extrude(profile: &Profile, frame: &Frame, distance: f64) -> Result<Body> {
    frame.validate()?;
    profile.validate()?;
    if !distance.is_finite() || distance < tol::MIN_LENGTH {
        return Err(KernelError::InvalidInput(format!(
            "extrusion distance must be >= {} mm, got {distance}",
            tol::MIN_LENGTH
        )));
    }
    let solid = guarded("extrude", || -> Result<Solid> {
        let face = profile_face(profile, frame)?;
        Ok(builder::tsweep(&face, v3(frame.z) * distance))
    })??;
    let body = Body { solid };
    check_single_closed(&body, "extrude")?;
    Ok(body)
}

/// Cut a profile (on `frame`) through all material of `target`, in both directions
/// along the frame normal. The cutter is extended past the body bounds so its caps
/// are never coplanar with body faces.
pub fn cut_through_all(target: &Body, profile: &Profile, frame: &Frame) -> Result<Body> {
    frame.validate()?;
    profile.validate()?;
    let (start, length) = through_all_cutter(target, frame)?;
    subtract_prism(target, profile, &start, length, "cut")
}

/// Start frame and length of a through-all cutter on `frame` for `target`.
fn through_all_cutter(target: &Body, frame: &Frame) -> Result<(Frame, f64)> {
    let m = measure::raw_bounds(&target.solid)?;
    let z = v3(frame.z);
    let o = p3(frame.origin);
    // Range of the body along the frame normal, relative to the frame origin.
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for cx in [m.min[0], m.max[0]] {
        for cy in [m.min[1], m.max[1]] {
            for cz in [m.min[2], m.max[2]] {
                let d = (Point3::new(cx, cy, cz) - o).dot(z);
                lo = lo.min(d);
                hi = hi.max(d);
            }
        }
    }
    let diag = ((m.max[0] - m.min[0]).powi(2)
        + (m.max[1] - m.min[1]).powi(2)
        + (m.max[2] - m.min[2]).powi(2))
    .sqrt();
    let margin = tol::THROUGH_ALL_MARGIN_ABS.max(diag * tol::THROUGH_ALL_MARGIN_REL);
    let start = frame.offset(lo - margin);
    let length = hi - lo + 2.0 * margin;
    Ok((start, length))
}

/// Blind cut (pocket): remove the prism of `profile` from the sketch plane of
/// `frame` to `depth` along `-frame.z` (into the material when `frame` is an
/// outward-facing planar face). The cutter starts a margin *above* the face so
/// its top cap is never coplanar with the face (Truck 0.6 rejects coplanar
/// boolean faces); its floor lies at exactly `depth`. The caller must ensure the
/// floor is strictly inside the body (`depth` less than the local thickness).
pub fn cut_blind(target: &Body, profile: &Profile, frame: &Frame, depth: f64) -> Result<Body> {
    frame.validate()?;
    profile.validate()?;
    if !depth.is_finite() || depth < tol::MIN_LENGTH {
        return Err(KernelError::InvalidInput(format!(
            "pocket depth must be >= {} mm, got {depth}",
            tol::MIN_LENGTH
        )));
    }
    let m = measure::raw_bounds(&target.solid)?;
    let diag = ((m.max[0] - m.min[0]).powi(2)
        + (m.max[1] - m.min[1]).powi(2)
        + (m.max[2] - m.min[2]).powi(2))
    .sqrt();
    let margin = tol::THROUGH_ALL_MARGIN_ABS.max(diag * tol::THROUGH_ALL_MARGIN_REL);
    let start = frame.offset(-depth);
    let length = depth + margin;
    subtract_prism(target, profile, &start, length, "pocket")
}

/// Subtract the prism of `profile` swept from `start` by `length` along `start.z`
/// from `target`, retrying Truck's boolean over [`SEAM_ANGLES`] x [`BOOLEAN_TOLS`]
/// (same exact geometry each time). The first single closed result is returned with
/// its intersection edges made explicit ([`with_exact_edges`]).
fn subtract_prism(
    target: &Body,
    profile: &Profile,
    start: &Frame,
    length: f64,
    what: &str,
) -> Result<Body> {
    let seams: &[f64] = match profile {
        Profile::Circle { .. } => &SEAM_ANGLES,
        Profile::Polygon(_) | Profile::Loop(_) => &SEAM_ANGLES[..1],
    };
    let mut failures = Vec::new();
    let attempts = seams
        .iter()
        .flat_map(|&s| BOOLEAN_TOLS.iter().map(move |&t| (s, t)));
    for (seam, btol) in attempts {
        match subtract_attempt(target, profile, start, length, seam, btol, what) {
            Ok(body) => return Ok(with_exact_edges(body)),
            Err(e) => failures.push(format!("seam {seam}/tol {btol}: {e}")),
        }
    }
    Err(KernelError::BooleanFailed(failures.join("; ")))
}

/// One boolean attempt of [`subtract_prism`] with a given circle seam angle and
/// boolean tolerance; the result is checked to be a single closed shell.
fn subtract_attempt(
    target: &Body,
    profile: &Profile,
    start: &Frame,
    length: f64,
    seam: f64,
    btol: f64,
    what: &str,
) -> Result<Body> {
    let tool = guarded(what, || -> Result<Solid> {
        let face = profile_face_seam(profile, start, seam)?;
        Ok(builder::tsweep(&face, v3(start.z) * length))
    })??;
    let solid = guarded(what, || {
        let mut t = tool;
        t.not();
        truck_shapeops::and(&target.solid, &t, btol)
    })?
    .ok_or_else(|| KernelError::BooleanFailed("truck_shapeops::and returned None".into()))?;
    let body = Body { solid };
    check_single_closed(&body, what)?;
    Ok(body)
}

/// Replace the boolean's implicit intersection edges with verified exact curves where
/// supported (see `crate::exact`). Falls back to the unchanged body if nothing was
/// replaced or the rebuilt solid is not a single closed shell.
fn with_exact_edges(body: Body) -> Body {
    match exact::exactify(&body.solid) {
        (Some(solid), _) => {
            let rebuilt = Body { solid };
            if check_single_closed(&rebuilt, "exact edges").is_ok() {
                rebuilt
            } else {
                body
            }
        }
        (None, _) => body,
    }
}

fn check_single_closed(body: &Body, what: &str) -> Result<()> {
    let shells = body.solid.boundaries();
    if shells.len() != 1 {
        return Err(KernelError::InvalidResult(format!(
            "{what} produced {} shells; exactly one connected solid is required",
            shells.len()
        )));
    }
    let cond = shells[0].shell_condition();
    if cond != ShellCondition::Closed {
        return Err(KernelError::InvalidResult(format!(
            "{what} produced a {cond:?} shell, not Closed"
        )));
    }
    Ok(())
}

/// Smallest x over a line/arc loop: segment endpoints, plus `cx - r` for an arc
/// whose angular span contains the direction pi (-x).
fn loop_min_x(segs: &[LoopSegment]) -> f64 {
    let n = segs.len();
    let mut m = f64::INFINITY;
    for (i, s) in segs.iter().enumerate() {
        let p = s.start();
        m = m.min(p[0]);
        if let LoopSegment::Arc { center, ccw, .. } = s {
            let q = segs[(i + 1) % n].start();
            let r = (p[0] - center[0]).hypot(p[1] - center[1]);
            // Express as a CCW span [a0, a0 + sweep] and test whether it covers pi.
            let (from, sweep) = if *ccw {
                (p, arc_signed_sweep(p, q, *center, true))
            } else {
                (q, -arc_signed_sweep(p, q, *center, false))
            };
            let a0 = (from[1] - center[1]).atan2(from[0] - center[0]);
            let d = (std::f64::consts::PI - a0).rem_euclid(2.0 * std::f64::consts::PI);
            if d <= sweep {
                m = m.min(center[0] - r);
            }
        }
    }
    m
}

/// Revolve a closed profile about the frame's local Y axis (through the frame origin) by
/// `angle` radians, 0 < angle <= 2*pi. The profile must lie strictly on the +X side
/// of the axis (no self-intersection, no degenerate apex).
pub fn revolve(profile: &Profile, frame: &Frame, angle: f64) -> Result<Body> {
    frame.validate()?;
    profile.validate()?;
    if !angle.is_finite() || angle < tol::MIN_ANGLE || angle > 2.0 * std::f64::consts::PI + 1e-12 {
        return Err(KernelError::InvalidInput(format!(
            "revolve angle must be in [{}, 2*pi] rad, got {angle}",
            tol::MIN_ANGLE
        )));
    }
    let min_x = match profile {
        Profile::Polygon(pts) => pts.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min),
        Profile::Circle { center, radius } => center[0] - radius,
        Profile::Loop(segs) => loop_min_x(segs),
    };
    if min_x < tol::MIN_LENGTH {
        return Err(KernelError::InvalidInput(format!(
            "profile must lie strictly on the +X side of the revolve axis (min x = {min_x} mm)"
        )));
    }
    // Truck treats >= 2*pi as a closed sweep; clamp tiny overshoot to exactly 2*pi.
    let angle = angle.min(2.0 * std::f64::consts::PI);
    let solid = guarded("revolve", || -> Result<Solid> {
        let face = profile_face(profile, frame)?;
        Ok(builder::rsweep(
            &face,
            p3(frame.origin),
            v3(frame.y),
            Rad(angle),
        ))
    })??;
    let mut body = Body { solid };
    check_single_closed(&body, "revolve")?;
    // Sweep orientation depends on the profile normal vs. the rotation sense; make the
    // shell outward-oriented (positive enclosed volume).
    if let Ok(m) = measure::measure(&body)
        && let measure::VolumeResult::Value { volume, .. } = m.volume
        && volume < 0.0
    {
        body.solid.not();
    }
    Ok(body)
}

#[cfg(test)]
mod tests;
