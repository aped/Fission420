//! Profile regions: one outer loop with inner loops (holes), extracted from a
//! sketch whose non-construction geometry forms several closed loops.
//!
//! Each connected group of entities (and each circle) must form one closed loop
//! on its own (the single-loop rules of [`crate::profile`] apply per loop). Loops
//! may not touch or cross each other (exact tests at the solve tolerance). The
//! nesting depth of a loop is the number of loops containing it: exactly one loop
//! may have depth 0 (the outer boundary) and every other loop must have depth 1
//! (a hole directly inside it). Islands inside holes and several separate outer
//! loops are refused by [`Sketch::profile_region`] (the default whole-sketch
//! profile); such sketches are extruded by picking one region:
//! [`Sketch::regions`] lists every bounded region (each loop minus the loops
//! directly inside it, any nesting depth) with a persistent [`RegionRef`], and
//! [`Sketch::region_at`] finds the region under a point.

use crate::profile::{Prim, arc_arc_hits, line_arc_hits, segments_touch};
use crate::{Constraint, Entity, EntityId, Profile, Sketch, SketchError, solve_tolerance};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::f64::consts::PI;

/// An outer loop and its holes, each with the sketch entities generating its
/// segments (see [`Sketch::profile_with_entities`]). All loops are
/// counter-clockwise.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub outer: Profile,
    pub outer_entities: Vec<EntityId>,
    pub holes: Vec<(Profile, Vec<EntityId>)>,
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Entity groups (connected through shared or coincident points); circles alone.
fn groups(s: &Sketch) -> Vec<Vec<EntityId>> {
    let pidx: HashMap<_, _> = s
        .points
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id, i))
        .collect();
    let mut parent: Vec<usize> = (0..s.points.len()).collect();
    let join = |parent: &mut Vec<usize>, a: usize, b: usize| {
        let (ra, rb) = (find(parent, a), find(parent, b));
        parent[ra] = rb;
    };
    for c in &s.constraints {
        if let Constraint::Coincident { a, b } = c
            && let (Some(&ia), Some(&ib)) = (pidx.get(a), pidx.get(b))
        {
            join(&mut parent, ia, ib);
        }
    }
    let live: Vec<&Entity> = s
        .entities
        .iter()
        .filter(|e| !s.is_construction(e.id()))
        .collect();
    for e in &live {
        let pts: Vec<usize> = match e {
            Entity::Line { a, b, .. } => vec![pidx[a], pidx[b]],
            Entity::Arc { start, end, .. } => vec![pidx[start], pidx[end]],
            Entity::Circle { .. } => vec![],
        };
        for w in pts.windows(2) {
            join(&mut parent, w[0], w[1]);
        }
    }
    let mut by_root: BTreeMap<usize, Vec<EntityId>> = BTreeMap::new();
    let mut out = Vec::new();
    for e in live {
        match e {
            Entity::Line { a, .. } | Entity::Arc { start: a, .. } => {
                let r = find(&mut parent, pidx[a]);
                by_root.entry(r).or_default().push(e.id());
            }
            Entity::Circle { id, .. } => out.push(vec![*id]),
        }
    }
    out.extend(by_root.into_values());
    out.sort();
    out
}

/// Boundary primitives of a loop (a circle as two half arcs).
fn prims(p: &Profile) -> Vec<Prim> {
    match p {
        Profile::Polygon(v) => (0..v.len())
            .map(|i| Prim::Seg {
                a: v[i],
                b: v[(i + 1) % v.len()],
            })
            .collect(),
        Profile::Loop(segs) => (0..segs.len())
            .map(|i| Prim::of(&segs[i], segs[(i + 1) % segs.len()].start()))
            .collect(),
        Profile::Circle { center, radius } => {
            // Exact, shared end points on y = center.y (no trigonometric noise),
            // so the half-open crossing rule in `inside` sees one vertex each.
            let right = [center[0] + radius, center[1]];
            let left = [center[0] - radius, center[1]];
            [(0.0, right, left), (PI, left, right)]
                .into_iter()
                .map(|(a0, p, q)| Prim::Arc {
                    p,
                    q,
                    c: *center,
                    r: *radius,
                    ccw: true,
                    a0,
                    sweep: PI,
                })
                .collect()
        }
    }
}

fn touch(x: &Prim, y: &Prim, tol: f64) -> bool {
    match (*x, *y) {
        (Prim::Seg { a, b }, Prim::Seg { a: c, b: d }) => segments_touch(a, b, c, d, tol),
        (Prim::Seg { a, b }, arc @ Prim::Arc { .. })
        | (arc @ Prim::Arc { .. }, Prim::Seg { a, b }) => line_arc_hits(a, b, &arc, &[], tol),
        (u @ Prim::Arc { .. }, v @ Prim::Arc { .. }) => arc_arc_hits(&u, &v, &[], tol),
    }
}

/// A point on the loop boundary (midpoint of its first primitive).
fn boundary_point(g: &[Prim]) -> [f64; 2] {
    match g[0] {
        Prim::Seg { a, b } => [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0],
        Prim::Arc {
            c, r, a0, sweep, ..
        } => {
            let t = a0 + sweep / 2.0;
            [c[0] + r * t.cos(), c[1] + r * t.sin()]
        }
    }
}

/// Is `x` inside the closed boundary `g`? `x` is not on the boundary (loops do
/// not touch). Parity of the crossings of the rightward horizontal ray from `x`
/// with the real boundary curves (not arc chords). Each primitive is split into
/// pieces monotone in y (an arc at its top and bottom points); a piece crosses
/// the ray once when its ends lie on different sides under the half-open rule
/// (`y > x.y` versus `y <= x.y`), and counts when that crossing lies right of
/// `x`. Neighbouring primitives share identical vertex coordinates, so a ray
/// through a vertex, along a horizontal edge or tangent to an arc is counted
/// consistently.
fn inside(x: [f64; 2], g: &[Prim]) -> bool {
    let mut odd = false;
    for prim in g {
        match *prim {
            Prim::Seg { a, b } => {
                if (a[1] > x[1]) != (b[1] > x[1]) {
                    let t = (x[1] - a[1]) / (b[1] - a[1]);
                    if a[0] + t * (b[0] - a[0]) > x[0] {
                        odd = !odd;
                    }
                }
            }
            Prim::Arc {
                p,
                q,
                c,
                r,
                ccw,
                a0,
                sweep,
            } => {
                // Ends in CCW order: the point set runs from `a0` over `sweep`.
                let (from, to) = if ccw { (p, q) } else { (q, p) };
                // Cut at the y extrema (angles pi/2 + k pi) strictly inside.
                let mut cuts = vec![(a0, from)];
                let mut k = ((a0 - PI / 2.0) / PI).floor() as i64;
                loop {
                    let t = PI / 2.0 + k as f64 * PI;
                    if t >= a0 + sweep {
                        break;
                    }
                    if t > a0 {
                        let y = if k.rem_euclid(2) == 0 {
                            c[1] + r
                        } else {
                            c[1] - r
                        };
                        cuts.push((t, [c[0], y]));
                    }
                    k += 1;
                }
                cuts.push((a0 + sweep, to));
                for w in cuts.windows(2) {
                    let ((t0, u), (t1, v)) = (w[0], w[1]);
                    if (u[1] > x[1]) == (v[1] > x[1]) {
                        continue;
                    }
                    // The piece lies on the right half of the circle when its
                    // middle angle has a non-negative cosine.
                    let dy = x[1] - c[1];
                    let half = (r * r - dy * dy).max(0.0).sqrt();
                    let hit = if ((t0 + t1) / 2.0).cos() >= 0.0 {
                        c[0] + half
                    } else {
                        c[0] - half
                    };
                    if hit > x[0] {
                        odd = !odd;
                    }
                }
            }
        }
    }
    odd
}

/// Axis-aligned box `[xmin, ymin, xmax, ymax]` of a loop's primitives (arcs by
/// their full circle), widened by `pad`.
fn bbox(g: &[Prim], pad: f64) -> [f64; 4] {
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    let mut add = |p: [f64; 2], r: f64| {
        b[0] = b[0].min(p[0] - r);
        b[1] = b[1].min(p[1] - r);
        b[2] = b[2].max(p[0] + r);
        b[3] = b[3].max(p[1] + r);
    };
    for x in g {
        match *x {
            Prim::Seg { a, b } => {
                add(a, 0.0);
                add(b, 0.0);
            }
            Prim::Arc { c, r, .. } => add(c, r),
        }
    }
    [b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad]
}

/// The closed loops of a sketch (each with its generating entities), their
/// boundary primitives, and the containing loop of each (`parent`: the
/// smallest loop containing it, `None` at depth 0). Loops must be disjoint.
struct Loops {
    loops: Vec<(Profile, Vec<EntityId>)>,
    geo: Vec<Vec<Prim>>,
    depth: Vec<usize>,
    parent: Vec<Option<usize>>,
}

fn analyse(s: &Sketch, groups: &[Vec<EntityId>]) -> Result<Loops, SketchError> {
    let tol = solve_tolerance(s.scale());
    // Extract each loop on its own: every other entity is masked as construction.
    let mut loops = Vec::with_capacity(groups.len());
    let mut names: Vec<String> = Vec::with_capacity(groups.len());
    for g in groups {
        let mut sub = s.clone();
        sub.construction = s
            .entities
            .iter()
            .map(Entity::id)
            .filter(|id| !g.contains(id))
            .collect();
        let (p, ents) = crate::profile::extract_with_entities(&sub).map_err(|e| {
            SketchError::UnsupportedProfile(format!(
                "loop of entities {:?}: {e}",
                g.iter().map(|i| i.0).collect::<Vec<_>>()
            ))
        })?;
        names.push(format!(
            "loop {} (entities {:?})",
            loops.len(),
            g.iter().map(|i| i.0).collect::<Vec<_>>()
        ));
        loops.push((p, ents));
    }
    // Text glyph contours (run 6): closed polygons with synthetic edge ids.
    for t in s.text_loops()? {
        names.push(format!(
            "text {} glyph '{}' (character {}) contour {}",
            t.text,
            t.ch,
            t.glyph + 1,
            t.contour
        ));
        loops.push((Profile::Polygon(t.polygon), t.edges));
    }
    let geo: Vec<Vec<Prim>> = loops.iter().map(|(p, _)| prims(p)).collect();
    // Bounding boxes (widened by tol): disjoint boxes cannot touch, and a loop
    // can only contain loops whose box lies inside its own.
    let boxes: Vec<[f64; 4]> = geo.iter().map(|g| bbox(g, tol)).collect();
    for i in 0..geo.len() {
        for j in (i + 1)..geo.len() {
            let (a, b) = (boxes[i], boxes[j]);
            if a[0] > b[2] || b[0] > a[2] || a[1] > b[3] || b[1] > a[3] {
                continue;
            }
            if geo[i]
                .iter()
                .any(|x| geo[j].iter().any(|y| touch(x, y, tol)))
            {
                return Err(SketchError::UnsupportedProfile(format!(
                    "{} and {} touch or cross; loops of one profile must be disjoint",
                    names[i], names[j]
                )));
            }
        }
    }
    // containers[i]: loops containing loop i (disjoint loops nest strictly).
    let containers: Vec<Vec<usize>> = (0..geo.len())
        .map(|i| {
            let x = boundary_point(&geo[i]);
            (0..geo.len())
                .filter(|&j| {
                    let (a, b) = (boxes[j], boxes[i]);
                    j != i
                        && a[0] <= b[0]
                        && a[1] <= b[1]
                        && a[2] >= b[2]
                        && a[3] >= b[3]
                        && inside(x, &geo[j])
                })
                .collect()
        })
        .collect();
    let depth: Vec<usize> = containers.iter().map(Vec::len).collect();
    let parent = containers
        .iter()
        .enumerate()
        .map(|(i, c)| c.iter().copied().find(|&j| depth[j] + 1 == depth[i]))
        .collect();
    Ok(Loops {
        loops,
        geo,
        depth,
        parent,
    })
}

/// Persistent identity of a bounded sketch region: the sorted entity ids of its
/// outer loop and of each of its direct inner loops (holes), holes sorted.
/// Entity ids are stable across dimension edits, so the same region keeps the
/// same identity; it changes (and a reference to it breaks) when a bounding loop
/// gains, loses or replaces an entity, or a hole is added or removed. Never an
/// index into a list.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RegionRef {
    pub outer: Vec<EntityId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holes: Vec<Vec<EntityId>>,
}

impl RegionRef {
    fn of(outer: &[EntityId], holes: &[&[EntityId]]) -> Self {
        // Text loops reduce to their contour id (the edge count is not identity).
        let sorted = |v: &[EntityId]| {
            let mut v: Vec<EntityId> = v.iter().map(|&e| crate::text::text_loop_key(e)).collect();
            v.sort();
            v.dedup();
            v
        };
        let mut holes: Vec<Vec<EntityId>> = holes.iter().map(|h| sorted(h)).collect();
        holes.sort();
        RegionRef {
            outer: sorted(outer),
            holes,
        }
    }

    /// Short label, e.g. `E1-4 − E5-8` (entity ids of the outer loop, minus holes).
    pub fn label(&self) -> String {
        let ids = |v: &[EntityId]| {
            if let Some((t, g, c, _)) = v.first().and_then(|e| crate::text::text_edge_parts(*e)) {
                return format!("T{t}.{}.{c}", g + 1);
            }
            let n: Vec<u32> = v.iter().map(|e| e.0).collect();
            let contiguous = n.windows(2).all(|w| w[1] == w[0] + 1);
            match (n.first(), n.last()) {
                (Some(a), Some(b)) if contiguous && n.len() > 2 => format!("E{a}-{b}"),
                _ => n
                    .iter()
                    .map(|i| format!("E{i}"))
                    .collect::<Vec<_>>()
                    .join(","),
            }
        };
        let mut s = ids(&self.outer);
        for h in &self.holes {
            s.push_str(" − ");
            s.push_str(&ids(h));
        }
        s
    }
}

/// One bounded region of a sketch (see [`Sketch::regions`]).
#[derive(Clone, Debug, PartialEq)]
pub struct SketchRegion {
    pub id: RegionRef,
    /// Outer loop and direct holes with their generating entities.
    pub region: Region,
    /// Number of loops containing the region's outer loop (0 = outermost).
    pub depth: usize,
    /// Region area (outer loop area minus hole areas), mm².
    pub area: f64,
}

/// Enclosed area of one loop (positive).
fn loop_area(p: &Profile) -> f64 {
    crate::profile::signed_area(&prims(p)).abs()
}

/// Every bounded region: each closed loop minus the loops directly inside it.
/// Loops may nest to any depth and there may be several outermost loops. Sorted
/// by (depth, identity), so the order is deterministic but carries no identity.
pub(crate) fn regions(s: &Sketch) -> Result<Vec<SketchRegion>, SketchError> {
    s.validate()?;
    let groups = groups(s);
    if groups.is_empty() && s.texts.is_empty() {
        // Same diagnostic as the single-profile extraction.
        crate::profile::extract_with_entities(s)?;
    }
    let l = analyse(s, &groups)?;
    let mut out = Vec::with_capacity(l.loops.len());
    for i in 0..l.loops.len() {
        let kids: Vec<usize> = (0..l.loops.len())
            .filter(|&j| l.parent[j] == Some(i))
            .collect();
        let holes: Vec<(Profile, Vec<EntityId>)> =
            kids.iter().map(|&j| l.loops[j].clone()).collect();
        let hole_ids: Vec<&[EntityId]> = holes.iter().map(|(_, e)| e.as_slice()).collect();
        let (outer, outer_entities) = l.loops[i].clone();
        let area = loop_area(&outer) - holes.iter().map(|(p, _)| loop_area(p)).sum::<f64>();
        out.push(SketchRegion {
            id: RegionRef::of(&outer_entities, &hole_ids),
            region: Region {
                outer,
                outer_entities,
                holes,
            },
            depth: l.depth[i],
            area,
        });
    }
    out.sort_by(|a, b| (a.depth, &a.id).cmp(&(b.depth, &b.id)));
    Ok(out)
}

/// The region `id`, or `None` when no region of the current sketch has it.
pub(crate) fn region_by_ref(s: &Sketch, id: &RegionRef) -> Result<Option<Region>, SketchError> {
    Ok(regions(s)?
        .into_iter()
        .find(|r| r.id == *id)
        .map(|r| r.region))
}

/// The region containing `p` (inside its outer loop, outside its holes);
/// `None` outside every loop or exactly on a boundary (within tolerance).
pub(crate) fn region_at(s: &Sketch, p: [f64; 2]) -> Result<Option<RegionRef>, SketchError> {
    let rs = regions(s)?;
    let tol = solve_tolerance(s.scale());
    let on_boundary = |g: &[Prim]| g.iter().any(|x| prim_distance(x, p) <= tol);
    let mut best: Option<&SketchRegion> = None;
    for r in &rs {
        let og = prims(&r.region.outer);
        if on_boundary(&og) || !inside(p, &og) {
            continue;
        }
        let in_hole = r.region.holes.iter().any(|(h, _)| {
            let hg = prims(h);
            on_boundary(&hg) || inside(p, &hg)
        });
        if !in_hole && best.is_none_or(|b| r.depth > b.depth) {
            best = Some(r);
        }
    }
    Ok(best.map(|r| r.id.clone()))
}

fn prim_distance(x: &Prim, p: [f64; 2]) -> f64 {
    match *x {
        Prim::Seg { a, b } => {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l2 = dx * dx + dy * dy;
            let t = if l2 > 0.0 {
                (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
        }
        Prim::Arc {
            p: s,
            q,
            c,
            r,
            a0,
            sweep,
            ..
        } => {
            // `a0..a0 + sweep` is the arc's point set (CCW) whatever `ccw` says.
            let ang = (p[1] - c[1]).atan2(p[0] - c[0]);
            let rel = (ang - a0).rem_euclid(std::f64::consts::TAU);
            if rel <= sweep {
                ((p[0] - c[0]).hypot(p[1] - c[1]) - r).abs()
            } else {
                let d = |e: [f64; 2]| (p[0] - e[0]).hypot(p[1] - e[1]);
                d(s).min(d(q))
            }
        }
    }
}

pub(crate) fn extract_region(s: &Sketch) -> Result<Region, SketchError> {
    // `groups` indexes points by id: dangling references must be refused first.
    s.validate()?;
    let groups = groups(s);
    if groups.len() <= 1 && s.texts.is_empty() {
        let (outer, outer_entities) = crate::profile::extract_with_entities(s)?;
        return Ok(Region {
            outer,
            outer_entities,
            holes: vec![],
        });
    }
    let l = analyse(s, &groups)?;
    if l.loops.is_empty() {
        return Err(SketchError::UnsupportedProfile(
            "no closed loops (only text without outlines)".into(),
        ));
    }
    let depth = &l.depth;
    let outers: Vec<usize> = (0..l.geo.len()).filter(|&i| depth[i] == 0).collect();
    if outers.len() != 1 {
        return Err(SketchError::UnsupportedProfile(format!(
            "{} separate outer loops; pick one region to extrude (or extrude them separately)",
            outers.len()
        )));
    }
    if let Some(i) = (0..l.geo.len()).find(|&i| depth[i] > 1) {
        return Err(SketchError::UnsupportedProfile(format!(
            "loop {i} is an island inside a hole (nesting depth {}); pick one region to extrude",
            depth[i]
        )));
    }
    let o = outers[0];
    let mut holes = Vec::new();
    let mut outer = None;
    for (i, lp) in l.loops.into_iter().enumerate() {
        if i == o {
            outer = Some(lp);
        } else {
            holes.push(lp);
        }
    }
    let (outer, outer_entities) = outer.expect("one outer loop");
    Ok(Region {
        outer,
        outer_entities,
        holes,
    })
}
