//! Topological shell merging (run 7, ADR 0012): exact gluing of closed solids
//! along coincident faces (sheet-metal flanges onto their plate), and analysis,
//! stitching, planar filling and thickening of open shells (surface bodies).
//!
//! Everything works on Truck's compressed representation: vertices within a
//! tolerance become one vertex, edges with the same end vertices and the same
//! midpoint (within the tolerance) become one edge, and faces keep their exact
//! surfaces. No geometry is approximated: the only change a merge makes to the
//! geometry is that a straight edge whose end vertices moved (healing, within
//! the stated tolerance) is rebuilt as the exact line between the merged
//! vertices. Curved edges keep their curve; their end points can then be off
//! the merged vertex by at most the tolerance, which is reported.
//!
//! An open shell is held in a [`Body`] whose solid is built without Truck's
//! closedness check (`Solid::new_unchecked`); [`Body::is_open_shell`] tells it
//! apart and every solid-only operation must refuse it (the document does so
//! in one place).
use crate::{Body, KernelError, Result, guarded};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use truck_modeling::*;
use truck_topology::compress::{
    CompressedEdge, CompressedEdgeIndex, CompressedFace, CompressedShell, CompressedSolid,
};
use truck_topology::shell::ShellCondition;

type CShell = CompressedShell<Point3, Curve, Surface>;

impl Body {
    /// True when some boundary shell of the body is not closed: an open
    /// surface body (no volume, no inside).
    pub fn is_open_shell(&self) -> bool {
        self.solid
            .boundaries()
            .iter()
            .any(|s| s.shell_condition() != ShellCondition::Closed)
    }

    /// A body holding one open (or closed) shell as it is, without the closed
    /// solid check. Used for surface bodies.
    pub(crate) fn from_shell_unchecked(shell: Shell) -> Body {
        Body {
            solid: Solid::new_unchecked(vec![shell]),
        }
    }
}

fn dist(a: Point3, b: Point3) -> f64 {
    (a - b).magnitude()
}

fn eval(c: &Curve, t: f64) -> Option<Point3> {
    match c {
        Curve::IntersectionCurve(ic) => ic.search_triple(t).map(|r| r.0),
        other => Some(other.subs(t)),
    }
}

fn curve_mid(c: &Curve) -> Option<Point3> {
    let (t0, t1) = c.range_tuple();
    eval(c, 0.5 * (t0 + t1))
}

/// Merge statistics.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MergeStats {
    /// Vertices merged into another vertex.
    pub vertices_merged: usize,
    /// Largest distance between two merged vertices (mm).
    pub max_vertex_gap: f64,
    /// Edges merged into another edge.
    pub edges_merged: usize,
    /// Largest distance between the midpoints of two merged edges (mm).
    pub max_edge_gap: f64,
    /// Pairs of coincident opposite faces removed (gluing).
    pub faces_glued: usize,
    /// Faces whose orientation was reversed to match their neighbours.
    pub faces_flipped: usize,
}

/// Union of `shells` with vertices/edges merged within `tol` (a negative
/// `tol` only renumbers: nothing is merged).
fn merge(shells: Vec<CShell>, tol: f64, stats: &mut MergeStats) -> Result<CShell> {
    let mut out = CShell {
        vertices: Vec::new(),
        edges: Vec::new(),
        faces: Vec::new(),
    };
    for sh in shells {
        let (vo, eo) = (out.vertices.len(), out.edges.len());
        out.vertices.extend(sh.vertices);
        for e in sh.edges {
            out.edges.push(CompressedEdge {
                vertices: (e.vertices.0 + vo, e.vertices.1 + vo),
                curve: e.curve,
            });
        }
        for mut f in sh.faces {
            for w in &mut f.boundaries {
                for ei in w {
                    ei.index += eo;
                }
            }
            out.faces.push(f);
        }
    }
    // Vertices: sweep along x, union into the earliest index.
    let n = out.vertices.len();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| out.vertices[a].x.total_cmp(&out.vertices[b].x));
    let mut rep: Vec<usize> = (0..n).collect();
    fn find(rep: &mut [usize], mut i: usize) -> usize {
        while rep[i] != i {
            rep[i] = rep[rep[i]];
            i = rep[i];
        }
        i
    }
    for a in 0..n {
        if tol < 0.0 {
            break;
        }
        let pa = out.vertices[order[a]];
        for &ob in &order[a + 1..] {
            let pb = out.vertices[ob];
            if pb.x - pa.x > tol {
                break;
            }
            let d = dist(pa, pb);
            if d <= tol {
                let (ra, rb) = (find(&mut rep, order[a]), find(&mut rep, ob));
                if ra != rb {
                    let (lo, hi) = (ra.min(rb), ra.max(rb));
                    rep[hi] = lo;
                    stats.vertices_merged += 1;
                    stats.max_vertex_gap = stats.max_vertex_gap.max(d);
                }
            }
        }
    }
    let vmap: Vec<usize> = (0..n).map(|i| find(&mut rep, i)).collect();
    // Edges: same (unordered) merged end vertices and midpoints within tol.
    let mut emap: Vec<(usize, bool)> = Vec::with_capacity(out.edges.len());
    let mut by_ends: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    let mut mids = Vec::with_capacity(out.edges.len());
    for e in &out.edges {
        mids.push(curve_mid(&e.curve).ok_or_else(|| {
            KernelError::InvalidInput("an edge curve cannot be evaluated".into())
        })?);
    }
    for (i, e) in out.edges.iter().enumerate() {
        let (a, b) = (vmap[e.vertices.0], vmap[e.vertices.1]);
        if a == b {
            return Err(KernelError::InvalidInput(format!(
                "edge {i} collapses to a point at tolerance {tol} mm (its ends are {:.6} mm apart)",
                dist(out.vertices[e.vertices.0], out.vertices[e.vertices.1])
            )));
        }
        let key = (a.min(b), a.max(b));
        let cands = by_ends.entry(key).or_default();
        let mut found = None;
        for &c in cands.iter() {
            let d = dist(mids[c], mids[i]);
            if d <= tol {
                found = Some((c, d));
                break;
            }
        }
        match found {
            Some((c, d)) => {
                let ce = &out.edges[c];
                let same = vmap[ce.vertices.0] == a;
                emap.push((c, same));
                stats.edges_merged += 1;
                stats.max_edge_gap = stats.max_edge_gap.max(d);
            }
            None => {
                cands.push(i);
                emap.push((i, true));
            }
        }
    }
    for f in &mut out.faces {
        for w in &mut f.boundaries {
            for ei in w {
                let (to, same) = emap[ei.index];
                ei.index = to;
                if !same {
                    ei.orientation = !ei.orientation;
                }
            }
        }
    }
    for e in &mut out.edges {
        e.vertices = (vmap[e.vertices.0], vmap[e.vertices.1]);
    }
    // Straight edges between moved vertices: the exact line between them.
    for e in &mut out.edges {
        if let Curve::Line(l) = &mut e.curve {
            let (p, q) = (out.vertices[e.vertices.0], out.vertices[e.vertices.1]);
            if dist(l.0, p) > 0.0 || dist(l.1, q) > 0.0 {
                *l = Line(p, q);
            }
        }
    }
    Ok(out)
}

/// Drop unused edges and vertices, renumbering.
fn compact(sh: &mut CShell) {
    let mut used_e = vec![false; sh.edges.len()];
    for f in &sh.faces {
        for w in &f.boundaries {
            for ei in w {
                used_e[ei.index] = true;
            }
        }
    }
    let mut new_e = vec![usize::MAX; sh.edges.len()];
    let mut edges = Vec::new();
    let mut used_v = vec![false; sh.vertices.len()];
    for (i, e) in sh.edges.iter().enumerate() {
        if used_e[i] {
            new_e[i] = edges.len();
            used_v[e.vertices.0] = true;
            used_v[e.vertices.1] = true;
            edges.push(e.clone());
        }
    }
    let mut new_v = vec![usize::MAX; sh.vertices.len()];
    let mut verts = Vec::new();
    for (i, p) in sh.vertices.iter().enumerate() {
        if used_v[i] {
            new_v[i] = verts.len();
            verts.push(*p);
        }
    }
    for e in &mut edges {
        e.vertices = (new_v[e.vertices.0], new_v[e.vertices.1]);
    }
    for f in &mut sh.faces {
        for w in &mut f.boundaries {
            for ei in w {
                ei.index = new_e[ei.index];
            }
        }
    }
    sh.edges = edges;
    sh.vertices = verts;
}

/// Effective direction in which face `f` uses an edge (true: along the edge).
fn uses_forward(f: &CompressedFace<Surface>, ei: &CompressedEdgeIndex) -> bool {
    ei.orientation == f.orientation
}

/// Outward normal of a planar face (None for other surfaces).
fn plane_normal(f: &CompressedFace<Surface>) -> Option<Vector3> {
    match &f.surface {
        Surface::Plane(p) => Some(if f.orientation {
            p.normal()
        } else {
            -p.normal()
        }),
        _ => None,
    }
}

/// Remove pairs of planar faces bounded by the same edges with opposite
/// normals (the shared wall of two glued solids).
fn remove_glued_faces(sh: &mut CShell, stats: &mut MergeStats) {
    let sig = |f: &CompressedFace<Surface>| {
        let mut v: Vec<usize> = f.boundaries.iter().flatten().map(|e| e.index).collect();
        v.sort_unstable();
        v
    };
    let mut by_sig: BTreeMap<Vec<usize>, Vec<usize>> = BTreeMap::new();
    for (i, f) in sh.faces.iter().enumerate() {
        if plane_normal(f).is_some() {
            by_sig.entry(sig(f)).or_default().push(i);
        }
    }
    let mut remove = vec![false; sh.faces.len()];
    for list in by_sig.values() {
        for (a, &i) in list.iter().enumerate() {
            for &j in &list[a + 1..] {
                if remove[i] || remove[j] {
                    continue;
                }
                let (ni, nj) = (plane_normal(&sh.faces[i]), plane_normal(&sh.faces[j]));
                if let (Some(ni), Some(nj)) = (ni, nj)
                    && (ni + nj).magnitude() < 1e-9
                {
                    remove[i] = true;
                    remove[j] = true;
                    stats.faces_glued += 1;
                }
            }
        }
    }
    let mut k = 0;
    sh.faces.retain(|_| {
        k += 1;
        !remove[k - 1]
    });
}

/// Edge → (face, uses forward) incidences.
fn incidences(sh: &CShell) -> Vec<Vec<(usize, bool)>> {
    let mut inc = vec![Vec::new(); sh.edges.len()];
    for (fi, f) in sh.faces.iter().enumerate() {
        for ei in f.boundaries.iter().flatten() {
            inc[ei.index].push((fi, uses_forward(f, ei)));
        }
    }
    inc
}

/// Make the face orientations consistent across shared edges (breadth-first
/// from face 0 of each connected component). Fails for non-orientable sets.
fn orient_consistently(sh: &mut CShell, stats: &mut MergeStats) -> Result<()> {
    let inc = incidences(sh);
    let nf = sh.faces.len();
    let mut adj: Vec<Vec<(usize, usize)>> = vec![Vec::new(); nf];
    for (e, list) in inc.iter().enumerate() {
        if list.len() == 2 {
            adj[list[0].0].push((list[1].0, e));
            adj[list[1].0].push((list[0].0, e));
        }
    }
    let mut flip: Vec<Option<bool>> = vec![None; nf];
    for s in 0..nf {
        if flip[s].is_some() {
            continue;
        }
        flip[s] = Some(false);
        let mut queue = std::collections::VecDeque::from([s]);
        while let Some(f) = queue.pop_front() {
            let ff = flip[f].unwrap_or(false);
            for &(g, e) in &adj[f] {
                let dir = |face: usize| {
                    inc[e]
                        .iter()
                        .find(|(x, _)| *x == face)
                        .map(|(_, d)| *d)
                        .unwrap_or(true)
                };
                // Effective directions after flips must differ.
                let df = dir(f) ^ ff;
                let want_g = !df;
                let gf = dir(g) ^ want_g; // flip needed for g
                match flip[g] {
                    None => {
                        flip[g] = Some(gf);
                        queue.push_back(g);
                    }
                    Some(x) if x != gf => {
                        return Err(KernelError::InvalidInput(format!(
                            "the faces cannot be oriented consistently (face {g} conflicts around edge {e}): a non-orientable set"
                        )));
                    }
                    _ => {}
                }
            }
        }
    }
    for (i, f) in flip.iter().enumerate() {
        if *f == Some(true) {
            sh.faces[i].orientation = !sh.faces[i].orientation;
            stats.faces_flipped += 1;
        }
    }
    Ok(())
}

fn shells_of(body: &Body) -> Vec<CShell> {
    body.solid.compress().boundaries
}

/// Signed enclosed volume sign check: flip a closed solid that is inside out.
fn orient_outward(mut body: Body) -> Result<Body> {
    let m = crate::measure(&body)?;
    match m.volume {
        crate::VolumeResult::Value {
            volume,
            error_estimate,
            ..
        } => {
            if volume.abs() <= error_estimate {
                return Err(KernelError::InvalidResult(format!(
                    "closed shell encloses no volume ({volume:.3e} mm³)"
                )));
            }
            if volume < 0.0 {
                body.solid.not();
            }
            Ok(body)
        }
        crate::VolumeResult::Unavailable { reason } => Err(KernelError::InvalidResult(format!(
            "cannot determine the orientation of the closed shell ({reason})"
        ))),
    }
}

/// Glue closed solids that touch along coincident planar faces (same edges
/// after merging within `tol`) into one closed solid: the shared faces are
/// removed. Exact: no geometry changes beyond `tol`-merged vertices.
pub fn glue(parts: &[Body], tol: f64) -> Result<(Body, MergeStats)> {
    if parts.is_empty() {
        return Err(KernelError::InvalidInput("nothing to glue".into()));
    }
    let mut stats = MergeStats::default();
    let mut shells = Vec::new();
    for p in parts {
        shells.extend(guarded("compress", || shells_of(p))?);
    }
    let mut sh = merge(shells, tol, &mut stats)?;
    remove_glued_faces(&mut sh, &mut stats);
    compact(&mut sh);
    let solid = guarded("glue extract", || {
        Solid::extract(CompressedSolid {
            boundaries: vec![sh],
        })
    })?
    .map_err(|e| {
        KernelError::InvalidResult(format!("glued parts are not one closed solid: {e}"))
    })?;
    let body = Body { solid };
    crate::build::check_single_closed(&body, "glue")?;
    Ok((orient_outward(body)?, stats))
}

/// One free (boundary) edge of an open shell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FreeEdge {
    pub from: [f64; 3],
    pub to: [f64; 3],
    /// Chord length (exact for lines).
    pub length: f64,
    /// Face (index in the body's face order) that owns it.
    pub face: usize,
}

/// Two free edges that run alongside each other: a gap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gap {
    /// Indices into [`ShellAnalysis::free_edges`].
    pub edges: [usize; 2],
    /// Largest of the end-point and midpoint distances (mm).
    pub size: f64,
}

/// A closed loop of free edges (a hole in the surface set).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundaryLoop {
    /// Indices into [`ShellAnalysis::free_edges`], in order.
    pub edges: Vec<usize>,
    /// Largest distance of the loop (vertices and edge midpoints) from its
    /// best plane (mm); planar loops can be filled.
    pub planarity: f64,
    /// Unit normal of the loop's plane (Newell).
    pub normal: [f64; 3],
}

/// Analysis of a body's shells before stitching or filling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShellAnalysis {
    pub shells: usize,
    pub faces: usize,
    /// All shells closed: a solid.
    pub closed: bool,
    pub free_edges: Vec<FreeEdge>,
    /// Free-edge pairs closer than the search distance, smallest first.
    pub gaps: Vec<Gap>,
    /// Closed loops of free edges that are not part of any gap.
    pub boundary_loops: Vec<BoundaryLoop>,
    /// Face pairs using a shared edge in the same direction (orientation
    /// conflict), face indices in the body's order.
    pub orientation_conflicts: Vec<[usize; 2]>,
    /// Edges used by more than two faces.
    pub non_manifold_edges: usize,
    /// Search distance used for gaps (mm).
    pub search: f64,
}

/// Free edges, gaps (free edges within `search` mm of each other), boundary
/// loops and orientation conflicts of `body`, without changing it.
pub fn analyze(body: &Body, search: f64) -> Result<ShellAnalysis> {
    if !(search.is_finite() && search >= 0.0) {
        return Err(KernelError::InvalidInput(format!(
            "gap search distance must be finite and >= 0, got {search}"
        )));
    }
    let shells = guarded("compress", || shells_of(body))?;
    let nshells = shells.len();
    let mut stats = MergeStats::default();
    // Number faces globally without merging anything.
    let sh = merge(shells, -1.0, &mut stats)?;
    let inc = incidences(&sh);
    let mut free = Vec::new();
    let mut free_ids = Vec::new();
    let mut conflicts = Vec::new();
    let mut non_manifold = 0;
    for (e, list) in inc.iter().enumerate() {
        match list.len() {
            1 => {
                let ce = &sh.edges[e];
                let (a, b) = (sh.vertices[ce.vertices.0], sh.vertices[ce.vertices.1]);
                // Report in the direction the face uses it.
                let (a, b) = if list[0].1 { (a, b) } else { (b, a) };
                free.push(FreeEdge {
                    from: a.into(),
                    to: b.into(),
                    length: dist(a, b),
                    face: list[0].0,
                });
                free_ids.push(e);
            }
            2 => {
                if list[0].1 == list[1].1 {
                    conflicts.push([list[0].0.min(list[1].0), list[0].0.max(list[1].0)]);
                }
            }
            _ => non_manifold += 1,
        }
    }
    conflicts.sort_unstable();
    conflicts.dedup();
    // Gaps: pairs of free edges whose ends and midpoints are within `search`.
    let mut gaps = Vec::new();
    let mids: Vec<Point3> = free_ids
        .iter()
        .map(|&e| curve_mid(&sh.edges[e].curve).unwrap_or(Point3::origin()))
        .collect();
    let p = |a: [f64; 3]| Point3::new(a[0], a[1], a[2]);
    for i in 0..free.len() {
        for j in i + 1..free.len() {
            let (a0, a1) = (p(free[i].from), p(free[i].to));
            let (b0, b1) = (p(free[j].from), p(free[j].to));
            let ends = (dist(a0, b0).max(dist(a1, b1))).min(dist(a0, b1).max(dist(a1, b0)));
            let size = ends.max(dist(mids[i], mids[j]));
            if size <= search {
                gaps.push(Gap {
                    edges: [i, j],
                    size,
                });
            }
        }
    }
    gaps.sort_by(|a, b| a.size.total_cmp(&b.size));
    let in_gap: std::collections::HashSet<usize> = gaps.iter().flat_map(|g| g.edges).collect();
    // Boundary loops among the remaining free edges, chained by shared vertices.
    let mut loops = Vec::new();
    let rest: Vec<usize> = (0..free.len()).filter(|i| !in_gap.contains(i)).collect();
    let start_of = |i: usize| {
        let ce = &sh.edges[free_ids[i]];
        let fwd = inc[free_ids[i]][0].1;
        if fwd {
            ce.vertices
        } else {
            (ce.vertices.1, ce.vertices.0)
        }
    };
    let mut used = vec![false; free.len()];
    for &s in &rest {
        if used[s] {
            continue;
        }
        let mut chain = vec![s];
        used[s] = true;
        let first = start_of(s).0;
        let mut cur = start_of(s).1;
        let mut closed = false;
        for _ in 0..rest.len() {
            if cur == first {
                closed = true;
                break;
            }
            let Some(&nx) = rest.iter().find(|&&k| !used[k] && start_of(k).0 == cur) else {
                break;
            };
            used[nx] = true;
            chain.push(nx);
            cur = start_of(nx).1;
        }
        if closed || cur == first {
            let pts: Vec<Point3> = chain
                .iter()
                .flat_map(|&k| [p(free[k].from), mids[k]])
                .collect();
            let (normal, planarity) = best_plane(&pts);
            loops.push(BoundaryLoop {
                edges: chain,
                planarity,
                normal: normal.into(),
            });
        }
    }
    Ok(ShellAnalysis {
        shells: nshells,
        faces: sh.faces.len(),
        closed: !body.is_open_shell(),
        free_edges: free,
        gaps,
        boundary_loops: loops,
        orientation_conflicts: conflicts,
        non_manifold_edges: non_manifold,
        search,
    })
}

/// Newell normal of a closed point loop and the largest distance of the
/// points from the plane through their centroid.
fn best_plane(pts: &[Point3]) -> (Vector3, f64) {
    let n = pts.len();
    let mut nv = Vector3::new(0.0, 0.0, 0.0);
    let mut c = Vector3::new(0.0, 0.0, 0.0);
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        nv.x += (a.y - b.y) * (a.z + b.z);
        nv.y += (a.z - b.z) * (a.x + b.x);
        nv.z += (a.x - b.x) * (a.y + b.y);
        c += a.to_vec();
    }
    let len = nv.magnitude();
    if len.is_nan() || len <= 0.0 || n == 0 {
        return (Vector3::new(0.0, 0.0, 1.0), f64::INFINITY);
    }
    let nv = nv / len;
    let c = c / n as f64;
    let dev = pts
        .iter()
        .map(|q| (q.to_vec() - c).dot(nv).abs())
        .fold(0.0, f64::max);
    (nv, dev)
}

/// Result of [`stitch`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StitchReport {
    /// Healing tolerance used (mm).
    pub tolerance: f64,
    pub stats: MergeStats,
    /// The result is one closed solid.
    pub closed: bool,
    /// Free edges left.
    pub free_edges: usize,
    /// Connected pieces (shells) of the result.
    pub pieces: usize,
}

/// Stitch the faces of `body` (open shells, possibly several) by merging
/// vertices and edges within the healing tolerance `tol` and making the face
/// orientations consistent. A closed result becomes a solid (outward), else
/// the result stays an open shell. The input is not modified.
pub fn stitch(body: &Body, tol: f64) -> Result<(Body, StitchReport)> {
    if !(tol.is_finite() && tol >= 0.0) {
        return Err(KernelError::InvalidInput(format!(
            "healing tolerance must be finite and >= 0, got {tol}"
        )));
    }
    let shells = guarded("compress", || shells_of(body))?;
    let mut stats = MergeStats::default();
    let mut sh = merge(shells, tol, &mut stats)?;
    compact(&mut sh);
    let inc = incidences(&sh);
    if inc.iter().any(|l| l.len() > 2) {
        return Err(KernelError::InvalidInput(format!(
            "stitching at {tol} mm makes an edge shared by more than two faces (non-manifold); use a smaller tolerance"
        )));
    }
    orient_consistently(&mut sh, &mut stats)?;
    let free_edges = incidences(&sh).iter().filter(|l| l.len() == 1).count();
    finish(sh, tol, stats, free_edges)
}

fn finish(
    sh: CShell,
    tol: f64,
    stats: MergeStats,
    free_edges: usize,
) -> Result<(Body, StitchReport)> {
    let shell = guarded("shell extract", || Shell::extract(sh))?
        .map_err(|e| KernelError::InvalidResult(format!("stitched faces: {e}")))?;
    if !shell.is_connected() {
        // Still several pieces at this tolerance: an open multi-shell body.
        let parts = shell.connected_components();
        let pieces = parts.len();
        return Ok((
            Body {
                solid: Solid::new_unchecked(parts),
            },
            StitchReport {
                tolerance: tol,
                stats,
                closed: false,
                free_edges,
                pieces,
            },
        ));
    }
    if free_edges == 0 {
        let solid = Solid::try_new(vec![shell])
            .map_err(|e| KernelError::InvalidResult(format!("stitched shell: {e}")))?;
        let body = orient_outward(Body { solid })?;
        Ok((
            body,
            StitchReport {
                tolerance: tol,
                stats,
                closed: true,
                free_edges: 0,
                pieces: 1,
            },
        ))
    } else {
        Ok((
            Body::from_shell_unchecked(shell),
            StitchReport {
                tolerance: tol,
                stats,
                closed: false,
                free_edges,
                pieces: 1,
            },
        ))
    }
}

/// Result of [`fill_planar`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FillReport {
    /// Boundary loops filled with a planar face.
    pub filled: usize,
    /// Largest planarity deviation of a filled loop (mm).
    pub max_planarity: f64,
    pub closed: bool,
    pub free_edges: usize,
}

/// Fill every closed boundary loop of the open shell `body` that is planar
/// within `tol` with an exact planar face bounded by the loop's own edges.
/// Non-planar loops are refused (by name) rather than skipped.
pub fn fill_planar(body: &Body, tol: f64) -> Result<(Body, FillReport)> {
    if !body.is_open_shell() {
        return Err(KernelError::InvalidInput(
            "the body is already a closed solid: nothing to fill".into(),
        ));
    }
    let a = analyze(body, 0.0)?;
    if !a.orientation_conflicts.is_empty() {
        return Err(KernelError::InvalidInput(format!(
            "{} face pair(s) have inconsistent orientation; stitch first",
            a.orientation_conflicts.len()
        )));
    }
    if a.boundary_loops.is_empty() {
        return Err(KernelError::InvalidInput(
            "the open shell has no closed boundary loop to fill".into(),
        ));
    }
    for (i, l) in a.boundary_loops.iter().enumerate() {
        if l.planarity > tol {
            return Err(KernelError::InvalidInput(format!(
                "boundary loop {i} is not planar ({:.6} mm from its plane, tolerance {tol} mm): only planar patches are supported",
                l.planarity
            )));
        }
    }
    let shells = body.solid.boundaries();
    if shells.len() != 1 {
        return Err(KernelError::InvalidInput(format!(
            "the body has {} separate shells; stitch them first",
            shells.len()
        )));
    }
    let mut shell = shells[0].clone();
    let wires = shell.extract_boundaries();
    let mut filled = 0;
    let mut max_planarity: f64 = 0.0;
    for w in wires {
        // The fill face runs the loop the other way round.
        let inv = w.inverse();
        let pts: Vec<Point3> = inv
            .edge_iter()
            .flat_map(|e| {
                let c = e.oriented_curve();
                let (t0, t1) = c.range_tuple();
                [
                    e.front().point(),
                    eval(&c, 0.5 * (t0 + t1)).unwrap_or(e.front().point()),
                ]
            })
            .collect();
        let (n, dev) = best_plane(&pts);
        if dev > tol {
            return Err(KernelError::InvalidInput(format!(
                "a boundary loop is not planar ({dev:.6} mm, tolerance {tol} mm)"
            )));
        }
        max_planarity = max_planarity.max(dev);
        let o = pts[0];
        let x = {
            let d = pts
                .iter()
                .map(|q| *q - o)
                .max_by(|a, b| a.magnitude().total_cmp(&b.magnitude()))
                .unwrap_or(Vector3::unit_x());
            let d = d - n * d.dot(n);
            d.normalize()
        };
        let y = n.cross(x);
        let plane = Plane::new(o, o + x, o + y);
        let face = Face::try_new(vec![inv], Surface::Plane(plane))
            .map_err(|e| KernelError::InvalidResult(format!("fill face: {e}")))?;
        shell.push(face);
        filled += 1;
    }
    let cond = shell.shell_condition();
    if cond == ShellCondition::Closed {
        let solid = Solid::try_new(vec![shell])
            .map_err(|e| KernelError::InvalidResult(format!("filled shell: {e}")))?;
        let body = orient_outward(Body { solid })?;
        Ok((
            body,
            FillReport {
                filled,
                max_planarity,
                closed: true,
                free_edges: 0,
            },
        ))
    } else {
        let free = shell.extract_boundaries().iter().map(|w| w.len()).sum();
        Ok((
            Body::from_shell_unchecked(shell),
            FillReport {
                filled,
                max_planarity,
                closed: false,
                free_edges: free,
            },
        ))
    }
}

/// Thicken a surface body made of one planar face by `distance` along its
/// normal (`reverse`: against it), giving the exact prism. Faces of other
/// types, several faces, or a closed body are refused with the reason.
pub fn thicken(body: &Body, distance: f64, reverse: bool) -> Result<Body> {
    if !(distance.is_finite() && distance >= crate::tol::MIN_LENGTH) {
        return Err(KernelError::InvalidInput(format!(
            "thickness must be >= {} mm, got {distance}",
            crate::tol::MIN_LENGTH
        )));
    }
    if !body.is_open_shell() {
        return Err(KernelError::InvalidInput(
            "thicken needs a surface (open shell) body, not a solid".into(),
        ));
    }
    let faces: Vec<&Face> = body.solid.face_iter().collect();
    let [face] = faces.as_slice() else {
        return Err(KernelError::InvalidInput(format!(
            "thicken supports one face; the surface body has {} (supported subset: a single planar or cylindrical face)",
            faces.len()
        )));
    };
    match face.surface() {
        Surface::Plane(pl) => {
            let mut n = pl.normal();
            if !face.orientation() {
                n = -n;
            }
            if reverse {
                n = -n;
            }
            let solid = guarded("thicken", || builder::tsweep(*face, n * distance))?;
            let body = Body { solid };
            crate::build::check_single_closed(&body, "thicken")?;
            orient_outward(body)
        }
        _ => crate::thicken::thicken_curved(body, distance, reverse),
    }
}
