//! B-rep of the slab construction's result from exact line/arc slab regions
//! (ADR 0006, 0007): planar caps, planar walls (one face per wall plane and
//! connected region) and cylindrical walls (exact rational patches, one per
//! quarter-turn-or-less span of the circle's global partition and contiguous
//! run of slabs).
use super::curves::{ELoop, Kind, P2, Partition, Rings};
use super::{Region, SAME, internal, normalize};
use crate::{Frame, KernelError, Result};
use std::collections::HashMap;
use truck_modeling::*;

/// Edge of a face loop from its vertex to the next: a line or an atomic arc
/// (one partition span) of circle `k`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum EK {
    Line,
    Arc(usize),
}

enum Surf {
    /// Plane through `origin` spanned by `ex`, `ey` (outward normal ex × ey).
    Plane {
        origin: [f64; 3],
        ex: [f64; 3],
        ey: [f64; 3],
    },
    /// Cylinder patch of circle `k` over partition span `j`; `hole` when the
    /// material is outside the circle (outward normal towards the axis).
    Cyl { hole: bool },
}

struct FaceDesc {
    surf: Surf,
    /// Loops (outer first) counter-clockwise about the outward normal:
    /// (vertex, edge to the next vertex).
    loops: Vec<Vec<(usize, EK)>>,
}

/// Vertex table in tool-frame coordinates (merged within `SAME`).
#[derive(Default)]
struct Verts {
    pts: Vec<[f64; 3]>,
    cells: HashMap<[i64; 3], Vec<usize>>,
    /// Circle of a vertex placed on one (for arc edge geometry).
    circle: Vec<Option<usize>>,
}

impl Verts {
    fn id(&mut self, p: [f64; 3], circle: Option<usize>) -> usize {
        let cell = |p: [f64; 3]| p.map(|c| (c / (4.0 * SAME)).floor() as i64);
        let c = cell(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(v) = self.cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) {
                        for &i in v {
                            let q = self.pts[i];
                            if (0..3).all(|j| (p[j] - q[j]).abs() <= SAME) {
                                if self.circle[i].is_none() {
                                    self.circle[i] = circle;
                                }
                                return i;
                            }
                        }
                    }
                }
            }
        }
        self.pts.push(p);
        self.circle.push(circle);
        self.cells.entry(c).or_default().push(self.pts.len() - 1);
        self.pts.len() - 1
    }
}

/// Face loop of an exact loop at level `w` (arcs expanded into partition
/// spans), in the loop's own direction.
fn loop_at(l: &ELoop, w: f64, part: &Partition, verts: &mut Verts) -> Result<Vec<(usize, EK)>> {
    let n = l.pts.len();
    let mut out = Vec::new();
    for i in 0..n {
        let (a, b) = (l.pts[i], l.pts[(i + 1) % n]);
        match l.kind[i] {
            Kind::Line => out.push((verts.id([a[0], a[1], w], None), EK::Line)),
            Kind::Arc { circle, ccw } => {
                let pk = &part.pts[circle];
                let m = pk.len();
                let (Some(ia), Some(ib)) = (part.index(circle, a), part.index(circle, b)) else {
                    return Err(internal("an arc end is not on its circle's partition"));
                };
                let mut j = ia;
                loop {
                    let p = pk[j].1;
                    out.push((verts.id([p[0], p[1], w], Some(circle)), EK::Arc(circle)));
                    j = if ccw { (j + 1) % m } else { (j + m - 1) % m };
                    if j == ib {
                        break;
                    }
                }
            }
        }
    }
    Ok(out)
}

/// The loop traversed backwards (edge kinds move with their edges).
fn reversed(l: &[(usize, EK)]) -> Vec<(usize, EK)> {
    let n = l.len();
    (0..n)
        .map(|i| {
            let v = l[(n - i) % n].0;
            let k = l[(2 * n - i - 1) % n].1;
            (v, k)
        })
        .collect()
}

/// Exact loops of every shape of a polygon region.
fn exact_region(rings: &Rings, r: &Region) -> Vec<Vec<ELoop>> {
    r.iter()
        .map(|s| s.iter().map(|c| rings.exact(c)).collect())
        .collect()
}

/// Build the result's faces from the levels and slab regions.
fn faces(
    levels: &[f64],
    slabs: &[Region],
    cap_regions: &[(f64, bool, Region)],
    rings: &Rings,
    verts: &mut Verts,
) -> Result<(Vec<FaceDesc>, Partition)> {
    // Caps and slab regions as exact loops.
    let caps: Vec<(f64, bool, Vec<Vec<ELoop>>)> = cap_regions
        .iter()
        .map(|(w, up, r)| (*w, *up, exact_region(rings, r)))
        .collect();
    let slab_loops: Vec<Vec<ELoop>> = slabs
        .iter()
        .map(|r| exact_region(rings, r).into_iter().flatten().collect())
        .collect();
    // Global partition of every circle: all exact vertices on it.
    let mut breaks: Vec<Vec<P2>> = vec![Vec::new(); rings.circles.len()];
    let all_loops = caps
        .iter()
        .flat_map(|c| c.2.iter().flatten())
        .chain(slab_loops.iter().flatten());
    for l in all_loops {
        for &p in &l.pts {
            for (k, b) in breaks.iter_mut().enumerate() {
                if rings.ring_angle(k, p).is_some() {
                    b.push(p);
                }
            }
        }
    }
    let part = Partition::new(rings, &breaks);
    let mut out = Vec::new();
    for (w, up, shapes) in &caps {
        for shape in shapes {
            let mut loops = Vec::new();
            for l in shape {
                let fl = loop_at(l, *w, &part, verts)?;
                loops.push(if *up { fl } else { reversed(&fl) });
            }
            out.push(FaceDesc {
                surf: Surf::Plane {
                    origin: [0.0, 0.0, *w],
                    ex: if *up {
                        [1.0, 0.0, 0.0]
                    } else {
                        [0.0, 1.0, 0.0]
                    },
                    ey: if *up {
                        [0.0, 1.0, 0.0]
                    } else {
                        [1.0, 0.0, 0.0]
                    },
                },
                loops,
            });
        }
    }
    // Planar walls: group line edges by plane (outward normal n, offset c =
    // n·p); each sweeps [s0, s1] x [w_k, w_k+1] in the plane's (t, w)
    // coordinates, t = n rotated +90° (the edge direction: material on its left).
    struct Wall {
        n: [f64; 2],
        c: f64,
        rects: Region,
    }
    let mut walls: Vec<Wall> = Vec::new();
    // Cylinder walls: (circle, hole side) -> span j -> slabs.
    let mut cyl: HashMap<(usize, bool), HashMap<usize, Vec<usize>>> = HashMap::new();
    for (s, loops) in slab_loops.iter().enumerate() {
        let (w0, w1) = (levels[s], levels[s + 1]);
        for l in loops {
            let n = l.pts.len();
            for i in 0..n {
                let (p, q) = (l.pts[i], l.pts[(i + 1) % n]);
                match l.kind[i] {
                    Kind::Arc { circle, ccw } => {
                        let pk = &part.pts[circle];
                        let m = pk.len();
                        let (Some(ia), Some(ib)) = (part.index(circle, p), part.index(circle, q))
                        else {
                            return Err(internal("an arc end is not on its circle's partition"));
                        };
                        let spans = cyl.entry((circle, !ccw)).or_default();
                        let mut j = ia;
                        loop {
                            let nxt = if ccw { (j + 1) % m } else { (j + m - 1) % m };
                            // Span index: the ccw-start of the span.
                            let span = if ccw { j } else { nxt };
                            spans.entry(span).or_default().push(s);
                            j = nxt;
                            if j == ib {
                                break;
                            }
                        }
                    }
                    Kind::Line => {
                        let d = [q[0] - p[0], q[1] - p[1]];
                        let len = d[0].hypot(d[1]);
                        if len == 0.0 {
                            continue;
                        }
                        let nrm = [d[1] / len, -d[0] / len];
                        let off = nrm[0] * p[0] + nrm[1] * p[1];
                        let on = |wall: &Wall, x: [f64; 2]| {
                            (wall.n[0] * x[0] + wall.n[1] * x[1] - wall.c).abs() <= SAME
                        };
                        let idx = walls.iter().position(|w| {
                            nrm[0] * w.n[0] + nrm[1] * w.n[1] > 0.5 && on(w, p) && on(w, q)
                        });
                        let idx = match idx {
                            Some(i) => i,
                            None => {
                                walls.push(Wall {
                                    n: nrm,
                                    c: off,
                                    rects: Vec::new(),
                                });
                                walls.len() - 1
                            }
                        };
                        let wall = &mut walls[idx];
                        let t = [-wall.n[1], wall.n[0]];
                        let (s0, s1) = (t[0] * p[0] + t[1] * p[1], t[0] * q[0] + t[1] * q[1]);
                        wall.rects
                            .push(vec![vec![[s0, w0], [s1, w0], [s1, w1], [s0, w1]]]);
                    }
                }
            }
        }
    }
    for wall in walls {
        let t = [-wall.n[1], wall.n[0]];
        let foot = [wall.n[0] * wall.c, wall.n[1] * wall.c];
        for shape in normalize(&wall.rects)? {
            let loops = shape
                .iter()
                .map(|c| {
                    c.iter()
                        .map(|&[s, w]| {
                            (
                                verts.id([foot[0] + s * t[0], foot[1] + s * t[1], w], None),
                                EK::Line,
                            )
                        })
                        .collect()
                })
                .collect();
            out.push(FaceDesc {
                surf: Surf::Plane {
                    origin: [foot[0], foot[1], 0.0],
                    ex: [t[0], t[1], 0.0],
                    ey: [0.0, 0.0, 1.0],
                },
                loops,
            });
        }
    }
    // Cylinder patches: per span, contiguous runs of slabs (deterministic order).
    let mut groups: Vec<_> = cyl.into_iter().collect();
    groups.sort_by_key(|g| g.0);
    for ((k, hole), spans) in groups {
        let pk = &part.pts[k];
        let m = pk.len();
        let mut spans: Vec<_> = spans.into_iter().collect();
        spans.sort_by_key(|s| s.0);
        for (j, mut slabs_j) in spans {
            slabs_j.sort_unstable();
            slabs_j.dedup();
            let mut runs: Vec<(usize, usize)> = Vec::new();
            for s in slabs_j {
                match runs.last_mut() {
                    Some(r) if r.1 + 1 == s => r.1 = s,
                    _ => runs.push((s, s)),
                }
            }
            let (a, b) = (pk[j].1, pk[(j + 1) % m].1);
            for (s0, s1) in runs {
                let (wa, wb) = (levels[s0], levels[s1 + 1]);
                let mut v = |p: P2, w: f64| verts.id([p[0], p[1], w], Some(k));
                // Counter-clockwise about the radial-out normal (u = ccw angle,
                // v = up): a@wa -> b@wa -> b@wb -> a@wb.
                let boss = vec![
                    (v(a, wa), EK::Arc(k)),
                    (v(b, wa), EK::Line),
                    (v(b, wb), EK::Arc(k)),
                    (v(a, wb), EK::Line),
                ];
                out.push(FaceDesc {
                    surf: Surf::Cyl { hole },
                    loops: vec![if hole { reversed(&boss) } else { boss }],
                });
            }
        }
    }
    Ok((out, part))
}

/// Exact quarter-turn-or-less arc from `a` to `b` (counter-clockwise about
/// the frame normal) about `c`, as a rational quadratic Bezier with weights
/// (1, cos(phi/2), 1); end control points are exactly the vertices.
fn arc_curve(a: Point3, b: Point3, c: Point3) -> Result<NurbsCurve<Vector4>> {
    let (da, db) = (a - c, b - c);
    let (ra, rb) = (da.magnitude(), db.magnitude());
    let r = 0.5 * (ra + rb);
    let cos = (da.dot(db) / (ra * rb)).clamp(-1.0, 1.0);
    let phi = cos.acos();
    let w = (phi / 2.0).cos();
    if w.is_nan() || w <= 0.5 {
        return Err(internal("arc span of half a turn or more"));
    }
    let bis = (da + db).normalize();
    let m = c + bis * (r / w);
    let ctrl = vec![
        a.to_homogeneous(),
        Vector4::new(m.x * w, m.y * w, m.z * w, w),
        b.to_homogeneous(),
    ];
    let bs = BSplineCurve::try_new(KnotVec::bezier_knot(2), ctrl)
        .map_err(|e| KernelError::Internal(format!("arc curve construction failed: {e:?}")))?;
    Ok(NurbsCurve::new(bs))
}

/// Truck solid of the result (`levels`/`slabs` as in the slab construction,
/// in `frame` coordinates): shared vertices and edges, T-junctions of
/// straight edges split, every edge used exactly twice in opposite
/// directions, one connected shell.
pub(super) fn build(
    levels: &[f64],
    slabs: &[Region],
    caps: &[(f64, bool, Region)],
    rings: &Rings,
    frame: &Frame,
) -> Result<Solid> {
    let mut verts = Verts::default();
    let (mut faces, _part) = faces(levels, slabs, caps, rings, &mut verts)?;
    let pts = verts.pts.clone();
    // Dedup and split straight edges at vertices lying on them.
    for f in &mut faces {
        for l in f.loops.iter_mut() {
            l.dedup_by(|b, a| a.0 == b.0);
            while l.len() > 1 && l.first().map(|x| x.0) == l.last().map(|x| x.0) {
                l.pop();
            }
            let mut out = Vec::with_capacity(l.len());
            for i in 0..l.len() {
                let (a, kind) = l[i];
                let b = l[(i + 1) % l.len()].0;
                out.push((a, kind));
                if kind != EK::Line {
                    continue;
                }
                let (pa, pb) = (pts[a], pts[b]);
                let d = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
                let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                let mut on: Vec<(f64, usize)> = pts
                    .iter()
                    .enumerate()
                    .filter(|&(j, _)| j != a && j != b)
                    .filter_map(|(j, p)| {
                        let v = [p[0] - pa[0], p[1] - pa[1], p[2] - pa[2]];
                        let s = (v[0] * d[0] + v[1] * d[1] + v[2] * d[2]) / dd;
                        if s <= 0.0 || s >= 1.0 {
                            return None;
                        }
                        let off = [v[0] - s * d[0], v[1] - s * d[1], v[2] - s * d[2]];
                        let dist2 = off[0] * off[0] + off[1] * off[1] + off[2] * off[2];
                        (dist2 <= SAME * SAME).then_some((s, j))
                    })
                    .collect();
                on.sort_by(|x, y| x.0.total_cmp(&y.0));
                out.extend(on.into_iter().map(|(_, j)| (j, EK::Line)));
            }
            *l = out;
        }
    }
    let world = |p: [f64; 3]| {
        Point3::new(
            frame.origin[0] + p[0] * frame.x[0] + p[1] * frame.y[0] + p[2] * frame.z[0],
            frame.origin[1] + p[0] * frame.x[1] + p[1] * frame.y[1] + p[2] * frame.z[1],
            frame.origin[2] + p[0] * frame.x[2] + p[1] * frame.y[2] + p[2] * frame.z[2],
        )
    };
    let wvec = |v: [f64; 3]| {
        let o = world([0.0; 3]);
        world(v) - o
    };
    let vertices: Vec<Vertex> = pts.iter().map(|&p| builder::vertex(world(p))).collect();
    // Edges: (lo, hi, circle) -> (edge, front vertex, signed use count).
    type Key = (usize, usize, Option<usize>);
    let mut edges: HashMap<Key, (Edge, usize, i32)> = HashMap::new();
    let mut uses: HashMap<Key, usize> = HashMap::new();
    // `sign`: +1 when the face's final orientation traverses a -> b, -1 when
    // the wire is built reversed (inverted face).
    let mut get_edge = |a: usize, b: usize, kind: EK, sign: i32| -> Result<(Key, Edge)> {
        let circle = match kind {
            EK::Line => None,
            EK::Arc(k) => Some(k),
        };
        let key = (a.min(b), a.max(b), circle);
        if let std::collections::hash_map::Entry::Vacant(slot) = edges.entry(key) {
            let (edge, front) = match circle {
                None => (builder::line(&vertices[key.0], &vertices[key.1]), key.0),
                Some(k) => {
                    let c = rings.circles[k].c;
                    let (p, q) = (pts[key.0], pts[key.1]);
                    // Counter-clockwise start: (p - c) x (q - c) > 0.
                    let cr = (p[0] - c[0]) * (q[1] - c[1]) - (p[1] - c[1]) * (q[0] - c[0]);
                    let (s, e) = if cr > 0.0 {
                        (key.0, key.1)
                    } else {
                        (key.1, key.0)
                    };
                    let cw = world([c[0], c[1], pts[s][2]]);
                    let curve = arc_curve(world(pts[s]), world(pts[e]), cw)?;
                    (
                        Edge::new(&vertices[s], &vertices[e], Curve::NurbsCurve(curve)),
                        s,
                    )
                }
            };
            slot.insert((edge, front, 0));
        }
        let entry = edges.get_mut(&key).ok_or_else(|| internal("edge lost"))?;
        entry.2 += sign * if a == entry.1 { 1 } else { -1 };
        *uses.entry(key).or_default() += 1;
        let e = if a == entry.1 {
            entry.0.clone()
        } else {
            entry.0.inverse()
        };
        Ok((key, e))
    };
    let mut tfaces = Vec::with_capacity(faces.len());
    let mut face_keys: Vec<Vec<Key>> = Vec::with_capacity(faces.len());
    for f in &faces {
        let mut wires = Vec::with_capacity(f.loops.len());
        let mut keys = Vec::new();
        let hole = matches!(f.surf, Surf::Cyl { hole: true, .. });
        for l in &f.loops {
            if l.len() < 2 || (l.len() < 3 && l.iter().all(|x| x.1 == EK::Line)) {
                return Err(internal("degenerate face loop"));
            }
            // Truck wires follow the surface's own orientation; a hole-side
            // cylinder face is built on the radial-out surface and inverted.
            let order = if hole { reversed(l) } else { l.clone() };
            let mut wire = Wire::new();
            for i in 0..order.len() {
                let (a, kind) = order[i];
                let b = order[(i + 1) % order.len()].0;
                let (key, e) = get_edge(a, b, kind, if hole { -1 } else { 1 })?;
                keys.push(key);
                wire.push_back(e);
            }
            wires.push(wire);
        }
        let face = match &f.surf {
            Surf::Plane { origin, ex, ey } => {
                let o = world(*origin);
                let plane = Plane::new(o, o + wvec(*ex), o + wvec(*ey));
                Face::try_new(wires, Surface::Plane(plane))
                    .map_err(|e| internal(&format!("face: {e:?}")))?
            }
            Surf::Cyl { .. } => {
                // The bottom and top arcs (counter-clockwise curves) of the
                // patch: the wire's first and third edges in surface order.
                let w0 = &wires[0];
                let es: Vec<Edge> = w0.edge_iter().cloned().collect();
                let arcs: Vec<NurbsCurve<Vector4>> = es
                    .iter()
                    .filter_map(|e| match e.curve() {
                        Curve::NurbsCurve(c) => Some(c),
                        _ => None,
                    })
                    .collect();
                if arcs.len() != 2 {
                    return Err(internal("cylinder patch without two arcs"));
                }
                // Bottom arc first (lower level along the frame normal).
                let lvl = |c: &NurbsCurve<Vector4>| {
                    let p = c.front();
                    wvec([0.0, 0.0, 1.0]).dot(p - world([0.0; 3]))
                };
                let (bot, top) = if lvl(&arcs[0]) <= lvl(&arcs[1]) {
                    (&arcs[0], &arcs[1])
                } else {
                    (&arcs[1], &arcs[0])
                };
                let surf = NurbsSurface::new(BSplineSurface::homotopy(
                    bot.non_rationalized().clone(),
                    top.non_rationalized().clone(),
                ));
                let mut face = Face::try_new(wires, Surface::NurbsSurface(surf))
                    .map_err(|e| internal(&format!("cylinder face: {e:?}")))?;
                if hole {
                    face.invert();
                }
                face
            }
        };
        tfaces.push(face);
        face_keys.push(keys);
    }
    if edges.values().any(|(_, _, s)| *s != 0) || uses.values().any(|&u| u != 2) {
        return Err(KernelError::InvalidResult(
            "the result would not be a manifold solid (bodies meeting along an edge or \
             at a point)"
                .into(),
        ));
    }
    // Connected?
    let mut parent: Vec<usize> = (0..face_keys.len()).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        p[i] = r;
        r
    }
    let mut owner: HashMap<Key, usize> = HashMap::new();
    for (fi, keys) in face_keys.iter().enumerate() {
        for k in keys {
            if let Some(&g) = owner.get(k) {
                let (x, y) = (find(&mut parent, fi), find(&mut parent, g));
                parent[x] = y;
            } else {
                owner.insert(*k, fi);
            }
        }
    }
    // Bounding box per connected component of faces.
    let mut boxes: HashMap<usize, ([f64; 3], [f64; 3])> = HashMap::new();
    for (fi, f) in faces.iter().enumerate() {
        let root = find(&mut parent, fi);
        let b = boxes
            .entry(root)
            .or_insert(([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]));
        for &(v, _) in f.loops.iter().flatten() {
            for (i, &x) in pts[v].iter().enumerate() {
                b.0[i] = b.0[i].min(x);
                b.1[i] = b.1[i].max(x);
            }
        }
    }
    let roots = boxes.len();
    if roots != 1 {
        // A component strictly inside another's box: a closed cavity (e.g. a
        // pocket covered by a joined plate) rather than separate bodies.
        let inside = |a: &([f64; 3], [f64; 3]), b: &([f64; 3], [f64; 3])| {
            (0..3).all(|i| a.0[i] > b.0[i] + SAME && a.1[i] < b.1[i] - SAME)
        };
        let all: Vec<_> = boxes.values().collect();
        let void = all
            .iter()
            .any(|a| all.iter().any(|b| !std::ptr::eq(*a, *b) && inside(a, b)));
        return Err(KernelError::InvalidResult(if void {
            "the result would enclose an internal void (a cavity closed on all sides); \
             enclosed internal voids are not supported (leave the cavity open, e.g. with a \
             vent hole)"
                .into()
        } else {
            format!(
                "the result would be {roots} disconnected solids; exactly one connected solid \
                 is required"
            )
        }));
    }
    let shell: Shell = tfaces.into();
    Solid::try_new(vec![shell]).map_err(|e| internal(&format!("solid: {e:?}")))
}
