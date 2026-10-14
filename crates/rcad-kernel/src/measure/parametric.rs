//! Volume of a closed B-rep from the divergence theorem over each face's own
//! parametrisation, for any smooth surfaces (not only the ruled, planar or
//! revolution cases of the other boundary-integral methods).
//!
//! With F = (0, 0, z − z0) (div F = 1), a face on S(u, v) trimmed to the
//! parameter region D contributes ∫∫_D g du dv, g = (z − z0)·(S_u × S_v)_z.
//! The mass moments use the same integral with the fields F = G a of
//! `mass.rs` (dG/dz = monomial) in place of (z − z0) e_z.
//! By Green's theorem that is the boundary integral ∮_∂D G dv with
//! G(u, v) = ∫_{u0}^{u} g(s, v) ds. The face's oriented boundary wires
//! (`Face::boundaries`) already carry the face orientation: when the outward
//! normal is −S_u × S_v the wires run clockwise in (u, v), and both signs flip.
//!
//! Evaluation: each edge is split into panels with Gauss–Legendre nodes in
//! the curve parameter t; at each node the exact edge point is located on the
//! surface (`search_parameter`, hinted by the previous node, verified to lie
//! on it), v'(t) comes from the surface Jacobian and the curve derivative, and
//! G by Gauss–Legendre in s. For smooth integrands the result converges
//! spectrally; the error estimate is the difference between two panel counts.
//!
//! Refused (Err, callers fall back) when an edge is an implicit intersection
//! curve, a boundary point does not lie on its surface, the parametrisation is
//! singular on the boundary (a pole), or a boundary loop does not close in
//! (u, v) (a face crossing a periodic seam).
use super::mass::{Mom, MomFrame, add_scaled};
use truck_modeling::*;

type Res<T> = std::result::Result<T, String>;

/// 8-point Gauss–Legendre nodes and weights on [-1, 1].
#[allow(clippy::excessive_precision)]
const GL8: [(f64, f64); 8] = [
    (-0.960_289_856_497_536_2, 0.101_228_536_290_376_26),
    (-0.796_666_477_413_626_7, 0.222_381_034_453_374_47),
    (-0.525_532_409_916_329_0, 0.313_706_645_877_887_3),
    (-0.183_434_642_495_649_8, 0.362_683_783_378_362_0),
    (0.183_434_642_495_649_8, 0.362_683_783_378_362_0),
    (0.525_532_409_916_329_0, 0.313_706_645_877_887_3),
    (0.796_666_477_413_626_7, 0.222_381_034_453_374_47),
    (0.960_289_856_497_536_2, 0.101_228_536_290_376_26),
];

/// Panels of the inner integral G (in s) per knot span, at the coarse
/// level (doubled at the fine level together with the edge panels, so the
/// error estimate covers both quadratures).
const S_PANELS: usize = 4;

/// Distinct interior knots of a knot vector strictly inside (lo, hi).
fn interior(knots: &[f64], lo: f64, hi: f64) -> Vec<f64> {
    let mut out: Vec<f64> = knots
        .iter()
        .copied()
        .filter(|k| *k > lo + 1e-12 * (1.0 + lo.abs()) && *k < hi - 1e-12 * (1.0 + hi.abs()))
        .collect();
    out.dedup_by(|a, b| (*a - *b).abs() <= 1e-12 * (1.0 + a.abs()));
    out
}

/// The u-direction knots of a surface (where its integrand may kink).
fn u_knots(s: &Surface) -> Vec<f64> {
    match s {
        Surface::BSplineSurface(b) => b.knot_vecs().0.to_vec(),
        Surface::NurbsSurface(n) => n.knot_vecs().0.to_vec(),
        Surface::RevolutedCurve(r) => curve_knots(r.entity().entity_curve()),
        Surface::Plane(_) => Vec::new(),
    }
}

/// Knots of an edge curve (where its derivative may jump).
fn curve_knots(c: &Curve) -> Vec<f64> {
    match c {
        Curve::BSplineCurve(b) => b.knot_vec().to_vec(),
        Curve::NurbsCurve(n) => n.knot_vec().to_vec(),
        _ => Vec::new(),
    }
}

struct FaceCtx<'a> {
    s: &'a Surface,
    fr: &'a MomFrame,
    u0: f64,
    /// Interior u-knots of the surface (sorted).
    uk: Vec<f64>,
    /// Panels per knot span of the inner integral.
    s_panels: usize,
    v_period: Option<f64>,
    u_period: Option<f64>,
    /// Distance tolerance for "on the surface" (mm).
    on_tol: f64,
}

impl FaceCtx<'_> {
    /// ∫_{u0}^{u} G(S(s, v)) (S_u × S_v · a) ds for every moment.
    /// Split at the surface's u-knots, so each Gauss–Legendre piece
    /// integrates a smooth (single-span) integrand.
    fn big_g(&self, u: f64, v: f64) -> Mom {
        let mut acc = [0.0; 10];
        let (lo, hi, sign) = if u >= self.u0 {
            (self.u0, u, 1.0)
        } else {
            (u, self.u0, -1.0)
        };
        let mut cuts = vec![lo];
        cuts.extend(interior(&self.uk, lo, hi));
        cuts.push(hi);
        for span in cuts.windows(2) {
            let h = (span[1] - span[0]) / self.s_panels as f64;
            for k in 0..self.s_panels {
                let m = span[0] + h * (k as f64 + 0.5);
                for (x, w) in GL8 {
                    let s = m + 0.5 * h * x;
                    let n = self.s.uder(s, v).cross(self.s.vder(s, v)).dot(self.fr.a);
                    add_scaled(
                        &mut acc,
                        &self.fr.g(self.s.subs(s, v)),
                        sign * 0.5 * h * w * n,
                    );
                }
            }
        }
        acc
    }
    /// Parameters of `p` near `hint`, unwrapped to the hint's period copy.
    fn locate(&self, p: Point3, hint: Option<(f64, f64)>) -> Res<(f64, f64)> {
        let found = match hint {
            Some(h) => self
                .s
                .search_parameter(p, Some(h), 100)
                .or_else(|| self.s.search_parameter(p, None, 100)),
            None => self.s.search_parameter(p, None, 100),
        };
        let (mut u, mut v) = found.ok_or("boundary point not found on its surface")?;
        if (self.s.subs(u, v) - p).magnitude() > self.on_tol {
            return Err("boundary point is off its surface".into());
        }
        if let Some(h) = hint {
            if let Some(per) = self.u_period {
                u += per * ((h.0 - u) / per).round();
            }
            if let Some(per) = self.v_period {
                v += per * ((h.1 - v) / per).round();
            }
        }
        Ok((u, v))
    }
}

/// Boundary integral of one face at `panels` panels per edge.
fn face_flux(f: &Face, fr: &MomFrame, on_tol: f64, panels: usize) -> Res<Mom> {
    let s = f.surface();
    let mut total = [0.0; 10];
    let mut uk = u_knots(&s);
    uk.sort_by(f64::total_cmp);
    let mut ctx = FaceCtx {
        s: &s,
        fr,
        u0: 0.0,
        uk,
        s_panels: S_PANELS * panels.div_ceil(PANELS).max(1),
        v_period: s.v_period(),
        u_period: s.u_period(),
        on_tol,
    };
    let mut have_ref = false;
    let same = |a: (f64, f64), b: (f64, f64)| {
        (a.0 - b.0).abs() <= 1e-7 * (1.0 + a.0.abs())
            && (a.1 - b.1).abs() <= 1e-7 * (1.0 + a.1.abs())
    };
    for w in f.boundaries() {
        // A seam edge (used twice by one wire) makes the parameter copy of
        // the boundary ambiguous.
        let mut ids = std::collections::HashSet::new();
        if !w.edge_iter().all(|e| ids.insert(e.id())) {
            return Err("a face boundary runs along a seam edge".into());
        }
        let mut prev: Option<(f64, f64)> = None;
        let mut start: Option<(f64, f64)> = None;
        for e in w.edge_iter() {
            let c = e.oriented_curve();
            if matches!(c, Curve::IntersectionCurve(_)) {
                return Err("implicit intersection edge".into());
            }
            let (t0, t1) = c.range_tuple();
            let first = ctx.locate(c.subs(t0), prev)?;
            // Consecutive edges must meet in parameter space too (no pole or
            // seam jump, where the domain boundary has no edge).
            if let Some(p) = prev
                && !same(p, first)
            {
                return Err("the face boundary jumps in parameter space (pole or seam)".into());
            }
            start.get_or_insert(first);
            if !have_ref {
                ctx.u0 = first.0;
                have_ref = true;
            }
            let mut last = (t0, first);
            // Edge pieces between the curve's knots, `panels` panels each.
            let mut cuts = vec![t0];
            cuts.extend(interior(&curve_knots(&c), t0.min(t1), t0.max(t1)));
            if t1 < t0 {
                cuts[1..].reverse();
            }
            cuts.push(t1);
            // Next evaluation point after `last`: located, and checked against
            // the curve at the parameter midpoint (a jump through a pole or
            // across a seam between samples fails this).
            let step = |t: f64, last: &mut (f64, (f64, f64))| -> Res<(f64, f64)> {
                let uv = ctx.locate(c.subs(t), Some(last.1))?;
                let mid = (0.5 * (uv.0 + last.1.0), 0.5 * (uv.1 + last.1.1));
                let chord = (c.subs(t) - c.subs(last.0)).magnitude();
                let off = (s.subs(mid.0, mid.1) - c.subs(0.5 * (t + last.0))).magnitude();
                if off > 0.05 * chord + ctx.on_tol {
                    return Err("the face boundary jumps in parameter space".into());
                }
                *last = (t, uv);
                Ok(uv)
            };
            for piece in cuts.windows(2) {
                let (p0, h) = (piece[0], (piece[1] - piece[0]) / panels as f64);
                for k in 0..panels {
                    let m = p0 + h * (k as f64 + 0.5);
                    for (x, wgt) in GL8 {
                        let t = m + 0.5 * h * x;
                        let (u, v) = step(t, &mut last)?;
                        let (su, sv, dc) = (s.uder(u, v), s.vder(u, v), c.der(t));
                        let (a, b, d) = (su.dot(su), su.dot(sv), sv.dot(sv));
                        let det = a * d - b * b;
                        if det <= 1e-10 * a * d || a * d == 0.0 {
                            return Err("singular surface parametrisation on a boundary".into());
                        }
                        let vdot = (a * sv.dot(dc) - b * su.dot(dc)) / det;
                        add_scaled(&mut total, &ctx.big_g(u, v), 0.5 * h * wgt * vdot);
                    }
                    step(p0 + h * (k + 1) as f64, &mut last)?;
                }
            }
            prev = Some(last.1);
        }
        if let (Some(a), Some(b)) = (start, prev)
            && !same(a, b)
        {
            return Err("a face boundary does not close in parameter space (seam)".into());
        }
    }
    Ok(total)
}

/// Largest face count tried.
pub(crate) const MAX_FACES: usize = 2000;
/// Gauss–Legendre panels per edge (and twice that for the error estimate).
pub(crate) const PANELS: usize = 4;
/// Largest relative difference between the two panel counts accepted, for
/// every moment against its scale (V, V·L, V·L² with L the bounding diagonal).
pub(crate) const MAX_REL_ERR: f64 = 1e-9;

/// The ten moments of `solid` in `fr` at `panels` and `2 * panels` panels
/// per edge (finer first).
pub(crate) fn parametric_moments_at(
    solid: &Solid,
    fr: &MomFrame,
    panels: usize,
) -> Res<(Mom, Mom)> {
    let b = super::raw_bounds(solid).map_err(|e| e.to_string())?;
    let diag = (0..3)
        .map(|i| (b.max[i] - b.min[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    let on_tol = 1e-7 * diag.max(1.0);
    let faces: Vec<&Face> = solid.face_iter().collect();
    let level = |n: usize| -> Res<Mom> {
        let mut acc = [0.0; 10];
        for f in &faces {
            add_scaled(&mut acc, &face_flux(f, fr, on_tol, n)?, 1.0);
        }
        Ok(acc)
    };
    let (lo, hi) = std::thread::scope(|sc| {
        let a = sc.spawn(|| level(panels));
        let b = level(2 * panels);
        (
            a.join()
                .unwrap_or_else(|_| Err("parametric worker panicked".into())),
            b,
        )
    });
    Ok((hi?, lo?))
}

/// The moments in `fr` when the method applies and has converged (the one
/// acceptance rule shared by the volume and the mass properties).
pub(crate) fn parametric_moments(solid: &Solid, fr: &MomFrame) -> Res<(Mom, Mom)> {
    if solid.face_iter().count() > MAX_FACES {
        return Err(format!("more than {MAX_FACES} faces"));
    }
    let (hi, lo) = parametric_moments_at(solid, fr, PANELS)?;
    let v = hi[0];
    let b = super::raw_bounds(solid).map_err(|e| e.to_string())?;
    let l = (0..3)
        .map(|i| (b.max[i] - b.min[i]).powi(2))
        .sum::<f64>()
        .sqrt()
        .max(1.0);
    // Every moment must converge, each against its own scale (V, V·L, V·L²).
    for k in 0..10 {
        let scale = v.abs()
            * l.powi(match k {
                0 => 0,
                1..=3 => 1,
                _ => 2,
            });
        if !(hi[k].is_finite() && (hi[k] - lo[k]).abs() <= MAX_REL_ERR * scale) {
            return Err(format!(
                "not converged (moment {k} difference {:.3e})",
                (hi[k] - lo[k]).abs()
            ));
        }
    }
    Ok((hi, lo))
}

/// World-axis moment frame at the bounding-box centre.
pub(crate) fn center_frame(solid: &Solid) -> Res<MomFrame> {
    let b = super::raw_bounds(solid).map_err(|e| e.to_string())?;
    Ok(MomFrame {
        o: Point3::new(
            0.5 * (b.min[0] + b.max[0]),
            0.5 * (b.min[1] + b.max[1]),
            0.5 * (b.min[2] + b.max[2]),
        ),
        e1: Vector3::unit_x(),
        e2: Vector3::unit_y(),
        a: Vector3::unit_z(),
    })
}

/// Volume of `solid` and an error estimate (difference between `panels` and
/// `2 * panels` per edge), without the acceptance rule.
pub(crate) fn parametric_volume(solid: &Solid, panels: usize) -> Res<(f64, f64)> {
    let fr = center_frame(solid)?;
    let (hi, lo) = parametric_moments_at(solid, &fr, panels)?;
    Ok((hi[0], (hi[0] - lo[0]).abs()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Body;

    /// Tent prism: the triangle (0,0)-(5,3)-(10,0) in XZ swept 4 along Y; its
    /// roof is ONE degree-1 B-spline face with an interior knot (two planar
    /// facets meeting at the ridge), so its integrand kinks inside the face.
    /// Volume 15 × 4 = 60, centroid (5, 2, 1).
    fn tent() -> Body {
        let v0 = builder::vertex(Point3::new(0.0, 0.0, 0.0));
        let v1 = builder::vertex(Point3::new(10.0, 0.0, 0.0));
        let roof = BSplineCurve::new(
            KnotVec::from(vec![0.0, 0.0, 0.37, 1.0, 1.0]),
            vec![
                Point3::new(10.0, 0.0, 0.0),
                Point3::new(5.0, 0.0, 3.0),
                Point3::new(0.0, 0.0, 0.0),
            ],
        );
        let wire: Wire = vec![
            builder::line(&v0, &v1),
            Edge::new(&v1, &v0, Curve::BSplineCurve(roof)),
        ]
        .into();
        let mut face = builder::try_attach_plane(&[wire]).unwrap();
        if let Surface::Plane(pl) = face.surface() {
            // Sweep along +Y with the face's normal pointing away from it.
            if pl.normal().y * if face.orientation() { 1.0 } else { -1.0 } > 0.0 {
                face = face.inverse();
            }
        }
        let solid = builder::tsweep(&face, Vector3::new(0.0, 4.0, 0.0));
        Body { solid }
    }

    #[test]
    fn multi_span_bspline_face_is_integrated_per_knot_span() {
        let b = tent();
        let fr = center_frame(&b.solid).unwrap();
        let (hi, lo) = parametric_moments(&b.solid, &fr).unwrap();
        let v = hi[0].abs();
        assert!(
            (v - 60.0).abs() < 1e-9 * 60.0,
            "volume {} (lo {})",
            hi[0],
            lo[0]
        );
        // Centroid (5, 2, 1) relative to the frame origin.
        let c = [hi[1] / hi[0], hi[2] / hi[0], hi[3] / hi[0]];
        let want = [5.0 - fr.o.x, 2.0 - fr.o.y, 1.0 - fr.o.z];
        for k in 0..3 {
            assert!((c[k] - want[k]).abs() < 1e-9, "centroid {c:?} vs {want:?}");
        }
    }
}
