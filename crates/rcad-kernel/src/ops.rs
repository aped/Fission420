//! General extrusion operations on explicit target bodies (ADR 0003): join and
//! cut with an arbitrary closed profile prism, plus the plane/face queries the
//! document needs to place tools without coplanar boolean faces.
use crate::build::{BOOLEAN_TOLS, check_single_closed, subtract_region, with_exact_edges};
use crate::measure::raw_bounds;
use crate::{Body, Bounds, Frame, KernelError, Profile, Result, extrude_region, guarded, tol};
use truck_modeling::*;

/// Axis-aligned bounds overlap with slack `eps` (touching counts).
fn overlaps(a: &Bounds, b: &Bounds, eps: f64) -> bool {
    (0..3).all(|i| a.min[i] <= b.max[i] + eps && b.min[i] <= a.max[i] + eps)
}

/// Union of `target` and the prism of `profile` swept from `start` by `length`
/// along `start.z`. Refuses tools that do not reach the target and results that
/// are not one closed shell (e.g. disconnected solids).
pub fn join_prism(target: &Body, profile: &Profile, start: &Frame, length: f64) -> Result<Body> {
    join_region(target, profile, &[], start, length)
}

/// [`join_prism`] of a region with inner loops (`holes`, counter-clockwise).
/// Truck 0.6 panicked on such tools; our truck-shapeops fork builds them.
pub fn join_region(
    target: &Body,
    profile: &Profile,
    holes: &[Profile],
    start: &Frame,
    length: f64,
) -> Result<Body> {
    let tool = extrude_region(profile, holes, start, length)?;
    let (tb, ob) = (raw_bounds(&target.solid)?, raw_bounds(&tool.solid)?);
    if !overlaps(&tb, &ob, tol::KERNEL_LINEAR) {
        return Err(KernelError::InvalidInput(
            "join: the extrusion does not touch the target body".into(),
        ));
    }
    let mut failures = Vec::new();
    for &btol in &BOOLEAN_TOLS {
        let r = guarded("join", || {
            truck_shapeops::or(&target.solid, &tool.solid, btol)
        })?;
        let Some(solid) = r else {
            failures.push(format!("tol {btol}: truck_shapeops::or returned None"));
            continue;
        };
        let body = Body { solid };
        let shells = body.solid.boundaries().len();
        if shells > 1 {
            // A deterministic geometric outcome, not a numerical failure: retrying
            // other tolerances would not connect the solids.
            return Err(KernelError::InvalidResult(format!(
                "join: the result would be {shells} disconnected solids; \
                 the extrusion must overlap or touch the target over an area"
            )));
        }
        match check_single_closed(&body, "join") {
            Ok(()) => return Ok(with_exact_edges(body)),
            Err(e) => failures.push(format!("tol {btol}: {e}")),
        }
    }
    Err(KernelError::BooleanFailed(failures.join("; ")))
}

/// Subtract the prism of `profile` swept from `start` by `length` from `target`.
/// Refuses tools whose bounds miss the target and cuts that remove nothing.
pub fn cut_prism(target: &Body, profile: &Profile, start: &Frame, length: f64) -> Result<Body> {
    cut_region(target, profile, &[], start, length)
}

/// [`cut_prism`] of a region with inner loops (`holes`, counter-clockwise).
/// Truck 0.6 panicked on such tools; our truck-shapeops fork builds them.
pub fn cut_region(
    target: &Body,
    profile: &Profile,
    holes: &[Profile],
    start: &Frame,
    length: f64,
) -> Result<Body> {
    let tool = extrude_region(profile, holes, start, length)?;
    let (tb, ob) = (raw_bounds(&target.solid)?, raw_bounds(&tool.solid)?);
    if !overlaps(&tb, &ob, -tol::KERNEL_LINEAR) {
        return Err(KernelError::InvalidInput(
            "cut: the extrusion does not intersect the target body".into(),
        ));
    }
    let out = subtract_region(target, profile, holes, start, length, "cut", false)?;
    if same_shape(target, &out) {
        return Err(KernelError::InvalidInput(
            "cut: the extrusion removes no material from the target body".into(),
        ));
    }
    Ok(out)
}

/// [`cut_region`] that accepts a result falling apart into several separate
/// solids (one closed shell each; see [`crate::split_pieces`]). A result with
/// a single shell is returned as is.
pub fn cut_region_pieces(
    target: &Body,
    profile: &Profile,
    holes: &[Profile],
    start: &Frame,
    length: f64,
) -> Result<Body> {
    let tool = extrude_region(profile, holes, start, length)?;
    let (tb, ob) = (raw_bounds(&target.solid)?, raw_bounds(&tool.solid)?);
    if !overlaps(&tb, &ob, -tol::KERNEL_LINEAR) {
        return Err(KernelError::InvalidInput(
            "cut: the extrusion does not intersect the target body".into(),
        ));
    }
    let out = subtract_region(target, profile, holes, start, length, "cut", true)?;
    if out.solid.boundaries().len() == 1 && same_shape(target, &out) {
        return Err(KernelError::InvalidInput(
            "cut: the extrusion removes no material from the target body".into(),
        ));
    }
    Ok(out)
}

/// Same face count and the same vertex positions (within the kernel tolerance):
/// a boolean that changed nothing.
fn same_shape(a: &Body, b: &Body) -> bool {
    if a.solid.face_iter().count() != b.solid.face_iter().count() {
        return false;
    }
    let pts = |x: &Body| {
        let mut v: Vec<[f64; 3]> = x
            .solid
            .vertex_iter()
            .map(|v| {
                let p = v.point();
                [p.x, p.y, p.z]
            })
            .collect();
        v.sort_by(|p, q| p.partial_cmp(q).unwrap_or(std::cmp::Ordering::Equal));
        v.dedup_by(|p, q| (0..3).all(|i| (p[i] - q[i]).abs() <= tol::KERNEL_LINEAR));
        v
    };
    let (pa, pb) = (pts(a), pts(b));
    pa.len() == pb.len()
        && pa
            .iter()
            .zip(&pb)
            .all(|(p, q)| (0..3).all(|i| (p[i] - q[i]).abs() <= 10.0 * tol::KERNEL_LINEAR))
}

/// Planar faces of `body` lying in `frame`'s plane, with either normal direction.
pub(crate) fn coplanar_faces(body: &Body, frame: &Frame) -> Vec<Face> {
    let z = Vector3::new(frame.z[0], frame.z[1], frame.z[2]);
    let o = Point3::new(frame.origin[0], frame.origin[1], frame.origin[2]);
    body.solid
        .face_iter()
        .filter(|f| match f.surface() {
            Surface::Plane(pl) => {
                pl.normal().cross(z).magnitude() < tol::FACE_MATCH
                    && (pl.origin() - o).dot(z).abs() < tol::FACE_MATCH
            }
            _ => false,
        })
        .cloned()
        .collect()
}

/// Whether `frame`'s plane contains a planar face of `body` (either orientation).
/// A tool starting on such a plane would create coplanar boolean faces.
pub fn plane_has_face(body: &Body, frame: &Frame) -> bool {
    !coplanar_faces(body, frame).is_empty()
}

/// Outward-normal side of the planar faces of `body` lying in `frame`'s plane:
/// `Some(1.0)` when they all face along `frame.z`, `Some(-1.0)` when they all face
/// against it, `None` when the plane holds no face. Faces of both orientations in
/// one plane are refused (the material side would be ambiguous).
pub fn coplanar_face_side(body: &Body, frame: &Frame) -> Result<Option<f64>> {
    let z = Vector3::new(frame.z[0], frame.z[1], frame.z[2]);
    let (mut pos, mut neg) = (false, false);
    for f in coplanar_faces(body, frame) {
        let Surface::Plane(pl) = f.surface() else {
            continue;
        };
        let mut n = pl.normal();
        if !f.orientation() {
            n = -n;
        }
        if n.dot(z) > 0.0 {
            pos = true;
        } else {
            neg = true;
        }
    }
    match (pos, neg) {
        (false, false) => Ok(None),
        (true, false) => Ok(Some(1.0)),
        (false, true) => Ok(Some(-1.0)),
        (true, true) => Err(KernelError::InvalidInput(
            "the sketch plane holds faces of the body facing both ways; \
             the material side is ambiguous"
                .into(),
        )),
    }
}

/// Chord count per edge when polygonizing face boundaries / profiles for the
/// containment test.
pub(crate) const REGION_SAMPLES: usize = 64;

/// Polygonized boundary loops of a planar face in `frame`'s 2D coordinates.
pub(crate) fn face_loops_2d(face: &Face, frame: &Frame) -> Option<Vec<Vec<[f64; 2]>>> {
    let o = Point3::new(frame.origin[0], frame.origin[1], frame.origin[2]);
    let x = Vector3::new(frame.x[0], frame.x[1], frame.x[2]);
    let y = Vector3::new(frame.y[0], frame.y[1], frame.y[2]);
    let mut loops = Vec::new();
    for w in face.boundaries() {
        let mut pts = Vec::new();
        for e in w.edge_iter() {
            let c = e.oriented_curve();
            let (t0, t1) = c.range_tuple();
            let n = if matches!(c, Curve::Line(_)) {
                1
            } else {
                REGION_SAMPLES
            };
            for i in 0..n {
                let t = t0 + (t1 - t0) * i as f64 / n as f64;
                let p = match &c {
                    Curve::IntersectionCurve(ic) => ic.search_triple(t)?.0,
                    other => other.subs(t),
                };
                pts.push([(p - o).dot(x), (p - o).dot(y)]);
            }
        }
        loops.push(pts);
    }
    Some(loops)
}

/// Even-odd point-in-polygon over several loops (outer + holes).
pub(crate) fn inside_loops(p: [f64; 2], loops: &[Vec<[f64; 2]>]) -> bool {
    let mut inside = false;
    for l in loops {
        let n = l.len();
        for i in 0..n {
            let (a, b) = (l[i], l[(i + 1) % n]);
            if (a[1] > p[1]) != (b[1] > p[1]) {
                let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                if p[0] < x {
                    inside = !inside;
                }
            }
        }
    }
    inside
}

pub(crate) fn dist_to_loops(p: [f64; 2], loops: &[Vec<[f64; 2]>]) -> f64 {
    let mut d = f64::INFINITY;
    for l in loops {
        let n = l.len();
        for i in 0..n {
            let (a, b) = (l[i], l[(i + 1) % n]);
            let e = [b[0] - a[0], b[1] - a[1]];
            let l2 = e[0] * e[0] + e[1] * e[1];
            let t = if l2 > 0.0 {
                (((p[0] - a[0]) * e[0] + (p[1] - a[1]) * e[1]) / l2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            d = d.min((p[0] - a[0] - t * e[0]).hypot(p[1] - a[1] - t * e[1]));
        }
    }
    d
}

/// Polygonized profile boundary in its own 2D coordinates.
pub(crate) fn profile_points(profile: &Profile) -> Vec<[f64; 2]> {
    match profile {
        Profile::Polygon(v) => {
            let n = v.len();
            (0..n)
                .flat_map(|i| {
                    let (a, b) = (v[i], v[(i + 1) % n]);
                    (0..8).map(move |k| {
                        let t = k as f64 / 8.0;
                        [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]
                    })
                })
                .collect()
        }
        Profile::Circle { center, radius } => (0..REGION_SAMPLES)
            .map(|i| {
                let t = std::f64::consts::TAU * i as f64 / REGION_SAMPLES as f64;
                [center[0] + radius * t.cos(), center[1] + radius * t.sin()]
            })
            .collect(),
        Profile::Loop(segs) => {
            let n = segs.len();
            let mut out = Vec::new();
            for (i, s) in segs.iter().enumerate() {
                let (p, q) = (s.start(), segs[(i + 1) % n].start());
                match s {
                    crate::LoopSegment::Line { .. } => {
                        for k in 0..8 {
                            let t = k as f64 / 8.0;
                            out.push([p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])]);
                        }
                    }
                    crate::LoopSegment::Arc { center, ccw, .. } => {
                        let r = (p[0] - center[0]).hypot(p[1] - center[1]);
                        let sweep = crate::arc_signed_sweep(p, q, *center, *ccw);
                        let a0 = (p[1] - center[1]).atan2(p[0] - center[0]);
                        for k in 0..REGION_SAMPLES {
                            let t = a0 + sweep * k as f64 / REGION_SAMPLES as f64;
                            out.push([center[0] + r * t.cos(), center[1] + r * t.sin()]);
                        }
                    }
                    crate::LoopSegment::Cubic { c1, c2, w, .. } => {
                        out.extend(crate::cubic::samples([p, *c1, *c2, q], *w, REGION_SAMPLES));
                    }
                }
            }
            out
        }
    }
}

/// Whether the closed `profile` (in `frame` coordinates) lies inside the region of
/// the planar face(s) of `body` in `frame`'s plane: every sampled profile boundary
/// point is inside or within `slack` of a face region, and no sampled face
/// boundary point lies strictly inside the profile (a hole or notch under it).
/// Sampling: lines at their vertices (+ 8 per edge for profiles), curves at 64
/// chords per edge; `slack` absorbs chord error and is
/// `max(10 * KERNEL_LINEAR, 1e-6 * extent)`.
pub fn profile_within_face(body: &Body, frame: &Frame, profile: &Profile) -> Result<bool> {
    guarded("face region", || -> Result<bool> {
        let faces = coplanar_faces(body, frame);
        if faces.is_empty() {
            return Ok(false);
        }
        let mut regions = Vec::new();
        for f in &faces {
            regions.push(
                face_loops_2d(f, frame).ok_or_else(|| {
                    KernelError::Internal("face boundary evaluation failed".into())
                })?,
            );
        }
        let pts = profile_points(profile);
        let extent = pts
            .iter()
            .flat_map(|p| p.iter())
            .fold(0.0f64, |m, v| m.max(v.abs()));
        let slack = (10.0 * tol::KERNEL_LINEAR).max(1e-6 * extent);
        let in_region = |p: [f64; 2]| {
            regions
                .iter()
                .any(|r| inside_loops(p, r) || dist_to_loops(p, r) <= slack)
        };
        if !pts.iter().all(|&p| in_region(p)) {
            return Ok(false);
        }
        let prof = vec![pts.clone()];
        for r in &regions {
            for l in r {
                for &q in l {
                    if inside_loops(q, &prof) && dist_to_loops(q, &prof) > slack {
                        return Ok(false);
                    }
                }
            }
        }
        // Exact area test (2-D boolean on the integer grid): sampling misses a
        // notch of the face whose edges run along the profile's (observed: a
        // profile along the face's edge across a boss footprint and a slot).
        let n = pts.len();
        let perimeter: f64 = (0..n)
            .map(|i| {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                (b[0] - a[0]).hypot(b[1] - a[1])
            })
            .sum();
        Ok(crate::prismatic::area_outside(&pts, &regions)? <= slack * perimeter)
    })?
}

/// Exact bounds of `body` (same method as [`crate::measure`]).
pub fn bounds(body: &Body) -> Result<Bounds> {
    guarded("bounds", || raw_bounds(&body.solid))?
}

/// Range `[lo, hi]` of `body` along `frame.z`, relative to `frame.origin`
/// (from the exact bounds' corners: conservative for oblique directions).
pub fn extent_along(body: &Body, frame: &Frame) -> Result<(f64, f64)> {
    let m = bounds(body)?;
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for cx in [m.min[0], m.max[0]] {
        for cy in [m.min[1], m.max[1]] {
            for cz in [m.min[2], m.max[2]] {
                let d = (cx - frame.origin[0]) * frame.z[0]
                    + (cy - frame.origin[1]) * frame.z[1]
                    + (cz - frame.origin[2]) * frame.z[2];
                lo = lo.min(d);
                hi = hi.max(d);
            }
        }
    }
    Ok((lo, hi))
}

/// Through-all overshoot for `body`: `max(THROUGH_ALL_MARGIN_ABS,
/// THROUGH_ALL_MARGIN_REL * bounding diagonal)` (see docs/TOLERANCES.md).
pub fn through_all_margin(body: &Body) -> Result<f64> {
    let m = bounds(body)?;
    let diag = (0..3)
        .map(|i| (m.max[i] - m.min[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    Ok(tol::THROUGH_ALL_MARGIN_ABS.max(diag * tol::THROUGH_ALL_MARGIN_REL))
}

#[cfg(test)]
mod tests;

/// Whether `body` has an implicit boolean intersection edge without a verified
/// exact replacement (e.g. cylinder ∩ cylinder): such a result is only
/// approximated (and STEP export refuses it). Used to prefer the exact slab
/// construction where it applies (ADR 0007).
pub fn has_implicit_edges(body: &Body) -> bool {
    let cs = body.solid.compress();
    cs.boundaries.iter().any(|shell| {
        shell.edges.iter().any(|e| match &e.curve {
            Curve::IntersectionCurve(ic) => {
                let (a, b) = (shell.vertices[e.vertices.0], shell.vertices[e.vertices.1]);
                crate::exact::exact_edge(ic, a, b).is_err()
            }
            _ => false,
        })
    })
}
