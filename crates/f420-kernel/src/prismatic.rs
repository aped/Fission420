//! Exact Cut/Join of a straight prism into a *prismatic* body without a 3-D
//! boolean (ADR 0006).
//!
//! Truck 0.6 booleans fail (or return non-closed shells) when tool faces
//! coincide with faces of the target: a pocket overlapping an earlier pocket of
//! the same depth, sharing a wall plane with it, or flush with an outer wall.
//! For the common case where every face of the target is planar and either
//! perpendicular to the tool axis (a *cap*) or parallel to it (a *wall*), the
//! body is exactly a stack of slabs: between two consecutive cap levels its
//! cross-section is a constant 2-D region. The boolean is then a 2-D region
//! boolean per slab, and the result is rebuilt from the slab regions:
//!
//! * slab regions: `S_k = (S_{k-1} \ Up_k) ∪ Down_k`, where `Up_k`/`Down_k`
//!   are the up-/down-facing cap faces at level `k` (material below / above);
//! * the tool's levels split slabs; slabs inside the tool span get
//!   `S_k \ P` (Cut) or `S_k ∪ P` (Join);
//! * result caps at level `k`: `S_{k-1} \ S_k` (up) and `S_k \ S_{k-1}` (down);
//! * result walls: each boundary edge of `S_k` sweeps a rectangle over the
//!   slab; rectangles in one wall plane are united into that plane's faces, so
//!   a wall is one face however many slabs it spans (face references resolve
//!   by plane and must stay unambiguous);
//! * vertices are shared, T-junctions split, every edge must bound exactly two
//!   faces in opposite directions, and the shell must be connected.
//!
//! The 2-D booleans run on an integer grid ([`GRID_STEPS_PER_MM`]) via
//! `i_overlay`, which makes coincident and collinear edges exact instead of a
//! numerical accident. Every coordinate of the result is on that grid in the
//! tool frame (1e-9 mm, far below [`tol::KERNEL_LINEAR`]). Profiles with arcs
//! and bodies with curved or oblique faces are not handled here (the caller
//! keeps Truck's error). Callers verify the result like any other boolean.
use crate::{Body, Frame, KernelError, LoopSegment, Profile, Result, tol};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::ShapeType;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::overlay::FloatOverlay;
use i_overlay::i_float::adapter::FloatPointAdapter;
use i_overlay::i_float::float::rect::FloatRect;
use std::collections::HashMap;
use truck_modeling::*;

/// Integer grid of the 2-D booleans: steps per mm (1e-9 mm resolution).
pub const GRID_STEPS_PER_MM: f64 = 1.0e9;

/// Distinct cap levels and wall planes closer than this are the same.
const SAME: f64 = tol::FACE_MATCH;

/// Join or Cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlabOp {
    Cut,
    Join,
}

type Contour = Vec<[f64; 2]>;
/// Outer contour (counter-clockwise) first, then holes (clockwise).
type Shape = Vec<Contour>;
type Region = Vec<Shape>;

/// Whether `body` is prismatic along `frame.z` (every face planar,
/// perpendicular or parallel to it) and `profile` has straight edges only: the
/// cases [`prism_boolean`] can build.
pub fn applicable(body: &Body, profile: &Profile, frame: &Frame) -> bool {
    not_applicable(body, profile, frame).is_none()
}

/// Why [`prism_boolean`] cannot build this case, or None when it can.
pub fn not_applicable(body: &Body, profile: &Profile, frame: &Frame) -> Option<String> {
    if profile_polygon(profile).is_none() {
        return Some("the profile has arcs or is a circle".into());
    }
    classify_faces(body, frame).err()
}

/// `target` cut by (or joined with) the prism of `profile` (in `start`'s 2-D
/// coordinates) swept from `start` by `length` along `start.z`, built exactly
/// from slab regions (see the module docs). `InvalidInput` when the case is not
/// prismatic; `InvalidResult` when the result is not one closed solid.
pub fn prism_boolean(
    target: &Body,
    profile: &Profile,
    start: &Frame,
    length: f64,
    op: SlabOp,
) -> Result<Body> {
    start.validate()?;
    if !length.is_finite() || length < tol::MIN_LENGTH {
        return Err(KernelError::InvalidInput(format!(
            "prism length must be >= {} mm, got {length}",
            tol::MIN_LENGTH
        )));
    }
    let tool = profile_polygon(profile).ok_or_else(|| {
        KernelError::InvalidInput("prismatic boolean: the profile has arcs or is a circle".into())
    })?;
    let caps = classify_faces(target, start)
        .map_err(|m| KernelError::InvalidInput(format!("prismatic boolean: {m}")))?;
    let tool_region = normalize(&[vec![tool]])?;
    if area(&tool_region) <= 0.0 {
        return Err(KernelError::InvalidInput(
            "prismatic boolean: empty profile".into(),
        ));
    }

    // Levels: every cap, plus the tool's two ends.
    let mut levels: Vec<f64> = caps.iter().map(|c| c.w).chain([0.0, length]).collect();
    levels.sort_by(f64::total_cmp);
    levels.dedup_by(|b, a| (*b - *a).abs() <= SAME);
    let level_of = |w: f64| levels.iter().position(|l| (l - w).abs() <= SAME);
    let n = levels.len();
    let (mut up, mut down): (Vec<Region>, Vec<Region>) = (vec![vec![]; n], vec![vec![]; n]);
    for c in &caps {
        let k = level_of(c.w).ok_or_else(|| internal("cap level lost"))?;
        if c.up {
            up[k].push(c.shape.clone());
        } else {
            down[k].push(c.shape.clone());
        }
    }
    // Slab k lies between levels k and k + 1.
    let mut slabs: Vec<Region> = Vec::with_capacity(n.saturating_sub(1));
    let mut prev: Region = Vec::new();
    for k in 0..n {
        let below_up = boolean(&prev, &normalize(&up[k])?, OverlayRule::Difference)?;
        let cur = boolean(&below_up, &normalize(&down[k])?, OverlayRule::Union)?;
        if k + 1 == n {
            if area(&cur) > 0.0 {
                return Err(internal(
                    "the body's cap faces do not close (not prismatic)",
                ));
            }
        } else {
            slabs.push(cur.clone());
        }
        prev = cur;
    }
    let (t0, t1) = (
        level_of(0.0).unwrap_or(0),
        level_of(length).unwrap_or(n - 1),
    );
    let mut changed = false;
    for (k, s) in slabs.iter_mut().enumerate() {
        if k >= t0 && k < t1 {
            let rule = match op {
                SlabOp::Cut => OverlayRule::Difference,
                SlabOp::Join => OverlayRule::Union,
            };
            let next = boolean(s, &tool_region, rule)?;
            // Any real change is far above one grid cell (1e-18 mm^2).
            changed |= (area(&next) - area(s)).abs() > 1.0e-12;
            *s = next;
        }
    }
    if !changed {
        return Err(KernelError::InvalidInput(match op {
            SlabOp::Cut => "cut: the extrusion removes no material from the target body".into(),
            SlabOp::Join => "join: the extrusion adds no material to the target body".into(),
        }));
    }
    let faces = rebuild_faces(&levels, &slabs)?;
    let solid = build_solid(&faces, start)?;
    Ok(Body { solid })
}

/// The straight-edged loop of `profile`, or None for arcs and circles.
fn profile_polygon(profile: &Profile) -> Option<Contour> {
    match profile {
        Profile::Polygon(v) if v.len() >= 3 => Some(v.clone()),
        Profile::Loop(segs) if segs.len() >= 3 => segs
            .iter()
            .map(|s| match s {
                LoopSegment::Line { start } => Some(*start),
                LoopSegment::Arc { .. } => None,
            })
            .collect(),
        _ => None,
    }
}

/// A cap face of the target in tool-frame coordinates: level `w` along the
/// axis, outward normal along (+) or against (-) it, and its region.
struct Cap {
    w: f64,
    up: bool,
    shape: Shape,
}

/// Caps of `body` in `frame` coordinates, or why the body is not prismatic
/// along `frame.z` (a curved face, or a face oblique to it).
fn classify_faces(body: &Body, frame: &Frame) -> std::result::Result<Vec<Cap>, String> {
    let z = Vector3::new(frame.z[0], frame.z[1], frame.z[2]);
    let mut caps = Vec::new();
    for f in body.solid.face_iter() {
        let Surface::Plane(pl) = f.surface() else {
            return Err("the body has a curved face".into());
        };
        let mut nrm = pl.normal();
        if !f.orientation() {
            nrm = -nrm;
        }
        let along = nrm.dot(z);
        if along.abs() <= SAME {
            continue; // wall
        }
        if nrm.cross(z).magnitude() > SAME {
            return Err(format!(
                "the body has a face oblique to the tool axis (normal {:.6?})",
                [nrm.x, nrm.y, nrm.z]
            ));
        }
        let mut loops: Vec<Contour> = Vec::new();
        let mut w = 0.0;
        let mut count = 0.0;
        for wire in f.absolute_boundaries() {
            let mut c = Vec::new();
            for e in wire.edge_iter() {
                if e.front() == e.back() {
                    return Err("a planar face has a closed (curved) edge".into());
                }
                let p = to_frame(frame, e.front().point());
                w += p[2];
                count += 1.0;
                c.push([p[0], p[1]]);
            }
            loops.push(c);
        }
        if count == 0.0 {
            return Err("a face has no boundary".into());
        }
        caps.push(Cap {
            w: w / count,
            up: along > 0.0,
            shape: loops,
        });
    }
    Ok(caps)
}

/// World point in `frame` coordinates (x, y in-plane, z along the normal).
fn to_frame(frame: &Frame, p: Point3) -> [f64; 3] {
    let d = [
        p.x - frame.origin[0],
        p.y - frame.origin[1],
        p.z - frame.origin[2],
    ];
    let dot = |a: [f64; 3]| d[0] * a[0] + d[1] * a[1] + d[2] * a[2];
    [dot(frame.x), dot(frame.y), dot(frame.z)]
}

fn internal(m: &str) -> KernelError {
    KernelError::InvalidResult(format!("prismatic boolean: {m}"))
}

fn signed_area(c: &[[f64; 2]]) -> f64 {
    let n = c.len();
    (0..n)
        .map(|i| {
            let (p, q) = (c[i], c[(i + 1) % n]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum::<f64>()
        / 2.0
}

fn area(r: &Region) -> f64 {
    r.iter().flatten().map(|c| signed_area(c)).sum()
}

/// Union of the shapes of `r` with consistent orientation (outer CCW, holes
/// CW, whatever the input's), on the grid.
fn normalize(r: &[Shape]) -> Result<Region> {
    let oriented: Region = r
        .iter()
        .map(|s| {
            let outer = s
                .iter()
                .enumerate()
                .max_by(|a, b| signed_area(a.1).abs().total_cmp(&signed_area(b.1).abs()))
                .map(|(i, _)| i)
                .unwrap_or(0);
            s.iter()
                .enumerate()
                .map(|(i, c)| {
                    let mut c = c.clone();
                    if (signed_area(&c) > 0.0) != (i == outer) {
                        c.reverse();
                    }
                    c
                })
                .collect()
        })
        .collect();
    boolean(&oriented, &Vec::new(), OverlayRule::Union)
}

/// `a op b` on the integer grid (non-zero fill), each output shape oriented
/// outer CCW / holes CW.
fn boolean(a: &Region, b: &Region, rule: OverlayRule) -> Result<Region> {
    if a.is_empty() && b.is_empty() {
        return Ok(Vec::new());
    }
    // Inputs and outputs live on the absolute grid k / GRID_STEPS_PER_MM. The
    // adapter's offset (the centre of its rectangle) is an integer millimetre
    // and its half-size a power of two, so float -> integer is exact for grid
    // values in every call: results of different calls (e.g. a slab region and
    // a raw cap face) can never be one grid step apart, which would leave
    // 1e-9 mm slivers.
    let snap_region = |r: &Region| -> Region {
        r.iter()
            .map(|s| {
                s.iter()
                    .map(|c| c.iter().map(|p| p.map(snap)).collect())
                    .collect()
            })
            .collect()
    };
    let (a, b) = (snap_region(a), snap_region(b));
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in a.iter().chain(&b).flatten().flatten() {
        for i in 0..2 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let c = [0, 1].map(|i| (0.5 * (lo[i] + hi[i])).round());
    let reach = (0..2)
        .map(|i| (hi[i] - c[i]).max(c[i] - lo[i]))
        .fold(0.0f64, f64::max);
    let half = (reach + 1.0).log2().ceil().exp2();
    if !half.is_finite() || half * GRID_STEPS_PER_MM > 1.0e15 {
        return Err(internal("2-D boolean: coordinates out of range"));
    }
    let rect = FloatRect::new(c[0] - half, c[0] + half, c[1] - half, c[1] + half);
    let adapter = FloatPointAdapter::<[f64; 2], i64>::with_scale(rect, GRID_STEPS_PER_MM);
    let cap = a.iter().chain(&b).flatten().map(|c| c.len()).sum();
    let out = FloatOverlay::<[f64; 2], i64>::with_adapter(adapter, cap)
        .unsafe_add_source(&a, ShapeType::Subject)
        .unsafe_add_source(&b, ShapeType::Clip)
        .overlay(rule, FillRule::NonZero);
    Ok(out
        .into_iter()
        .map(|shape| {
            shape
                .into_iter()
                .enumerate()
                .map(|(i, c)| {
                    let mut c: Contour = c.into_iter().map(|p| p.map(snap)).collect();
                    if (signed_area(&c) > 0.0) != (i == 0) {
                        c.reverse();
                    }
                    c
                })
                .filter(|c| c.len() >= 3)
                .collect::<Shape>()
        })
        .filter(|s: &Shape| !s.is_empty())
        .collect())
}

/// `x` on the absolute grid of the 2-D booleans.
fn snap(x: f64) -> f64 {
    (x * GRID_STEPS_PER_MM).round() / GRID_STEPS_PER_MM + 0.0
}

/// Area of the polygon `p` outside the union of the face regions `faces`
/// (each: outer loop and holes, any orientation), on the integer grid. Exact
/// for straight edges; used to confirm that a profile lies inside a face where
/// point sampling misses notches whose edges run along the profile's.
pub(crate) fn area_outside(p: &[[f64; 2]], faces: &[Vec<Vec<[f64; 2]>>]) -> Result<f64> {
    let mut region: Region = Vec::new();
    for f in faces {
        region = boolean(
            &region,
            &normalize(std::slice::from_ref(f))?,
            OverlayRule::Union,
        )?;
    }
    let profile = normalize(&[vec![p.to_vec()]])?;
    Ok(area(&boolean(&profile, &region, OverlayRule::Difference)?))
}

/// A planar face of the result in tool-frame coordinates: loops (outer first)
/// counter-clockwise about the outward normal, and the face's in-plane axes
/// (`ex × ey` = outward normal).
struct FaceLoops {
    loops: Vec<Vec<[f64; 3]>>,
    origin: [f64; 3],
    ex: [f64; 3],
    ey: [f64; 3],
}

fn rebuild_faces(levels: &[f64], slabs: &[Region]) -> Result<Vec<FaceLoops>> {
    let mut faces = Vec::new();
    let empty: Region = Vec::new();
    // Caps.
    for (k, &w) in levels.iter().enumerate() {
        let below = if k > 0 { &slabs[k - 1] } else { &empty };
        let above = slabs.get(k).unwrap_or(&empty);
        for (up, region) in [
            (true, boolean(below, above, OverlayRule::Difference)?),
            (false, boolean(above, below, OverlayRule::Difference)?),
        ] {
            for shape in region {
                let loops = shape
                    .iter()
                    .map(|c| {
                        let mut l: Vec<[f64; 3]> = c.iter().map(|p| [p[0], p[1], w]).collect();
                        if !up {
                            l.reverse(); // counter-clockwise seen from -z
                        }
                        l
                    })
                    .collect();
                faces.push(FaceLoops {
                    loops,
                    origin: [0.0, 0.0, w],
                    ex: if up { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] },
                    ey: if up { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] },
                });
            }
        }
    }
    // Walls: group the slab boundary edges by plane (outward normal n, offset
    // c = n·p); each edge sweeps [s0, s1] x [w_k, w_k+1] in the plane's (t, z)
    // coordinates, t = n rotated +90° (the edge direction: material on its left).
    struct Wall {
        n: [f64; 2],
        c: f64,
        rects: Region,
    }
    let mut walls: Vec<Wall> = Vec::new();
    for (k, region) in slabs.iter().enumerate() {
        let (w0, w1) = (levels[k], levels[k + 1]);
        for c in region.iter().flatten() {
            for i in 0..c.len() {
                let (p, q) = (c[i], c[(i + 1) % c.len()]);
                let d = [q[0] - p[0], q[1] - p[1]];
                let len = d[0].hypot(d[1]);
                if len == 0.0 {
                    continue;
                }
                let n = [d[1] / len, -d[0] / len];
                let off = n[0] * p[0] + n[1] * p[1];
                let on = |wall: &Wall, x: [f64; 2]| {
                    (wall.n[0] * x[0] + wall.n[1] * x[1] - wall.c).abs() <= SAME
                };
                let idx = walls
                    .iter()
                    .position(|w| n[0] * w.n[0] + n[1] * w.n[1] > 0.5 && on(w, p) && on(w, q));
                let idx = match idx {
                    Some(i) => i,
                    None => {
                        walls.push(Wall {
                            n,
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
    for wall in walls {
        let t = [-wall.n[1], wall.n[0]];
        let foot = [wall.n[0] * wall.c, wall.n[1] * wall.c];
        for shape in normalize(&wall.rects)? {
            let loops = shape
                .iter()
                .map(|c| {
                    c.iter()
                        .map(|&[s, w]| [foot[0] + s * t[0], foot[1] + s * t[1], w])
                        .collect()
                })
                .collect();
            faces.push(FaceLoops {
                loops,
                origin: [foot[0], foot[1], 0.0],
                ex: [t[0], t[1], 0.0],
                ey: [0.0, 0.0, 1.0],
            });
        }
    }
    Ok(faces)
}

/// Truck solid from the face loops (tool-frame coordinates mapped through
/// `frame`): shared vertices and edges, T-junctions split.
fn build_solid(faces: &[FaceLoops], frame: &Frame) -> Result<Solid> {
    // Global vertex table (merge within SAME).
    let mut pts: Vec<[f64; 3]> = Vec::new();
    let mut cells: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    let cell = |p: [f64; 3]| p.map(|c| (c / (4.0 * SAME)).floor() as i64);
    let mut index = |p: [f64; 3]| -> usize {
        let c = cell(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(v) = cells.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) {
                        for &i in v {
                            let q = pts[i];
                            if (0..3).all(|j| (p[j] - q[j]).abs() <= SAME) {
                                return i;
                            }
                        }
                    }
                }
            }
        }
        pts.push(p);
        cells.entry(c).or_default().push(pts.len() - 1);
        pts.len() - 1
    };
    let mut face_ids: Vec<Vec<Vec<usize>>> = faces
        .iter()
        .map(|f| {
            f.loops
                .iter()
                .map(|l| {
                    let mut ids: Vec<usize> = l.iter().map(|&p| index(p)).collect();
                    ids.dedup();
                    while ids.len() > 1 && ids.first() == ids.last() {
                        ids.pop();
                    }
                    ids
                })
                .collect()
        })
        .collect();
    // Split edges at vertices lying on them (T-junctions between faces).
    for f in &mut face_ids {
        for l in f.iter_mut() {
            let mut out = Vec::with_capacity(l.len());
            for i in 0..l.len() {
                let (a, b) = (l[i], l[(i + 1) % l.len()]);
                out.push(a);
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
                out.extend(on.into_iter().map(|(_, j)| j));
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
    // Each undirected edge is built once and must be used exactly twice, in
    // opposite directions.
    let mut edges: HashMap<(usize, usize), (Edge, i32)> = HashMap::new();
    let mut tfaces = Vec::with_capacity(faces.len());
    for (f, ids) in faces.iter().zip(&face_ids) {
        let mut wires = Vec::with_capacity(ids.len());
        for l in ids {
            if l.len() < 3 {
                return Err(internal("degenerate face loop"));
            }
            let mut wire = Wire::new();
            for i in 0..l.len() {
                let (a, b) = (l[i], l[(i + 1) % l.len()]);
                let key = (a.min(b), a.max(b));
                let entry = edges
                    .entry(key)
                    .or_insert_with(|| (builder::line(&vertices[key.0], &vertices[key.1]), 0));
                entry.1 += if a < b { 1 } else { -1 };
                wire.push_back(if a < b {
                    entry.0.clone()
                } else {
                    entry.0.inverse()
                });
            }
            wires.push(wire);
        }
        let o = world(f.origin);
        let plane = Plane::new(o, o + wvec(f.ex), o + wvec(f.ey));
        let face = Face::try_new(wires, Surface::Plane(plane))
            .map_err(|e| internal(&format!("face: {e:?}")))?;
        tfaces.push(face);
    }
    // Use counts: +1 and -1 per edge sum to 0; also each edge used twice.
    let mut uses: HashMap<(usize, usize), usize> = HashMap::new();
    for ids in &face_ids {
        for l in ids {
            for i in 0..l.len() {
                let (a, b) = (l[i], l[(i + 1) % l.len()]);
                *uses.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
    }
    if edges.values().any(|(_, s)| *s != 0) || uses.values().any(|&u| u != 2) {
        return Err(KernelError::InvalidResult(
            "the result would not be a manifold solid (bodies meeting along an edge or \
             at a point)"
                .into(),
        ));
    }
    // Connected?
    let mut parent: Vec<usize> = (0..face_ids.len()).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        p[i] = r;
        r
    }
    let mut owner: HashMap<(usize, usize), usize> = HashMap::new();
    for (fi, ids) in face_ids.iter().enumerate() {
        for l in ids {
            for i in 0..l.len() {
                let (a, b) = (l[i], l[(i + 1) % l.len()]);
                if let Some(&g) = owner.get(&(a.min(b), a.max(b))) {
                    let (x, y) = (find(&mut parent, fi), find(&mut parent, g));
                    parent[x] = y;
                } else {
                    owner.insert((a.min(b), a.max(b)), fi);
                }
            }
        }
    }
    let roots = (0..face_ids.len())
        .map(|i| find(&mut parent, i))
        .collect::<std::collections::HashSet<_>>()
        .len();
    if roots != 1 {
        return Err(KernelError::InvalidResult(format!(
            "the result would be {roots} disconnected solids; exactly one connected solid is \
             required"
        )));
    }
    let shell: Shell = tfaces.into();
    Solid::try_new(vec![shell]).map_err(|e| internal(&format!("solid: {e:?}")))
}

#[cfg(test)]
mod tests;
