//! Geometric measurement on the B-rep (not on a mesh).
use crate::{Body, Frame, KernelError, Result, guarded, tol};
use serde::{Deserialize, Serialize};
use truck_modeling::*;

mod mass;
mod revolution;

pub use mass::{VolumeProperties, mesh_volume_properties, volume_properties};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

/// How a [`VolumeResult::Value`] was computed. Unit variants only (the type stays
/// `Copy + Eq`); the accuracy is in `error_estimate`. Serialized by variant name, so
/// existing documents with `"BrepBoundaryIntegralZ"` keep deserializing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VolumeMethod {
    /// Divergence theorem with F = (0, 0, z): V = sum over faces of the integral of
    /// z * n_z dA. Curved faces must be Z-ruled (n_z == 0) or lie in a plane
    /// z = const; planar faces may have any orientation. Planar face integrals become
    /// Green's-theorem line integrals over the exact B-rep edge curves, evaluated by
    /// dense chord sums on exact curve points with Richardson extrapolation. Not a
    /// mesh volume.
    BrepBoundaryIntegralZ,
    /// As [`VolumeMethod::BrepBoundaryIntegralZ`] with F = (x, 0, 0): curved faces
    /// must be X-ruled (n_x == 0) or lie in a plane x = const. Used when Z does not
    /// apply (e.g. a hole drilled along X).
    BrepBoundaryIntegralX,
    /// As [`VolumeMethod::BrepBoundaryIntegralZ`] with F = (0, y, 0): curved faces
    /// must be Y-ruled (n_y == 0) or lie in a plane y = const. Tried after Z and X.
    BrepBoundaryIntegralY,
    /// Divergence theorem with the radial field F = (p - o)_perp / 2 about an axis
    /// (o, a), tried after Z, X and Y. Curved faces must be cones, cylinders or tori
    /// of revolution about that common axis (meridian a line or a circle not touching
    /// the axis, except at a cone apex), or circular cylinders about any axes. Each
    /// face's flux is an exact closed-form 1-form integrated over its exact boundary
    /// edges (Stokes), evaluated like the axis methods (chord sums, Richardson). Covers
    /// lofted frusta, revolved cones/tori and swept circular channels. Not a mesh
    /// volume. See `measure/revolution.rs`.
    BrepBoundaryIntegralRevolution,
    /// Fallback when no exact boundary integral applies (e.g. a sphere): signed
    /// volume of the welded watertight tessellation at two chord tolerances; the
    /// finer value is reported and `error_estimate` is the difference between the
    /// two levels. An estimate, not an exact or bounded value (see
    /// `tessellation_volume` in measure.rs).
    TessellationEstimate,
    /// Mesh bodies (imported STL/3MF): the divergence-theorem integral over the
    /// body's own closed triangle mesh, exact for that mesh (which is the body's
    /// geometry; there is no B-rep). Used by [`mesh_volume_properties`] only.
    TriangleMesh,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VolumeResult {
    Value {
        volume: f64,
        error_estimate: f64,
        method: VolumeMethod,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurements {
    pub shell_count: usize,
    pub all_shells_closed: bool,
    pub face_count: usize,
    pub edge_count: usize,
    pub vertex_count: usize,
    pub bounds: Bounds,
    pub volume: VolumeResult,
}

/// Chord count per edge for the boundary integral (doubled once for Richardson).
const VOLUME_CHORDS: usize = 1024;
/// Chord count per edge for bounds sampling.
const BOUNDS_SAMPLES: usize = 256;

fn eval_curve(c: &Curve, t: f64) -> Option<Point3> {
    match c {
        Curve::IntersectionCurve(ic) => ic.search_triple(t).map(|x| x.0),
        other => Some(other.subs(t)),
    }
}

fn curve_range(c: &Curve) -> (f64, f64) {
    c.range_tuple()
}

/// Bounds from vertices plus dense samples of every edge curve, with each axis
/// extremum refined by a golden-section search between its neighbouring samples.
/// Exact when extrema lie on vertices or straight edges; for explicit curved edges
/// (NURBS arcs) exact to the search precision (unimodal near the extremum, true for
/// circular arcs); boolean intersection curves keep the sampling error.
pub(crate) fn raw_bounds(solid: &Solid) -> Result<Bounds> {
    let mut b = Bounds {
        min: [f64::INFINITY; 3],
        max: [f64::NEG_INFINITY; 3],
    };
    let mut add = |p: Point3| {
        for i in 0..3 {
            b.min[i] = b.min[i].min(p[i]);
            b.max[i] = b.max[i].max(p[i]);
        }
    };
    for v in solid.vertex_iter() {
        add(v.point());
    }
    for e in solid.edge_iter() {
        let c = e.curve();
        if matches!(c, Curve::Line(_)) {
            continue;
        }
        let (t0, t1) = curve_range(&c);
        let fail = || KernelError::Internal("intersection curve evaluation failed".into());
        let ts: Vec<f64> = (0..=BOUNDS_SAMPLES)
            .map(|i| t0 + (t1 - t0) * i as f64 / BOUNDS_SAMPLES as f64)
            .collect();
        let mut pts = Vec::with_capacity(ts.len());
        for &t in &ts {
            let p = eval_curve(&c, t).ok_or_else(fail)?;
            add(p);
            pts.push(p);
        }
        // Refine each axis extremum between the neighbouring samples (golden-section
        // search), so extrema between samples (e.g. an arc's quadrant point) are exact
        // to the curve evaluation rather than to the sampling density. Skipped for
        // boolean intersection curves: each evaluation is a costly projection, and
        // under the product limits (cuts strictly inside the profile) they never
        // carry a bounding extremum.
        if matches!(c, Curve::IntersectionCurve(_)) {
            continue;
        }
        for axis in 0..3 {
            for sign in [1.0, -1.0] {
                let best = (0..pts.len())
                    .max_by(|&i, &j| (sign * pts[i][axis]).total_cmp(&(sign * pts[j][axis])))
                    .unwrap_or(0);
                let (mut a, mut b) = (ts[best.saturating_sub(1)], ts[(best + 1).min(ts.len() - 1)]);
                let f = |t: f64| eval_curve(&c, t).map(|p| (sign * p[axis], p));
                const G: f64 = 0.618_033_988_749_894_8;
                for _ in 0..60 {
                    let (x1, x2) = (b - G * (b - a), a + G * (b - a));
                    let (f1, f2) = (f(x1).ok_or_else(fail)?, f(x2).ok_or_else(fail)?);
                    if f1.0 >= f2.0 { b = x2 } else { a = x1 }
                }
                add(f(0.5 * (a + b)).ok_or_else(fail)?.1);
            }
        }
    }
    if b.min.iter().chain(&b.max).any(|v| !v.is_finite()) {
        return Err(KernelError::InvalidResult(
            "empty or non-finite bounds".into(),
        ));
    }
    Ok(b)
}

/// Integration axis `a` of the boundary-integral volume (F = (p . a) a, div F = 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Z,
    X,
    Y,
}

impl Axis {
    /// Order in which axes are tried. Z first keeps the original method (and its exact
    /// arithmetic) for every body it supported before X and Y were added.
    const ORDER: [Axis; 3] = [Axis::Z, Axis::X, Axis::Y];

    /// World coordinate indices (u, v, k): k is the axis and (e_u, e_v, e_k) is
    /// right-handed, so a wire that is counter-clockwise about +a has positive
    /// projected area in (u, v). For Z this is (x, y, z).
    fn indices(self) -> (usize, usize, usize) {
        match self {
            Axis::Z => (0, 1, 2),
            Axis::X => (1, 2, 0),
            Axis::Y => (2, 0, 1),
        }
    }

    fn method(self) -> VolumeMethod {
        match self {
            Axis::Z => VolumeMethod::BrepBoundaryIntegralZ,
            Axis::X => VolumeMethod::BrepBoundaryIntegralX,
            Axis::Y => VolumeMethod::BrepBoundaryIntegralY,
        }
    }
}

const EVAL_FAIL: &str = "intersection curve evaluation failed";

/// 1/2 * closed-loop sum of (p_u dp_v - p_v dp_u) over a wire, `n` chords per edge:
/// the signed area of the wire projected on the (u, v) coordinate plane. For
/// (u, v) = (x, y) this is the original `x dy - y dx` sum, bit for bit.
fn wire_area_proj(w: &Wire, n: usize, u: usize, v: usize) -> Option<f64> {
    let mut a = 0.0;
    for e in w.edge_iter() {
        let c = e.oriented_curve();
        let (t0, t1) = c.range_tuple();
        let mut prev = eval_curve(&c, t0)?;
        for i in 1..=n {
            let p = eval_curve(&c, t0 + (t1 - t0) * i as f64 / n as f64)?;
            a += prev[u] * p[v] - p[u] * prev[v];
            prev = p;
        }
    }
    Some(a / 2.0)
}

/// Classification of a non-`Plane` face surface relative to an axis a.
enum AxisClass {
    /// n . a == 0 everywhere (ruled along a): contributes nothing to the integral of
    /// (p . a)(n . a) dA.
    Ruled,
    /// Lies in the plane p . a = c with normal +-a (e.g. a revolved annulus).
    Perpendicular(f64),
}

fn bound(b: std::ops::Bound<f64>) -> Option<f64> {
    match b {
        std::ops::Bound::Included(x) | std::ops::Bound::Excluded(x) => Some(x),
        std::ops::Bound::Unbounded => None,
    }
}

/// Classify a non-`Plane` surface by dense sampling of its (finite) parameter domain.
/// Anything that is neither ruled along the axis nor a plane perpendicular to it is
/// unsupported. For Z this is the original Z-ruled / horizontal test.
fn classify_by_sampling(s: &Surface, axis: Axis) -> std::result::Result<AxisClass, String> {
    let (u, v, k) = axis.indices();
    let ((a0, a1), (b0, b1)) = s.parameter_range();
    let (Some(u0), Some(u1), Some(v0), Some(v1)) = (bound(a0), bound(a1), bound(b0), bound(b1))
    else {
        return Err("unbounded surface parameter range".into());
    };
    const K: usize = 16;
    let (mut ruled, mut perpendicular) = (true, true);
    let c0 = s.subs(u0, v0)[k];
    for i in 0..=K {
        for j in 0..=K {
            let (pu, pv) = (
                u0 + (u1 - u0) * i as f64 / K as f64,
                v0 + (v1 - v0) * j as f64 / K as f64,
            );
            let nn = s.normal(pu, pv);
            if !nn[u].is_finite() || !nn[k].is_finite() {
                continue; // degenerate point (e.g. on the axis of a revolution); judged by neighbours
            }
            ruled &= nn[k].abs() <= 1e-12;
            perpendicular &=
                nn[u].abs() + nn[v].abs() <= 1e-12 && (s.subs(pu, pv)[k] - c0).abs() <= 1e-9;
        }
    }
    match (ruled, perpendicular) {
        (true, _) => Ok(AxisClass::Ruled),
        (_, true) => Ok(AxisClass::Perpendicular(c0)),
        _ => Err(format!(
            "curved face is neither {axis:?}-ruled nor perpendicular to {axis:?}"
        )),
    }
}

/// Whether every curved face of `solid` is ruled along / perpendicular to `axis`
/// (planar faces of any orientation are always supported).
fn axis_supported(solid: &Solid, axis: Axis) -> std::result::Result<(), String> {
    for f in solid.face_iter() {
        let s = f.surface();
        if !matches!(s, Surface::Plane(_)) {
            classify_by_sampling(&s, axis)?;
        }
    }
    Ok(())
}

/// Integral of (p . a) dA over a planar face that is neither parallel nor
/// perpendicular to the axis, times (n_plane . a). Green's theorem in the plane's own
/// orthonormal frame (o, e1, e2), e1 x e2 = n_plane: with s, t the frame coordinates,
/// the integral is (o . a) A + (e1 . a) M_s + (e2 . a) M_t, where A, M_s, M_t are the
/// signed area and first moments of all boundary wires (inner loops have the opposite
/// sense and subtract). The wires carry the face orientation, so when the outward
/// normal is -n_plane both A, M and the normal flip sign and the product is
/// unchanged: the result is the face's contribution (p . a)(n_out . a) dA.
fn tilted_plane_integral(f: &Face, pl: &Plane, axis: Axis, n: usize) -> Option<f64> {
    let (_, _, k) = axis.indices();
    let np = pl.normal();
    let e1 = pl.u_axis().normalize();
    let e2 = np.cross(e1);
    let o = pl.origin();
    let mut m = [0.0; 3];
    for w in f.boundaries() {
        let wm = wire_moments_by(&w, n, |p| {
            let d = p - o;
            [d.dot(e1), d.dot(e2)]
        })?;
        for i in 0..3 {
            m[i] += wm[i];
        }
    }
    Some(np[k] * (o[k] * m[0] + e1[k] * m[1] + e2[k] * m[2]))
}

/// Boundary-integral volume with F = (p . a) a at `n` chords per edge. Faces
/// perpendicular to the axis use the projected-area arithmetic of the original
/// Z-only method (identical results for Z); tilted planar faces use
/// [`tilted_plane_integral`]; curved faces must classify (see [`classify_by_sampling`]).
fn volume_axis(solid: &Solid, axis: Axis, n: usize) -> std::result::Result<f64, String> {
    let (u, v, k) = axis.indices();
    let mut vol = 0.0;
    for f in solid.face_iter() {
        match f.surface() {
            Surface::Plane(pl) => {
                let mut nrm = pl.normal();
                if !f.orientation() {
                    nrm = -nrm;
                }
                if nrm[u].abs() + nrm[v].abs() < 1e-12 {
                    let c = pl.origin()[k];
                    let mut area = 0.0;
                    for w in f.boundaries() {
                        area += wire_area_proj(&w, n, u, v).ok_or(EVAL_FAIL)?;
                    }
                    vol += c * area;
                } else if nrm[k].abs() > 1e-12 {
                    vol += tilted_plane_integral(f, &pl, axis, n).ok_or(EVAL_FAIL)?;
                }
            }
            other => match classify_by_sampling(&other, axis)? {
                AxisClass::Ruled => {}
                AxisClass::Perpendicular(c) => {
                    let mut area = 0.0;
                    for w in f.boundaries() {
                        area += wire_area_proj(&w, n, u, v).ok_or(EVAL_FAIL)?;
                    }
                    vol += c * area;
                }
            },
        }
    }
    Ok(vol)
}

/// Exact-path volume about `axis`: three chord levels (n/2, n, 2n), Richardson
/// extrapolation, error estimate = difference of the two extrapolants.
fn exact_volume(solid: &Solid, axis: Axis) -> std::result::Result<(f64, f64), String> {
    let n = VOLUME_CHORDS;
    // The three refinement levels are independent; evaluate them concurrently.
    let [a, b, c] = std::thread::scope(|sc| {
        let hs = [n / 2, n, 2 * n].map(|k| sc.spawn(move || volume_axis(solid, axis, k)));
        hs.map(|h| {
            h.join()
                .unwrap_or_else(|_| Err("volume worker panicked".into()))
        })
    });
    let (a, b, c) = (a?, b?, c?);
    let r1 = (4.0 * b - a) / 3.0;
    let r2 = (4.0 * c - b) / 3.0;
    Ok((r2, (r2 - r1).abs()))
}

/// Coarse chord of the tessellation fallback, relative to the bounding diagonal.
const FALLBACK_CHORD_REL: f64 = 1e-4;
/// Fine chord = coarse chord / this.
const FALLBACK_REFINE: f64 = 4.0;

/// Fallback volume: signed (divergence theorem) volume of the welded tessellation at
/// chords c = max(1e-4 x bbox diagonal, 4 x the minimum mesh chord) and c / 4. Both
/// meshes must be watertight. Returns (fine volume, |coarse - fine|).
///
/// Why the difference is a usable estimate: the tessellation vertices lie on the
/// exact surfaces, so the mesh is inscribed and its volume error is driven by the
/// sagitta, which is O(chord) (O(h^2) in edge length h). For smooth curved faces
/// the error behaves like E(c) ~ K c, so E(c) - E(c/4) ~ (3/4) K c = 3 E(c/4): the
/// difference over-estimates the fine error by about 3x once the meshes are in the
/// asymptotic regime. For convex(-ish) faces both errors have the same sign (the
/// inscribed mesh lies inside), which is what makes the difference dominate the
/// fine error rather than cancel. It is still an estimate, not a bound: Truck's
/// triangulation is not guaranteed to honour the chord exactly or to refine
/// uniformly, saddle regions (e.g. a torus' inner half) partially cancel, and
/// features smaller than the chord can be missed entirely. Observed (torus R 40,
/// r 10, tests/volume_general.rs): errors at chord rel 1e-3 / 3e-4 / 1e-4 were
/// -4.4e-3 / -1.1e-3 / -3.3e-4 relative (linear in the chord), estimate / error ~3,
/// ~180 ms for revolve + measure (revolve measures once itself) at the chosen 1e-4.
fn tessellation_volume(body: &Body) -> std::result::Result<(f64, f64), String> {
    let [coarse, fine] = fallback_chords(body)?;
    let level = |chord: f64| -> std::result::Result<f64, String> {
        let m = crate::mesh::tessellate(body, chord).map_err(|e| e.to_string())?;
        let c = crate::mesh::check_mesh(&m);
        if !c.watertight {
            return Err(format!(
                "tessellation at chord {chord:.3e} mm is not watertight \
                 ({} boundary, {} non-manifold, {} misoriented edges)",
                c.boundary_edges, c.nonmanifold_edges, c.misoriented_edges
            ));
        }
        Ok(c.mesh_volume)
    };
    let [vc, vf] = std::thread::scope(|sc| {
        let hs = [coarse, fine].map(|c| sc.spawn(move || level(c)));
        hs.map(|h| {
            h.join()
                .unwrap_or_else(|_| Err("tessellation worker panicked".into()))
        })
    });
    let (vc, vf) = (vc?, vf?);
    Ok((vf, (vc - vf).abs()))
}

/// The (coarse, fine) chords of the tessellation fallback (see [`tessellation_volume`]).
fn fallback_chords(body: &Body) -> std::result::Result<[f64; 2], String> {
    let b = raw_bounds(&body.solid).map_err(|e| e.to_string())?;
    let diag = (0..3)
        .map(|i| (b.max[i] - b.min[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    let min_chord = tol::MIN_MESH_CHORD_ABS.max(diag * tol::MIN_MESH_CHORD_REL);
    let coarse = (diag * FALLBACK_CHORD_REL).max(FALLBACK_REFINE * min_chord);
    Ok([coarse, coarse / FALLBACK_REFINE])
}

/// Volume of `body`: the exact boundary integral about the first axis (Z, X, Y) whose
/// curved faces are all ruled along or perpendicular to it; else the exact
/// surface-of-revolution integral (cones/tori about one axis, cylinders about any);
/// otherwise (or if the exact evaluation fails) the tessellation estimate;
/// `Unavailable` if all fail.
fn volume(body: &Body) -> VolumeResult {
    let solid = &body.solid;
    let mut reasons = Vec::new();
    let mut eval_failed = false;
    for axis in Axis::ORDER {
        if let Err(e) = axis_supported(solid, axis) {
            reasons.push(format!("{axis:?}: {e}"));
            continue;
        }
        match exact_volume(solid, axis) {
            Ok((volume, error_estimate)) => {
                return VolumeResult::Value {
                    volume,
                    error_estimate,
                    method: axis.method(),
                };
            }
            Err(e) => {
                // Another axis would evaluate the same edge curves; go to the fallback.
                reasons.push(format!("{axis:?}: {e}"));
                eval_failed = true;
                break;
            }
        }
    }
    if !eval_failed {
        match revolution::revolution_volume(solid, VOLUME_CHORDS) {
            Ok((volume, error_estimate)) => {
                return VolumeResult::Value {
                    volume,
                    error_estimate,
                    method: VolumeMethod::BrepBoundaryIntegralRevolution,
                };
            }
            Err(e) => reasons.push(format!("revolution: {e}")),
        }
    }
    match tessellation_volume(body) {
        Ok((volume, error_estimate)) => VolumeResult::Value {
            volume,
            error_estimate,
            method: VolumeMethod::TessellationEstimate,
        },
        Err(e) => {
            reasons.push(format!("tessellation fallback: {e}"));
            VolumeResult::Unavailable {
                reason: reasons.join("; "),
            }
        }
    }
}

/// Number of B-rep faces (cheap; no geometry evaluation).
pub fn face_count(body: &Body) -> usize {
    body.solid.face_iter().count()
}

pub fn measure(body: &Body) -> Result<Measurements> {
    let s = &body.solid;
    guarded("measure", || -> Result<Measurements> {
        let shells = s.boundaries();
        Ok(Measurements {
            shell_count: shells.len(),
            all_shells_closed: shells
                .iter()
                .all(|sh| sh.shell_condition() == ShellCondition::Closed),
            face_count: s.face_iter().count(),
            edge_count: s.edge_iter().count() / 2,
            vertex_count: unique_vertices(s),
            bounds: raw_bounds(s)?,
            volume: volume(body),
        })
    })?
}

fn unique_vertices(s: &Solid) -> usize {
    s.vertex_iter()
        .map(|v| v.id())
        .collect::<std::collections::HashSet<_>>()
        .len()
}

/// Number of planar faces of `body` lying in `frame`'s plane with outward normal
/// equal to `frame.z`. Used to validate that a semantic face reference resolves to
/// real B-rep geometry (expected: exactly 1).
pub fn planar_faces_on(body: &Body, frame: &Frame) -> usize {
    let z = Vector3::new(frame.z[0], frame.z[1], frame.z[2]);
    let o = Point3::new(frame.origin[0], frame.origin[1], frame.origin[2]);
    body.solid
        .face_iter()
        .filter(|f| match f.surface() {
            Surface::Plane(pl) => {
                let mut n = pl.normal();
                if !f.orientation() {
                    n = -n;
                }
                (n - z).magnitude() < tol::FACE_MATCH
                    && (pl.origin() - o).dot(z).abs() < tol::FACE_MATCH
            }
            _ => false,
        })
        .count()
}

/// The plane of a planar B-rep face: a point on it and the outward normal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FacePlane {
    pub point: [f64; 3],
    pub normal: [f64; 3],
}

/// Planes of all planar faces of `body` (outward normals), deduplicated
/// (coplanar faces with the same normal appear once). Every planar face counts,
/// including those made by patterns, mirrors and chamfers, so a client can
/// apply the same "sketch plane holds a face of the target" rule as
/// regeneration ([`crate::coplanar_face_side`]).
pub fn face_planes(body: &Body) -> Vec<FacePlane> {
    let mut out: Vec<FacePlane> = Vec::new();
    for f in body.solid.face_iter() {
        let Surface::Plane(pl) = f.surface() else {
            continue;
        };
        let mut n = pl.normal();
        if !f.orientation() {
            n = -n;
        }
        let o = pl.origin();
        let fp = FacePlane {
            point: [o.x, o.y, o.z],
            normal: [n.x, n.y, n.z],
        };
        let same = |q: &FacePlane| {
            let d = (0..3)
                .map(|i| (q.normal[i] - fp.normal[i]).powi(2))
                .sum::<f64>();
            let off = (0..3)
                .map(|i| (q.point[i] - fp.point[i]) * fp.normal[i])
                .sum::<f64>();
            d.sqrt() < tol::FACE_MATCH && off.abs() < tol::FACE_MATCH
        };
        if !out.iter().any(same) {
            out.push(fp);
        }
    }
    out
}

/// An inner boundary loop (hole outline) of a planar face perpendicular to Z.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InnerLoop {
    /// Z of the face plane.
    pub z: f64,
    /// Area centroid of the loop (Green's theorem on exact edge curves, Richardson).
    pub centroid_xy: [f64; 2],
    /// Enclosed area (mm^2), same method.
    pub area: f64,
    /// Diameter of the circle with the same area (equals the diameter for circles).
    pub equivalent_diameter: f64,
}

/// Area and first moments (A, Mx, My) of a wire via chord sums on exact curve points.
fn wire_moments(w: &Wire, n: usize) -> Option<[f64; 3]> {
    wire_moments_by(w, n, |p| [p.x, p.y])
}

/// Signed area and first moments (A, M_s, M_t) of a wire after mapping each exact curve
/// point to 2D coordinates (s, t) with `to2d`; chord sums, `n` chords per edge.
fn wire_moments_by(w: &Wire, n: usize, to2d: impl Fn(Point3) -> [f64; 2]) -> Option<[f64; 3]> {
    let (mut a, mut mx, mut my) = (0.0, 0.0, 0.0);
    for e in w.edge_iter() {
        let c = e.oriented_curve();
        let (t0, t1) = c.range_tuple();
        let mut prev = to2d(eval_curve(&c, t0)?);
        for i in 1..=n {
            let p = to2d(eval_curve(&c, t0 + (t1 - t0) * i as f64 / n as f64)?);
            let cr = prev[0] * p[1] - p[0] * prev[1];
            a += cr;
            mx += (prev[0] + p[0]) * cr;
            my += (prev[1] + p[1]) * cr;
            prev = p;
        }
    }
    Some([a / 2.0, mx / 6.0, my / 6.0])
}

/// Inner loops of all Z-perpendicular planar faces, measured on exact edge curves.
/// A loop is "inner" when its signed area about the face normal is negative.
pub fn z_face_inner_loops(body: &Body) -> Result<Vec<InnerLoop>> {
    let fail = || KernelError::Internal("intersection curve evaluation failed".into());
    guarded("inner loops", || -> Result<Vec<InnerLoop>> {
        let mut out = Vec::new();
        for f in body.solid.face_iter() {
            let Surface::Plane(pl) = f.surface() else {
                continue;
            };
            let mut n = pl.normal();
            if !f.orientation() {
                n = -n;
            }
            if n.x.abs() + n.y.abs() > 1e-12 {
                continue;
            }
            for w in f.boundaries() {
                let m1 = wire_moments(&w, VOLUME_CHORDS).ok_or_else(fail)?;
                if m1[0] * n.z >= 0.0 {
                    continue;
                }
                let m2 = wire_moments(&w, 2 * VOLUME_CHORDS).ok_or_else(fail)?;
                let r: Vec<f64> = (0..3).map(|i| (4.0 * m2[i] - m1[i]) / 3.0).collect();
                let area = r[0].abs();
                out.push(InnerLoop {
                    z: pl.origin().z,
                    centroid_xy: [r[1] / r[0], r[2] / r[0]],
                    area,
                    equivalent_diameter: 2.0 * (area / std::f64::consts::PI).sqrt(),
                });
            }
        }
        out.sort_by(|a, b| a.z.total_cmp(&b.z));
        Ok(out)
    })?
}
