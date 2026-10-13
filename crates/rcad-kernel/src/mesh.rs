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
        // Canonical wire starts: Truck's planar triangulation depends on where
        // each boundary loop starts (a quad's diagonal follows it), and that
        // start varies between runs like everything keyed by pointer ids.
        let solid = canonical_wires(&body.solid);
        let solid = solid.as_ref().unwrap_or(&body.solid);
        // Run 7: an open surface body cannot go through `Solid` triangulation
        // (Truck rebuilds a closed solid and panics); mesh its shells instead.
        // Face order is the same (the solid's faces are its shells' in order).
        let open = body.is_open_shell();
        let shells: Vec<_> = if open {
            solid
                .boundaries()
                .iter()
                .map(|sh| sh.triangulation(chord))
                .collect()
        } else {
            Vec::new()
        };
        let meshed_solid = (!open).then(|| solid.triangulation(chord));
        let mut out = TriMesh {
            chord_tolerance: chord,
            ..Default::default()
        };
        let mut weld = Welder::new(WELD_TOLERANCE);
        // Visit faces in a canonical geometric order, not Truck's (which follows
        // pointer-derived ids and changes from run to run). Welding keeps the
        // first face's sample of a shared boundary point, and neighbouring faces
        // sample a shared edge ~1e-7 mm apart, so the visiting order decided the
        // welded coordinates: the same document tessellated differently each run.
        let order = canonical_face_order(solid);
        let faces: Vec<_> = match &meshed_solid {
            Some(m) => m.face_iter().collect(),
            None => shells.iter().flat_map(|sh| sh.face_iter()).collect(),
        };
        for fi in order {
            if fi >= faces.len() {
                continue;
            }
            let face = &faces[fi];
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
        let planar: Vec<bool> = solid
            .face_iter()
            .map(|f| matches!(f.surface(), truck_modeling::Surface::Plane(_)))
            .collect();
        canonical_planar_diagonals(&mut out, &planar);
        Ok(out)
    })?
}

/// A copy of `solid` whose face boundary loops start at a canonical edge (the
/// one whose start vertex has the smallest position, rounded to 1e-6 mm) and
/// whose inner loops are sorted the same way. Faces, their order, surfaces and
/// orientation are unchanged. None if the copy cannot be rebuilt.
fn canonical_wires(solid: &truck_modeling::Solid) -> Option<truck_modeling::Solid> {
    use truck_modeling::*;
    let q = |p: Point3| [p.x, p.y, p.z].map(|c| (c * 1.0e6).round() as i64);
    let mut cs = solid.compress();
    for shell in &mut cs.boundaries {
        let verts = shell.vertices.clone();
        let edges = shell.edges.clone();
        let start = |e: &truck_topology::compress::CompressedEdgeIndex| {
            let (a, b) = edges[e.index].vertices;
            q(verts[if e.orientation { a } else { b }])
        };
        for f in &mut shell.faces {
            for w in &mut f.boundaries {
                if let Some(r) = (0..w.len()).min_by_key(|&i| start(&w[i])) {
                    w.rotate_left(r);
                }
            }
            if f.boundaries.len() > 2 {
                f.boundaries[1..].sort_by_key(|w| w.iter().map(start).min());
            }
        }
    }
    Solid::extract(cs).ok()
}

/// Face indices of `solid` (in `face_iter` order) sorted by a geometric key:
/// boundary vertex positions and edge midpoints rounded to 1e-6 mm (the edge
/// midpoints tell apart faces with the same corners, e.g. the two halves of a
/// split cylinder), then the surface kind. Ties keep `face_iter` order.
fn canonical_face_order(solid: &truck_modeling::Solid) -> Vec<usize> {
    use truck_modeling::*;
    let q = |p: Point3| [p.x, p.y, p.z].map(|c| (c * 1.0e6).round() as i64);
    let mut keyed: Vec<(Vec<[i64; 3]>, u8, usize)> = solid
        .face_iter()
        .enumerate()
        .map(|(i, f)| {
            let mut pts: Vec<[i64; 3]> = Vec::new();
            for w in f.absolute_boundaries() {
                for e in w.edge_iter() {
                    pts.push(q(e.front().point()));
                    let c = e.curve();
                    let (t0, t1) = c.range_tuple();
                    let mid = match &c {
                        Curve::IntersectionCurve(ic) => ic
                            .search_triple(0.5 * (t0 + t1))
                            .map(|x| x.0)
                            .unwrap_or_else(|| e.front().point()),
                        other => other.subs(0.5 * (t0 + t1)),
                    };
                    pts.push(q(mid));
                }
            }
            pts.sort();
            let kind = match f.surface() {
                Surface::Plane(_) => 0,
                _ => 1,
            };
            (pts, kind, i)
        })
        .collect();
    keyed.sort();
    keyed.into_iter().map(|(_, _, i)| i).collect()
}

/// Re-triangulate the interior of every planar face to its 3-D Delaunay
/// triangulation (constrained by the face boundary) by edge flips, breaking
/// exact ties symbolically by vertex position.
///
/// Truck triangulates a plane in its (not necessarily orthonormal) parameter
/// space, and a rectangle's two diagonals are an exact Delaunay tie there, so
/// ~1e-15 mm of noise in the B-rep vertices (which differs between runs) chose
/// the diagonal. Any triangulation of a planar polygon is the same surface, so
/// flipping is exact; the tie rule makes the choice depend on positions only.
///
/// Tie rule (simulation of simplicity): the vertex with the smallest position
/// key (rounded to [`CANONICAL_GRID`]) is treated as lifted infinitesimally
/// above the paraboloid, so of two cocircular diagonals the one avoiding it is
/// Delaunay. That is a consistent perturbation of the whole point set, so flips
/// converge to a unique result.
fn canonical_planar_diagonals(m: &mut TriMesh, planar: &[bool]) {
    let key = |p: [f64; 3]| p.map(|c| (c / CANONICAL_GRID).round() as i64);
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    // Directed edge (a, b) -> triangle containing it in that winding.
    let mut by_edge: HashMap<(u32, u32), usize> = HashMap::new();
    for (t, tri) in m.triangles.iter().enumerate() {
        if planar.get(m.tri_face[t] as usize) == Some(&true) {
            for k in 0..3 {
                by_edge.insert((tri[k], tri[(k + 1) % 3]), t);
            }
        }
    }
    // Is edge (a, b), shared by t1 = (a, b, c) and t2 = (b, a, d), to be
    // flipped to (c, d)?
    let wants_flip = |m: &TriMesh, a: u32, b: u32, c: u32, d: u32| -> bool {
        let [pa, pb, pc, pd] = [a, b, c, d].map(|i| m.positions[i as usize]);
        let n = cross(sub(pb, pa), sub(pc, pa));
        let nl = dot(n, n).sqrt();
        let l = [pb, pc, pd]
            .iter()
            .map(|p| dot(sub(*p, pa), sub(*p, pa)).sqrt())
            .fold(0.0f64, f64::max);
        if nl <= 1e-12 * l * l {
            return false;
        }
        // In-plane 2-D frame (the face is planar, so d lies in it as well).
        let ex = sub(pb, pa).map(|c| c / dot(sub(pb, pa), sub(pb, pa)).sqrt());
        let ey = cross(n.map(|c| c / nl), ex);
        let to2 = |p: [f64; 3]| [dot(sub(p, pa), ex), dot(sub(p, pa), ey)];
        let [qa, qb, qc, qd] = [pa, pb, pc, pd].map(to2);
        let orient = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| {
            (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
        };
        // The quad a, d, b, c must be strictly convex for (c, d) to be valid.
        let eps = 1e-12 * l * l;
        if orient(qd, qc, qa) <= eps || orient(qc, qd, qb) <= eps {
            return false;
        }
        // In-circle of d against (a, b, c) (counter-clockwise in this frame).
        let row = |p: [f64; 2]| {
            let (x, y) = (p[0] - qd[0], p[1] - qd[1]);
            [x, y, x * x + y * y]
        };
        let [r0, r1, r2] = [qa, qb, qc].map(row);
        let det = r0[0] * (r1[1] * r2[2] - r1[2] * r2[1]) - r0[1] * (r1[0] * r2[2] - r1[2] * r2[0])
            + r0[2] * (r1[0] * r2[1] - r1[1] * r2[0]);
        if det.abs() > 1e-9 * l.powi(4) {
            return det > 0.0;
        }
        // Cocircular: the diagonal must avoid the smallest-key vertex.
        let lowest = [a, b, c, d]
            .into_iter()
            .min_by_key(|&i| key(m.positions[i as usize]))
            .unwrap();
        lowest == a || lowest == b
    };
    let mut stack: Vec<(u32, u32)> = by_edge.keys().copied().filter(|(a, b)| a < b).collect();
    stack.sort_unstable();
    // Lawson flips terminate in O(n^2); the cap only guards against a tolerance
    // inconsistency turning that into a cycle.
    let mut budget = 4 * m.triangles.len() * m.triangles.len().max(16);
    while let Some((a, b)) = stack.pop() {
        if budget == 0 {
            break;
        }
        budget -= 1;
        let (Some(&t1), Some(&t2)) = (by_edge.get(&(a, b)), by_edge.get(&(b, a))) else {
            continue;
        };
        if m.tri_face[t1] != m.tri_face[t2] {
            continue;
        }
        let third =
            |t: usize, x: u32, y: u32| *m.triangles[t].iter().find(|&&v| v != x && v != y).unwrap();
        let (c, d) = (third(t1, a, b), third(t2, a, b));
        if c == d || by_edge.contains_key(&(c, d)) || by_edge.contains_key(&(d, c)) {
            continue;
        }
        if !wants_flip(m, a, b, c, d) {
            continue;
        }
        for tri in [[a, b, c], [b, a, d]] {
            for k in 0..3 {
                by_edge.remove(&(tri[k], tri[(k + 1) % 3]));
            }
        }
        // Quad a, d, b, c (counter-clockwise) split along c-d.
        m.triangles[t1] = [a, d, c];
        m.triangles[t2] = [d, b, c];
        for (t, tri) in [(t1, [a, d, c]), (t2, [d, b, c])] {
            for k in 0..3 {
                by_edge.insert((tri[k], tri[(k + 1) % 3]), t);
            }
        }
        for (x, y) in [(a, d), (d, b), (b, c), (c, a)] {
            stack.push((x.min(y), x.max(y)));
        }
    }
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

/// Relative zero-area threshold for [`MeshCheck::degenerate_triangles`]:
/// `|(b - a) x (c - a)| <= DEGENERATE_REL_AREA * (longest edge)^2`. Dimensionless.
pub const DEGENERATE_REL_AREA: f64 = 1e-12;

/// What [`check_mesh`] says about self-intersection (it does not test it).
pub const SELF_INTERSECTION_NOT_CHECKED: &str = "not checked";

/// Topological/geometric diagnostics of an indexed triangle mesh. All topology is
/// by vertex **index** (call [`weld_exact`] first on meshes with unshared corners).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshCheck {
    pub triangles: usize,
    pub vertices: usize,
    /// Every undirected edge is used by exactly two triangles with opposite
    /// direction, and every triangle has three distinct in-range vertices.
    pub watertight: bool,
    pub boundary_edges: usize,
    pub nonmanifold_edges: usize,
    pub misoriented_edges: usize,
    /// Signed volume by the divergence theorem over triangles (a mesh value).
    pub mesh_volume: f64,
    pub bounds_min: [f64; 3],
    pub bounds_max: [f64; 3],
    pub chord_tolerance: f64,
    /// Every edge is used by exactly two triangles (direction not considered).
    #[serde(default)]
    pub closed_edges: bool,
    /// Vertices whose incident triangles form more than one edge-connected fan
    /// (e.g. two shells touching at a point).
    #[serde(default)]
    pub nonmanifold_vertices: usize,
    /// Every two-triangle edge is traversed in opposite directions.
    #[serde(default)]
    pub orientation_consistent: bool,
    /// `mesh_volume > 0`: triangles wound counter-clockwise seen from outside.
    #[serde(default)]
    pub outward: bool,
    /// Zero-area triangles with three distinct vertices (see [`DEGENERATE_REL_AREA`]).
    #[serde(default)]
    pub degenerate_triangles: usize,
    /// Triangles with a repeated or out-of-range vertex index (excluded from the
    /// edge analysis).
    #[serde(default)]
    pub invalid_triangles: usize,
    /// Edge-connected components (shells).
    #[serde(default)]
    pub shells: usize,
    /// Always [`SELF_INTERSECTION_NOT_CHECKED`]: triangle-triangle intersection is
    /// not tested, so a closed, manifold, outward mesh may still intersect itself.
    #[serde(default = "not_checked")]
    pub self_intersection: String,
}

fn not_checked() -> String {
    SELF_INTERSECTION_NOT_CHECKED.to_string()
}

impl MeshCheck {
    /// Closed, manifold (edges and vertices), consistently and outwardly oriented,
    /// every triangle valid. Degenerate (zero-area) triangles are reported but do
    /// not invalidate a mesh that is otherwise a closed solid.
    pub fn is_valid_solid(&self) -> bool {
        self.watertight
            && self.closed_edges
            && self.nonmanifold_vertices == 0
            && self.invalid_triangles == 0
            && self.outward
    }

    /// Human-readable reasons why [`MeshCheck::is_valid_solid`] fails (empty when valid).
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if self.triangles == 0 {
            p.push("no triangles".into());
        }
        if self.boundary_edges > 0 {
            p.push(format!(
                "{} boundary (open) edges: the surface has holes",
                self.boundary_edges
            ));
        }
        if self.nonmanifold_edges > 0 {
            p.push(format!(
                "{} non-manifold edges (shared by 3+ triangles)",
                self.nonmanifold_edges
            ));
        }
        if self.misoriented_edges > 0 {
            p.push(format!(
                "{} inconsistently oriented edges (flipped triangles)",
                self.misoriented_edges
            ));
        }
        if self.nonmanifold_vertices > 0 {
            p.push(format!(
                "{} non-manifold vertices (shells touching at a point)",
                self.nonmanifold_vertices
            ));
        }
        if self.invalid_triangles > 0 {
            p.push(format!(
                "{} triangles with repeated/out-of-range vertices",
                self.invalid_triangles
            ));
        }
        if self.triangles > 0 && !self.outward {
            p.push(format!(
                "signed volume {:.6} mm^3 is not positive (inside-out or open)",
                self.mesh_volume
            ));
        }
        p
    }
}

fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross3(u: [f64; 3], v: [f64; 3]) -> [f64; 3] {
    [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ]
}

fn norm3(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn uf_find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn uf_union(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (uf_find(parent, a), uf_find(parent, b));
    if ra != rb {
        parent[ra.max(rb)] = ra.min(rb);
    }
}

pub fn check_mesh(m: &TriMesh) -> MeshCheck {
    let nv = m.positions.len();
    let valid = |t: &[u32; 3]| {
        t.iter().all(|&i| (i as usize) < nv) && t[0] != t[1] && t[1] != t[2] && t[0] != t[2]
    };
    let mut edges: HashMap<(u32, u32), (i32, i32)> = HashMap::new();
    let mut vol = 0.0;
    let (mut invalid, mut degenerate) = (0, 0);
    // (vertex, triangle, other corner, other corner) for the vertex-fan test.
    let mut corners: Vec<(u32, usize, u32, u32)> = Vec::with_capacity(3 * m.triangles.len());
    let mut parent: Vec<usize> = (0..m.triangles.len()).collect();
    let mut edge_tri: HashMap<(u32, u32), usize> = HashMap::new();
    for (ti, t) in m.triangles.iter().enumerate() {
        if !valid(t) {
            invalid += 1;
            continue;
        }
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let key = (a.min(b), a.max(b));
            let e = edges.entry(key).or_default();
            if a < b {
                e.0 += 1
            } else {
                e.1 += 1
            }
            match edge_tri.get(&key) {
                Some(&o) => uf_union(&mut parent, o, ti),
                None => {
                    edge_tri.insert(key, ti);
                }
            }
            corners.push((t[k], ti, t[(k + 1) % 3], t[(k + 2) % 3]));
        }
        let [p, q, r] = t.map(|i| m.positions[i as usize]);
        vol += p[0] * (q[1] * r[2] - q[2] * r[1]) - p[1] * (q[0] * r[2] - q[2] * r[0])
            + p[2] * (q[0] * r[1] - q[1] * r[0]);
        let (u, v, w) = (sub3(q, p), sub3(r, p), sub3(r, q));
        let longest = norm3(u).max(norm3(v)).max(norm3(w));
        if norm3(cross3(u, v)) <= DEGENERATE_REL_AREA * longest * longest {
            degenerate += 1;
        }
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
    // Vertex fans: the triangles around a vertex, joined through the edges at that
    // vertex; more than one group is a non-manifold vertex.
    corners.sort_unstable_by_key(|c| (c.0, c.1));
    let mut nonmanifold_vertices = 0;
    let mut i = 0;
    while i < corners.len() {
        let v = corners[i].0;
        let mut j = i;
        while j < corners.len() && corners[j].0 == v {
            j += 1;
        }
        let group = &corners[i..j];
        if group.len() > 1 {
            let mut local: Vec<usize> = (0..group.len()).collect();
            let mut by_other: HashMap<u32, usize> = HashMap::new();
            for (gi, c) in group.iter().enumerate() {
                for w in [c.2, c.3] {
                    match by_other.get(&w) {
                        Some(&o) => uf_union(&mut local, o, gi),
                        None => {
                            by_other.insert(w, gi);
                        }
                    }
                }
            }
            let fans = (0..group.len())
                .filter(|&g| uf_find(&mut local, g) == g)
                .count();
            if fans > 1 {
                nonmanifold_vertices += 1;
            }
        }
        i = j;
    }
    let shells = (0..m.triangles.len())
        .filter(|&t| valid(&m.triangles[t]) && uf_find(&mut parent, t) == t)
        .count();
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in &m.positions {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let mesh_volume = vol / 6.0;
    MeshCheck {
        triangles: m.triangles.len(),
        vertices: m.positions.len(),
        watertight: boundary == 0
            && nonmanifold == 0
            && misoriented == 0
            && invalid == 0
            && !m.triangles.is_empty(),
        boundary_edges: boundary,
        nonmanifold_edges: nonmanifold,
        misoriented_edges: misoriented,
        mesh_volume,
        bounds_min: lo,
        bounds_max: hi,
        chord_tolerance: m.chord_tolerance,
        closed_edges: boundary == 0 && nonmanifold == 0 && !m.triangles.is_empty(),
        nonmanifold_vertices,
        orientation_consistent: misoriented == 0,
        outward: mesh_volume > 0.0,
        degenerate_triangles: degenerate,
        invalid_triangles: invalid,
        shells,
        self_intersection: not_checked(),
    }
}

/// Copy of `m` with bitwise-equal positions (`-0.0 == 0.0`) merged in first-use
/// order and triangles re-indexed (a merged vertex keeps its first normal). For
/// checking meshes stored with unshared corners (imported mesh bodies).
pub fn weld_exact(m: &TriMesh) -> TriMesh {
    let key = |p: [f64; 3]| p.map(|c| if c == 0.0 { 0u64 } else { c.to_bits() });
    let mut map: HashMap<[u64; 3], u32> = HashMap::new();
    let mut out = TriMesh {
        chord_tolerance: m.chord_tolerance,
        ..Default::default()
    };
    let mut remap = Vec::with_capacity(m.positions.len());
    for (i, p) in m.positions.iter().enumerate() {
        let id = *map.entry(key(*p)).or_insert_with(|| {
            out.positions.push(*p);
            out.normals.push(
                <[_]>::get(&m.normals, i)
                    .copied()
                    .unwrap_or([0.0, 0.0, 1.0]),
            );
            (out.positions.len() - 1) as u32
        });
        remap.push(id);
    }
    out.triangles = m
        .triangles
        .iter()
        .map(|t| t.map(|i| <[_]>::get(&remap, i as usize).copied().unwrap_or(u32::MAX)))
        .collect();
    out.tri_face = m.tri_face.clone();
    out
}

/// Largest angle (degrees) between the normals of two triangles that share an
/// edge and belong to the same B-rep face (`tri_face`): the facet-to-facet angular
/// deviation of the tessellation of curved faces. Edges between different B-rep
/// faces (model edges) are excluded, so a planar-only mesh gives 0.
pub fn max_facet_angle_deg(m: &TriMesh) -> f64 {
    let normal = |t: &[u32; 3]| {
        let [p, q, r] = t.map(|i| m.positions[i as usize]);
        let n = cross3(sub3(q, p), sub3(r, p));
        let l = norm3(n);
        (l > 0.0).then(|| n.map(|c| c / l))
    };
    let mut first: HashMap<(u32, u32), usize> = HashMap::new();
    let mut worst: f64 = 0.0;
    for (ti, t) in m.triangles.iter().enumerate() {
        if t.iter().any(|&i| i as usize >= m.positions.len()) {
            continue;
        }
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let key = (a.min(b), a.max(b));
            match first.get(&key) {
                Some(&o) if <[_]>::get(&m.tri_face, o) == <[_]>::get(&m.tri_face, ti) => {
                    if let (Some(n1), Some(n2)) = (normal(&m.triangles[o]), normal(t)) {
                        let d = (n1[0] * n2[0] + n1[1] * n2[1] + n1[2] * n2[2]).clamp(-1.0, 1.0);
                        worst = worst.max(d.acos().to_degrees());
                    }
                }
                Some(_) => {}
                None => {
                    first.insert(key, ti);
                }
            }
        }
    }
    worst
}

/// Rigid placement `p' = R p + t` with `R` a proper rotation (orthonormal, det +1).
/// Construction refuses reflections and non-orthonormal matrices, so a placed mesh
/// keeps volume, distances and handedness (outward winding stays outward).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RigidTransform {
    /// Row-major rotation.
    pub rotation: [[f64; 3]; 3],
    pub translation: [f64; 3],
}

impl RigidTransform {
    pub const IDENTITY: RigidTransform = RigidTransform {
        rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        translation: [0.0; 3],
    };

    /// Checked constructor: `rotation` must be orthonormal with det +1 within 1e-9.
    pub fn new(rotation: [[f64; 3]; 3], translation: [f64; 3]) -> Result<Self> {
        let r = rotation;
        let finite = r
            .iter()
            .flatten()
            .chain(&translation)
            .all(|c| c.is_finite());
        let det = r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1])
            - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
            + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0]);
        let mut ortho = true;
        for i in 0..3 {
            for j in 0..3 {
                let d: f64 = (0..3).map(|k| r[i][k] * r[j][k]).sum();
                ortho &= (d - if i == j { 1.0 } else { 0.0 }).abs() <= 1e-9;
            }
        }
        if !finite || !ortho || (det - 1.0).abs() > 1e-9 {
            return Err(KernelError::InvalidInput(format!(
                "placement must be a proper rotation (orthonormal, det +1); det = {det}"
            )));
        }
        Ok(Self {
            rotation,
            translation,
        })
    }

    /// Proper rotation taking direction `n` to `-Z` (shortest arc; for `n = +Z`
    /// a half turn about X).
    pub fn rotation_to_minus_z(n: [f64; 3]) -> Result<Self> {
        let l = norm3(n);
        if !(l.is_finite() && l > 0.0) {
            return Err(KernelError::InvalidInput("zero face normal".into()));
        }
        let a = n.map(|c| c / l);
        let c = -a[2]; // a . (0, 0, -1)
        let r = if c > 1.0 - 1e-12 {
            Self::IDENTITY.rotation
        } else if c < -1.0 + 1e-12 {
            [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]]
        } else {
            // Rodrigues: R = I + [v]x + [v]x^2 / (1 + c), v = a x (0, 0, -1).
            let v = cross3(a, [0.0, 0.0, -1.0]);
            let vx = [[0.0, -v[2], v[1]], [v[2], 0.0, -v[0]], [-v[1], v[0], 0.0]];
            let mut r = Self::IDENTITY.rotation;
            for i in 0..3 {
                for j in 0..3 {
                    let vx2: f64 = (0..3).map(|k| vx[i][k] * vx[k][j]).sum();
                    r[i][j] += vx[i][j] + vx2 / (1.0 + c);
                }
            }
            r
        };
        Self::new(r, [0.0; 3])
    }

    pub fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        let t = self.translation;
        let v = self.rotate(p);
        [v[0] + t[0], v[1] + t[1], v[2] + t[2]]
    }

    pub fn rotate(&self, v: [f64; 3]) -> [f64; 3] {
        let r = &self.rotation;
        [
            r[0][0] * v[0] + r[0][1] * v[1] + r[0][2] * v[2],
            r[1][0] * v[0] + r[1][1] * v[1] + r[1][2] * v[2],
            r[2][0] * v[0] + r[2][1] * v[1] + r[2][2] * v[2],
        ]
    }

    /// `self` followed by a translation by `t`.
    pub fn then_translate(mut self, t: [f64; 3]) -> Self {
        for (a, b) in self.translation.iter_mut().zip(t) {
            *a += b;
        }
        self
    }

    /// Copy of `m` placed by `self`; triangle order and winding unchanged (a proper
    /// rotation keeps outward winding outward).
    pub fn apply_mesh(&self, m: &TriMesh) -> TriMesh {
        TriMesh {
            positions: m.positions.iter().map(|p| self.apply(*p)).collect(),
            normals: m.normals.iter().map(|n| self.rotate(*n)).collect(),
            triangles: m.triangles.clone(),
            tri_face: m.tri_face.clone(),
            chord_tolerance: m.chord_tolerance,
        }
    }
}

/// Binary STL bytes (little-endian, f32 as the format requires).
pub fn write_stl_binary(m: &TriMesh, name: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(84 + 50 * m.triangles.len());
    let mut header = [0u8; 80];
    let tag = format!("R-CAD STL {name} chord={}mm", m.chord_tolerance);
    let n = tag.len().min(80);
    header[..n].copy_from_slice(&tag.as_bytes()[..n]);
    out.extend_from_slice(&header);
    out.extend_from_slice(&(m.triangles.len() as u32).to_le_bytes());
    for t in &m.triangles {
        // The facet normal is computed from the f32 positions actually
        // written, so it follows from the file's own coordinates (and does not
        // carry f64 noise below what the file can express).
        let [p, q, r] = t.map(|i| m.positions[i as usize].map(|c| f64::from(c as f32)));
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

/// Grid (mm) of the ordering keys of [`canonical_mesh`]: far above the
/// floating-point noise of recomputed geometry (~1e-14 mm), far below the
/// distance between distinct welded vertices (>= 1e-5 mm).
pub const CANONICAL_GRID: f64 = 1.0e-6;

/// The same triangles in a canonical order, independent of the order in which
/// the kernel happened to visit faces and edges (Truck keys its topology by
/// pointer-derived ids, so that order changes from run to run): vertices sorted
/// by position (keys rounded to [`CANONICAL_GRID`]; positions themselves are not
/// changed), each triangle rotated to start at its lowest vertex index with its
/// winding kept, triangles sorted. `tri_face` follows its triangle.
pub fn canonical_mesh(m: &TriMesh) -> TriMesh {
    let key = |p: [f64; 3]| p.map(|c| (c / CANONICAL_GRID).round() as i64);
    let mut order: Vec<u32> = (0..m.positions.len() as u32).collect();
    order.sort_by_key(|&i| (key(m.positions[i as usize]), i));
    let mut new_index = vec![0u32; m.positions.len()];
    for (n, &o) in order.iter().enumerate() {
        new_index[o as usize] = n as u32;
    }
    let positions: Vec<[f64; 3]> = order.iter().map(|&o| m.positions[o as usize]).collect();
    let normals: Vec<[f64; 3]> = order
        .iter()
        .map(|&o| {
            let o = o as usize;
            if o < m.normals.len() {
                m.normals[o]
            } else {
                [0.0, 0.0, 1.0]
            }
        })
        .collect();
    let mut tris: Vec<([u32; 3], u32)> = m
        .triangles
        .iter()
        .enumerate()
        .map(|(ti, t)| {
            let t = t.map(|i| new_index[i as usize]);
            let r = (0..3).min_by_key(|&k| t[k]).unwrap_or(0);
            (
                [t[r], t[(r + 1) % 3], t[(r + 2) % 3]],
                if ti < m.tri_face.len() {
                    m.tri_face[ti]
                } else {
                    0
                },
            )
        })
        .collect();
    tris.sort();
    TriMesh {
        positions,
        normals,
        triangles: tris.iter().map(|(t, _)| *t).collect(),
        tri_face: tris.iter().map(|(_, f)| *f).collect(),
        chord_tolerance: m.chord_tolerance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(tris: Vec<[u32; 3]>, pts: Vec<[f64; 3]>) -> TriMesh {
        TriMesh {
            tri_face: vec![0; tris.len()],
            normals: vec![[0.0, 0.0, 1.0]; pts.len()],
            positions: pts,
            triangles: tris,
            chord_tolerance: 0.1,
        }
    }

    fn tri_set(m: &TriMesh) -> Vec<[[i64; 3]; 3]> {
        let mut v: Vec<_> = m
            .triangles
            .iter()
            .map(|t| {
                let mut k = t.map(|i| m.positions[i as usize].map(|c| (c * 1e6).round() as i64));
                let r = (0..3).min_by_key(|&i| k[i]).unwrap();
                k.rotate_left(r);
                k
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn cocircular_quad_diagonal_is_canonical() {
        // A sloped 60 x 1.41 rectangle (the bracket chamfer) with ~1e-15 noise,
        // triangulated along either diagonal: both must end up the same.
        let pts = vec![
            [-2.1e-13, -1.35e-13, 4.000_000_000_000_003_6],
            [60.0, -1.5e-13, 4.0],
            [60.0, 1.0, 5.0],
            [-2.1e-13, 1.0, 5.0],
        ];
        let mut a = quad(vec![[0, 1, 2], [0, 2, 3]], pts.clone());
        let mut b = quad(vec![[0, 1, 3], [1, 2, 3]], pts);
        canonical_planar_diagonals(&mut a, &[true]);
        canonical_planar_diagonals(&mut b, &[true]);
        assert_eq!(tri_set(&a), tri_set(&b));
        // The smallest-key vertex (index 0) is avoided by the diagonal.
        assert!(a.triangles.iter().filter(|t| t.contains(&0)).count() == 1);
        // Non-planar faces are left alone.
        let mut c = quad(vec![[0, 1, 2], [0, 2, 3]], a.positions.clone());
        canonical_planar_diagonals(&mut c, &[false]);
        assert_eq!(c.triangles, vec![[0, 1, 2], [0, 2, 3]]);
    }

    #[test]
    fn non_delaunay_diagonal_is_flipped_and_winding_kept() {
        // Long thin rhombus split along its long diagonal -> flipped to the short one.
        let pts = vec![
            [0.0, 0.0, 0.0],
            [10.0, -1.0, 0.0],
            [20.0, 0.0, 0.0],
            [10.0, 1.0, 0.0],
        ];
        let mut m = quad(vec![[0, 1, 2], [0, 2, 3]], pts);
        canonical_planar_diagonals(&mut m, &[true]);
        for t in &m.triangles {
            assert!(t.contains(&1) && t.contains(&3), "{t:?}");
            let [p, q, r] = t.map(|i| m.positions[i as usize]);
            let z = (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
            assert!(z > 0.0, "winding flipped: {t:?}");
        }
    }
}
