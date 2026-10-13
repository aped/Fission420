//! Sheet metal, bounded subset (run 6 pull-forward of SESSION_7 §5, coverage
//! SM01): a contour flange from an open chain of sketch lines.
//!
//! Convention (documented in docs/SHEET_METAL.md): the sketch chain is one
//! surface of the sheet (the "sketch side"); the material lies on the right
//! of the chain's direction (on the left with `flip`), `thickness` thick, and
//! the part is extruded `width` along the sketch normal. At every interior
//! vertex a cylindrical bend of inner radius `radius` replaces the corner:
//! where the material is outside the turn the sketch side is the inner
//! surface (radius R), where it is inside the turn the sketch side is the
//! outer surface (radius R + t). The straight flange lengths are measured
//! between tangent points. The flat length uses the neutral axis at
//! `k_factor · t` from the inner surface: L = Σ straights + Σ θ (R + K t)
//! (bend allowance; no deduction tables). Everything is exact: lines and
//! circular arcs in the section, extruded.
use crate::{DocError, FeatureId, Result};
use rcad_sketch::{LoopSegment, Profile};

/// One straight flange and the bend after it.
#[derive(Debug, Clone, PartialEq)]
pub struct Flange {
    /// Straight length between tangent points (mm).
    pub straight: f64,
    /// Bend after this flange: signed turn angle (rad, + = left), none for the last.
    pub bend: Option<f64>,
}

/// The analysed contour flange.
#[derive(Debug, Clone)]
pub struct SheetPart {
    /// Closed section (sketch coordinates), counter-clockwise.
    pub section: Profile,
    pub flanges: Vec<Flange>,
    /// Neutral-axis flat length (mm).
    pub flat_length: f64,
    /// Positions of the bend centre lines along the flat length (mm).
    pub bend_lines: Vec<f64>,
}

type P = [f64; 2];

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}
fn add(a: P, b: P) -> P {
    [a[0] + b[0], a[1] + b[1]]
}
fn mul(a: P, k: f64) -> P {
    [a[0] * k, a[1] * k]
}
fn norm(a: P) -> f64 {
    a[0].hypot(a[1])
}
fn cross(a: P, b: P) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

/// Analyse the chain `pts` (vertices in order, at least two) with the given
/// rule; refused with a reason for impossible input.
pub fn contour_flange(
    id: FeatureId,
    pts: &[P],
    thickness: f64,
    radius: f64,
    k: f64,
    flip: bool,
) -> Result<SheetPart> {
    let bad = |m: String| DocError::Validation(format!("{id}: sheet metal: {m}"));
    if !(thickness.is_finite() && thickness > 0.0) {
        return Err(bad(format!("thickness must be > 0 mm, got {thickness}")));
    }
    if !(radius.is_finite() && radius > 0.0) {
        return Err(bad(format!("bend radius must be > 0 mm, got {radius}")));
    }
    if !(k.is_finite() && (0.0..=1.0).contains(&k)) {
        return Err(bad(format!("K-factor must be in [0, 1], got {k}")));
    }
    if pts.len() < 2 {
        return Err(bad("the profile needs at least one line".into()));
    }
    let n = pts.len();
    let dirs: Vec<P> = (0..n - 1)
        .map(|i| {
            let d = sub(pts[i + 1], pts[i]);
            let l = norm(d);
            mul(d, 1.0 / l)
        })
        .collect();
    let lens: Vec<f64> = (0..n - 1).map(|i| norm(sub(pts[i + 1], pts[i]))).collect();
    if lens.iter().any(|l| *l < rcad_kernel::tol::MIN_LENGTH) {
        return Err(bad("a profile line has no length".into()));
    }
    // Material side: right of the direction (−90°), or left with `flip`.
    let side = if flip { 1.0 } else { -1.0 };
    let normal = |d: P| mul([-d[1], d[0]], side); // unit, toward the material
    // Bends at interior vertices.
    let mut turns = Vec::new();
    for i in 1..n - 1 {
        let (a, b) = (dirs[i - 1], dirs[i]);
        let th = cross(a, b).atan2(a[0] * b[0] + a[1] * b[1]);
        if th.abs() < 1e-9 {
            return Err(bad(format!(
                "collinear lines at vertex {i}: no bend to make"
            )));
        }
        if th.abs() > std::f64::consts::PI - 1e-6 {
            return Err(bad(format!(
                "a 180° fold at vertex {i} is not supported (hem)"
            )));
        }
        // Material outside the turn: the turn goes away from the material.
        let material_inside = (th > 0.0) == (side > 0.0);
        let rs = if material_inside {
            radius + thickness
        } else {
            radius
        };
        turns.push((th, rs, material_inside));
    }
    let setback = |i: usize| -> f64 {
        // At vertex i (1..n-1) of the chain.
        let (th, rs, _) = turns[i - 1];
        rs * (th.abs() / 2.0).tan()
    };
    let mut flanges = Vec::new();
    for j in 0..n - 1 {
        let s0 = if j > 0 { setback(j) } else { 0.0 };
        let s1 = if j + 1 < n - 1 { setback(j + 1) } else { 0.0 };
        let straight = lens[j] - s0 - s1;
        if straight <= rcad_kernel::tol::MIN_LENGTH {
            return Err(bad(format!(
                "flange {} is too short for its bends ({:.4} mm left between tangent points)",
                j + 1,
                straight
            )));
        }
        flanges.push(Flange {
            straight,
            bend: (j + 1 < n - 1).then(|| turns[j].0),
        });
    }
    // Sketch-side path with arcs, and the material-side path (offset by t).
    // Each is a list of segments (Line start, or Arc start/centre/ccw) plus end.
    let mut near: Vec<LoopSegment> = Vec::new();
    let mut far: Vec<LoopSegment> = Vec::new();
    let t = thickness;
    let mut cur = pts[0];
    let mut cur_far = add(pts[0], mul(normal(dirs[0]), t));
    for j in 0..n - 1 {
        let s1 = if j + 1 < n - 1 { setback(j + 1) } else { 0.0 };
        let d = dirs[j];
        let end = sub(pts[j + 1], mul(d, s1));
        near.push(LoopSegment::Line { start: cur });
        far.push(LoopSegment::Line { start: cur_far });
        cur = end;
        cur_far = add(end, mul(normal(d), t));
        if j + 1 < n - 1 {
            let (th, rs, inside) = turns[j];
            // Arc centre: on the inside of the turn (left for th > 0).
            let left = [-d[1], d[0]];
            let c = add(end, mul(left, th.signum() * rs));
            let next = dirs[j + 1];
            let start_next = add(pts[j + 1], mul(next, setback(j + 1)));
            near.push(LoopSegment::Arc {
                start: end,
                center: c,
                ccw: th > 0.0,
            });
            let _ = inside;
            far.push(LoopSegment::Arc {
                start: cur_far,
                center: c,
                ccw: th > 0.0,
            });
            cur = start_next;
            cur_far = add(start_next, mul(normal(next), t));
        }
    }
    let end_near = cur;
    let end_far = cur_far;
    // Closed loop: near path forward, cap to the far end, far path backward,
    // cap back to the start.
    let mut segs = near.clone();
    segs.push(LoopSegment::Line { start: end_near });
    // Reverse the far path: segment k from s_k to s_{k+1} becomes s_{k+1}→s_k.
    let far_starts: Vec<P> = far.iter().map(|s| s.start()).chain([end_far]).collect();
    for k in (0..far.len()).rev() {
        let to = far_starts[k + 1];
        match far[k] {
            LoopSegment::Line { .. } => segs.push(LoopSegment::Line { start: to }),
            LoopSegment::Arc { center, ccw, .. } => segs.push(LoopSegment::Arc {
                start: to,
                center,
                ccw: !ccw,
            }),
        }
    }
    // Start cap: from the far start back to pts[0] (closes the loop).
    segs.push(LoopSegment::Line {
        start: far_starts[0],
    });
    let segs = orient_ccw(segs);
    check_simple(&segs).map_err(|m| bad(format!("the flange overlaps itself ({m})")))?;
    // Flat pattern along the neutral axis.
    let mut x = 0.0;
    let mut bend_lines = Vec::new();
    for f in &flanges {
        x += f.straight;
        if let Some(th) = f.bend {
            let ba = th.abs() * (radius + k * t);
            bend_lines.push(x + ba / 2.0);
            x += ba;
        }
    }
    Ok(SheetPart {
        section: Profile::Loop(segs),
        flanges,
        flat_length: x,
        bend_lines,
    })
}

/// Polygonized loop (arcs at ≤ 2° steps).
fn polyline(segs: &[LoopSegment]) -> Vec<P> {
    let n = segs.len();
    let mut out = Vec::new();
    for i in 0..n {
        let a = segs[i].start();
        let b = segs[(i + 1) % n].start();
        out.push(a);
        if let LoopSegment::Arc { center, ccw, .. } = segs[i] {
            let r = norm(sub(a, center));
            let a0 = (a[1] - center[1]).atan2(a[0] - center[0]);
            let a1 = (b[1] - center[1]).atan2(b[0] - center[0]);
            let tau = std::f64::consts::TAU;
            let sweep = if ccw {
                (a1 - a0).rem_euclid(tau)
            } else {
                -(a0 - a1).rem_euclid(tau)
            };
            let steps = ((sweep.abs() / 2f64.to_radians()).ceil() as usize).max(2);
            for k in 1..steps {
                let ang = a0 + sweep * k as f64 / steps as f64;
                out.push([center[0] + r * ang.cos(), center[1] + r * ang.sin()]);
            }
        }
    }
    out
}

fn signed_area(p: &[P]) -> f64 {
    let n = p.len();
    (0..n).map(|i| cross(p[i], p[(i + 1) % n])).sum::<f64>() / 2.0
}

/// The loop traversed counter-clockwise.
fn orient_ccw(segs: Vec<LoopSegment>) -> Vec<LoopSegment> {
    if signed_area(&polyline(&segs)) >= 0.0 {
        return segs;
    }
    let n = segs.len();
    let starts: Vec<P> = segs.iter().map(|s| s.start()).collect();
    let mut out = Vec::with_capacity(n);
    for i in (0..n).rev() {
        let to = starts[(i + 1) % n];
        out.push(match segs[i] {
            LoopSegment::Line { .. } => LoopSegment::Line { start: to },
            LoopSegment::Arc { center, ccw, .. } => LoopSegment::Arc {
                start: to,
                center,
                ccw: !ccw,
            },
        });
    }
    out
}

/// No two non-adjacent edges of the polygonized loop intersect.
fn check_simple(segs: &[LoopSegment]) -> std::result::Result<(), String> {
    let p = polyline(segs);
    let n = p.len();
    let hit = |a: P, b: P, c: P, d: P| {
        let o = |p: P, q: P, r: P| cross(sub(q, p), sub(r, p));
        let (d1, d2, d3, d4) = (o(a, b, c), o(a, b, d), o(c, d, a), o(c, d, b));
        d1 * d2 < 0.0 && d3 * d4 < 0.0
    };
    for i in 0..n {
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            if hit(p[i], p[(i + 1) % n], p[j], p[(j + 1) % n]) {
                return Err(format!("edges near ({:.3}, {:.3}) cross", p[i][0], p[i][1]));
            }
        }
    }
    Ok(())
}

/// The non-construction lines of `s` as one open chain of vertices, from
/// the end with the lower entity id (deterministic); anything else (arcs,
/// circles, branches, loops, several pieces) is refused.
pub fn line_chain(s: &rcad_sketch::Sketch) -> std::result::Result<Vec<[f64; 2]>, String> {
    let mut edges = Vec::new();
    for e in &s.entities {
        if s.construction.contains(&e.id()) {
            continue;
        }
        match e {
            rcad_sketch::Entity::Line { a, b, .. } => edges.push((*a, *b)),
            _ => return Err("only lines are supported (arcs/circles are not)".into()),
        }
    }
    if edges.is_empty() {
        return Err("the sketch has no lines".into());
    }
    let xy = |p: rcad_sketch::PointId| {
        s.points
            .iter()
            .find(|q| q.id == p)
            .map(|q| [q.x, q.y])
            .ok_or_else(|| format!("missing point {p:?}"))
    };
    // Coincident points count as one vertex.
    let key = |p: rcad_sketch::PointId| -> std::result::Result<[i64; 2], String> {
        let v = xy(p)?;
        Ok([(v[0] * 1e6).round() as i64, (v[1] * 1e6).round() as i64])
    };
    let mut deg: std::collections::BTreeMap<[i64; 2], usize> = Default::default();
    for (a, b) in &edges {
        *deg.entry(key(*a)?).or_default() += 1;
        *deg.entry(key(*b)?).or_default() += 1;
    }
    if deg.values().any(|d| *d > 2) {
        return Err("the lines branch".into());
    }
    let ends: Vec<_> = deg
        .iter()
        .filter(|(_, d)| **d == 1)
        .map(|(k, _)| *k)
        .collect();
    if ends.len() != 2 {
        return Err("the lines must form one open chain (it is closed or in pieces)".into());
    }
    let mut used = vec![false; edges.len()];
    // Start at the end belonging to the first edge in entity order.
    let first = edges
        .iter()
        .flat_map(|(a, b)| [*a, *b])
        .find(|p| key(*p).is_ok_and(|k| ends.contains(&k)))
        .ok_or("no chain end")?;
    let mut cur = key(first)?;
    let mut out = vec![xy(first)?];
    for _ in 0..edges.len() {
        let Some(i) = (0..edges.len())
            .find(|&i| !used[i] && (key(edges[i].0) == Ok(cur) || key(edges[i].1) == Ok(cur)))
        else {
            break;
        };
        used[i] = true;
        let next = if key(edges[i].0)? == cur {
            edges[i].1
        } else {
            edges[i].0
        };
        cur = key(next)?;
        out.push(xy(next)?);
    }
    if used.iter().any(|u| !u) {
        return Err("the lines must form one open chain".into());
    }
    Ok(out)
}

/// Flat pattern as a sketch: the outline (length × width) and the bend
/// centre lines as construction lines (DXF: layer CUT and BEND).
pub fn flat_pattern_sketch(part: &SheetPart, width: f64) -> rcad_sketch::Sketch {
    let mut s = rcad_sketch::Sketch::default();
    let l = part.flat_length;
    let c: Vec<_> = [[0.0, 0.0], [l, 0.0], [l, width], [0.0, width]]
        .iter()
        .map(|q| s.add_point(q[0], q[1]))
        .collect();
    for i in 0..4 {
        s.add_line(c[i], c[(i + 1) % 4]);
    }
    for x in &part.bend_lines {
        let (a, b) = (s.add_point(*x, 0.0), s.add_point(*x, width));
        let e = s.add_line(a, b);
        s.construction.insert(e);
    }
    s
}
