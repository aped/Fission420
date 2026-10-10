//! Tessellation (derived display/export cache), mesh checks, and STL output.
use crate::{Body, KernelError, Result, guarded};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use truck_meshalgo::prelude::*;

/// Indexed triangle mesh with a per-triangle B-rep face index (display grouping only;
/// never used as a persistent reference).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TriMesh {
    pub positions: Vec<[f64; 3]>,
    pub normals: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub tri_face: Vec<u32>,
    pub chord_tolerance: f64,
}

/// Tessellate with chordal tolerance `chord` (mm). Vertices on shared B-rep edges are
/// welded (see [`WELD_TOLERANCE`]) so the result can be checked for watertightness
/// and exported. For display use [`tessellate_display`], which keeps per-face normals.
pub fn tessellate(body: &Body, chord: f64) -> Result<TriMesh> {
    tessellate_impl(body, chord, true)
}

/// Like [`tessellate`] but vertices are only welded within a B-rep face, so sharp
/// edges keep their face normals. Not watertight by construction.
pub fn tessellate_display(body: &Body, chord: f64) -> Result<TriMesh> {
    tessellate_impl(body, chord, false)
}

fn tessellate_impl(body: &Body, chord: f64, weld_across_faces: bool) -> Result<TriMesh> {
    // Bound the chord from below relative to the part size: tiny chords make Truck
    // allocate unboundedly (observed: 1e-6 mm aborts on a ~1 GB allocation).
    let b = crate::measure::raw_bounds(&body.solid)?;
    let diag = (0..3)
        .map(|i| (b.max[i] - b.min[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    let min_chord = crate::tol::MIN_MESH_CHORD_ABS.max(diag * crate::tol::MIN_MESH_CHORD_REL);
    if !chord.is_finite() || chord < min_chord {
        return Err(KernelError::InvalidInput(format!(
            "chord tolerance must be >= {min_chord:.3e} mm for this part, got {chord}"
        )));
    }
    guarded("tessellate", || {
        let meshed = body.solid.triangulation(chord);
        let mut out = TriMesh {
            chord_tolerance: chord,
            ..Default::default()
        };
        let mut weld = Welder::new(WELD_TOLERANCE);
        for (fi, face) in meshed.face_iter().enumerate() {
            if !weld_across_faces {
                weld = Welder::new(WELD_TOLERANCE);
            }
            let Some(pm) = face.surface() else {
                return Err(KernelError::Internal(format!(
                    "face {fi} failed to tessellate"
                )));
            };
            let pos = pm.positions();
            let nor = pm.normals();
            let flip = !face.orientation();
            for tri in pm.faces().triangle_iter() {
                let mut ids = [0u32; 3];
                for (k, v) in tri.iter().enumerate() {
                    let p = pos[v.pos];
                    let n = v.nor.map(|i| nor[i]).unwrap_or(Vector3::unit_z());
                    let n = if flip { -n } else { n };
                    ids[k] = weld.index(&mut out, [p.x, p.y, p.z], [n.x, n.y, n.z]);
                }
                if flip {
                    ids.swap(1, 2);
                }
                if ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2] {
                    out.triangles.push(ids);
                    out.tri_face.push(fi as u32);
                }
            }
        }
        Ok(out)
    })?
}

/// Vertices of adjacent faces closer than this are merged. Each face samples its
/// shared edges independently, so equal parameters can differ by float noise;
/// ten times the kernel's linear tolerance.
const WELD_TOLERANCE: f64 = 10.0 * crate::tol::KERNEL_LINEAR;

/// Spatial-hash vertex welder (cell = tolerance, 27-cell neighbourhood lookup).
struct Welder {
    tol: f64,
    cells: HashMap<[i64; 3], Vec<u32>>,
}

impl Welder {
    fn new(tol: f64) -> Self {
        Self {
            tol,
            cells: HashMap::new(),
        }
    }
    fn cell(&self, p: [f64; 3]) -> [i64; 3] {
        p.map(|c| (c / self.tol).floor() as i64)
    }
    fn index(&mut self, out: &mut TriMesh, p: [f64; 3], n: [f64; 3]) -> u32 {
        let c = self.cell(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(list) = self.cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) {
                        for &i in list {
                            let q = out.positions[i as usize];
                            let d2 = (p[0] - q[0]).powi(2)
                                + (p[1] - q[1]).powi(2)
                                + (p[2] - q[2]).powi(2);
                            if d2 <= self.tol * self.tol {
                                return i;
                            }
                        }
                    }
                }
            }
        }
        let i = out.positions.len() as u32;
        out.positions.push(p);
        out.normals.push(n);
        self.cells.entry(c).or_default().push(i);
        i
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshCheck {
    pub triangles: usize,
    pub vertices: usize,
    /// Every undirected edge is used by exactly two triangles with opposite direction.
    pub watertight: bool,
    pub boundary_edges: usize,
    pub nonmanifold_edges: usize,
    pub misoriented_edges: usize,
    /// Signed volume by the divergence theorem over triangles (a mesh value).
    pub mesh_volume: f64,
    pub bounds_min: [f64; 3],
    pub bounds_max: [f64; 3],
    pub chord_tolerance: f64,
}

pub fn check_mesh(m: &TriMesh) -> MeshCheck {
    let mut edges: HashMap<(u32, u32), (i32, i32)> = HashMap::new();
    let mut vol = 0.0;
    for t in &m.triangles {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let e = edges.entry((a.min(b), a.max(b))).or_default();
            if a < b { e.0 += 1 } else { e.1 += 1 }
        }
        let [p, q, r] = t.map(|i| m.positions[i as usize]);
        vol += p[0] * (q[1] * r[2] - q[2] * r[1]) - p[1] * (q[0] * r[2] - q[2] * r[0])
            + p[2] * (q[0] * r[1] - q[1] * r[0]);
    }
    let (mut boundary, mut nonmanifold, mut misoriented) = (0, 0, 0);
    for (f, b) in edges.values() {
        match (f + b, f, b) {
            (1, _, _) => boundary += 1,
            (2, 1, 1) => {}
            (2, _, _) => misoriented += 1,
            _ => nonmanifold += 1,
        }
    }
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in &m.positions {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    MeshCheck {
        triangles: m.triangles.len(),
        vertices: m.positions.len(),
        watertight: boundary == 0
            && nonmanifold == 0
            && misoriented == 0
            && !m.triangles.is_empty(),
        boundary_edges: boundary,
        nonmanifold_edges: nonmanifold,
        misoriented_edges: misoriented,
        mesh_volume: vol / 6.0,
        bounds_min: lo,
        bounds_max: hi,
        chord_tolerance: m.chord_tolerance,
    }
}

/// Binary STL bytes (little-endian, f32 as the format requires).
pub fn write_stl_binary(m: &TriMesh, name: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(84 + 50 * m.triangles.len());
    let mut header = [0u8; 80];
    let tag = format!("Fission420 STL {name} chord={}mm", m.chord_tolerance);
    let n = tag.len().min(80);
    header[..n].copy_from_slice(&tag.as_bytes()[..n]);
    out.extend_from_slice(&header);
    out.extend_from_slice(&(m.triangles.len() as u32).to_le_bytes());
    for t in &m.triangles {
        let [p, q, r] = t.map(|i| m.positions[i as usize]);
        let u = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
        let v = [r[0] - p[0], r[1] - p[1], r[2] - p[2]];
        let mut n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 {
            n.iter_mut().for_each(|c| *c /= len);
        }
        for c in n.iter().chain(&p).chain(&q).chain(&r) {
            out.extend_from_slice(&(*c as f32).to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    out
}
