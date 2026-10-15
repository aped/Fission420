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
pub(crate) const BOOLEAN_TOLS: [f64; 3] = [tol::BOOLEAN, 0.02, 0.1];

pub(crate) fn profile_face(profile: &Profile, frame: &Frame) -> Result<Face> {
    region_face(profile, &[], frame, 0.0)
}

/// Closed wire of one profile loop (counter-clockwise about `frame.z`).
pub(crate) fn profile_wire(profile: &Profile, frame: &Frame, seam: f64) -> Result<Wire> {
    Ok(match profile {
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
    })
}

/// Planar face bounded by `outer` with inner loops `holes` (all given
/// counter-clockwise; holes are reversed here). The caller guarantees the holes
/// lie strictly inside `outer` and do not touch each other (`rcad_sketch`
/// region extraction checks it). Circle seams (`seam`) apply to the outer loop.
pub(crate) fn region_face(
    outer: &Profile,
    holes: &[Profile],
    frame: &Frame,
    seam: f64,
) -> Result<Face> {
    let mut wires = vec![profile_wire(outer, frame, seam)?];
    for h in holes {
        h.validate()?;
        wires.push(profile_wire(h, frame, 0.0)?.inverse());
    }
    let face = builder::try_attach_plane(&wires)
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
            LoopSegment::Cubic { start, c1, c2, w } => {
                let end = segs[(i + 1) % n].start();
                let curve = crate::cubic::nurbs([*start, *c1, *c2, end], *w, frame)?;
                wire.push_back(Edge::new(v0, v1, Curve::NurbsCurve(curve)));
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
    extrude_region(profile, &[], frame, distance)
}

/// [`extrude`] of the region bounded by `outer` minus the `holes` (one face with
/// inner loops, swept: one closed shell with through-holes).
pub fn extrude_region(
    outer: &Profile,
    holes: &[Profile],
    frame: &Frame,
    distance: f64,
) -> Result<Body> {
    let profile = outer;
    frame.validate()?;
    profile.validate()?;
    if !distance.is_finite() || distance < tol::MIN_LENGTH {
        return Err(KernelError::InvalidInput(format!(
            "extrusion distance must be >= {} mm, got {distance}",
            tol::MIN_LENGTH
        )));
    }
    let solid = guarded("extrude", || -> Result<Solid> {
        let face = region_face(profile, holes, frame, 0.0)?;
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
pub(crate) fn subtract_prism(
    target: &Body,
    profile: &Profile,
    start: &Frame,
    length: f64,
    what: &str,
) -> Result<Body> {
    subtract_region(target, profile, &[], start, length, what, false)
}

/// [`subtract_prism`] of a region with inner loops.
pub(crate) fn subtract_region(
    target: &Body,
    profile: &Profile,
    holes: &[Profile],
    start: &Frame,
    length: f64,
    what: &str,
    pieces: bool,
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
        match subtract_attempt(
            target, profile, holes, start, length, seam, btol, what, pieces,
        ) {
            Ok(body) if pieces => return Ok(with_exact_edges_pieces(body)),
            Ok(body) => return Ok(with_exact_edges(body)),
            Err(e) => failures.push(format!("seam {seam}/tol {btol}: {e}")),
        }
    }
    Err(KernelError::BooleanFailed(failures.join("; ")))
}

/// One boolean attempt of [`subtract_prism`] with a given circle seam angle and
/// boolean tolerance; the result is checked to be a single closed shell.
#[allow(clippy::too_many_arguments)]
fn subtract_attempt(
    target: &Body,
    profile: &Profile,
    holes: &[Profile],
    start: &Frame,
    length: f64,
    seam: f64,
    btol: f64,
    what: &str,
    pieces: bool,
) -> Result<Body> {
    let tool = guarded(what, || -> Result<Solid> {
        let face = region_face(profile, holes, start, seam)?;
        Ok(builder::tsweep(&face, v3(start.z) * length))
    })??;
    let solid = guarded(what, || {
        let mut t = tool;
        t.not();
        truck_shapeops::and(&target.solid, &t, btol)
    })?
    .ok_or_else(|| KernelError::BooleanFailed("truck_shapeops::and returned None".into()))?;
    let body = Body { solid };
    if pieces {
        check_closed_shells(&body, what)?;
    } else {
        check_single_closed(&body, what)?;
    }
    Ok(body)
}

/// [`with_exact_edges`] for a body that may hold several closed shells (one
/// per separate piece): the rebuilt solid is kept only when it has the same
/// number of shells, all closed.
pub(crate) fn with_exact_edges_pieces(body: Body) -> Body {
    match exact::exactify(&body.solid) {
        (Some(solid), _) => {
            let rebuilt = Body { solid };
            if rebuilt.solid.boundaries().len() == body.solid.boundaries().len()
                && check_closed_shells(&rebuilt, "exact edges").is_ok()
            {
                rebuilt
            } else {
                body
            }
        }
        (None, _) => body,
    }
}

/// Every shell of `body` is closed (one or more shells).
pub(crate) fn check_closed_shells(body: &Body, what: &str) -> Result<()> {
    let shells = body.solid.boundaries();
    if shells.is_empty() {
        return Err(KernelError::InvalidResult(format!(
            "{what} produced no shell"
        )));
    }
    for sh in shells {
        let cond = sh.shell_condition();
        if cond != ShellCondition::Closed {
            return Err(KernelError::InvalidResult(format!(
                "{what} produced a {cond:?} shell, not Closed"
            )));
        }
    }
    Ok(())
}

/// Replace the boolean's implicit intersection edges with verified exact curves where
/// supported (see `crate::exact`). Falls back to the unchanged body if nothing was
/// replaced or the rebuilt solid is not a single closed shell.
pub(crate) fn with_exact_edges(body: Body) -> Body {
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

pub(crate) fn check_single_closed(body: &Body, what: &str) -> Result<()> {
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

/// Smallest x over a loop: segment endpoints, plus `cx - r` for an arc
/// whose angular span contains the direction pi (-x), and the control-point
/// minimum of a cubic span (a lower bound, so the revolve check is conservative).
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
        if let LoopSegment::Cubic {
            start,
            c1,
            c2,
            w: _,
        } = s
        {
            // Convex-hull bound: never above the true minimum (conservative).
            let q = segs[(i + 1) % n].start();
            m = m.min(crate::cubic::hull_range([*start, *c1, *c2, q], [1.0, 0.0]).0);
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
    if profile.has_cubic() {
        // Truck builds the revolved rational cubic faces, but its mesher fails
        // on many of them (size- and chord-dependent), so such a body could
        // not be displayed or exported: refuse instead of returning it.
        return Err(KernelError::InvalidInput(
            "revolving ellipse or spline profiles is not supported (their revolved faces \
             cannot be tessellated reliably for display and export); extrude them, or revolve a \
             profile of lines and arcs"
                .into(),
        ));
    }
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
    if min_x <= -tol::KERNEL_LINEAR {
        return Err(KernelError::InvalidInput(format!(
            "profile must lie on the +X side of the revolve axis (min x = {min_x} mm)"
        )));
    }
    if min_x < tol::MIN_LENGTH {
        return revolve_touching_axis(profile, frame, angle);
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

/// Full revolve of a profile that touches the axis along one straight edge
/// (a cup, knob or spindle section drawn against the axis). Truck's face
/// sweep would keep the axis edge as zero-area faces that other readers
/// reject, so the solid is built without them: the profile wire minus the
/// axis edge is swept (one revolved face per edge), and the two circles left
/// open where the wire leaves the axis are closed by planar disks. Supported
/// when the edges next to the axis edge are lines perpendicular to the axis
/// (their sweeps are exactly those disks) and no other point of the profile
/// lies on the axis; a full turn only. Otherwise refused by name (a profile
/// meeting the axis at a point or at an angle makes a cone tip or pole).
fn revolve_touching_axis(profile: &Profile, frame: &Frame, angle: f64) -> Result<Body> {
    let refuse = |m: &str| {
        Err(KernelError::InvalidInput(format!(
            "revolve: the profile touches the axis; {m}"
        )))
    };
    if (angle - 2.0 * std::f64::consts::PI).abs() > 1e-9 {
        return refuse("only a full 360° revolve is supported when the profile touches the axis");
    }
    let wire = profile_wire(profile, frame, 0.0)?;
    let o = p3(frame.origin);
    let ax = v3(frame.y).normalize();
    let dist = |p: Point3| {
        let d = p - o;
        (d - ax * d.dot(ax)).magnitude()
    };
    let on_axis = |p: Point3| dist(p) <= tol::KERNEL_LINEAR;
    let n = wire.len();
    let axis_edges: Vec<usize> = (0..n)
        .filter(|&i| {
            let e = &wire[i];
            let mid = e.oriented_curve().subs(
                0.5 * (e.oriented_curve().range_tuple().0 + e.oriented_curve().range_tuple().1),
            );
            on_axis(e.front().point()) && on_axis(e.back().point()) && on_axis(mid)
        })
        .collect();
    let [k] = axis_edges[..] else {
        return refuse(
            "it must touch the axis along exactly one straight edge with flat ends (a profile \
             meeting the axis at a point — a dome, hemisphere, bowl or cone tip — makes a pole, \
             not supported yet; leave a flat of at least the minimum length on the axis)",
        );
    };
    if n < 3 {
        return refuse(
            "the section needs flat ends: lines perpendicular to the axis at both ends of the \
             axis edge (a dome or hemisphere meets the axis at a pole, not supported yet)",
        );
    }
    let order: Vec<usize> = (1..n).map(|j| (k + j) % n).collect();
    let (first, last) = (order[0], order[n - 2]);
    // The rest of the section must stay at least MIN_LENGTH away from the
    // axis (no point-pinched or sub-MIN_LENGTH neck): exact at the ends,
    // dense samples in between (lines are exact at their ends; arcs and
    // spline spans are sampled finely enough for their curvature at part
    // scale, and the final closed-shell check still applies).
    for &i in &order[1..n - 2] {
        let c = wire[i].oriented_curve();
        let (t0, t1) = c.range_tuple();
        let near = (0..=256).any(|j| {
            let p = c.subs(t0 + (t1 - t0) * f64::from(j) / 256.0);
            dist(p) < tol::MIN_LENGTH
        });
        if near {
            return refuse(&format!(
                "no other part of the profile may come within {} mm of the axis",
                tol::MIN_LENGTH
            ));
        }
    }
    for i in [first, last] {
        let e = &wire[i];
        let (a, b) = (e.front().point(), e.back().point());
        let d = b - a;
        let c = e.oriented_curve();
        let (t0, t1) = c.range_tuple();
        let mid = c.subs(0.5 * (t0 + t1));
        let straight = ((mid - a).cross(d)).magnitude() <= tol::KERNEL_LINEAR * d.magnitude();
        if !straight || d.normalize().dot(ax).abs() > 1e-9 {
            return refuse(
                "the edges leaving the axis must be lines perpendicular to it (a flat end); an \
                 edge meeting the axis at an angle or as an arc makes a cone tip or pole (cone, \
                 dome, bowl), not supported yet",
            );
        }
    }
    if n < 4 {
        return refuse(
            "the profile needs at least one edge besides the axis edge and its neighbours",
        );
    }
    let middle: Wire = order[1..n - 2].iter().map(|&i| wire[i].clone()).collect();
    let solid = guarded("revolve", || -> Result<Solid> {
        let side: Shell = builder::rsweep(&middle, o, ax, Rad(angle));
        let rims = side.extract_boundaries();
        if rims.len() != 2 {
            return Err(KernelError::Internal(format!(
                "revolve: the swept side has {} boundary loops, expected 2",
                rims.len()
            )));
        }
        let disks: Vec<Face> = rims
            .iter()
            .map(|w| {
                builder::try_attach_plane(std::slice::from_ref(&w.inverse()))
                    .map_err(|e| KernelError::Internal(format!("revolve: end disk: {e:?}")))
            })
            .collect::<Result<_>>()?;
        let mut faces: Vec<Face> = side.face_iter().cloned().collect();
        faces.extend(disks);
        let shell: Shell = faces.into();
        Solid::try_new(vec![shell])
            .map_err(|e| KernelError::Internal(format!("revolve: closed shell: {e:?}")))
    })??;
    let mut body = Body { solid };
    check_single_closed(&body, "revolve")?;
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

/// Subtract an arbitrary solid `tool` from `target` (tolerances retried), the
/// result checked to be one closed shell. Test-only (the countersink spike,
/// `validate::tests::spike_countersink_cone`, which with the truck-shapeops
/// fork cuts the revolved cone exactly); product paths use the checked prism
/// booleans or direct constructions.
#[cfg(test)]
pub(crate) fn subtract_solid(target: &Body, tool: &Body, what: &str) -> Result<Body> {
    let mut failures = Vec::new();
    for &btol in &BOOLEAN_TOLS {
        let r = guarded(what, || {
            let mut t = tool.solid.clone();
            t.not();
            truck_shapeops::and(&target.solid, &t, btol)
        })?;
        match r {
            Some(solid) => {
                let body = Body { solid };
                match check_single_closed(&body, what) {
                    Ok(()) => return Ok(with_exact_edges(body)),
                    Err(e) => failures.push(format!("tol {btol}: {e}")),
                }
            }
            None => failures.push(format!("tol {btol}: truck_shapeops::and returned None")),
        }
    }
    Err(KernelError::BooleanFailed(failures.join("; ")))
}

#[cfg(test)]
pub(crate) fn subtract_solid_for_test(target: &Body, tool: &Body) -> Result<Body> {
    subtract_solid(target, tool, "test")
}
