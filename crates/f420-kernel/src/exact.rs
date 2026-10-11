//! Exact explicit curves for boolean intersection edges.
//!
//! Truck 0.6 booleans return every new edge as an `IntersectionCurve`: an implicit
//! curve (surface0 ∩ surface1) whose every evaluation runs an un-hinted double
//! projection (two nearest-point searches plus Newton refinement, ~0.1 ms each on a
//! NURBS surface). Measurement and tessellation evaluate edges thousands of times, so
//! these edges dominated regeneration time (see docs/behavior/timings.md).
//!
//! For the case the product creates, a plane cut by a surface extruded along the
//! plane's normal (a cylinder wall from a Ø hole meeting a top/bottom face, or an
//! extruded line/arc wall meeting a cap), the intersection is *exactly* the
//! extrusion's generating curve projected onto the plane along the extrusion
//! direction. That projection is an affine map, and an affine map applied to the
//! homogeneous control points of a rational B-spline gives the exact image curve, so
//! the rebuilt curve is the true intersection (a circle arc stays a rational
//! quadratic arc), not an approximation.
//!
//! Every replacement is *verified* against the original edge before it is accepted
//! (see [`VERIFY_TOL`]); any mismatch keeps the original intersection curve, which
//! stays correct, only slow. Other intersection types (cylinder ∩ cylinder, plane
//! oblique to the extrusion) are never replaced.
use crate::{KernelError, Result, guarded};
use truck_modeling::*;

/// Endpoint and on-surface tolerance for an accepted rebuilt curve (mm): the rebuilt
/// curve must start/end within this distance of the boolean's vertices and its
/// samples must lie this close to both surfaces.
pub(crate) const REBUILD_TOL: f64 = 1.0e-7;
/// Agreement tolerance (mm) between points of the original intersection curve and the
/// rebuilt curve. The original's points come from Truck's double projection, which
/// stops at `truck_base::tolerance::TOLERANCE` (1e-6); this is ten times that.
pub(crate) const VERIFY_TOL: f64 = 1.0e-5;
/// Interior samples of the original curve checked against the rebuilt one.
const VERIFY_SAMPLES: usize = 7;

fn unsupported(msg: &str) -> KernelError {
    KernelError::InvalidInput(format!("exact intersection unsupported: {msg}"))
}

/// Plane through `o` with unit normal `n`; project a point onto it along `n`.
fn project(p: Point3, o: Point3, n: Vector3) -> Point3 {
    p - n * (p - o).dot(n)
}

/// True when the surface's normals are perpendicular to `d` at a 9 x 9 parameter
/// grid, i.e. the surface is (to rounding) a general cylinder ruled along `d`.
fn is_extrusion_along(s: &NurbsSurface<Vector4>, d: Vector3) -> bool {
    let ((u0, u1), (v0, v1)) = s.range_tuple();
    (0..=8).all(|i| {
        (0..=8).all(|j| {
            let n = s.normal(
                u0 + (u1 - u0) * i as f64 / 8.0,
                v0 + (v1 - v0) * j as f64 / 8.0,
            );
            n.dot(d).abs() < 1e-12
        })
    })
}

/// Generating curve of a NURBS surface extruded along `n`, projected onto the plane
/// through `o` with normal `n`. The projection p -> p - ((p - o).n) n is affine, so
/// applying it to the Euclidean part of each homogeneous control point (and keeping
/// the weight) maps the rational curve exactly.
fn generating_curve_on(
    s: &NurbsSurface<Vector4>,
    o: Point3,
    n: Vector3,
) -> Result<NurbsCurve<Vector4>> {
    // One parameter direction is the pure extrusion: try both iso-curves.
    for mut c in [s.column_curve(0), s.row_curve(0)] {
        c.transform_control_points(|v| {
            let w = v.w;
            if w == 0.0 {
                return;
            }
            let p = project(Point3::new(v.x / w, v.y / w, v.z / w), o, n);
            *v = Vector4::new(p.x * w, p.y * w, p.z * w, w);
        });
        if c.control_points().iter().any(|v| v.w <= 0.0) {
            return Err(unsupported("non-positive NURBS weight"));
        }
        // Accept the iso-curve that varies in the plane (not the extrusion line,
        // which projects to a point).
        let (t0, t1) = c.range_tuple();
        if c.subs(t0).distance(c.subs((t0 + t1) / 2.0)) > 1e-9 {
            return Ok(c);
        }
    }
    Err(unsupported("could not identify generating curve"))
}

/// Exact replacement for one intersection edge from vertex `p0` to vertex `p1` (in the
/// edge's absolute direction), or an error when the case is unsupported or the
/// rebuilt curve fails verification.
pub(crate) fn exact_edge(
    ic: &IntersectionCurve<Leader, Surface>,
    p0: Point3,
    p1: Point3,
) -> Result<Curve> {
    if let (Surface::Plane(a), Surface::Plane(b)) = (ic.surface0(), ic.surface1()) {
        return plane_plane_edge(ic, &a, &b, p0, p1);
    }
    let (plane, other) = match (ic.surface0(), ic.surface1()) {
        (Surface::Plane(p), o) | (o, Surface::Plane(p)) => (p, o),
        _ => return Err(unsupported("intersection without a planar face")),
    };
    let n = plane.normal();
    let o = plane.origin();
    if (n.magnitude() - 1.0).abs() > 1e-12 {
        return Err(unsupported("degenerate plane normal"));
    }
    let Surface::NurbsSurface(ns) = other else {
        return Err(unsupported("intersection with a non-NURBS surface"));
    };
    if !is_extrusion_along(ns, n) {
        return Err(unsupported(
            "curved face is not extruded along the plane normal",
        ));
    }
    let full = generating_curve_on(ns, o, n)?;
    let t0 = full
        .search_nearest_parameter(p0, None, 100)
        .ok_or_else(|| unsupported("edge start not on generating curve"))?;
    let t1 = full
        .search_nearest_parameter(p1, None, 100)
        .ok_or_else(|| unsupported("edge end not on generating curve"))?;
    if (t1 - t0).abs() < 1e-12 {
        return Err(unsupported("edge end points map to one parameter"));
    }
    let (lo, hi) = if t0 <= t1 { (t0, t1) } else { (t1, t0) };
    let mut c = full;
    let mut sub = c.cut(lo);
    let _tail = sub.cut(hi);
    if t0 > t1 {
        sub.invert();
    }
    let (s0, s1) = sub.range_tuple();
    if sub.subs(s0).distance(p0) > REBUILD_TOL || sub.subs(s1).distance(p1) > REBUILD_TOL {
        return Err(unsupported("rebuilt edge endpoints do not match vertices"));
    }
    // Interior samples must lie on both surfaces (i.e. on the exact intersection) ...
    for k in 1..8 {
        let q = sub.subs(s0 + (s1 - s0) * k as f64 / 8.0);
        let on_plane = (q - o).dot(n).abs() < REBUILD_TOL;
        let on_surface = ns
            .search_nearest_parameter(q, None, 100)
            .is_some_and(|(u, v)| ns.subs(u, v).distance(q) < REBUILD_TOL);
        if !(on_plane && on_surface) {
            return Err(unsupported("rebuilt edge does not lie on both surfaces"));
        }
    }
    // ... and be the same branch, traversed the same way, as Truck's edge: the
    // original's end points and interior samples (at increasing leader parameter)
    // must lie on the rebuilt curve at increasing parameter.
    let (l0, l1) = ic.leader().range_tuple();
    let mut prev_t = s0 - 1e-9;
    for k in 0..=VERIFY_SAMPLES + 1 {
        let l = l0 + (l1 - l0) * k as f64 / (VERIFY_SAMPLES + 1) as f64;
        let q = ic
            .search_triple(l)
            .ok_or_else(|| unsupported("cannot evaluate original edge"))?
            .0;
        let t = sub
            .search_nearest_parameter(q, None, 100)
            .ok_or_else(|| unsupported("original edge point not on rebuilt edge"))?;
        let inside = t >= s0 - 1e-9 && t <= s1 + 1e-9;
        if !inside || sub.subs(t).distance(q) > VERIFY_TOL || t < prev_t {
            return Err(unsupported(
                "rebuilt edge is not the same arc as the original",
            ));
        }
        prev_t = t;
    }
    Ok(Curve::NurbsCurve(sub))
}

/// Two planes meet in a straight line, so the edge between its vertices is exactly
/// the segment `p0 -> p1`. Verified: both ends on both planes, and the original
/// curve's samples on the segment at non-decreasing parameter.
fn plane_plane_edge(
    ic: &IntersectionCurve<Leader, Surface>,
    a: &Plane,
    b: &Plane,
    p0: Point3,
    p1: Point3,
) -> Result<Curve> {
    let (na, nb) = (a.normal(), b.normal());
    if na.cross(nb).magnitude() < 1e-9 {
        return Err(unsupported("parallel planes"));
    }
    for p in [p0, p1] {
        if (p - a.origin()).dot(na).abs() > REBUILD_TOL
            || (p - b.origin()).dot(nb).abs() > REBUILD_TOL
        {
            return Err(unsupported("edge vertex not on both planes"));
        }
    }
    let d = p1 - p0;
    let len2 = d.magnitude2();
    if len2 < 1e-18 {
        return Err(unsupported("zero-length edge"));
    }
    let (l0, l1) = ic.leader().range_tuple();
    let mut prev = -1e-9;
    for k in 0..=VERIFY_SAMPLES + 1 {
        let l = l0 + (l1 - l0) * k as f64 / (VERIFY_SAMPLES + 1) as f64;
        let q = ic
            .search_triple(l)
            .ok_or_else(|| unsupported("cannot evaluate original edge"))?
            .0;
        let t = (q - p0).dot(d) / len2;
        let off = (q - (p0 + d * t)).magnitude();
        if off > VERIFY_TOL || t < prev || !(-1e-9..=1.0 + 1e-9).contains(&t) {
            return Err(unsupported(
                "original edge is not the segment between its vertices",
            ));
        }
        prev = t;
    }
    Ok(Curve::Line(Line(p0, p1)))
}

/// Outcome of [`exactify`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ExactifyStats {
    /// Intersection edges replaced by verified exact curves.
    pub replaced: usize,
    /// Intersection edges kept as implicit intersection curves.
    pub kept: usize,
}

/// Rebuild `solid` with every supported boolean intersection edge replaced by its
/// verified exact explicit curve. Topology (vertices, edge/face incidence and
/// orientation) and surfaces are unchanged; only edge curve representations differ.
/// Unsupported or unverified edges keep their original curve. Returns `None` when
/// nothing was replaced (or the rebuilt topology could not be assembled), in which
/// case the caller keeps the original solid.
pub(crate) fn exactify(solid: &Solid) -> (Option<Solid>, ExactifyStats) {
    let mut stats = ExactifyStats::default();
    let mut cs = solid.compress();
    for shell in &mut cs.boundaries {
        let verts = shell.vertices.clone();
        for e in &mut shell.edges {
            let Curve::IntersectionCurve(ic) = &e.curve else {
                continue;
            };
            let (a, b) = (verts[e.vertices.0], verts[e.vertices.1]);
            // Truck searches may panic on numerical paths; a panic keeps the original.
            match guarded("exact edge", || exact_edge(ic, a, b)) {
                Ok(Ok(c)) => {
                    e.curve = c;
                    stats.replaced += 1;
                }
                _ => stats.kept += 1,
            }
        }
    }
    if stats.replaced == 0 {
        return (None, stats);
    }
    match Solid::extract(cs) {
        Ok(s) => (Some(s), stats),
        Err(_) => {
            stats.kept += stats.replaced;
            stats.replaced = 0;
            (None, stats)
        }
    }
}
