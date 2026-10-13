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

/// No two edges of the polygonized loop intersect or touch, other than
/// neighbours at their shared vertex (review S1: collinear overlaps, an end
/// point on another edge and a spike back along the previous edge count).
fn check_simple(segs: &[LoopSegment]) -> std::result::Result<(), String> {
    let p = polyline(segs);
    let n = p.len();
    let scale = p
        .iter()
        .fold(1.0f64, |m, q| m.max(q[0].abs()).max(q[1].abs()));
    let eps = 1e-9 * scale;
    let o = |a: P, b: P, c: P| cross(sub(b, a), sub(c, a));
    // Point c on the closed segment a-b (within eps).
    let on = |a: P, b: P, c: P| {
        let l = norm(sub(b, a));
        if l == 0.0 {
            return norm(sub(c, a)) <= eps;
        }
        let d = o(a, b, c).abs() / l;
        let t = ((c[0] - a[0]) * (b[0] - a[0]) + (c[1] - a[1]) * (b[1] - a[1])) / (l * l);
        d <= eps && t >= -eps / l && t <= 1.0 + eps / l
    };
    let hit = |a: P, b: P, c: P, d: P| segs_meet(a, b, c, d, eps);
    let _ = (&o, &on);
    for i in 0..n {
        let (a, b) = (p[i], p[(i + 1) % n]);
        for j in i + 1..n {
            let (c, d) = (p[j], p[(j + 1) % n]);
            let next = j == i + 1;
            let wrap = i == 0 && j == n - 1;
            if next || wrap {
                // Neighbours share one vertex; they must not fold back onto
                // each other (collinear and opposite).
                let (u, v) = if next {
                    (sub(b, a), sub(d, c))
                } else {
                    (sub(d, c), sub(b, a))
                };
                let (lu, lv) = (norm(u), norm(v));
                if lu > 0.0
                    && lv > 0.0
                    && (cross(u, v) / (lu * lv)).abs() < 1e-12
                    && u[0] * v[0] + u[1] * v[1] < 0.0
                {
                    return Err(format!("edges fold back at ({:.3}, {:.3})", b[0], b[1]));
                }
                continue;
            }
            if hit(a, b, c, d) {
                return Err(format!("edges near ({:.3}, {:.3}) meet", a[0], a[1]));
            }
        }
    }
    Ok(())
}

/// Closed segments a-b and c-d intersect or touch (within `eps`).
fn segs_meet(a: P, b: P, c: P, d: P, eps: f64) -> bool {
    let o = |a: P, b: P, c: P| cross(sub(b, a), sub(c, a));
    let on = |a: P, b: P, c: P| {
        let l = norm(sub(b, a));
        if l == 0.0 {
            return norm(sub(c, a)) <= eps;
        }
        let dd = o(a, b, c).abs() / l;
        let t = ((c[0] - a[0]) * (b[0] - a[0]) + (c[1] - a[1]) * (b[1] - a[1])) / (l * l);
        dd <= eps && t >= -eps / l && t <= 1.0 + eps / l
    };
    let (d1, d2, d3, d4) = (o(a, b, c), o(a, b, d), o(c, d, a), o(c, d, b));
    (d1 * d2 < 0.0 && d3 * d4 < 0.0) || on(a, b, c) || on(a, b, d) || on(c, d, a) || on(c, d, b)
}

/// Even-odd point-in-polygon.
fn inside(poly: &[P], q: P) -> bool {
    let n = poly.len();
    let mut c = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > q[1]) != (b[1] > q[1])
            && q[0] < a[0] + (q[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
        {
            c = !c;
        }
    }
    c
}

/// Polygon of a kernel hole profile (arcs/circles at ≤ 2° chords).
fn hole_poly(h: &kk::Profile) -> Vec<P> {
    match h {
        kk::Profile::Polygon(v) => v.clone(),
        kk::Profile::Loop(l) => polyline(&to_sk(l)),
        kk::Profile::Circle { center, radius } => (0..180)
            .map(|k| {
                let a = k as f64 * std::f64::consts::TAU / 180.0;
                [center[0] + radius * a.cos(), center[1] + radius * a.sin()]
            })
            .collect(),
    }
}

/// Review S4: every hole stays strictly inside the (relief-cut) outline.
fn check_holes(outline: &[LoopSegment], holes: &[kk::Profile]) -> std::result::Result<(), String> {
    let p = polyline(outline);
    let scale = p
        .iter()
        .fold(1.0f64, |m, q| m.max(q[0].abs()).max(q[1].abs()));
    let eps = 1e-9 * scale;
    for (k, h) in holes.iter().enumerate() {
        let hp = hole_poly(h);
        if hp.is_empty() {
            continue;
        }
        if !inside(&p, hp[0]) {
            return Err(format!("hole {} is no longer inside the plate", k + 1));
        }
        for i in 0..hp.len() {
            let (a, b) = (hp[i], hp[(i + 1) % hp.len()]);
            for j in 0..p.len() {
                if segs_meet(a, b, p[j], p[(j + 1) % p.len()], eps) {
                    return Err(format!(
                        "hole {} meets the outline near ({:.3}, {:.3})",
                        k + 1,
                        a[0],
                        a[1]
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Polygonized loop area (arcs at ≤ 2° chords).
fn poly_area(segs: &[LoopSegment]) -> f64 {
    signed_area(&polyline(segs))
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

// ---------------------------------------------------------------------------
// Run 7: sheet rule, base face, edge flanges, flat pattern (docs/SHEET_METAL.md,
// ADR 0012). A sheet body is a planar plate (closed sketch profile, `thickness`
// thick) with straight flanges bent outside its outline along straight edges.
// The folded B-rep is built exactly from that definition at every
// regeneration: the plate and one prism per flange (the flange's section —
// cylindrical bend and straight leg — extruded along its edge), glued along
// their shared rectangle (`rcad_kernel::glue`). The flat pattern unfolds each
// flange about its bend by the bend allowance θ·(R + K·t).
// ---------------------------------------------------------------------------

use crate::BodyId;
use rcad_kernel::{self as kk, Frame, LoopSegment as KSeg};
use serde::{Deserialize, Serialize};

/// Bend relief cut into the plate beside a flange that does not span its
/// whole edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReliefShape {
    /// Rectangular notch `relief_width` wide, `relief_depth` deep.
    #[default]
    Rectangular,
    /// Notch with a semicircular end of diameter `relief_width` (depth
    /// includes the semicircle; depth ≥ width / 2).
    Obround,
}

/// The sheet rule shared by every sheet feature of a body (stored on its
/// base feature; edit it there and every flange and the flat pattern
/// regenerate).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SheetRule {
    /// Sheet thickness t (mm).
    pub thickness: f64,
    /// Inside bend radius R (mm).
    pub radius: f64,
    /// Neutral axis at K·t from the inside surface, K in [0, 1].
    pub k_factor: f64,
    #[serde(default)]
    pub relief: ReliefShape,
    /// Relief width along the edge (mm).
    pub relief_width: f64,
    /// Relief depth into the plate from the bend line (mm).
    pub relief_depth: f64,
}

impl SheetRule {
    /// Thickness `t`, radius `r`, K `k`, reliefs t wide and t deep.
    pub fn new(t: f64, r: f64, k: f64) -> Self {
        SheetRule {
            thickness: t,
            radius: r,
            k_factor: k,
            relief: ReliefShape::Rectangular,
            relief_width: t,
            relief_depth: t,
        }
    }

    pub fn validate(&self, id: FeatureId) -> Result<()> {
        let bad = |m: String| DocError::Validation(format!("{id}: sheet rule: {m}"));
        let pos = |v: f64, what: &str| -> Result<()> {
            if v.is_finite() && v >= rcad_kernel::tol::MIN_LENGTH {
                Ok(())
            } else {
                Err(bad(format!(
                    "{what} must be >= {} mm, got {v}",
                    rcad_kernel::tol::MIN_LENGTH
                )))
            }
        };
        pos(self.thickness, "thickness")?;
        pos(self.radius, "inside bend radius")?;
        pos(self.relief_width, "relief width")?;
        pos(self.relief_depth, "relief depth")?;
        if !(self.k_factor.is_finite() && (0.0..=1.0).contains(&self.k_factor)) {
            return Err(bad(format!(
                "K-factor must be in [0, 1], got {}",
                self.k_factor
            )));
        }
        if self.relief == ReliefShape::Obround && self.relief_depth < self.relief_width / 2.0 {
            return Err(bad(format!(
                "an obround relief needs depth >= width / 2 ({} < {})",
                self.relief_depth,
                self.relief_width / 2.0
            )));
        }
        Ok(())
    }

    /// Bend allowance (neutral-axis arc length) of a bend of `angle` rad.
    pub fn bend_allowance(&self, angle: f64) -> f64 {
        angle * (self.radius + self.k_factor * self.thickness)
    }

    /// Names and values of the rule parameters (feature parameters of the base).
    pub fn parameters(&self) -> Vec<(String, f64)> {
        vec![
            ("thickness".into(), self.thickness),
            ("radius".into(), self.radius),
            ("k_factor".into(), self.k_factor),
            ("relief_width".into(), self.relief_width),
            ("relief_depth".into(), self.relief_depth),
        ]
    }

    /// Set one rule parameter (validated as a whole).
    pub fn set(&mut self, id: FeatureId, name: &str, value: f64) -> Result<bool> {
        let mut r = self.clone();
        match name {
            "thickness" => r.thickness = value,
            "radius" => r.radius = value,
            "k_factor" => r.k_factor = value,
            "relief_width" => r.relief_width = value,
            "relief_depth" => r.relief_depth = value,
            _ => return Ok(false),
        }
        r.validate(id)?;
        *self = r;
        Ok(true)
    }
}

/// One edge flange of a sheet body.
#[derive(Debug, Clone, PartialEq)]
pub struct FlangeDef {
    pub id: FeatureId,
    /// Sketch line entity (of the base profile) the flange is attached to.
    pub edge: u32,
    /// Bend angle (rad), in (0, π]: 90° makes the flange perpendicular, π
    /// folds it back over the plate (open hem, gap 2R).
    pub angle: f64,
    /// Straight flange length beyond the bend (tangent line to flange end).
    pub length: f64,
    /// Bend toward −normal of the base sketch (default +normal).
    pub down: bool,
    pub start_offset: f64,
    pub end_offset: f64,
}

/// Regeneration state of one sheet body.
#[derive(Debug, Clone)]
pub struct SheetModel {
    pub base: FeatureId,
    pub body: BodyId,
    /// Base sketch frame (the plate's bottom with `flip` is −t below it).
    pub frame: Frame,
    pub flip: bool,
    pub rule: SheetRule,
    /// Base outline, counter-clockwise in sketch coordinates, with the sketch
    /// line entity of each straight segment.
    pub outline: Outline,
    pub holes: Vec<kk::Profile>,
    pub flanges: Vec<FlangeDef>,
    /// Last feature that changed the body (base or flange).
    pub last_feature: FeatureId,
    /// The flat-pattern feature evaluated so far, if any (later flanges are
    /// refused: the flat would be out of date).
    pub flat: Option<FeatureId>,
}

/// A bend line of the flat pattern (sketch coordinates).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BendLine {
    pub flange: FeatureId,
    /// Bend centre line (middle of the bend zone).
    pub from: [f64; 2],
    pub to: [f64; 2],
    /// Bend direction seen from the base sketch normal.
    pub up: bool,
    pub angle_deg: f64,
    pub inside_radius: f64,
    pub allowance: f64,
}

/// Folded body and flat pattern of a sheet model.
#[derive(Debug, Clone)]
pub struct SheetBuild {
    pub folded: kk::Body,
    /// Flat outline (sketch coordinates, CCW) and holes.
    pub flat_outline: Vec<KSeg>,
    pub flat_holes: Vec<kk::Profile>,
    pub bends: Vec<BendLine>,
    /// Plate bottom frame (flat pattern body is extruded from it).
    pub bottom: Frame,
    /// Bounding box of the flat outline (sketch coordinates): min, max.
    pub flat_extent: ([f64; 2], [f64; 2]),
}

fn to_sk(segs: &[KSeg]) -> Vec<LoopSegment> {
    segs.iter()
        .map(|s| match *s {
            KSeg::Line { start } => LoopSegment::Line { start },
            KSeg::Arc { start, center, ccw } => LoopSegment::Arc { start, center, ccw },
        })
        .collect()
}

/// Outline segments with the sketch line entity of each straight one.
pub type Outline = Vec<(KSeg, Option<u32>)>;

/// Base outline with entity ids from a sketch region and the solved sketch:
/// a straight segment carries the line entity whose end points it joins.
pub fn base_outline(
    id: FeatureId,
    region: &rcad_sketch::Region,
    solved: &rcad_sketch::Sketch,
) -> Result<(Outline, Vec<kk::Profile>)> {
    let segs: Vec<KSeg> = match &region.outer {
        rcad_sketch::Profile::Polygon(v) => v.iter().map(|p| KSeg::Line { start: *p }).collect(),
        rcad_sketch::Profile::Loop(l) => l
            .iter()
            .map(|s| match *s {
                LoopSegment::Line { start } => KSeg::Line { start },
                LoopSegment::Arc { start, center, ccw } => KSeg::Arc { start, center, ccw },
            })
            .collect(),
        rcad_sketch::Profile::Circle { .. } => {
            return Err(DocError::Validation(format!(
                "{id}: sheet base: a circle has no straight edge (use a profile with lines)"
            )));
        }
    };
    let pt = |p: rcad_sketch::PointId| solved.points.iter().find(|q| q.id == p).map(|q| [q.x, q.y]);
    let lines: Vec<(u32, P, P)> = solved
        .entities
        .iter()
        .filter(|e| !solved.construction.contains(&e.id()))
        .filter_map(|e| match e {
            rcad_sketch::Entity::Line { id, a, b } => Some((id.0, pt(*a)?, pt(*b)?)),
            _ => None,
        })
        .collect();
    let n = segs.len();
    let close = |a: P, b: P| norm(sub(a, b)) <= 1e-9 * (1.0 + norm(a));
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let ent = match segs[i] {
            KSeg::Line { start } => {
                let end = segs[(i + 1) % n].start();
                lines
                    .iter()
                    .find(|(_, a, b)| {
                        (close(*a, start) && close(*b, end)) || (close(*a, end) && close(*b, start))
                    })
                    .map(|l| l.0)
            }
            KSeg::Arc { .. } => None,
        };
        out.push((segs[i].clone(), ent));
    }
    let holes = region
        .holes
        .iter()
        .map(|(p, _)| crate::hole::to_kernel(p))
        .collect();
    Ok((out, holes))
}

/// The flange section in its (outward, up) plane: a closed CCW loop, plus
/// its x extent (min, max).
fn flange_section(rule: &SheetRule, f: &FlangeDef) -> (Vec<KSeg>, f64, f64) {
    let (t, r, l, th) = (rule.thickness, rule.radius, f.length, f.angle);
    let c = [0.0, t + r];
    let on = |rad: f64, phi: f64| [c[0] + rad * phi.sin(), c[1] - rad * phi.cos()];
    let dir = [th.cos(), th.sin()];
    let po = on(r + t, th);
    let pi = on(r, th);
    let po2 = add(po, mul(dir, l));
    let pi2 = add(pi, mul(dir, l));
    let mut segs = vec![
        KSeg::Arc {
            start: [0.0, 0.0],
            center: c,
            ccw: true,
        },
        KSeg::Line { start: po },
        KSeg::Line { start: po2 },
        KSeg::Line { start: pi2 },
        KSeg::Arc {
            start: pi,
            center: c,
            ccw: false,
        },
        KSeg::Line { start: [0.0, t] },
    ];
    let xmax = [
        (r + t) * th.min(std::f64::consts::FRAC_PI_2).sin(),
        po2[0],
        pi2[0],
    ]
    .into_iter()
    .fold(0.0, f64::max);
    let xmin = [po2[0], pi2[0]].into_iter().fold(0.0, f64::min);
    if f.down {
        // Mirror about the plate's mid-plane y = t/2, then restore CCW.
        let m = |p: P| [p[0], t - p[1]];
        segs = segs
            .into_iter()
            .map(|s| match s {
                KSeg::Line { start } => KSeg::Line { start: m(start) },
                KSeg::Arc { start, center, ccw } => KSeg::Arc {
                    start: m(start),
                    center: m(center),
                    ccw: !ccw,
                },
            })
            .collect();
        let n = segs.len();
        let starts: Vec<P> = segs.iter().map(|s| s.start()).collect();
        segs = (0..n)
            .rev()
            .map(|i| {
                let to = starts[(i + 1) % n];
                match segs[i] {
                    KSeg::Line { .. } => KSeg::Line { start: to },
                    KSeg::Arc { center, ccw, .. } => KSeg::Arc {
                        start: to,
                        center,
                        ccw: !ccw,
                    },
                }
            })
            .collect();
    }
    (segs, xmin, xmax)
}

/// Rectangle in sketch coordinates: points A + u·s + n·x for s in [s0, s1],
/// x in [x0, x1].
fn rect(a: P, u: P, n: P, s0: f64, s1: f64, x0: f64, x1: f64) -> [P; 4] {
    let p = |s: f64, x: f64| add(add(a, mul(u, s)), mul(n, x));
    [p(s0, x0), p(s1, x0), p(s1, x1), p(s0, x1)]
}

/// Strict overlap (positive area) of two convex quadrilaterals (SAT).
fn quads_overlap(a: &[P; 4], b: &[P; 4], eps: f64) -> bool {
    for poly in [a, b] {
        for i in 0..4 {
            let e = sub(poly[(i + 1) % 4], poly[i]);
            let ax = [-e[1], e[0]];
            let l = norm(ax);
            if l == 0.0 {
                continue;
            }
            let ax = mul(ax, 1.0 / l);
            let pr = |q: &[P; 4]| {
                q.iter()
                    .map(|p| p[0] * ax[0] + p[1] * ax[1])
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
                        (lo.min(v), hi.max(v))
                    })
            };
            let (a0, a1) = pr(a);
            let (b0, b1) = pr(b);
            if a1 <= b0 + eps || b1 <= a0 + eps {
                return false;
            }
        }
    }
    true
}

/// Whether segment p→q passes through the open box (s0, s1) × (x0, x1) in
/// the (u, n) coordinates about `a` (Liang–Barsky).
#[allow(clippy::too_many_arguments)]
fn seg_hits_box(a: P, u: P, n: P, p: P, q: P, s0: f64, s1: f64, x0: f64, x1: f64) -> bool {
    let loc = |w: P| {
        let d = sub(w, a);
        [d[0] * u[0] + d[1] * u[1], d[0] * n[0] + d[1] * n[1]]
    };
    let (p, q) = (loc(p), loc(q));
    let d = sub(q, p);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (pk, qk) in [
        (-d[0], p[0] - s0),
        (d[0], s1 - p[0]),
        (-d[1], p[1] - x0),
        (d[1], x1 - p[1]),
    ] {
        if pk == 0.0 {
            if qk <= 0.0 {
                return false;
            }
        } else {
            let r = qk / pk;
            if pk < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
        }
    }
    t0 < t1
}

/// Exact folded B-rep and flat pattern of `m`; impossible geometry is refused
/// with the feature responsible and the reason.
pub fn build(m: &SheetModel) -> Result<SheetBuild> {
    let rule = &m.rule;
    rule.validate(m.base)?;
    let t = rule.thickness;
    let n_out = m.outline.len();
    // Resolve every flange to a straight outline segment.
    let mut on_seg: Vec<Vec<(usize, f64, f64)>> = vec![Vec::new(); n_out];
    for (fi, f) in m.flanges.iter().enumerate() {
        let bad = |s: String| DocError::Validation(format!("{}: flange: {s}", f.id));
        if !(f.angle.is_finite() && f.angle > 1e-6 && f.angle <= std::f64::consts::PI + 1e-12) {
            return Err(bad(format!(
                "bend angle must be in (0°, 180°] (180° = open hem), got {:.6}°",
                f.angle.to_degrees()
            )));
        }
        if !(f.length.is_finite() && f.length >= rcad_kernel::tol::MIN_LENGTH) {
            return Err(bad(format!("length must be > 0 mm, got {}", f.length)));
        }
        if !(f.start_offset.is_finite()
            && f.end_offset.is_finite()
            && f.start_offset >= 0.0
            && f.end_offset >= 0.0)
        {
            return Err(bad("offsets must be >= 0 mm".into()));
        }
        let Some(k) = m
            .outline
            .iter()
            .position(|(s, e)| matches!(s, KSeg::Line { .. }) && *e == Some(f.edge))
        else {
            return Err(DocError::BrokenReference {
                from: f.id,
                detail: format!(
                    "sketch line {} is not a straight edge of the base outline of {}",
                    f.edge, m.base
                ),
            });
        };
        let a = m.outline[k].0.start();
        let b = m.outline[(k + 1) % n_out].0.start();
        let len = norm(sub(b, a));
        let (s0, s1) = (f.start_offset, len - f.end_offset);
        if s1 - s0 < rcad_kernel::tol::MIN_LENGTH {
            return Err(bad(format!(
                "the offsets ({} + {} mm) leave no flange on the {len:.6} mm edge",
                f.start_offset, f.end_offset
            )));
        }
        on_seg[k].push((fi, s0, s1));
    }
    let w = rule.relief_width;
    let d = rule.relief_depth;
    let mut folded: Vec<KSeg> = Vec::new();
    let mut flat: Vec<KSeg> = Vec::new();
    let mut bends = Vec::new();
    let mut prisms = Vec::new();
    let mut footprints: Vec<(FeatureId, [P; 4])> = Vec::new();
    let mut strip_areas: Vec<f64> = Vec::new();
    let mut outward: Vec<(FeatureId, P, P, P, f64, f64, f64)> = Vec::new();
    let bottom = if m.flip { m.frame.offset(-t) } else { m.frame };
    let world_dir = |v: P| -> [f64; 3] {
        [
            m.frame.x[0] * v[0] + m.frame.y[0] * v[1],
            m.frame.x[1] * v[0] + m.frame.y[1] * v[1],
            m.frame.x[2] * v[0] + m.frame.y[2] * v[1],
        ]
    };
    // Convexity of the outline at each vertex (CCW: left turn = convex).
    let turn_at = |k: usize| -> f64 {
        // Turn at the start vertex of segment k (from segment k-1 into k),
        // using chord directions (arcs: tangent at the vertex).
        let prev = (k + n_out - 1) % n_out;
        let tin = match m.outline[prev].0 {
            KSeg::Line { start } => sub(m.outline[k].0.start(), start),
            KSeg::Arc { center, ccw, .. } => {
                let r = sub(m.outline[k].0.start(), center);
                if ccw { [-r[1], r[0]] } else { [r[1], -r[0]] }
            }
        };
        let tout = match m.outline[k].0 {
            KSeg::Line { start } => sub(m.outline[(k + 1) % n_out].0.start(), start),
            KSeg::Arc { start, center, ccw } => {
                let r = sub(start, center);
                if ccw { [-r[1], r[0]] } else { [r[1], -r[0]] }
            }
        };
        cross(tin, tout)
    };
    #[allow(clippy::needless_range_loop)] // k also indexes neighbours
    for k in 0..n_out {
        let seg = m.outline[k].0.clone();
        let KSeg::Line { start: a } = seg else {
            folded.push(seg.clone());
            flat.push(seg);
            continue;
        };
        let b = m.outline[(k + 1) % n_out].0.start();
        let len = norm(sub(b, a));
        let u = mul(sub(b, a), 1.0 / len);
        let nn = [u[1], -u[0]]; // outward (right of a CCW outline)
        let p = |s: f64, x: f64| add(add(a, mul(u, s)), mul(nn, x));
        let mut fl = on_seg[k].clone();
        fl.sort_by(|x, y| x.1.total_cmp(&y.1));
        folded.push(KSeg::Line { start: a });
        flat.push(KSeg::Line { start: a });
        let mut cursor = 0.0;
        for &(fi, s0, s1) in &fl {
            let f = &m.flanges[fi];
            let bad = |s: String| DocError::Validation(format!("{}: flange: {s}", f.id));
            let need0 = s0 > 1e-9;
            let need1 = s1 < len - 1e-9;
            if !need0 && turn_at(k) < 0.0 {
                return Err(bad(format!(
                    "the flange reaches a concave corner of the base at its start; set start_offset >= relief width ({w} mm) + relief, or move it"
                )));
            }
            if !need1 && turn_at((k + 1) % n_out) < 0.0 {
                return Err(bad(format!(
                    "the flange reaches a concave corner of the base at its end; set end_offset >= relief width ({w} mm), or move it"
                )));
            }
            let r0 = if need0 { s0 - w } else { s0 };
            if r0 < cursor - 1e-9 || (need0 && r0 < 1e-9) {
                return Err(bad(if r0 < 1e-9 && cursor == 0.0 {
                    format!(
                        "impossible relief: start offset {s0} mm leaves no room for the {w} mm relief (needs > {w} mm, or 0 for no relief)"
                    )
                } else {
                    format!(
                        "flange or relief overlaps another flange on the same edge (starts at {r0:.6} mm, previous ends at {cursor:.6} mm)"
                    )
                }));
            }
            let notch = |from: f64, to: f64, out: &mut Vec<KSeg>| match rule.relief {
                ReliefShape::Rectangular => {
                    out.push(KSeg::Line {
                        start: p(from, 0.0),
                    });
                    out.push(KSeg::Line { start: p(from, -d) });
                    out.push(KSeg::Line { start: p(to, -d) });
                }
                ReliefShape::Obround => {
                    let h = d - (to - from) / 2.0;
                    if h > 1e-9 {
                        out.push(KSeg::Line {
                            start: p(from, 0.0),
                        });
                    }
                    let h = h.max(0.0);
                    out.push(KSeg::Arc {
                        start: p(from, -h),
                        center: p((from + to) / 2.0, -h),
                        ccw: false,
                    });
                    if h > 1e-9 {
                        out.push(KSeg::Line { start: p(to, -h) });
                    }
                }
            };
            if need0 {
                notch(r0, s0, &mut folded);
                notch(r0, s0, &mut flat);
            }
            // Attachment: folded keeps the edge, flat adds the unfolded strip.
            let ba = rule.bend_allowance(f.angle);
            let wflat = ba + f.length;
            strip_areas.push((s1 - s0) * wflat);
            if s0 > 1e-9 {
                folded.push(KSeg::Line { start: p(s0, 0.0) });
                flat.push(KSeg::Line { start: p(s0, 0.0) });
            }
            flat.push(KSeg::Line {
                start: p(s0, wflat),
            });
            flat.push(KSeg::Line {
                start: p(s1, wflat),
            });
            if need1 {
                let r1 = s1 + w;
                if r1 > len - 1e-9 {
                    return Err(bad(format!(
                        "impossible relief: end offset {} mm leaves no room for the {w} mm relief (needs > {w} mm, or 0 for no relief)",
                        f.end_offset
                    )));
                }
                // The notch after the flange, then the edge continues.
                notch(s1, r1, &mut folded);
                notch(s1, r1, &mut flat);
                folded.push(KSeg::Line { start: p(r1, 0.0) });
                flat.push(KSeg::Line { start: p(r1, 0.0) });
                cursor = r1;
            } else {
                cursor = s1;
            }
            bends.push(BendLine {
                flange: f.id,
                from: p(s0, ba / 2.0),
                to: p(s1, ba / 2.0),
                up: !f.down,
                angle_deg: f.angle.to_degrees(),
                inside_radius: rule.radius,
                allowance: ba,
            });
            // Folded flange prism: section at the end point, swept back along −u.
            let (sec, xmin, xmax) = flange_section(rule, f);
            let x = world_dir(nn);
            let y = bottom.z;
            let z = [
                x[1] * y[2] - x[2] * y[1],
                x[2] * y[0] - x[0] * y[2],
                x[0] * y[1] - x[1] * y[0],
            ];
            let o2 = p(s1, 0.0);
            let fr = Frame {
                origin: bottom.to_world(o2),
                x,
                y,
                z,
            };
            let prism =
                kk::extrude_region(&kk::Profile::Loop(sec), &[], &fr, s1 - s0).map_err(|e| {
                    DocError::Kernel {
                        feature: f.id,
                        detail: e.to_string(),
                    }
                })?;
            prisms.push(prism);
            footprints.push((f.id, rect(a, u, nn, s0, s1, xmin, xmax)));
            outward.push((f.id, a, u, nn, s0, s1, xmax));
        }
        let _ = cursor;
    }
    // Remove consecutive duplicate points (a flange starting at a corner).
    let dedup = |v: Vec<KSeg>| -> Vec<KSeg> {
        let mut out: Vec<KSeg> = Vec::with_capacity(v.len());
        for s in v {
            if let Some(last) = out.last()
                && matches!(last, KSeg::Line { .. })
                && norm(sub(last.start(), s.start())) < 1e-12
            {
                out.pop();
            }
            out.push(s);
        }
        while out.len() > 1
            && norm(sub(out[out.len() - 1].start(), out[0].start())) < 1e-12
            && matches!(out[out.len() - 1], KSeg::Line { .. })
        {
            out.pop();
        }
        out
    };
    let folded = dedup(folded);
    let flat = merge_collinear(dedup(flat));
    let who = |fid: Option<FeatureId>| fid.unwrap_or(m.base);
    check_simple(&to_sk(&folded)).map_err(|e| {
        DocError::Validation(format!(
            "{}: a relief cuts through the plate ({e}); use a smaller relief depth",
            who(m.flanges.last().map(|f| f.id))
        ))
    })?;
    check_holes(&to_sk(&folded), &m.holes).map_err(|e| {
        DocError::Validation(format!(
            "{}: a relief cuts into a hole of the plate ({e}); move the flange or use a smaller relief",
            who(m.flanges.last().map(|f| f.id))
        ))
    })?;
    // Flanges against each other and against the plate.
    for i in 0..footprints.len() {
        for j in i + 1..footprints.len() {
            if quads_overlap(&footprints[i].1, &footprints[j].1, 1e-7) {
                return Err(DocError::Validation(format!(
                    "{}: flange overlaps flange {} (their footprints on the base plane intersect); shorten, reduce the angle or offset them",
                    footprints[j].0, footprints[i].0
                )));
            }
        }
    }
    let pl = polyline(&to_sk(&folded));
    for &(fid, a, u, nn, s0, s1, xmax) in &outward {
        let e = 1e-7;
        for i in 0..pl.len() {
            let (p, q) = (pl[i], pl[(i + 1) % pl.len()]);
            if seg_hits_box(a, u, nn, p, q, s0 + e, s1 - e, e, xmax - e) {
                return Err(DocError::Validation(format!(
                    "{fid}: flange runs into the base plate near ({:.3}, {:.3}); flanges must not overlap the sheet",
                    p[0], p[1]
                )));
            }
        }
    }
    check_simple(&to_sk(&flat)).map_err(|e| {
        DocError::Validation(format!(
            "{}: the flat pattern overlaps itself ({e}); flanges collide when unfolded",
            who(m.flanges.last().map(|f| f.id))
        ))
    })?;
    // Review S1: the flat outline must add exactly the unfolded strips to the
    // plate (same arcs on both, so their chord areas cancel).
    let strips: f64 = strip_areas.iter().sum();
    let grown = poly_area(&to_sk(&flat)) - poly_area(&to_sk(&folded));
    if (grown - strips).abs() > 1e-9 * (1.0 + strips.abs()) * 1e3 {
        return Err(DocError::Validation(format!(
            "{}: the flat pattern overlaps itself (unfolded strips overlap by {:.6} mm²); flanges collide when unfolded",
            who(m.flanges.last().map(|f| f.id)),
            strips - grown
        )));
    }
    let plate = kk::extrude_region(&kk::Profile::Loop(folded.clone()), &m.holes, &bottom, t)
        .map_err(|e| DocError::Kernel {
            feature: m.base,
            detail: e.to_string(),
        })?;
    let last = who(m.flanges.last().map(|f| f.id));
    let folded_body = if prisms.is_empty() {
        plate
    } else {
        let mut parts = vec![plate];
        parts.extend(prisms);
        kk::glue(&parts, GLUE_TOL)
            .map_err(|e| DocError::Kernel {
                feature: last,
                detail: e.to_string(),
            })?
            .0
    };
    let pts = polyline(&to_sk(&flat));
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in &pts {
        for i in 0..2 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    Ok(SheetBuild {
        folded: folded_body,
        flat_outline: flat,
        flat_holes: m.holes.clone(),
        bends,
        bottom,
        flat_extent: (lo, hi),
    })
}

/// Drop the vertex between two collinear, same-direction line segments (the
/// flat outline then has one CUT line per straight edge).
fn merge_collinear(mut v: Vec<KSeg>) -> Vec<KSeg> {
    loop {
        let n = v.len();
        if n <= 3 {
            return v;
        }
        let hit = (0..n).find(|&i| {
            let j = (i + 1) % n;
            let (KSeg::Line { start: a }, KSeg::Line { start: b }) = (&v[i], &v[j]) else {
                return false;
            };
            let c = v[(j + 1) % n].start();
            let (d1, d2) = (sub(*b, *a), sub(c, *b));
            let (l1, l2) = (norm(d1), norm(d2));
            l1 > 0.0
                && l2 > 0.0
                && (cross(d1, d2) / (l1 * l2)).abs() < 1e-12
                && d1[0] * d2[0] + d1[1] * d2[1] > 0.0
        });
        match hit {
            Some(i) => {
                v.remove((i + 1) % n);
            }
            None => return v,
        }
    }
}

/// Vertex tolerance for gluing the plate and flange prisms: they are built
/// from the same exact points by different rigid placements, so their shared
/// corners agree to rounding (~1e-13 mm); far below any feature size.
pub const GLUE_TOL: f64 = 1e-7;

/// Exact flat-pattern solid (the flat outline extruded by the thickness from
/// the plate's bottom).
pub fn flat_body(b: &SheetBuild, rule: &SheetRule, id: FeatureId) -> Result<kk::Body> {
    kk::extrude_region(
        &kk::Profile::Loop(b.flat_outline.clone()),
        &b.flat_holes,
        &b.bottom,
        rule.thickness,
    )
    .map_err(|e| DocError::Kernel {
        feature: id,
        detail: e.to_string(),
    })
}

/// Flat pattern as an R12 DXF (mm, `$INSUNITS` 4): outline and holes on
/// layer CUT (LINE / ARC), bend centre lines on BEND_UP or BEND_DOWN, and a
/// TEXT note per bend on BEND_NOTES ("UP 90° R2 BA 3.770").
pub fn flat_dxf(b: &SheetBuild) -> String {
    let mut s = String::new();
    let mut w = |code: i32, v: &str| {
        s.push_str(&format!("{code}\n{v}\n"));
    };
    let f = |v: f64| format!("{v:.9}");
    w(0, "SECTION");
    w(2, "HEADER");
    w(9, "$ACADVER");
    w(1, "AC1009");
    w(9, "$INSUNITS");
    w(70, "4");
    w(0, "ENDSEC");
    w(0, "SECTION");
    w(2, "TABLES");
    w(0, "TABLE");
    w(2, "LAYER");
    w(70, "4");
    for (name, color) in [
        ("CUT", 7),
        ("BEND_UP", 3),
        ("BEND_DOWN", 1),
        ("BEND_NOTES", 2),
    ] {
        w(0, "LAYER");
        w(2, name);
        w(70, "0");
        w(62, &color.to_string());
        w(
            6,
            if name.starts_with("BEND_") && name != "BEND_NOTES" {
                "DASHED"
            } else {
                "CONTINUOUS"
            },
        );
    }
    w(0, "ENDTAB");
    w(0, "ENDSEC");
    w(0, "SECTION");
    w(2, "ENTITIES");
    let mut loops: Vec<Vec<KSeg>> = vec![b.flat_outline.clone()];
    for h in &b.flat_holes {
        match h {
            kk::Profile::Polygon(v) => {
                loops.push(v.iter().map(|p| KSeg::Line { start: *p }).collect())
            }
            kk::Profile::Loop(l) => loops.push(l.clone()),
            kk::Profile::Circle { center, radius } => {
                w(0, "CIRCLE");
                w(8, "CUT");
                w(10, &f(center[0]));
                w(20, &f(center[1]));
                w(30, "0.0");
                w(40, &f(*radius));
            }
        }
    }
    for l in &loops {
        let n = l.len();
        for i in 0..n {
            let a = l[i].start();
            let e = l[(i + 1) % n].start();
            match l[i] {
                KSeg::Line { .. } => {
                    w(0, "LINE");
                    w(8, "CUT");
                    w(10, &f(a[0]));
                    w(20, &f(a[1]));
                    w(30, "0.0");
                    w(11, &f(e[0]));
                    w(21, &f(e[1]));
                    w(31, "0.0");
                }
                KSeg::Arc { center, ccw, .. } => {
                    // DXF arcs run counter-clockwise from start to end angle.
                    let (p, q) = if ccw { (a, e) } else { (e, a) };
                    let ang = |v: P| {
                        let d = (v[1] - center[1]).atan2(v[0] - center[0]).to_degrees();
                        if d < 0.0 { d + 360.0 } else { d }
                    };
                    w(0, "ARC");
                    w(8, "CUT");
                    w(10, &f(center[0]));
                    w(20, &f(center[1]));
                    w(30, "0.0");
                    w(40, &f(norm(sub(a, center))));
                    w(50, &f(ang(p)));
                    w(51, &f(ang(q)));
                }
            }
        }
    }
    for bl in &b.bends {
        let layer = if bl.up { "BEND_UP" } else { "BEND_DOWN" };
        w(0, "LINE");
        w(8, layer);
        w(10, &f(bl.from[0]));
        w(20, &f(bl.from[1]));
        w(30, "0.0");
        w(11, &f(bl.to[0]));
        w(21, &f(bl.to[1]));
        w(31, "0.0");
        let mid = mul(add(bl.from, bl.to), 0.5);
        let dir = sub(bl.to, bl.from);
        w(0, "TEXT");
        w(8, "BEND_NOTES");
        w(10, &f(mid[0]));
        w(20, &f(mid[1]));
        w(30, "0.0");
        w(40, "2.5");
        w(
            1,
            &format!(
                "{} {:.1}%%d R{} BA {:.3}",
                if bl.up { "UP" } else { "DOWN" },
                bl.angle_deg,
                bl.inside_radius,
                bl.allowance
            ),
        );
        w(50, &f(dir[1].atan2(dir[0]).to_degrees()));
    }
    w(0, "ENDSEC");
    w(0, "EOF");
    s
}

/// The sheet model of a base feature.
#[allow(clippy::too_many_arguments)]
pub fn base_model(
    id: FeatureId,
    body: BodyId,
    frame: Frame,
    flip: bool,
    rule: &SheetRule,
    region: &rcad_sketch::Region,
    solved: &rcad_sketch::Sketch,
) -> Result<SheetModel> {
    rule.validate(id)?;
    let (outline, holes) = base_outline(id, region, solved)?;
    Ok(SheetModel {
        base: id,
        body,
        frame,
        flip,
        rule: rule.clone(),
        outline,
        holes,
        flanges: Vec::new(),
        last_feature: id,
        flat: None,
    })
}

/// The flange definition of a `SheetFlange` feature.
pub fn flange_def(id: FeatureId, kind: &crate::FeatureKind) -> Option<FlangeDef> {
    match kind {
        crate::FeatureKind::SheetFlange {
            edge,
            angle,
            length,
            down,
            start_offset,
            end_offset,
            ..
        } => Some(FlangeDef {
            id,
            edge: *edge,
            angle: *angle,
            length: *length,
            down: *down,
            start_offset: *start_offset,
            end_offset: *end_offset,
        }),
        _ => None,
    }
}

/// Regeneration notes of a sheet build (flat size and bends).
pub fn notes(b: &SheetBuild, rule: &SheetRule) -> Vec<String> {
    let (lo, hi) = b.flat_extent;
    let mut v = vec![
        format!(
            "sheet rule: t {} mm, inside radius {} mm, K {} (neutral axis at K·t), {:?} relief {} x {} mm",
            rule.thickness,
            rule.radius,
            rule.k_factor,
            rule.relief,
            rule.relief_width,
            rule.relief_depth
        ),
        format!(
            "flat pattern {:.6} x {:.6} mm (sketch x/y extent), {} bend(s)",
            hi[0] - lo[0],
            hi[1] - lo[1],
            b.bends.len()
        ),
    ];
    for bl in &b.bends {
        v.push(format!(
            "{}: bend {} {:.3}° R {} BA {:.6} mm",
            bl.flange,
            if bl.up { "up" } else { "down" },
            bl.angle_deg,
            bl.inside_radius,
            bl.allowance
        ));
    }
    v
}
