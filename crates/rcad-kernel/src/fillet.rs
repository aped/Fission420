//! Exact constant-radius fillet of a straight convex edge between two planar
//! faces (run 5, SESSION_5 §6/§9.7).
//!
//! The edge is named by its two faces exactly as for [`crate::chamfer_edge`].
//! In the plane perpendicular to the edge, with interior angle θ between the
//! faces, the fillet arc of radius r touches each face at the tangent distance
//! t = r·cot(θ/2) from the edge; the removed corner region is bounded by the
//! two faces and that arc, of area t·r − (π − θ)·r²/2. It is swept along the
//! edge (past both ends by the through-all margin) and cut with the exact slab
//! construction (ADR 0006/0007), so the new face is an exact cylinder of
//! radius r tangent to both faces. Scope (refused by name otherwise): the body
//! must be prismatic along the edge (its end faces perpendicular to it, all
//! other faces parallel), the edge convex, the tangent points inside both
//! faces, and the cut must remove exactly the corner region along the edge
//! (no other feature within reach). The result also passes the independent
//! result check (ADR 0005).
use crate::chamfer::{ConvexEdge, convex_edge, on_face};
use crate::ops::through_all_margin;
use crate::prismatic::{SlabOp, not_applicable, prism_boolean};
use crate::{Body, Frame, KernelError, LoopSegment, Prism, PrismOp, Profile, Result};
use truck_modeling::*;

fn arr(a: Vector3) -> [f64; 3] {
    [a.x, a.y, a.z]
}

/// Fillet the straight edge where the faces in planes `f1` and `f2` (z =
/// outward normal) meet with radius `r` mm. Same frame conventions as
/// [`crate::chamfer_edge`].
pub fn fillet_edge(body: &Body, f1: &Frame, f2: &Frame, r: f64) -> Result<Body> {
    f1.validate()?;
    f2.validate()?;
    if !r.is_finite() || r < crate::tol::MIN_LENGTH {
        return Err(KernelError::InvalidInput(format!(
            "fillet radius must be >= {} mm, got {r}",
            crate::tol::MIN_LENGTH
        )));
    }
    let ConvexEdge {
        p,
        e,
        t1,
        t2,
        smin,
        smax,
        ..
    } = convex_edge(body, f1, f2, 1e-3f64.min(0.25 * r), "fillet")?;
    // Interior angle between the faces and the tangent distance.
    let theta = t1.dot(t2).clamp(-1.0, 1.0).acos();
    if theta < 1e-6 || theta > std::f64::consts::PI - 1e-6 {
        return Err(KernelError::InvalidInput(
            "fillet: the faces are (nearly) parallel at the edge".into(),
        ));
    }
    let t = r / (theta / 2.0).tan();
    let mid = p + e * (0.5 * (smin + smax));
    for (f, tv, which) in [(f1, t1, "first"), (f2, t2, "second")] {
        if !on_face(body, f, mid + tv * t)? {
            return Err(KernelError::InvalidInput(format!(
                "fillet: radius {r} mm does not fit: its tangent point is {t:.6} mm from the \
                 edge, past the {which} face"
            )));
        }
    }
    // Section frame: x = t1, y = e × t1, normal e; the tool spans the edge
    // plus a margin at both ends.
    let m = through_all_margin(body)?;
    let frame = Frame {
        origin: arr(p + e * (smin - m)),
        x: arr(t1),
        y: arr(e.cross(t1)),
        z: arr(e),
    };
    let loc = |w: Vector3| [w.dot(t1), w.dot(e.cross(t1))];
    let (c, a, b) = ([0.0, 0.0], loc(t1 * t), loc(t2 * t));
    let o = loc((t1 + t2).normalize() * (r / (theta / 2.0).sin()));
    // Corner region C -> A -> arc (about O, the short way) -> B -> C,
    // counter-clockwise (reversed when the triangle C, A, B is clockwise).
    let cross = |u: [f64; 2], v: [f64; 2]| u[0] * v[1] - u[1] * v[0];
    let arc_ccw = |from: [f64; 2], to: [f64; 2]| {
        cross(
            [from[0] - o[0], from[1] - o[1]],
            [to[0] - o[0], to[1] - o[1]],
        ) > 0.0
    };
    let (first, second) = if cross(a, b) > 0.0 { (a, b) } else { (b, a) };
    let profile = Profile::Loop(vec![
        LoopSegment::Line { start: c },
        LoopSegment::Arc {
            start: first,
            center: o,
            ccw: arc_ccw(first, second),
        },
        LoopSegment::Line { start: second },
    ]);
    if let Some(why) = not_applicable(body, &profile, &frame) {
        return Err(KernelError::InvalidInput(format!(
            "fillet: the body is not prismatic along the edge ({why}); only edges whose \
             ends meet faces perpendicular to them, on bodies with no other faces across \
             the edge direction, are supported"
        )));
    }
    let len = smax - smin + 2.0 * m;
    let out = prism_boolean(body, &profile, &frame, len, SlabOp::Cut)
        .map_err(|x| KernelError::InvalidInput(format!("fillet: {x}")))?;
    // Exactly the corner region along the edge must have gone.
    let area = t * r - (std::f64::consts::PI - theta) * r * r / 2.0;
    let want = area * (smax - smin);
    let vol = |b: &Body| match crate::measure(b)?.volume {
        crate::VolumeResult::Value {
            volume,
            error_estimate,
            ..
        } => Ok((volume, error_estimate)),
        crate::VolumeResult::Unavailable { reason } => Err(KernelError::InvalidInput(format!(
            "fillet: cannot verify the removed volume ({reason})"
        ))),
    };
    let ((v0, e0), (v1, e1)) = (vol(body)?, vol(&out)?);
    let removed = v0 - v1;
    let tol = (1e-7 * v0).max(1e-6) + 3.0 * (e0 + e1);
    if (removed - want).abs() > tol {
        return Err(KernelError::InvalidInput(format!(
            "fillet: would remove {removed:.6} mm^3 instead of the edge corner {want:.6} mm^3 \
             (another feature lies within the fillet's reach)"
        )));
    }
    crate::verify_prism_boolean(
        body,
        &out,
        PrismOp::Cut,
        &Prism {
            profile: &profile,
            holes: &[],
            start: &frame,
            length: len,
        },
        (0.0, len),
    )
    .map_err(|x| KernelError::InvalidResult(format!("fillet: {x}")))?;
    Ok(out)
}

#[cfg(test)]
mod tests;
