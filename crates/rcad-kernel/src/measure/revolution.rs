//! Exact boundary-integral volume for bodies whose curved faces are surfaces of
//! revolution about one common axis (cones, cylinders, tori: lofted frusta, revolved
//! and swept bodies) plus circular cylinders about any other axes (extrusions, holes,
//! the straight parts of a sweep). See [`VolumeMethod::BrepBoundaryIntegralRevolution`].
//!
//! Divergence theorem with the radial field F = (p - o)_perp / 2 about the axis (o, a)
//! (div F = 1): V = sum over faces of the flux of F. Each face's flux is reduced to a
//! line integral over its exact boundary edges:
//!
//! * planar face: F . n is linear in p, so the flux is a combination of the face's
//!   signed area and first moments (Green's theorem, as for the axis methods);
//! * surface of revolution about a, meridian (rho(z), z) a line or a circle: in
//!   (meridian parameter, azimuth phi) the flux 2-form is -rho^2/2 dz ^ dphi = d(h dphi)
//!   with h = -1/2 int rho^2 dz along the meridian (closed form for lines and
//!   circles), so the flux is the boundary integral of h dphi (Stokes);
//! * circular cylinder about any axis (c, d), radius R: with s along d and psi the
//!   angle about it, the 2-form is d(G(s, psi) dpsi) with G quadratic in s and periodic
//!   in psi (closed form below).
//!
//! The boundary integrals are chord (trapezoid) sums on exact edge points with
//! Richardson extrapolation, like the axis methods; along meridians and cylinder
//! rulings the azimuth is constant and contributes nothing, and along the circles of a
//! revolution h is constant, so for untrimmed revolution faces the sums are exact.
//! Faces are trimmed arbitrarily (booleans): Stokes holds for any trim provided the
//! primitive is a smooth single-valued 1-form on the face, which the classification
//! below guarantees (or refuses).
use super::{EVAL_FAIL, eval_curve, wire_moments_by};
use truck_modeling::*;

type Res<T> = std::result::Result<T, String>;

/// Relative tolerance (of the body's size) for recognising a surface as an exact plane,
/// cone, torus or cylinder from samples of its own evaluation.
const SHAPE_REL: f64 = 1e-9;
/// Samples per parameter direction for the classification.
const GRID: usize = 12;

/// The integration axis (o, a) with an orthonormal frame (e1, e2, a).
#[derive(Clone, Copy)]
struct AxisFrame {
    o: Point3,
    a: Vector3,
    e1: Vector3,
    e2: Vector3,
}

fn perp_frame(a: Vector3) -> (Vector3, Vector3) {
    let t = if a.x.abs() < 0.6 {
        Vector3::unit_x()
    } else {
        Vector3::unit_y()
    };
    let e1 = (t - a * t.dot(a)).normalize();
    (e1, a.cross(e1))
}

impl AxisFrame {
    fn new(o: Point3, a: Vector3) -> Self {
        let a = a.normalize();
        let (e1, e2) = perp_frame(a);
        AxisFrame { o, a, e1, e2 }
    }
    /// (rho, z, phi) of p.
    fn cyl(&self, p: Point3) -> (f64, f64, f64) {
        let w = p - self.o;
        let (x, y) = (w.dot(self.e1), w.dot(self.e2));
        (x.hypot(y), w.dot(self.a), y.atan2(x))
    }
}

/// Meridian of a face of revolution about the integration axis, in (rho, z).
#[derive(Clone, Copy)]
enum Meridian {
    /// z constant (annulus): dz = 0, the flux is zero.
    Flat,
    /// rho = rho0 + k (z - z0); (rho0, z0) is the apex (rho0 = 0) when the face
    /// reaches the axis, so h vanishes there.
    Line { rho0: f64, z0: f64, k: f64 },
    /// (rho - rc)^2 + (z - zc)^2 = r^2, theta measured from +rho towards +z;
    /// `theta_ref` picks the branch (the face never reaches the axis).
    Circle {
        rc: f64,
        zc: f64,
        r: f64,
        theta_ref: f64,
    },
}

impl Meridian {
    /// h(p) = -1/2 int rho^2 dz along the meridian (a fixed antiderivative).
    fn h(&self, rho: f64, z: f64) -> f64 {
        match *self {
            Meridian::Flat => 0.0,
            Meridian::Line { rho0, z0, k } => {
                let d = z - z0;
                -0.5 * (rho0 * rho0 * d + rho0 * k * d * d + k * k * d * d * d / 3.0)
            }
            Meridian::Circle {
                rc,
                zc,
                r,
                theta_ref,
            } => {
                let t = wrap_near((z - zc).atan2(rho - rc), theta_ref);
                let (s, c) = t.sin_cos();
                // int (rc + r cos t)^2 r cos t dt
                let i = rc * rc * r * s
                    + 2.0 * rc * r * r * (t / 2.0 + s * c / 2.0)
                    + r * r * r * (s - s * s * s / 3.0);
                -0.5 * i
            }
        }
    }
}

/// `t` shifted by a multiple of 2 pi into (r - pi, r + pi].
fn wrap_near(t: f64, r: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    let mut d = (t - r) % TAU;
    if d > PI {
        d -= TAU;
    } else if d <= -PI {
        d += TAU;
    }
    r + d
}

fn wrap_delta(d: f64) -> f64 {
    wrap_near(d, 0.0)
}

/// Circular cylinder: axis point c, unit direction d, frame (e1, e2, d), radius r.
#[derive(Clone, Copy)]
struct Cylinder {
    c: Point3,
    d: Vector3,
    e1: Vector3,
    e2: Vector3,
    r: f64,
}

/// How one face's flux is evaluated.
enum Plan {
    Plane {
        o: Point3,
        e1: Vector3,
        e2: Vector3,
        n: Vector3,
    },
    Revolution(Meridian),
    Cylinder(Cylinder),
}

fn bounded(s: &Surface) -> Option<(f64, f64, f64, f64)> {
    let ((a0, a1), (b0, b1)) = s.parameter_range();
    Some((
        super::bound(a0)?,
        super::bound(a1)?,
        super::bound(b0)?,
        super::bound(b1)?,
    ))
}

/// Points and unit normals of `s` on a GRID x GRID sample of its parameter range
/// (points with a non-finite normal are skipped).
fn samples(s: &Surface) -> Res<Vec<(Point3, Vector3)>> {
    let (u0, u1, v0, v1) = bounded(s).ok_or("unbounded surface parameter range")?;
    let mut out = Vec::new();
    for i in 0..=GRID {
        for j in 0..=GRID {
            let (u, v) = (
                u0 + (u1 - u0) * i as f64 / GRID as f64,
                v0 + (v1 - v0) * j as f64 / GRID as f64,
            );
            let n = s.normal(u, v);
            if n.x.is_finite() && n.y.is_finite() && n.z.is_finite() {
                out.push((s.subs(u, v), n));
            }
        }
    }
    if out.len() < 4 {
        return Err("too few regular surface samples".into());
    }
    Ok(out)
}

/// The surface as an exact circular cylinder (any axis), if it is one.
fn as_cylinder(pts: &[(Point3, Vector3)], tol: f64) -> Option<Cylinder> {
    let (p0, n0) = pts[0];
    let (pb, nb) = *pts.iter().max_by(|x, y| {
        n0.cross(x.1)
            .magnitude()
            .total_cmp(&n0.cross(y.1).magnitude())
    })?;
    let cr = n0.cross(nb);
    if cr.magnitude() < 1e-3 {
        return None;
    }
    let d = cr.normalize();
    if pts.iter().any(|(_, n)| n.dot(d).abs() > 1e-9) {
        return None;
    }
    let perp = |v: Vector3| v - d * v.dot(d);
    let dn = nb - n0;
    let rs = perp(pb - p0).dot(dn) / dn.dot(dn);
    let c = p0 - n0 * rs;
    if rs.abs() <= tol
        || pts
            .iter()
            .any(|(p, n)| (perp(*p - c) - n * rs).magnitude() > tol)
    {
        return None;
    }
    let (e1, e2) = perp_frame(d);
    Some(Cylinder {
        c,
        d,
        e1,
        e2,
        r: rs.abs(),
    })
}

/// The surface as an exact plane, if it is one.
fn as_plane(pts: &[(Point3, Vector3)], tol: f64) -> Option<Plan> {
    let (p0, n0) = pts[0];
    let n0 = n0.normalize();
    if pts
        .iter()
        .any(|(p, n)| n0.cross(*n).magnitude() > 1e-9 || (*p - p0).dot(n0).abs() > tol)
    {
        return None;
    }
    let (e1, e2) = perp_frame(n0);
    Some(Plan::Plane {
        o: p0,
        e1,
        e2,
        n: n0,
    })
}

/// Axis of a revolved surface (after its processor transform), if rigid.
fn revolution_axis(s: &Surface) -> Option<(Point3, Vector3)> {
    let Surface::RevolutedCurve(p) = s else {
        return None;
    };
    let m = p.transform();
    let o = m.transform_point(p.entity().origin());
    let a = m.transform_vector(p.entity().axis());
    (a.magnitude() > 1e-12).then(|| (o, a.normalize()))
}

/// Meridian of a revolved surface about `ax`, if the surface is rotationally symmetric
/// about it and its meridian is a line or a circle.
fn meridian(s: &Surface, ax: &AxisFrame, tol: f64) -> Option<Meridian> {
    if !matches!(s, Surface::RevolutedCurve(_)) {
        return None;
    }
    let (u0, u1, v0, v1) = bounded(s)?;
    let mut mer = Vec::with_capacity(GRID + 1);
    for i in 0..=GRID {
        let u = u0 + (u1 - u0) * i as f64 / GRID as f64;
        let (rho, z, _) = ax.cyl(s.subs(u, v0));
        for j in 1..=4 {
            let (r2, z2, _) = ax.cyl(s.subs(u, v0 + (v1 - v0) * j as f64 / 5.0));
            if (r2 - rho).abs() > tol || (z2 - z).abs() > tol {
                return None; // not symmetric about this axis
            }
        }
        mer.push([rho, z]);
    }
    let (a, b) = (mer[0], mer[GRID]);
    let rho_min = mer.iter().map(|m| m[0]).fold(f64::INFINITY, f64::min);
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    let off_line =
        |m: &[f64; 2]| ((b[0] - a[0]) * (m[1] - a[1]) - (b[1] - a[1]) * (m[0] - a[0])) / len;
    if len > tol && mer.iter().all(|m| off_line(m).abs() <= tol) {
        let dz = b[1] - a[1];
        if dz.abs() <= tol {
            return Some(Meridian::Flat);
        }
        let k = (b[0] - a[0]) / dz;
        if rho_min <= tol {
            // Reaches the axis: anchor h at the apex (h = 0 there).
            if k.abs() < 1e-6 {
                return None;
            }
            return Some(Meridian::Line {
                rho0: 0.0,
                z0: a[1] - a[0] / k,
                k,
            });
        }
        return Some(Meridian::Line {
            rho0: a[0],
            z0: a[1],
            k,
        });
    }
    if rho_min <= tol {
        return None; // e.g. a sphere through its poles: not handled
    }
    // Circle through the first, middle and last meridian points.
    let c = mer[GRID / 2];
    let (bx, by, cx, cy) = (b[0] - a[0], b[1] - a[1], c[0] - a[0], c[1] - a[1]);
    let det = 2.0 * (bx * cy - by * cx);
    if det.abs() < 1e-12 * (bx * bx + by * by + cx * cx + cy * cy) {
        return None;
    }
    let (b2, c2) = (bx * bx + by * by, cx * cx + cy * cy);
    let (ux, uy) = ((cy * b2 - by * c2) / det, (bx * c2 - cx * b2) / det);
    let (rc, zc) = (a[0] + ux, a[1] + uy);
    let r = ux.hypot(uy);
    if mer
        .iter()
        .any(|m| ((m[0] - rc).hypot(m[1] - zc) - r).abs() > tol)
    {
        return None;
    }
    // Branch: centred on the face's meridian arc (it spans less than a full turn,
    // checked again on the boundary samples).
    let theta_ref = (c[1] - zc).atan2(c[0] - rc);
    Some(Meridian::Circle {
        rc,
        zc,
        r,
        theta_ref,
    })
}

/// Classify every face; Err names the first unsupported one.
fn plan(solid: &Solid) -> Res<(AxisFrame, Vec<Plan>, f64)> {
    let mut scale: f64 = 1.0;
    for v in solid.vertex_iter() {
        let p = v.point();
        scale = scale.max(p.x.abs()).max(p.y.abs()).max(p.z.abs());
    }
    let tol = SHAPE_REL * scale;
    // The integration axis: that of the first revolved face that is not a cylinder
    // about its own axis (a cone, torus or annulus); Z when there is none (all curved
    // faces are then cylinders, handled about their own axes).
    let mut ax = AxisFrame::new(Point3::origin(), Vector3::unit_z());
    for f in solid.face_iter() {
        let s = f.surface();
        if let Some((o, a)) = revolution_axis(&s) {
            let cand = AxisFrame::new(o, a);
            if let Some(m) = meridian(&s, &cand, tol)
                && !matches!(m, Meridian::Line { k, .. } if k.abs() < 1e-9)
            {
                ax = cand;
                break;
            }
        }
    }
    let mut plans = Vec::new();
    for f in solid.face_iter() {
        let s = f.surface();
        if let Surface::Plane(pl) = &s {
            let n = pl.normal();
            let e1 = pl.u_axis().normalize();
            plans.push(Plan::Plane {
                o: pl.origin(),
                e1,
                e2: n.cross(e1),
                n,
            });
            continue;
        }
        if let Some(m) = meridian(&s, &ax, tol) {
            plans.push(Plan::Revolution(m));
            continue;
        }
        let pts = samples(&s)?;
        if let Some(c) = as_cylinder(&pts, tol) {
            plans.push(Plan::Cylinder(c));
        } else if let Some(p) = as_plane(&pts, tol) {
            plans.push(p);
        } else {
            return Err(
                "curved face is not a plane, a cone/torus about the common axis or a circular cylinder"
                    .into(),
            );
        }
    }
    Ok((ax, plans, tol))
}

/// Calls `f(prev, p)` for every chord of every boundary edge of `face`, `n` per edge.
fn for_chords(face: &Face, n: usize, mut f: impl FnMut(Point3, Point3)) -> Res<()> {
    for w in face.boundaries() {
        for e in w.edge_iter() {
            let c = e.oriented_curve();
            let (t0, t1) = c.range_tuple();
            let mut prev = eval_curve(&c, t0).ok_or(EVAL_FAIL)?;
            for i in 1..=n {
                let p = eval_curve(&c, t0 + (t1 - t0) * i as f64 / n as f64).ok_or(EVAL_FAIL)?;
                f(prev, p);
                prev = p;
            }
        }
    }
    Ok(())
}

fn face_flux(face: &Face, plan: &Plan, ax: &AxisFrame, tol: f64, n: usize) -> Res<f64> {
    match plan {
        Plan::Plane { o, e1, e2, n: np } => {
            // F . n = (p - o_ax) . m / 2, m = n - (a . n) a: linear in p.
            let m = *np - ax.a * ax.a.dot(*np);
            if m.magnitude() < 1e-15 {
                return Ok(0.0);
            }
            let mut mom = [0.0; 3];
            for w in face.boundaries() {
                let wm = wire_moments_by(&w, n, |p| {
                    let d = p - *o;
                    [d.dot(*e1), d.dot(*e2)]
                })
                .ok_or(EVAL_FAIL)?;
                for i in 0..3 {
                    mom[i] += wm[i];
                }
            }
            Ok(0.5 * ((*o - ax.o).dot(m) * mom[0] + e1.dot(m) * mom[1] + e2.dot(m) * mom[2]))
        }
        Plan::Revolution(Meridian::Flat) => Ok(0.0),
        Plan::Revolution(mer) => {
            let mut sum = 0.0;
            let mut bad = false;
            let theta_ref = match mer {
                Meridian::Circle { theta_ref, .. } => Some(*theta_ref),
                _ => None,
            };
            for_chords(face, n, |p, q| {
                let (r0, z0, f0) = ax.cyl(p);
                let (r1, z1, f1) = ax.cyl(q);
                if let (Some(tr), Meridian::Circle { rc, zc, .. }) = (theta_ref, mer) {
                    // Single branch on the face: no sample may sit near the cut.
                    let t0 = wrap_near((z0 - zc).atan2(r0 - rc), tr);
                    let t1 = wrap_near((z1 - zc).atan2(r1 - rc), tr);
                    if (t0 - tr).abs() > 3.0 || (t1 - t0).abs() > 1.0 {
                        bad = true;
                    }
                }
                let dphi = if r0 <= tol || r1 <= tol {
                    0.0 // on the axis (cone apex): h = 0 there
                } else {
                    wrap_delta(f1 - f0)
                };
                sum += 0.5 * (mer.h(r0, z0) + mer.h(r1, z1)) * dphi;
            })?;
            if bad {
                return Err("torus face spans the meridian branch cut".into());
            }
            Ok(sum)
        }
        Plan::Cylinder(c) => {
            // G(s, psi) = -R/2 [ (q.u + R - (q_a + R u_a) u_a) s - d_a u_a s^2 / 2 ],
            // q = c - o, u = cos psi e1 + sin psi e2, x_a = x . a.
            let q = c.c - ax.o;
            let (qa, da) = (q.dot(ax.a), c.d.dot(ax.a));
            let g = |p: Point3| -> (f64, f64) {
                let w = p - c.c;
                let s = w.dot(c.d);
                let psi = w.dot(c.e2).atan2(w.dot(c.e1));
                let u = c.e1 * psi.cos() + c.e2 * psi.sin();
                let ua = u.dot(ax.a);
                let lin = q.dot(u) + c.r - (qa + c.r * ua) * ua;
                (-0.5 * c.r * (lin * s - 0.5 * da * ua * s * s), psi)
            };
            let mut sum = 0.0;
            for_chords(face, n, |p, q| {
                let ((g0, s0), (g1, s1)) = (g(p), g(q));
                sum += 0.5 * (g0 + g1) * wrap_delta(s1 - s0);
            })?;
            Ok(sum)
        }
    }
}

fn volume_at(solid: &Solid, ax: &AxisFrame, plans: &[Plan], tol: f64, n: usize) -> Res<f64> {
    let mut v = 0.0;
    for (f, p) in solid.face_iter().zip(plans) {
        v += face_flux(f, p, ax, tol, n)?;
    }
    Ok(v)
}

/// Volume and error estimate (three chord levels, Richardson), or why the method does
/// not apply.
pub(super) fn revolution_volume(solid: &Solid, n: usize) -> Res<(f64, f64)> {
    let (ax, plans, tol) = plan(solid)?;
    let (ax, plans) = (&ax, &plans[..]);
    let [a, b, c] = std::thread::scope(|sc| {
        let hs = [n / 2, n, 2 * n].map(|k| sc.spawn(move || volume_at(solid, ax, plans, tol, k)));
        hs.map(|h| {
            h.join()
                .unwrap_or_else(|_| Err("volume worker panicked".into()))
        })
    });
    let (a, b, c) = (a?, b?, c?);
    let r1 = (4.0 * b - a) / 3.0;
    let r2 = (4.0 * c - b) / 3.0;
    Ok((r2, (r2 - r1).abs()))
}

// ---------------------------------------------------------------------------
// Moments (mass properties): the same faces with the axial field F = G a of
// `mass.rs` (dG/dz = monomial). The flux 2-form of G a is G dx ^ dy in the axis
// frame: on a surface of revolution G rho drho ^ dphi = d(H dphi) with
// H = int G rho drho along the meridian at fixed phi; on a circular cylinder about
// (c, d) it is -R G (u . a) ds ^ dpsi = d(K dpsi) with K = int_0^s -R G (u . a) ds'.
// Planar faces are fanned into triangles (exact cubature for cubic G).

use super::mass::{Mom, MomFrame, add_scaled, fan_flux, richardson};
use std::sync::OnceLock;

/// Gauss-Legendre nodes and weights on [-1, 1] (Newton on the Legendre recurrence).
fn gauss_legendre(n: usize) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let mut x = (std::f64::consts::PI * (i as f64 + 0.75) / (n as f64 + 0.5)).cos();
        let mut dp = 1.0;
        for _ in 0..100 {
            let (mut p0, mut p1) = (1.0, x);
            for j in 2..=n {
                let p2 = ((2 * j - 1) as f64 * x * p1 - (j - 1) as f64 * p0) / j as f64;
                p0 = p1;
                p1 = p2;
            }
            dp = n as f64 * (x * p1 - p0) / (x * x - 1.0);
            let dx = p1 / dp;
            x -= dx;
            if dx.abs() < 1e-16 {
                break;
            }
        }
        out.push((x, 2.0 / ((1.0 - x * x) * dp * dp)));
    }
    out
}

/// 3 points: exact for polynomials of degree 5 (the line and annulus integrands are
/// at most quartic in their parameter).
fn gl3() -> &'static [(f64, f64)] {
    static R: OnceLock<Vec<(f64, f64)>> = OnceLock::new();
    R.get_or_init(|| gauss_legendre(3))
}

/// 8 points per segment of at most 0.5 rad for the (trigonometric) torus integrand.
fn gl8() -> &'static [(f64, f64)] {
    static R: OnceLock<Vec<(f64, f64)>> = OnceLock::new();
    R.get_or_init(|| gauss_legendre(8))
}

/// int_a^b f(t) dt with `segs` equal segments of the rule.
fn quad(a: f64, b: f64, segs: usize, rule: &[(f64, f64)], f: impl Fn(f64) -> Mom) -> Mom {
    let mut acc = [0.0; 10];
    let h = (b - a) / segs as f64;
    for s in 0..segs {
        let (lo, half) = (a + h * s as f64, 0.5 * h);
        for &(x, w) in rule {
            add_scaled(&mut acc, &f(lo + half * (x + 1.0)), w * half);
        }
    }
    acc
}

impl Meridian {
    /// H at the point (rho, z, phi) of the face: int G rho drho along the meridian at
    /// azimuth phi from its reference point (apex / axis / `theta_ref`).
    fn h_moments(&self, ax: &AxisFrame, fr: &MomFrame, rho: f64, z: f64, phi: f64) -> Mom {
        let u = ax.e1 * phi.cos() + ax.e2 * phi.sin();
        let at = |r: f64, zz: f64| ax.o + u * r + ax.a * zz;
        let scale = |mut m: Mom, s: f64| {
            m.iter_mut().for_each(|x| *x *= s);
            m
        };
        match *self {
            Meridian::Flat => quad(0.0, rho, 1, gl3(), |r| scale(fr.g(at(r, z)), r)),
            Meridian::Line { rho0, z0, k } => quad(z0, z, 1, gl3(), |t| {
                let r = rho0 + k * (t - z0);
                scale(fr.g(at(r, t)), r * k)
            }),
            Meridian::Circle {
                rc,
                zc,
                r,
                theta_ref,
            } => {
                let th = wrap_near((z - zc).atan2(rho - rc), theta_ref);
                let segs = (((th - theta_ref).abs() / 0.5).ceil() as usize).max(1);
                quad(theta_ref, th, segs, gl8(), |t| {
                    let (s, c) = t.sin_cos();
                    let rr = rc + r * c;
                    scale(fr.g(at(rr, zc + r * s)), rr * (-r * s))
                })
            }
        }
    }
}

fn face_moments(
    face: &Face,
    plan: &Plan,
    ax: &AxisFrame,
    fr: &MomFrame,
    tol: f64,
    n: usize,
) -> Res<Mom> {
    match plan {
        Plan::Plane { .. } => fan_flux(face, fr, n),
        Plan::Revolution(mer) => {
            let mut acc = [0.0; 10];
            let mut bad = false;
            for_chords(face, n, |p, q| {
                let (r0, z0, f0) = ax.cyl(p);
                let (r1, z1, f1) = ax.cyl(q);
                if let Meridian::Circle {
                    rc, zc, theta_ref, ..
                } = mer
                {
                    let t0 = wrap_near((z0 - zc).atan2(r0 - rc), *theta_ref);
                    let t1 = wrap_near((z1 - zc).atan2(r1 - rc), *theta_ref);
                    if (t0 - theta_ref).abs() > 3.0 || (t1 - t0).abs() > 1.0 {
                        bad = true;
                    }
                }
                let dphi = if r0 <= tol || r1 <= tol {
                    0.0
                } else {
                    wrap_delta(f1 - f0)
                };
                if dphi != 0.0 {
                    add_scaled(&mut acc, &mer.h_moments(ax, fr, r0, z0, f0), 0.5 * dphi);
                    add_scaled(&mut acc, &mer.h_moments(ax, fr, r1, z1, f1), 0.5 * dphi);
                }
            })?;
            if bad {
                return Err("torus face spans the meridian branch cut".into());
            }
            Ok(acc)
        }
        Plan::Cylinder(c) => {
            let k = |p: Point3| -> (Mom, f64) {
                let w = p - c.c;
                let s = w.dot(c.d);
                let psi = w.dot(c.e2).atan2(w.dot(c.e1));
                let u = c.e1 * psi.cos() + c.e2 * psi.sin();
                let ua = u.dot(fr.a);
                let base = c.c + u * c.r;
                let m = quad(0.0, s, 1, gl3(), |t| {
                    let mut g = fr.g(base + c.d * t);
                    g.iter_mut().for_each(|x| *x *= -c.r * ua);
                    g
                });
                (m, psi)
            };
            let mut acc = [0.0; 10];
            for_chords(face, n, |p, q| {
                let ((k0, s0), (k1, s1)) = (k(p), k(q));
                let d = wrap_delta(s1 - s0);
                add_scaled(&mut acc, &k0, 0.5 * d);
                add_scaled(&mut acc, &k1, 0.5 * d);
            })?;
            Ok(acc)
        }
    }
}

/// Moments of a body the revolution volume method accepts, in a frame along the
/// common axis with its origin at the projection of `center` on it: (frame, best,
/// second-best Richardson extrapolants).
pub(super) fn revolution_moments(
    solid: &Solid,
    n: usize,
    center: Point3,
) -> Res<(MomFrame, Mom, Mom)> {
    let (ax, plans, tol) = plan(solid)?;
    let fr = MomFrame {
        o: ax.o + ax.a * ax.a.dot(center - ax.o),
        e1: ax.e1,
        e2: ax.e2,
        a: ax.a,
    };
    let (ax, plans, fr2) = (&ax, &plans[..], &fr);
    let (best, other) = richardson(n, |k| {
        let mut acc = [0.0; 10];
        for (f, p) in solid.face_iter().zip(plans) {
            add_scaled(&mut acc, &face_moments(f, p, ax, fr2, tol, k)?, 1.0);
        }
        Ok(acc)
    })?;
    Ok((fr, best, other))
}

#[cfg(test)]
mod moment_tests {
    use super::*;

    #[test]
    fn gauss_legendre_is_exact_to_degree_2n_minus_1() {
        for n in [3usize, 8] {
            let r = gauss_legendre(n);
            for deg in 0..(2 * n) {
                let got: f64 = r.iter().map(|&(x, w)| w * x.powi(deg as i32)).sum();
                let want = if deg % 2 == 1 {
                    0.0
                } else {
                    2.0 / (deg as f64 + 1.0)
                };
                assert!(
                    (got - want).abs() < 1e-14,
                    "n {n} deg {deg}: {got} vs {want}"
                );
            }
        }
    }
}
