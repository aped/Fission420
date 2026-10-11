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

use crate::profile::{Prim, arc_arc_hits, dist, line_arc_hits, segments_touch};
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
            let at = |t: f64| [center[0] + radius * t.cos(), center[1] + radius * t.sin()];
            [0.0, PI]
                .into_iter()
                .map(|a0| Prim::Arc {
                    p: at(a0),
                    q: at(a0 + PI),
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
/// not touch). Parity of the chord polygon (horizontal ray, half-open rule at
/// vertices), toggled once for every arc whose circular segment (the region
/// between the arc and its chord) contains `x`: a simple loop's region is its
/// chord polygon with each segment added or removed, so the parities combine.
fn inside(x: [f64; 2], g: &[Prim]) -> bool {
    let mut odd = false;
    for p in g {
        let (a, b) = match *p {
            Prim::Seg { a, b } => (a, b),
            Prim::Arc { p, q, .. } => (p, q),
        };
        if (a[1] > x[1]) != (b[1] > x[1]) {
            let t = (x[1] - a[1]) / (b[1] - a[1]);
            if a[0] + t * (b[0] - a[0]) > x[0] {
                odd = !odd;
            }
        }
        if let Prim::Arc {
            p,
            q,
            c,
            r,
            a0,
            sweep,
            ..
        } = *p
        {
            let t = a0 + sweep / 2.0;
            let mid = [c[0] + r * t.cos(), c[1] + r * t.sin()];
            let side = |z: [f64; 2]| (q[0] - p[0]) * (z[1] - p[1]) - (q[1] - p[1]) * (z[0] - p[0]);
            if dist(x, c) < r && side(x) * side(mid) > 0.0 {
                odd = !odd;
            }
        }
    }
    odd
}

pub(crate) fn extract_region(s: &Sketch) -> Result<Region, SketchError> {
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
