//! Modeled screw threads as real solid geometry (run 6, ADR 0011): a threaded
//! rod (external thread) and a square nut (internal thread), **faceted** with a
//! declared chord tolerance (the helicoid flanks are not rational surfaces;
//! Truck has no exact helical sweep). Every face is an exact plane through
//! points of the ideal thread surface; the facets are labelled as such
//! ([`ThreadReport`]), never presented as exact helicoids.
//!
//! Test profile (stated, nonproprietary; not a certified standard thread):
//! symmetric 60° V, fundamental triangle height `H = p·√3/2`, crest flat
//! `p/8`, root flat `p/4`, thread depth `5H/8`; over one pitch along the axis
//! the radius is: root flat `[0, p/4]` at the core radius, flank up over
//! `5p/16`, crest flat `p/8` at the major radius, flank down over `5p/16`.
//!
//! Construction (no booleans): the thread surface is a helical ribbon of
//! cells between consecutive angle samples (`n` per turn) and consecutive
//! profile vertices; each cell is split into two planar triangles. The first
//! and last turn are a lead-in / runout: the ridge height grows linearly from
//! zero over one turn (and shrinks over the last), so at both ends the
//! surface meets the core cylinder and closes with faceted walls (planar
//! quads between vertical lines) and flat end caps. Handedness: a left-hand
//! thread is the mirror image (angles clockwise). Length must be a whole
//! number (≥ 3) of pitches.
//!
//! Chord tolerance: `n = ceil(π / acos(1 − tol / r_major))` angle samples per
//! turn (at least 24): the sagitta of the major circle's chords is ≤ `tol`.
//! The report gives the measured maximum distance between the ideal surface
//! (at cell centres) and the facets.
use crate::{Body, Frame, KernelError, Result, guarded, tol};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::f64::consts::{PI, TAU};
use truck_modeling::*;
use truck_topology::compress::{
    CompressedEdge, CompressedEdgeIndex, CompressedFace, CompressedShell, CompressedSolid,
};

/// Thread parameters (mm).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ThreadSpec {
    /// Nominal (major) diameter of the external thread.
    pub major_diameter: f64,
    pub pitch: f64,
    /// Threaded length: a whole number (≥ 3) of pitches, including the
    /// one-turn lead-in and runout.
    pub length: f64,
    pub left_hand: bool,
    /// Largest accepted chord sagitta of the facets (mm).
    pub chord_tol: f64,
}

/// What was built (for the UI / tests: the geometry is faceted).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadReport {
    pub segments_per_turn: usize,
    pub faces: usize,
    /// Thread depth `5H/8` and the core (minor) diameter of the profile.
    pub depth: f64,
    pub minor_diameter: f64,
    /// Largest distance from the ideal surface at cell centres to the facets.
    pub max_facet_deviation: f64,
    pub faceted: bool,
}

fn refuse<T>(m: impl Into<String>) -> Result<T> {
    Err(KernelError::InvalidInput(format!("thread: {}", m.into())))
}

/// Axial positions of the four profile vertices within one pitch (fractions
/// of `p`) and whether each is on the ridge (major radius) side.
const PROFILE: [(f64, bool); 4] = [(0.0, false), (0.25, false), (0.5625, true), (0.6875, true)];

struct Grid {
    p: f64,
    n: usize,
    turns: usize,
    r_core: f64,
    r_ridge: f64,
    sign: f64,
}

impl Grid {
    /// Ideal surface point at (fractional) row `t` and profile vertex `j`
    /// (local frame: axis z, z = 0 at the start).
    fn point(&self, t: f64, j: usize) -> [f64; 3] {
        let (s, ridge) = PROFILE[j];
        let lam = {
            let m = ((self.turns - 1) * self.n) as f64;
            let n = self.n as f64;
            (t / n).min((m - t) / n).clamp(0.0, 1.0)
        };
        let r = if ridge {
            self.r_core + lam * (self.r_ridge - self.r_core)
        } else {
            self.r_core
        };
        let th = self.sign * TAU * t / self.n as f64;
        [r * th.cos(), r * th.sin(), self.p * (t / self.n as f64 + s)]
    }
}

/// Faces of the helical solid of revolution-like rod between z = 0 and
/// z = length (local coordinates), as polygons of vertex keys, outward for a
/// rod. Keys: (row, j) for grid points, (-1, i) bottom ring, (-2, i) top ring.
/// Polygon faces (vertex keys) and the key -> local point table.
type FacesAndPoints = (Vec<Vec<(i64, usize)>>, HashMap<(i64, usize), [f64; 3]>);

fn rod_faces(g: &Grid) -> FacesAndPoints {
    let n = g.n;
    let m = (g.turns - 1) * n;
    let mut pts: HashMap<(i64, usize), [f64; 3]> = HashMap::new();
    let key = |i: usize, j: usize| -> (i64, usize) {
        // Profile vertex 4 of row i is vertex 0 of row i + n.
        if j == 4 {
            (i as i64 + n as i64, 0)
        } else {
            (i as i64, j)
        }
    };
    for i in 0..=m {
        for j in 0..4 {
            pts.insert((i as i64, j), g.point(i as f64, j));
        }
    }
    for i in m + 1..=m + n {
        pts.insert((i as i64, 0), g.point(i as f64, 0));
    }
    let z1 = g.p * g.turns as f64;
    for i in 0..n {
        let th = g.sign * TAU * i as f64 / n as f64;
        pts.insert((-1, i), [g.r_core * th.cos(), g.r_core * th.sin(), 0.0]);
        pts.insert((-2, i), [g.r_core * th.cos(), g.r_core * th.sin(), z1]);
    }
    let mut faces: Vec<Vec<(i64, usize)>> = Vec::new();
    // Ribbon cells: two triangles each (diagonal (i,j)-(i+1,j+1)).
    for i in 0..m {
        for j in 0..4 {
            let (a, b, c, d) = (key(i, j), key(i + 1, j), key(i + 1, j + 1), key(i, j + 1));
            faces.push(vec![a, b, c]);
            faces.push(vec![a, c, d]);
        }
    }
    // Bottom wall: B_i, B_{i+1}, P(i+1,0), P(i,0); B_0 = P(0,0).
    let b = |i: usize| {
        if i.is_multiple_of(n) {
            (0i64, 0usize)
        } else {
            (-1, i % n)
        }
    };
    for i in 0..n {
        let mut f = vec![b(i), b(i + 1)];
        if i == n - 1 {
            // The seam at angle 0: up the vertical chain of row 0.
            f.extend([(0, 1), (0, 2), (0, 3), (n as i64, 0)]);
        } else {
            f.push((i as i64 + 1, 0));
        }
        if i != 0 {
            f.push((i as i64, 0));
        }
        faces.push(f);
    }
    // Top wall: P(i,0), P(i+1,0), T_{i+1}, T_i for i in [m, m+n); T_m = P(m+n, 0).
    let t = |i: usize| {
        if (i - m).is_multiple_of(n) {
            ((m + n) as i64, 0usize)
        } else {
            (-2, (i - m) % n)
        }
    };
    for i in m..m + n {
        let mut f = vec![(i as i64, 0), (i as i64 + 1, 0)];
        if i + 1 != m + n {
            f.push(t(i + 1));
        }
        f.push(t(i));
        if i == m {
            f.extend([(m as i64, 3), (m as i64, 2), (m as i64, 1)]);
        }
        faces.push(f);
    }
    // Caps: bottom (outward −z: clockwise seen from +z), top (counter-clockwise).
    faces.push((0..n).rev().map(b).collect());
    faces.push((m..m + n).map(t).collect());
    if g.sign < 0.0 {
        // Mirror image: every loop reverses to stay outward.
        for f in &mut faces {
            f.reverse();
        }
    }
    (faces, pts)
}

/// Build a shell from polygon faces (vertex keys, outward counter-clockwise),
/// mapping local points through `frame`; `flip` reverses every face.
fn shell_from(
    faces: &[Vec<(i64, usize)>],
    pts: &HashMap<(i64, usize), [f64; 3]>,
    frame: &Frame,
    flip: bool,
    vids: &mut HashMap<(i64, usize), usize>,
    sh: &mut CompressedShell<Point3, Curve, Surface>,
    eids: &mut HashMap<(usize, usize), usize>,
) -> Result<()> {
    let world = |p: [f64; 3]| {
        let q = frame.offset(p[2]).to_world([p[0], p[1]]);
        Point3::new(q[0], q[1], q[2])
    };
    for f in faces {
        let mut loop_ids = Vec::with_capacity(f.len());
        for k in f {
            let id = match vids.get(k) {
                Some(id) => *id,
                None => {
                    let p = pts
                        .get(k)
                        .ok_or_else(|| KernelError::Internal(format!("thread: vertex {k:?}")))?;
                    sh.vertices.push(world(*p));
                    vids.insert(*k, sh.vertices.len() - 1);
                    sh.vertices.len() - 1
                }
            };
            loop_ids.push(id);
        }
        if flip {
            loop_ids.reverse();
        }
        let mut wire = Vec::with_capacity(loop_ids.len());
        for k in 0..loop_ids.len() {
            let (a, b) = (loop_ids[k], loop_ids[(k + 1) % loop_ids.len()]);
            let (lo, hi) = (a.min(b), a.max(b));
            let e = *eids.entry((lo, hi)).or_insert_with(|| {
                sh.edges.push(CompressedEdge {
                    vertices: (lo, hi),
                    curve: Curve::Line(Line(sh.vertices[lo], sh.vertices[hi])),
                });
                sh.edges.len() - 1
            });
            wire.push(CompressedEdgeIndex {
                index: e,
                orientation: a == lo,
            });
        }
        // Plane through the polygon (Newell normal), outward = loop CCW.
        let v: Vec<Point3> = loop_ids.iter().map(|i| sh.vertices[*i]).collect();
        let mut nrm = Vector3::zero();
        for k in 0..v.len() {
            let (a, b) = (v[k], v[(k + 1) % v.len()]);
            nrm += Vector3::new(
                (a.y - b.y) * (a.z + b.z),
                (a.z - b.z) * (a.x + b.x),
                (a.x - b.x) * (a.y + b.y),
            );
        }
        if nrm.magnitude() < 1e-14 {
            return Err(KernelError::Internal("thread: degenerate facet".into()));
        }
        let nrm = nrm.normalize();
        let x = (v[1] - v[0]).normalize();
        let y = nrm.cross(x);
        sh.faces.push(CompressedFace {
            boundaries: vec![wire],
            orientation: true,
            surface: Surface::Plane(Plane::new(v[0], v[0] + x, v[0] + y)),
        });
    }
    Ok(())
}

fn grid(spec: &ThreadSpec, clearance: f64) -> Result<(Grid, usize)> {
    let ThreadSpec {
        major_diameter: d,
        pitch: p,
        length: l,
        chord_tol: ctol,
        ..
    } = *spec;
    if ![d, p, l, ctol, clearance].iter().all(|v| v.is_finite()) {
        return refuse("non-finite parameter");
    }
    if !(p >= 0.05 && d > 2.0 * p && ctol > 0.0) {
        return refuse(format!(
            "pitch must be at least 0.05 mm and the diameter more than twice the pitch \
             (d = {d}, p = {p}); chord tolerance > 0"
        ));
    }
    let turns = (l / p).round();
    if (turns * p - l).abs() > tol::MIN_LENGTH || turns < 3.0 {
        return refuse(format!(
            "length {l} mm must be a whole number (at least 3) of pitches ({p} mm)"
        ));
    }
    let depth = 5.0 * p * 3f64.sqrt() / 16.0;
    let r_ridge = d / 2.0 + clearance;
    let r_core = d / 2.0 - depth + clearance;
    let n = ((PI / (1.0 - ctol / r_ridge).clamp(-1.0, 1.0).acos()).ceil() as usize).max(24);
    if n > 720 {
        return refuse(format!(
            "chord tolerance {ctol} mm needs {n} segments per turn (limit 720)"
        ));
    }
    Ok((
        Grid {
            p,
            n,
            turns: turns as usize,
            r_core,
            r_ridge,
            sign: if spec.left_hand { -1.0 } else { 1.0 },
        },
        n,
    ))
}

/// Largest distance between the ideal surface at the cell centres and the
/// facet plane of the cell (sampled over every cell of the first full turn
/// after the lead-in, which has the full profile).
fn deviation(g: &Grid) -> f64 {
    let mut worst: f64 = 0.0;
    let n = g.n;
    for i in n..(2 * n).min((g.turns - 1) * n) {
        for j in 0..3 {
            let c = g.point(i as f64 + 0.5, j);
            let c2 = g.point(i as f64 + 0.5, j + 1);
            let ideal = [
                0.5 * (c[0] + c2[0]),
                0.5 * (c[1] + c2[1]),
                0.5 * (c[2] + c2[2]),
            ];
            // Facet (i,j),(i+1,j),(i+1,j+1): its plane distance.
            let (a, b, d) = (
                g.point(i as f64, j),
                g.point(i as f64 + 1.0, j),
                g.point(i as f64 + 1.0, j + 1),
            );
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [d[0] - a[0], d[1] - a[1], d[2] - a[2]];
            let nn = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let l = (nn[0] * nn[0] + nn[1] * nn[1] + nn[2] * nn[2]).sqrt();
            let w = [ideal[0] - a[0], ideal[1] - a[1], ideal[2] - a[2]];
            worst = worst.max(((w[0] * nn[0] + w[1] * nn[1] + w[2] * nn[2]) / l).abs());
        }
    }
    worst
}

fn finish(sh: CompressedShell<Point3, Curve, Surface>, what: &str) -> Result<Body> {
    let solid = guarded(what, || {
        Solid::extract(CompressedSolid {
            boundaries: vec![sh],
        })
    })?
    .map_err(|e| KernelError::Internal(format!("{what}: {e}")))?;
    let body = Body { solid };
    let m = crate::measure(&body)?;
    match m.volume {
        crate::VolumeResult::Value { volume, .. }
            if volume > 0.0 && m.shell_count == 1 && m.all_shells_closed =>
        {
            Ok(body)
        }
        other => Err(KernelError::Internal(format!(
            "{what}: not a closed positive solid ({other:?})"
        ))),
    }
}

/// Threaded rod (external thread) along `frame.z` from `frame`'s plane, axis
/// through `frame.origin`. Faceted (see the module docs).
pub fn threaded_rod(frame: &Frame, spec: &ThreadSpec) -> Result<(Body, ThreadReport)> {
    frame.validate()?;
    let (g, n) = grid(spec, 0.0)?;
    let (faces, pts) = rod_faces(&g);
    let mut sh = CompressedShell {
        vertices: Vec::new(),
        edges: Vec::new(),
        faces: Vec::new(),
    };
    shell_from(
        &faces,
        &pts,
        frame,
        false,
        &mut HashMap::new(),
        &mut sh,
        &mut HashMap::new(),
    )?;
    let count = sh.faces.len();
    let body = finish(sh, "threaded rod")?;
    Ok((
        body,
        ThreadReport {
            segments_per_turn: n,
            faces: count,
            depth: g.r_ridge - g.r_core,
            minor_diameter: 2.0 * g.r_core,
            max_facet_deviation: deviation(&g),
            faceted: true,
        },
    ))
}

/// Square nut `width` x `width` x `spec.length` with an internal thread
/// matching [`threaded_rod`] of the same spec: the thread void is the rod
/// shape offset radially by `clearance` (mm, ≥ 0). Faceted.
pub fn threaded_nut(
    frame: &Frame,
    spec: &ThreadSpec,
    width: f64,
    clearance: f64,
) -> Result<(Body, ThreadReport)> {
    frame.validate()?;
    if clearance.is_nan() || clearance < 0.0 {
        return refuse("clearance must be ≥ 0");
    }
    let (g, n) = grid(spec, clearance)?;
    if !(width.is_finite() && width / 2.0 > g.r_ridge + 1.0) {
        return refuse(format!(
            "the nut must be wider than the thread's root diameter + 2 mm ({:.3} mm)",
            2.0 * g.r_ridge + 2.0
        ));
    }
    let (mut faces, mut pts) = rod_faces(&g);
    // The void's caps become the inner loops of the nut's end faces: drop
    // them from the void and add the box.
    let top_cap = faces.pop().unwrap_or_default();
    let bottom_cap = faces.pop().unwrap_or_default();
    let h = g.p * g.turns as f64;
    let w = width / 2.0;
    let corners = [[-w, -w], [w, -w], [w, w], [-w, w]];
    for (k, c) in corners.iter().enumerate() {
        pts.insert((-3, k), [c[0], c[1], 0.0]);
        pts.insert((-4, k), [c[0], c[1], h]);
    }
    let mut sh = CompressedShell {
        vertices: Vec::new(),
        edges: Vec::new(),
        faces: Vec::new(),
    };
    let (mut vids, mut eids) = (HashMap::new(), HashMap::new());
    // Void lateral faces, reversed (material outside).
    shell_from(&faces, &pts, frame, true, &mut vids, &mut sh, &mut eids)?;
    // Box sides (outward), counter-clockwise from outside.
    let sides: Vec<Vec<(i64, usize)>> = (0..4)
        .map(|k| vec![(-3, k), (-3, (k + 1) % 4), (-4, (k + 1) % 4), (-4, k)])
        .collect();
    shell_from(&sides, &pts, frame, false, &mut vids, &mut sh, &mut eids)?;
    // End faces with the void's cap loop as an inner loop (reversed).
    for (outer, inner) in [
        (vec![(-3, 3), (-3, 2), (-3, 1), (-3, 0)], &bottom_cap),
        (vec![(-4, 0), (-4, 1), (-4, 2), (-4, 3)], &top_cap),
    ] {
        let before = sh.faces.len();
        shell_from(&[outer], &pts, frame, false, &mut vids, &mut sh, &mut eids)?;
        shell_from(
            std::slice::from_ref(inner),
            &pts,
            frame,
            true,
            &mut vids,
            &mut sh,
            &mut eids,
        )?;
        let inner_face = sh
            .faces
            .pop()
            .ok_or_else(|| KernelError::Internal("thread: nut cap".into()))?;
        sh.faces[before]
            .boundaries
            .push(inner_face.boundaries.into_iter().next().unwrap_or_default());
    }
    let count = sh.faces.len();
    let body = finish(sh, "threaded nut")?;
    Ok((
        body,
        ThreadReport {
            segments_per_turn: n,
            faces: count,
            depth: g.r_ridge - g.r_core,
            minor_diameter: 2.0 * g.r_core,
            max_facet_deviation: deviation(&g),
            faceted: true,
        },
    ))
}

#[cfg(test)]
mod tests;
