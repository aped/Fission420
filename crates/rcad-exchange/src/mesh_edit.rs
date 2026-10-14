//! Mesh edits (docs/MESH.md): pure functions from an indexed triangle mesh and a
//! [`MeshEditOp`] to new mesh(es) and a [`MeshEditReport`]. The input is never changed;
//! the document layer turns each result into a new mesh body (the source is kept).
//!
//! Every edit first merges bit-identical vertices ([`crate::mesh_query::weld_exact`]): a
//! display mesh stores each triangle's corners separately.
//!
//! ## Transform ([`MeshTransform`])
//! `p' = R · (S · p) + t`: per-axis scale `S = diag(scale)` (each factor in
//! `[`[`MIN_SCALE`]`, `[`MAX_SCALE`]`]`; mirroring is refused because it would reverse
//! the winding), then a rotation `R` of `angle_deg` about `axis` through the origin
//! (right hand rule), then the translation `t` (mm). Unit-conversion presets are uniform
//! scales: inch → mm ×25.4, m → mm ×1000, cm → mm ×10 ([`MeshTransform::unit_conversion`]).
//! The enclosed volume of a closed mesh scales by `sx·sy·sz` (reported before/after).
//!
//! ## Plane cut ([`PlaneCut`])
//! Plane through `origin + offset·n̂` with unit normal `n̂ = normal/|normal|`. Signed
//! distances within `1e-9 ×` the bounds diagonal of the plane count as **on** the plane.
//! `keep`: `below` keeps `d ≤ 0` (the side opposite the normal), `above` keeps `d ≥ 0`,
//! `both` makes two meshes (below first). Triangles crossing the plane are clipped exactly
//! (the new vertex on edge `ab` is `a + (b − a)·d_a/(d_a − d_b)`, shared by both
//! triangles of the edge, so a closed input stays watertight along the cut). A triangle
//! lying in the plane is kept on the side its normal faces away from (it is that side's
//! face). With `cap` and a **closed** input, the cut loops (boundary edges on the plane) are
//! triangulated in the plane (outer loops and holes by orientation, holes bridged, ear
//! clipping) with the cap's normal pointing to the removed side; a loop that cannot be
//! capped (touching loops, a hole outside every outline) refuses the cut. Without `cap`,
//! or for an open input, the result is open and the loops are counted, not filled.
//!
//! ## Decimation ([`Decimate`])
//! Quadric edge collapse (Garland & Heckbert 1997): each vertex carries the sum of the
//! area-weighted squared-distance quadrics of its original triangles' planes; edges are
//! collapsed cheapest first to the quadric-optimal point (or the cheaper endpoint/midpoint
//! when the 3×3 system is singular). Constraints, each a refusal of that collapse:
//! * **boundaries are preserved**: a vertex on a boundary or non-manifold edge never moves
//!   (an edge with one such end collapses onto it; with two, it is not collapsed);
//! * **topology is preserved**: the link condition (the edge's endpoints share exactly the
//!   two opposite vertices), no new vertex of valence < 3, no duplicate triangle;
//! * no face normal turns by more than 78° (`n_new · n_old > 0.2`) or becomes degenerate;
//! * **deviation**: every source vertex and every source triangle centroid is kept within
//!   `max_deviation` of the result (each is tracked on the result triangle nearest to it and
//!   re-checked when that triangle changes), and the new vertex plus the new triangles'
//!   centroids and edge midpoints are within `max_deviation` of the source surface.
//!
//! It stops at `target_triangles` (0: as far as the deviation allows) or when no edge can
//! be collapsed. The result is then **measured** ([`DeviationReport`]): distances from the
//! source's vertices and triangle centroids to the result surface and from the result's
//! vertices, edge midpoints and centroids to the source surface (exact point-to-triangle
//! distances; the point sets are samples, so the maximum is a lower bound on the true
//! Hausdorff distance). A measured maximum above `max_deviation` refuses the result.
use crate::mesh::{MeshDiagnostics, diagnose};
use crate::mesh_query::{
    TriGrid, add, bounds_of, cross, dot, norm, point_triangle_distance, scale, sub, weld_exact,
};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

/// Smallest and largest scale factor a transform accepts.
pub const MIN_SCALE: f64 = 1e-6;
pub const MAX_SCALE: f64 = 1e6;
/// Input bounds (triangles).
pub const MAX_EDIT_TRIANGLES: usize = 5_000_000;
pub const MAX_DECIMATE_TRIANGLES: usize = 2_000_000;
/// Total vertices of the cut loops one cap may triangulate.
pub const MAX_CAP_VERTICES: usize = 20_000;

fn one3() -> [f64; 3] {
    [1.0; 3]
}
fn z_axis() -> [f64; 3] {
    [0.0, 0.0, 1.0]
}
fn yes() -> bool {
    true
}

/// Scale, then rotate about `axis` through the origin, then translate (module docs).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct MeshTransform {
    #[serde(default = "one3")]
    pub scale: [f64; 3],
    #[serde(default = "z_axis")]
    pub axis: [f64; 3],
    #[serde(default)]
    pub angle_deg: f64,
    #[serde(default)]
    pub translate: [f64; 3],
}

impl Default for MeshTransform {
    fn default() -> Self {
        MeshTransform {
            scale: one3(),
            axis: z_axis(),
            angle_deg: 0.0,
            translate: [0.0; 3],
        }
    }
}

impl MeshTransform {
    /// Uniform scale converting a mesh drawn in `unit` ("inch", "m", "cm", "mm") to mm.
    pub fn unit_conversion(unit: &str) -> Option<MeshTransform> {
        let s = match unit {
            "inch" | "in" => 25.4,
            "m" => 1000.0,
            "cm" => 10.0,
            "mm" => 1.0,
            _ => return None,
        };
        Some(MeshTransform {
            scale: [s; 3],
            ..Default::default()
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        for (k, s) in self.scale.iter().enumerate() {
            if !(s.is_finite() && (MIN_SCALE..=MAX_SCALE).contains(s)) {
                return Err(format!(
                    "scale {} must be in [{MIN_SCALE}, {MAX_SCALE}] (mirroring is not supported), \
                     got {s}",
                    ["x", "y", "z"][k]
                ));
            }
        }
        if !self.angle_deg.is_finite() || self.angle_deg.abs() > 3600.0 {
            return Err(format!(
                "rotation angle must be finite and at most 3600 degrees, got {}",
                self.angle_deg
            ));
        }
        if self.angle_deg != 0.0
            && !(self.axis.iter().all(|x| x.is_finite()) && norm(self.axis) > 1e-12)
        {
            return Err("the rotation axis must be a finite non-zero vector".into());
        }
        if self
            .translate
            .iter()
            .any(|x| !x.is_finite() || x.abs() > 1e9)
        {
            return Err("the translation must be finite (at most 1e9 mm)".into());
        }
        Ok(())
    }

    /// Row-major rotation matrix (Rodrigues).
    pub fn rotation(&self) -> [[f64; 3]; 3] {
        let mut r = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        if self.angle_deg == 0.0 {
            return r;
        }
        let k = scale(self.axis, 1.0 / norm(self.axis));
        let (s, c) = self.angle_deg.to_radians().sin_cos();
        let t = 1.0 - c;
        r = [
            [
                c + k[0] * k[0] * t,
                k[0] * k[1] * t - k[2] * s,
                k[0] * k[2] * t + k[1] * s,
            ],
            [
                k[1] * k[0] * t + k[2] * s,
                c + k[1] * k[1] * t,
                k[1] * k[2] * t - k[0] * s,
            ],
            [
                k[2] * k[0] * t - k[1] * s,
                k[2] * k[1] * t + k[0] * s,
                c + k[2] * k[2] * t,
            ],
        ];
        r
    }

    pub fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        let r = self.rotation();
        let q = [
            p[0] * self.scale[0],
            p[1] * self.scale[1],
            p[2] * self.scale[2],
        ];
        add([dot(r[0], q), dot(r[1], q), dot(r[2], q)], self.translate)
    }
}

/// Which side(s) of the plane a cut keeps.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CutKeep {
    /// The side opposite the normal.
    #[default]
    Below,
    Above,
    /// Two meshes: below, then above.
    Both,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct PlaneCut {
    pub origin: [f64; 3],
    pub normal: [f64; 3],
    /// Distance of the plane from `origin` along the unit normal, mm.
    #[serde(default)]
    pub offset: f64,
    #[serde(default)]
    pub keep: CutKeep,
    /// Close the cut loops of a closed input.
    #[serde(default = "yes")]
    pub cap: bool,
}

impl PlaneCut {
    pub fn validate(&self) -> Result<(), String> {
        if self
            .origin
            .iter()
            .chain(&[self.offset])
            .any(|x| !x.is_finite())
        {
            return Err("the cut plane origin and offset must be finite".into());
        }
        if !(self.normal.iter().all(|x| x.is_finite()) && norm(self.normal) > 1e-12) {
            return Err("the cut plane normal must be a finite non-zero vector".into());
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Decimate {
    /// Stop at this many triangles (0: as far as `max_deviation` allows).
    #[serde(default)]
    pub target_triangles: u64,
    /// Bound on the deviation from the source, mm (> 0).
    pub max_deviation: f64,
}

impl Decimate {
    pub fn validate(&self) -> Result<(), String> {
        if !(self.max_deviation.is_finite() && self.max_deviation > 0.0) {
            return Err(format!(
                "the maximum deviation must be finite and > 0 mm, got {}",
                self.max_deviation
            ));
        }
        Ok(())
    }
}

/// One mesh edit (serialized in the document's history).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MeshEditOp {
    Transform(MeshTransform),
    PlaneCut(PlaneCut),
    Decimate(Decimate),
}

impl MeshEditOp {
    pub fn name(&self) -> &'static str {
        match self {
            MeshEditOp::Transform(_) => "transform",
            MeshEditOp::PlaneCut(_) => "plane cut",
            MeshEditOp::Decimate(_) => "decimate",
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            MeshEditOp::Transform(t) => t.validate(),
            MeshEditOp::PlaneCut(c) => c.validate(),
            MeshEditOp::Decimate(d) => d.validate(),
        }
    }

    /// Number of meshes the edit produces.
    pub fn outputs(&self) -> usize {
        match self {
            MeshEditOp::PlaneCut(PlaneCut {
                keep: CutKeep::Both,
                ..
            }) => 2,
            _ => 1,
        }
    }
}

/// Measured deviation between a source and a result surface (module docs).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DeviationReport {
    pub method: String,
    /// Max / mean distance of source samples to the result surface, mm.
    pub source_to_result_max: f64,
    pub source_to_result_mean: f64,
    /// Max / mean distance of result samples to the source surface, mm.
    pub result_to_source_max: f64,
    pub result_to_source_mean: f64,
    /// Larger of the two maxima (a sampled Hausdorff estimate).
    pub max: f64,
    pub samples: usize,
}

/// What an edit changed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MeshEditReport {
    pub op: MeshEditOp,
    pub before: MeshDiagnostics,
    /// One per produced mesh.
    pub after: Vec<MeshDiagnostics>,
    pub vertices_moved: usize,
    pub triangles_cut: usize,
    pub triangles_removed: usize,
    pub triangles_added: usize,
    pub cut_loops: usize,
    pub capped_loops: usize,
    pub collapses: usize,
    /// Collapses refused by the topology / normal / deviation / boundary rules.
    pub refused_topology: usize,
    pub refused_normal: usize,
    pub refused_deviation: usize,
    pub deviation: Option<DeviationReport>,
    pub notes: Vec<String>,
}

/// An indexed mesh.
pub type IndexedMesh = (Vec<[f64; 3]>, Vec<[u32; 3]>);

/// Apply `op` (module docs). `Err` names the refusal; the input is unchanged.
pub fn edit(
    positions: &[[f64; 3]],
    triangles: &[[u32; 3]],
    op: &MeshEditOp,
) -> Result<(Vec<IndexedMesh>, MeshEditReport), String> {
    op.validate()?;
    if triangles.is_empty() {
        return Err("the mesh has no triangles".into());
    }
    if triangles.len() > MAX_EDIT_TRIANGLES {
        return Err(format!(
            "mesh edits are limited to {MAX_EDIT_TRIANGLES} triangles, got {}",
            triangles.len()
        ));
    }
    if triangles
        .iter()
        .any(|t| t.iter().any(|&i| i as usize >= positions.len()))
    {
        return Err("triangles reference missing vertices".into());
    }
    if positions.iter().flatten().any(|x| !x.is_finite()) {
        return Err("the mesh has non-finite coordinates".into());
    }
    let (pos, tris) = weld_exact(positions, triangles);
    let before = diagnose(&pos, &tris);
    let mut report = MeshEditReport {
        op: *op,
        before: before.clone(),
        after: vec![],
        vertices_moved: 0,
        triangles_cut: 0,
        triangles_removed: 0,
        triangles_added: 0,
        cut_loops: 0,
        capped_loops: 0,
        collapses: 0,
        refused_topology: 0,
        refused_normal: 0,
        refused_deviation: 0,
        deviation: None,
        notes: vec![],
    };
    let meshes = match op {
        MeshEditOp::Transform(t) => {
            let out: Vec<[f64; 3]> = pos.iter().map(|p| t.apply(*p)).collect();
            report.vertices_moved = pos.iter().zip(&out).filter(|(a, b)| a != b).count();
            let f = t.scale[0] * t.scale[1] * t.scale[2];
            if (f - 1.0).abs() > 0.0 {
                report
                    .notes
                    .push(format!("volume scales by sx·sy·sz = {f}"));
            }
            vec![(out, tris)]
        }
        MeshEditOp::PlaneCut(c) => plane_cut(&pos, &tris, c, before.closed, &mut report)?,
        MeshEditOp::Decimate(d) => vec![decimate(&pos, &tris, d, &mut report)?],
    };
    report.after = meshes.iter().map(|(p, t)| diagnose(p, t)).collect();
    Ok((meshes, report))
}

// ---------------------------------------------------------------- plane cut

fn plane_cut(
    pos: &[[f64; 3]],
    tris: &[[u32; 3]],
    c: &PlaneCut,
    closed: bool,
    report: &mut MeshEditReport,
) -> Result<Vec<IndexedMesh>, String> {
    let n = scale(c.normal, 1.0 / norm(c.normal));
    let o = add(c.origin, scale(n, c.offset));
    let diag = bounds_of(pos.iter()).map_or(0.0, |(lo, hi)| norm(sub(hi, lo)));
    let eps = 1e-9 * diag;
    let d: Vec<f64> = pos
        .iter()
        .map(|p| {
            let v = dot(sub(*p, o), n);
            if v.abs() <= eps { 0.0 } else { v }
        })
        .collect();
    if d.iter().all(|&v| v >= 0.0) || d.iter().all(|&v| v <= 0.0) {
        return Err(
            "the plane does not cut the mesh (every vertex is on one side or on the plane)".into(),
        );
    }
    if c.cap && closed && report.before.signed_volume.is_some_and(|v| v < 0.0) {
        return Err(
            "the mesh is inside out (negative enclosed volume): orient it outward first (mesh \
             repair) before a capped cut"
                .into(),
        );
    }
    if c.cap && !closed {
        report
            .notes
            .push("the source mesh is not closed: cut loops are left open (not capped)".into());
    }
    let sides: Vec<f64> = match c.keep {
        CutKeep::Below => vec![1.0],
        CutKeep::Above => vec![-1.0],
        CutKeep::Both => vec![1.0, -1.0],
    };
    let mut out = vec![];
    for (k, s) in sides.into_iter().enumerate() {
        let (mut p2, mut t2, on_plane, cut, dropped) = clip(pos, tris, &d, s, n);
        // A source triangle clipped on both sides is one clipped triangle.
        if k == 0 {
            report.triangles_cut += cut;
        }
        report.triangles_removed += dropped;
        let (loops, open_off_plane) = cut_loops(&t2, &on_plane);
        report.cut_loops += loops.len();
        if c.cap && closed {
            if open_off_plane > 0 {
                return Err(format!(
                    "{open_off_plane} boundary edges off the cut plane or in open cut chains: the cut cannot be capped"
                ));
            }
            let w = scale(n, s);
            let caps = cap_loops(&p2, &loops, w)?;
            report.capped_loops += loops.len();
            report.triangles_added += caps.len();
            t2.extend(caps);
        }
        // Drop unused vertices.
        let mut used = vec![u32::MAX; p2.len()];
        let mut np = vec![];
        for t in t2.iter_mut() {
            for i in t.iter_mut() {
                if used[*i as usize] == u32::MAX {
                    used[*i as usize] = np.len() as u32;
                    np.push(p2[*i as usize]);
                }
                *i = used[*i as usize];
            }
        }
        p2 = np;
        if t2.is_empty() {
            return Err("nothing is kept on that side of the plane".into());
        }
        out.push((p2, t2));
    }
    if c.keep == CutKeep::Both {
        // Every source triangle lands (whole or clipped) on one side or the
        // other (an in-plane triangle on the side it faces away from): none is
        // discarded.
        report.triangles_removed = 0;
    }
    Ok(out)
}

/// Clipped mesh: positions (source + new cut vertices), triangles, on-plane flag per
/// vertex, clipped triangles, source triangles dropped entirely.
type Clipped = (Vec<[f64; 3]>, Vec<[u32; 3]>, Vec<bool>, usize, usize);

/// Keep the part with `s·d ≤ 0` (`n`: the plane's unit normal).
fn clip(pos: &[[f64; 3]], tris: &[[u32; 3]], d: &[f64], s: f64, n: [f64; 3]) -> Clipped {
    let mut p = pos.to_vec();
    let mut on: Vec<bool> = d.iter().map(|&v| v == 0.0).collect();
    let mut cache: HashMap<(u32, u32), u32> = HashMap::new();
    let mut out = vec![];
    let mut cut = 0;
    let mut dropped = 0;
    for t in tris {
        let dv = t.map(|i| s * d[i as usize]);
        if dv.iter().all(|&v| v == 0.0) {
            // In the plane: keep when its normal faces the removed side.
            let [a, b, c] = t.map(|i| pos[i as usize]);
            let nn = cross(sub(b, a), sub(c, a));
            if s * dot(nn, n) > 0.0 {
                out.push(*t);
            } else {
                dropped += 1;
            }
            continue;
        }
        if dv.iter().all(|&v| v <= 0.0) {
            out.push(*t);
            continue;
        }
        if dv.iter().all(|&v| v >= 0.0) {
            dropped += 1;
            continue;
        }
        cut += 1;
        // Sutherland-Hodgman against one plane, keeping the order.
        let mut poly: Vec<u32> = Vec::with_capacity(4);
        for k in 0..3 {
            let (i, j) = (t[k], t[(k + 1) % 3]);
            let (di, dj) = (dv[k], dv[(k + 1) % 3]);
            if di <= 0.0 {
                poly.push(i);
            }
            if (di < 0.0 && dj > 0.0) || (di > 0.0 && dj < 0.0) {
                let key = (i.min(j), i.max(j));
                let v = *cache.entry(key).or_insert_with(|| {
                    let (a, b) = (key.0 as usize, key.1 as usize);
                    let (da, db) = (d[a], d[b]);
                    let q = add(pos[a], scale(sub(pos[b], pos[a]), da / (da - db)));
                    p.push(q);
                    on.push(true);
                    (p.len() - 1) as u32
                });
                poly.push(v);
            }
        }
        for k in 1..poly.len().saturating_sub(1) {
            out.push([poly[0], poly[k], poly[k + 1]]);
        }
    }
    (p, out, on, cut, dropped)
}

/// Chain the result's boundary edges that lie on the plane into closed loops (in
/// the kept triangles' direction). Also returns the number of boundary edges off
/// the plane.
fn cut_loops(tris: &[[u32; 3]], on: &[bool]) -> (Vec<Vec<u32>>, usize) {
    let mut directed: HashMap<(u32, u32), i32> = HashMap::new();
    for t in tris {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if let Some(c) = directed.get_mut(&(b, a)) {
                *c -= 1;
                if *c == 0 {
                    directed.remove(&(b, a));
                }
            } else {
                *directed.entry((a, b)).or_default() += 1;
            }
        }
    }
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut off = 0;
    let mut edges: Vec<(u32, u32)> = directed
        .iter()
        .filter(|(_, c)| **c > 0)
        .map(|(e, _)| *e)
        .collect();
    edges.sort_unstable();
    for &(a, b) in &edges {
        if on[a as usize] && on[b as usize] {
            next.entry(a).or_default().push(b);
        } else {
            off += 1;
        }
    }
    let mut loops = vec![];
    let mut starts: Vec<u32> = next.keys().copied().collect();
    starts.sort_unstable();
    for s in starts {
        while next.get(&s).is_some_and(|v| !v.is_empty()) {
            let mut lp = vec![s];
            let mut cur = s;
            let mut closed = false;
            while let Some(nx) = next.get_mut(&cur).and_then(|v| v.pop()) {
                if nx == s {
                    closed = true;
                    break;
                }
                lp.push(nx);
                cur = nx;
            }
            if closed {
                loops.push(lp);
            } else {
                // An open chain cannot be capped: counted with the off-plane edges.
                off += lp.len();
            }
        }
    }
    (loops, off)
}

/// Triangulate planar loops (each in the kept triangles' direction) into caps whose
/// normal is `w`.
fn cap_loops(pos: &[[f64; 3]], loops: &[Vec<u32>], w: [f64; 3]) -> Result<Vec<[u32; 3]>, String> {
    let total: usize = loops.iter().map(Vec::len).sum();
    if total > MAX_CAP_VERTICES {
        return Err(format!(
            "the cut loops have {total} vertices; capping is limited to {MAX_CAP_VERTICES}"
        ));
    }
    // Shared vertex between loops means they touch: refuse.
    let mut seen = HashSet::new();
    for l in loops {
        for &v in l {
            if !seen.insert(v) {
                return Err("cut loops touch at a vertex: the cut cannot be capped".into());
            }
        }
    }
    let ax = if w[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = {
        let v = cross(w, ax);
        scale(v, 1.0 / norm(v))
    };
    let v = cross(w, u);
    let p2 = |i: u32| {
        let p = pos[i as usize];
        [dot(p, u), dot(p, v)]
    };
    // Cap polygons run opposite to the kept boundary.
    let polys: Vec<Vec<u32>> = loops
        .iter()
        .map(|l| l.iter().rev().copied().collect())
        .collect();
    let area = |l: &[u32]| {
        let mut a = 0.0;
        for k in 0..l.len() {
            let (p, q) = (p2(l[k]), p2(l[(k + 1) % l.len()]));
            a += p[0] * q[1] - q[0] * p[1];
        }
        0.5 * a
    };
    let areas: Vec<f64> = polys.iter().map(|l| area(l)).collect();
    let mut outers: Vec<(usize, Vec<usize>)> = vec![];
    for (i, a) in areas.iter().enumerate() {
        if *a > 0.0 {
            outers.push((i, vec![]));
        }
    }
    for (i, a) in areas.iter().enumerate() {
        if *a > 0.0 {
            continue;
        }
        if *a == 0.0 {
            return Err("a cut loop has no area".into());
        }
        // Smallest outer containing the hole's first vertex.
        let q = p2(polys[i][0]);
        let host = outers
            .iter_mut()
            .filter(|(o, _)| {
                point_in_poly(q, &polys[*o].iter().map(|&x| p2(x)).collect::<Vec<_>>())
            })
            .min_by(|a, b| areas[a.0].total_cmp(&areas[b.0]));
        match host {
            Some((_, holes)) => holes.push(i),
            None => {
                return Err(
                    "a cut loop runs the wrong way (not inside any outline): the mesh's \
                     orientation is inconsistent, cannot cap"
                        .into(),
                );
            }
        }
    }
    let mut out = vec![];
    for (o, holes) in outers {
        let mut poly = polys[o].clone();
        let mut hs: Vec<Vec<u32>> = holes.iter().map(|&h| polys[h].clone()).collect();
        // Rightmost holes first.
        hs.sort_by(|a, b| {
            let ma = a
                .iter()
                .map(|&x| p2(x)[0])
                .fold(f64::NEG_INFINITY, f64::max);
            let mb = b
                .iter()
                .map(|&x| p2(x)[0])
                .fold(f64::NEG_INFINITY, f64::max);
            mb.total_cmp(&ma)
        });
        for k in 0..hs.len() {
            let (rest, cur) = (&hs[k + 1..], &hs[k]);
            poly = bridge(&poly, cur, rest, &p2)?;
        }
        out.extend(ear_clip(&poly, &p2)?);
    }
    Ok(out)
}

fn point_in_poly(q: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > q[1]) != (b[1] > q[1]) {
            let x = a[0] + (q[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if q[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

fn orient2(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])
}

fn segments_cross(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let (d1, d2) = (orient2(a, b, c), orient2(a, b, d));
    let (d3, d4) = (orient2(c, d, a), orient2(c, d, b));
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

/// Merge `hole` (clockwise) into `poly` (counter-clockwise) through a bridge from the
/// hole's rightmost vertex to the nearest visible polygon vertex.
fn bridge(
    poly: &[u32],
    hole: &[u32],
    other_holes: &[Vec<u32>],
    p2: &impl Fn(u32) -> [f64; 2],
) -> Result<Vec<u32>, String> {
    let hm = (0..hole.len())
        .max_by(|&a, &b| p2(hole[a])[0].total_cmp(&p2(hole[b])[0]))
        .ok_or("empty hole")?;
    let m = p2(hole[hm]);
    let mut cands: Vec<usize> = (0..poly.len()).collect();
    cands.sort_by(|&a, &b| {
        let (pa, pb) = (p2(poly[a]), p2(poly[b]));
        let da = (pa[0] - m[0]).powi(2) + (pa[1] - m[1]).powi(2);
        let db = (pb[0] - m[0]).powi(2) + (pb[1] - m[1]).powi(2);
        da.total_cmp(&db)
    });
    let all_edges = |f: &mut dyn FnMut([f64; 2], [f64; 2]) -> bool| -> bool {
        for l in std::iter::once(poly)
            .chain(std::iter::once(hole))
            .chain(other_holes.iter().map(|h| h.as_slice()))
        {
            for k in 0..l.len() {
                if f(p2(l[k]), p2(l[(k + 1) % l.len()])) {
                    return true;
                }
            }
        }
        false
    };
    for vi in cands {
        let pv = p2(poly[vi]);
        // The bridge must leave the polygon vertex into the polygon's interior.
        let (prev, next) = (
            p2(poly[(vi + poly.len() - 1) % poly.len()]),
            p2(poly[(vi + 1) % poly.len()]),
        );
        let convex = orient2(prev, pv, next) > 0.0;
        let inside_cone = if convex {
            orient2(prev, pv, m) > 0.0 && orient2(pv, next, m) > 0.0
        } else {
            !(orient2(prev, pv, m) <= 0.0 && orient2(pv, next, m) <= 0.0)
        };
        if !inside_cone {
            continue;
        }
        if all_edges(&mut |a, b| segments_cross(m, pv, a, b)) {
            continue;
        }
        let mut out = Vec::with_capacity(poly.len() + hole.len() + 2);
        out.extend_from_slice(&poly[..=vi]);
        for k in 0..=hole.len() {
            out.push(hole[(hm + k) % hole.len()]);
        }
        out.push(poly[vi]);
        out.extend_from_slice(&poly[vi + 1..]);
        return Ok(out);
    }
    Err("no bridge from a hole to its outline was found: cannot cap".into())
}

/// Ear clipping of a counter-clockwise polygon (possibly with bridge seams).
fn ear_clip(poly: &[u32], p2: &impl Fn(u32) -> [f64; 2]) -> Result<Vec<[u32; 3]>, String> {
    let mut idx: Vec<u32> = poly.to_vec();
    let mut out = vec![];
    let mut from = 0usize;
    let limit = 4 * poly.len() * poly.len() + 16;
    let mut guard = 0;
    while idx.len() > 3 {
        let m = idx.len();
        let mut clipped = false;
        for step in 0..m {
            guard += 1;
            if guard > limit {
                return Err("cap triangulation did not converge".into());
            }
            let k = (from + step) % m;
            let (ia, ib, ic) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            let (a, b, c) = (p2(ia), p2(ib), p2(ic));
            if orient2(a, b, c) <= 0.0 {
                continue;
            }
            let blocked = idx.iter().any(|&j| {
                if j == ia || j == ib || j == ic {
                    return false;
                }
                let q = p2(j);
                if q == a || q == b || q == c {
                    return false;
                }
                orient2(a, b, q) >= 0.0 && orient2(b, c, q) >= 0.0 && orient2(c, a, q) >= 0.0
            });
            if blocked {
                continue;
            }
            out.push([ia, ib, ic]);
            idx.remove(k);
            from = k.saturating_sub(1);
            clipped = true;
            break;
        }
        if !clipped {
            // Dropping a vertex here would leave a T-junction (the cap would
            // not close the mesh): refuse instead.
            return Err("no ear found while capping (degenerate or self-touching outline)".into());
        }
    }
    if orient2(p2(idx[0]), p2(idx[1]), p2(idx[2])) > 0.0 {
        out.push([idx[0], idx[1], idx[2]]);
    } else {
        return Err("the cap's last triangle is degenerate (collinear outline)".into());
    }
    Ok(out)
}

// --------------------------------------------------------------- decimation

#[derive(Clone, Copy, Default)]
struct Quadric([f64; 10]);

impl Quadric {
    fn plane(n: [f64; 3], d: f64, w: f64) -> Quadric {
        let [a, b, c] = n;
        Quadric([
            w * a * a,
            w * a * b,
            w * a * c,
            w * a * d,
            w * b * b,
            w * b * c,
            w * b * d,
            w * c * c,
            w * c * d,
            w * d * d,
        ])
    }
    fn add(&self, o: &Quadric) -> Quadric {
        let mut r = *self;
        for k in 0..10 {
            r.0[k] += o.0[k];
        }
        r
    }
    fn eval(&self, p: [f64; 3]) -> f64 {
        let q = &self.0;
        let [x, y, z] = p;
        q[0] * x * x
            + 2.0 * q[1] * x * y
            + 2.0 * q[2] * x * z
            + 2.0 * q[3] * x
            + q[4] * y * y
            + 2.0 * q[5] * y * z
            + 2.0 * q[6] * y
            + q[7] * z * z
            + 2.0 * q[8] * z
            + q[9]
    }
    fn optimum(&self) -> Option<[f64; 3]> {
        let q = &self.0;
        let m = [[q[0], q[1], q[2]], [q[1], q[4], q[5]], [q[2], q[5], q[7]]];
        let b = [-q[3], -q[6], -q[8]];
        let det = dot(m[0], cross(m[1], m[2]));
        let scale_m = m.iter().flatten().map(|x| x.abs()).fold(0.0, f64::max);
        if det.abs().partial_cmp(&(1e-9 * scale_m.powi(3))) != Some(Ordering::Greater)
            || scale_m == 0.0
        {
            return None;
        }
        let col = |k: usize, v: [f64; 3]| {
            let mut mm = m;
            for r in 0..3 {
                mm[r][k] = v[r];
            }
            dot(mm[0], cross(mm[1], mm[2]))
        };
        let x = [col(0, b) / det, col(1, b) / det, col(2, b) / det];
        x.iter().all(|v| v.is_finite()).then_some(x)
    }
}

#[derive(PartialEq)]
struct Cand {
    cost: f64,
    u: u32,
    v: u32,
    vu: u32,
    vv: u32,
}
impl Eq for Cand {}
impl PartialOrd for Cand {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Cand {
    fn cmp(&self, o: &Self) -> Ordering {
        // Min-heap on cost, deterministic ties.
        o.cost
            .total_cmp(&self.cost)
            .then(o.u.cmp(&self.u))
            .then(o.v.cmp(&self.v))
    }
}

struct Decimator<'a> {
    src_pos: &'a [[f64; 3]],
    src_tris: &'a [[u32; 3]],
    src_grid: TriGrid,
    pos: Vec<[f64; 3]>,
    tris: Vec<[u32; 3]>,
    alive: Vec<bool>,
    vtris: Vec<Vec<u32>>,
    quad: Vec<Quadric>,
    locked: Vec<bool>,
    version: Vec<u32>,
    /// Source sample points tracked on each result triangle.
    samples: Vec<Vec<[f64; 3]>>,
    /// Vertex still at its source position (a source sample itself).
    original: Vec<bool>,
    tol: f64,
}

fn tri_normal(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> [f64; 3] {
    cross(sub(b, a), sub(c, a))
}

impl Decimator<'_> {
    fn neighbours(&self, v: u32) -> Vec<u32> {
        let mut out: Vec<u32> = self.vtris[v as usize]
            .iter()
            .flat_map(|&t| self.tris[t as usize])
            .filter(|&x| x != v)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    fn candidate(&self, u: u32, v: u32) -> Option<(f64, [f64; 3])> {
        let (lu, lv) = (self.locked[u as usize], self.locked[v as usize]);
        if lu && lv {
            return None;
        }
        let q = self.quad[u as usize].add(&self.quad[v as usize]);
        let (pu, pv) = (self.pos[u as usize], self.pos[v as usize]);
        let p = if lu {
            pu
        } else if lv {
            pv
        } else {
            let mid = scale(add(pu, pv), 0.5);
            let mut best = [pu, pv, mid]
                .into_iter()
                .min_by(|a, b| q.eval(*a).total_cmp(&q.eval(*b)))
                .unwrap_or(mid);
            if let Some(o) = q.optimum() {
                // Only when it stays near the edge (a far optimum is a sign of a
                // nearly singular system).
                let l = norm(sub(pv, pu));
                if norm(sub(o, mid)) <= 2.0 * l && q.eval(o) <= q.eval(best) {
                    best = o;
                }
            }
            best
        };
        Some((q.eval(p).max(0.0), p))
    }

    fn push(&self, heap: &mut BinaryHeap<Cand>, u: u32, v: u32) {
        let (u, v) = (u.min(v), u.max(v));
        if let Some((cost, _)) = self.candidate(u, v) {
            heap.push(Cand {
                cost,
                u,
                v,
                vu: self.version[u as usize],
                vv: self.version[v as usize],
            });
        }
    }

    fn src_distance(&self, p: [f64; 3]) -> f64 {
        self.src_grid.nearest(self.src_pos, self.src_tris, p).0
    }

    /// Try to collapse edge (u, v); `Err(reason)` refuses it (0 topology, 1 normal,
    /// 2 deviation).
    fn collapse(&mut self, u: u32, v: u32, p: [f64; 3]) -> Result<(), u8> {
        let tu = self.vtris[u as usize].clone();
        let tv = self.vtris[v as usize].clone();
        let shared: Vec<u32> = tu.iter().copied().filter(|t| tv.contains(t)).collect();
        if shared.len() != 2 {
            return Err(0);
        }
        let nu = self.neighbours(u);
        let nv = self.neighbours(v);
        let common: Vec<u32> = nu.iter().copied().filter(|x| nv.contains(x)).collect();
        if common.len() != 2 {
            return Err(0);
        }
        // Valence of the merged vertex.
        let mut merged: Vec<u32> = nu
            .iter()
            .chain(&nv)
            .copied()
            .filter(|&x| x != u && x != v)
            .collect();
        merged.sort_unstable();
        merged.dedup();
        if merged.len() < 3 {
            return Err(0);
        }
        // New triangles.
        let changed: Vec<u32> = tu
            .iter()
            .chain(&tv)
            .copied()
            .filter(|t| !shared.contains(t))
            .collect();
        let mut new_tris: Vec<(u32, [u32; 3])> = vec![];
        let mut keys = HashSet::new();
        for &t in &changed {
            let old = self.tris[t as usize];
            let nt = old.map(|x| if x == v { u } else { x });
            let [a, b, c] = old.map(|i| self.pos[i as usize]);
            let n_old = tri_normal(a, b, c);
            let q = nt.map(|i| if i == u { p } else { self.pos[i as usize] });
            let n_new = tri_normal(q[0], q[1], q[2]);
            let (lo, ln) = (norm(n_old), norm(n_new));
            let longest = norm(sub(q[1], q[0]))
                .max(norm(sub(q[2], q[1])))
                .max(norm(sub(q[0], q[2])));
            if ln <= crate::mesh::DEGENERATE_REL_AREA * longest * longest {
                return Err(1);
            }
            if lo > 0.0 && dot(n_old, n_new) <= 0.2 * lo * ln {
                return Err(1);
            }
            let mut k = nt;
            k.sort_unstable();
            if !keys.insert(k) {
                return Err(0);
            }
            new_tris.push((t, nt));
        }
        // Duplicate of an existing triangle elsewhere.
        for &(_, nt) in &new_tris {
            let others = nt.iter().find(|&&x| x != u).copied().unwrap_or(u);
            for &t in &self.vtris[others as usize] {
                if changed.contains(&t) || shared.contains(&t) {
                    continue;
                }
                let mut a = self.tris[t as usize];
                let mut b = nt;
                a.sort_unstable();
                b.sort_unstable();
                if a == b {
                    return Err(0);
                }
            }
        }
        // Deviation, result -> source: new vertex, new centroids and edge midpoints.
        let tol = self.tol;
        if self.src_distance(p) > tol {
            return Err(2);
        }
        for &(_, nt) in &new_tris {
            let q = nt.map(|i| if i == u { p } else { self.pos[i as usize] });
            let cen = scale(add(add(q[0], q[1]), q[2]), 1.0 / 3.0);
            if self.src_distance(cen) > tol {
                return Err(2);
            }
            for k in 0..3 {
                if (nt[k] == u || nt[(k + 1) % 3] == u)
                    && self.src_distance(scale(add(q[k], q[(k + 1) % 3]), 0.5)) > tol
                {
                    return Err(2);
                }
            }
        }
        // Deviation, source -> result: re-home every tracked sample.
        let mut pts: Vec<[f64; 3]> = vec![];
        for &t in changed.iter().chain(&shared) {
            pts.extend_from_slice(&self.samples[t as usize]);
        }
        for w in [u, v] {
            if self.original[w as usize] && self.pos[w as usize] != p {
                pts.push(self.pos[w as usize]);
            }
        }
        let mut homes: Vec<Vec<[f64; 3]>> = vec![vec![]; new_tris.len()];
        for q in &pts {
            let mut best = (f64::INFINITY, 0usize);
            for (k, &(_, nt)) in new_tris.iter().enumerate() {
                let c = nt.map(|i| if i == u { p } else { self.pos[i as usize] });
                let d = point_triangle_distance(*q, c[0], c[1], c[2]);
                if d < best.0 {
                    best = (d, k);
                }
            }
            if best.0 > tol {
                return Err(2);
            }
            homes[best.1].push(*q);
        }
        // Commit.
        for &t in &shared {
            self.alive[t as usize] = false;
            self.samples[t as usize].clear();
            for &x in &self.tris[t as usize] {
                self.vtris[x as usize].retain(|&y| y != t);
            }
        }
        for (k, &(t, nt)) in new_tris.iter().enumerate() {
            self.tris[t as usize] = nt;
            self.samples[t as usize] = std::mem::take(&mut homes[k]);
        }
        let moved: Vec<u32> = self.vtris[v as usize].clone();
        for t in moved {
            if !self.vtris[u as usize].contains(&t) {
                self.vtris[u as usize].push(t);
            }
        }
        self.vtris[v as usize].clear();
        if self.pos[u as usize] != p {
            self.original[u as usize] = false;
        }
        self.original[v as usize] = false;
        self.pos[u as usize] = p;
        self.quad[u as usize] = self.quad[u as usize].add(&self.quad[v as usize]);
        self.locked[u as usize] |= self.locked[v as usize];
        self.version[u as usize] += 1;
        self.version[v as usize] += 1;
        Ok(())
    }
}

fn decimate(
    pos: &[[f64; 3]],
    tris: &[[u32; 3]],
    o: &Decimate,
    report: &mut MeshEditReport,
) -> Result<IndexedMesh, String> {
    if tris.len() > MAX_DECIMATE_TRIANGLES {
        return Err(format!(
            "decimation is limited to {MAX_DECIMATE_TRIANGLES} triangles, got {}",
            tris.len()
        ));
    }
    if report.before.degenerate_triangles > 0 || report.before.duplicate_triangles > 0 {
        return Err(format!(
            "the mesh has {} degenerate and {} duplicate triangles: repair it first",
            report.before.degenerate_triangles, report.before.duplicate_triangles
        ));
    }
    let src_grid = TriGrid::new(pos, tris).ok_or("the mesh has no triangles")?;
    let n = pos.len();
    let mut d = Decimator {
        src_pos: pos,
        src_tris: tris,
        src_grid,
        pos: pos.to_vec(),
        tris: tris.to_vec(),
        alive: vec![true; tris.len()],
        vtris: vec![vec![]; n],
        quad: vec![Quadric::default(); n],
        locked: vec![false; n],
        version: vec![0; n],
        samples: vec![],
        original: vec![true; n],
        tol: o.max_deviation,
    };
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        let [a, b, c] = t.map(|i| pos[i as usize]);
        let nn = tri_normal(a, b, c);
        let l = norm(nn);
        let area = 0.5 * l;
        if l > 0.0 {
            let un = scale(nn, 1.0 / l);
            let q = Quadric::plane(un, -dot(un, a), area);
            for &i in t {
                d.quad[i as usize] = d.quad[i as usize].add(&q);
            }
        }
        for &i in t {
            d.vtris[i as usize].push(ti as u32);
        }
        for k in 0..3 {
            let (x, y) = (t[k], t[(k + 1) % 3]);
            *edges.entry((x.min(y), x.max(y))).or_default() += 1;
        }
        d.samples.push(vec![scale(add(add(a, b), c), 1.0 / 3.0)]);
    }
    let mut locked_edges = 0;
    for (&(x, y), &c) in &edges {
        if c != 2 {
            d.locked[x as usize] = true;
            d.locked[y as usize] = true;
            locked_edges += 1;
        }
    }
    if locked_edges > 0 {
        report.notes.push(format!(
            "{locked_edges} boundary / non-manifold edges: their vertices are kept in place"
        ));
    }
    let mut heap = BinaryHeap::new();
    let mut keys: Vec<(u32, u32)> = edges.keys().copied().collect();
    keys.sort_unstable();
    for (x, y) in keys {
        d.push(&mut heap, x, y);
    }
    let target = o.target_triangles as usize;
    let mut alive = tris.len();
    while let Some(c) = heap.pop() {
        if target > 0 && alive <= target {
            break;
        }
        if d.version[c.u as usize] != c.vu || d.version[c.v as usize] != c.vv {
            continue;
        }
        let Some((_, p)) = d.candidate(c.u, c.v) else {
            continue;
        };
        // Collapse onto the locked end if any.
        let (keep, gone) = if d.locked[c.v as usize] {
            (c.v, c.u)
        } else {
            (c.u, c.v)
        };
        match d.collapse(keep, gone, p) {
            Ok(()) => {
                report.collapses += 1;
                alive -= 2;
                for w in d.neighbours(keep) {
                    d.push(&mut heap, keep, w);
                }
            }
            Err(0) => report.refused_topology += 1,
            Err(1) => report.refused_normal += 1,
            Err(_) => report.refused_deviation += 1,
        }
    }
    // Compact.
    let mut map = vec![u32::MAX; n];
    let mut out_p = vec![];
    let mut out_t = vec![];
    for (ti, t) in d.tris.iter().enumerate() {
        if !d.alive[ti] {
            continue;
        }
        out_t.push(t.map(|i| {
            if map[i as usize] == u32::MAX {
                map[i as usize] = out_p.len() as u32;
                out_p.push(d.pos[i as usize]);
            }
            map[i as usize]
        }));
    }
    report.triangles_removed = tris.len() - out_t.len();
    report.vertices_moved = (0..n)
        .filter(|&i| map[i] != u32::MAX && d.pos[i] != pos[i])
        .count();
    if target > 0 && out_t.len() > target {
        report.notes.push(format!(
            "stopped at {} triangles (target {target}): no further collapse keeps the deviation \
             within {} mm and the topology",
            out_t.len(),
            o.max_deviation
        ));
    }
    let dev = measure_deviation(pos, tris, &out_p, &out_t)?;
    if dev.max > o.max_deviation * (1.0 + 1e-9) {
        return Err(format!(
            "measured deviation {:.6} mm exceeds the requested {} mm; result refused",
            dev.max, o.max_deviation
        ));
    }
    report.deviation = Some(dev);
    Ok((out_p, out_t))
}

/// Sampled two-sided deviation between two meshes (module docs).
pub fn measure_deviation(
    src_p: &[[f64; 3]],
    src_t: &[[u32; 3]],
    res_p: &[[f64; 3]],
    res_t: &[[u32; 3]],
) -> Result<DeviationReport, String> {
    let gs = TriGrid::new(src_p, src_t).ok_or("the source mesh has no triangles")?;
    let gr = TriGrid::new(res_p, res_t).ok_or("the result mesh has no triangles")?;
    let tri_pts = |p: &[[f64; 3]], t: &[u32; 3]| t.map(|i| p[i as usize]);
    // Vertices referenced by a triangle (unused positions are not surface).
    let used = |p: &[[f64; 3]], t: &[[u32; 3]]| {
        let mut u = vec![false; p.len()];
        for f in t {
            for &i in f {
                u[i as usize] = true;
            }
        }
        p.iter()
            .zip(u)
            .filter_map(|(q, k)| k.then_some(*q))
            .collect::<Vec<_>>()
    };
    // Source -> result: vertices and centroids.
    let mut s_samples: Vec<[f64; 3]> = used(src_p, src_t);
    s_samples.extend(src_t.iter().map(|t| {
        let [a, b, c] = tri_pts(src_p, t);
        scale(add(add(a, b), c), 1.0 / 3.0)
    }));
    // Result -> source: vertices, edge midpoints, centroids.
    let mut r_samples: Vec<[f64; 3]> = used(res_p, res_t);
    let mut seen = HashSet::new();
    for t in res_t {
        let [a, b, c] = tri_pts(res_p, t);
        r_samples.push(scale(add(add(a, b), c), 1.0 / 3.0));
        for k in 0..3 {
            let (x, y) = (t[k], t[(k + 1) % 3]);
            if seen.insert((x.min(y), x.max(y))) {
                r_samples.push(scale(add(res_p[x as usize], res_p[y as usize]), 0.5));
            }
        }
    }
    let stats = |pts: &[[f64; 3]], g: &TriGrid, p: &[[f64; 3]], t: &[[u32; 3]]| {
        let mut max: f64 = 0.0;
        let mut sum = 0.0;
        for q in pts {
            let d = g.nearest(p, t, *q).0;
            max = max.max(d);
            sum += d;
        }
        (max, sum / pts.len().max(1) as f64)
    };
    let (sm, sa) = stats(&s_samples, &gr, res_p, res_t);
    let (rm, ra) = stats(&r_samples, &gs, src_p, src_t);
    Ok(DeviationReport {
        method: format!(
            "exact point-to-triangle distances (uniform grid search) of {} source samples \
             (vertices, triangle centroids) to the result and {} result samples (vertices, \
             edge midpoints, centroids) to the source; sampled, so the maximum is a lower bound \
             of the true (Hausdorff) deviation",
            s_samples.len(),
            r_samples.len()
        ),
        source_to_result_max: sm,
        source_to_result_mean: sa,
        result_to_source_max: rm,
        result_to_source_mean: ra,
        max: sm.max(rm),
        samples: s_samples.len() + r_samples.len(),
    })
}
