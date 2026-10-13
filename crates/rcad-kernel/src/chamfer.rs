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
//!
//! The edge's two END faces may be slanted (planar, not perpendicular to the
//! edge; run 5): the tool is clipped by them, so the removed solid is the
//! triangle T (edge, A, B) swept between the two end planes. Each end plane is
//! s_k(w) = (n_k·(Q_k - P) - n_k·w) / (n_k·e) along the edge, linear in the
//! in-section offset w, so the removed volume is exactly
//! area(T) · (s_1 - s_0)(centroid of T). The result must remove exactly that
//! (anything else: another feature within reach), the ends must be single
//! planar faces not parallel to the edge, and T must lie between them.
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
pub(crate) fn on_face(body: &Body, face: &Frame, p: Vector3) -> Result<bool> {
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
    let ConvexEdge {
        p,
        e,
        n1,
        n2,
        t1,
        t2,
        smin,
        smax,
    } = convex_edge(body, f1, f2, 1e-3f64.min(0.25 * d), "chamfer")?;
    let mid = p + e * (0.5 * (smin + smax));
    for (f, t, which) in [(f1, t1, "first"), (f2, t2, "second")] {
        if !on_face(body, f, mid + t * d)? {
            return Err(KernelError::InvalidInput(format!(
                "chamfer: {d} mm reaches past the {which} face"
            )));
        }
    }
    // End planes at the two edge endpoints (validated before any boolean).
    let end0 = end_plane(body, p + e * smin, n1, n2)?;
    let end1 = end_plane(body, p + e * smax, n1, n2)?;
    // Axial coordinate (from p along e) of end plane (q, n) at offset w.
    let s_at = |(q, n): (Vector3, Vector3), w: Vector3| (n.dot(q - p) - n.dot(w)) / n.dot(e);
    for w in [Vector3::new(0.0, 0.0, 0.0), t1 * d, t2 * d] {
        if s_at(end1, w) - s_at(end0, w) <= crate::tol::MIN_LENGTH {
            return Err(KernelError::InvalidInput(
                "chamfer: the end faces cross within the chamfer (the edge is too short for \
                 this distance at these end angles)"
                    .into(),
            ));
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
    // swept between the end planes, area(T) times the axial length at T's
    // centroid (exact: that length is linear over T). Anything else (other
    // features within the tool's reach) is refused rather than silently cut.
    let centroid = (t1 + t2) * (d / 3.0);
    let wedge =
        0.5 * t1.cross(t2).magnitude() * d * d * (s_at(end1, centroid) - s_at(end0, centroid));
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
             (only straight convex edges ending at single planar faces, with no other \
             features within the chamfer region, are supported)"
        )));
    }
    Ok(out)
}

/// The planar END face of the edge at its endpoint `q`: the one face through
/// `q` other than those in the edge's two planes (normals `n1`, `n2`), as
/// (point, outward unit normal). Refused when it is not a single plane, is
/// curved, or is parallel to the edge.
pub(crate) fn end_plane(
    body: &Body,
    q: Vector3,
    n1: Vector3,
    n2: Vector3,
) -> Result<(Vector3, Vector3)> {
    let e = n1.cross(n2).normalize();
    let at = |pt: Point3| (Vector3::new(pt.x, pt.y, pt.z) - q).magnitude() <= 1e-6;
    let mut planes: Vec<Vector3> = Vec::new();
    for f in body.solid.face_iter() {
        let touches = f
            .boundaries()
            .iter()
            .any(|w| w.vertex_iter().any(|v| at(v.point())));
        if !touches {
            continue;
        }
        let Surface::Plane(pl) = f.surface() else {
            return Err(KernelError::InvalidInput(
                "chamfer: an end of the edge meets a curved face (only planar end faces are \
                 supported)"
                    .into(),
            ));
        };
        let mut n = pl.normal();
        if !f.orientation() {
            n = -n;
        }
        if n.cross(n1).magnitude() < 1e-9 || n.cross(n2).magnitude() < 1e-9 {
            continue; // one of the edge's own faces (or coplanar with one)
        }
        if !planes.iter().any(|m| m.dot(n) > 1.0 - 1e-12) {
            planes.push(n);
        }
    }
    match planes.as_slice() {
        [n] if n.dot(e).abs() > 1e-6 => Ok((q, *n)),
        [_] => Err(KernelError::InvalidInput(
            "chamfer: an end face of the edge is parallel to it".into(),
        )),
        [] => Err(KernelError::InvalidInput(
            "chamfer: no end face found at an end of the edge".into(),
        )),
        _ => Err(KernelError::InvalidInput(format!(
            "chamfer: {} faces meet at an end of the edge (only a single planar end face is \
             supported)",
            planes.len()
        ))),
    }
}

#[cfg(test)]
mod tests;

/// A straight convex edge between two planar faces: a point `p` on it, unit
/// direction `e`, outward face normals `n1`, `n2`, in-face unit directions
/// `t1`, `t2` away from the edge (perpendicular to it), and its extent
/// `[smin, smax]` along `e` from `p`.
pub(crate) struct ConvexEdge {
    pub p: Vector3,
    pub e: Vector3,
    pub n1: Vector3,
    pub n2: Vector3,
    pub t1: Vector3,
    pub t2: Vector3,
    pub smin: f64,
    pub smax: f64,
}

/// The straight edge where the faces in planes `f1` and `f2` (z = outward
/// normal) meet, refused (`what`: the operation's name) when the planes are
/// parallel, no straight edge of `body` lies on their line, the side of a face
/// cannot be told (probing `probe` mm off the edge), or the edge is concave.
pub(crate) fn convex_edge(
    body: &Body,
    f1: &Frame,
    f2: &Frame,
    probe: f64,
    what: &str,
) -> Result<ConvexEdge> {
    let (edge, convex) = straight_edge(body, f1, f2, probe, what)?;
    if !convex {
        return Err(KernelError::InvalidInput(format!(
            "{what}: the edge is concave (it would add material); not supported"
        )));
    }
    Ok(edge)
}

/// As [`convex_edge`], also accepting a concave edge: returns the edge and
/// whether it is convex. For a concave edge `t1`, `t2` still point along the
/// faces away from the edge; the air wedge lies between them.
pub(crate) fn straight_edge(
    body: &Body,
    f1: &Frame,
    f2: &Frame,
    probe: f64,
    what: &str,
) -> Result<(ConvexEdge, bool)> {
    let (n1, n2) = (v(f1.z), v(f2.z));
    let e = n1.cross(n2);
    if e.magnitude() < 1e-9 {
        return Err(KernelError::InvalidInput(format!(
            "{what}: the two faces are parallel; they share no edge"
        )));
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
    let mut spans: Vec<(f64, f64)> = Vec::new();
    guarded("edge search", || {
        for edge in body.solid.edge_iter() {
            let c = edge.oriented_curve();
            let (t0, t1) = c.range_tuple();
            let (a, b, m) = (c.subs(t0), c.subs(t1), c.subs(0.5 * (t0 + t1)));
            if on_line(a) && on_line(b) && on_line(m) {
                let s = |q: Point3| (Vector3::new(q.x, q.y, q.z) - p).dot(e);
                let (sa, sb) = (s(a), s(b));
                spans.push((sa.min(sb), sa.max(sb)));
            }
        }
    })?;
    if spans.is_empty() {
        return Err(KernelError::InvalidInput(format!(
            "{what}: the two faces do not meet in a straight edge of the body"
        )));
    }
    // The edge must be one uninterrupted run (review run 6 #1): collinear
    // edges with a gap between them (e.g. a slot crossing the corner) are
    // separate edges, and the fillet/chamfer tool would bridge the gap.
    spans.sort_by(|x, y| x.0.total_cmp(&y.0));
    let (smin, mut smax) = spans[0];
    for &(a, b) in &spans[1..] {
        if a > smax + eps {
            return Err(KernelError::InvalidInput(format!(
                "{what}: the edge is interrupted between {smax:.6} and {a:.6} mm along it \
                 (several separate collinear edges); only one continuous edge is supported"
            )));
        }
        smax = smax.max(b);
    }
    if smin >= smax {
        return Err(KernelError::InvalidInput(format!(
            "{what}: the two faces do not meet in a straight edge of the body"
        )));
    }
    // In-face directions away from the edge, at the edge midpoint.
    let mid = p + e * (0.5 * (smin + smax));
    let mut dirs = Vec::new();
    for (f, n) in [(f1, n1), (f2, n2)] {
        let t = n.cross(e).normalize();
        let fwd = on_face(body, f, mid + t * probe)?;
        let back = on_face(body, f, mid - t * probe)?;
        let t = match (fwd, back) {
            (true, false) => t,
            (false, true) => -t,
            _ => {
                return Err(KernelError::InvalidInput(format!(
                    "{what}: cannot tell which side of the edge a face lies on (the edge \
                         is interior to a face or the faces are not adjacent)"
                )));
            }
        };
        dirs.push(t);
    }
    let (t1, t2) = (dirs[0], dirs[1]);
    let convex = match (t1.dot(n2) < 0.0, t2.dot(n1) < 0.0) {
        (true, true) => true,
        (false, false) => false,
        _ => {
            return Err(KernelError::InvalidInput(format!(
                "{what}: the faces' sides at the edge are inconsistent (not a simple edge)"
            )));
        }
    };
    let edge = ConvexEdge {
        p,
        e,
        n1,
        n2,
        t1,
        t2,
        smin,
        smax,
    };
    Ok((edge, convex))
}
