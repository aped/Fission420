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
            other => {
                let ((u0, u1), (v0, v1)) = match &other {
                    Surface::NurbsSurface(s) => s.range_tuple(),
                    Surface::BSplineSurface(s) => s.range_tuple(),
                    _ => return Err("unsupported surface type for Z boundary integral".into()),
                };
                const K: usize = 16;
                for i in 0..=K {
                    for j in 0..=K {
                        let nn = other.normal(
                            u0 + (u1 - u0) * i as f64 / K as f64,
                            v0 + (v1 - v0) * j as f64 / K as f64,
                        );
                        if nn.z.abs() > 1e-12 {
                            return Err("curved face is not Z-ruled (n_z != 0)".into());
                        }
                    }
                }
            }
        }
    }
    Ok(v)
}

fn volume(solid: &Solid) -> VolumeResult {
    let r = (|| -> std::result::Result<(f64, f64), String> {
        let n = VOLUME_CHORDS;
        let a = volume_z(solid, n / 2)?;
        let b = volume_z(solid, n)?;
        let c = volume_z(solid, 2 * n)?;
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
