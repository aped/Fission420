//! Planar arrangement of the sketch's curves: regions from curves that cross,
//! touch or meet at T-junctions (as Fusion offers them), when the loops are
//! not disjoint simple loops (the ordinary path in `region.rs`).
//!
//! Lines, arcs and circles are split at their intersections (exact analytic
//! line/line, line/circle and circle/circle points), dangling pieces are
//! dropped, and the bounded faces of the resulting planar graph are traced
//! (half-edges sorted by direction around each vertex, face on the left).
//! Every face is a region; the outer boundaries of components that lie
//! inside a face are its holes. The stored geometry is not changed: pieces
//! keep their entity ids and the region's loops are exact lines and arcs.
//!
//! Identity: a face's [`RegionRef`] lists its boundary entities (sorted) and,
//! in `outside`, the entities the face lies to the right of (a line traversed
//! against its a→b direction, a circle or arc clockwise), which separates
//! faces bounded by the same entities (the lens and the two crescents of two
//! overlapping circles).
//!
//! Ellipses, splines and text are refused by name in a sketch that needs the
//! arrangement; overlapping (collinear or concentric equal) curves too.

use crate::profile::{LoopSegment, Prim};
use crate::region::{Region, RegionRef, SketchRegion, inside_loop, profile_prims};
use crate::{Entity, EntityId, Profile, Sketch, SketchError};
use std::f64::consts::{PI, TAU};

#[derive(Clone, Copy, Debug)]
enum Curve {
    Line {
        e: EntityId,
        a: [f64; 2],
        b: [f64; 2],
    },
    /// CCW from angle `a0` sweeping `sweep` (2π for a circle).
    Round {
        e: EntityId,
        c: [f64; 2],
        r: f64,
        a0: f64,
        sweep: f64,
    },
}

impl Curve {
    fn entity(&self) -> EntityId {
        match *self {
            Curve::Line { e, .. } | Curve::Round { e, .. } => e,
        }
    }
    fn full(&self) -> bool {
        matches!(*self, Curve::Round { sweep, .. } if sweep >= TAU - 1e-12)
    }
    /// Point at parameter `t` (line: 0..1, round: angle offset).
    fn at(&self, t: f64) -> [f64; 2] {
        match *self {
            Curve::Line { a, b, .. } => [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])],
            Curve::Round { c, r, a0, .. } => [c[0] + r * (a0 + t).cos(), c[1] + r * (a0 + t).sin()],
        }
    }
    /// Parameter of `p` if it lies on the curve (within `tol`).
    fn param(&self, p: [f64; 2], tol: f64) -> Option<f64> {
        match *self {
            Curve::Line { a, b, .. } => {
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let l2 = dx * dx + dy * dy;
                let t = ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2;
                let q = self.at(t);
                let et = tol / l2.sqrt();
                ((q[0] - p[0]).hypot(q[1] - p[1]) <= tol && t >= -et && t <= 1.0 + et)
                    .then_some(t.clamp(0.0, 1.0))
            }
            Curve::Round {
                c, r, a0, sweep, ..
            } => {
                if ((p[0] - c[0]).hypot(p[1] - c[1]) - r).abs() > tol {
                    return None;
                }
                let rel = ((p[1] - c[1]).atan2(p[0] - c[0]) - a0).rem_euclid(TAU);
                let ea = tol / r;
                if self.full() {
                    Some(if rel > TAU - ea { 0.0 } else { rel })
                } else if rel <= sweep + ea {
                    Some(rel.min(sweep))
                } else if rel >= TAU - ea {
                    Some(0.0)
                } else {
                    None
                }
            }
        }
    }
}

/// Intersection points of the supports (infinite line / full circle).
fn support_hits(x: &Curve, y: &Curve, tol: f64) -> Vec<[f64; 2]> {
    let line_circle = |a: [f64; 2], b: [f64; 2], c: [f64; 2], r: f64| -> Vec<[f64; 2]> {
        let d = [b[0] - a[0], b[1] - a[1]];
        let f = [a[0] - c[0], a[1] - c[1]];
        let aa = d[0] * d[0] + d[1] * d[1];
        let bb = 2.0 * (f[0] * d[0] + f[1] * d[1]);
        let cc = f[0] * f[0] + f[1] * f[1] - r * r;
        let disc = bb * bb - 4.0 * aa * cc;
        // Tangency within tolerance: one point at the foot.
        let foot = -bb / (2.0 * aa);
        let fp = [a[0] + foot * d[0], a[1] + foot * d[1]];
        let dist = (fp[0] - c[0]).hypot(fp[1] - c[1]);
        if disc < 0.0 {
            return if (dist - r).abs() <= tol {
                vec![fp]
            } else {
                vec![]
            };
        }
        let sq = disc.sqrt();
        [(-bb - sq) / (2.0 * aa), (-bb + sq) / (2.0 * aa)]
            .iter()
            .map(|t| [a[0] + t * d[0], a[1] + t * d[1]])
            .collect()
    };
    match (*x, *y) {
        (Curve::Line { a, b, .. }, Curve::Line { a: c, b: d, .. }) => {
            let r = [b[0] - a[0], b[1] - a[1]];
            let s = [d[0] - c[0], d[1] - c[1]];
            let den = r[0] * s[1] - r[1] * s[0];
            if den.abs() <= 1e-14 * r[0].hypot(r[1]) * s[0].hypot(s[1]) {
                return vec![];
            }
            let w = [c[0] - a[0], c[1] - a[1]];
            let t = (w[0] * s[1] - w[1] * s[0]) / den;
            vec![[a[0] + t * r[0], a[1] + t * r[1]]]
        }
        (Curve::Line { a, b, .. }, Curve::Round { c, r, .. })
        | (Curve::Round { c, r, .. }, Curve::Line { a, b, .. }) => line_circle(a, b, c, r),
        (Curve::Round { c: c1, r: r1, .. }, Curve::Round { c: c2, r: r2, .. }) => {
            let dx = [c2[0] - c1[0], c2[1] - c1[1]];
            let dd = dx[0].hypot(dx[1]);
            if dd <= tol || dd > r1 + r2 + tol || dd < (r1 - r2).abs() - tol {
                return vec![];
            }
            let u = [dx[0] / dd, dx[1] / dd];
            let a = (r1 * r1 - r2 * r2 + dd * dd) / (2.0 * dd);
            let h = (r1 * r1 - a * a).max(0.0).sqrt();
            let m = [c1[0] + a * u[0], c1[1] + a * u[1]];
            vec![
                [m[0] - h * u[1], m[1] + h * u[0]],
                [m[0] + h * u[1], m[1] - h * u[0]],
            ]
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Edge {
    v0: usize,
    v1: usize,
    curve: usize,
    /// Parameter range along the curve (t0 < t1), traversed v0 -> v1.
    t0: f64,
    t1: f64,
}

/// Regions of the planar arrangement of the sketch's live curves.
pub(crate) fn arrangement_regions(s: &Sketch) -> Result<Vec<SketchRegion>, SketchError> {
    let refused = |m: String| SketchError::UnsupportedProfile(m);
    if !s.texts.is_empty() {
        return Err(refused(
            "text in a sketch whose curves cross or touch is not supported (regions from \
             crossing curves support lines, arcs and circles)"
                .into(),
        ));
    }
    let mut curves = Vec::new();
    for e in s.entities.iter().filter(|e| !s.is_construction(e.id())) {
        match e {
            Entity::Line { a, b, .. } => {
                let (Some(pa), Some(pb)) = (s.point_xy(*a), s.point_xy(*b)) else {
                    return Err(SketchError::MissingPoint(*a));
                };
                curves.push(Curve::Line {
                    e: e.id(),
                    a: pa,
                    b: pb,
                });
            }
            Entity::Circle { center, radius, .. } => {
                let c = s
                    .point_xy(*center)
                    .ok_or(SketchError::MissingPoint(*center))?;
                curves.push(Curve::Round {
                    e: e.id(),
                    c,
                    r: *radius,
                    a0: 0.0,
                    sweep: TAU,
                });
            }
            Entity::Arc { id, .. } => {
                let g = s.arc_geometry(*id).ok_or(SketchError::MissingEntity(*id))?;
                curves.push(Curve::Round {
                    e: *id,
                    c: g.center,
                    r: g.radius,
                    a0: g.start_angle,
                    sweep: g.sweep,
                });
            }
            Entity::Ellipse { .. } | Entity::Spline { .. } => {
                return Err(refused(format!(
                    "regions from crossing or touching curves support lines, arcs and circles; \
                     entity {} is an ellipse or spline",
                    e.id().0
                )));
            }
        }
    }
    let scale = s.scale();
    let tol = (1e-9 * scale).max(1e-7);
    // Split parameters per curve.
    let mut params: Vec<Vec<f64>> = curves
        .iter()
        .map(|c| match c {
            Curve::Line { .. } => vec![0.0, 1.0],
            Curve::Round { sweep, .. } if c.full() => {
                let _ = sweep;
                vec![]
            }
            Curve::Round { sweep, .. } => vec![0.0, *sweep],
        })
        .collect();
    let ends = |c: &Curve| -> Vec<[f64; 2]> {
        match *c {
            Curve::Line { a, b, .. } => vec![a, b],
            Curve::Round { sweep, .. } if !c.full() => vec![c.at(0.0), c.at(sweep)],
            _ => vec![],
        }
    };
    for i in 0..curves.len() {
        for j in 0..curves.len() {
            if i == j {
                continue;
            }
            // Ends of j on i (T-junctions, touching ends).
            for p in ends(&curves[j]) {
                if let Some(t) = curves[i].param(p, tol) {
                    params[i].push(t);
                }
            }
            if j > i {
                for p in support_hits(&curves[i], &curves[j], tol) {
                    if let (Some(ti), Some(tj)) = (curves[i].param(p, tol), curves[j].param(p, tol))
                    {
                        params[i].push(ti);
                        params[j].push(tj);
                    }
                }
            }
        }
    }
    // Vertices (merged within tol) and edges.
    let mut verts: Vec<[f64; 2]> = Vec::new();
    let mut vertex = |p: [f64; 2]| -> usize {
        if let Some(i) = verts
            .iter()
            .position(|q| (q[0] - p[0]).hypot(q[1] - p[1]) <= 10.0 * tol)
        {
            return i;
        }
        verts.push(p);
        verts.len() - 1
    };
    let mut edges: Vec<Edge> = Vec::new();
    for (ci, c) in curves.iter().enumerate() {
        let mut ts = std::mem::take(&mut params[ci]);
        ts.sort_by(f64::total_cmp);
        let eps = match *c {
            Curve::Line { a, b, .. } => tol / (b[0] - a[0]).hypot(b[1] - a[1]),
            Curve::Round { r, .. } => tol / r,
        };
        ts.dedup_by(|x, y| (*x - *y).abs() <= 10.0 * eps);
        if c.full() {
            if ts.len() > 1 && ts[0] + TAU - ts[ts.len() - 1] <= 10.0 * eps {
                ts.pop();
            }
            // A circle needs two vertices to be two edges.
            match ts.len() {
                0 => ts = vec![0.0, PI],
                1 => {
                    let o = (ts[0] + PI).rem_euclid(TAU);
                    ts.push(o);
                    ts.sort_by(f64::total_cmp);
                }
                _ => {}
            }
            for k in 0..ts.len() {
                let (t0, t1) = (
                    ts[k],
                    if k + 1 < ts.len() {
                        ts[k + 1]
                    } else {
                        ts[0] + TAU
                    },
                );
                let (v0, v1) = (vertex(c.at(t0)), vertex(c.at(t1)));
                edges.push(Edge {
                    v0,
                    v1,
                    curve: ci,
                    t0,
                    t1,
                });
            }
        } else {
            for w in ts.windows(2) {
                let (v0, v1) = (vertex(c.at(w[0])), vertex(c.at(w[1])));
                if v0 != v1 {
                    edges.push(Edge {
                        v0,
                        v1,
                        curve: ci,
                        t0: w[0],
                        t1: w[1],
                    });
                }
            }
        }
    }
    let mid = |e: &Edge| curves[e.curve].at(0.5 * (e.t0 + e.t1));
    // Overlapping curves give two edges on the same points.
    for i in 0..edges.len() {
        for j in (i + 1)..edges.len() {
            let (a, b) = (&edges[i], &edges[j]);
            let same_ends = (a.v0 == b.v0 && a.v1 == b.v1) || (a.v0 == b.v1 && a.v1 == b.v0);
            let (ma, mb) = (mid(a), mid(b));
            if same_ends && (ma[0] - mb[0]).hypot(ma[1] - mb[1]) <= 10.0 * tol {
                return Err(refused(format!(
                    "entities {} and {} overlap; regions need curves that cross or touch, not \
                     coincide",
                    curves[a.curve].entity().0,
                    curves[b.curve].entity().0
                )));
            }
        }
    }
    // Drop dangling pieces (and loops of a single edge).
    loop {
        let mut deg = vec![0usize; verts.len()];
        for e in &edges {
            deg[e.v0] += 1;
            deg[e.v1] += 1;
        }
        let n = edges.len();
        edges.retain(|e| deg[e.v0] > 1 && deg[e.v1] > 1);
        if edges.len() == n {
            break;
        }
    }
    if edges.is_empty() {
        return Err(refused(
            "no closed region: the curves form no closed loop".into(),
        ));
    }
    // Half-edges: 2k forward (v0 -> v1), 2k + 1 backward.
    let half = |h: usize| -> (usize, usize) {
        let e = &edges[h / 2];
        if h.is_multiple_of(2) {
            (e.v0, e.v1)
        } else {
            (e.v1, e.v0)
        }
    };
    // Direction key: angle of the secant to a point a little along the edge.
    let key = |h: usize| -> f64 {
        let e = &edges[h / 2];
        let c = &curves[e.curve];
        let f = 1e-4;
        let t = if h.is_multiple_of(2) {
            e.t0 + f * (e.t1 - e.t0)
        } else {
            e.t1 - f * (e.t1 - e.t0)
        };
        let (o, _) = half(h);
        let p = c.at(t);
        (p[1] - verts[o][1]).atan2(p[0] - verts[o][0])
    };
    let mut out_of: Vec<Vec<usize>> = vec![vec![]; verts.len()];
    for h in 0..2 * edges.len() {
        out_of[half(h).0].push(h);
    }
    for v in &mut out_of {
        v.sort_by(|a, b| key(*a).total_cmp(&key(*b)));
    }
    let next = |h: usize| -> usize {
        let (_, v) = half(h);
        let twin = h ^ 1;
        let ring = &out_of[v];
        let k = ring.iter().position(|&x| x == twin).unwrap_or(0);
        ring[(k + ring.len() - 1) % ring.len()]
    };
    // Trace cycles.
    let mut seen = vec![false; 2 * edges.len()];
    let mut cycles: Vec<Vec<usize>> = Vec::new();
    for h0 in 0..2 * edges.len() {
        if seen[h0] {
            continue;
        }
        let mut cyc = Vec::new();
        let mut h = h0;
        while !seen[h] {
            seen[h] = true;
            cyc.push(h);
            h = next(h);
            if cyc.len() > 2 * edges.len() {
                return Err(refused("region tracing did not close".into()));
            }
        }
        cycles.push(cyc);
    }
    // Segments of a cycle (merging consecutive pieces of one entity).
    // Signed area of a cycle from its unmerged pieces (exact).
    let area_of = |cyc: &[usize]| -> f64 {
        let raw: Vec<LoopSegment> = cyc
            .iter()
            .map(|&h| {
                let start = verts[half(h).0];
                match curves[edges[h / 2].curve] {
                    Curve::Line { .. } => LoopSegment::Line { start },
                    Curve::Round { c, .. } => LoopSegment::Arc {
                        start,
                        center: c,
                        ccw: h % 2 == 0,
                    },
                }
            })
            .collect();
        crate::profile::loop_area(&raw)
    };
    let segs_of = |cyc: &[usize]| -> (Vec<LoopSegment>, Vec<EntityId>, Vec<EntityId>) {
        let mut segs: Vec<(LoopSegment, EntityId, usize)> = Vec::new();
        for &h in cyc {
            let e = &edges[h / 2];
            let c = &curves[e.curve];
            let start = verts[half(h).0];
            let seg = match *c {
                Curve::Line { .. } => LoopSegment::Line { start },
                Curve::Round { c, .. } => LoopSegment::Arc {
                    start,
                    center: c,
                    ccw: h % 2 == 0,
                },
            };
            if let Some((_, pe, pc)) = segs.last()
                && *pe == c.entity()
                && *pc == h % 2
            {
                continue; // same entity, same direction: one segment
            }
            segs.push((seg, c.entity(), h % 2));
        }
        // The last piece may continue the first.
        while segs.len() > 1 {
            let (f, l) = (&segs[0], &segs[segs.len() - 1]);
            if f.1 == l.1 && f.2 == l.2 {
                let last = segs.pop().expect("non-empty");
                segs[0].0 = last.0;
            } else {
                break;
            }
        }
        let mut outside: Vec<EntityId> = segs
            .iter()
            .filter(|(_, _, back)| *back == 1)
            .map(|(_, e, _)| *e)
            .collect();
        outside.sort();
        outside.dedup();
        (
            segs.iter().map(|(s, _, _)| s.clone()).collect(),
            segs.iter().map(|(_, e, _)| *e).collect(),
            outside,
        )
    };
    let profile_of = |segs: Vec<LoopSegment>, ents: Vec<EntityId>| -> (Profile, Vec<EntityId>) {
        if segs.len() == 1
            && let LoopSegment::Arc { start, center, .. } = segs[0]
        {
            let r = (start[0] - center[0]).hypot(start[1] - center[1]);
            return (Profile::Circle { center, radius: r }, ents);
        }
        // Start at the lowest, then leftmost vertex.
        let k = (0..segs.len())
            .min_by(|&i, &j| {
                let (a, b) = (segs[i].start(), segs[j].start());
                (a[1], a[0])
                    .partial_cmp(&(b[1], b[0]))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(0);
        let mut segs = segs;
        let mut ents = ents;
        segs.rotate_left(k);
        ents.rotate_left(k);
        if segs.iter().all(|s| matches!(s, LoopSegment::Line { .. })) {
            (
                Profile::Polygon(segs.iter().map(LoopSegment::start).collect()),
                ents,
            )
        } else {
            (Profile::Loop(segs), ents)
        }
    };
    // Components (vertex union-find through edges).
    let mut comp: Vec<usize> = (0..verts.len()).collect();
    fn root(c: &mut [usize], mut i: usize) -> usize {
        while c[i] != i {
            c[i] = c[c[i]];
            i = c[i];
        }
        i
    }
    for e in &edges {
        let (a, b) = (root(&mut comp, e.v0), root(&mut comp, e.v1));
        comp[a] = b;
    }
    struct Face {
        outer: (Profile, Vec<EntityId>),
        outside: Vec<EntityId>,
        area: f64,
        comp: usize,
        holes: Vec<(Profile, Vec<EntityId>)>,
        hole_outside: Vec<EntityId>,
        hole_area: f64,
    }
    let mut faces: Vec<Face> = Vec::new();
    let mut bounds: Vec<(Vec<usize>, usize)> = Vec::new();
    for cyc in &cycles {
        let (segs, ents, outside) = segs_of(cyc);
        let area = area_of(cyc);
        let c = root(&mut comp, half(cyc[0]).0);
        if area > 0.0 {
            faces.push(Face {
                outer: profile_of(segs, ents),
                outside,
                area,
                comp: c,
                holes: vec![],
                hole_outside: vec![],
                hole_area: 0.0,
            });
        } else {
            bounds.push((cyc.clone(), c));
        }
    }
    // Each component's outer boundary is a hole of the smallest face of
    // another component that contains it.
    for (cyc, c) in bounds {
        let p = verts[half(cyc[0]).0];
        let host = (0..faces.len())
            .filter(|&f| faces[f].comp != c)
            .filter(|&f| inside_loop(p, &profile_prims(&faces[f].outer.0)))
            .min_by(|&a, &b| faces[a].area.total_cmp(&faces[b].area));
        let Some(f) = host else { continue };
        // The hole loop, counter-clockwise: the twins in reverse order.
        let rev: Vec<usize> = cyc.iter().rev().map(|h| h ^ 1).collect();
        let (segs, ents, _) = segs_of(&rev);
        let (_, _, outside) = segs_of(&cyc);
        let a = area_of(&rev);
        faces[f].holes.push(profile_of(segs, ents));
        faces[f].hole_outside.extend(outside);
        faces[f].hole_area += a.abs();
    }
    let mut out = Vec::with_capacity(faces.len());
    for f in faces {
        let hole_ids: Vec<&[EntityId]> = f.holes.iter().map(|(_, e)| e.as_slice()).collect();
        let mut id = RegionRef::of_parts(&f.outer.1, &hole_ids);
        let mut outside = f.outside;
        outside.extend(f.hole_outside);
        outside.sort();
        outside.dedup();
        id.outside = outside;
        out.push(SketchRegion {
            id,
            region: Region {
                outer: f.outer.0,
                outer_entities: f.outer.1,
                holes: f.holes,
            },
            depth: 0,
            area: f.area - f.hole_area,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    // Faces bounded by the same entities on the same sides are ambiguous:
    // refused rather than told apart by position.
    for w in out.windows(2) {
        if w[0].id == w[1].id {
            return Err(refused(format!(
                "two regions are bounded by the same curves ({}); add or split a curve to tell \
                 them apart",
                w[0].id.label()
            )));
        }
    }
    let _ = Prim::of;
    Ok(out)
}
