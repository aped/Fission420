//! Equal-distance chamfer of a straight convex edge between two planar faces.
//!
//! The edge is named by its two faces (planes with outward normals), never by a
//! B-rep index. The chamfer removes the wedge between the edge and the chamfer
//! line A-B (A, B at distance `d` from the edge on each face) by subtracting a
//! prism swept along the edge (past both ends by the through-all margin). Its
//! cross-section is the half-plane beyond A-B, bounded: the chamfer line extended
//! past both faces, closed on the outer side, so the tool meets the body only
//! where the chamfer plane crosses the faces (Truck 0.6 booleans fail on
//! coplanar faces and on tool edges lying in body faces).
use crate::build::{check_single_closed, subtract_prism};
use crate::ops::{coplanar_faces, face_loops_2d, inside_loops, through_all_margin};
use crate::{Body, Frame, KernelError, Profile, Result, guarded};
use truck_modeling::*;

fn v(a: [f64; 3]) -> Vector3 {
    Vector3::new(a[0], a[1], a[2])
}
fn arr(a: Vector3) -> [f64; 3] {
    [a.x, a.y, a.z]
}

/// Is `p` strictly inside a face of `body` lying in `face`'s plane?
fn on_face(body: &Body, face: &Frame, p: Vector3) -> Result<bool> {
    let o = v(face.origin);
    let q = [(p - o).dot(v(face.x)), (p - o).dot(v(face.y))];
    for f in coplanar_faces(body, face) {
        let loops = face_loops_2d(&f, face)
            .ok_or_else(|| KernelError::Internal("face boundary evaluation failed".into()))?;
        if inside_loops(q, &loops) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Chamfer the straight edge where the faces in planes `f1` and `f2` (z = outward
/// normal) meet, `d` mm along each face. Refused when the planes are parallel, no
/// straight edge of `body` lies on their intersection line, the edge is concave,
/// or `d` reaches past either face.
pub fn chamfer_edge(body: &Body, f1: &Frame, f2: &Frame, d: f64) -> Result<Body> {
    f1.validate()?;
    f2.validate()?;
    if !d.is_finite() || d < crate::tol::MIN_LENGTH {
        return Err(KernelError::InvalidInput(format!(
            "chamfer distance must be >= {} mm, got {d}",
            crate::tol::MIN_LENGTH
        )));
    }
    let (n1, n2) = (v(f1.z), v(f2.z));
    let e = n1.cross(n2);
    if e.magnitude() < 1e-9 {
        return Err(KernelError::InvalidInput(
            "chamfer: the two faces are parallel; they share no edge".into(),
        ));
    }
    let e = e.normalize();
    // Point on both planes: P = (h1 (n2 × e) + h2 (e × n1)) / |n1 × n2|^2 with e
    // unnormalized; with e normalized, divide by (n1 × n2)·e instead.
    let (h1, h2) = (n1.dot(v(f1.origin)), n2.dot(v(f2.origin)));
    let p = (n2.cross(e) * h1 + e.cross(n1) * h2) / n1.cross(n2).dot(e);
    let eps = 1e-6;
    let on_line = |q: Point3| {
        let w = Vector3::new(q.x, q.y, q.z) - p;
        (w - e * w.dot(e)).magnitude() <= eps
    };
    let (mut smin, mut smax) = (f64::INFINITY, f64::NEG_INFINITY);
    guarded("chamfer edge search", || {
        for edge in body.solid.edge_iter() {
            let c = edge.oriented_curve();
            let (t0, t1) = c.range_tuple();
            let (a, b, m) = (c.subs(t0), c.subs(t1), c.subs(0.5 * (t0 + t1)));
            if on_line(a) && on_line(b) && on_line(m) {
                for q in [a, b] {
                    let s = (Vector3::new(q.x, q.y, q.z) - p).dot(e);
                    smin = smin.min(s);
                    smax = smax.max(s);
                }
            }
        }
    })?;
    if smin >= smax {
        return Err(KernelError::InvalidInput(
            "chamfer: the two faces do not meet in a straight edge of the body".into(),
        ));
    }
    // In-face directions away from the edge, at the edge midpoint.
    let mid = p + e * (0.5 * (smin + smax));
    let probe = 1e-3f64.min(0.25 * d);
    let mut dirs = Vec::new();
    for (f, n) in [(f1, n1), (f2, n2)] {
        let t = n.cross(e).normalize();
        let fwd = on_face(body, f, mid + t * probe)?;
        let back = on_face(body, f, mid - t * probe)?;
        let t = match (fwd, back) {
            (true, false) => t,
            (false, true) => -t,
            _ => {
                return Err(KernelError::InvalidInput(
                    "chamfer: cannot tell which side of the edge a face lies on (the edge \
                     is interior to a face or the faces are not adjacent)"
                        .into(),
                ));
            }
        };
        dirs.push(t);
    }
    let (t1, t2) = (dirs[0], dirs[1]);
    if t1.dot(n2) >= 0.0 || t2.dot(n1) >= 0.0 {
        return Err(KernelError::InvalidInput(
            "chamfer: the edge is concave (a chamfer would add material); not supported".into(),
        ));
    }
    for (f, t, which) in [(f1, t1, "first"), (f2, t2, "second")] {
        if !on_face(body, f, mid + t * d)? {
            return Err(KernelError::InvalidInput(format!(
                "chamfer: {d} mm reaches past the {which} face"
            )));
        }
    }
    let m = through_all_margin(body)?;
    let frame = Frame {
        origin: arr(p + e * (smin - m)),
        x: arr(t1),
        y: arr(e.cross(t1)),
        z: arr(e),
    };
    let loc = |w: Vector3| [w.dot(t1), w.dot(e.cross(t1))];
    // Chamfer line A-B extended by d past both faces (it leaves the material
    // transversally at A and B), closed on the outer side at distance d: the
    // tool touches the body only where the chamfer plane crosses the two faces.
    let (a, b) = (t1 * d, t2 * d);
    let u = (a - b).normalize();
    let (a2, b2) = (a + u * d, b - u * d);
    let out_dir = (n1 + n2).normalize() * d;
    let mut pts = vec![loc(a2), loc(a2 + out_dir), loc(b2 + out_dir), loc(b2)];
    let area2: f64 = (0..pts.len())
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    if area2 < 0.0 {
        pts.reverse();
    }
    let out = subtract_prism(
        body,
        &Profile::Polygon(pts),
        &frame,
        smax - smin + 2.0 * m,
        "chamfer",
    )?;
    check_single_closed(&out, "chamfer")?;
    // The tool must have removed exactly the edge wedge: triangle (edge, A, B)
    // times the edge length. Anything else (oblique end faces, other features
    // within the tool's reach) is refused rather than silently cut.
    let wedge = 0.5 * t1.cross(t2).magnitude() * d * d * (smax - smin);
    let vol = |b: &Body| match crate::measure(b)?.volume {
        crate::VolumeResult::Value {
            volume,
            error_estimate,
            ..
        } => Ok((volume, error_estimate)),
        crate::VolumeResult::Unavailable { reason } => Err(KernelError::InvalidInput(format!(
            "chamfer: cannot verify the removed volume ({reason})"
        ))),
    };
    let ((v0, e0), (v1, e1)) = (vol(body)?, vol(&out)?);
    let removed = v0 - v1;
    let tol = (1e-7 * v0).max(1e-6) + 3.0 * (e0 + e1);
    if (removed - wedge).abs() > tol {
        return Err(KernelError::InvalidInput(format!(
            "chamfer: would remove {removed:.6} mm^3 instead of the edge wedge {wedge:.6} mm^3 \
             (only straight convex edges whose ends meet faces perpendicular to the edge, with \
             no other features within the chamfer region, are supported)"
        )));
    }
    Ok(out)
}
