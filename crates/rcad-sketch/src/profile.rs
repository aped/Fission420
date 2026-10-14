//! Closed-profile extraction from solved sketch geometry.
//!
//! A profile is either a single circle or a single closed loop of lines and
//! arcs (shared endpoints or `Coincident` points, every vertex of degree 2).
//! Line-only loops keep the [`Profile::Polygon`] form; loops with arcs
//! are [`Profile::Loop`].
//!
//! An ellipse or a closed spline is a loop on its own; an open spline joins
//! lines and arcs at its end control points like any other curve. Both become
//! exact rational cubic [`LoopSegment::Cubic`] spans ([`crate::curves`]).
//!
//! Construction entities (`Sketch::construction`) are ignored here: they take
//! part in solving only.
//!
//! Tolerance: every geometric test uses the sketch solve tolerance
//! `tol = solve_tolerance(scale)` (mm); angular tests on an arc of radius `r`
//! use `tol / r` (rad), i.e. the same distance measured along the arc.

use crate::arc::ccw_angle;
use crate::{Constraint, Entity, EntityId, PointId, Sketch, SketchError, solve_tolerance};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::f64::consts::TAU;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Profile {
    /// Simple polygon, counter-clockwise, starting at the lowest (then leftmost)
    /// vertex. Not explicitly closed (last != first).
    Polygon(Vec<[f64; 2]>),
    Circle {
        center: [f64; 2],
        radius: f64,
    },
    /// Simple closed loop of lines and arcs (at least one arc), counter-clockwise
    /// overall, starting at the lowest (then leftmost) vertex. Each segment ends
    /// where the next one starts; the last ends at the first one's start.
    Loop(Vec<LoopSegment>),
}

/// One segment of a [`Profile::Loop`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum LoopSegment {
    Line {
        start: [f64; 2],
    },
    /// Circular arc about `center` (radius `|start − center|`), traversed
    /// counter-clockwise when `ccw`, clockwise otherwise.
    Arc {
        start: [f64; 2],
        center: [f64; 2],
        ccw: bool,
    },
    /// Rational cubic Bézier span with control points `start, c1, c2, end`
    /// (`end` the next segment's start) and weights `1, w[0], w[1], 1`; all
    /// weights positive. Exact span of an ellipse (rational) or of a cubic
    /// B-spline (`w == [1, 1]`), see [`crate::curves`]. Serde-additive.
    Cubic {
        start: [f64; 2],
        c1: [f64; 2],
        c2: [f64; 2],
        w: [f64; 2],
    },
}

impl LoopSegment {
    pub fn start(&self) -> [f64; 2] {
        match self {
            LoopSegment::Line { start }
            | LoopSegment::Arc { start, .. }
            | LoopSegment::Cubic { start, .. } => *start,
        }
    }
}

/// Does any segment of the loop carry a free-form span?
pub(crate) fn has_cubic(segs: &[LoopSegment]) -> bool {
    segs.iter().any(|s| matches!(s, LoopSegment::Cubic { .. }))
}

/// Exact signed area of a loop (counter-clockwise positive): per segment the
/// Green's-theorem term `∮ (x dy − y dx) / 2`, closed form for lines and arcs,
/// Gauss–Legendre for cubic spans ([`crate::curves::cubic_area_term`]).
pub fn loop_area(segs: &[LoopSegment]) -> f64 {
    let k = segs.len();
    let mut a = 0.0;
    for (i, s) in segs.iter().enumerate() {
        let p = s.start();
        let q = segs[(i + 1) % k].start();
        match *s {
            LoopSegment::Line { .. } => a += 0.5 * (p[0] * q[1] - q[0] * p[1]),
            LoopSegment::Arc { center, ccw, .. } => {
                a += 0.5 * (p[0] * q[1] - q[0] * p[1]);
                let r = dist(center, p);
                let (from, to) = if ccw { (p, q) } else { (q, p) };
                let sweep = ccw_angle(center, from, to);
                let th = if ccw { sweep } else { -sweep };
                a += 0.5 * r * r * (th - th.sin());
            }
            LoopSegment::Cubic { c1, c2, w, .. } => {
                a += crate::curves::cubic_area_term(p, c1, c2, q, w);
            }
        }
    }
    a
}

/// Union-find over point indices, merging `Coincident` points into one vertex.
fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

#[derive(Clone, Copy, Debug)]
enum EdgeKind {
    Line,
    /// Stored CCW from the edge's `u` to `v`.
    Arc {
        center: [f64; 2],
    },
    /// Open spline: index into the span table, spans stored from `u` to `v`.
    Spline(usize),
}

pub(crate) fn extract(s: &Sketch) -> Result<Profile, SketchError> {
    extract_with_entities(s).map(|(p, _)| p)
}

/// Like [`extract`], also returning the sketch entity that generates each profile
/// segment (polygon edge `i` runs from vertex `i` to `i + 1`; a circle has one).
pub(crate) fn extract_with_entities(s: &Sketch) -> Result<(Profile, Vec<EntityId>), SketchError> {
    let residual = s.residual()?;
    let tol = solve_tolerance(s.scale());
    if residual > tol {
        return Err(SketchError::NotSolved {
            residual,
            tolerance: tol,
        });
    }

    let pidx: HashMap<PointId, usize> = s
        .points
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id, i))
        .collect();
    let coord = |id: &PointId| -> [f64; 2] {
        let p = &s.points[pidx[id]];
        [p.x, p.y]
    };

    // (start point, end point, kind) per line/arc; circles separately.
    let mut curves: Vec<(PointId, PointId, EdgeKind)> = Vec::new();
    let mut curve_ids: Vec<EntityId> = Vec::new();
    let mut circles: Vec<(PointId, f64)> = Vec::new();
    let mut circle_ids: Vec<EntityId> = Vec::new();
    // Ellipses and closed splines (each a loop of its own).
    let mut closed: Vec<&Entity> = Vec::new();
    // Bézier spans of each open spline, from its first to its last point.
    let mut spline_spans: Vec<Vec<[[f64; 2]; 4]>> = Vec::new();
    for e in s.entities.iter().filter(|e| !s.is_construction(e.id())) {
        match e {
            Entity::Ellipse { .. } | Entity::Spline { closed: true, .. } => closed.push(e),
            Entity::Spline { id, points, .. } => {
                let (Some(a), Some(b)) = (points.first(), points.last()) else {
                    continue;
                };
                curves.push((*a, *b, EdgeKind::Spline(spline_spans.len())));
                spline_spans.push(crate::curves::open_spline_spans(s, points));
                curve_ids.push(*id);
            }
            Entity::Line { a, b, id } => {
                curves.push((*a, *b, EdgeKind::Line));
                curve_ids.push(*id);
            }
            Entity::Arc {
                center,
                start,
                end,
                id,
            } => {
                curves.push((
                    *start,
                    *end,
                    EdgeKind::Arc {
                        center: coord(center),
                    },
                ));
                curve_ids.push(*id);
            }
            Entity::Circle { center, radius, id } => {
                circles.push((*center, *radius));
                circle_ids.push(*id);
            }
        }
    }
    let n_arcs = curves
        .iter()
        .filter(|c| !matches!(c.2, EdgeKind::Line))
        .count();
    let n_lines = curves.len() - n_arcs;
    let n_splines = spline_spans.len();
    if !closed.is_empty() {
        if closed.len() == 1 && curves.is_empty() && circles.is_empty() {
            return closed_curve_profile(s, closed[0], tol);
        }
        return Err(SketchError::UnsupportedProfile(format!(
            "expected one closed loop; found {} ellipse(s)/closed spline(s) with {} other curves \
             and {} circles",
            closed.len(),
            curves.len(),
            circles.len()
        )));
    }

    match (curves.len(), circles.len()) {
        (0, 1) => {
            let (c, r) = circles[0];
            if r <= tol {
                return Err(SketchError::DegenerateProfile(format!(
                    "circle radius {r} mm is not positive"
                )));
            }
            return Ok((
                Profile::Circle {
                    center: coord(&c),
                    radius: r,
                },
                circle_ids,
            ));
        }
        (0, 0) => {
            return Err(SketchError::UnsupportedProfile(
                "sketch has no curves".into(),
            ));
        }
        (_, 0) => {}
        _ => {
            return Err(SketchError::UnsupportedProfile(if n_splines > 0 {
                format!(
                    "expected one closed loop or a single circle; found {} curves (with {} \
                     splines) and {} circles",
                    curves.len(),
                    n_splines,
                    circles.len()
                )
            } else if n_arcs == 0 {
                format!(
                    "expected one closed loop of lines or a single circle; found {} lines and {} circles",
                    n_lines,
                    circles.len()
                )
            } else {
                format!(
                    "expected one closed loop of lines/arcs or a single circle; found {} lines, \
                     {} arcs and {} circles",
                    n_lines,
                    n_arcs,
                    circles.len()
                )
            }));
        }
    }
    let noun = if n_arcs == 0 { "lines" } else { "curves" };

    // Merge coincident points.
    let mut parent: Vec<usize> = (0..s.points.len()).collect();
    for c in &s.constraints {
        if let Constraint::Coincident { a, b } = c {
            let (ra, rb) = (find(&mut parent, pidx[a]), find(&mut parent, pidx[b]));
            parent[ra] = rb;
        }
    }
    let mut edges: Vec<(usize, usize, EdgeKind)> = Vec::with_capacity(curves.len());
    for (a, b, kind) in &curves {
        let (ra, rb) = (find(&mut parent, pidx[a]), find(&mut parent, pidx[b]));
        if ra == rb {
            return Err(SketchError::DegenerateProfile(match kind {
                EdgeKind::Line => "line endpoints are coincident".into(),
                EdgeKind::Arc { .. } => {
                    "arc start and end are the same vertex (zero or full sweep)".into()
                }
                EdgeKind::Spline(_) => "an open spline's first and last control points are \
                                        the same vertex (use a closed spline)"
                    .into(),
            }));
        }
        edges.push((ra, rb, *kind));
    }
    if n_arcs == 0 && edges.len() < 3 {
        return Err(SketchError::UnsupportedProfile(format!(
            "{} line(s) cannot form a closed loop",
            edges.len()
        )));
    }
    // Every vertex must have degree exactly 2.
    let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
    for (ei, &(a, b, _)) in edges.iter().enumerate() {
        adj.entry(a).or_default().push(ei);
        adj.entry(b).or_default().push(ei);
    }
    if let Some((_, v)) = adj.iter().find(|(_, v)| v.len() != 2) {
        return Err(SketchError::UnsupportedProfile(format!(
            "{noun} do not form a single closed loop (a vertex joins {} {noun})",
            v.len()
        )));
    }
    // Walk the cycle: (edge, traversed from its stored start to its stored end).
    let start = edges[0].0;
    let mut walk: Vec<(usize, bool)> = Vec::with_capacity(edges.len());
    let mut used = vec![false; edges.len()];
    let mut cur = start;
    let mut prev_edge = usize::MAX;
    while let Some(&e) = adj[&cur].iter().find(|&&e| e != prev_edge && !used[e]) {
        used[e] = true;
        prev_edge = e;
        let (a, b, _) = edges[e];
        let forward = a == cur;
        walk.push((e, forward));
        cur = if forward { b } else { a };
        if cur == start {
            break;
        }
    }
    if used.iter().any(|u| !u) || cur != start {
        return Err(SketchError::UnsupportedProfile(format!(
            "{noun} form more than one loop"
        )));
    }

    let rep_xy = |v: usize| {
        let p = &s.points[v];
        [p.x, p.y]
    };
    let mut segs: Vec<LoopSegment> = Vec::with_capacity(walk.len());
    let mut ents: Vec<EntityId> = Vec::with_capacity(walk.len());
    for &(e, forward) in &walk {
        let (a, b, kind) = edges[e];
        let from = if forward { a } else { b };
        match kind {
            EdgeKind::Line => segs.push(LoopSegment::Line {
                start: rep_xy(from),
            }),
            EdgeKind::Arc { center } => segs.push(LoopSegment::Arc {
                start: rep_xy(from),
                center,
                ccw: forward,
            }),
            EdgeKind::Spline(i) => {
                let spans = &spline_spans[i];
                let n = spans.len();
                for j in 0..n {
                    let [p0, c1, c2, _] = if forward {
                        spans[j]
                    } else {
                        let [p0, c1, c2, p3] = spans[n - 1 - j];
                        [p3, c2, c1, p0]
                    };
                    // The vertex joining other curves keeps its representative
                    // coordinates (coincident points agree to the solve tolerance).
                    let start = if j == 0 { rep_xy(from) } else { p0 };
                    segs.push(LoopSegment::Cubic {
                        start,
                        c1,
                        c2,
                        w: [1.0, 1.0],
                    });
                    ents.push(curve_ids[e]);
                }
                continue;
            }
        }
        ents.push(curve_ids[e]);
    }
    let k = segs.len();
    check_cubic_spans(&segs, tol)?;
    let delta = crate::curves::loop_flatten_tolerance(&segs);
    let geo: Vec<Prim> = loop_prims(&segs, delta);
    let ctol = if has_cubic(&segs) { tol + delta } else { tol };
    for g in &geo {
        match *g {
            Prim::Seg { a, b } => {
                if dist(a, b) <= tol {
                    return Err(SketchError::DegenerateProfile("zero-length edge".into()));
                }
            }
            Prim::Arc { p, q, r, sweep, .. } => {
                if r <= tol {
                    return Err(SketchError::DegenerateProfile(format!(
                        "arc radius {r:.3e} mm is not positive"
                    )));
                }
                if dist(p, q) <= tol || sweep * r <= tol {
                    return Err(SketchError::DegenerateProfile(
                        "arc has zero sweep (start and end coincide)".into(),
                    ));
                }
            }
        }
    }
    if loop_self_intersects(&geo, ctol) {
        return Err(SketchError::UnsupportedProfile(
            "loop is self-intersecting".into(),
        ));
    }
    let area = if has_cubic(&segs) {
        loop_area(&segs)
    } else {
        signed_area(&geo)
    };
    let perimeter: f64 = geo.iter().map(Prim::length).sum();
    if area.abs() <= 0.5 * tol * perimeter {
        return Err(SketchError::DegenerateProfile(
            "loop encloses zero area".into(),
        ));
    }
    if area < 0.0 {
        segs = reversed(&segs);
        ents.reverse();
    }
    // Start at the lowest vertex (within tol), then leftmost.
    let ymin = segs
        .iter()
        .map(|s| s.start()[1])
        .fold(f64::INFINITY, f64::min);
    let first = (0..k)
        .filter(|&i| segs[i].start()[1] <= ymin + tol)
        .min_by(|&i, &j| segs[i].start()[0].total_cmp(&segs[j].start()[0]))
        .unwrap_or(0);
    segs.rotate_left(first);
    ents.rotate_left(first);
    if n_arcs == 0 {
        return Ok((
            Profile::Polygon(segs.iter().map(LoopSegment::start).collect()),
            ents,
        ));
    }
    Ok((Profile::Loop(segs), ents))
}

/// Every cubic span must have a chord longer than `tol` and finite, positive
/// data (a span whose ends coincide comes from repeated control points).
fn check_cubic_spans(segs: &[LoopSegment], tol: f64) -> Result<(), SketchError> {
    let k = segs.len();
    for (i, s) in segs.iter().enumerate() {
        if let LoopSegment::Cubic { start, c1, c2, w } = *s {
            let end = segs[(i + 1) % k].start();
            if [start, c1, c2, end]
                .iter()
                .flatten()
                .any(|v| !v.is_finite())
                || !(w[0] > 0.0 && w[1] > 0.0)
            {
                return Err(SketchError::DegenerateProfile(
                    "curve span has non-finite control data".into(),
                ));
            }
            if dist(start, end) <= tol {
                return Err(SketchError::DegenerateProfile(
                    "spline span starts and ends at the same point (coincident control \
                     points)"
                        .into(),
                ));
            }
        }
    }
    Ok(())
}

/// Profile of a lone ellipse or closed spline.
fn closed_curve_profile(
    s: &Sketch,
    e: &Entity,
    tol: f64,
) -> Result<(Profile, Vec<EntityId>), SketchError> {
    if let Entity::Ellipse {
        minor,
        center,
        major,
        ..
    } = e
    {
        let a = match (s.point_xy(*center), s.point_xy(*major)) {
            (Some(c), Some(m)) => dist(c, m),
            _ => 0.0,
        };
        if *minor <= tol || a <= tol {
            return Err(SketchError::DegenerateProfile(format!(
                "ellipse radii {a:.3e} / {minor:.3e} mm must be positive"
            )));
        }
    }
    let mut segs = crate::curves::closed_segments(s, e);
    check_cubic_spans(&segs, tol)?;
    let delta = crate::curves::loop_flatten_tolerance(&segs);
    let geo = loop_prims(&segs, delta);
    if loop_self_intersects(&geo, tol + delta) {
        return Err(SketchError::UnsupportedProfile(
            "closed spline is self-intersecting".into(),
        ));
    }
    let area = loop_area(&segs);
    let perimeter: f64 = geo.iter().map(Prim::length).sum();
    if area.abs() <= 0.5 * tol * perimeter {
        return Err(SketchError::DegenerateProfile(
            "loop encloses zero area".into(),
        ));
    }
    if area < 0.0 {
        segs = reversed(&segs);
    }
    let k = segs.len();
    let ymin = segs
        .iter()
        .map(|s| s.start()[1])
        .fold(f64::INFINITY, f64::min);
    let first = (0..k)
        .filter(|&i| segs[i].start()[1] <= ymin + tol)
        .min_by(|&i, &j| segs[i].start()[0].total_cmp(&segs[j].start()[0]))
        .unwrap_or(0);
    segs.rotate_left(first);
    Ok((Profile::Loop(segs), vec![e.id(); k]))
}

/// Boundary primitives of a loop: lines and arcs exactly, cubic spans as a
/// polyline within `delta` of the curve ([`crate::curves::flatten`]).
pub(crate) fn loop_prims(segs: &[LoopSegment], delta: f64) -> Vec<Prim> {
    let k = segs.len();
    let mut out = Vec::with_capacity(k);
    for i in 0..k {
        let end = segs[(i + 1) % k].start();
        match segs[i] {
            LoopSegment::Cubic { start, c1, c2, w } => {
                let pts = crate::curves::flatten(start, c1, c2, end, w, delta);
                out.extend(pts.windows(2).map(|p| Prim::Seg { a: p[0], b: p[1] }));
            }
            ref seg => out.push(Prim::of(seg, end)),
        }
    }
    out
}

/// The same loop traversed the other way.
fn reversed(segs: &[LoopSegment]) -> Vec<LoopSegment> {
    let k = segs.len();
    (0..k)
        .map(|j| {
            let i = k - 1 - j;
            let start = segs[(i + 1) % k].start();
            match segs[i] {
                LoopSegment::Line { .. } => LoopSegment::Line { start },
                LoopSegment::Arc { center, ccw, .. } => LoopSegment::Arc {
                    start,
                    center,
                    ccw: !ccw,
                },
                LoopSegment::Cubic { c1, c2, w, .. } => LoopSegment::Cubic {
                    start,
                    c1: c2,
                    c2: c1,
                    w: [w[1], w[0]],
                },
            }
        })
        .collect()
}

pub(crate) fn dist(p: [f64; 2], q: [f64; 2]) -> f64 {
    (q[0] - p[0]).hypot(q[1] - p[1])
}

/// Segment geometry with its end point resolved.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Prim {
    Seg {
        a: [f64; 2],
        b: [f64; 2],
    },
    /// Traversed from `p` to `q`. The point set is the CCW arc of radius `r`
    /// (= |p − c|) from polar angle `a0` sweeping `sweep` in (0, 2π).
    Arc {
        p: [f64; 2],
        q: [f64; 2],
        c: [f64; 2],
        r: f64,
        ccw: bool,
        a0: f64,
        sweep: f64,
    },
}

impl Prim {
    /// A line or arc segment (a cubic span gives its chord; use [`loop_prims`]).
    pub(crate) fn of(seg: &LoopSegment, end: [f64; 2]) -> Prim {
        match *seg {
            LoopSegment::Line { start } | LoopSegment::Cubic { start, .. } => {
                Prim::Seg { a: start, b: end }
            }
            LoopSegment::Arc { start, center, ccw } => {
                let (from, to) = if ccw { (start, end) } else { (end, start) };
                Prim::Arc {
                    p: start,
                    q: end,
                    c: center,
                    r: dist(center, start),
                    ccw,
                    a0: (from[1] - center[1]).atan2(from[0] - center[0]),
                    sweep: ccw_angle(center, from, to),
                }
            }
        }
    }

    fn length(&self) -> f64 {
        match *self {
            Prim::Seg { a, b } => dist(a, b),
            Prim::Arc { r, sweep, .. } => r * sweep,
        }
    }

    fn start(&self) -> [f64; 2] {
        match *self {
            Prim::Seg { a, .. } => a,
            Prim::Arc { p, .. } => p,
        }
    }
}

/// Exact signed area: shoelace over the chords plus, for every arc, the signed
/// circular-segment area `r²/2 (θ − sin θ)` between arc and chord (θ the signed
/// sweep, positive counter-clockwise).
pub(crate) fn signed_area(geo: &[Prim]) -> f64 {
    let k = geo.len();
    let mut a2 = 0.0;
    let mut seg = 0.0;
    for (i, g) in geo.iter().enumerate() {
        let p = g.start();
        let q = geo[(i + 1) % k].start();
        a2 += p[0] * q[1] - q[0] * p[1];
        if let Prim::Arc { r, ccw, sweep, .. } = *g {
            let th = if ccw { sweep } else { -sweep };
            seg += 0.5 * r * r * (th - th.sin());
        }
    }
    0.5 * a2 + seg
}

fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Is point `x` (assumed on the arc's circle) within the arc's angular span,
/// widened by `tol` measured along the arc?
fn in_span(a0: f64, sweep: f64, c: [f64; 2], r: f64, x: [f64; 2], tol: f64) -> bool {
    let phi = ((x[1] - c[1]).atan2(x[0] - c[0]) - a0).rem_euclid(TAU);
    let d = tol / r;
    phi <= sweep + d || phi >= TAU - d
}

/// O(n²) check of every pair of segments. Non-adjacent segments may not touch
/// at all; adjacent segments may only touch at their shared vertex (or both
/// shared vertices for a two-segment loop). Touching within `tol` counts.
pub(crate) fn loop_self_intersects(geo: &[Prim], tol: f64) -> bool {
    let k = geo.len();
    for i in 0..k {
        for j in (i + 1)..k {
            let mut shared: Vec<[f64; 2]> = Vec::new();
            if j == i + 1 {
                shared.push(geo[j].start());
            }
            if i == 0 && j == k - 1 {
                shared.push(geo[0].start());
            }
            let hit = match (geo[i], geo[j]) {
                (Prim::Seg { .. }, Prim::Seg { .. }) if !shared.is_empty() => false,
                (Prim::Seg { a, b }, Prim::Seg { a: c, b: d }) => segments_touch(a, b, c, d, tol),
                (Prim::Seg { a, b }, arc @ Prim::Arc { .. })
                | (arc @ Prim::Arc { .. }, Prim::Seg { a, b }) => {
                    line_arc_hits(a, b, &arc, &shared, tol)
                }
                (x @ Prim::Arc { .. }, y @ Prim::Arc { .. }) => arc_arc_hits(&x, &y, &shared, tol),
            };
            if hit {
                return true;
            }
        }
    }
    false
}

/// Line-line test (exact orientation predicates at `tol × length`).
pub(crate) fn segments_touch(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2], tol: f64) -> bool {
    let la = dist(a, b);
    let lc = dist(c, d);
    // Orientation values are areas; compare against tol * length.
    let (d1, d2) = (orient(c, d, a), orient(c, d, b));
    let (d3, d4) = (orient(a, b, c), orient(a, b, d));
    let t1 = tol * lc;
    let t2 = tol * la;
    let straddle1 = (d1 > t1 && d2 < -t1) || (d1 < -t1 && d2 > t1);
    let straddle2 = (d3 > t2 && d4 < -t2) || (d3 < -t2 && d4 > t2);
    if straddle1 && straddle2 {
        return true;
    }
    // Touching / collinear overlap: a vertex lying on the other edge.
    let on = |p: [f64; 2], s0: [f64; 2], s1: [f64; 2], o: f64, t: f64| {
        o.abs() <= t
            && p[0] >= s0[0].min(s1[0]) - tol
            && p[0] <= s0[0].max(s1[0]) + tol
            && p[1] >= s0[1].min(s1[1]) - tol
            && p[1] <= s0[1].max(s1[1]) + tol
    };
    on(a, c, d, d1, t1) || on(b, c, d, d2, t1) || on(c, a, b, d3, t2) || on(d, a, b, d4, t2)
}

fn near_any(x: [f64; 2], pts: &[[f64; 2]], tol: f64) -> bool {
    pts.iter().any(|&p| dist(p, x) <= tol)
}

/// Does segment `a`-`b` meet the arc anywhere except the `shared` vertices?
pub(crate) fn line_arc_hits(
    a: [f64; 2],
    b: [f64; 2],
    arc: &Prim,
    shared: &[[f64; 2]],
    tol: f64,
) -> bool {
    let Prim::Arc {
        c, r, a0, sweep, ..
    } = *arc
    else {
        return false;
    };
    let span = |x: [f64; 2]| in_span(a0, sweep, c, r, x, tol);
    match shared.len() {
        // A two-segment loop: the line is a chord through both arc ends; a line
        // meets a circle at most twice, so there is nothing else.
        2 => false,
        1 => {
            // Both lie on the circle through the shared vertex V. The line's other
            // circle crossing is at V + t2·u with t2 = −2 u·(V − c) (no square root,
            // so a tangent joint gives t2 ≈ 0 robustly).
            let v = shared[0];
            let w = if dist(a, v) <= dist(b, v) { b } else { a };
            let l = dist(v, w);
            let u = [(w[0] - v[0]) / l, (w[1] - v[1]) / l];
            let t2 = -2.0 * (u[0] * (v[0] - c[0]) + u[1] * (v[1] - c[1]));
            if t2 <= tol || t2 > l + tol {
                return false;
            }
            span([v[0] + t2 * u[0], v[1] + t2 * u[1]])
        }
        _ => {
            let l = dist(a, b);
            let u = [(b[0] - a[0]) / l, (b[1] - a[1]) / l];
            let ac = [c[0] - a[0], c[1] - a[1]];
            let t0 = ac[0] * u[0] + ac[1] * u[1];
            let h = (u[0] * ac[1] - u[1] * ac[0]).abs();
            if h > r + tol {
                return false;
            }
            let m = (r * r - h * h).max(0.0).sqrt();
            [t0 - m, t0 + m]
                .into_iter()
                .any(|t| t >= -tol && t <= l + tol && span([a[0] + t * u[0], a[1] + t * u[1]]))
        }
    }
}

/// Do two arcs meet anywhere except the `shared` vertices?
pub(crate) fn arc_arc_hits(x: &Prim, y: &Prim, shared: &[[f64; 2]], tol: f64) -> bool {
    let (
        Prim::Arc {
            p: p1,
            q: q1,
            c: c1,
            r: r1,
            a0: s1,
            sweep: w1,
            ..
        },
        Prim::Arc {
            p: p2,
            q: q2,
            c: c2,
            r: r2,
            a0: s2,
            sweep: w2,
            ..
        },
    ) = (*x, *y)
    else {
        return false;
    };
    let on1 = |z: [f64; 2]| in_span(s1, w1, c1, r1, z, tol);
    let on2 = |z: [f64; 2]| in_span(s2, w2, c2, r2, z, tol);
    let foreign = |z: [f64; 2]| !near_any(z, shared, tol);
    let dc = dist(c1, c2);
    if dc <= tol && (r1 - r2).abs() <= tol {
        // Same circle: any overlap of positive length, or a touching endpoint that
        // is not a shared vertex.
        let r = 0.5 * (r1 + r2);
        if span_overlap(s1, w1, s2, w2) * r > tol {
            return true;
        }
        return [p2, q2].into_iter().any(|z| on1(z) && foreign(z))
            || [p1, q1].into_iter().any(|z| on2(z) && foreign(z));
    }
    if dc <= tol {
        return false; // concentric, different radii
    }
    let u = [(c2[0] - c1[0]) / dc, (c2[1] - c1[1]) / dc];
    if shared.len() == 1 {
        // Both circles pass through V; the other common point is V mirrored in
        // the line of centres (tangent circles: the mirror image is V itself).
        let v = shared[0];
        let d = [v[0] - c1[0], v[1] - c1[1]];
        let t = d[0] * u[0] + d[1] * u[1];
        let w = [c1[0] + 2.0 * t * u[0] - d[0], c1[1] + 2.0 * t * u[1] - d[1]];
        return foreign(w) && on1(w) && on2(w);
    }
    if dc > r1 + r2 + tol || dc < (r1 - r2).abs() - tol {
        return false;
    }
    let a = (r1 * r1 - r2 * r2 + dc * dc) / (2.0 * dc);
    let h = (r1 * r1 - a * a).max(0.0).sqrt();
    let m = [c1[0] + a * u[0], c1[1] + a * u[1]];
    [1.0, -1.0].into_iter().any(|sgn| {
        let z = [m[0] - sgn * h * u[1], m[1] + sgn * h * u[0]];
        foreign(z) && on1(z) && on2(z)
    })
}

/// Angular length of the intersection of two CCW intervals on a circle.
fn span_overlap(s1: f64, w1: f64, s2: f64, w2: f64) -> f64 {
    let d = (s2 - s1).rem_euclid(TAU);
    let clip = |lo: f64, hi: f64| (hi.min(w1) - lo.max(0.0)).max(0.0);
    clip(d, (d + w2).min(TAU)) + clip(0.0, d + w2 - TAU)
}
