//! Profile regions: one outer loop with inner loops (holes), extracted from a
//! sketch whose non-construction geometry forms several closed loops.
//!
//! Each connected group of entities (and each circle) must form one closed loop
//! on its own (the single-loop rules of [`crate::profile`] apply per loop). Loops
//! may not touch or cross each other (exact tests at the solve tolerance). The
//! nesting depth of a loop is the number of loops containing it: exactly one loop
//! may have depth 0 (the outer boundary) and every other loop must have depth 1
//! (a hole directly inside it). Islands inside holes and several separate outer
//! loops are refused (one region per extrusion; no region picking yet).

use crate::profile::{Prim, arc_arc_hits, line_arc_hits, segments_touch};
use crate::{Constraint, Entity, EntityId, Profile, Sketch, SketchError, solve_tolerance};
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

pub(crate) fn extract_region(s: &Sketch) -> Result<Region, SketchError> {
    // `groups` indexes points by id: dangling references must be refused first.
    s.validate()?;
    let groups = groups(s);
    if groups.len() <= 1 {
        let (outer, outer_entities) = crate::profile::extract_with_entities(s)?;
        return Ok(Region {
            outer,
            outer_entities,
            holes: vec![],
        });
    }
    let tol = solve_tolerance(s.scale());
    // Extract each loop on its own: every other entity is masked as construction.
    let mut loops = Vec::with_capacity(groups.len());
    for g in &groups {
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
        loops.push((p, ents));
    }
    let geo: Vec<Vec<Prim>> = loops.iter().map(|(p, _)| prims(p)).collect();
    for i in 0..geo.len() {
        for j in (i + 1)..geo.len() {
            if geo[i]
                .iter()
                .any(|x| geo[j].iter().any(|y| touch(x, y, tol)))
            {
                return Err(SketchError::UnsupportedProfile(format!(
                    "loops {i} and {j} touch or cross; loops of one profile must be disjoint"
                )));
            }
        }
    }
    let depth: Vec<usize> = (0..geo.len())
        .map(|i| {
            let x = boundary_point(&geo[i]);
            (0..geo.len())
                .filter(|&j| j != i && inside(x, &geo[j]))
                .count()
        })
        .collect();
    let outers: Vec<usize> = (0..geo.len()).filter(|&i| depth[i] == 0).collect();
    if outers.len() != 1 {
        return Err(SketchError::UnsupportedProfile(format!(
            "{} separate outer loops; one profile region per extrusion (extrude them separately)",
            outers.len()
        )));
    }
    if let Some(i) = (0..geo.len()).find(|&i| depth[i] > 1) {
        return Err(SketchError::UnsupportedProfile(format!(
            "loop {i} is an island inside a hole (nesting depth {}); only one level of holes \
             is supported",
            depth[i]
        )));
    }
    let o = outers[0];
    let mut holes = Vec::new();
    let mut outer = None;
    for (i, l) in loops.into_iter().enumerate() {
        if i == o {
            outer = Some(l);
        } else {
            holes.push(l);
        }
    }
    let (outer, outer_entities) = outer.expect("one outer loop");
    Ok(Region {
        outer,
        outer_entities,
        holes,
    })
}
