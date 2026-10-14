//! Exact Cut/Join of a straight prism into a *prismatic* body without a 3-D
//! boolean (ADR 0006, extended to lines and arcs by ADR 0007).
//!
//! Truck 0.6 booleans fail (or return non-closed shells or wrong solids) when
//! tool faces coincide with faces of the target, or when a cut engulfs an
//! existing hole. For the common case where every face of the target is
//! either a plane perpendicular to the tool axis (a *cap*) or a surface
//! parallel to it (a *wall*: a plane, or a cylinder whose cap edges are
//! circular arcs), the body is exactly a stack of slabs: between two
//! consecutive cap levels its cross-section is a constant 2-D region bounded
//! by lines and circular arcs. The boolean is then a 2-D region boolean per
//! slab, and the result is rebuilt from the slab regions:
//!
//! * slab regions: `S_k = (S_{k-1} \ Up_k) ∪ Down_k`, where `Up_k`/`Down_k`
//!   are the up-/down-facing cap faces at level `k` (material below / above);
//! * the tool's levels split slabs; slabs inside the tool span get
//!   `S_k \ P` (Cut) or `S_k ∪ P` (Join), `P` = outer profile minus holes;
//! * result caps at level `k`: `S_{k-1} \ S_k` (up) and `S_k \ S_{k-1}` (down);
//! * result walls: each straight boundary edge of `S_k` sweeps a rectangle
//!   over the slab; rectangles in one wall plane are united into that plane's
//!   faces, so a wall is one face however many slabs it spans (face
//!   references resolve by plane and must stay unambiguous). Each arc sweeps a
//!   cylinder patch, one per span (at most a quarter turn) of the circle's
//!   partition and contiguous run of slabs;
//! * vertices are shared, T-junctions split, every edge must bound exactly two
//!   faces in opposite directions, and the shell must be connected.
//!
//! The 2-D booleans run on an integer grid ([`GRID_STEPS_PER_MM`]) via
//! `i_overlay`, which makes coincident and collinear edges exact instead of a
//! numerical accident. Arcs enter them as chains on a per-circle ring shared
//! by every loop (see `curves`), so the polygons decide only the topology; the
//! output is rebuilt from exact lines and arcs. Every coordinate of the result
//! is on the grid in the tool frame (1e-9 mm, far below
//! [`tol::KERNEL_LINEAR`]). Bodies with oblique or non-ruled faces, cap edges
//! that are neither straight nor circular, and tangent or ambiguous contacts
//! are refused with the reason. Callers verify the result like any other
//! boolean (ADR 0005).
mod curves;
mod solid;

use crate::{Body, Frame, KernelError, LoopSegment, Profile, Result, tol};
use curves::{CircleSet, ELoop, FResult, Fail, Kind, Rings};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::ShapeType;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::overlay::FloatOverlay;
use i_overlay::i_float::adapter::FloatPointAdapter;
use i_overlay::i_float::float::rect::FloatRect;
use truck_modeling::*;

/// Integer grid of the 2-D booleans: steps per mm (1e-9 mm resolution).
pub const GRID_STEPS_PER_MM: f64 = 1.0e9;

/// Distinct cap levels and wall planes closer than this are the same.
const SAME: f64 = tol::FACE_MATCH;

/// Ring resolutions tried (chords per full turn at most): a configuration
/// that is ambiguous at one resolution is retried at the next, then refused.
const RING_CHORDS: [usize; 4] = [64, 128, 256, 512];

/// Edges of a cap face and surfaces of a wall are classified to this
/// tolerance (Truck's linear tolerance): straight, circular, ruled.
const CURVE_TOL: f64 = tol::KERNEL_LINEAR;

/// A wall's unit normal may have at most this component along the axis.
const RULED_TOL: f64 = 1.0e-9;

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

impl From<KernelError> for Fail {
    fn from(e: KernelError) -> Fail {
        Fail::Kernel(e)
    }
}

/// Whether `body` is prismatic along `frame.z` and `profile` is a valid
/// line/arc profile: the cases [`prism_boolean`] can attempt.
pub fn applicable(body: &Body, profile: &Profile, frame: &Frame) -> bool {
    not_applicable(body, profile, frame).is_none()
}

/// Why [`prism_boolean`] cannot build this case, or None when it can be
/// attempted (it may still refuse a tangent or ambiguous contact).
pub fn not_applicable(body: &Body, profile: &Profile, frame: &Frame) -> Option<String> {
    if let Err(e) = profile.validate() {
        return Some(e.to_string());
    }
    let (frame, _) = right_handed(frame);
    classify_faces(body, &frame, &mut CircleSet::default()).err()
}

/// `target` cut by (or joined with) the prism of `profile` (in `start`'s 2-D
/// coordinates) swept from `start` by `length` along `start.z`, built exactly
/// from slab regions (see the module docs). `InvalidInput` when the case is not
/// prismatic or is ambiguous; `InvalidResult` when the result is not one
/// closed solid.
pub fn prism_boolean(
    target: &Body,
    profile: &Profile,
    start: &Frame,
    length: f64,
    op: SlabOp,
) -> Result<Body> {
    prism_region_boolean(target, profile, &[], start, length, op)
}

/// [`prism_boolean`] with a tool region `outer` minus `holes` (inner loops
/// strictly inside `outer`, not touching each other; the caller's sketch
/// region extraction guarantees it).
pub fn prism_region_boolean(
    target: &Body,
    outer: &Profile,
    holes: &[Profile],
    start: &Frame,
    length: f64,
    op: SlabOp,
) -> Result<Body> {
    region_boolean(target, outer, holes, start, length, op, false)
}

/// [`prism_region_boolean`] that accepts a result falling apart into several
/// separate solids: the returned body then has one closed shell per piece
/// (see [`crate::split_pieces`]). An enclosed internal void is still refused.
pub fn prism_region_boolean_pieces(
    target: &Body,
    outer: &Profile,
    holes: &[Profile],
    start: &Frame,
    length: f64,
    op: SlabOp,
) -> Result<Body> {
    region_boolean(target, outer, holes, start, length, op, true)
}

fn region_boolean(
    target: &Body,
    outer: &Profile,
    holes: &[Profile],
    start: &Frame,
    length: f64,
    op: SlabOp,
    pieces: bool,
) -> Result<Body> {
    start.validate()?;
    if !length.is_finite() || length < tol::MIN_LENGTH {
        return Err(KernelError::InvalidInput(format!(
            "prism length must be >= {} mm, got {length}",
            tol::MIN_LENGTH
        )));
    }
    outer.validate()?;
    for h in holes {
        h.validate()?;
    }
    let (frame, mirrored) = right_handed(start);
    let refuse = |m: String| KernelError::InvalidInput(format!("prismatic boolean: {m}"));
    let mut set = CircleSet::default();
    let mut tool: Vec<ELoop> = std::iter::once(outer)
        .chain(holes)
        .map(|p| tool_loop(p, mirrored, &mut set))
        .collect();
    let mut caps = classify_faces(target, &frame, &mut set).map_err(refuse)?;
    let circles = set.circles.clone();
    let fail = |f: Fail| match f {
        Fail::Refine(m) | Fail::Refuse(m) => refuse(m),
        Fail::Kernel(e) => e,
    };
    // Canonical vertices over all loops at once (tool first, so its exact
    // values represent clusters).
    let mut all: Vec<ELoop> = std::mem::take(&mut tool);
    let ntool = all.len();
    for c in &mut caps {
        all.append(&mut c.loops);
    }
    curves::canonicalize(&mut all, &circles).map_err(fail)?;
    let mut rest = all.split_off(ntool);
    tool = all;
    for c in caps.iter_mut().rev() {
        let n = c.count;
        c.loops = rest.split_off(rest.len() - n);
    }
    let mut last = String::new();
    for chords in RING_CHORDS {
        let step = std::f64::consts::TAU / chords as f64;
        match attempt(&caps, &tool, &circles, step, length, op, &frame, pieces) {
            Ok(solid) => return Ok(Body { solid }),
            Err(Fail::Refine(m)) => last = m,
            Err(f) => return Err(fail(f)),
        }
    }
    Err(refuse(format!(
        "ambiguous at the finest arc resolution ({} chords per turn): {last}",
        RING_CHORDS[RING_CHORDS.len() - 1]
    )))
}

/// `frame`, or (left-handed, e.g. a mirror copy) the same frame with its y
/// axis negated, and whether it was negated: the construction's orientation
/// rules assume x × y = z.
fn right_handed(frame: &Frame) -> (Frame, bool) {
    let (x, y, z) = (frame.x, frame.y, frame.z);
    let c = [
        x[1] * y[2] - x[2] * y[1],
        x[2] * y[0] - x[0] * y[2],
        x[0] * y[1] - x[1] * y[0],
    ];
    if c[0] * z[0] + c[1] * z[1] + c[2] * z[2] >= 0.0 {
        (*frame, false)
    } else {
        let mut f = *frame;
        f.y = [-y[0], -y[1], -y[2]];
        (f, true)
    }
}

/// Exact loop of a tool profile (y negated and arcs reversed in a mirrored
/// frame).
fn tool_loop(p: &Profile, mirrored: bool, set: &mut CircleSet) -> ELoop {
    let m = |q: [f64; 2]| if mirrored { [q[0], -q[1]] } else { q };
    match p {
        Profile::Polygon(v) => ELoop {
            pts: v.iter().map(|&q| m(q)).collect(),
            kind: vec![Kind::Line; v.len()],
        },
        Profile::Circle { center, radius } => {
            let c = m(*center);
            ELoop {
                pts: vec![[c[0] + radius, c[1]]],
                kind: vec![Kind::Arc {
                    circle: set.id(c, *radius),
                    ccw: true,
                }],
            }
        }
        Profile::Loop(segs) => {
            let mut pts = Vec::with_capacity(segs.len());
            let mut kind = Vec::with_capacity(segs.len());
            for s in segs {
                pts.push(m(s.start()));
                kind.push(match s {
                    LoopSegment::Line { .. } => Kind::Line,
                    LoopSegment::Arc { start, center, ccw } => Kind::Arc {
                        circle: set.id(
                            m(*center),
                            (start[0] - center[0]).hypot(start[1] - center[1]),
                        ),
                        ccw: *ccw != mirrored,
                    },
                });
            }
            ELoop { pts, kind }
        }
    }
}

/// One construction at ring resolution `step` (radians per chord at most).
#[allow(clippy::too_many_arguments)]
fn attempt(
    caps: &[Cap],
    tool: &[ELoop],
    circles: &[curves::Circ],
    step: f64,
    length: f64,
    op: SlabOp,
    frame: &Frame,
    pieces: bool,
) -> FResult<Solid> {
    let all: Vec<ELoop> = caps
        .iter()
        .flat_map(|c| c.loops.iter().cloned())
        .chain(tool.iter().cloned())
        .collect();
    let mut rings = Rings::new(circles, &all, step)?;
    let mut cap_shapes: Vec<(f64, bool, Shape)> = Vec::with_capacity(caps.len());
    for c in caps {
        let shape: Shape = c
            .loops
            .iter()
            .map(|l| rings.polygon(l))
            .collect::<FResult<_>>()?;
        cap_shapes.push((c.w, c.up, shape));
    }
    let tool_shape: Shape = tool
        .iter()
        .map(|l| rings.polygon(l))
        .collect::<FResult<_>>()?;
    for c in cap_shapes
        .iter()
        .flat_map(|s| s.2.iter())
        .chain(&tool_shape)
    {
        rings.learn(c);
    }
    rings.check_vertices()?;
    let rings = rings;
    let bool_op = |a: &Region, b: &Region, rule: OverlayRule| -> FResult<Region> {
        let r = boolean_keep(a, b, rule, &|p| rings.is_ring_point(p))?;
        rings.check_output(&r)?;
        Ok(r)
    };
    let norm = |r: &[Shape]| bool_op(&orient(r), &Vec::new(), OverlayRule::Union);
    let tool_region = norm(&[tool_shape])?;
    if area(&tool_region) <= 0.0 {
        return Err(Fail::Refuse("empty profile".into()));
    }

    // Levels: every cap, plus the tool's two ends.
    let mut levels: Vec<f64> = caps.iter().map(|c| c.w).chain([0.0, length]).collect();
    levels.sort_by(f64::total_cmp);
    levels.dedup_by(|b, a| (*b - *a).abs() <= SAME);
    let level_of = |w: f64| levels.iter().position(|l| (l - w).abs() <= SAME);
    let n = levels.len();
    let (mut up, mut down): (Vec<Vec<Shape>>, Vec<Vec<Shape>>) = (vec![vec![]; n], vec![vec![]; n]);
    for (w, is_up, shape) in cap_shapes {
        let k = level_of(w).ok_or_else(|| internal("cap level lost"))?;
        if is_up {
            up[k].push(shape);
        } else {
            down[k].push(shape);
        }
    }
    // Slab k lies between levels k and k + 1.
    let mut slabs: Vec<Region> = Vec::with_capacity(n.saturating_sub(1));
    let mut prev: Region = Vec::new();
    for k in 0..n {
        let below_up = bool_op(&prev, &norm(&up[k])?, OverlayRule::Difference)?;
        let cur = bool_op(&below_up, &norm(&down[k])?, OverlayRule::Union)?;
        if k + 1 == n {
            if area(&cur) > 0.0 {
                return Err(Fail::Kernel(internal(
                    "the body's cap faces do not close (not prismatic)",
                )));
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
            let next = bool_op(s, &tool_region, rule)?;
            // Any real change is far above one grid cell (1e-18 mm^2).
            changed |= (area(&next) - area(s)).abs() > 1.0e-12;
            *s = next;
        }
    }
    if !changed {
        return Err(Fail::Kernel(KernelError::InvalidInput(match op {
            SlabOp::Cut => "cut: the extrusion removes no material from the target body".into(),
            SlabOp::Join => "join: the extrusion adds no material to the target body".into(),
        })));
    }
    // Result caps.
    let empty: Region = Vec::new();
    let mut cap_regions = Vec::with_capacity(2 * n);
    for (k, &w) in levels.iter().enumerate() {
        let below = if k > 0 { &slabs[k - 1] } else { &empty };
        let above = slabs.get(k).unwrap_or(&empty);
        cap_regions.push((w, true, bool_op(below, above, OverlayRule::Difference)?));
        cap_regions.push((w, false, bool_op(above, below, OverlayRule::Difference)?));
    }
    Ok(solid::build(
        &levels,
        &slabs,
        &cap_regions,
        &rings,
        frame,
        pieces,
    )?)
}

/// A cap face of the target in tool-frame coordinates: level `w` along the
/// axis, outward normal along (+) or against (-) it, and its exact loops.
struct Cap {
    w: f64,
    up: bool,
    loops: Vec<ELoop>,
    /// Number of loops (kept while the loops are canonicalized elsewhere).
    count: usize,
}

/// Caps of `body` in `frame` coordinates (circles registered in `set`), or
/// why the body is not prismatic along `frame.z`: a face oblique to it, a
/// curved face not parallel to it, or a cap edge neither straight nor a
/// circular arc.
fn classify_faces(
    body: &Body,
    frame: &Frame,
    set: &mut CircleSet,
) -> std::result::Result<Vec<Cap>, String> {
    let z = Vector3::new(frame.z[0], frame.z[1], frame.z[2]);
    let mut caps = Vec::new();
    for f in body.solid.face_iter() {
        let surface = f.surface();
        let Surface::Plane(pl) = &surface else {
            ruled_along(&surface, z)?;
            continue; // curved wall
        };
        let mut nrm = pl.normal();
        if !f.orientation() {
            nrm = -nrm;
        }
        let along = nrm.dot(z);
        if along.abs() <= SAME {
            continue; // planar wall
        }
        if nrm.cross(z).magnitude() > SAME {
            return Err(format!(
                "the body has a face oblique to the tool axis (normal {:.6?})",
                [nrm.x, nrm.y, nrm.z]
            ));
        }
        let mut loops = Vec::new();
        let mut w = 0.0;
        let mut count = 0.0;
        for wire in f.absolute_boundaries() {
            let mut l = ELoop {
                pts: Vec::new(),
                kind: Vec::new(),
            };
            for e in wire.edge_iter() {
                let p = to_frame(frame, e.front().point());
                w += p[2];
                count += 1.0;
                l.pts.push([p[0], p[1]]);
                l.kind.push(edge_kind(e, frame, set)?);
            }
            if l.pts.len() == 1 && l.kind[0] == Kind::Line {
                return Err("a cap face has a closed straight edge".into());
            }
            if l.pts.len() > 1 && wire.edge_iter().any(|e| e.front() == e.back()) {
                return Err("a cap face loop mixes a closed edge with others".into());
            }
            loops.push(l);
        }
        if count == 0.0 {
            return Err("a face has no boundary".into());
        }
        caps.push(Cap {
            w: w / count,
            up: along > 0.0,
            count: loops.len(),
            loops,
        });
    }
    Ok(caps)
}

/// A curved face must be a wall: unit normal perpendicular to `z` on a 9 × 9
/// grid of its parameter domain.
fn ruled_along(s: &Surface, z: Vector3) -> std::result::Result<(), String> {
    let bound = |b: std::ops::Bound<f64>| match b {
        std::ops::Bound::Included(x) | std::ops::Bound::Excluded(x) => Some(x),
        std::ops::Bound::Unbounded => None,
    };
    let ((a0, a1), (b0, b1)) = s.parameter_range();
    let (Some(u0), Some(u1), Some(v0), Some(v1)) = (bound(a0), bound(a1), bound(b0), bound(b1))
    else {
        return Err("the body has a curved face with an unbounded domain".into());
    };
    for i in 0..=8 {
        for j in 0..=8 {
            let (u, v) = (
                u0 + (u1 - u0) * i as f64 / 8.0,
                v0 + (v1 - v0) * j as f64 / 8.0,
            );
            let n = s.normal(u, v);
            if !(n.x.is_finite() && n.y.is_finite() && n.z.is_finite()) {
                continue;
            }
            if n.dot(z).abs() > RULED_TOL {
                return Err(
                    "the body has a curved face that is not parallel to the tool axis".into(),
                );
            }
        }
    }
    Ok(())
}

/// Straight edge, or circular arc (circle registered in `set`) traversed
/// counter-clockwise or clockwise about the frame normal, to [`CURVE_TOL`].
fn edge_kind(e: &Edge, frame: &Frame, set: &mut CircleSet) -> std::result::Result<Kind, String> {
    let curve = e.oriented_curve();
    if matches!(curve, Curve::Line(_)) {
        return Ok(Kind::Line);
    }
    let (t0, t1) = curve.range_tuple();
    const N: usize = 16;
    let s: Vec<[f64; 3]> = (0..=N)
        .map(|i| to_frame(frame, curve.subs(t0 + (t1 - t0) * i as f64 / N as f64)))
        .collect();
    let p = to_frame(frame, e.front().point());
    let q = to_frame(frame, e.back().point());
    if s.iter().any(|x| (x[2] - p[2]).abs() > CURVE_TOL) {
        return Err("a cap face has an edge that leaves its plane".into());
    }
    let p2 = |x: [f64; 3]| [x[0], x[1]];
    let closed = e.front() == e.back();
    if !closed
        && s.iter()
            .all(|x| seg_dist(p2(*x), p2(p), p2(q)) <= CURVE_TOL)
    {
        return Ok(Kind::Line);
    }
    // Circle through three samples spread over the edge.
    let (a, b, c) = (p2(s[0]), p2(s[N / 3]), p2(s[2 * N / 3]));
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d.abs() < 1e-300 {
        return Err("a cap face has a curved edge that is not a circular arc".into());
    }
    let sq = |v: [f64; 2]| v[0] * v[0] + v[1] * v[1];
    let center = [
        (sq(a) * (b[1] - c[1]) + sq(b) * (c[1] - a[1]) + sq(c) * (a[1] - b[1])) / d,
        (sq(a) * (c[0] - b[0]) + sq(b) * (a[0] - c[0]) + sq(c) * (b[0] - a[0])) / d,
    ];
    let r = (a[0] - center[0]).hypot(a[1] - center[1]);
    let on = |x: [f64; 2]| ((x[0] - center[0]).hypot(x[1] - center[1]) - r).abs() <= CURVE_TOL;
    if !s.iter().all(|x| on(p2(*x))) || !on(p2(p)) || !on(p2(q)) {
        return Err("a cap face has a curved edge that is not a circular arc".into());
    }
    // Direction from the first sample step (1/16 of the edge: under a half turn).
    let (u, v) = (p2(s[0]), p2(s[1]));
    let cr = (u[0] - center[0]) * (v[1] - center[1]) - (u[1] - center[1]) * (v[0] - center[0]);
    Ok(Kind::Arc {
        circle: set.id(center, r),
        ccw: cr > 0.0,
    })
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
    boolean(&orient(r), &Vec::new(), OverlayRule::Union)
}

/// Each shape's largest contour counter-clockwise, the others clockwise.
fn orient(r: &[Shape]) -> Region {
    r.iter()
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
        .collect()
}

/// `a op b` on the integer grid (non-zero fill), each output shape oriented
/// outer CCW / holes CW.
fn boolean(a: &Region, b: &Region, rule: OverlayRule) -> Result<Region> {
    boolean_keep(a, b, rule, &|_| false)
}

/// [`boolean`] whose output clean-up never removes a point for which `keep`
/// holds (the ring points of the arc construction: two special points on a
/// circle very close together leave a ring vertex within `SAME` of their
/// chord, which must stay an arc vertex).
fn boolean_keep(
    a: &Region,
    b: &Region,
    rule: OverlayRule,
    keep: &dyn Fn([f64; 2]) -> bool,
) -> Result<Region> {
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
                    (i, clean_contour(c, keep))
                })
                // A shape whose outer contour vanished at the merge scale is a
                // sliver; drop it with its holes.
                .take_while(|(i, c)| *i > 0 || c.is_some())
                .filter_map(|(_, c)| c)
                .collect::<Shape>()
        })
        .filter(|s: &Shape| !s.is_empty())
        .collect())
}

/// Remove features of a boolean output contour narrower than [`SAME`], the
/// scale at which the 3-D rebuild merges vertices anyway: consecutive points
/// within `SAME`, vertices within `SAME` of the segment joining their
/// neighbours (collinear points and zero-width spikes `a -> b -> a`), and
/// contours whose area is no more than `SAME` times their half-perimeter
/// (slivers). B-rep coordinates carry float noise (e.g. a wall at
/// x = 117.99999999955 at one end and 117.99999999949 at the other); on the
/// 1e-9 mm grid that can round to adjacent grid points and leave a 1e-9 mm
/// sliver or spike, which would become a non-simple face loop (rim notch
/// repro). None when nothing is left.
fn clean_contour(mut c: Contour, keep: &dyn Fn([f64; 2]) -> bool) -> Option<Contour> {
    let near =
        |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() <= SAME && (a[1] - b[1]).abs() <= SAME;
    loop {
        let n = c.len();
        if n < 3 {
            return None;
        }
        let mut removed = false;
        for i in 0..n {
            let (a, b, d) = (c[(i + n - 1) % n], c[i], c[(i + 1) % n]);
            if near(a, b) && !(keep(a) && keep(b)) {
                // Drop the one that is not protected.
                c.remove(if keep(b) { (i + n - 1) % n } else { i });
                removed = true;
                break;
            }
            // Spike tip (a ≈ d): drop the tip and the returning point.
            if near(a, d) {
                let j = (i + 1) % n;
                let (hi, lo) = if j > i { (j, i) } else { (i, j) };
                c.remove(hi);
                c.remove(lo);
                removed = true;
                break;
            }
            if seg_dist(b, a, d) <= SAME && !keep(b) {
                c.remove(i);
                removed = true;
                break;
            }
        }
        if !removed {
            break;
        }
    }
    let perim: f64 = (0..c.len())
        .map(|i| {
            let (p, q) = (c[i], c[(i + 1) % c.len()]);
            (q[0] - p[0]).hypot(q[1] - p[1])
        })
        .sum();
    (signed_area(&c).abs() > SAME * perim / 2.0).then_some(c)
}

/// Distance from `p` to the segment `ab`.
fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let e = [b[0] - a[0], b[1] - a[1]];
    let l2 = e[0] * e[0] + e[1] * e[1];
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * e[0] + (p[1] - a[1]) * e[1]) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * e[0]).hypot(p[1] - a[1] - t * e[1])
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

#[cfg(test)]
mod arc_tests;
#[cfg(test)]
mod tests;
