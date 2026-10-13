//! STEP shell topology -> Truck compressed shell, with exact handling of faces on
//! periodic surfaces.
//!
//! Truck faces need *simple* boundary wires (no repeated vertex). STEP (OpenCascade)
//! writes a full cylinder / cone wall as one face whose loop runs over a seam edge
//! twice, a full sphere as one face bounded by a single VERTEX_LOOP, and a full
//! torus as one face bounded by two closed seam circles. Such faces are split into
//! simple faces with *exact* new edges:
//! * one seam on a surface of revolution: the seam rotated by pi about the axis
//!   (an affine map of the seam curve, so exact) becomes a new edge; closed circles
//!   crossing it are split there by exact knot insertion;
//! * sphere with a vertex loop at a pole: two exact meridian half circles;
//! * torus with two closed seams: a 2 x 2 grid of exact quarter-torus faces.
//!
//! Every edge is checked against its vertices and against the surfaces of the faces
//! that use it (`tol`), so inconsistent files are rejected rather than imported.
use super::geom::{Analytic, Geo, Placement, analytic_distance, conic_arc, expected_normal};
use super::{StepError, fail};
use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI};
use std::result::Result;
use truck_modeling::*;
use truck_topology::compress::{
    CompressedEdge, CompressedEdgeIndex, CompressedFace, CompressedShell,
};

pub(super) type CShell = CompressedShell<Point3, Curve, Surface>;

/// Upper bound on faces/edges per shell after splitting (allocation guard).
const MAX_ITEMS: usize = 4_000_000;

pub(super) struct Built {
    pub shell: CShell,
    /// STEP surface entity type per face (for statistics).
    pub surface_types: Vec<String>,
    /// STEP curve entity type per edge (for statistics; synthesized edges excluded).
    pub curve_types: Vec<String>,
}

struct B<'a> {
    geo: &'a Geo<'a>,
    tol: f64,
    vmap: HashMap<u64, usize>,
    emap: HashMap<u64, usize>,
    sh: CShell,
    analytic: Vec<Analytic>,
    surface_types: Vec<String>,
    curve_types: Vec<String>,
    /// Faces bounded by a single VERTEX_LOOP (index, vertex index).
    vertex_loop_faces: Vec<(usize, usize)>,
}

pub(super) fn build_shell(geo: &Geo, shell_id: u64, tol: f64) -> Result<Built, StepError> {
    let (ty, p) = geo.idx.simple(shell_id, &["CLOSED_SHELL", "OPEN_SHELL"])?;
    if ty == "OPEN_SHELL" {
        return Err(fail(format!("OPEN_SHELL #{shell_id}: not a solid")));
    }
    let mut b = B {
        geo,
        tol,
        vmap: HashMap::new(),
        emap: HashMap::new(),
        sh: CompressedShell {
            vertices: Vec::new(),
            edges: Vec::new(),
            faces: Vec::new(),
        },
        analytic: Vec::new(),
        surface_types: Vec::new(),
        curve_types: Vec::new(),
        vertex_loop_faces: Vec::new(),
    };
    for fid in p.refs(1)? {
        b.face(fid)?;
    }
    if b.sh.faces.is_empty() {
        return Err(fail(format!("CLOSED_SHELL #{shell_id} has no faces")));
    }
    b.check_edges_on_faces()?;
    b.split_periodic()?;
    b.compact();
    b.split_closed_edges()?;
    Ok(Built {
        shell: b.sh,
        surface_types: b.surface_types,
        curve_types: b.curve_types,
    })
}

impl B<'_> {
    fn vertex(&mut self, vid: u64) -> Result<usize, StepError> {
        if let Some(&i) = self.vmap.get(&vid) {
            return Ok(i);
        }
        let p = self.geo.vertex(vid)?;
        let i = self.push_vertex(p)?;
        self.vmap.insert(vid, i);
        Ok(i)
    }

    fn push_vertex(&mut self, p: Point3) -> Result<usize, StepError> {
        if self.sh.vertices.len() >= MAX_ITEMS {
            return Err(fail("too many vertices".into()));
        }
        self.sh.vertices.push(p);
        Ok(self.sh.vertices.len() - 1)
    }

    fn push_edge(&mut self, a: usize, b: usize, curve: Curve) -> Result<usize, StepError> {
        if self.sh.edges.len() >= MAX_ITEMS {
            return Err(fail("too many edges".into()));
        }
        self.sh.edges.push(CompressedEdge {
            vertices: (a, b),
            curve,
        });
        Ok(self.sh.edges.len() - 1)
    }

    fn edge(&mut self, eid: u64) -> Result<usize, StepError> {
        if let Some(&i) = self.emap.get(&eid) {
            return Ok(i);
        }
        let (_, p) = self.geo.idx.simple(eid, &["EDGE_CURVE"])?;
        let (v1, v2, gid, same) = (p.r(1)?, p.r(2)?, p.r(3)?, p.b(4)?);
        let (a, b) = (self.vertex(v1)?, self.vertex(v2)?);
        let (pa, pb) = (self.sh.vertices[a], self.sh.vertices[b]);
        let closed = v1 == v2;
        let ctx = |e: StepError| {
            fail(format!(
                "EDGE_CURVE #{eid} ({} #{gid}): {}",
                self.geo.idx.type_name(gid),
                e.0
            ))
        };
        let curve = if same {
            self.geo
                .edge_geometry(gid, pa, pb, closed, 0)
                .map_err(ctx)?
        } else {
            let mut c = self
                .geo
                .edge_geometry(gid, pb, pa, closed, 0)
                .map_err(ctx)?;
            c.invert();
            c
        };
        let (t0, t1) = curve.range_tuple();
        let (d0, d1) = (curve.subs(t0).distance(pa), curve.subs(t1).distance(pb));
        if !(d0 <= self.tol && d1 <= self.tol) {
            return Err(fail(format!(
                "EDGE_CURVE #{eid}: curve ends are {d0:.3e} / {d1:.3e} mm from its vertices (tolerance {:.1e} mm)",
                self.tol
            )));
        }
        let i = self.push_edge(a, b, curve)?;
        self.curve_types.push(curve_type_name(self.geo, gid));
        self.emap.insert(eid, i);
        Ok(i)
    }

    fn face(&mut self, fid: u64) -> Result<(), StepError> {
        let (ty, p) = self
            .geo
            .idx
            .simple(fid, &["ADVANCED_FACE", "FACE_SURFACE", "ORIENTED_FACE"])?;
        let (face_id, orientation) = if ty == "ORIENTED_FACE" {
            (p.r(2)?, p.b(3)?)
        } else {
            (fid, true)
        };
        let (fty, fp) = self
            .geo
            .idx
            .simple(face_id, &["ADVANCED_FACE", "FACE_SURFACE"])?;
        let sid = fp.r(2)?;
        let same_sense = fp.b(3)?;
        let (mut surface, analytic) = self
            .geo
            .surface(sid)
            .map_err(|e| fail(format!("{fty} #{face_id}: {}", e.0)))?;
        if !same_sense {
            surface.invert();
        }
        let mut boundaries = Vec::new();
        let mut vertex_loop = None;
        let bounds = fp.refs(1)?;
        for bid in &bounds {
            let (_, bp) = self
                .geo
                .idx
                .simple(*bid, &["FACE_BOUND", "FACE_OUTER_BOUND"])?;
            let (lid, bori) = (bp.r(1)?, bp.b(2)?);
            let (lty, lp) = self.geo.idx.simple(lid, &["EDGE_LOOP", "VERTEX_LOOP"])?;
            if lty == "VERTEX_LOOP" {
                if bounds.len() != 1 || !matches!(analytic, Analytic::Sphere { .. }) {
                    return Err(fail(format!(
                        "VERTEX_LOOP #{lid}: only supported as the single bound of a full sphere face"
                    )));
                }
                vertex_loop = Some(self.vertex(lp.r(1)?)?);
                continue;
            }
            let mut wire = Vec::new();
            for oid in lp.refs(1)? {
                let (oty, op) = self.geo.idx.simple(oid, &["ORIENTED_EDGE", "EDGE_CURVE"])?;
                let (eid, eori) = if oty == "ORIENTED_EDGE" {
                    (op.r(3)?, op.b(4)?)
                } else {
                    (oid, true)
                };
                let index = self.edge(eid)?;
                wire.push(CompressedEdgeIndex {
                    index,
                    orientation: eori == bori,
                });
            }
            if !bori {
                wire.reverse();
            }
            if wire.is_empty() {
                return Err(fail(format!("EDGE_LOOP #{lid} is empty")));
            }
            // Loop connectivity.
            let n = wire.len();
            for i in 0..n {
                let (_, e) = self.ends(wire[i]);
                let (s, _) = self.ends(wire[(i + 1) % n]);
                if e != s {
                    return Err(fail(format!("EDGE_LOOP #{lid} is not connected")));
                }
            }
            boundaries.push(wire);
        }
        let idx = self.sh.faces.len();
        if let Some(v) = vertex_loop {
            self.vertex_loop_faces.push((idx, v));
        } else if boundaries.is_empty() {
            return Err(fail(format!("{fty} #{face_id} has no bounds")));
        }
        self.sh.faces.push(CompressedFace {
            boundaries,
            orientation,
            surface,
        });
        self.analytic.push(analytic);
        self.surface_types.push(self.geo.idx.type_name(sid));
        Ok(())
    }

    /// (start, end) vertex of an oriented edge.
    fn ends(&self, e: CompressedEdgeIndex) -> (usize, usize) {
        let v = self.sh.edges[e.index].vertices;
        if e.orientation { v } else { (v.1, v.0) }
    }

    /// Every edge must lie on the surface of each face that uses it.
    fn check_edges_on_faces(&self) -> Result<(), StepError> {
        for (fi, f) in self.sh.faces.iter().enumerate() {
            for w in &f.boundaries {
                for e in w {
                    let c = &self.sh.edges[e.index].curve;
                    let (t0, t1) = c.range_tuple();
                    for k in 0..=4 {
                        let p = c.subs(t0 + (t1 - t0) * k as f64 / 4.0);
                        let d = surface_distance(&f.surface, &self.analytic[fi], p).ok_or_else(
                            || fail(format!("face {fi}: projection onto surface failed")),
                        )?;
                        if d > self.tol {
                            return Err(fail(format!(
                                "face {fi} ({}): edge lies {d:.3e} mm off the surface (tolerance {:.1e} mm)",
                                self.surface_types[fi], self.tol
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Split faces on periodic surfaces into faces with simple boundary wires.
    fn split_periodic(&mut self) -> Result<(), StepError> {
        for (fi, v) in std::mem::take(&mut self.vertex_loop_faces) {
            self.split_sphere(fi, v, None)?;
        }
        let n = self.sh.faces.len();
        for fi in 0..n {
            let seams = self.seams(fi);
            match seams.len() {
                0 => {}
                1 if matches!(self.analytic[fi], Analytic::Sphere { .. })
                    && self.sh.faces[fi].boundaries.len() == 1
                    && self.sh.faces[fi].boundaries[0].len() == 2 =>
                {
                    // Full sphere bounded by one pole-to-pole seam.
                    let (a, b) = self.sh.edges[seams[0]].vertices;
                    self.split_sphere(fi, a, Some(b))?
                }
                1 => self.split_one_seam(fi, seams[0])?,
                2 if matches!(self.analytic[fi], Analytic::Torus { .. }) => {
                    self.split_torus(fi, &seams)?
                }
                k => {
                    return Err(fail(format!(
                        "face {fi} ({}): {k} seam edges unsupported",
                        self.surface_types[fi]
                    )));
                }
            }
        }
        Ok(())
    }

    /// Edges used twice in the same face.
    fn seams(&self, fi: usize) -> Vec<usize> {
        let mut count: HashMap<usize, usize> = HashMap::new();
        for w in &self.sh.faces[fi].boundaries {
            for e in w {
                *count.entry(e.index).or_default() += 1;
            }
        }
        let mut s: Vec<usize> = count
            .into_iter()
            .filter(|&(_, c)| c >= 2)
            .map(|(i, _)| i)
            .collect();
        s.sort_unstable();
        s
    }

    /// +1 when the face's surface normal (before the face orientation flag) agrees
    /// with the analytic outward normal, -1 otherwise.
    fn normal_sign(&self, fi: usize) -> Result<f64, StepError> {
        let s = &self.sh.faces[fi].surface;
        let ((u0, u1), (v0, v1)) = finite_range(s)?;
        let (u, v) = (u0 + 0.37 * (u1 - u0), v0 + 0.29 * (v1 - v0));
        let n = s.normal(u, v);
        let want = expected_normal(&self.analytic[fi], s.subs(u, v))
            .ok_or_else(|| fail("degenerate normal sample".into()))?;
        let d = n.dot(want);
        if !(d.abs() > 0.5) {
            return Err(fail("surface normal check failed".into()));
        }
        Ok(d.signum())
    }

    /// Replace the face `fi` by faces with the given wires (first replaces `fi`).
    fn replace_face(&mut self, fi: usize, wires: Vec<Vec<CompressedEdgeIndex>>) {
        let template = (
            self.sh.faces[fi].surface.clone(),
            self.sh.faces[fi].orientation,
            self.analytic[fi],
            self.surface_types[fi].clone(),
        );
        for (k, w) in wires.into_iter().enumerate() {
            if k == 0 {
                self.sh.faces[fi].boundaries = vec![w];
            } else {
                self.sh.faces.push(CompressedFace {
                    boundaries: vec![w],
                    orientation: template.1,
                    surface: template.0.clone(),
                });
                self.analytic.push(template.2);
                self.surface_types.push(template.3.clone());
            }
        }
    }

    /// Full sphere (bounded by a vertex loop at a pole, or by one pole-to-pole seam):
    /// rebuilt as 8 triangular faces (2 hemispheres x 4 quarter-turn sectors) bounded
    /// by exact quarter meridians and quarter equator arcs, like Truck's own revolved
    /// spheres. Each hemisphere's faces lie on the surface of revolution of one exact
    /// quarter meridian (a subset of the sphere).
    fn split_sphere(&mut self, fi: usize, v: usize, other: Option<usize>) -> Result<(), StepError> {
        let Analytic::Sphere { o, z, r } = self.analytic[fi] else {
            return Err(fail("vertex loop on a non-sphere".into()));
        };
        let pv = self.sh.vertices[v];
        let up = (pv - o).dot(z) / r;
        if ((pv - o).magnitude() - r).abs() > self.tol || (up.abs() - 1.0).abs() * r > self.tol {
            return Err(fail(format!(
                "sphere face {fi}: boundary vertex is not at a pole of the sphere axis"
            )));
        }
        // Axis a from the given (south) pole to the other (north) one.
        let a = if up < 0.0 { z } else { -z };
        let s_v = v;
        let n_v = match other {
            Some(n) => n,
            None => self.push_vertex(o + a * r)?,
        };
        let x = any_perpendicular(a);
        let y = a.cross(x);
        let dir = |i: usize| {
            let t = FRAC_PI_2 * i as f64;
            x * t.cos() + y * t.sin()
        };
        let mut eq = [0usize; 4];
        for (i, e) in eq.iter_mut().enumerate() {
            *e = self.push_vertex(o + dir(i) * r)?;
        }
        let arc = |c: Vector3, d: Vector3, t0: f64| {
            conic_arc(o, c, d, r, r, t0, FRAC_PI_2).map(Curve::NurbsCurve)
        };
        let (mut lo, mut hi, mut ring) = ([0usize; 4], [0usize; 4], [0usize; 4]);
        for i in 0..4 {
            lo[i] = self.push_edge(s_v, eq[i], arc(dir(i), a, -FRAC_PI_2)?)?;
            hi[i] = self.push_edge(eq[i], n_v, arc(dir(i), a, 0.0)?)?;
            ring[i] = self.push_edge(eq[i], eq[(i + 1) % 4], arc(x, y, FRAC_PI_2 * i as f64)?)?;
        }
        let pl = Placement { o, x, y, z: a };
        let surf = |c: Curve| -> Result<Surface, StepError> {
            super::geom::revolved(c, &pl, Analytic::Sphere { o, z: a, r })
        };
        // Chart (angle about a, latitude): d/dangle x d/dlat is outward, so these
        // loops are counter-clockwise about the outward normal.
        let fw = |i| CompressedEdgeIndex {
            index: i,
            orientation: true,
        };
        let bw = |i| CompressedEdgeIndex {
            index: i,
            orientation: false,
        };
        let mut faces = Vec::new();
        for i in 0..4 {
            let j = (i + 1) % 4;
            // Each face lies on the revolution of its own sector-start meridian, so
            // its angular range is [0, pi/2] (no parameter seam inside the face).
            let lower = surf(self.sh.edges[lo[i]].curve.clone())?;
            let upper = surf(self.sh.edges[hi[i]].curve.clone())?;
            faces.push((vec![fw(lo[j]), bw(ring[i]), bw(lo[i])], lower));
            faces.push((vec![fw(ring[i]), fw(hi[j]), bw(hi[i])], upper));
        }
        self.replace_outward(fi, faces)
    }

    /// Replace face `fi` by faces whose surfaces and loops are oriented outward
    /// (analytic convention); the face orientation flag reproduces the original
    /// face's sense (outward or inward).
    fn replace_outward(
        &mut self,
        fi: usize,
        faces: Vec<(Vec<CompressedEdgeIndex>, Surface)>,
    ) -> Result<(), StepError> {
        let outward = (self.normal_sign(fi)? > 0.0) == self.sh.faces[fi].orientation;
        let (an, ty) = (self.analytic[fi], self.surface_types[fi].clone());
        for (k, (w, surface)) in faces.into_iter().enumerate() {
            let f = CompressedFace {
                boundaries: vec![w],
                orientation: outward,
                surface,
            };
            if k == 0 {
                self.sh.faces[fi] = f;
            } else {
                self.sh.faces.push(f);
                self.analytic.push(an);
                self.surface_types.push(ty.clone());
            }
        }
        Ok(())
    }

    /// Split every remaining closed edge (start vertex == end vertex) at its parameter
    /// midpoint (exact knot insertion); Truck edges need two distinct vertices.
    fn split_closed_edges(&mut self) -> Result<(), StepError> {
        let n = self.sh.edges.len();
        for i in 0..n {
            let (a, b) = self.sh.edges[i].vertices;
            if a == b && self.is_used(i) {
                let c = &self.sh.edges[i].curve;
                let (t0, t1) = c.range_tuple();
                let q = c.subs(0.5 * (t0 + t1));
                self.split_closed_edge(i, q)?;
            }
        }
        Ok(())
    }

    fn is_used(&self, e: usize) -> bool {
        self.sh
            .faces
            .iter()
            .flat_map(|f| f.boundaries.iter().flatten())
            .any(|x| x.index == e)
    }

    /// Drop edges and vertices no face uses (replaced seams), remapping indices.
    fn compact(&mut self) {
        let mut used_e = vec![false; self.sh.edges.len()];
        for f in &self.sh.faces {
            for e in f.boundaries.iter().flatten() {
                used_e[e.index] = true;
            }
        }
        let mut emap = vec![usize::MAX; used_e.len()];
        let mut edges = Vec::new();
        let mut used_v = vec![false; self.sh.vertices.len()];
        for (i, e) in std::mem::take(&mut self.sh.edges).into_iter().enumerate() {
            if used_e[i] {
                emap[i] = edges.len();
                used_v[e.vertices.0] = true;
                used_v[e.vertices.1] = true;
                edges.push(e);
            }
        }
        let mut vmap = vec![usize::MAX; used_v.len()];
        let mut verts = Vec::new();
        for (i, v) in std::mem::take(&mut self.sh.vertices)
            .into_iter()
            .enumerate()
        {
            if used_v[i] {
                vmap[i] = verts.len();
                verts.push(v);
            }
        }
        for e in &mut edges {
            e.vertices = (vmap[e.vertices.0], vmap[e.vertices.1]);
        }
        for f in &mut self.sh.faces {
            for e in f.boundaries.iter_mut().flatten() {
                e.index = emap[e.index];
            }
        }
        self.sh.edges = edges;
        self.sh.vertices = verts;
    }

    /// One seam on a surface of revolution: add the seam rotated by pi about the axis.
    fn split_one_seam(&mut self, fi: usize, seam: usize) -> Result<(), StepError> {
        let (o, z) = self.analytic[fi].axis().ok_or_else(|| {
            fail(format!(
                "face {fi} ({}): seam on a non-revolved surface unsupported",
                self.surface_types[fi]
            ))
        })?;
        if self.sh.faces[fi].boundaries.len() != 1 {
            return Err(fail(format!("face {fi}: seam face with holes unsupported")));
        }
        let rot = half_turn(o, z);
        let (sa, sb) = self.sh.edges[seam].vertices;
        let new_curve = self.sh.edges[seam].curve.transformed(rot);
        let mut ends = [0usize; 2];
        for (k, sv) in [sa, sb].into_iter().enumerate() {
            let p = self.sh.vertices[sv];
            let q = rot.transform_point(p);
            if p.distance(q) <= self.tol {
                ends[k] = sv; // on the axis (pole)
                continue;
            }
            // The closed circle of this face through the seam vertex.
            let closed: Vec<usize> = self.sh.faces[fi].boundaries[0]
                .iter()
                .map(|e| e.index)
                .filter(|&i| {
                    let v = self.sh.edges[i].vertices;
                    v.0 == sv && v.1 == sv
                })
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            let [ci] = closed.as_slice() else {
                return Err(fail(format!(
                    "face {fi}: expected one closed edge at the seam vertex, found {}",
                    closed.len()
                )));
            };
            ends[k] = self.split_closed_edge(*ci, q)?;
        }
        let ne = self.push_edge(ends[0], ends[1], new_curve)?;
        let wire = self.sh.faces[fi].boundaries[0].clone();
        let starts: Vec<usize> = wire.iter().map(|&e| self.ends(e).0).collect();
        let pos = |v: usize| {
            let p: Vec<usize> = (0..starts.len()).filter(|&i| starts[i] == v).collect();
            match p.as_slice() {
                [i] => Ok(*i),
                _ => Err(fail(format!("face {fi}: seam split is ambiguous"))),
            }
        };
        let (i, j) = (pos(ends[0])?, pos(ends[1])?);
        let n = wire.len();
        let path = |from: usize, to: usize| -> Vec<CompressedEdgeIndex> {
            let mut out = Vec::new();
            let mut k = from;
            while k != to {
                out.push(wire[k]);
                k = (k + 1) % n;
            }
            out
        };
        let mut w1 = path(i, j);
        w1.push(CompressedEdgeIndex {
            index: ne,
            orientation: false,
        });
        let mut w2 = path(j, i);
        w2.push(CompressedEdgeIndex {
            index: ne,
            orientation: true,
        });
        if w1.len() < 2 || w2.len() < 2 {
            return Err(fail(format!("face {fi}: degenerate seam split")));
        }
        self.replace_face(fi, vec![w1, w2]);
        Ok(())
    }

    /// Split closed edge `ci` at the point nearest to `q` (exact knot insertion);
    /// returns the new vertex. All uses of the edge in every face are updated.
    fn split_closed_edge(&mut self, ci: usize, q: Point3) -> Result<usize, StepError> {
        let mut c = self.sh.edges[ci].curve.clone();
        let t = c
            .search_nearest_parameter(q, None, 100)
            .ok_or_else(|| fail("split point not found on closed edge".into()))?;
        let d = c.subs(t).distance(q);
        let (t0, t1) = c.range_tuple();
        if d > self.tol || t <= t0 + 1e-9 * (t1 - t0) || t >= t1 - 1e-9 * (t1 - t0) {
            return Err(fail(format!(
                "closed edge cannot be split at the opposite seam point ({d:.3e} mm off)"
            )));
        }
        let tail = c.cut(t);
        let v = self.sh.edges[ci].vertices.0;
        let w = self.push_vertex(c.subs(t))?;
        self.sh.edges[ci] = CompressedEdge {
            vertices: (v, w),
            curve: c,
        };
        let m = self.push_edge(w, v, tail)?;
        for f in &mut self.sh.faces {
            for wire in &mut f.boundaries {
                let mut out = Vec::with_capacity(wire.len() + 1);
                for &e in wire.iter() {
                    if e.index != ci {
                        out.push(e);
                    } else if e.orientation {
                        out.push(e);
                        out.push(CompressedEdgeIndex {
                            index: m,
                            orientation: true,
                        });
                    } else {
                        out.push(CompressedEdgeIndex {
                            index: m,
                            orientation: false,
                        });
                        out.push(e);
                    }
                }
                *wire = out;
            }
        }
        Ok(w)
    }

    /// Full torus face bounded by its two closed seam circles through one vertex:
    /// replaced by a 2 x 2 grid of faces whose edges are exact half circles.
    fn split_torus(&mut self, fi: usize, seams: &[usize]) -> Result<(), StepError> {
        let Analytic::Torus { o, z, big, small } = self.analytic[fi] else {
            return Err(fail("not a torus".into()));
        };
        let users = |e: usize| {
            self.sh
                .faces
                .iter()
                .flat_map(|f| f.boundaries.iter().flatten())
                .filter(|x| x.index == e)
                .count()
        };
        let v = self.sh.edges[seams[0]].vertices.0;
        let ok = self.sh.faces[fi].boundaries.len() == 1
            && self.sh.faces[fi].boundaries[0].len() == 4
            && seams
                .iter()
                .all(|&s| self.sh.edges[s].vertices == (v, v) && users(s) == 2);
        if !ok {
            return Err(fail(format!(
                "torus face {fi}: only a full torus bounded by its two seam circles is supported"
            )));
        }
        let pv = self.sh.vertices[v];
        let d = pv - o;
        let h = d.dot(z);
        let radial = d - z * h;
        let rho = radial.magnitude();
        if !(rho > 0.0) {
            return Err(fail("torus seam vertex on the axis".into()));
        }
        let u0 = radial / rho;
        let e0 = z.cross(u0);
        let phi0 = h.atan2(rho - big);
        // P(theta, phi) = o + (big + small cos phi) u(theta) + small sin phi z
        let u = |th: f64| u0 * th.cos() + e0 * th.sin();
        let point =
            |th: f64, ph: f64| o + u(th) * (big + small * ph.cos()) + z * (small * ph.sin());
        let ths = [0.0, PI];
        let phs = [phi0, phi0 + PI];
        let mut vid = [[0usize; 2]; 2];
        for i in 0..2 {
            for j in 0..2 {
                vid[i][j] = if i == 0 && j == 0 {
                    v
                } else {
                    self.push_vertex(point(ths[i], phs[j]))?
                };
            }
        }
        // Parallels: at phi_j, theta from ths[i] to ths[i] + pi.
        let mut par = [[0usize; 2]; 2];
        for j in 0..2 {
            let ph = phs[j];
            let c = o + z * (small * ph.sin());
            let r = big + small * ph.cos();
            for i in 0..2 {
                let arc = conic_arc(c, u0, e0, r, r, ths[i], PI)?;
                par[i][j] =
                    self.push_edge(vid[i][j], vid[(i + 1) % 2][j], Curve::NurbsCurve(arc))?;
            }
        }
        // Meridians: at theta_i, phi from phs[j] to phs[j] + pi.
        let mut mer = [[0usize; 2]; 2];
        for i in 0..2 {
            let ui = u(ths[i]);
            for j in 0..2 {
                let arc = conic_arc(o + ui * big, ui, z, small, small, phs[j], PI)?;
                mer[i][j] =
                    self.push_edge(vid[i][j], vid[i][(j + 1) % 2], Curve::NurbsCurve(arc))?;
            }
        }
        // Chart (theta, phi): d/dtheta x d/dphi points away from the tube centre
        // (outward), so this loop is counter-clockwise about the outward normal.
        let fw = |i| CompressedEdgeIndex {
            index: i,
            orientation: true,
        };
        let bw = |i| CompressedEdgeIndex {
            index: i,
            orientation: false,
        };
        let pl = Placement { o, x: u0, y: e0, z };
        let mut faces = Vec::new();
        for i in 0..2 {
            for j in 0..2 {
                let w = vec![
                    fw(par[i][j]),
                    fw(mer[(i + 1) % 2][j]),
                    bw(par[i][(j + 1) % 2]),
                    bw(mer[i][j]),
                ];
                // Surface: revolution of this face's own start meridian (no
                // parameter seam inside the face).
                let surface = super::geom::revolved(
                    self.sh.edges[mer[i][j]].curve.clone(),
                    &pl,
                    self.analytic[fi],
                )?;
                faces.push((w, surface));
            }
        }
        self.replace_outward(fi, faces)
    }
}

fn any_perpendicular(z: Vector3) -> Vector3 {
    let a = if z.x.abs() < 0.9 {
        Vector3::unit_x()
    } else {
        Vector3::unit_y()
    };
    (a - z * a.dot(z)).normalize()
}

/// Rotation by pi about the axis through `o` along unit `z`: p -> o + (2 z z^T - I)(p - o).
fn half_turn(o: Point3, z: Vector3) -> Matrix4 {
    let m = Matrix3::new(
        2.0 * z.x * z.x - 1.0,
        2.0 * z.x * z.y,
        2.0 * z.x * z.z,
        2.0 * z.y * z.x,
        2.0 * z.y * z.y - 1.0,
        2.0 * z.y * z.z,
        2.0 * z.z * z.x,
        2.0 * z.z * z.y,
        2.0 * z.z * z.z - 1.0,
    );
    Matrix4::from_translation(o.to_vec())
        * Matrix4::from(m)
        * Matrix4::from_translation(-o.to_vec())
}

type Range2 = ((f64, f64), (f64, f64));

fn finite_range(s: &Surface) -> Result<Range2, StepError> {
    use std::ops::Bound::*;
    let b = |x: std::ops::Bound<f64>| match x {
        Included(v) | Excluded(v) => Some(v),
        Unbounded => None,
    };
    let ((a0, a1), (b0, b1)) = s.parameter_range();
    match (b(a0), b(a1), b(b0), b(b1)) {
        (Some(u0), Some(u1), Some(v0), Some(v1)) => Ok(((u0, u1), (v0, v1))),
        _ => Err(fail("unbounded surface parameter range".into())),
    }
}

/// Distance from `p` to a face surface: exact for planes and analytic surfaces,
/// Truck's nearest-point projection for free-form B-spline surfaces.
pub(super) fn surface_distance(s: &Surface, a: &Analytic, p: Point3) -> Option<f64> {
    if let Surface::Plane(pl) = s {
        return Some((p - pl.origin()).dot(pl.normal()).abs());
    }
    if let Some(d) = analytic_distance(a, p) {
        return Some(d);
    }
    let (u, v) = s.search_nearest_parameter(p, None, 100)?;
    Some(s.subs(u, v).distance(p))
}

fn curve_type_name(geo: &Geo, gid: u64) -> String {
    let mut id = gid;
    for _ in 0..4 {
        match geo.idx.get(id) {
            Ok(e) if e.is_simple("SURFACE_CURVE") || e.is_simple("SEAM_CURVE") => {
                match e.part("SURFACE_CURVE").or(e.part("SEAM_CURVE")) {
                    Some(p) => match p.get(1).and_then(super::ast::as_ref) {
                        Some(next) => id = next,
                        None => break,
                    },
                    None => break,
                }
            }
            _ => break,
        }
    }
    geo.idx.type_name(id)
}
