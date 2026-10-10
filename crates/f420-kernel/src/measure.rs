//! Geometric measurement on the B-rep (not on a mesh).
use crate::{Body, Frame, KernelError, Result, guarded, tol};
use serde::{Deserialize, Serialize};
use truck_modeling::*;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VolumeMethod {
    /// Divergence theorem with F = (0, 0, z): V = sum over faces of the integral of
    /// z * n_z dA. Faces must be planes or surfaces with n_z == 0 (Z-ruled walls).
    /// Planar face integrals become Green's-theorem line integrals over the exact
    /// B-rep edge curves, evaluated by dense chord sums on exact curve points with
    /// Richardson extrapolation. Not a mesh volume.
    BrepBoundaryIntegralZ,
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

/// Bounds from vertices plus dense samples of every edge curve. Exact when extrema
/// lie on vertices or straight edges (true for plates with interior holes); otherwise
/// within the chord sampling error of curved edges.
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
        for i in 0..=BOUNDS_SAMPLES {
            let t = t0 + (t1 - t0) * i as f64 / BOUNDS_SAMPLES as f64;
            let p = eval_curve(&c, t).ok_or_else(|| {
                KernelError::Internal("intersection curve evaluation failed".into())
            })?;
            add(p);
        }
    }
    if b.min.iter().chain(&b.max).any(|v| !v.is_finite()) {
        return Err(KernelError::InvalidResult(
            "empty or non-finite bounds".into(),
        ));
    }
    Ok(b)
}

/// 1/2 * closed-loop sum of (x dy - y dx) over a wire, `n` chords per edge.
fn wire_area_xy(w: &Wire, n: usize) -> Option<f64> {
    let mut a = 0.0;
    for e in w.edge_iter() {
        let c = e.oriented_curve();
        let (t0, t1) = c.range_tuple();
        let mut prev = eval_curve(&c, t0)?;
        for i in 1..=n {
            let p = eval_curve(&c, t0 + (t1 - t0) * i as f64 / n as f64)?;
            a += prev.x * p.y - p.x * prev.y;
            prev = p;
        }
    }
    Some(a / 2.0)
}

enum ZClass {
    /// n_z == 0 everywhere: contributes nothing to the integral of z n_z dA.
    Vertical,
    /// Lies in the plane z = c with normal +-Z (e.g. a revolved annulus).
    Horizontal(f64),
}

fn bound(b: std::ops::Bound<f64>) -> Option<f64> {
    match b {
        std::ops::Bound::Included(x) | std::ops::Bound::Excluded(x) => Some(x),
        std::ops::Bound::Unbounded => None,
    }
}

/// Classify a non-`Plane` surface by dense sampling of its (finite) parameter domain.
/// Anything that is neither Z-ruled nor a horizontal plane is unsupported.
fn classify_by_sampling(s: &Surface) -> std::result::Result<ZClass, String> {
    let ((a0, a1), (b0, b1)) = s.parameter_range();
    let (Some(u0), Some(u1), Some(v0), Some(v1)) = (bound(a0), bound(a1), bound(b0), bound(b1))
    else {
        return Err("unbounded surface parameter range".into());
    };
    const K: usize = 16;
    let (mut vertical, mut horizontal) = (true, true);
    let z0 = s.subs(u0, v0).z;
    for i in 0..=K {
        for j in 0..=K {
            let (u, v) = (
                u0 + (u1 - u0) * i as f64 / K as f64,
                v0 + (v1 - v0) * j as f64 / K as f64,
            );
            let nn = s.normal(u, v);
            if !nn.x.is_finite() || !nn.z.is_finite() {
                continue; // degenerate point (e.g. on the axis of a revolution); judged by neighbours
            }
            vertical &= nn.z.abs() <= 1e-12;
            horizontal &= nn.x.abs() + nn.y.abs() <= 1e-12 && (s.subs(u, v).z - z0).abs() <= 1e-9;
        }
    }
    match (vertical, horizontal) {
        (true, _) => Ok(ZClass::Vertical),
        (_, true) => Ok(ZClass::Horizontal(z0)),
        _ => Err("curved face is neither Z-ruled nor horizontal".into()),
    }
}

fn volume_z(solid: &Solid, n: usize) -> std::result::Result<f64, String> {
    let mut v = 0.0;
    for f in solid.face_iter() {
        match f.surface() {
            Surface::Plane(pl) => {
                let mut nrm = pl.normal();
                if !f.orientation() {
                    nrm = -nrm;
                }
                if nrm.x.abs() + nrm.y.abs() < 1e-12 {
                    let z = pl.origin().z;
                    let mut area = 0.0;
                    for w in f.boundaries() {
                        area +=
                            wire_area_xy(&w, n).ok_or("intersection curve evaluation failed")?;
                    }
                    v += z * area;
                } else if nrm.z.abs() > 1e-12 {
                    return Err("planar face not parallel or perpendicular to Z".into());
                }
            }
            other => match classify_by_sampling(&other)? {
                ZClass::Vertical => {}
                ZClass::Horizontal(z) => {
                    let mut area = 0.0;
                    for w in f.boundaries() {
                        area +=
                            wire_area_xy(&w, n).ok_or("intersection curve evaluation failed")?;
                    }
                    v += z * area;
                }
            },
        }
    }
    Ok(v)
}

fn volume(solid: &Solid) -> VolumeResult {
    let r = (|| -> std::result::Result<(f64, f64), String> {
        let n = VOLUME_CHORDS;
        // The three refinement levels are independent; evaluate them concurrently.
        let [a, b, c] = std::thread::scope(|sc| {
            let hs = [n / 2, n, 2 * n].map(|k| sc.spawn(move || volume_z(solid, k)));
            hs.map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("volume worker panicked".into()))
            })
        });
        let (a, b, c) = (a?, b?, c?);
        let r1 = (4.0 * b - a) / 3.0;
        let r2 = (4.0 * c - b) / 3.0;
        Ok((r2, (r2 - r1).abs()))
    })();
    match r {
        Ok((volume, error_estimate)) => VolumeResult::Value {
            volume,
            error_estimate,
            method: VolumeMethod::BrepBoundaryIntegralZ,
        },
        Err(reason) => VolumeResult::Unavailable { reason },
    }
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
            volume: volume(s),
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
    let (mut a, mut mx, mut my) = (0.0, 0.0, 0.0);
    for e in w.edge_iter() {
        let c = e.oriented_curve();
        let (t0, t1) = c.range_tuple();
        let mut prev = eval_curve(&c, t0)?;
        for i in 1..=n {
            let p = eval_curve(&c, t0 + (t1 - t0) * i as f64 / n as f64)?;
            let cr = prev.x * p.y - p.x * prev.y;
            a += cr;
            mx += (prev.x + p.x) * cr;
            my += (prev.y + p.y) * cr;
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
