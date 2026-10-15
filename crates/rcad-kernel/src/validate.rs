//! Independent checks of boolean results.
//!
//! Truck 0.6 booleans can return a single closed shell that is still the wrong
//! solid (an engulfed hole's walls left inside a cut; a Join overlap poking into
//! a cavity). These checks never call the boolean algorithm again. They compare
//! the result against the *nominal* operation the user asked for:
//!
//! * Join: `expected = target ∪ prism`; Cut: `expected = target \ prism`, where
//!   the prism is the user's nominal tool (no overlap or margin workarounds),
//!   classified analytically.
//! * Target membership is the generalized winding number of the target's
//!   tessellation (exact for a closed polyhedron; probes are placed farther from
//!   the surface than the chord error).
//!
//! [`verify_prism_boolean`] probes both sides of every result face near the
//! tool: just inside a face must be expected material, just outside must not.
//! A face with expected material on neither side (a stray wall left in a void,
//! the floor of an overlap that leaked into a cavity) or on both sides fails.
//! Interior points of every nominal tool are also classified against the
//! result's own winding number (a tool that was never applied fails). [`clear_depth`] finds how
//! deep the material under a face is free of other surfaces, from the exact
//! planar triangles (and the chord-bounded curved ones), so a Join overlap can
//! be sized to stay inside the material instead of using a fixed depth.
use crate::mesh::tessellate;
use crate::{Body, Frame, KernelError, LoopSegment, Profile, Result, guarded, tol};
use serde::{Deserialize, Serialize};
use truck_modeling::*;

/// Set operation of a prism tool on a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrismOp {
    Join,
    Cut,
}

/// Outcome statistics of a passed check (for logs and tests).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BooleanCheck {
    /// Result faces near the tool that were probed.
    pub faces_probed: usize,
    /// Probe pairs evaluated (inside/outside of a face).
    pub probes: usize,
    /// Probe pairs skipped because a point was too close to a surface to classify.
    pub skipped: usize,
}

/// The nominal prism: `profile` minus its inner loops `holes` (in `start`'s
/// 2D coordinates) swept from `start` by `length` along `start.z`.
pub struct Prism<'a> {
    pub profile: &'a Profile,
    /// Inner loops of the tool region (empty for a single closed loop).
    pub holes: &'a [Profile],
    pub start: &'a Frame,
    pub length: f64,
}

impl Prism<'_> {
    /// The nominal span `(0, length)` along `start.z`.
    pub fn span(&self) -> (f64, f64) {
        (0.0, self.length)
    }
}

/// Chord used for the validation tessellations, relative to the part size.
pub(crate) fn check_chord(diag: f64) -> f64 {
    (diag * 1.0e-4).clamp(1.0e-3, 0.02)
}

fn diag_of(b: &crate::Bounds) -> f64 {
    (0..3)
        .map(|i| (b.max[i] - b.min[i]).powi(2))
        .sum::<f64>()
        .sqrt()
}

// ------------------------------------------------------------------ 2D profile

/// Profile boundary as a closed polygon dense enough that its distance to the
/// true boundary is below `1e-6 * scale` (arcs, circles and cubic spans are
/// subdivided).
fn profile_polygon(p: &Profile) -> Vec<[f64; 2]> {
    fn arc_pts(out: &mut Vec<[f64; 2]>, c: [f64; 2], r: f64, a0: f64, sweep: f64) {
        let sag = (1.0e-6 * r).max(1.0e-9);
        let step = 2.0 * (1.0 - sag / r).clamp(-1.0, 1.0).acos();
        let n = ((sweep.abs() / step.max(1e-6)).ceil() as usize).clamp(8, 20_000);
        for k in 0..n {
            let t = a0 + sweep * k as f64 / n as f64;
            out.push([c[0] + r * t.cos(), c[1] + r * t.sin()]);
        }
    }
    match p {
        Profile::Polygon(v) => v.clone(),
        Profile::Circle { center, radius } => {
            let mut out = Vec::new();
            arc_pts(&mut out, *center, *radius, 0.0, std::f64::consts::TAU);
            out
        }
        Profile::Loop(segs) => {
            let n = segs.len();
            let mut out = Vec::new();
            for (i, s) in segs.iter().enumerate() {
                let (a, b) = (s.start(), segs[(i + 1) % n].start());
                match s {
                    LoopSegment::Line { .. } => out.push(a),
                    LoopSegment::Arc { center, ccw, .. } => {
                        let r = (a[0] - center[0]).hypot(a[1] - center[1]);
                        let sweep = crate::arc_signed_sweep(a, b, *center, *ccw);
                        let a0 = (a[1] - center[1]).atan2(a[0] - center[0]);
                        arc_pts(&mut out, *center, r, a0, sweep);
                    }
                    LoopSegment::Cubic { c1, c2, w, .. } => {
                        // Polyline within 1e-6 × the span's control-polygon size
                        // of the exact curve (convex-hull flattening bound,
                        // `cubic::flatten_into`), the same bound as the arcs.
                        let pts = [a, *c1, *c2, b];
                        let size = pts
                            .iter()
                            .flat_map(|p| pts.iter().map(move |q| (p[0] - q[0]).hypot(p[1] - q[1])))
                            .fold(0.0, f64::max);
                        crate::cubic::flatten_into(&mut out, pts, *w, (1.0e-6 * size).max(1.0e-9));
                    }
                }
            }
            out
        }
    }
}

/// Up to n×n grid points of the polygon's bounding box that lie inside it,
/// away from its boundary (1 % of the box size).
fn interior_samples(poly: &[[f64; 2]], n: usize) -> Vec<[f64; 2]> {
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in poly {
        for i in 0..2 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let size = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    let mut out = Vec::new();
    for i in 0..n {
        for j in 0..n {
            let q = [
                lo[0] + (hi[0] - lo[0]) * (i as f64 + 0.5) / n as f64,
                lo[1] + (hi[1] - lo[1]) * (j as f64 + 0.5) / n as f64,
            ];
            if point_in_polygon(q, poly) && dist_to_polygon(q, poly) > 0.01 * size {
                out.push(q);
            }
        }
    }
    out
}

/// The sample point farthest from the boundary (grid + fan centroids).
fn deepest_point(poly: &[[f64; 2]]) -> Option<([f64; 2], f64)> {
    let n = poly.len();
    let fan = (1..n.saturating_sub(1)).map(|i| {
        let (a, b, c) = (poly[0], poly[i], poly[i + 1]);
        [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0]
    });
    interior_samples(poly, 15)
        .into_iter()
        .chain(fan)
        .filter(|q| point_in_polygon(*q, poly))
        .map(|q| (q, dist_to_polygon(q, poly)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

fn point_in_polygon(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let n = poly.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

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

fn dist_to_polygon(p: [f64; 2], poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| seg_dist(p, poly[i], poly[(i + 1) % n]))
        .fold(f64::INFINITY, f64::min)
}

/// Local coordinates of a world point in `f` (u, v in-plane, w along `f.z`).
fn local(f: &Frame, p: [f64; 3]) -> [f64; 3] {
    let d = [p[0] - f.origin[0], p[1] - f.origin[1], p[2] - f.origin[2]];
    let dot = |a: [f64; 3]| a[0] * d[0] + a[1] * d[1] + a[2] * d[2];
    [dot(f.x), dot(f.y), dot(f.z)]
}

/// Analytic prism membership: `Some(inside)`, or `None` within `eps` of its boundary.
struct PrismTest {
    start: Frame,
    length: f64,
    poly: Vec<[f64; 2]>,
    holes: Vec<Vec<[f64; 2]>>,
    /// Tan of the draft angle; the section at height `w` is the
    /// profile offset inward by `w * taper` (0: a right prism).
    taper: f64,
    /// Tapered sections: the exact base shape.
    base: Section,
}

/// Base shape of a tapered section.
#[derive(Clone)]
enum Section {
    Polygon(Vec<[f64; 2]>),
    Circle([f64; 2], f64),
    /// Not taperable (the check uses the base polygon at every height).
    Other,
}

impl PrismTest {
    fn tapered(p: &Prism, taper_tan: f64) -> Self {
        Self {
            start: *p.start,
            length: p.length,
            poly: profile_polygon(p.profile),
            holes: p.holes.iter().map(profile_polygon).collect(),
            taper: taper_tan,
            base: match p.profile {
                Profile::Polygon(v) => Section::Polygon(v.clone()),
                Profile::Circle { center, radius } => Section::Circle(*center, *radius),
                Profile::Loop(_) => Section::Other,
            },
        }
    }
    /// Section polygon at height `w` (tapered polygons; None when the offset
    /// is degenerate there: an empty section).
    fn section_poly(&self, w: f64) -> Option<std::borrow::Cow<'_, [[f64; 2]]>> {
        if self.taper == 0.0 {
            return Some(std::borrow::Cow::Borrowed(self.poly.as_slice()));
        }
        match &self.base {
            Section::Polygon(v) => {
                crate::taper::offset_polygon(v, w * self.taper).map(std::borrow::Cow::Owned)
            }
            Section::Circle(c, r) => {
                let rr = r - w * self.taper;
                (rr > 0.0).then(|| {
                    std::borrow::Cow::Owned(profile_polygon(&Profile::Circle {
                        center: *c,
                        radius: rr,
                    }))
                })
            }
            Section::Other => Some(std::borrow::Cow::Borrowed(self.poly.as_slice())),
        }
    }
    /// (inside, distance to the boundary) of `q` in the section at `w`.
    fn at(&self, q: [f64; 2], w: f64) -> (bool, f64) {
        if self.taper != 0.0
            && let Section::Circle(c, r) = &self.base
        {
            let rr = r - w * self.taper;
            let d = (q[0] - c[0]).hypot(q[1] - c[1]);
            return (d < rr, (d - rr).abs());
        }
        match self.section_poly(w) {
            Some(poly) => {
                let inside = point_in_polygon(q, &poly)
                    && !self.holes.iter().any(|h| point_in_polygon(q, h));
                let d = self
                    .holes
                    .iter()
                    .map(|h| dist_to_polygon(q, h))
                    .fold(dist_to_polygon(q, &poly), f64::min);
                (inside, d)
            }
            None => (false, f64::INFINITY),
        }
    }
    /// Inside the profile region (outer loop minus holes).
    fn inside(&self, q: [f64; 2]) -> bool {
        point_in_polygon(q, &self.poly) && !self.holes.iter().any(|h| point_in_polygon(q, h))
    }
    /// Distance to the region's boundary (outer loop and holes).
    fn boundary_dist(&self, q: [f64; 2]) -> f64 {
        self.holes
            .iter()
            .map(|h| dist_to_polygon(q, h))
            .fold(dist_to_polygon(q, &self.poly), f64::min)
    }
    /// In-plane sample points of the actual tool region (outer loop minus
    /// holes): a 7 x 7 grid of its bounding box (points inside the region, 1 %
    /// of the size clear of its boundary), plus, for up to 128 boundary edges
    /// spread over all loops, the middle of the chord the inward normal at the
    /// edge midpoint cuts through the region on either side. The chord
    /// midpoints reach rings and slots thinner than the grid spacing or the 1 %
    /// margin.
    fn samples(&self) -> Vec<[f64; 2]> {
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for p in &self.poly {
            for i in 0..2 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        let size = (hi[0] - lo[0]).max(hi[1] - lo[1]);
        let mut out = Vec::new();
        for i in 0..7 {
            for j in 0..7 {
                let q = [
                    lo[0] + (hi[0] - lo[0]) * (i as f64 + 0.5) / 7.0,
                    lo[1] + (hi[1] - lo[1]) * (j as f64 + 0.5) / 7.0,
                ];
                if self.inside(q) && self.boundary_dist(q) > 0.01 * size {
                    out.push(q);
                }
            }
        }
        let loops: Vec<&Vec<[f64; 2]>> = std::iter::once(&self.poly).chain(&self.holes).collect();
        let edges: Vec<([f64; 2], [f64; 2])> = loops
            .iter()
            .flat_map(|l| (0..l.len()).map(move |i| (l[i], l[(i + 1) % l.len()])))
            .collect();
        let n = edges.len();
        let picks = n.min(128);
        for k in 0..picks {
            let (a, b) = edges[k * n / picks.max(1)];
            let d = [b[0] - a[0], b[1] - a[1]];
            let len = d[0].hypot(d[1]);
            if len == 0.0 {
                continue;
            }
            let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            for sgn in [1.0, -1.0] {
                let u = [-sgn * d[1] / len, sgn * d[0] / len];
                // Nearest boundary crossing of the ray m + s u, s > 0.
                let mut best = f64::INFINITY;
                for &(c, e) in &edges {
                    let f = [e[0] - c[0], e[1] - c[1]];
                    let den = u[0] * f[1] - u[1] * f[0];
                    if den.abs() < 1e-300 {
                        continue;
                    }
                    let g = [c[0] - m[0], c[1] - m[1]];
                    let s_ray = (g[0] * f[1] - g[1] * f[0]) / den;
                    let t_seg = (g[0] * u[1] - g[1] * u[0]) / den;
                    if s_ray > 1e-9 * size.max(1.0) && (0.0..=1.0).contains(&t_seg) {
                        best = best.min(s_ray);
                    }
                }
                if best.is_finite() {
                    let q = [m[0] + u[0] * best / 2.0, m[1] + u[1] * best / 2.0];
                    if self.inside(q) {
                        out.push(q);
                    }
                }
            }
        }
        out
    }
    fn classify(&self, x: [f64; 3], eps: f64) -> Option<bool> {
        let [u, v, w] = local(&self.start, x);
        let d_cap = w.abs().min((w - self.length).abs());
        let (inside, d_side) = self.at([u, v], w.clamp(0.0, self.length));
        // A drafted side is inclined: its distance is at most the in-plane one.
        let d_side = d_side * (1.0 + self.taper * self.taper).sqrt().recip();
        if d_cap < eps && (inside || d_side < eps) {
            return None;
        }
        if d_side < eps && w > -eps && w < self.length + eps {
            return None;
        }
        Some(w > 0.0 && w < self.length && inside)
    }
}

// -------------------------------------------------------------- mesh winding

/// Generalized winding number of a closed triangle mesh (1 inside, 0 outside).
struct Winding {
    tris: Vec<[[f64; 3]; 3]>,
    /// Ray-crossing acceleration (closed, consistently oriented meshes only):
    /// one grid per ray axis (+X, +Y, +Z); a ray that grazes an edge or
    /// vertex is retried along the next axis before the exact sum is used.
    grids: Vec<RayGrid>,
}

/// Most triangle entries in a [`RayGrid`] (memory bound).
const MAX_GRID_ENTRIES: usize = 20_000_000;

/// Uniform grid of triangle indices over the two coordinates other than
/// `axis`, for ray casting along +`axis`.
struct RayGrid {
    axis: usize,
    lo: [f64; 2],
    cell: [f64; 2],
    n: usize,
    cells: Vec<Vec<u32>>,
    /// Relative tolerance for "the ray grazes an edge, vertex or plane".
    eps: f64,
}

impl Winding {
    fn new(m: &crate::TriMesh) -> Self {
        let tris: Vec<[[f64; 3]; 3]> = m
            .triangles
            .iter()
            .map(|t| t.map(|i| m.positions[i as usize]))
            .collect();
        // Performance: for a watertight mesh the winding
        // number equals the signed count of +X ray crossings; a grid makes a
        // probe cost O(triangles per cell) instead of O(all triangles).
        let closed = tris.len() >= 256 && crate::check_mesh(m).watertight;
        let build = |axis: usize| -> Option<RayGrid> {
            let ax = [(axis + 1) % 3, (axis + 2) % 3];
            {
                let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
                for t in &tris {
                    for p in t {
                        for k in 0..2 {
                            lo[k] = lo[k].min(p[ax[k]]);
                            hi[k] = hi[k].max(p[ax[k]]);
                        }
                    }
                }
                let n = ((tris.len() as f64).sqrt() as usize / 2).clamp(1, 512);
                let cell = [0, 1].map(|k| ((hi[k] - lo[k]) / n as f64).max(1e-12));
                let mut cells = vec![Vec::new(); n * n];
                // Bounded total size (a triangle is listed in
                // every cell its (y, z) box touches); beyond it, no grid.
                let mut total = 0usize;
                let idx = |v: f64, k: usize| {
                    (((v - lo[k]) / cell[k]).floor().max(0.0) as usize).min(n - 1)
                };
                for (ti, t) in tris.iter().enumerate() {
                    let (mut a, mut b) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
                    for p in t {
                        for k in 0..2 {
                            a[k] = a[k].min(p[ax[k]]);
                            b[k] = b[k].max(p[ax[k]]);
                        }
                    }
                    let (y0, y1, z0, z1) = (idx(a[0], 0), idx(b[0], 0), idx(a[1], 1), idx(b[1], 1));
                    total += (y1 - y0 + 1) * (z1 - z0 + 1);
                    if total > MAX_GRID_ENTRIES {
                        return None;
                    }
                    for y in y0..=y1 {
                        for z in z0..=z1 {
                            cells[y * n + z].push(ti as u32);
                        }
                    }
                }
                let size = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-9);
                Some(RayGrid {
                    axis,
                    lo,
                    cell,
                    n,
                    cells,
                    eps: 1e-9 * size,
                })
            }
        };
        // At most one grid per axis; each is bounded (#26).
        let grids = if closed {
            (0..3).filter_map(build).collect()
        } else {
            Vec::new()
        };
        Self { tris, grids }
    }

    /// Exact solid-angle sum over all triangles.
    fn brute(&self, p: [f64; 3]) -> f64 {
        let mut sum = 0.0;
        for t in &self.tris {
            let a = sub(t[0], p);
            let b = sub(t[1], p);
            let c = sub(t[2], p);
            let (la, lb, lc) = (norm(a), norm(b), norm(c));
            let num = dot(a, cross(b, c));
            let den = la * lb * lc + dot(a, b) * lc + dot(b, c) * la + dot(c, a) * lb;
            sum += 2.0 * num.atan2(den);
        }
        sum / (4.0 * std::f64::consts::PI)
    }

    /// Signed +X ray crossings, or None when the ray grazes a candidate
    /// (then the caller uses the exact sum).
    fn crossings(&self, g: &RayGrid, p: [f64; 3]) -> Option<f64> {
        let ax = [(g.axis + 1) % 3, (g.axis + 2) % 3];
        let (y, z) = (p[ax[0]], p[ax[1]]);
        // Outside the mesh's (y, z) bounds (with margin): no crossing at all.
        let hi = [0, 1].map(|k| g.lo[k] + g.cell[k] * g.n as f64);
        if y < g.lo[0] - g.eps || z < g.lo[1] - g.eps || y > hi[0] + g.eps || z > hi[1] + g.eps {
            return Some(0.0);
        }
        let iy = (((y - g.lo[0]) / g.cell[0]).floor().max(0.0) as usize).min(g.n - 1);
        let iz = (((z - g.lo[1]) / g.cell[1]).floor().max(0.0) as usize).min(g.n - 1);
        // A probe on a cell border could need the neighbouring cell's
        // triangles: use the exact sum there.
        let fy = (y - g.lo[0]) / g.cell[0];
        let fz = (z - g.lo[1]) / g.cell[1];
        let border = |f: f64| (f - f.round()).abs() * g.cell[0].min(g.cell[1]) <= g.eps;
        if border(fy) || border(fz) {
            return None;
        }
        let mut count = 0i64;
        for &ti in &g.cells[iy * g.n + iz] {
            let [a, b, c] = self.tris[ti as usize];
            // Edge functions of (y, z) in the triangle's projection.
            let (i, j) = (ax[0], ax[1]);
            let e =
                |u: [f64; 3], v: [f64; 3]| (v[i] - u[i]) * (z - u[j]) - (v[j] - u[j]) * (y - u[i]);
            let (w0, w1, w2) = (e(b, c), e(c, a), e(a, b));
            let area = w0 + w1 + w2;
            let scale = g.eps * (norm(sub(b, a)) + norm(sub(c, b)) + norm(sub(a, c)));
            if area.abs() <= scale * scale {
                // Triangle seen edge-on by the ray: only a problem if the ray
                // lies in its plane within its extent.
                let (ymin, ymax) = (a[i].min(b[i]).min(c[i]), a[i].max(b[i]).max(c[i]));
                let (zmin, zmax) = (a[j].min(b[j]).min(c[j]), a[j].max(b[j]).max(c[j]));
                if y >= ymin - g.eps && y <= ymax + g.eps && z >= zmin - g.eps && z <= zmax + g.eps
                {
                    return None;
                }
                continue;
            }
            let s = area.signum();
            let (w0, w1, w2) = (w0 * s, w1 * s, w2 * s);
            let tol = scale * area.abs().sqrt().max(scale);
            if w0.min(w1).min(w2) < -tol {
                continue; // the ray misses the projection
            }
            if w0.abs() <= tol || w1.abs() <= tol || w2.abs() <= tol {
                return None; // through (or very near) an edge or vertex
            }
            // Hit point x along the ray.
            let k = g.axis;
            let x = (w0 * a[k] + w1 * b[k] + w2 * c[k]) / (w0 + w1 + w2);
            if (x - p[k]).abs() <= g.eps {
                return None; // the probe lies on the triangle
            }
            if x > p[k] {
                // Normal x component: positive = the ray exits the solid.
                count += if area > 0.0 { 1 } else { -1 };
            }
        }
        Some(count as f64)
    }

    fn number(&self, p: [f64; 3]) -> f64 {
        for g in &self.grids {
            if let Some(w) = self.crossings(g, p) {
                return w;
            }
        }
        self.brute(p)
    }
    /// `Some(inside)`, or `None` when the winding number is not near 0 or 1.
    fn classify(&self, p: [f64; 3]) -> Option<bool> {
        let w = self.number(p);
        if (w - 1.0).abs() < 0.1 {
            Some(true)
        } else if w.abs() < 0.1 {
            Some(false)
        } else {
            None
        }
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

/// Whether B-rep face `i` of `body` (in `face_iter` order, the tessellation's
/// `tri_face`) is planar: its triangles lie exactly on the surface.
fn planar_faces(body: &Body) -> Vec<bool> {
    body.solid
        .face_iter()
        .map(|f| matches!(f.surface(), Surface::Plane(_)))
        .collect()
}

// ------------------------------------------------------------ result check

/// Check that `result` is `target` joined with / cut by the nominal `prism`
/// (see the module docs). `used` is the span `(a, b)` along `prism.start.z`
/// of the tool actually handed to the kernel (with any overlap or margin): faces
/// of the result near it are probed, the rest are untouched target faces.
/// `Err(InvalidResult)` names the failing evidence.
pub fn verify_prism_boolean(
    target: &Body,
    result: &Body,
    op: PrismOp,
    prism: &Prism,
    used: (f64, f64),
) -> Result<BooleanCheck> {
    verify_prisms_boolean(target, result, op, &[(prism, used)])
}

/// [`verify_prism_boolean`] for several tools applied together (a pattern or
/// mirror): `expected = target ∪ ⋃ prisms` (Join) or `target \ ⋃ prisms`
/// (Cut). Faces near any tool actually used are probed.
pub fn verify_prisms_boolean(
    target: &Body,
    result: &Body,
    op: PrismOp,
    tools: &[(&Prism, (f64, f64))],
) -> Result<BooleanCheck> {
    verify_impl(target, result, op, tools, 0.0, None).map(|(c, _)| c)
}

/// The check tessellation of a body (chord, closed mesh), reusable as the
/// target mesh of the next check on the same, unchanged body.
pub type CheckMesh = std::sync::Arc<(f64, crate::TriMesh)>;

/// [`verify_prism_boolean`] that reuses `target_mesh` (the check tessellation
/// of exactly this `target`, e.g. returned by the previous check whose result
/// is this target) when its chord matches, and returns the result's check
/// tessellation for the next one. Same verdict as [`verify_prism_boolean`]:
/// the tessellation is a deterministic function of the body and the chord.
pub fn verify_prism_boolean_reusing(
    target: &Body,
    target_mesh: Option<&CheckMesh>,
    result: &Body,
    op: PrismOp,
    prism: &Prism,
    used: (f64, f64),
) -> Result<(BooleanCheck, CheckMesh)> {
    verify_impl(target, result, op, &[(prism, used)], 0.0, target_mesh)
}

/// [`verify_prism_boolean`] for a drafted tool (`crate::taper`): the
/// section at height `w` along `prism.start.z` is `prism.profile` offset
/// inward by `w * tan(taper)` (polygons mitred, circles by radius). `used` as
/// for [`verify_prism_boolean`].
pub fn verify_tapered_boolean(
    target: &Body,
    result: &Body,
    op: PrismOp,
    prism: &Prism,
    taper: f64,
    used: (f64, f64),
) -> Result<BooleanCheck> {
    if !(taper.is_finite() && taper.abs() < 89f64.to_radians()) {
        return Err(KernelError::InvalidInput(
            "result check: draft angle must be below 89 degrees".into(),
        ));
    }
    if matches!(prism.profile, Profile::Loop(_)) && taper != 0.0 {
        return Err(KernelError::InvalidInput(
            "result check: tapered line/arc loops are not supported".into(),
        ));
    }
    verify_impl(target, result, op, &[(prism, used)], taper.tan(), None).map(|(c, _)| c)
}

fn verify_impl(
    target: &Body,
    result: &Body,
    op: PrismOp,
    tools: &[(&Prism, (f64, f64))],
    taper_tan: f64,
    target_mesh: Option<&CheckMesh>,
) -> Result<(BooleanCheck, CheckMesh)> {
    if tools.is_empty() {
        return Err(KernelError::InvalidInput("result check: no tools".into()));
    }
    let tb = crate::measure::raw_bounds(&target.solid)?;
    let rb = crate::measure::raw_bounds(&result.solid)?;
    let diag = diag_of(&tb).max(diag_of(&rb));
    let chord = check_chord(diag);
    let reused = target_mesh.filter(|m| m.0 == chord);
    let owned;
    let tmesh = match reused {
        Some(m) => &m.1,
        None => {
            owned = tessellate(target, chord)?;
            &owned
        }
    };
    let rmesh = tessellate(result, chord)?;
    let tcheck = crate::check_mesh(tmesh);
    let rcheck = crate::check_mesh(&rmesh);
    if !rcheck.watertight {
        return Err(KernelError::InvalidResult(format!(
            "result check: the result's tessellation is not closed ({} boundary, {} \
             non-manifold, {} misoriented edges)",
            rcheck.boundary_edges, rcheck.nonmanifold_edges, rcheck.misoriented_edges
        )));
    }
    if !tcheck.watertight {
        // The target's own tessellation must be closed for the winding number.
        return Err(KernelError::InvalidResult(
            "result check: the target's tessellation is not closed; cannot verify".into(),
        ));
    }
    let c = guarded("result check", || {
        check_faces(
            target, result, op, tools, taper_tan, tmesh, &rmesh, chord, diag,
        )
    })??;
    Ok((c, std::sync::Arc::new((chord, rmesh))))
}

#[allow(clippy::too_many_arguments)]
fn check_faces(
    target: &Body,
    result: &Body,
    op: PrismOp,
    tools: &[(&Prism, (f64, f64))],
    taper_tan: f64,
    tmesh: &crate::TriMesh,
    rmesh: &crate::TriMesh,
    chord: f64,
    diag: f64,
) -> Result<BooleanCheck> {
    let tests: Vec<PrismTest> = tools
        .iter()
        .map(|(p, _)| PrismTest::tapered(p, taper_tan))
        .collect();
    let wind = Winding::new(tmesh);
    let planar = planar_faces(result);
    // Probe offsets: exact faces need only clear float noise; curved faces must
    // clear the chord error of both tessellations.
    let h_plane = (1.0e-6 * diag).max(20.0 * tol::KERNEL_LINEAR);
    let h_curved = 3.0 * chord;
    let eps_tool = 0.5 * h_plane;
    // Neighbourhood of the tools actually used: faces outside it are untouched
    // target faces.
    let tool_boxes: Vec<([f64; 3], [f64; 3])> = tools
        .iter()
        .zip(&tests)
        .map(|((prism, used), tool)| {
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            for w in [used.0.min(0.0), used.1.max(prism.length)] {
                // Tapered: the section at that end (the widest one bounds it).
                let sect = match &tool.base {
                    Section::Circle(c, r) if tool.taper != 0.0 => {
                        profile_polygon(&Profile::Circle {
                            center: *c,
                            radius: (r - w * tool.taper).max(0.0) + 1e-9,
                        })
                    }
                    _ => tool
                        .section_poly(w)
                        .map_or_else(|| tool.poly.clone(), |c| c.into_owned()),
                };
                for q in sect.iter().chain(&tool.poly) {
                    let p = prism.start.offset(w).to_world(*q);
                    for i in 0..3 {
                        lo[i] = lo[i].min(p[i]);
                        hi[i] = hi[i].max(p[i]);
                    }
                }
            }
            let m = (0.02 * diag).max(1.0);
            (lo.map(|v| v - m), hi.map(|v| v + m))
        })
        .collect();
    let near_tool = |c: [f64; 3]| {
        tool_boxes
            .iter()
            .any(|(lo, hi)| (0..3).all(|i| c[i] >= lo[i] && c[i] <= hi[i]))
    };
    // Union of the tools: inside any → inside; ambiguous near one and not
    // inside another → unclassified.
    let in_tools_eps = |p: [f64; 3], eps: f64| -> Option<bool> {
        let mut unknown = false;
        for t in &tests {
            match t.classify(p, eps) {
                Some(true) => return Some(true),
                Some(false) => {}
                None => unknown = true,
            }
        }
        (!unknown).then_some(false)
    };
    let expected_eps = |p: [f64; 3], eps: f64| -> Option<bool> {
        let in_tool = in_tools_eps(p, eps)?;
        match (op, in_tool) {
            (PrismOp::Join, true) => Some(true),
            (PrismOp::Cut, true) => Some(false),
            _ => wind.classify(p),
        }
    };
    let expected = |p: [f64; 3]| expected_eps(p, eps_tool);
    // Per face: the triangles (largest first) to probe.
    let nfaces = planar.len();
    let mut by_face: Vec<Vec<(f64, usize)>> = vec![Vec::new(); nfaces];
    for (ti, t) in rmesh.triangles.iter().enumerate() {
        let [a, b, c] = t.map(|i| rmesh.positions[i as usize]);
        let n = cross(sub(b, a), sub(c, a));
        let area2 = norm(n);
        let fi = rmesh.tri_face.get(ti).copied().unwrap_or(0) as usize;
        if fi < nfaces && area2 > 0.0 {
            by_face[fi].push((area2, ti));
        }
    }
    let mut stats = BooleanCheck::default();
    for (fi, tris) in by_face.iter_mut().enumerate() {
        tris.sort_by(|x, y| y.0.total_cmp(&x.0));
        let mut face_probed = false;
        let mut decided = 0;
        for &(area2, ti) in tris.iter().take(24) {
            if decided >= 3 {
                break;
            }
            let t = rmesh.triangles[ti];
            let [a, b, c] = t.map(|i| rmesh.positions[i as usize]);
            let centroid = [
                (a[0] + b[0] + c[0]) / 3.0,
                (a[1] + b[1] + c[1]) / 3.0,
                (a[2] + b[2] + c[2]) / 3.0,
            ];
            if !near_tool(centroid) {
                continue;
            }
            let n = cross(sub(b, a), sub(c, a));
            let n = [n[0] / area2, n[1] / area2, n[2] / area2];
            let h = if planar[fi] { h_plane } else { h_curved };
            // The triangle must be wide enough that the probes stay over it.
            let inr = area2 / (norm(sub(b, a)) + norm(sub(c, b)) + norm(sub(a, c)));
            if inr < 2.0 * h {
                continue;
            }
            face_probed = true;
            stats.probes += 1;
            let inside = [
                centroid[0] - h * n[0],
                centroid[1] - h * n[1],
                centroid[2] - h * n[2],
            ];
            let outside = [
                centroid[0] + h * n[0],
                centroid[1] + h * n[1],
                centroid[2] + h * n[2],
            ];
            match (expected(inside), expected(outside)) {
                (Some(true), Some(false)) => decided += 1,
                (Some(i), Some(o)) => {
                    return Err(KernelError::InvalidResult(format!(
                        "result check: a face of the result near {} does not bound the \
                         expected solid ({} on its material side, {} on its open side); the \
                         kernel returned a wrong {} result",
                        fmt_pt(centroid),
                        if i { "material" } else { "no material" },
                        if o { "material" } else { "no material" },
                        match op {
                            PrismOp::Join => "join",
                            PrismOp::Cut => "cut",
                        }
                    )));
                }
                _ => stats.skipped += 1,
            }
        }
        if face_probed {
            stats.faces_probed += 1;
        }
    }
    // A check where no face probe could be decided proves nothing.
    if stats.probes > 0 && stats.probes == stats.skipped {
        return Err(KernelError::InvalidResult(
            "result check: no probe point near the tool could be classified; cannot verify".into(),
        ));
    }
    let rwind = Winding::new(rmesh);
    // Target faces near the tools, probed on both sides against the result's
    // own winding number: a boolean that refills an existing void (a
    // shallow pocket beside a tangent cut lost its floor and walls) or removes
    // untouched material leaves no result face there for the probes above.
    let tplanar = planar_faces(target);
    let near_tri = |t: &[[f64; 3]; 3]| {
        tool_boxes.iter().any(|(lo, hi)| {
            (0..3).all(|i| {
                let mn = t.iter().map(|p| p[i]).fold(f64::INFINITY, f64::min);
                let mx = t.iter().map(|p| p[i]).fold(f64::NEG_INFINITY, f64::max);
                mx >= lo[i] && mn <= hi[i]
            })
        })
    };
    let tfaces = tplanar.len();
    let mut tby_face: Vec<Vec<(f64, usize)>> = vec![Vec::new(); tfaces];
    for (ti, t) in tmesh.triangles.iter().enumerate() {
        let [a, b, c] = t.map(|i| tmesh.positions[i as usize]);
        let area2 = norm(cross(sub(b, a), sub(c, a)));
        let fi = tmesh.tri_face.get(ti).copied().unwrap_or(0) as usize;
        if fi < tfaces && area2 > 0.0 && near_tri(&[a, b, c]) {
            tby_face[fi].push((area2, ti));
        }
    }
    for (fi, tris) in tby_face.iter_mut().enumerate() {
        tris.sort_by(|x, y| y.0.total_cmp(&x.0));
        let mut decided = 0;
        for &(area2, ti) in tris.iter().take(24) {
            if decided >= 3 {
                break;
            }
            let [a, b, c] = tmesh.triangles[ti].map(|i| tmesh.positions[i as usize]);
            let h = if tplanar[fi] { h_plane } else { h_curved };
            let inr = area2 / (norm(sub(b, a)) + norm(sub(c, b)) + norm(sub(a, c)));
            if inr < 2.0 * h {
                continue;
            }
            let n = cross(sub(b, a), sub(c, a));
            let n = [n[0] / area2, n[1] / area2, n[2] / area2];
            let centroid = [
                (a[0] + b[0] + c[0]) / 3.0,
                (a[1] + b[1] + c[1]) / 3.0,
                (a[2] + b[2] + c[2]) / 3.0,
            ];
            let mut both = true;
            for sgn in [-1.0, 1.0] {
                let p = [
                    centroid[0] + sgn * h * n[0],
                    centroid[1] + sgn * h * n[1],
                    centroid[2] + sgn * h * n[2],
                ];
                // A target face may cross the tool's wall: probes within the
                // tessellation's chord error of a curved tool wall (h_curved)
                // are not classified, as the result's mesh of that wall is
                // only that accurate.
                match (expected_eps(p, eps_tool.max(h_curved)), rwind.classify(p)) {
                    (Some(e), Some(r)) if e != r => {
                        return Err(KernelError::InvalidResult(format!(
                            "result check: beside a face of the target near {} the result has {} \
                             where the expected solid has {} (existing geometry away from the \
                             tool was changed); the kernel returned a wrong {} result",
                            fmt_pt(centroid),
                            if r { "material" } else { "no material" },
                            if e { "material" } else { "none" },
                            match op {
                                PrismOp::Join => "join",
                                PrismOp::Cut => "cut",
                            }
                        )));
                    }
                    (Some(_), Some(_)) => {}
                    _ => both = false,
                }
            }
            stats.probes += 1;
            if both {
                decided += 1;
            } else {
                stats.skipped += 1;
            }
        }
    }
    // Interior probes of every nominal tool against the result itself: a tool
    // the kernel never applied leaves no faces to probe above. Cut: points of
    // tool ∩ target must be outside the result; Join: points of the tool must
    // be inside it.
    for (k, ((prism, _), test)) in tools.iter().zip(&tests).enumerate() {
        let mut decided = 0;
        let mut considered = 0;
        for q in test.samples() {
            for frac in [0.2, 0.5, 0.8] {
                let p = prism.start.offset(frac * prism.length).to_world(q);
                if test.classify(p, eps_tool) != Some(true) {
                    continue;
                }
                let in_target = match wind.classify(p) {
                    Some(v) => v,
                    None => {
                        // Near a target surface: the tool reaches the target here.
                        considered += 1;
                        continue;
                    }
                };
                if op == PrismOp::Cut && !in_target {
                    continue;
                }
                considered += 1;
                let Some(in_result) = rwind.classify(p) else {
                    continue;
                };
                let want = op == PrismOp::Join;
                if in_result != want {
                    return Err(KernelError::InvalidResult(format!(
                        "result check: inside tool {} at {} the result has {} (the {} was not \
                         applied there)",
                        k + 1,
                        fmt_pt(p),
                        if in_result { "material" } else { "no material" },
                        if want { "join" } else { "cut" },
                    )));
                }
                decided += 1;
                stats.probes += 1;
            }
        }
        // The tool overlaps the target (or reaches its surface) but no interior
        // point could be decided: no evidence, not a pass.
        if considered > 0 && decided == 0 {
            return Err(KernelError::InvalidResult(format!(
                "result check: no interior point of tool {} could be classified in the result; \
                 cannot verify",
                k + 1
            )));
        }
    }
    Ok(stats)
}

fn fmt_pt(p: [f64; 3]) -> String {
    format!("({:.3}, {:.3}, {:.3}) mm", p[0], p[1], p[2])
}

// ------------------------------------------------------------- clear depth

/// Depth (mm, ≥ 0) below the plane of `face` (z = outward normal; material on
/// the -z side) to which the prism of `profile` is free of any other surface of
/// `body`, capped at `max`. A Join overlap of up to this depth into the support
/// stays inside the material. Planar faces are exact; curved faces are
/// tessellated and their chord error is subtracted, and their triangles are
/// ignored within twice the chord of the profile boundary (a wall that shares
/// the boundary, e.g. a cylinder stacked on a cylinder).
pub fn clear_depth(body: &Body, face: &Frame, profile: &Profile, max: f64) -> Result<f64> {
    let b = crate::measure::raw_bounds(&body.solid)?;
    let diag = diag_of(&b);
    let chord = check_chord(diag);
    let mesh = tessellate(body, chord)?;
    let planar = planar_faces(body);
    let poly = profile_polygon(profile);
    let (mut fp_lo, mut fp_hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in &poly {
        for i in 0..2 {
            fp_lo[i] = fp_lo[i].min(p[i]);
            fp_hi[i] = fp_hi[i].max(p[i]);
        }
    }
    guarded("clear depth", || {
        let w_min = (1.0e-7 * diag).max(10.0 * tol::KERNEL_LINEAR);
        let mut best = max;
        for (ti, t) in mesh.triangles.iter().enumerate() {
            let fi = mesh.tri_face.get(ti).copied().unwrap_or(0) as usize;
            let curved = !planar.get(fi).copied().unwrap_or(false);
            let slack = if curved {
                2.0 * chord
            } else {
                (1.0e-7 * diag).max(10.0 * tol::KERNEL_LINEAR)
            };
            // Depth below the face: w = -z.
            let pts: Vec<[f64; 3]> = t
                .iter()
                .map(|&i| {
                    let l = local(face, mesh.positions[i as usize]);
                    [l[0], l[1], -l[2]]
                })
                .collect();
            if pts.iter().all(|p| p[2] < w_min) || pts.iter().all(|p| p[2] >= best) {
                continue;
            }
            // Footprint bounding-box reject.
            if pts.iter().all(|p| p[0] < fp_lo[0] - slack)
                || pts.iter().all(|p| p[0] > fp_hi[0] + slack)
                || pts.iter().all(|p| p[1] < fp_lo[1] - slack)
                || pts.iter().all(|p| p[1] > fp_hi[1] + slack)
            {
                continue;
            }
            let clipped = clip_below(&pts, w_min);
            if clipped.is_empty() {
                continue;
            }
            if let Some(d) = invasion_depth(&clipped, &poly, slack) {
                let d = if curved { (d - chord).max(0.0) } else { d };
                best = best.min(d);
            }
        }
        Ok(best.max(0.0))
    })?
}

/// Clip a convex polygon (u, v, w) to `w >= w_min`.
fn clip_below(p: &[[f64; 3]], w_min: f64) -> Vec<[f64; 3]> {
    let n = p.len();
    let mut out = Vec::with_capacity(n + 1);
    for i in 0..n {
        let (a, b) = (p[i], p[(i + 1) % n]);
        let (ia, ib) = (a[2] >= w_min, b[2] >= w_min);
        if ia {
            out.push(a);
        }
        if ia != ib {
            let t = (w_min - a[2]) / (b[2] - a[2]);
            out.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1]), w_min]);
        }
    }
    out
}

/// Smallest depth `w` of the part of the convex 3D polygon `p` whose (u, v)
/// projection lies strictly inside `poly` (by more than `slack`), if any.
fn invasion_depth(p: &[[f64; 3]], poly: &[[f64; 2]], slack: f64) -> Option<f64> {
    let mut best: Option<f64> = None;
    let mut take = |w: f64| best = Some(best.map_or(w, |b: f64| b.min(w)));
    let strictly_inside =
        |q: [f64; 2]| point_in_polygon(q, poly) && dist_to_polygon(q, poly) > slack;
    // (a) polygon vertices inside the footprint.
    for v in p {
        if strictly_inside([v[0], v[1]]) {
            take(v[2]);
        }
    }
    // (b) proper crossings of polygon edges with the footprint boundary.
    let n = p.len();
    let m = poly.len();
    for i in 0..n {
        let (a, b) = (p[i], p[(i + 1) % n]);
        let d = [b[0] - a[0], b[1] - a[1]];
        if d[0].hypot(d[1]) <= slack {
            continue;
        }
        for j in 0..m {
            let (c, e) = (poly[j], poly[(j + 1) % m]);
            let f = [e[0] - c[0], e[1] - c[1]];
            let den = d[0] * f[1] - d[1] * f[0];
            if den.abs() < 1e-15 {
                continue;
            }
            let g = [c[0] - a[0], c[1] - a[1]];
            let t = (g[0] * f[1] - g[1] * f[0]) / den;
            let s = (g[0] * d[1] - g[1] * d[0]) / den;
            let tl = slack / d[0].hypot(d[1]);
            if t > tl && t < 1.0 - tl && (0.0..=1.0).contains(&s) {
                // The edge passes into the interior: probe just past the crossing.
                let q = |t: f64| [a[0] + t * d[0], a[1] + t * d[1]];
                let inward =
                    strictly_inside(q((t + tl).min(1.0))) || strictly_inside(q((t - tl).max(0.0)));
                if inward {
                    take(a[2] + t * (b[2] - a[2]));
                }
            }
        }
    }
    // (c) footprint vertices inside the polygon's projection (non-degenerate).
    if n >= 3 {
        let a = p[0];
        // Newell normal: robust to repeated vertices from clipping.
        let mut nrm = [0.0; 3];
        for i in 0..n {
            let (u, v) = (p[i], p[(i + 1) % n]);
            nrm[0] += (u[1] - v[1]) * (u[2] + v[2]);
            nrm[1] += (u[2] - v[2]) * (u[0] + v[0]);
            nrm[2] += (u[0] - v[0]) * (u[1] + v[1]);
        }
        if nrm[2].abs() > 1e-12 * norm(nrm).max(1e-300) {
            let proj: Vec<[f64; 2]> = p.iter().map(|v| [v[0], v[1]]).collect();
            for q in poly {
                if point_in_polygon(*q, &proj) {
                    // Plane through a with normal nrm: w at (u, v).
                    let w = a[2] - (nrm[0] * (q[0] - a[0]) + nrm[1] * (q[1] - a[1])) / nrm[2];
                    take(w);
                }
            }
        }
    }
    best
}

#[cfg(test)]
mod tests;

/// Point membership of `points` in `body` (`Some(inside)`, `None` when a point
/// is too close to the surface to classify), from the generalized winding
/// number of a fine tessellation. For inspection probes and tests; points must
/// be farther than the chord (≤ 0.02 mm, relative to the part size) from the
/// surface to be classified reliably.
pub fn point_membership(body: &Body, points: &[[f64; 3]]) -> Result<Vec<Option<bool>>> {
    let b = crate::measure::raw_bounds(&body.solid)?;
    let chord = check_chord(diag_of(&b));
    let mesh = tessellate(body, chord)?;
    if !crate::check_mesh(&mesh).watertight {
        return Err(KernelError::InvalidResult(
            "point membership: the tessellation is not closed".into(),
        ));
    }
    guarded("point membership", || {
        let w = Winding::new(&mesh);
        points.iter().map(|p| w.classify(*p)).collect()
    })
}

/// Whether the prism of `profile` from the plane of `plane` to `depth` beyond
/// it (`beyond` = +1/-1 along `plane.z`) contains no material of `body`: no
/// surface of `body` crosses it ([`clear_depth`]; surfaces lying in the plane
/// itself do not count) and a point inside it is outside the body. Never
/// calls a boolean.
pub fn prism_is_empty(
    body: &Body,
    plane: &Frame,
    profile: &Profile,
    beyond: f64,
    depth: f64,
) -> Result<bool> {
    // clear_depth looks along -face.z.
    let face = Frame {
        z: [
            -beyond * plane.z[0],
            -beyond * plane.z[1],
            -beyond * plane.z[2],
        ],
        ..*plane
    };
    if clear_depth(body, &face, profile, depth)? < depth {
        return Ok(false);
    }
    let poly = profile_polygon(profile);
    // The deepest interior point, farther from the boundary than the chord
    // error of the membership test.
    let b = crate::measure::raw_bounds(&body.solid)?;
    let chord = check_chord(diag_of(&b));
    let Some((q, d)) = deepest_point(&poly) else {
        return Ok(false);
    };
    if d <= 3.0 * chord {
        return Ok(false);
    }
    let w = plane.offset(beyond * depth / 2.0).to_world(q);
    Ok(point_membership(body, &[w])?.first().copied().flatten() == Some(false))
}
