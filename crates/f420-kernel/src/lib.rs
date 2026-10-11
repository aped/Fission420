//! Kernel adapter over the Truck B-rep kernel.
//!
//! This crate is the only place that touches Truck types. Everything above it
//! (document, commands, UI) sees [`Body`], [`Frame`], [`Profile`], and plain
//! measurement / mesh structs. See `docs/adr/0001-kernel.md` and
//! `docs/TOLERANCES.md` for the tolerance policy.

mod build;
mod exact;
mod measure;
mod mesh;
mod step;

pub use build::{cut_blind, cut_through_all, extrude, revolve};
pub use measure::{
    Bounds, InnerLoop, Measurements, VolumeMethod, VolumeResult, face_count, measure,
    planar_faces_on, z_face_inner_loops,
};
pub use mesh::{MeshCheck, TriMesh, check_mesh, tessellate, tessellate_display, write_stl_binary};
pub use step::export_step;

use serde::{Deserialize, Serialize};
use truck_modeling::Solid;

/// Tolerance policy (millimetres). Kept in one place; see docs/TOLERANCES.md.
pub mod tol {
    /// Linear tolerance used by Truck internally (`truck_base::tolerance::TOLERANCE`).
    pub const KERNEL_LINEAR: f64 = 1.0e-6;
    /// Tolerance handed to `truck_shapeops::and` (intersection-curve polyline accuracy).
    pub const BOOLEAN: f64 = 0.05;
    /// Coplanarity / direction tolerance for matching semantic face references.
    pub const FACE_MATCH: f64 = 1.0e-7;
    /// Smallest accepted revolve angle (rad).
    pub const MIN_ANGLE: f64 = 1.0e-6;
    /// Smallest accepted feature length; anything below this is rejected as degenerate.
    pub const MIN_LENGTH: f64 = 1.0e-3;
    /// Extra length a through-all cutter extends past the body on both sides, as a
    /// fraction of the body's bounding diagonal (plus an absolute floor). This avoids
    /// coplanar cutter caps, which Truck 0.6 booleans reject (see ADR 0001).
    pub const THROUGH_ALL_MARGIN_REL: f64 = 0.05;
    pub const THROUGH_ALL_MARGIN_ABS: f64 = 1.0;
    /// Smallest accepted chordal tolerance: max(ABS, REL * bounding diagonal).
    pub const MIN_MESH_CHORD_ABS: f64 = 1.0e-3;
    pub const MIN_MESH_CHORD_REL: f64 = 1.0e-5;
    /// Default chordal tolerance for display/STL tessellation.
    pub const MESH_CHORD: f64 = 0.01;
    /// Largest accepted difference (mm) between an arc's start radius and its end
    /// radius in a [`crate::Profile::Loop`]. The arc curve is built from the start
    /// radius with its last control point placed exactly on the end vertex, so a
    /// mismatch distorts only the last span by at most this much; keeping it at
    /// Truck's own linear tolerance keeps the edge consistent with its vertices.
    /// Upstream sketches are solved to `max(1e-9, 1e-12 * scale)` mm, far below.
    pub const ARC_RADIUS_MATCH: f64 = KERNEL_LINEAR;
}

/// Errors from geometric operations. Always recoverable; never panics to the caller.
#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
pub enum KernelError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("boolean operation failed: {0}")]
    BooleanFailed(String),
    #[error("result is not a valid solid: {0}")]
    InvalidResult(String),
    #[error("kernel internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, KernelError>;

/// Right-handed orthonormal placement frame. `z` is the plane normal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub origin: [f64; 3],
    pub x: [f64; 3],
    pub y: [f64; 3],
    pub z: [f64; 3],
}

impl Frame {
    /// World XZ plane: local x = world X, local y = world Z, normal = -Y (right-handed).
    pub const XZ: Frame = Frame {
        origin: [0.0, 0.0, 0.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [0.0, -1.0, 0.0],
    };

    pub const XY: Frame = Frame {
        origin: [0.0, 0.0, 0.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 1.0, 0.0],
        z: [0.0, 0.0, 1.0],
    };

    /// Frame translated along its own normal.
    pub fn offset(&self, d: f64) -> Frame {
        let mut f = *self;
        for i in 0..3 {
            f.origin[i] += self.z[i] * d;
        }
        f
    }

    /// Local 2D point to world.
    pub fn to_world(&self, p: [f64; 2]) -> [f64; 3] {
        let mut w = self.origin;
        for (i, wi) in w.iter_mut().enumerate() {
            *wi += self.x[i] * p[0] + self.y[i] * p[1];
        }
        w
    }

    fn validate(&self) -> Result<()> {
        let all = self
            .origin
            .iter()
            .chain(&self.x)
            .chain(&self.y)
            .chain(&self.z);
        if all.clone().any(|v| !v.is_finite()) {
            return Err(KernelError::InvalidInput(
                "frame has non-finite values".into(),
            ));
        }
        let dot = |a: &[f64; 3], b: &[f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let unit = |a: &[f64; 3]| (dot(a, a) - 1.0).abs() < 1e-9;
        if !(unit(&self.x) && unit(&self.y) && unit(&self.z))
            || dot(&self.x, &self.y).abs() > 1e-9
            || dot(&self.x, &self.z).abs() > 1e-9
            || dot(&self.y, &self.z).abs() > 1e-9
        {
            return Err(KernelError::InvalidInput("frame is not orthonormal".into()));
        }
        Ok(())
    }
}

/// One segment of a [`Profile::Loop`]. Each segment ends where the next one
/// starts (the last wraps to the first).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LoopSegment {
    Line {
        start: [f64; 2],
    },
    /// Circular arc about `center` with radius `|start - center|`, traversed
    /// counter-clockwise when `ccw` (clockwise otherwise). Sweep in (0, 2*pi).
    Arc {
        start: [f64; 2],
        center: [f64; 2],
        ccw: bool,
    },
}

impl LoopSegment {
    pub fn start(&self) -> [f64; 2] {
        match self {
            LoopSegment::Line { start } | LoopSegment::Arc { start, .. } => *start,
        }
    }
}

/// Signed sweep (rad) of an arc from `start` to `end` about `center`: in (0, 2*pi]
/// when `ccw`, in [-2*pi, 0) otherwise (a zero angular difference counts as a full
/// turn; callers reject coincident endpoints first).
pub(crate) fn arc_signed_sweep(start: [f64; 2], end: [f64; 2], center: [f64; 2], ccw: bool) -> f64 {
    let a0 = (start[1] - center[1]).atan2(start[0] - center[0]);
    let a1 = (end[1] - center[1]).atan2(end[0] - center[0]);
    let tau = 2.0 * std::f64::consts::PI;
    let d = if ccw { a1 - a0 } else { a0 - a1 };
    let mut s = d.rem_euclid(tau);
    if s <= 0.0 {
        s = tau;
    }
    if ccw { s } else { -s }
}

/// Exact signed area of a loop: shoelace over the chords plus, for each arc, the
/// signed circular-segment area (r^2 / 2)(theta - sin theta).
pub(crate) fn loop_signed_area(segs: &[LoopSegment]) -> f64 {
    let n = segs.len();
    let mut a = 0.0;
    for i in 0..n {
        let p = segs[i].start();
        let q = segs[(i + 1) % n].start();
        a += 0.5 * (p[0] * q[1] - q[0] * p[1]);
        if let LoopSegment::Arc { center, ccw, .. } = &segs[i] {
            let r = (p[0] - center[0]).hypot(p[1] - center[1]);
            let t = arc_signed_sweep(p, q, *center, *ccw);
            a += 0.5 * r * r * (t - t.sin());
        }
    }
    a
}

/// Closed planar profile in frame-local 2D coordinates (mm).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Profile {
    /// Simple polygon, counter-clockwise.
    Polygon(Vec<[f64; 2]>),
    Circle {
        center: [f64; 2],
        radius: f64,
    },
    /// Closed loop of lines and circular arcs, counter-clockwise overall.
    /// Simplicity (no self-intersection) is the caller's responsibility
    /// (`f420_sketch` profile extraction checks it).
    Loop(Vec<LoopSegment>),
}

impl Profile {
    fn validate(&self) -> Result<()> {
        match self {
            Profile::Polygon(pts) => {
                if pts.len() < 3 {
                    return Err(KernelError::InvalidInput(
                        "polygon needs >= 3 points".into(),
                    ));
                }
                if pts.iter().flatten().any(|v| !v.is_finite()) {
                    return Err(KernelError::InvalidInput(
                        "polygon has non-finite point".into(),
                    ));
                }
                let n = pts.len();
                let mut area2 = 0.0;
                for i in 0..n {
                    let (a, b) = (pts[i], pts[(i + 1) % n]);
                    if (a[0] - b[0]).hypot(a[1] - b[1]) < tol::MIN_LENGTH {
                        return Err(KernelError::InvalidInput(
                            "polygon has a degenerate edge".into(),
                        ));
                    }
                    area2 += a[0] * b[1] - b[0] * a[1];
                }
                if area2 <= 0.0 {
                    return Err(KernelError::InvalidInput(
                        "polygon must be counter-clockwise with positive area".into(),
                    ));
                }
                Ok(())
            }
            Profile::Loop(segs) => {
                if segs.len() < 2 {
                    return Err(KernelError::InvalidInput("loop needs >= 2 segments".into()));
                }
                let n = segs.len();
                for (i, seg) in segs.iter().enumerate() {
                    let p = seg.start();
                    let q = segs[(i + 1) % n].start();
                    let center = match seg {
                        LoopSegment::Arc { center, .. } => Some(*center),
                        LoopSegment::Line { .. } => None,
                    };
                    if p.iter()
                        .chain(center.iter().flatten())
                        .any(|v| !v.is_finite())
                    {
                        return Err(KernelError::InvalidInput(
                            "loop has a non-finite coordinate".into(),
                        ));
                    }
                    if (p[0] - q[0]).hypot(p[1] - q[1]) < tol::MIN_LENGTH {
                        return Err(KernelError::InvalidInput(format!(
                            "loop segment {i} has a degenerate chord"
                        )));
                    }
                    if let Some(c) = center {
                        let r0 = (p[0] - c[0]).hypot(p[1] - c[1]);
                        let r1 = (q[0] - c[0]).hypot(q[1] - c[1]);
                        if r0 < tol::MIN_LENGTH {
                            return Err(KernelError::InvalidInput(format!(
                                "loop arc {i} radius too small"
                            )));
                        }
                        if (r0 - r1).abs() > tol::ARC_RADIUS_MATCH {
                            return Err(KernelError::InvalidInput(format!(
                                "loop arc {i} end is not on its circle (radius {r0} vs {r1} mm)"
                            )));
                        }
                    }
                }
                if loop_signed_area(segs) <= 0.0 {
                    return Err(KernelError::InvalidInput(
                        "loop must be counter-clockwise with positive area".into(),
                    ));
                }
                Ok(())
            }
            Profile::Circle { center, radius } => {
                if !(center[0].is_finite() && center[1].is_finite() && radius.is_finite()) {
                    return Err(KernelError::InvalidInput(
                        "circle has non-finite values".into(),
                    ));
                }
                if *radius < tol::MIN_LENGTH {
                    return Err(KernelError::InvalidInput("circle radius too small".into()));
                }
                Ok(())
            }
        }
    }
}

/// A B-rep body. The Truck solid is the source of truth; meshes are derived.
///
/// `Clone` is shallow: Truck shares vertex points, edge curves and face
/// surfaces behind `Arc<Mutex<_>>`. Use [`Body::deep_copy`] before handing a
/// body to another thread that evaluates it concurrently.
#[derive(Debug, Clone)]
pub struct Body {
    pub(crate) solid: Solid,
}

impl Body {
    /// Independent copy sharing no geometry with `self` (Truck compress →
    /// extract allocates fresh topology and geometry), safe to evaluate on
    /// another thread while `self` is in use.
    pub fn deep_copy(&self) -> Result<Body> {
        let compressed = guarded("compress body", || self.solid.compress())?;
        let solid = Solid::extract(compressed)
            .map_err(|e| KernelError::Internal(format!("extract body: {e}")))?;
        Ok(Body { solid })
    }
}

/// Run a Truck call, converting panics inside the kernel into errors.
/// Truck 0.6 has `unwrap()`s on some numerical paths (e.g. intersection-curve
/// projection); a modeling failure must never take the application down.
pub(crate) fn guarded<T>(what: &str, f: impl FnOnce() -> T) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .map_err(|_| KernelError::Internal(format!("{what}: kernel panicked")))
}
