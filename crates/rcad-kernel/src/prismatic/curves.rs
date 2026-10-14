//! Exact line/arc loops for the slab construction and their deterministic
//! polygonization (ADR 0007).
//!
//! The 2-D region booleans run on polygons (`i_overlay`, integer grid). Arcs
//! are polygonized on a *ring* per circle shared by every loop of one
//! construction, so coincident arcs give identical chains and merge exactly.
//! Every point where a circle meets another curve (arc endpoints, exact
//! line–circle and circle–circle intersections: *special* points) is a ring
//! vertex and splits the lines through it, so the polygon boolean meets a
//! chain only at those shared vertices. The polygons then only decide the
//! topology: the output segments are tagged back to their source curve (two
//! ring-adjacent vertices of one circle: an arc of it; anything else: a line)
//! and rebuilt as exact lines and arcs.
//!
//! Preconditions, checked (a failure is a refusal naming the reason, or a
//! retry on a finer ring):
//! * no vertex of any loop or ring lies inside a circle but outside its ring
//!   polygon (the sag band between chord and arc) — [`Rings::check_vertices`];
//! * every vertex created by a polygon boolean lies outside every circle's
//!   sag band (it is a line–line crossing, not a chord artefact) —
//!   [`Rings::check_output`];
//! * no line or circle is tangent to a used arc of a circle except at a shared vertex.
use super::{Contour, Region, SAME, snap};
use crate::tol;
use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, TAU};

pub(super) type P2 = [f64; 2];

/// Two circles closer than this in centre and radius are the same circle; a
/// vertex this close to a circle is placed on it (Truck's linear tolerance).
const CIRCLE_MATCH: f64 = tol::KERNEL_LINEAR;
/// A line or circle this close to tangency with a circle (and not meeting it
/// at a shared vertex) is refused as ambiguous.
const TANGENT: f64 = tol::KERNEL_LINEAR;
/// Margin (mm) by which a vertex inside a circle must be inside its ring
/// polygon: two grid steps.
const RING_MARGIN: f64 = 2.0 / super::GRID_STEPS_PER_MM;

/// Kind of a loop edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Line,
    /// Arc of circle `circle`, counter-clockwise when `ccw`.
    Arc {
        circle: usize,
        ccw: bool,
    },
}

/// Closed loop: edge `i` runs from `pts[i]` to `pts[(i + 1) % n]`; a loop
/// with one vertex and one arc is a full circle.
#[derive(Clone, Debug)]
pub(super) struct ELoop {
    pub pts: Vec<P2>,
    pub kind: Vec<Kind>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Circ {
    pub c: P2,
    pub r: f64,
}

/// Why a construction attempt stopped.
#[derive(Debug)]
pub(super) enum Fail {
    /// Ambiguous at this ring resolution: retry finer, then refuse with it.
    Refine(String),
    /// Refuse now.
    Refuse(String),
    /// A kernel error (passed through).
    Kernel(crate::KernelError),
}

pub(super) type FResult<T> = std::result::Result<T, Fail>;

/// The circles of one construction (deduplicated within [`CIRCLE_MATCH`]).
#[derive(Default)]
pub(super) struct CircleSet {
    pub circles: Vec<Circ>,
}

impl CircleSet {
    pub fn id(&mut self, c: P2, r: f64) -> usize {
        if let Some(i) = self.circles.iter().position(|k| {
            (k.c[0] - c[0]).abs() <= CIRCLE_MATCH
                && (k.c[1] - c[1]).abs() <= CIRCLE_MATCH
                && (k.r - r).abs() <= CIRCLE_MATCH
        }) {
            return i;
        }
        self.circles.push(Circ { c, r });
        self.circles.len() - 1
    }
}

fn key(p: P2) -> [i64; 2] {
    p.map(|x| (x * super::GRID_STEPS_PER_MM).round() as i64)
}

fn dist(a: P2, b: P2) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn angle(c: &Circ, p: P2) -> f64 {
    (p[1] - c.c[1]).atan2(p[0] - c.c[0])
}

fn on_circle(c: &Circ, t: f64) -> P2 {
    [c.c[0] + c.r * t.cos(), c.c[1] + c.r * t.sin()]
}

/// Angular spans `(start, sweep)` of circle `k` that the loops' arcs use (a
/// full circle is `(0, 2π)`). Where no arc of a circle runs, the circle has
/// no boundary: tangency or near-contact there touches nothing (e.g. two
/// fillet arcs whose full circles touch beyond both arcs).
fn arc_cover(loops: &[ELoop], circles: &[Circ], k: usize) -> Vec<(f64, f64)> {
    use std::f64::consts::TAU;
    let c = &circles[k];
    let mut out = Vec::new();
    for l in loops {
        let n = l.pts.len();
        for i in 0..n {
            let Kind::Arc { circle, ccw } = l.kind[i] else {
                continue;
            };
            if circle != k {
                continue;
            }
            if n == 1 {
                out.push((0.0, TAU));
                continue;
            }
            let (s, e) = (l.pts[i], l.pts[(i + 1) % n]);
            let (a0, a1) = if ccw {
                (angle(c, s), angle(c, e))
            } else {
                (angle(c, e), angle(c, s))
            };
            let sweep = (a1 - a0).rem_euclid(TAU);
            out.push((a0, if sweep == 0.0 { TAU } else { sweep }));
        }
    }
    out
}

/// Whether `p` (near circle `c`) lies within `slack` radians of a span of
/// `cover`.
/// Angular half-width (rad) of a near-tangent contact on `c`: a line or
/// circle within [`TANGENT`] of tangency crosses `c` up to √(2r·TANGENT) from
/// the foot, so a used arc ending that close may still be crossed.
fn tangent_slack(c: &Circ) -> f64 {
    (CIRCLE_MATCH + (2.0 * c.r * TANGENT).sqrt()) / c.r
}

fn covered(cover: &[(f64, f64)], c: &Circ, p: P2, slack: f64) -> bool {
    use std::f64::consts::TAU;
    let ap = angle(c, p);
    cover
        .iter()
        .any(|&(a0, sweep)| (ap - a0 + slack).rem_euclid(TAU) <= sweep + 2.0 * slack)
}

/// Place every loop vertex within [`CIRCLE_MATCH`] of one circle exactly on
/// it (two circles: on their nearest intersection) and snap it to the grid.
/// Deterministic, so a vertex shared by several loops moves identically.
pub(super) fn canonicalize(loops: &mut [ELoop], circles: &[Circ]) -> FResult<()> {
    for l in loops.iter_mut() {
        for p in l.pts.iter_mut() {
            let near: Vec<&Circ> = circles
                .iter()
                .filter(|k| (dist(*p, k.c) - k.r).abs() <= CIRCLE_MATCH)
                .collect();
            let q = match near.as_slice() {
                [k] => {
                    let d = dist(*p, k.c);
                    if d == 0.0 {
                        *p
                    } else {
                        [
                            k.c[0] + (p[0] - k.c[0]) * k.r / d,
                            k.c[1] + (p[1] - k.c[1]) * k.r / d,
                        ]
                    }
                }
                [a, b] => {
                    let x = circle_circle(a, b);
                    x.into_iter()
                        .min_by(|u, v| dist(*u, *p).total_cmp(&dist(*v, *p)))
                        .filter(|x| dist(*x, *p) <= 2.0 * CIRCLE_MATCH)
                        .ok_or_else(|| {
                            Fail::Refuse(format!(
                                "a vertex at ({:.6}, {:.6}) lies on two circles that do not \
                                 cross there (near-tangent circles)",
                                p[0], p[1]
                            ))
                        })?
                }
                _ => *p,
            };
            *p = q.map(snap);
        }
    }
    // One representative per cluster of points within SAME (the same 2-D
    // point seen on several cap faces carries ~1e-8 mm of noise; first seen
    // wins, so the result is deterministic).
    let mut reps: Vec<P2> = Vec::new();
    let mut cells: HashMap<[i64; 2], Vec<usize>> = HashMap::new();
    let cell = |p: P2| p.map(|c| (c / (4.0 * SAME)).floor() as i64);
    for l in loops.iter_mut() {
        for p in l.pts.iter_mut() {
            let c = cell(*p);
            let mut found = None;
            'search: for dx in -1..=1 {
                for dy in -1..=1 {
                    if let Some(v) = cells.get(&[c[0] + dx, c[1] + dy]) {
                        for &i in v {
                            let q = reps[i];
                            if (p[0] - q[0]).abs() <= SAME && (p[1] - q[1]).abs() <= SAME {
                                found = Some(q);
                                break 'search;
                            }
                        }
                    }
                }
            }
            match found {
                Some(q) => *p = q,
                None => {
                    reps.push(*p);
                    cells.entry(c).or_default().push(reps.len() - 1);
                }
            }
        }
    }
    Ok(())
}

/// Intersections of two circles (empty when they do not cross or are
/// concentric).
fn circle_circle(a: &Circ, b: &Circ) -> Vec<P2> {
    let d = dist(a.c, b.c);
    if d <= SAME || d > a.r + b.r || d < (a.r - b.r).abs() {
        return Vec::new();
    }
    let x = (d * d + a.r * a.r - b.r * b.r) / (2.0 * d);
    let h = (a.r * a.r - x * x).max(0.0).sqrt();
    let u = [(b.c[0] - a.c[0]) / d, (b.c[1] - a.c[1]) / d];
    let m = [a.c[0] + x * u[0], a.c[1] + x * u[1]];
    vec![
        [m[0] - h * u[1], m[1] + h * u[0]],
        [m[0] + h * u[1], m[1] - h * u[0]],
    ]
}

/// One circle's ring: points counter-clockwise from its first special point,
/// with their angles (increasing, within one turn) and whether each is special.
struct Ring {
    pts: Vec<P2>,
    ang: Vec<f64>,
    special: Vec<bool>,
    /// Largest sagitta of a chord (mm).
    sag: f64,
}

/// The rings of all circles of one construction and the lookup from a grid
/// point to its ring positions.
pub(super) struct Rings {
    pub circles: Vec<Circ>,
    rings: Vec<Ring>,
    at: HashMap<[i64; 2], Vec<(usize, usize)>>,
    /// Every point of the polygonized inputs and rings (an output vertex not
    /// in it was created by a polygon boolean).
    known: std::collections::HashSet<[i64; 2]>,
    /// Per circle: the angular spans its input arcs use ([`arc_cover`]).
    cover: Vec<Vec<(f64, f64)>>,
    max_step: f64,
}

impl Rings {
    /// Rings for `circles` given the (canonical) input loops: special points
    /// are loop vertices on a circle and the exact intersections of every line
    /// edge and every other circle with it. `max_step` is the largest angle
    /// between consecutive ring points; at least two chords join consecutive
    /// special points (so no line can coincide with a chord).
    pub fn new(circles: &[Circ], loops: &[ELoop], max_step: f64) -> FResult<Rings> {
        let cover: Vec<Vec<(f64, f64)>> = (0..circles.len())
            .map(|k| arc_cover(loops, circles, k))
            .collect();
        let mut special: Vec<Vec<P2>> = vec![Vec::new(); circles.len()];
        for l in loops {
            for &p in &l.pts {
                for (k, c) in circles.iter().enumerate() {
                    if (dist(p, c.c) - c.r).abs() <= CIRCLE_MATCH {
                        special[k].push(p);
                    }
                }
            }
        }
        // Tangent junctions: a line and an arc (or two arcs of different
        // circles) adjacent in one loop. Tangency is accepted only there (a
        // rounded corner, a fillet tool meeting a face): along the junction's
        // own line (any segment collinear with it touches the ring polygon
        // only at the junction vertex), or between the junction's two arcs.
        // Anywhere else it is a contact the polygons cannot resolve (e.g. a
        // pocket wall touching a hole: a non-manifold edge).
        let mut junctions: Vec<(usize, Option<usize>, P2)> = Vec::new();
        let mut line_junctions: Vec<(usize, P2, P2)> = Vec::new();
        for l in loops {
            let n = l.pts.len();
            for i in 0..n {
                let (kp, kc) = (l.kind[(i + n - 1) % n], l.kind[i]);
                let v = l.pts[i];
                let unit = |a: P2, b: P2| {
                    let d = dist(a, b);
                    [(b[0] - a[0]) / d, (b[1] - a[1]) / d]
                };
                match (kp, kc) {
                    (Kind::Line, Kind::Arc { circle, .. }) => {
                        junctions.push((circle, None, v));
                        line_junctions.push((circle, v, unit(l.pts[(i + n - 1) % n], v)));
                    }
                    (Kind::Arc { circle, .. }, Kind::Line) => {
                        junctions.push((circle, None, v));
                        line_junctions.push((circle, v, unit(v, l.pts[(i + 1) % n])));
                    }
                    (Kind::Arc { circle: a, .. }, Kind::Arc { circle: b, .. }) if a != b => {
                        junctions.push((a, Some(b), v));
                        junctions.push((b, Some(a), v));
                    }
                    _ => {}
                }
            }
        }
        let junction = |k: usize, other: Option<usize>, p: P2| {
            junctions
                .iter()
                .any(|(c, o, q)| *c == k && *o == other && dist(*q, p) <= CIRCLE_MATCH)
        };
        // Line–circle intersections.
        for l in loops {
            let n = l.pts.len();
            for i in 0..n {
                if l.kind[i] != Kind::Line {
                    continue;
                }
                let (a, b) = (l.pts[i], l.pts[(i + 1) % n]);
                for (k, c) in circles.iter().enumerate() {
                    let u = [b[0] - a[0], b[1] - a[1]];
                    let lu = u[0].hypot(u[1]);
                    let tangent_ok = |p: P2| {
                        line_junctions.iter().any(|(kk, q, d)| {
                            *kk == k
                                && dist(*q, p) <= CIRCLE_MATCH
                                && (d[0] * u[1] - d[1] * u[0]).abs() <= 1e-9 * lu
                        })
                    };
                    let used = |p: P2| covered(&cover[k], c, p, tangent_slack(c));
                    for x in line_circle(a, b, c, &special[k], &tangent_ok, &used)? {
                        special[k].push(x);
                    }
                }
            }
        }
        // Circle–circle intersections and tangency.
        for i in 0..circles.len() {
            for j in i + 1..circles.len() {
                let (a, b) = (&circles[i], &circles[j]);
                let d = dist(a.c, b.c);
                if d <= SAME {
                    continue; // concentric, different radii
                }
                let ext = (d - (a.r + b.r)).abs() <= TANGENT;
                let int = (d - (a.r - b.r).abs()).abs() <= TANGENT;
                if ext || int {
                    // Tangent: only where both already have a vertex (a
                    // tangent junction of a loop).
                    let u = [(b.c[0] - a.c[0]) / d, (b.c[1] - a.c[1]) / d];
                    let s = if ext || a.r >= b.r { 1.0 } else { -1.0 };
                    let t = [a.c[0] + s * a.r * u[0], a.c[1] + s * a.r * u[1]];
                    if !junction(i, Some(j), t)
                        && covered(&cover[i], a, t, tangent_slack(a))
                        && covered(&cover[j], b, t, tangent_slack(b))
                    {
                        return Err(Fail::Refuse(format!(
                            "two circles (r {:.6} and {:.6} mm) are tangent at ({:.6}, {:.6}); \
                             tangent contact is ambiguous",
                            a.r, b.r, t[0], t[1]
                        )));
                    }
                    continue;
                }
                for x in circle_circle(a, b) {
                    let x = x.map(snap);
                    special[i].push(x);
                    special[j].push(x);
                }
            }
        }
        let mut rings = Vec::with_capacity(circles.len());
        for (k, c) in circles.iter().enumerate() {
            let mut sp: Vec<(f64, P2)> = special[k].iter().map(|&p| (angle(c, p), p)).collect();
            if sp.is_empty() {
                sp.push((0.0, on_circle(c, 0.0).map(snap)));
            }
            sp.sort_by(|a, b| a.0.total_cmp(&b.0));
            sp.dedup_by(|b, a| dist(a.1, b.1) <= SAME);
            if sp.len() > 1 && dist(sp[0].1, sp[sp.len() - 1].1) <= SAME {
                sp.pop();
            }
            let t0 = sp[0].0;
            let mut ring = Ring {
                pts: Vec::new(),
                ang: Vec::new(),
                special: Vec::new(),
                sag: 0.0,
            };
            for (i, &(t, p)) in sp.iter().enumerate() {
                let t = (t - t0).rem_euclid(TAU);
                let next = if i + 1 < sp.len() {
                    (sp[i + 1].0 - t0).rem_euclid(TAU)
                } else {
                    TAU
                };
                let gap = next - t;
                let m = ((gap / max_step).ceil() as usize).max(2);
                ring.sag = ring.sag.max(c.r * (1.0 - (gap / m as f64 / 2.0).cos()));
                ring.pts.push(p);
                ring.ang.push(t);
                ring.special.push(true);
                for s in 1..m {
                    let a = t + gap * s as f64 / m as f64;
                    ring.pts.push(on_circle(c, t0 + a).map(snap));
                    ring.ang.push(a);
                    ring.special.push(false);
                }
            }
            rings.push(ring);
        }
        let mut at: HashMap<[i64; 2], Vec<(usize, usize)>> = HashMap::new();
        let mut known = std::collections::HashSet::new();
        for (k, r) in rings.iter().enumerate() {
            for (i, &p) in r.pts.iter().enumerate() {
                at.entry(key(p)).or_default().push((k, i));
                known.insert(key(p));
            }
        }
        Ok(Rings {
            circles: circles.to_vec(),
            rings,
            at,
            known,
            cover,
            max_step,
        })
    }

    /// Whether `p` (a grid point) is a vertex of some ring.
    pub fn is_ring_point(&self, p: P2) -> bool {
        self.at.contains_key(&key(p))
    }

    /// Ring position of `p` on circle `k`.
    fn index(&self, k: usize, p: P2) -> Option<usize> {
        self.at
            .get(&key(p))?
            .iter()
            .find(|(c, _)| *c == k)
            .map(|(_, i)| *i)
    }

    /// Special points of all rings strictly inside segment `ab` (within
    /// [`SAME`] of it), ordered from `a`.
    fn splits(&self, a: P2, b: P2) -> Vec<P2> {
        let d = [b[0] - a[0], b[1] - a[1]];
        let l2 = d[0] * d[0] + d[1] * d[1];
        let mut out: Vec<(f64, P2)> = Vec::new();
        if l2 == 0.0 {
            return Vec::new();
        }
        for r in &self.rings {
            for (i, &p) in r.pts.iter().enumerate() {
                if !r.special[i] || dist(p, a) <= SAME || dist(p, b) <= SAME {
                    continue;
                }
                let t = ((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2;
                if t <= 0.0 || t >= 1.0 {
                    continue;
                }
                let off = (p[0] - a[0] - t * d[0]).hypot(p[1] - a[1] - t * d[1]);
                if off <= SAME {
                    out.push((t, p));
                }
            }
        }
        out.sort_by(|x, y| x.0.total_cmp(&y.0));
        out.dedup_by(|y, x| key(x.1) == key(y.1));
        out.into_iter().map(|(_, p)| p).collect()
    }

    /// The polygon of a loop: lines split at special points, arcs replaced by
    /// their ring chains.
    pub fn polygon(&self, l: &ELoop) -> FResult<Contour> {
        let n = l.pts.len();
        let mut out = Vec::new();
        for i in 0..n {
            let (a, b) = (l.pts[i], l.pts[(i + 1) % n]);
            out.push(a);
            match l.kind[i] {
                Kind::Line => out.extend(self.splits(a, b)),
                Kind::Arc { circle, ccw } => {
                    let r = &self.rings[circle];
                    let m = r.pts.len();
                    let (Some(ia), Some(ib)) = (self.index(circle, a), self.index(circle, b))
                    else {
                        return Err(Fail::Refuse(format!(
                            "internal: an arc end ({:.9}, {:.9}) is not on its ring",
                            a[0], a[1]
                        )));
                    };
                    let mut j = ia;
                    loop {
                        j = if ccw { (j + 1) % m } else { (j + m - 1) % m };
                        if j == ib {
                            break;
                        }
                        out.push(r.pts[j]);
                    }
                }
            }
        }
        out.dedup_by(|b, a| key(*a) == key(*b));
        while out.len() > 1 && key(out[0]) == key(out[out.len() - 1]) {
            out.pop();
        }
        Ok(out)
    }

    /// Register polygon points of the inputs as known (not created by a boolean).
    pub fn learn(&mut self, c: &Contour) {
        for p in c {
            self.known.insert(key(*p));
        }
    }

    /// Every known vertex inside a circle must be inside that circle's ring
    /// polygon, and none may lie on a circle without being one of its ring
    /// points (it would be on the wrong side of a chord).
    pub fn check_vertices(&self) -> FResult<()> {
        // Sorted: the first violation found (refine vs refuse) must not depend
        // on hash order.
        let mut pts: Vec<[i64; 2]> = self.known.iter().copied().collect();
        pts.sort_unstable();
        for kp in pts {
            let p = kp.map(|v| v as f64 / super::GRID_STEPS_PER_MM);
            for (k, c) in self.circles.iter().enumerate() {
                if self
                    .at
                    .get(&kp)
                    .is_some_and(|v| v.iter().any(|(cc, _)| *cc == k))
                {
                    continue;
                }
                let d = dist(p, c.c);
                if d >= c.r + SAME {
                    continue;
                }
                // Away from every arc of this circle (by more than two ring
                // chords) no ring chord is in any contour: nothing to resolve.
                if !covered(&self.cover[k], c, p, 2.0 * self.max_step + SAME / c.r) {
                    continue;
                }
                if d > c.r - SAME {
                    return Err(Fail::Refuse(format!(
                        "a vertex at ({:.6}, {:.6}) lies on a circle (r {:.6} mm) without \
                         meeting it at a shared point (tangent or near-coincident geometry)",
                        p[0], p[1], c.r
                    )));
                }
                if d < c.r - self.rings[k].sag - SAME {
                    continue; // inside every chord
                }
                if self.inside_ring(k, p) <= RING_MARGIN {
                    return Err(Fail::Refine(format!(
                        "a vertex at ({:.6}, {:.6}) lies {:.3e} mm inside a circle (r {:.6} mm), \
                         closer than its polygonization can resolve",
                        p[0],
                        p[1],
                        c.r - d,
                        c.r
                    )));
                }
            }
        }
        Ok(())
    }

    /// Signed distance of `p` inside the chord of ring `k` spanning its angle.
    fn inside_ring(&self, k: usize, p: P2) -> f64 {
        let r = &self.rings[k];
        let c = &self.circles[k];
        let t0 = angle(c, r.pts[0]);
        let t = (angle(c, p) - t0).rem_euclid(TAU);
        let i = match r.ang.binary_search_by(|a| a.total_cmp(&t)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let (a, b) = (r.pts[i], r.pts[(i + 1) % r.pts.len()]);
        let e = [b[0] - a[0], b[1] - a[1]];
        let le = e[0].hypot(e[1]);
        (e[0] * (p[1] - a[1]) - e[1] * (p[0] - a[0])) / le
    }

    /// A vertex created by a polygon boolean must be a line–line crossing,
    /// clear of every circle's chord band.
    pub fn check_output(&self, region: &Region) -> FResult<()> {
        for p in region.iter().flatten().flatten() {
            if self.known.contains(&key(*p)) {
                continue;
            }
            for (k, c) in self.circles.iter().enumerate() {
                let d = dist(*p, c.c);
                if d <= c.r + SAME && d >= c.r - self.rings[k].sag - SAME {
                    return Err(Fail::Refine(format!(
                        "the polygon boolean crossed the polygonized circle (r {:.6} mm) at \
                         ({:.6}, {:.6}) away from an exact intersection",
                        c.r, p[0], p[1]
                    )));
                }
            }
        }
        Ok(())
    }

    /// The exact loop of a boolean output contour: segments between two
    /// ring-adjacent points of one circle are arcs, others lines; vertices
    /// between two pieces of the same arc are dropped. A full circle keeps its
    /// ring's first (special) point.
    pub fn exact(&self, c: &Contour) -> ELoop {
        let n = c.len();
        let kinds: Vec<Kind> = (0..n)
            .map(|i| {
                let (p, q) = (c[i], c[(i + 1) % n]);
                let (Some(kp), Some(kq)) = (self.at.get(&key(p)), self.at.get(&key(q))) else {
                    return Kind::Line;
                };
                for &(k, ip) in kp {
                    let m = self.rings[k].pts.len();
                    for &(k2, iq) in kq {
                        if k2 != k {
                            continue;
                        }
                        if iq == (ip + 1) % m {
                            return Kind::Arc {
                                circle: k,
                                ccw: true,
                            };
                        }
                        if ip == (iq + 1) % m {
                            return Kind::Arc {
                                circle: k,
                                ccw: false,
                            };
                        }
                    }
                }
                Kind::Line
            })
            .collect();
        let keep: Vec<bool> = (0..n)
            .map(|i| {
                let prev = kinds[(i + n - 1) % n];
                prev != kinds[i] || kinds[i] == Kind::Line
            })
            .collect();
        let mut pts = Vec::new();
        let mut kind = Vec::new();
        if keep.iter().any(|&k| k) {
            // Start at a kept vertex; each kept vertex's edge kind runs to the
            // next kept vertex.
            let s = keep.iter().position(|&k| k).unwrap_or(0);
            for d in 0..n {
                let i = (s + d) % n;
                if keep[i] {
                    pts.push(c[i]);
                    kind.push(kinds[i]);
                }
            }
        } else if let Kind::Arc { circle, .. } = kinds[0] {
            // Full circle: seam at the ring's first point.
            let seam = self.rings[circle].pts[0];
            pts.push(if c.iter().any(|p| key(*p) == key(seam)) {
                seam
            } else {
                c[0]
            });
            kind.push(kinds[0]);
        }
        ELoop { pts, kind }
    }

    /// Ring position of `p` on circle `k` and that ring's angle at it.
    pub fn ring_angle(&self, k: usize, p: P2) -> Option<f64> {
        let i = self.index(k, p)?;
        Some(self.rings[k].ang[i])
    }

    /// Angle origin of ring `k` (absolute angle of its first point).
    pub fn ring_origin(&self, k: usize) -> f64 {
        angle(&self.circles[k], self.rings[k].pts[0])
    }
}

/// Exact intersections of segment `ab` with circle `c` strictly inside the
/// segment (not at a point already special on the circle); a tangent contact
/// away from a shared vertex, on an arc some loop uses, is refused.
fn line_circle(
    a: P2,
    b: P2,
    c: &Circ,
    special: &[P2],
    tangent_ok: &dyn Fn(P2) -> bool,
    used: &dyn Fn(P2) -> bool,
) -> FResult<Vec<P2>> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = d[0].hypot(d[1]);
    if len == 0.0 {
        return Ok(Vec::new());
    }
    let u = [d[0] / len, d[1] / len];
    // Foot of the centre on the line, signed distance h.
    let t_foot = (c.c[0] - a[0]) * u[0] + (c.c[1] - a[1]) * u[1];
    let foot = [a[0] + t_foot * u[0], a[1] + t_foot * u[1]];
    let h = dist(foot, c.c);
    let is_special = |p: P2| special.iter().any(|s| dist(*s, p) <= CIRCLE_MATCH);
    let inside = |t: f64| t > SAME && t < len - SAME;
    if (h - c.r).abs() <= TANGENT {
        let touches = t_foot > -SAME && t_foot < len + SAME;
        let junction = tangent_ok(foot);
        if touches && !junction && used(foot) {
            return Err(Fail::Refuse(format!(
                "a straight edge is tangent to a circle (r {:.6} mm) at ({:.6}, {:.6}); \
                 tangent contact is ambiguous",
                c.r, foot[0], foot[1]
            )));
        }
        return Ok(Vec::new());
    }
    if h > c.r {
        return Ok(Vec::new());
    }
    let s = (c.r * c.r - h * h).sqrt();
    let mut out = Vec::new();
    for t in [t_foot - s, t_foot + s] {
        let p = [a[0] + t * u[0], a[1] + t * u[1]];
        if inside(t) && !is_special(p) {
            out.push(p.map(snap));
        }
    }
    Ok(out)
}

/// Global partition of each circle used by the exact rebuild: the angles (in
/// ring coordinates) of every vertex on it, each gap split into spans of at
/// most a quarter turn. Points are returned with their absolute positions.
pub(super) struct Partition {
    /// Per circle: (ring-relative angle, point), increasing; empty when unused.
    pub pts: Vec<Vec<(f64, P2)>>,
}

impl Partition {
    pub fn new(rings: &Rings, breaks: &[Vec<P2>]) -> Partition {
        let mut all = Vec::with_capacity(breaks.len());
        for (k, b) in breaks.iter().enumerate() {
            let c = rings.circles[k];
            let mut pts: Vec<(f64, P2)> = b
                .iter()
                .filter_map(|&p| rings.ring_angle(k, p).map(|a| (a, p)))
                .collect();
            pts.sort_by(|a, b| a.0.total_cmp(&b.0));
            pts.dedup_by(|b, a| key(a.1) == key(b.1));
            let mut out = Vec::new();
            let t0 = rings.ring_origin(k);
            for i in 0..pts.len() {
                let (a, p) = pts[i];
                let next = if i + 1 < pts.len() {
                    pts[i + 1].0
                } else {
                    pts[0].0 + TAU
                };
                let gap = next - a;
                let s = ((gap / FRAC_PI_2 - 1e-9).ceil() as usize).max(1);
                out.push((a, p));
                for j in 1..s {
                    let t = a + gap * j as f64 / s as f64;
                    out.push((t, on_circle(&c, t0 + t).map(snap)));
                }
            }
            all.push(out);
        }
        Partition { pts: all }
    }

    /// Partition index of point `p` (a vertex) on circle `k`.
    pub fn index(&self, k: usize, p: P2) -> Option<usize> {
        self.pts[k].iter().position(|q| key(q.1) == key(p))
    }
}

#[cfg(test)]
mod tangent_tests {
    use super::*;

    /// A line 5e-7 mm inside tangency of r = 100 crosses the
    /// circle about ±0.01 mm from the foot; an arc ending 5e-3 mm before the
    /// foot is crossed, so the contact must be refused, not ignored.
    #[test]
    fn near_tangent_crossing_inside_a_used_arc_is_refused() {
        let c = Circ {
            c: [0.0, 0.0],
            r: 100.0,
        };
        let y = 100.0 - 5.0e-7;
        let (a, b) = ([-50.0, y], [50.0, y]);
        let start = std::f64::consts::FRAC_PI_2 + 5.0e-3 / 100.0;
        let cover = [(start, 1.0)];
        let used = |p: P2| covered(&cover, &c, p, tangent_slack(&c));
        let r = line_circle(a, b, &c, &[], &|_| false, &used);
        assert!(
            matches!(r, Err(Fail::Refuse(ref m)) if m.contains("tangent")),
            "{r:?}"
        );
        // An arc ending well clear of the contact (0.5 mm) is not touched.
        let far = [(std::f64::consts::FRAC_PI_2 + 0.5 / 100.0, 1.0)];
        let used = |p: P2| covered(&far, &c, p, tangent_slack(&c));
        assert!(
            line_circle(a, b, &c, &[], &|_| false, &used)
                .unwrap()
                .is_empty()
        );
    }
}
