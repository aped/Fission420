//! Direct editing of B-rep bodies without feature history (run 6, ADR 0010):
//! offset a planar face, resize or remove a cylindrical hole or boss. Meant for
//! imported (STEP) bodies, but it works on any single-shell body.
//!
//! Every edit is exact and works on the body's compressed topology:
//!
//! * **Offset a planar face** by `d` along its outward normal `n`. Every face
//!   touching the face (along an edge or at a vertex) must be invariant under
//!   translation along `n`: a plane containing `n`, or a curved wall whose
//!   normal is perpendicular to `n` everywhere (cylinders whose axis is along
//!   `n`). The face's plane, edges and vertices are translated by `d·n` (an
//!   affine map, so exact); every other edge ending on the face must be a
//!   straight segment along `n` and is rebuilt as the exact segment to the
//!   moved vertex. Topology and face order are unchanged. Refused when a side
//!   edge would shrink below `MIN_LENGTH` or flip (a face degenerates or the
//!   topology would change), or a neighbour is oblique.
//! * **Resize a cylindrical feature** (a hole: concave; a boss: convex): the
//!   coaxial cylinder faces of one radius connected to the picked face, plus
//!   its *caps* (planar faces perpendicular to the axis bounded only by those
//!   cylinders: a blind hole's flat floor, a boss's top). Every other face it
//!   meets (*pierced* faces) must be a plane perpendicular to the axis. The
//!   cylinders, caps, their edges and vertices are mapped by the radial scaling
//!   about the axis (affine, so circles stay exact circles). Growing is checked
//!   exactly against every other boundary edge of the pierced faces (lines and
//!   circular arcs; free-form edges are refused).
//! * **Remove a cylindrical feature**: delete its faces and the inner loops it
//!   leaves on the pierced faces (they must be whole loops, never a face's only
//!   loop); the pierced planes close the opening.
//!
//! Every result is then checked independently (ADR 0005): one closed shell,
//! every touched edge on the surfaces of both its faces, and
//! `verify_prisms_boolean` against the nominal prism the edit adds or removes
//! (an offset adds/removes the face region swept by `d`; a resize/removal the
//! disk or ring between the two radii over the feature's axial span).
use crate::validate::{Prism, PrismOp, verify_prisms_boolean};
use crate::{Body, Frame, KernelError, LoopSegment, Profile, Result, guarded, tol};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use truck_modeling::*;
use truck_topology::compress::{CompressedFace, CompressedShell, CompressedSolid};

type CS = CompressedShell<Point3, Curve, Surface>;

/// A wall's unit normal may have at most this component along the offset
/// direction (as `prismatic::RULED_TOL`).
const WALL_TOL: f64 = 1.0e-9;

/// What kind of face a direct edit sees.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DirectFace {
    /// Plane through `origin` with outward unit normal `normal`.
    Plane { origin: [f64; 3], normal: [f64; 3] },
    /// Part of a circular cylinder: axis through `origin` along `axis`,
    /// `radius`; `hole`: the material is outside (outward normal toward the axis).
    Cylinder {
        origin: [f64; 3],
        axis: [f64; 3],
        radius: f64,
        hole: bool,
    },
    /// Anything else (not editable directly).
    Other,
}

/// A cylindrical hole or boss found from one of its faces (face indices in
/// `face_iter` order).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CylinderFeature {
    /// Coaxial cylinder faces of this radius (a STEP cylinder is split into two).
    pub faces: Vec<usize>,
    /// Planar disks perpendicular to the axis bounded only by those cylinders
    /// (a blind hole's floor, a boss's top).
    pub caps: Vec<usize>,
    /// Planes perpendicular to the axis that the feature opens into.
    pub pierced: Vec<usize>,
    /// Axis point at the low end of the span, unit axis, radius.
    pub origin: [f64; 3],
    pub axis: [f64; 3],
    pub radius: f64,
    /// Material outside the cylinder (a hole) or inside (a boss).
    pub hole: bool,
    /// Length of the feature along the axis from `origin`.
    pub length: f64,
}

impl CylinderFeature {
    /// "through hole", "blind hole" or "boss".
    pub fn describe(&self) -> &'static str {
        match (self.hole, self.caps.len()) {
            (true, 0) => "through hole",
            (true, _) => "blind hole",
            (false, _) => "boss",
        }
    }
}

/// Outcome of an edit: the new body and the indices (old `face_iter` order) of
/// faces that no longer exist. Faces not listed keep their identity; their
/// indices shift down past removed ones ([`renumber`]).
#[derive(Debug, Clone)]
pub struct DirectEdit {
    pub body: Body,
    pub removed_faces: Vec<usize>,
}

/// New index of old face `old` after removing `removed` (sorted), or None.
pub fn renumber(old: usize, removed: &[usize]) -> Option<usize> {
    if removed.binary_search(&old).is_ok() {
        return None;
    }
    Some(old - removed.partition_point(|r| *r < old))
}

fn invalid(m: impl Into<String>) -> KernelError {
    KernelError::InvalidInput(m.into())
}

fn shell_of(body: &Body) -> Result<CS> {
    let mut cs = guarded("compress body", || body.solid.compress())?;
    if cs.boundaries.len() != 1 {
        return Err(invalid(format!(
            "the body has {} shells; direct edits need one closed shell",
            cs.boundaries.len()
        )));
    }
    Ok(cs.boundaries.remove(0))
}

fn rebuild(sh: CS) -> Result<Body> {
    let solid = guarded("rebuild body", || {
        Solid::extract(CompressedSolid {
            boundaries: vec![sh],
        })
    })?
    .map_err(|e| KernelError::InvalidResult(format!("edited body is not a valid solid: {e}")))?;
    Ok(Body { solid })
}

fn face_at(sh: &CS, face: usize) -> Result<&CompressedFace<Surface>> {
    sh.faces.get(face).ok_or_else(|| {
        invalid(format!(
            "face {face} does not exist (the body has {} faces)",
            sh.faces.len()
        ))
    })
}

fn p3(p: Point3) -> [f64; 3] {
    [p.x, p.y, p.z]
}

fn v3(v: Vector3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

/// Size scale of the shell (bounding diagonal of its vertices, at least 1).
fn scale(sh: &CS) -> f64 {
    let mut lo = Point3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut hi = Point3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in &sh.vertices {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (hi - lo).magnitude().max(1.0)
}

/// Geometric tolerance for "lies on" checks of this shell.
fn geo_tol(sh: &CS) -> f64 {
    // Same scale-aware rule as the STEP importer's consistency tolerance.
    1.0e-5_f64.max(1.0e-9 * scale(sh))
}

/// Edges (with their direction in the wire) of a face, all wires.
fn face_edges(f: &CompressedFace<Surface>) -> impl Iterator<Item = usize> + '_ {
    f.boundaries.iter().flatten().map(|e| e.index)
}

/// For each edge, the faces using it.
fn edge_faces(sh: &CS) -> Vec<Vec<usize>> {
    let mut out = vec![Vec::new(); sh.edges.len()];
    for (i, f) in sh.faces.iter().enumerate() {
        for e in face_edges(f) {
            if let Some(v) = out.get_mut(e)
                && !v.contains(&i)
            {
                v.push(i);
            }
        }
    }
    out
}

/// Vertices of a set of edges.
fn edge_vertices(sh: &CS, edges: &BTreeSet<usize>) -> BTreeSet<usize> {
    edges
        .iter()
        .flat_map(|e| [sh.edges[*e].vertices.0, sh.edges[*e].vertices.1])
        .collect()
}

/// For each vertex, the faces touching it.
fn vertex_faces(sh: &CS) -> Vec<BTreeSet<usize>> {
    let mut out = vec![BTreeSet::new(); sh.vertices.len()];
    for (i, f) in sh.faces.iter().enumerate() {
        for e in face_edges(f) {
            let (a, b) = sh.edges[e].vertices;
            out[a].insert(i);
            out[b].insert(i);
        }
    }
    out
}

fn eval_curve(c: &Curve, t: f64) -> Result<Point3> {
    match c {
        Curve::IntersectionCurve(_) => Err(invalid(
            "the body has an implicit intersection edge; direct edits need exact edges",
        )),
        other => Ok(other.subs(t)),
    }
}

fn curve_samples(c: &Curve, n: usize) -> Result<Vec<Point3>> {
    let (t0, t1) = c.range_tuple();
    (0..=n)
        .map(|i| eval_curve(c, t0 + (t1 - t0) * i as f64 / n as f64))
        .collect()
}

/// Outward unit normal of a face at a surface parameter.
fn outward_normal(f: &CompressedFace<Surface>, u: f64, v: f64) -> Vector3 {
    let n = f.surface.normal(u, v);
    if f.orientation { n } else { -n }
}

/// Plane of a planar face: a point and the outward unit normal.
fn plane_of(f: &CompressedFace<Surface>) -> Option<(Point3, Vector3)> {
    let Surface::Plane(pl) = &f.surface else {
        return None;
    };
    let n = pl.normal();
    Some((pl.origin(), if f.orientation { n } else { -n }))
}

/// Surface points with outward normals over a face: boundary samples plus a
/// 7 x 7 grid of the (u, v) box their parameters span.
fn face_samples(sh: &CS, f: &CompressedFace<Surface>) -> Result<Vec<(Point3, Vector3)>> {
    let mut uv = Vec::new();
    for e in face_edges(f) {
        for p in curve_samples(&sh.edges[e].curve, 8)? {
            let Some(q) = f.surface.search_parameter(p, None, 100) else {
                return Err(invalid(
                    "a face's boundary does not lie on its surface (parameter search failed)",
                ));
            };
            uv.push(q);
        }
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for q in &uv {
        lo = [lo[0].min(q.0), lo[1].min(q.1)];
        hi = [hi[0].max(q.0), hi[1].max(q.1)];
    }
    for i in 0..=6 {
        for j in 0..=6 {
            uv.push((
                lo[0] + (hi[0] - lo[0]) * i as f64 / 6.0,
                lo[1] + (hi[1] - lo[1]) * j as f64 / 6.0,
            ));
        }
    }
    Ok(uv
        .into_iter()
        .map(|(u, v)| (f.surface.subs(u, v), outward_normal(f, u, v)))
        .filter(|(p, n)| p.x.is_finite() && n.x.is_finite() && n.magnitude() > 0.5)
        .collect())
}

/// Circular cylinder fit of a curved face: (axis point, unit axis, radius,
/// hole), or None when the face is not a cylinder to `tol`.
fn cylinder_of(
    sh: &CS,
    f: &CompressedFace<Surface>,
    tol: f64,
) -> Result<Option<(Point3, Vector3, f64, bool)>> {
    if matches!(f.surface, Surface::Plane(_)) {
        return Ok(None);
    }
    let s = face_samples(sh, f)?;
    if s.len() < 3 {
        return Ok(None);
    }
    // Axis: the cross product of the two least parallel normals.
    let (mut best, mut axis) = (0.0, Vector3::zero());
    for (i, a) in s.iter().enumerate() {
        for b in &s[i + 1..] {
            let c = a.1.cross(b.1);
            if c.magnitude() > best {
                best = c.magnitude();
                axis = c;
            }
        }
    }
    if best < 1e-3 {
        return Ok(None);
    }
    let axis = axis.normalize();
    if s.iter().any(|(_, n)| n.dot(axis).abs() > WALL_TOL * 1e3) {
        return Ok(None);
    }
    // Centre: intersection of the normal lines of the two samples whose
    // normals are least parallel, in the plane perpendicular to the axis.
    let (mut i0, mut j0, mut bc) = (0, 1, 0.0);
    for i in 0..s.len() {
        for j in i + 1..s.len() {
            let c = s[i].1.cross(s[j].1).magnitude();
            if c > bc {
                (i0, j0, bc) = (i, j, c);
            }
        }
    }
    let flat = |v: Vector3| v - axis * v.dot(axis);
    let (p, n) = (s[i0].0, flat(s[i0].1));
    let (q, m) = (s[j0].0, flat(s[j0].1));
    // p + t n = q + u m (projected): solve with the cross product about the axis.
    let w = flat(q - p);
    let den = n.cross(m).dot(axis);
    if den.abs() < 1e-12 {
        return Ok(None);
    }
    let t = w.cross(m).dot(axis) / den;
    let c = p + n * t;
    let radial = |x: Point3| flat(x - c);
    let r = radial(p).magnitude();
    if r.is_nan() || r <= tol::MIN_LENGTH {
        return Ok(None);
    }
    let mut toward = 0;
    for (x, nn) in &s {
        let d = radial(*x);
        if (d.magnitude() - r).abs() > tol {
            return Ok(None);
        }
        let u = d / d.magnitude();
        let along = nn.dot(u);
        if (along.abs() - 1.0).abs() > 1e-6 {
            return Ok(None);
        }
        if along < 0.0 {
            toward += 1;
        }
    }
    let hole = toward * 2 > s.len();
    if toward != 0 && toward != s.len() {
        return Ok(None);
    }
    Ok(Some((c, axis, r, hole)))
}

/// Classify face `face` (in `face_iter` order) of `body` for direct editing.
pub fn classify_face(body: &Body, face: usize) -> Result<DirectFace> {
    let sh = shell_of(body)?;
    let f = face_at(&sh, face)?;
    if let Some((o, n)) = plane_of(f) {
        return Ok(DirectFace::Plane {
            origin: p3(o),
            normal: v3(n),
        });
    }
    let tol = geo_tol(&sh);
    Ok(match cylinder_of(&sh, f, tol)? {
        Some((c, a, r, hole)) => DirectFace::Cylinder {
            origin: p3(c),
            axis: v3(a),
            radius: r,
            hole,
        },
        None => DirectFace::Other,
    })
}

/// [`classify_face`] for every face of `body` (one pass over its topology).
pub fn classify_faces(body: &Body) -> Result<Vec<DirectFace>> {
    let sh = shell_of(body)?;
    let tol = geo_tol(&sh);
    sh.faces
        .iter()
        .map(|f| {
            if let Some((o, n)) = plane_of(f) {
                return Ok(DirectFace::Plane {
                    origin: p3(o),
                    normal: v3(n),
                });
            }
            Ok(match cylinder_of(&sh, f, tol)? {
                Some((c, a, r, hole)) => DirectFace::Cylinder {
                    origin: p3(c),
                    axis: v3(a),
                    radius: r,
                    hole,
                },
                None => DirectFace::Other,
            })
        })
        .collect()
}

/// Whether a face is a wall along `n`: a plane containing `n`, or a curved
/// face whose outward normal is perpendicular to `n` everywhere sampled.
fn wall_along(sh: &CS, f: &CompressedFace<Surface>, n: Vector3) -> Result<bool> {
    if let Some((_, m)) = plane_of(f) {
        return Ok(m.dot(n).abs() <= WALL_TOL);
    }
    let s = face_samples(sh, f)?;
    Ok(!s.is_empty() && s.iter().all(|(_, m)| m.dot(n).abs() <= WALL_TOL * 1e3))
}

/// Every edge in `edges` lies on the surfaces of the faces using it.
fn check_edges_on_faces(sh: &CS, edges: &BTreeSet<usize>, tol: f64) -> Result<()> {
    let ef = edge_faces(sh);
    for &e in edges {
        let pts = curve_samples(&sh.edges[e].curve, 16)?;
        for &fi in &ef[e] {
            let s = &sh.faces[fi].surface;
            for p in &pts {
                let ok = s
                    .search_parameter(*p, None, 100)
                    .map(|(u, v)| (s.subs(u, v) - *p).magnitude() <= tol)
                    .unwrap_or(false);
                if !ok {
                    return Err(KernelError::InvalidResult(format!(
                        "direct edit: edge {e} leaves the surface of face {fi} (the face's \
                         geometry does not extend that far)"
                    )));
                }
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ 2-D loops

/// Right-handed frame on a plane: origin `o`, normal `n`.
fn frame_on(o: Point3, n: Vector3) -> Frame {
    let x = crate::step_in::least_aligned_axis(n);
    let y = n.cross(x);
    Frame {
        origin: p3(o),
        x: v3(x),
        y: v3(y),
        z: v3(n),
    }
}

fn to2(fr: &Frame, p: Point3) -> [f64; 2] {
    let d = [p.x - fr.origin[0], p.y - fr.origin[1], p.z - fr.origin[2]];
    let dot = |a: [f64; 3]| d[0] * a[0] + d[1] * a[1] + d[2] * a[2];
    [dot(fr.x), dot(fr.y)]
}

/// One boundary edge in a plane frame: a segment or a circular arc.
#[derive(Debug, Clone, Copy)]
enum Edge2 {
    Line([f64; 2], [f64; 2]),
    Arc {
        a: [f64; 2],
        b: [f64; 2],
        center: [f64; 2],
        ccw: bool,
    },
}

/// Classify edge `e` traversed forward (`fwd`) in frame `fr`: straight or a
/// circular arc to `tol`, else None.
fn edge2(sh: &CS, e: usize, fwd: bool, fr: &Frame, tol: f64) -> Result<Option<Edge2>> {
    let ce = &sh.edges[e];
    let (mut a, mut b) = (
        to2(fr, sh.vertices[ce.vertices.0]),
        to2(fr, sh.vertices[ce.vertices.1]),
    );
    let mut s: Vec<[f64; 2]> = curve_samples(&ce.curve, 16)?
        .into_iter()
        .map(|p| to2(fr, p))
        .collect();
    if !fwd {
        std::mem::swap(&mut a, &mut b);
        s.reverse();
    }
    let seg = |p: [f64; 2]| {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l2 = dx * dx + dy * dy;
        let t = if l2 > 0.0 {
            (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
    };
    let closed = ce.vertices.0 == ce.vertices.1;
    if matches!(ce.curve, Curve::Line(_)) || (!closed && s.iter().all(|p| seg(*p) <= tol)) {
        return Ok(Some(Edge2::Line(a, b)));
    }
    let n = s.len() - 1;
    let (p, q, r) = (s[0], s[n / 3], s[2 * n / 3]);
    let d = 2.0 * (p[0] * (q[1] - r[1]) + q[0] * (r[1] - p[1]) + r[0] * (p[1] - q[1]));
    if d.abs() < 1e-300 {
        return Ok(None);
    }
    let sq = |v: [f64; 2]| v[0] * v[0] + v[1] * v[1];
    let center = [
        (sq(p) * (q[1] - r[1]) + sq(q) * (r[1] - p[1]) + sq(r) * (p[1] - q[1])) / d,
        (sq(p) * (r[0] - q[0]) + sq(q) * (p[0] - r[0]) + sq(r) * (q[0] - p[0])) / d,
    ];
    let rad = (p[0] - center[0]).hypot(p[1] - center[1]);
    if s.iter()
        .any(|x| ((x[0] - center[0]).hypot(x[1] - center[1]) - rad).abs() > tol)
    {
        return Ok(None);
    }
    let (u, v) = (s[0], s[1]);
    let cr = (u[0] - center[0]) * (v[1] - center[1]) - (u[1] - center[1]) * (v[0] - center[0]);
    Ok(Some(Edge2::Arc {
        a,
        b,
        center,
        ccw: cr > 0.0,
    }))
}

/// Exact distance from `p` to a segment or arc.
fn dist2(p: [f64; 2], e: &Edge2) -> f64 {
    let d = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    match *e {
        Edge2::Line(a, b) => {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l2 = dx * dx + dy * dy;
            let t = if l2 > 0.0 {
                (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            d(p, [a[0] + t * dx, a[1] + t * dy])
        }
        Edge2::Arc { a, b, center, ccw } => {
            let r = d(a, center);
            let to_p = (p[1] - center[1]).atan2(p[0] - center[0]);
            let a0 = (a[1] - center[1]).atan2(a[0] - center[0]);
            let sweep = crate::arc_signed_sweep(a, b, center, ccw);
            let rel = if ccw {
                (to_p - a0).rem_euclid(std::f64::consts::TAU)
            } else {
                (a0 - to_p).rem_euclid(std::f64::consts::TAU)
            };
            let dc = d(p, center);
            if rel <= sweep.abs() && dc > 0.0 {
                (dc - r).abs()
            } else {
                d(p, a).min(d(p, b))
            }
        }
    }
}

/// A wire of a planar face as a counter-clockwise [`Profile`] in `fr`, and
/// its signed area as traversed.
fn wire_profile(
    sh: &CS,
    wire: &[truck_topology::compress::CompressedEdgeIndex],
    fr: &Frame,
    tol: f64,
) -> Result<(Profile, f64)> {
    let mut segs = Vec::new();
    for ei in wire {
        match edge2(sh, ei.index, ei.orientation, fr, tol)? {
            Some(Edge2::Line(a, _)) => segs.push(LoopSegment::Line { start: a }),
            Some(Edge2::Arc { a, center, ccw, .. }) => segs.push(LoopSegment::Arc {
                start: a,
                center,
                ccw,
            }),
            None => {
                return Err(invalid(
                    "the face has a boundary edge that is neither straight nor a circular arc",
                ));
            }
        }
    }
    if let [LoopSegment::Arc { start, center, .. }] = segs.as_slice() {
        let radius = (start[0] - center[0]).hypot(start[1] - center[1]);
        let area = std::f64::consts::PI * radius * radius;
        return Ok((
            Profile::Circle {
                center: *center,
                radius,
            },
            area,
        ));
    }
    let area = crate::loop_signed_area(&segs);
    if area < 0.0 {
        segs = reverse_loop(&segs);
    }
    Ok((Profile::Loop(segs), area))
}

fn reverse_loop(segs: &[LoopSegment]) -> Vec<LoopSegment> {
    let n = segs.len();
    (0..n)
        .rev()
        .map(|i| {
            let end = segs[(i + 1) % n].start();
            match &segs[i] {
                LoopSegment::Line { .. } => LoopSegment::Line { start: end },
                LoopSegment::Arc { center, ccw, .. } => LoopSegment::Arc {
                    start: end,
                    center: *center,
                    ccw: !ccw,
                },
            }
        })
        .collect()
}

/// Region of a planar face: frame (z = outward normal), outer profile, holes,
/// exact area.
fn face_region(sh: &CS, face: usize, tol: f64) -> Result<(Frame, Profile, Vec<Profile>, f64)> {
    let f = face_at(sh, face)?;
    let (o, n) = plane_of(f).ok_or_else(|| invalid("the face is not planar"))?;
    let fr = frame_on(o, n);
    let mut loops = f
        .boundaries
        .iter()
        .map(|w| wire_profile(sh, w, &fr, tol))
        .collect::<Result<Vec<_>>>()?;
    let outer = (0..loops.len())
        .max_by(|a, b| loops[*a].1.abs().total_cmp(&loops[*b].1.abs()))
        .ok_or_else(|| invalid("the face has no boundary"))?;
    let (p, a) = loops.remove(outer);
    let holes: f64 = loops.iter().map(|l| l.1.abs()).sum();
    Ok((
        fr,
        p,
        loops.into_iter().map(|l| l.0).collect(),
        a.abs() - holes,
    ))
}

/// Area (mm²) of planar face `face` of `body` (exact for lines and arcs).
pub fn planar_face_area(body: &Body, face: usize) -> Result<f64> {
    let sh = shell_of(body)?;
    let tol = geo_tol(&sh);
    Ok(face_region(&sh, face, tol)?.3)
}

// ------------------------------------------------------------------ checks

/// One closed shell with positive volume.
fn check_solid(body: &Body) -> Result<()> {
    let m = crate::measure(body)?;
    if m.shell_count != 1 || !m.all_shells_closed {
        return Err(KernelError::InvalidResult(
            "direct edit: the result is not one closed shell".into(),
        ));
    }
    if let crate::VolumeResult::Value {
        volume,
        error_estimate,
        ..
    } = m.volume
        && volume <= error_estimate
    {
        return Err(KernelError::InvalidResult(format!(
            "direct edit: the result has non-positive volume {volume}"
        )));
    }
    Ok(())
}

fn verify(target: &Body, result: &Body, op: PrismOp, prism: &Prism) -> Result<()> {
    verify_prisms_boolean(target, result, op, &[(prism, (0.0, prism.length))]).map_err(|e| {
        KernelError::InvalidResult(format!(
            "direct edit failed the independent result check: {e}"
        ))
    })?;
    Ok(())
}

// ------------------------------------------------------------------ offset

/// Offset planar face `face` of `body` by `distance` mm along its outward
/// normal (positive adds material). See the module docs for the scope.
pub fn offset_face(body: &Body, face: usize, distance: f64) -> Result<DirectEdit> {
    if !distance.is_finite() {
        return Err(invalid("offset distance must be finite"));
    }
    if distance.abs() < tol::MIN_LENGTH {
        return Err(invalid(format!(
            "offset distance {distance} mm is below the smallest feature length {} mm",
            tol::MIN_LENGTH
        )));
    }
    let mut sh = shell_of(body)?;
    let tol = geo_tol(&sh);
    let f = face_at(&sh, face)?;
    let (_, n) = plane_of(f).ok_or_else(|| {
        invalid(format!(
            "face {face} is not planar; only planar faces can be offset"
        ))
    })?;
    let fedges: BTreeSet<usize> = face_edges(f).collect();
    let fverts = edge_vertices(&sh, &fedges);
    let vf = vertex_faces(&sh);
    let mut neighbours = BTreeSet::new();
    for v in &fverts {
        neighbours.extend(vf[*v].iter().copied().filter(|g| *g != face));
    }
    for g in &neighbours {
        if !wall_along(&sh, &sh.faces[*g], n)? {
            return Err(invalid(format!(
                "face {g} next to face {face} is not perpendicular to it (not a wall along \
                 its normal); offsetting would change that face's shape, which is not \
                 supported"
            )));
        }
    }
    // Region of the face before the edit (for the result check).
    let (fr, outer, holes, _) = face_region(&sh, face, tol)?;
    let t = n * distance;
    let mv = Matrix4::from_translation(t);
    // Side edges: one end on the face, straight along n.
    let mut side = BTreeMap::new();
    for (e, ce) in sh.edges.iter().enumerate() {
        if fedges.contains(&e) {
            continue;
        }
        let (a, b) = ce.vertices;
        let (on_a, on_b) = (fverts.contains(&a), fverts.contains(&b));
        if !(on_a || on_b) {
            continue;
        }
        if on_a && on_b {
            return Err(invalid(format!(
                "edge {e} joins two vertices of face {face} without being one of its edges; \
                 the offset would change the topology"
            )));
        }
        let (fixed, moving) = if on_a { (b, a) } else { (a, b) };
        let (p, q) = (sh.vertices[fixed], sh.vertices[moving]);
        let along = (q - p).dot(n);
        let pts = curve_samples(&ce.curve, 16)?;
        let off_line = pts
            .iter()
            .any(|x| ((*x - p) - n * (*x - p).dot(n)).magnitude() > tol);
        if off_line || ((q - p) - n * along).magnitude() > tol {
            return Err(invalid(format!(
                "edge {e} ending on face {face} is not a straight edge along its normal"
            )));
        }
        let after = along + distance;
        if after.signum() != along.signum() || after.abs() < tol::MIN_LENGTH {
            return Err(invalid(format!(
                "offsetting face {face} by {distance} mm would collapse or invert an adjacent \
                 {:.6} mm edge (a face would degenerate or the topology would change)",
                along.abs()
            )));
        }
        side.insert(e, (a, b));
    }
    for v in &fverts {
        sh.vertices[*v] += t;
    }
    for e in &fedges {
        sh.edges[*e].curve.transform_by(mv);
    }
    for (e, (a, b)) in &side {
        sh.edges[*e].curve = Curve::Line(Line(sh.vertices[*a], sh.vertices[*b]));
    }
    sh.faces[face].surface.transform_by(mv);
    let touched: BTreeSet<usize> = fedges.iter().chain(side.keys()).copied().collect();
    check_edges_on_faces(&sh, &touched, tol)?;
    let out = rebuild(sh)?;
    check_solid(&out)?;
    let (start, op) = if distance > 0.0 {
        (fr, PrismOp::Join)
    } else {
        (fr.offset(distance), PrismOp::Cut)
    };
    let prism = Prism {
        profile: &outer,
        holes: &holes,
        start: &start,
        length: distance.abs(),
    };
    verify(body, &out, op, &prism)?;
    Ok(DirectEdit {
        body: out,
        removed_faces: vec![],
    })
}

// ------------------------------------------------------------------ cylinders

fn feature_of(sh: &CS, face: usize, tol: f64) -> Result<CylinderFeature> {
    let f = face_at(sh, face)?;
    let Some((c, axis, r, hole)) = cylinder_of(sh, f, tol)? else {
        return Err(invalid(format!(
            "face {face} is not a circular cylinder; pick the wall of a round hole or boss"
        )));
    };
    let ef = edge_faces(sh);
    let same_axis = |c2: Point3, a2: Vector3, r2: f64, h2: bool| {
        let d = c2 - c;
        a2.cross(axis).magnitude() <= 1e-9
            && (d - axis * d.dot(axis)).magnitude() <= tol
            && (r2 - r).abs() <= tol
            && h2 == hole
    };
    let mut faces = BTreeSet::from([face]);
    let mut todo = vec![face];
    let mut others = BTreeSet::new();
    while let Some(g) = todo.pop() {
        for e in face_edges(&sh.faces[g]) {
            for &h in &ef[e] {
                if faces.contains(&h) || others.contains(&h) {
                    continue;
                }
                match cylinder_of(sh, &sh.faces[h], tol)? {
                    Some((c2, a2, r2, h2)) if same_axis(c2, a2, r2, h2) => {
                        faces.insert(h);
                        todo.push(h);
                    }
                    _ => {
                        others.insert(h);
                    }
                }
            }
        }
    }
    let cyl_edges: BTreeSet<usize> = faces
        .iter()
        .flat_map(|g| face_edges(&sh.faces[*g]))
        .collect();
    let (mut caps, mut pierced) = (Vec::new(), Vec::new());
    for &g in &others {
        let gf = &sh.faces[g];
        let Some((_, m)) = plane_of(gf) else {
            return Err(invalid(format!(
                "the cylinder meets face {g}, which is not planar; only holes and bosses \
                 ending on planes perpendicular to their axis can be edited"
            )));
        };
        if m.cross(axis).magnitude() > 1e-9 {
            return Err(invalid(format!(
                "the cylinder ends on face {g}, which is not perpendicular to its axis"
            )));
        }
        if face_edges(gf).all(|e| cyl_edges.contains(&e)) {
            caps.push(g);
        } else {
            pierced.push(g);
        }
    }
    let all: BTreeSet<usize> = faces.iter().chain(&caps).copied().collect();
    let all_edges: BTreeSet<usize> = all.iter().flat_map(|g| face_edges(&sh.faces[*g])).collect();
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in edge_vertices(sh, &all_edges) {
        let s = (sh.vertices[v] - c).dot(axis);
        lo = lo.min(s);
        hi = hi.max(s);
    }
    match (hole, caps.len(), pierced.len()) {
        (true, 0, 2) | (true, 1, 1) | (false, 1, 1) => {}
        (h, nc, np) => {
            return Err(invalid(format!(
                "unsupported cylindrical {} configuration ({nc} end cap(s), opening into {np} \
                 face(s)); supported: a through hole between two planes, a blind hole with a \
                 flat floor, a boss with a flat top",
                if h { "hole" } else { "boss" }
            )));
        }
    }
    Ok(CylinderFeature {
        faces: faces.into_iter().collect(),
        caps,
        pierced,
        origin: p3(c + axis * lo),
        axis: v3(axis),
        radius: r,
        hole,
        length: hi - lo,
    })
}

/// The hole or boss that cylindrical face `face` of `body` belongs to.
pub fn cylinder_feature(body: &Body, face: usize) -> Result<CylinderFeature> {
    let sh = shell_of(body)?;
    let tol = geo_tol(&sh);
    feature_of(&sh, face, tol)
}

fn axis_frame(cf: &CylinderFeature) -> Frame {
    let o = Point3::new(cf.origin[0], cf.origin[1], cf.origin[2]);
    let a = Vector3::new(cf.axis[0], cf.axis[1], cf.axis[2]);
    frame_on(o, a)
}

/// Radial scaling by `k` about the axis through `c` along unit `a`.
fn radial_scaling(c: Point3, a: Vector3, k: f64) -> Matrix4 {
    // S = k I + (1 - k) a a^T, about c.
    let s = Matrix3::from_cols(
        Vector3::new(k, 0.0, 0.0) + a * ((1.0 - k) * a.x),
        Vector3::new(0.0, k, 0.0) + a * ((1.0 - k) * a.y),
        Vector3::new(0.0, 0.0, k) + a * ((1.0 - k) * a.z),
    );
    let cv = c.to_vec();
    Matrix4::from_translation(cv) * Matrix4::from(s) * Matrix4::from_translation(-cv)
}

/// The cylinder surface mapped by the radial scaling `m` (exact): B-spline
/// and NURBS control points are mapped directly; a revolved surface gets its
/// generatrix mapped in the revolution's own frame (the scaling commutes with
/// the revolution about the same axis), keeping the surface rigidly placed.
fn scaled_surface(s: &Surface, m: Matrix4, c: Point3, a: Vector3, k: f64) -> Result<Surface> {
    Ok(match s {
        Surface::RevolutedCurve(p) => {
            let t = *p.transform();
            let inv = t
                .inverse_transform()
                .ok_or_else(|| invalid("singular surface placement"))?;
            // Axis in the entity's frame must be the revolution axis.
            let (ec, ea) = (inv.transform_point(c), inv.transform_vector(a));
            let rc = p.entity();
            let d = ec - rc.origin();
            let ra = rc.axis().normalize();
            if ea.normalize().cross(ra).magnitude() > 1e-9
                || (d - ra * d.dot(ra)).magnitude() > 1e-6
            {
                return Err(invalid(
                    "the cylinder is a surface of revolution about another axis",
                ));
            }
            let me = radial_scaling(ec, ea.normalize(), k);
            let mut g = rc.entity_curve().clone();
            g.transform_by(me);
            let (o, ax) = (rc.origin(), rc.axis());
            Surface::RevolutedCurve(p.map_ref(|_| RevolutedCurve::by_revolution(g, o, ax)))
        }
        Surface::NurbsSurface(_) | Surface::BSplineSurface(_) => {
            let mut s = s.clone();
            s.transform_by(m);
            s
        }
        Surface::Plane(_) => s.clone(),
    })
}

/// Resize the hole or boss of cylindrical face `face` to `diameter` mm.
pub fn resize_cylinder(body: &Body, face: usize, diameter: f64) -> Result<DirectEdit> {
    if !(diameter.is_finite() && diameter / 2.0 >= tol::MIN_LENGTH) {
        return Err(invalid(format!(
            "diameter must be a finite length of at least {} mm",
            2.0 * tol::MIN_LENGTH
        )));
    }
    let mut sh = shell_of(body)?;
    let tol = geo_tol(&sh);
    let cf = feature_of(&sh, face, tol)?;
    let r1 = diameter / 2.0;
    if (r1 - cf.radius).abs() < tol::MIN_LENGTH {
        return Err(invalid(format!(
            "the {} already has diameter {:.6} mm",
            cf.describe(),
            2.0 * cf.radius
        )));
    }
    let c = Point3::new(cf.origin[0], cf.origin[1], cf.origin[2]);
    let a = Vector3::new(cf.axis[0], cf.axis[1], cf.axis[2]);
    let own: BTreeSet<usize> = cf.faces.iter().chain(&cf.caps).copied().collect();
    let own_edges: BTreeSet<usize> = own.iter().flat_map(|g| face_edges(&sh.faces[*g])).collect();
    // Clearance on every pierced face: the new circle must stay clear of every
    // other boundary edge (exact distances to lines and arcs).
    if r1 > cf.radius {
        for &g in &cf.pierced {
            let (o, n) =
                plane_of(&sh.faces[g]).ok_or_else(|| invalid("pierced face not planar"))?;
            let fr = frame_on(o, n);
            let centre = to2(&fr, c + a * (o - c).dot(a));
            for w in &sh.faces[g].boundaries {
                for ei in w {
                    if own_edges.contains(&ei.index) {
                        continue;
                    }
                    let Some(e2) = edge2(&sh, ei.index, ei.orientation, &fr, tol)? else {
                        return Err(invalid(format!(
                            "face {g} has a free-form boundary edge; the clearance of the \
                             enlarged {} cannot be checked exactly",
                            cf.describe()
                        )));
                    };
                    let d = dist2(centre, &e2);
                    if d <= r1 + tol::MIN_LENGTH {
                        return Err(invalid(format!(
                            "a Ø{diameter} mm {} would reach another edge of face {g} \
                             ({d:.6} mm from its axis; largest clear diameter < {:.6} mm)",
                            cf.describe(),
                            2.0 * (d - tol::MIN_LENGTH)
                        )));
                    }
                }
            }
        }
    }
    let k = r1 / cf.radius;
    let m = radial_scaling(c, a, k);
    for v in edge_vertices(&sh, &own_edges) {
        sh.vertices[v] = m.transform_point(sh.vertices[v]);
    }
    for e in &own_edges {
        sh.edges[*e].curve.transform_by(m);
    }
    for g in &cf.faces {
        let s = scaled_surface(&sh.faces[*g].surface, m, c, a, k)?;
        sh.faces[*g].surface = s;
    }
    check_edges_on_faces(&sh, &own_edges, tol)?;
    let out = rebuild(sh)?;
    check_solid(&out)?;
    let fr = axis_frame(&cf);
    let (big, small) = (r1.max(cf.radius), r1.min(cf.radius));
    let disk = |r| Profile::Circle {
        center: [0.0, 0.0],
        radius: r,
    };
    let (outer, holes) = if (r1 > cf.radius) == cf.hole {
        // Material removed: the whole new hole, or the ring of a shrunk boss.
        if cf.hole {
            (disk(big), vec![])
        } else {
            (disk(big), vec![disk(small)])
        }
    } else if cf.hole {
        (disk(big), vec![disk(small)])
    } else {
        (disk(big), vec![])
    };
    let op = if (r1 > cf.radius) == cf.hole {
        PrismOp::Cut
    } else {
        PrismOp::Join
    };
    let prism = Prism {
        profile: &outer,
        holes: &holes,
        start: &fr,
        length: cf.length,
    };
    verify(body, &out, op, &prism)?;
    Ok(DirectEdit {
        body: out,
        removed_faces: vec![],
    })
}

/// Remove the hole or boss of cylindrical face `face`: its faces are deleted
/// and the openings it leaves on the pierced planes are closed.
pub fn remove_cylinder(body: &Body, face: usize) -> Result<DirectEdit> {
    let sh = shell_of(body)?;
    let tol = geo_tol(&sh);
    let cf = feature_of(&sh, face, tol)?;
    let own: BTreeSet<usize> = cf.faces.iter().chain(&cf.caps).copied().collect();
    let own_edges: BTreeSet<usize> = own.iter().flat_map(|g| face_edges(&sh.faces[*g])).collect();
    let ef = edge_faces(&sh);
    let mut out_faces = Vec::new();
    let mut closed_edges = BTreeSet::new();
    for (g, f) in sh.faces.iter().enumerate() {
        if own.contains(&g) {
            continue;
        }
        let mut f = f.clone();
        let before = f.boundaries.len();
        let mut keep = Vec::new();
        for w in f.boundaries.drain(..) {
            let n_own = w.iter().filter(|e| own_edges.contains(&e.index)).count();
            if n_own == 0 {
                keep.push(w);
            } else if n_own == w.len() {
                closed_edges.extend(w.iter().map(|e| e.index));
            } else {
                return Err(invalid(format!(
                    "the {} shares only part of a boundary loop with face {g}; it cannot be \
                     removed by closing the opening",
                    cf.describe()
                )));
            }
        }
        if keep.is_empty() && before > 0 {
            return Err(invalid(format!(
                "face {g} is bounded only by the {}; removing it would remove the face",
                cf.describe()
            )));
        }
        f.boundaries = keep;
        out_faces.push(f);
    }
    for e in &own_edges {
        let users = ef[*e].iter().filter(|g| own.contains(g)).count();
        if !(users == 2 || (users == 1 && closed_edges.contains(e))) {
            return Err(invalid(format!(
                "edge {e} of the {} borders other geometry; it cannot be removed cleanly",
                cf.describe()
            )));
        }
    }
    // Compact edges and vertices.
    let mut emap = vec![usize::MAX; sh.edges.len()];
    let mut vmap = vec![usize::MAX; sh.vertices.len()];
    let mut out = CS {
        vertices: Vec::new(),
        edges: Vec::new(),
        faces: Vec::new(),
    };
    for f in &mut out_faces {
        for w in &mut f.boundaries {
            for ei in w.iter_mut() {
                if emap[ei.index] == usize::MAX {
                    let ce = &sh.edges[ei.index];
                    let mut vid = |v: usize, out: &mut CS| {
                        if vmap[v] == usize::MAX {
                            vmap[v] = out.vertices.len();
                            out.vertices.push(sh.vertices[v]);
                        }
                        vmap[v]
                    };
                    let a = vid(ce.vertices.0, &mut out);
                    let b = vid(ce.vertices.1, &mut out);
                    emap[ei.index] = out.edges.len();
                    out.edges.push(truck_topology::compress::CompressedEdge {
                        vertices: (a, b),
                        curve: ce.curve.clone(),
                    });
                }
                ei.index = emap[ei.index];
            }
        }
    }
    out.faces = out_faces;
    let result = rebuild(out)?;
    check_solid(&result)?;
    let fr = axis_frame(&cf);
    let disk = Profile::Circle {
        center: [0.0, 0.0],
        radius: cf.radius,
    };
    let prism = Prism {
        profile: &disk,
        holes: &[],
        start: &fr,
        length: cf.length,
    };
    let op = if cf.hole { PrismOp::Join } else { PrismOp::Cut };
    verify(body, &result, op, &prism)?;
    Ok(DirectEdit {
        body: result,
        removed_faces: own.into_iter().collect(),
    })
}

mod countersink;
mod replace;
pub use countersink::countersink;
pub use replace::{replace_face, tilt_offset_frame};

#[cfg(test)]
mod tests;
