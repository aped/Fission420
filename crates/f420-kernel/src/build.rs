//! Solid construction: extrusion and through-all cut.
use crate::{Body, Frame, KernelError, Profile, Result, guarded, measure, tol};
use truck_modeling::*;

fn p3(a: [f64; 3]) -> Point3 {
    Point3::new(a[0], a[1], a[2])
}
fn v3(a: [f64; 3]) -> Vector3 {
    Vector3::new(a[0], a[1], a[2])
}

/// Planar face for a profile, with normal along `frame.z`.
fn profile_face(profile: &Profile, frame: &Frame) -> Result<Face> {
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
        Profile::Circle { center, radius } => {
            let c = frame.to_world(*center);
            let start = frame.to_world([center[0] + radius, center[1]]);
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
    let tool = extrude(profile, &start, hi - lo + 2.0 * margin)?;
    let solid = guarded("cut", || {
        let mut t = tool.solid.clone();
        t.not();
        truck_shapeops::and(&target.solid, &t, tol::BOOLEAN)
    })?
    .ok_or_else(|| KernelError::BooleanFailed("truck_shapeops::and returned None".into()))?;
    let body = Body { solid };
    check_single_closed(&body, "cut")?;
    Ok(body)
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
