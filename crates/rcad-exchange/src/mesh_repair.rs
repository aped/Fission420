//! Bounded mesh repair (run 7, Run-8 pull-forward ME01; docs/MESH.md).
//!
//! Repair never edits its input: [`repair`] returns a new indexed mesh and a
//! [`RepairReport`] with the diagnosis before and after ([`crate::mesh::diagnose`])
//! and a count for every change. Each step is optional and defined:
//!
//! 1. **Weld** (always, exact): vertices with bit-identical coordinates share
//!    one index (STL stores every triangle's corners separately). With
//!    `weld_tolerance > 0`, vertices closer than the tolerance (Euclidean, mm)
//!    are merged too: clusters are built by union-find over pairs within the
//!    tolerance, so a chain of close vertices merges transitively (stated); the
//!    representative is the lowest original index (deterministic).
//! 2. **Degenerate triangles** (`remove_degenerate`): repeated indices after
//!    welding, or zero area by [`crate::mesh::DEGENERATE_REL_AREA`].
//! 3. **Duplicate triangles** (`remove_duplicates`): same three indices in any
//!    order and orientation; the first one (file order) is kept.
//! 4. **Orientation** (`orient`): within each component connected through
//!    two-triangle (manifold) edges, triangles are flipped to agree with the
//!    component's first triangle (breadth-first, deterministic); a closed
//!    component with negative signed volume is then flipped entirely so normals
//!    point outward. Non-manifold edges do not propagate orientation.
//! 5. **Hole filling** (`fill_holes_max_edges > 0`): every closed loop of boundary
//!    edges with at most that many edges, whose vertices lie within
//!    `max(weld_tolerance, 1e-3 × loop size)` of their best-fit plane (Newell
//!    normal) and whose projection is a simple polygon, is triangulated by ear
//!    clipping, oriented against its neighbours. Other loops are left open and
//!    counted (`unfilled_holes`), with the reason in `notes`.
//!
//! What it does not do: remove self-intersections, fix non-manifold edges, or
//! make a mesh printable by itself; a closed result is not a proof of a valid
//! solid. Limits: at most [`MAX_REPAIR_TRIANGLES`] triangles.
use crate::mesh::{DEGENERATE_REL_AREA, MeshDiagnostics, diagnose};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};

/// Resource bound for one repair (triangles in the input).
pub const MAX_REPAIR_TRIANGLES: usize = 5_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RepairOptions {
    /// Merge vertices closer than this (mm); 0 = exact duplicates only.
    #[serde(default)]
    pub weld_tolerance: f64,
    #[serde(default = "yes")]
    pub remove_degenerate: bool,
    #[serde(default = "yes")]
    pub remove_duplicates: bool,
    #[serde(default = "yes")]
    pub orient: bool,
    /// Fill boundary loops with at most this many edges (0 = do not fill).
    #[serde(default)]
    pub fill_holes_max_edges: u32,
}

fn yes() -> bool {
    true
}

impl Default for RepairOptions {
    fn default() -> Self {
        RepairOptions {
            weld_tolerance: 0.0,
            remove_degenerate: true,
            remove_duplicates: true,
            orient: true,
            fill_holes_max_edges: 0,
        }
    }
}

impl RepairOptions {
    pub fn validate(&self) -> Result<(), String> {
        if !(self.weld_tolerance.is_finite() && self.weld_tolerance >= 0.0) {
            return Err(format!(
                "weld tolerance must be finite and >= 0, got {}",
                self.weld_tolerance
            ));
        }
        if self.weld_tolerance > 1.0 {
            return Err(format!(
                "weld tolerance {} mm is larger than this repair supports (1 mm); \
                 merging at that size changes the shape",
                self.weld_tolerance
            ));
        }
        if self.fill_holes_max_edges > 10_000 {
            return Err("hole filling is limited to loops of at most 10000 edges".into());
        }
        Ok(())
    }
}

/// What a repair changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepairReport {
    pub options: RepairOptions,
    pub before: MeshDiagnostics,
    pub after: MeshDiagnostics,
    /// Vertices merged into another (exact duplicates + within tolerance).
    pub welded_vertices: usize,
    /// Of those, merged only because of the tolerance (not bit-identical).
    pub welded_by_tolerance: usize,
    pub removed_degenerate: usize,
    pub removed_duplicates: usize,
    pub flipped_triangles: usize,
    pub filled_holes: usize,
    pub fill_triangles: usize,
    pub unfilled_holes: usize,
    pub notes: Vec<String>,
}

impl RepairReport {
    /// Whether anything changed beyond the exact weld (which only rebuilds
    /// shared indices).
    pub fn changed(&self) -> bool {
        self.welded_by_tolerance
            + self.removed_degenerate
            + self.removed_duplicates
            + self.flipped_triangles
            + self.fill_triangles
            > 0
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn len(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

fn find(p: &mut [u32], mut i: u32) -> u32 {
    while p[i as usize] != i {
        let q = p[i as usize];
        p[i as usize] = p[q as usize];
        i = q;
    }
    i
}

/// Weld: returns (new positions, map old index -> new index, merged count,
/// merged by tolerance).
type Welded = (Vec<[f64; 3]>, Vec<u32>, usize, usize);

fn weld(positions: &[[f64; 3]], tol: f64) -> Result<Welded, String> {
    let n = positions.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    // Exact duplicates.
    let mut exact: HashMap<[u64; 3], u32> = HashMap::new();
    for (i, p) in positions.iter().enumerate() {
        let k = p.map(|x| (x + 0.0).to_bits());
        let r = *exact.entry(k).or_insert(i as u32);
        if r != i as u32 {
            parent[i] = r;
        }
    }
    let exact_merged = parent
        .iter()
        .enumerate()
        .filter(|(i, p)| **p != *i as u32)
        .count();
    if tol > 0.0 {
        // Review run 7 #11: the grid index must not saturate (tolerance tiny
        // relative to the coordinates) and cells must stay small.
        let big = positions
            .iter()
            .flatten()
            .fold(0.0f64, |m, x| m.max(x.abs()));
        if big / tol > 1e12 {
            return Err(format!(
                "weld tolerance {tol} mm is too small for coordinates up to {big} mm (below \
                 floating-point resolution); use 0 for exact welding"
            ));
        }
        // Grid of cell size `tol`: pairs within `tol` are in the same or
        // neighbouring cells. Only exact-weld representatives take part.
        let cell = |p: [f64; 3]| p.map(|x| (x / tol).floor() as i64);
        let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
        for i in 0..n {
            if parent[i] == i as u32 {
                grid.entry(cell(positions[i])).or_default().push(i as u32);
            }
        }
        if let Some(c) = grid.values().map(Vec::len).max()
            && c > MAX_CELL
        {
            return Err(format!(
                "weld tolerance {tol} mm is too coarse for this mesh ({c} vertices within one \
                 tolerance cell); use a smaller tolerance"
            ));
        }
        let keys: Vec<[i64; 3]> = {
            let mut k: Vec<_> = grid.keys().copied().collect();
            k.sort();
            k
        };
        for c in keys {
            let here = grid[&c].clone();
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let Some(other) = grid.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) else {
                            continue;
                        };
                        for &a in &here {
                            for &b in other {
                                if a < b
                                    && len(sub(positions[a as usize], positions[b as usize])) < tol
                                {
                                    let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                                    if ra != rb {
                                        // Lowest index represents the cluster.
                                        let (lo, hi) = (ra.min(rb), ra.max(rb));
                                        parent[hi as usize] = lo;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut map = vec![0u32; n];
    let mut out = Vec::new();
    let mut new_of_root: HashMap<u32, u32> = HashMap::new();
    for (i, slot) in map.iter_mut().enumerate() {
        let r = find(&mut parent, i as u32);
        *slot = *new_of_root.entry(r).or_insert_with(|| {
            out.push(positions[r as usize]);
            (out.len() - 1) as u32
        });
    }
    // Review run 7 #27: union-find merges transitively; refuse when a chain
    // moved a vertex farther than the tolerance from where it was.
    if tol > 0.0 {
        let far = (0..n)
            .map(|i| len(sub(positions[i], out[map[i] as usize])))
            .fold(0.0f64, f64::max);
        if far > tol {
            return Err(format!(
                "weld tolerance {tol} mm chains vertices together: one would move {far:.4} mm \
                 (more than the tolerance); use a smaller tolerance"
            ));
        }
    }
    let merged = n - out.len();
    Ok((out, map, merged, merged - exact_merged))
}

/// Largest number of vertices in one weld cell.
const MAX_CELL: usize = 4096;

fn signed_volume(pos: &[[f64; 3]], tris: &[[u32; 3]]) -> f64 {
    tris.iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| pos[i as usize]);
            dot(a, cross(b, c)) / 6.0
        })
        .sum()
}

/// A repaired mesh: positions, triangles and the report.
pub type Repaired = (Vec<[f64; 3]>, Vec<[u32; 3]>, RepairReport);

/// Repair `triangles` over `positions` (see the module docs).
pub fn repair(
    positions: &[[f64; 3]],
    triangles: &[[u32; 3]],
    opts: &RepairOptions,
) -> Result<Repaired, String> {
    opts.validate()?;
    if triangles.len() > MAX_REPAIR_TRIANGLES {
        return Err(format!(
            "mesh repair is limited to {MAX_REPAIR_TRIANGLES} triangles, got {}",
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
    let before = diagnose(positions, triangles);
    let mut notes = Vec::new();
    let (pos, map, welded, welded_tol) = weld(positions, opts.weld_tolerance)?;
    let mut tris: Vec<[u32; 3]> = triangles
        .iter()
        .map(|t| t.map(|i| map[i as usize]))
        .collect();
    // Degenerate.
    let mut removed_degenerate = 0;
    if opts.remove_degenerate {
        tris.retain(|t| {
            let [a, b, c] = t.map(|i| pos[i as usize]);
            let longest = len(sub(b, a)).max(len(sub(c, b))).max(len(sub(a, c)));
            let bad = t[0] == t[1]
                || t[1] == t[2]
                || t[0] == t[2]
                || len(cross(sub(b, a), sub(c, a))) <= DEGENERATE_REL_AREA * longest * longest;
            if bad {
                removed_degenerate += 1;
            }
            !bad
        });
    }
    // Duplicates.
    let mut removed_duplicates = 0;
    if opts.remove_duplicates {
        let mut seen: HashMap<[u32; 3], ()> = HashMap::new();
        tris.retain(|t| {
            let mut k = *t;
            k.sort_unstable();
            let dup = seen.insert(k, ()).is_some();
            if dup {
                removed_duplicates += 1;
            }
            !dup
        });
    }
    // Orientation (consistency only; the outward pass runs after filling).
    let original = tris.clone();
    if opts.orient && !tris.is_empty() {
        let mut edge_tris: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (ti, t) in tris.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                edge_tris.entry((a.min(b), a.max(b))).or_default().push(ti);
            }
        }
        let has_dir =
            |t: &[u32; 3], a: u32, b: u32| (0..3).any(|k| t[k] == a && t[(k + 1) % 3] == b);
        let mut flip = vec![false; tris.len()];
        let mut seen = vec![false; tris.len()];
        for start in 0..tris.len() {
            if seen[start] {
                continue;
            }
            seen[start] = true;
            let mut q = VecDeque::from([start]);
            while let Some(ti) = q.pop_front() {
                let t = tris[ti];
                for k in 0..3 {
                    let (mut a, mut b) = (t[k], t[(k + 1) % 3]);
                    if flip[ti] {
                        std::mem::swap(&mut a, &mut b);
                    }
                    let list = &edge_tris[&(a.min(b), a.max(b))];
                    if list.len() != 2 {
                        continue;
                    }
                    let other = if list[0] == ti { list[1] } else { list[0] };
                    if seen[other] {
                        continue;
                    }
                    seen[other] = true;
                    // Consistent: the neighbour traverses a→b as b→a.
                    flip[other] = has_dir(&tris[other], a, b);
                    q.push_back(other);
                }
            }
        }
        for (ti, f) in flip.iter().enumerate() {
            if *f {
                tris[ti].swap(1, 2);
            }
        }
    }
    // Hole filling.
    let (mut filled_holes, mut fill_triangles, mut unfilled_holes) = (0, 0, 0);
    if opts.fill_holes_max_edges > 0 {
        // Directed boundary edges: an edge used once, in its triangle's direction.
        let mut count: HashMap<(u32, u32), usize> = HashMap::new();
        for t in &tris {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                *count.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        // next[b] = a for a boundary edge a→b: the fill traverses it as b→a.
        let mut next: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for t in &tris {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if count[&(a.min(b), a.max(b))] == 1 {
                    next.entry(b).or_default().push(a);
                }
            }
        }
        let mut new_tris = Vec::new();
        let mut used: HashMap<(u32, u32), bool> = HashMap::new();
        // Review run 7 #10: computed once (was per walk: quadratic).
        let cap = count.values().filter(|c| **c == 1).count() + 1;
        // Review run 7 #9: the outline of a planar open sheet is not a hole
        // (filling it makes a zero-volume "closed" pillow).
        let planar_sheet = planar_components(&pos, &tris, opts.weld_tolerance);
        let starts: Vec<u32> = next.keys().copied().collect();
        for s in starts {
            let outs = next[&s].clone();
            for first in outs {
                if used.contains_key(&(s, first)) {
                    continue;
                }
                // Walk s → first → ... back to s (reverse of the boundary).
                let mut lp = vec![s];
                let mut cur = first;
                let mut prev = s;
                used.insert((prev, cur), true);
                let mut ok = true;
                // Walk the whole loop (marking its edges) before deciding;
                // bounded by the number of boundary edges.
                while cur != s {
                    lp.push(cur);
                    if lp.len() > cap {
                        ok = false;
                        break;
                    }
                    let cands = next.get(&cur).cloned().unwrap_or_default();
                    let Some(&n) = cands.iter().find(|&&n| !used.contains_key(&(cur, n))) else {
                        ok = false;
                        break;
                    };
                    if cands.len() > 1 {
                        notes.push(format!(
                            "boundary vertex {cur} has several open edges (pinched loop)"
                        ));
                    }
                    used.insert((cur, n), true);
                    prev = cur;
                    cur = n;
                }
                let _ = prev;
                // `lp` holds the loop's vertices = its edge count.
                if ok && lp.len() > opts.fill_holes_max_edges as usize {
                    unfilled_holes += 1;
                    notes.push(format!(
                        "a {}-edge hole was left open (limit {} edges)",
                        lp.len(),
                        opts.fill_holes_max_edges
                    ));
                    continue;
                }
                if !ok {
                    unfilled_holes += 1;
                    notes.push(
                        "a boundary chain that does not close into a loop was left open"
                            .to_string(),
                    );
                    continue;
                }
                if planar_sheet.contains(&lp[0]) {
                    unfilled_holes += 1;
                    notes.push(format!(
                        "a {}-edge loop is the outline of a flat open sheet, not a hole; left open",
                        lp.len()
                    ));
                    continue;
                }
                match fill_loop(&pos, &lp, opts.weld_tolerance) {
                    Ok(t) => {
                        filled_holes += 1;
                        fill_triangles += t.len();
                        new_tris.extend(t);
                    }
                    Err(why) => {
                        unfilled_holes += 1;
                        notes.push(format!("a {}-edge hole was left open: {why}", lp.len()));
                    }
                }
            }
        }
        tris.extend(new_tris);
    }
    // Outward (review run 7 #4, #8): after filling; each closed component is
    // oriented by its nesting depth among the closed components (even: outward,
    // odd: a void, inward).
    if opts.orient {
        orient_outward(&pos, &mut tris);
    }
    let flipped_triangles = original
        .iter()
        .zip(&tris)
        .filter(|(a, b)| !same_cycle(a, b))
        .count();
    let after = diagnose(&pos, &tris);
    if after.closed && !before.closed {
        notes
            .push("closed after repair (topology only; self-intersections are not checked)".into());
    }
    Ok((
        pos,
        tris,
        RepairReport {
            options: *opts,
            before,
            after,
            welded_vertices: welded,
            welded_by_tolerance: welded_tol,
            removed_degenerate,
            removed_duplicates,
            flipped_triangles,
            filled_holes,
            fill_triangles,
            unfilled_holes,
            notes,
        },
    ))
}

/// Whether `b` has the same cyclic vertex order as `a`.
fn same_cycle(a: &[u32; 3], b: &[u32; 3]) -> bool {
    (0..3).any(|r| (0..3).all(|k| a[k] == b[(k + r) % 3]))
}

/// Vertices of connected components (through shared vertices) whose vertices
/// all lie within `max(tol, 1e-9 × size)` of one plane: flat open sheets.
fn planar_components(
    pos: &[[f64; 3]],
    tris: &[[u32; 3]],
    tol: f64,
) -> std::collections::HashSet<u32> {
    let mut parent: Vec<u32> = (0..pos.len() as u32).collect();
    for t in tris {
        let a = find(&mut parent, t[0]);
        for &v in &t[1..] {
            let b = find(&mut parent, v);
            if a != b {
                parent[b as usize] = a;
            }
        }
    }
    let mut comps: HashMap<u32, Vec<usize>> = HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        comps.entry(find(&mut parent, t[0])).or_default().push(ti);
    }
    let mut out = std::collections::HashSet::new();
    for ts in comps.values() {
        let mut nrm = [0.0; 3];
        let mut c = [0.0; 3];
        let mut nv = 0.0;
        for &ti in ts {
            let [a, b, cc] = tris[ti].map(|i| pos[i as usize]);
            let n = cross(sub(b, a), sub(cc, a));
            for k in 0..3 {
                nrm[k] += n[k];
                c[k] += a[k];
            }
            nv += 1.0;
        }
        let l = len(nrm);
        if l <= 0.0 {
            continue;
        }
        let nrm = nrm.map(|x| x / l);
        let c = c.map(|x| x / nv);
        let verts = ts.iter().flat_map(|&ti| tris[ti]);
        let size = verts
            .clone()
            .map(|v| len(sub(pos[v as usize], c)))
            .fold(0.0, f64::max);
        let t = tol.max(1e-9 * size);
        if verts
            .clone()
            .all(|v| dot(sub(pos[v as usize], c), nrm).abs() <= t)
        {
            out.extend(verts);
        }
    }
    out
}

/// Generalized winding number of `p` with respect to triangles `ts`.
fn winding(pos: &[[f64; 3]], ts: &[[u32; 3]], p: [f64; 3]) -> f64 {
    let mut sum = 0.0;
    for t in ts {
        let [a, b, c] = t.map(|i| sub(pos[i as usize], p));
        let (la, lb, lc) = (len(a), len(b), len(c));
        let num = dot(a, cross(b, c));
        let den = la * lb * lc + dot(a, b) * lc + dot(b, c) * la + dot(c, a) * lb;
        sum += 2.0 * num.atan2(den);
    }
    sum / (4.0 * std::f64::consts::PI)
}

/// Orient each closed component (connected through two-triangle edges) by its
/// nesting depth: a component inside an odd number of others is a void and
/// faces inward, otherwise outward.
fn orient_outward(pos: &[[f64; 3]], tris: &mut [[u32; 3]]) {
    let mut edge_tris: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            edge_tris.entry((a.min(b), a.max(b))).or_default().push(ti);
        }
    }
    let mut seen = vec![false; tris.len()];
    let mut comps: Vec<Vec<usize>> = Vec::new();
    for start in 0..tris.len() {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        let (mut comp, mut q, mut closed) = (vec![start], VecDeque::from([start]), true);
        while let Some(ti) = q.pop_front() {
            for k in 0..3 {
                let (a, b) = (tris[ti][k], tris[ti][(k + 1) % 3]);
                let list = &edge_tris[&(a.min(b), a.max(b))];
                if list.len() != 2 {
                    closed = false;
                    continue;
                }
                let other = if list[0] == ti { list[1] } else { list[0] };
                if !seen[other] {
                    seen[other] = true;
                    comp.push(other);
                    q.push_back(other);
                }
            }
        }
        if closed {
            comps.push(comp);
        }
    }
    let sets: Vec<Vec<[u32; 3]>> = comps
        .iter()
        .map(|c| c.iter().map(|&ti| tris[ti]).collect())
        .collect();
    let boxes: Vec<([f64; 3], [f64; 3])> = sets
        .iter()
        .map(|ts| {
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            for v in ts.iter().flatten() {
                for k in 0..3 {
                    lo[k] = lo[k].min(pos[*v as usize][k]);
                    hi[k] = hi[k].max(pos[*v as usize][k]);
                }
            }
            (lo, hi)
        })
        .collect();
    for (ci, comp) in comps.iter().enumerate() {
        let p = pos[sets[ci][0][0] as usize];
        let depth = (0..sets.len())
            .filter(|&o| {
                o != ci
                    && (0..3).all(|k| p[k] >= boxes[o].0[k] && p[k] <= boxes[o].1[k])
                    && winding(pos, &sets[o], p).abs() > 0.5
            })
            .count();
        let v = signed_volume(pos, &sets[ci]);
        let want_positive = depth % 2 == 0;
        if (v > 0.0) != want_positive {
            for &ti in comp {
                tris[ti].swap(1, 2);
            }
        }
    }
}

/// Ear-clipping triangulation of a nearly planar loop (vertex indices in
/// fill order: the triangles keep this orientation).
fn fill_loop(pos: &[[f64; 3]], lp: &[u32], tol: f64) -> Result<Vec<[u32; 3]>, String> {
    if lp.len() < 3 {
        return Err("fewer than three edges".into());
    }
    let pts: Vec<[f64; 3]> = lp.iter().map(|&i| pos[i as usize]).collect();
    // Newell normal and centroid.
    let mut nrm = [0.0; 3];
    let mut c = [0.0; 3];
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        nrm[0] += (a[1] - b[1]) * (a[2] + b[2]);
        nrm[1] += (a[2] - b[2]) * (a[0] + b[0]);
        nrm[2] += (a[0] - b[0]) * (a[1] + b[1]);
        for k in 0..3 {
            c[k] += a[k] / pts.len() as f64;
        }
    }
    let nl = len(nrm);
    if nl <= 0.0 {
        return Err("the loop has no area".into());
    }
    let nrm = nrm.map(|x| x / nl);
    let size = pts.iter().map(|p| len(sub(*p, c))).fold(0.0, f64::max);
    let flat_tol = tol.max(1e-3 * size);
    let dev = pts
        .iter()
        .map(|p| dot(sub(*p, c), nrm).abs())
        .fold(0.0, f64::max);
    if dev > flat_tol {
        return Err(format!(
            "not planar (deviation {dev:.4} mm > {flat_tol:.4} mm)"
        ));
    }
    // 2D frame.
    let ax = if nrm[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = {
        let v = cross(nrm, ax);
        let l = len(v);
        v.map(|x| x / l)
    };
    let v = cross(nrm, u);
    let p2: Vec<[f64; 2]> = pts
        .iter()
        .map(|p| {
            let d = sub(*p, c);
            [dot(d, u), dot(d, v)]
        })
        .collect();
    // Orientation of the 2D polygon in (u, v) = the loop's own order is
    // counter-clockwise about `nrm`.
    let area2 = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])
    };
    // Simple polygon check (no two non-adjacent edges cross).
    let n = p2.len();
    let seg_cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]| {
        let (d1, d2) = (area2(a, b, c), area2(a, b, d));
        let (d3, d4) = (area2(c, d, a), area2(c, d, b));
        d1 * d2 < 0.0 && d3 * d4 < 0.0
    };
    for i in 0..n {
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            if seg_cross(p2[i], p2[(i + 1) % n], p2[j], p2[(j + 1) % n]) {
                return Err("the hole's outline crosses itself".into());
            }
        }
    }
    let mut idx: Vec<usize> = (0..n).collect();
    let mut out = Vec::new();
    let mut guard = 0;
    // Review run 7 #28: resume after the last ear instead of restarting.
    let mut from = 0usize;
    while idx.len() > 3 {
        guard += 1;
        if guard > 4 * n * n {
            return Err("ear clipping did not converge".into());
        }
        let m = idx.len();
        let mut clipped = false;
        for step in 0..m {
            let k = (from + step) % m;
            let (ia, ib, ic) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            let (a, b, cc) = (p2[ia], p2[ib], p2[ic]);
            if area2(a, b, cc) <= 0.0 {
                continue; // reflex or flat
            }
            let inside = idx.iter().any(|&j| {
                j != ia
                    && j != ib
                    && j != ic
                    && area2(a, b, p2[j]) >= 0.0
                    && area2(b, cc, p2[j]) >= 0.0
                    && area2(cc, a, p2[j]) >= 0.0
            });
            if inside {
                continue;
            }
            out.push([lp[ia], lp[ib], lp[ic]]);
            idx.remove(k);
            from = k.saturating_sub(1);
            clipped = true;
            break;
        }
        if !clipped {
            return Err("no ear found (degenerate outline)".into());
        }
    }
    if area2(p2[idx[0]], p2[idx[1]], p2[idx[2]]) > 0.0 {
        out.push([lp[idx[0]], lp[idx[1]], lp[idx[2]]]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unit cube, 12 triangles, outward, indexed.
    fn cube() -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
        let p = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ];
        let t = vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        (p, t)
    }

    /// STL-style: every triangle with its own three vertices.
    fn unshare(p: &[[f64; 3]], t: &[[u32; 3]]) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
        let mut pos = Vec::new();
        let mut tris = Vec::new();
        for tr in t {
            let b = pos.len() as u32;
            pos.extend(tr.map(|i| p[i as usize]));
            tris.push([b, b + 1, b + 2]);
        }
        (pos, tris)
    }

    #[test]
    fn clean_cube_only_welds() {
        let (p, t) = cube();
        let (p, t) = unshare(&p, &t);
        let (pos, tris, r) = repair(&p, &t, &RepairOptions::default()).unwrap();
        assert_eq!(pos.len(), 8);
        assert_eq!(tris.len(), 12);
        assert_eq!(r.welded_vertices, 36 - 8);
        assert_eq!(r.welded_by_tolerance, 0);
        assert!(!r.changed());
        assert!(r.after.closed);
        assert!((r.after.signed_volume.unwrap() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn flipped_triangles_degenerates_and_duplicates() {
        let (p, mut t) = cube();
        // Flip two triangles, add a duplicate (reversed) and a degenerate one.
        t[3].swap(1, 2);
        t[8].swap(1, 2);
        t.push([4, 7, 6]);
        t.push([0, 0, 1]);
        let (p, t) = unshare(&p, &t);
        assert!(!diagnose(&p, &t).closed);
        let (_, tris, r) = repair(&p, &t, &RepairOptions::default()).unwrap();
        assert_eq!(r.removed_degenerate, 1);
        assert_eq!(r.removed_duplicates, 1);
        assert_eq!(r.flipped_triangles, 2);
        assert_eq!(tris.len(), 12);
        assert!(r.after.closed, "{:?}", r.after);
        assert!((r.after.signed_volume.unwrap() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn inside_out_cube_is_turned_outward() {
        let (p, mut t) = cube();
        for x in &mut t {
            x.swap(1, 2);
        }
        let (_, _, r) = repair(&p, &t, &RepairOptions::default()).unwrap();
        assert_eq!(r.flipped_triangles, 12);
        assert!((r.after.signed_volume.unwrap() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn tolerance_weld_closes_a_cracked_cube() {
        let (mut p, t) = cube();
        let (mut p2, t2) = unshare(&p, &t);
        // Nudge one copy of vertex 6 by 1e-4 mm: exact welding leaves a crack.
        p2[8][0] += 1e-4;
        let (_, _, r) = repair(&p2, &t2, &RepairOptions::default()).unwrap();
        assert!(!r.after.closed);
        let o = RepairOptions {
            weld_tolerance: 1e-3,
            ..Default::default()
        };
        let (pos, _, r) = repair(&p2, &t2, &o).unwrap();
        assert_eq!(r.welded_by_tolerance, 1);
        assert_eq!(pos.len(), 8);
        assert!(r.after.closed);
        p.clear();
    }

    #[test]
    fn planar_holes_are_filled_and_others_reported() {
        let (p, mut t) = cube();
        // Remove the top face (2 triangles): a 4-edge planar hole.
        t.retain(|x| !(x.contains(&4) && x.contains(&6) && (x.contains(&5) || x.contains(&7))));
        assert_eq!(t.len(), 10);
        let o = RepairOptions {
            fill_holes_max_edges: 8,
            ..Default::default()
        };
        let (_, tris, r) = repair(&p, &t, &o).unwrap();
        assert_eq!(
            (r.filled_holes, r.fill_triangles, r.unfilled_holes),
            (1, 2, 0)
        );
        assert_eq!(tris.len(), 12);
        assert!(r.after.closed, "{:?}", r.after);
        assert!((r.after.signed_volume.unwrap() - 1.0).abs() < 1e-12);
        // Limit 3 edges: left open and counted.
        let o = RepairOptions {
            fill_holes_max_edges: 3,
            ..Default::default()
        };
        let (_, _, r) = repair(&p, &t, &o).unwrap();
        assert_eq!((r.filled_holes, r.unfilled_holes), (0, 1));
        assert!(!r.after.closed);
    }

    #[test]
    fn non_planar_hole_is_left_open() {
        let (mut p, mut t) = cube();
        t.retain(|x| !(x.contains(&4) && x.contains(&6) && (x.contains(&5) || x.contains(&7))));
        // Lift corner 6 and bend the side triangles with it: the hole is a
        // skew quadrilateral (0.3 mm out of plane for a 1 mm hole).
        p[6][2] = 1.3;
        let o = RepairOptions {
            fill_holes_max_edges: 8,
            ..Default::default()
        };
        let (_, _, r) = repair(&p, &t, &o).unwrap();
        assert_eq!(r.unfilled_holes, 1);
        assert!(
            r.notes.iter().any(|n| n.contains("not planar")),
            "{:?}",
            r.notes
        );
    }

    /// Cube of edge `e` at `o`, outward.
    fn cube_at(o: [f64; 3], e: f64, base: u32) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
        let (p, t) = cube();
        (
            p.iter()
                .map(|q| [o[0] + q[0] * e, o[1] + q[1] * e, o[2] + q[2] * e])
                .collect(),
            t.iter().map(|x| x.map(|i| i + base)).collect(),
        )
    }

    /// Review run 7 #4: a hollow part (outer shell outward, inner void shell
    /// inward) is already correct: nothing flips, volume 8 − 1.
    #[test]
    fn void_shells_keep_their_inward_orientation() {
        let (mut p, mut t) = cube_at([0.0; 3], 2.0, 0);
        let (p2, mut t2) = cube_at([0.5; 3], 1.0, 8);
        for x in &mut t2 {
            x.swap(1, 2); // void: inward
        }
        p.extend(p2);
        t.extend(t2);
        let (_, _, r) = repair(&p, &t, &RepairOptions::default()).unwrap();
        assert_eq!(r.flipped_triangles, 0, "{r:?}");
        assert!((r.after.signed_volume.unwrap() - 7.0).abs() < 1e-12);
        // Both shells inside out: both flip, same result.
        let rev: Vec<[u32; 3]> = t.iter().map(|x| [x[0], x[2], x[1]]).collect();
        let (_, _, r) = repair(&p, &rev, &RepairOptions::default()).unwrap();
        assert_eq!(r.flipped_triangles, 24);
        assert!((r.after.signed_volume.unwrap() - 7.0).abs() < 1e-12);
    }

    /// Review run 7 #8: an inside-out open cube is closed by the fill and
    /// then turned outward (the outward pass runs after filling).
    #[test]
    fn inside_out_open_mesh_ends_outward_after_filling() {
        let (p, mut t) = cube();
        t.retain(|x| !(x.contains(&4) && x.contains(&6) && (x.contains(&5) || x.contains(&7))));
        for x in &mut t {
            x.swap(1, 2);
        }
        let o = RepairOptions {
            fill_holes_max_edges: 8,
            ..Default::default()
        };
        let (_, _, r) = repair(&p, &t, &o).unwrap();
        assert!(r.after.closed);
        assert!(
            (r.after.signed_volume.unwrap() - 1.0).abs() < 1e-12,
            "{r:?}"
        );
        assert_eq!(r.flipped_triangles, 10);
    }

    /// Review run 7 #9: a flat open sheet's outline is not filled.
    #[test]
    fn flat_sheet_outline_is_not_a_hole() {
        let p = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let t = vec![[0, 1, 2], [0, 2, 3]];
        let o = RepairOptions {
            fill_holes_max_edges: 8,
            ..Default::default()
        };
        let (_, tris, r) = repair(&p, &t, &o).unwrap();
        assert_eq!((tris.len(), r.filled_holes, r.unfilled_holes), (2, 0, 1));
        assert!(!r.after.closed);
    }

    /// Review run 7 #11, #27: tolerances below floating-point resolution and
    /// chains that move vertices farther than the tolerance are refused.
    #[test]
    fn weld_tolerance_guards() {
        let (p, t) = cube_at([1.0e6; 3], 1.0, 0);
        let o = RepairOptions {
            weld_tolerance: 1e-9,
            ..Default::default()
        };
        assert!(repair(&p, &t, &o).unwrap_err().contains("too small"));
        // A chain of vertices 0.09 apart with tolerance 0.1 would collapse
        // 1 mm of mesh: refused.
        let p: Vec<[f64; 3]> = (0..12).map(|i| [i as f64 * 0.09, 0.0, 0.0]).collect();
        let t: Vec<[u32; 3]> = (0..10).map(|i| [i, i + 1, i + 2]).collect();
        let o = RepairOptions {
            weld_tolerance: 0.1,
            ..Default::default()
        };
        assert!(repair(&p, &t, &o).unwrap_err().contains("chains"));
    }

    #[test]
    fn invalid_options_and_inputs_are_refused() {
        let (p, t) = cube();
        for o in [
            RepairOptions {
                weld_tolerance: -1.0,
                ..Default::default()
            },
            RepairOptions {
                weld_tolerance: f64::NAN,
                ..Default::default()
            },
            RepairOptions {
                weld_tolerance: 5.0,
                ..Default::default()
            },
        ] {
            assert!(repair(&p, &t, &o).is_err());
        }
        assert!(repair(&p, &[[0, 1, 99]], &RepairOptions::default()).is_err());
    }
}
