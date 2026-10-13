//! Exact conversion of STEP geometry entities to truck-modeling curves and surfaces.
//!
//! Lengths are scaled to millimetres when points and radii are read (`Geo::k`), which
//! is exact for every representation used here: control points of (rational)
//! B-splines are scaled in their Euclidean part only (weights unchanged), and the
//! conics are built directly in millimetres. Circles and ellipses become rational
//! quadratic B-splines (exact), cylinders / cones / spheres / tori become surfaces of
//! revolution of an exact line or rational arc. Anything else is an error naming the
//! entity, never an approximation.
use super::ast::{Index, P};
use super::{StepError, fail};
use std::f64::consts::{FRAC_PI_2, PI};
use std::result::Result;
use truck_modeling::*;

/// Largest accepted B-spline degree.
const MAX_DEGREE: i64 = 25;
/// Largest accepted reference nesting for curve wrappers (SURFACE_CURVE -> curve).
const MAX_CURVE_DEPTH: usize = 4;

pub(super) struct Geo<'a> {
    pub idx: &'a Index,
    /// File length unit -> mm.
    pub k: f64,
    /// File plane-angle unit -> rad.
    pub angle: f64,
}

/// Analytic description of a face surface, used for exact distance checks and for
/// splitting faces on periodic surfaces along a seam.
#[derive(Clone, Copy, Debug)]
pub(super) enum Analytic {
    Plane,
    Cylinder {
        o: Point3,
        z: Vector3,
        r: f64,
    },
    Cone {
        o: Point3,
        z: Vector3,
        r: f64,
        semi: f64,
    },
    Sphere {
        o: Point3,
        z: Vector3,
        r: f64,
    },
    Torus {
        o: Point3,
        z: Vector3,
        big: f64,
        small: f64,
    },
    Freeform,
}

impl Analytic {
    /// Axis of revolution (origin, unit direction) for surfaces of revolution.
    pub fn axis(&self) -> Option<(Point3, Vector3)> {
        match *self {
            Analytic::Cylinder { o, z, .. }
            | Analytic::Cone { o, z, .. }
            | Analytic::Sphere { o, z, .. }
            | Analytic::Torus { o, z, .. } => Some((o, z)),
            Analytic::Plane | Analytic::Freeform => None,
        }
    }
}

pub(super) struct Placement {
    pub o: Point3,
    pub x: Vector3,
    pub y: Vector3,
    pub z: Vector3,
}

impl Geo<'_> {
    pub fn point(&self, id: u64) -> Result<Point3, StepError> {
        let (_, p) = self.idx.simple(id, &["CARTESIAN_POINT"])?;
        let c = p.reals(1)?;
        if c.len() != 3 {
            return Err(fail(format!(
                "CARTESIAN_POINT #{id} has {} coordinates, expected 3",
                c.len()
            )));
        }
        Ok(Point3::new(c[0] * self.k, c[1] * self.k, c[2] * self.k))
    }

    pub fn vertex(&self, id: u64) -> Result<Point3, StepError> {
        let (_, p) = self.idx.simple(id, &["VERTEX_POINT"])?;
        self.point(p.r(1)?)
    }

    pub fn direction(&self, id: u64) -> Result<Vector3, StepError> {
        let (_, p) = self.idx.simple(id, &["DIRECTION"])?;
        let c = p.reals(1)?;
        if c.len() != 3 {
            return Err(fail(format!("DIRECTION #{id} is not 3D")));
        }
        let v = Vector3::new(c[0], c[1], c[2]);
        let m = v.magnitude();
        if !(m > 1e-12) {
            return Err(fail(format!("DIRECTION #{id} has zero length")));
        }
        Ok(v / m)
    }

    /// AXIS2_PLACEMENT_3D: right-handed orthonormal frame (ISO 10303-42 rules: the
    /// ref_direction is projected into the plane normal to the axis; defaults to the
    /// world axes when omitted).
    pub fn placement(&self, id: u64) -> Result<Placement, StepError> {
        let (_, p) = self.idx.simple(id, &["AXIS2_PLACEMENT_3D"])?;
        let o = self.point(p.r(1)?)?;
        let z = if p.is_unset(2) {
            Vector3::unit_z()
        } else {
            self.direction(p.r(2)?)?
        };
        let xr = if p.is_unset(3) {
            // ISO 10303-42 default ref direction: (1,0,0) unless parallel to z.
            if z.x.abs() < 1.0 - 1e-9 {
                Vector3::unit_x()
            } else {
                Vector3::unit_z()
            }
        } else {
            self.direction(p.r(3)?)?
        };
        let xp = xr - z * xr.dot(z);
        let m = xp.magnitude();
        if !(m > 1e-9) {
            return Err(fail(format!(
                "AXIS2_PLACEMENT_3D #{id}: ref_direction parallel to axis"
            )));
        }
        let x = xp / m;
        Ok(Placement {
            o,
            x,
            y: z.cross(x),
            z,
        })
    }

    fn length(&self, p: &P, i: usize) -> Result<f64, StepError> {
        let v = p.f(i)? * self.k;
        if !(v > 0.0) {
            return Err(fail(format!(
                "{} #{}: length parameter {} must be positive",
                p.ty,
                p.id,
                i + 1
            )));
        }
        Ok(v)
    }

    // ---------------------------------------------------------------- curves

    /// Curve of EDGE_CURVE geometry `gid` from `start` to `end` (in the direction of
    /// the curve's own parametrisation, i.e. already swapped for `same_sense = .F.`).
    /// `closed` = the edge starts and ends at the same vertex (a full period).
    pub fn edge_geometry(
        &self,
        gid: u64,
        start: Point3,
        end: Point3,
        closed: bool,
        depth: usize,
    ) -> Result<Curve, StepError> {
        if depth > MAX_CURVE_DEPTH {
            return Err(fail(format!("curve #{gid}: nesting too deep")));
        }
        let e = self.idx.get(gid)?;
        let ty = e.type_name();
        if let [(name, params)] = e.parts.as_slice() {
            let p = P::new(gid, name, params);
            match name.as_str() {
                "LINE" => {
                    if closed {
                        return Err(fail(format!("LINE #{gid} used by a closed edge")));
                    }
                    return Ok(Curve::Line(Line(start, end)));
                }
                "CIRCLE" => {
                    let pl = self.placement(p.r(1)?)?;
                    let r = self.length(&p, 2)?;
                    return conic_edge(&pl, r, r, start, end, closed);
                }
                "ELLIPSE" => {
                    let pl = self.placement(p.r(1)?)?;
                    let a = self.length(&p, 2)?;
                    let b = self.length(&p, 3)?;
                    return conic_edge(&pl, a, b, start, end, closed);
                }
                "SURFACE_CURVE" | "SEAM_CURVE" => {
                    // The 3D curve is authoritative (pcurves are redundant data).
                    return self.edge_geometry(p.r(1)?, start, end, closed, depth + 1);
                }
                "B_SPLINE_CURVE_WITH_KNOTS" => {
                    let full = self.bspline_curve_simple(&p)?;
                    return trim_curve(Curve::BSplineCurve(full), start, end, closed, gid, &ty);
                }
                _ => {}
            }
        } else if e.has("B_SPLINE_CURVE") && e.has("RATIONAL_B_SPLINE_CURVE") {
            let full = self.nurbs_curve_complex(gid)?;
            return trim_curve(Curve::NurbsCurve(full), start, end, closed, gid, &ty);
        } else if e.has("B_SPLINE_CURVE") && e.has("B_SPLINE_CURVE_WITH_KNOTS") {
            let full = self.bspline_curve_complex(gid)?;
            return trim_curve(Curve::BSplineCurve(full), start, end, closed, gid, &ty);
        }
        Err(fail(format!("unsupported curve entity {ty} #{gid}")))
    }

    fn knots(
        &self,
        id: u64,
        degree: i64,
        n_ctrl: usize,
        mults: &[i64],
        knots: &[f64],
    ) -> Result<KnotVec, StepError> {
        if !(1..=MAX_DEGREE).contains(&degree) {
            return Err(fail(format!("B-spline #{id}: unsupported degree {degree}")));
        }
        if mults.len() != knots.len() || mults.iter().any(|&m| m < 1 || m > degree + 1) {
            return Err(fail(format!("B-spline #{id}: invalid knot multiplicities")));
        }
        let total: i64 = mults.iter().sum();
        if total != n_ctrl as i64 + degree + 1 {
            return Err(fail(format!(
                "B-spline #{id}: {total} knots for {n_ctrl} control points of degree {degree}"
            )));
        }
        KnotVec::from_single_multi(knots.to_vec(), mults.iter().map(|&m| m as usize).collect())
            .map_err(|e| fail(format!("B-spline #{id}: {e}")))
    }

    fn points(&self, ids: &[u64]) -> Result<Vec<Point3>, StepError> {
        ids.iter().map(|&i| self.point(i)).collect()
    }

    /// `B_SPLINE_CURVE_WITH_KNOTS('', deg, (pts), form, closed, self_int, (mults), (knots), spec)`
    fn bspline_curve_simple(&self, p: &P) -> Result<BSplineCurve<Point3>, StepError> {
        let deg = p.int(1)?;
        let pts = self.points(&p.refs(2)?)?;
        let kv = self.knots(p.id, deg, pts.len(), &p.ints(6)?, &p.reals(7)?)?;
        BSplineCurve::try_new(kv, pts).map_err(|e| fail(format!("{} #{}: {e}", p.ty, p.id)))
    }

    fn bspline_parts(&self, id: u64) -> Result<(i64, Vec<u64>, KnotVecParts), StepError> {
        let e = self.idx.get(id)?;
        let b = e
            .part("B_SPLINE_CURVE")
            .ok_or_else(|| fail(format!("#{id}: missing B_SPLINE_CURVE")))?;
        let b = P::new(id, "B_SPLINE_CURVE", b);
        let k = e.part("B_SPLINE_CURVE_WITH_KNOTS").ok_or_else(|| {
            fail(format!(
                "{} #{id}: only B_SPLINE_CURVE_WITH_KNOTS is supported",
                e.type_name()
            ))
        })?;
        let k = P::new(id, "B_SPLINE_CURVE_WITH_KNOTS", k);
        Ok((b.int(0)?, b.refs(1)?, (k.ints(0)?, k.reals(1)?)))
    }

    fn bspline_curve_complex(&self, id: u64) -> Result<BSplineCurve<Point3>, StepError> {
        let (deg, ids, (m, kn)) = self.bspline_parts(id)?;
        let pts = self.points(&ids)?;
        let kv = self.knots(id, deg, pts.len(), &m, &kn)?;
        BSplineCurve::try_new(kv, pts).map_err(|e| fail(format!("B-spline curve #{id}: {e}")))
    }

    fn nurbs_curve_complex(&self, id: u64) -> Result<NurbsCurve<Vector4>, StepError> {
        let (deg, ids, (m, kn)) = self.bspline_parts(id)?;
        let pts = self.points(&ids)?;
        let e = self.idx.get(id)?;
        let w = P::new(
            id,
            "RATIONAL_B_SPLINE_CURVE",
            e.part("RATIONAL_B_SPLINE_CURVE").unwrap_or(&[]),
        )
        .reals(0)?;
        if w.len() != pts.len() || w.iter().any(|&x| !(x > 0.0)) {
            return Err(fail(format!(
                "rational B-spline curve #{id}: weights must be positive, one per control point"
            )));
        }
        let kv = self.knots(id, deg, pts.len(), &m, &kn)?;
        let hom = pts
            .iter()
            .zip(&w)
            .map(|(p, &w)| Vector4::new(p.x * w, p.y * w, p.z * w, w))
            .collect();
        let bs = BSplineCurve::try_new(kv, hom)
            .map_err(|e| fail(format!("rational B-spline curve #{id}: {e}")))?;
        Ok(NurbsCurve::new(bs))
    }

    // -------------------------------------------------------------- surfaces

    /// Surface of a face (`face_geometry`), with its analytic description.
    pub fn surface(&self, sid: u64) -> Result<(Surface, Analytic), StepError> {
        let e = self.idx.get(sid)?;
        if let [(name, params)] = e.parts.as_slice() {
            let p = P::new(sid, name, params);
            match name.as_str() {
                "PLANE" => {
                    let pl = self.placement(p.r(1)?)?;
                    return Ok((
                        Surface::Plane(Plane::new(pl.o, pl.o + pl.x, pl.o + pl.y)),
                        Analytic::Plane,
                    ));
                }
                "CYLINDRICAL_SURFACE" => {
                    let pl = self.placement(p.r(1)?)?;
                    let r = self.length(&p, 2)?;
                    let a = Analytic::Cylinder {
                        o: pl.o,
                        z: pl.z,
                        r,
                    };
                    let base = pl.o + pl.x * r;
                    let generatrix = Curve::Line(Line(base, base + pl.z));
                    return Ok((revolved(generatrix, &pl, a)?, a));
                }
                "CONICAL_SURFACE" => {
                    let pl = self.placement(p.r(1)?)?;
                    let r = p.f(2)? * self.k;
                    let semi = p.f(3)? * self.angle;
                    if !(r >= 0.0) || !(semi > 0.0 && semi < FRAC_PI_2 - 1e-9) {
                        return Err(fail(format!(
                            "CONICAL_SURFACE #{sid}: invalid radius or semi-angle"
                        )));
                    }
                    let a = Analytic::Cone {
                        o: pl.o,
                        z: pl.z,
                        r,
                        semi,
                    };
                    let base = pl.o + pl.x * r;
                    let dir = pl.x * semi.tan() + pl.z;
                    let generatrix = Curve::Line(Line(base, base + dir));
                    return Ok((revolved(generatrix, &pl, a)?, a));
                }
                "SPHERICAL_SURFACE" => {
                    let pl = self.placement(p.r(1)?)?;
                    let r = self.length(&p, 2)?;
                    let a = Analytic::Sphere {
                        o: pl.o,
                        z: pl.z,
                        r,
                    };
                    // Meridian half circle from the south to the north pole in the
                    // x-z half plane.
                    let generatrix =
                        Curve::NurbsCurve(conic_arc(pl.o, pl.x, pl.z, r, r, -FRAC_PI_2, PI)?);
                    return Ok((revolved(generatrix, &pl, a)?, a));
                }
                "TOROIDAL_SURFACE" => {
                    let pl = self.placement(p.r(1)?)?;
                    let big = self.length(&p, 2)?;
                    let small = self.length(&p, 3)?;
                    if small >= big {
                        return Err(fail(format!(
                            "TOROIDAL_SURFACE #{sid}: self-intersecting torus (minor >= major radius) unsupported"
                        )));
                    }
                    let a = Analytic::Torus {
                        o: pl.o,
                        z: pl.z,
                        big,
                        small,
                    };
                    let generatrix = Curve::NurbsCurve(conic_arc(
                        pl.o + pl.x * big,
                        pl.x,
                        pl.z,
                        small,
                        small,
                        0.0,
                        2.0 * PI,
                    )?);
                    return Ok((revolved(generatrix, &pl, a)?, a));
                }
                "B_SPLINE_SURFACE_WITH_KNOTS" => {
                    // ('', ud, vd, ((pts)), form, uc, vc, si, (um), (vm), (uk), (vk), spec)
                    let (ud, vd) = (p.int(1)?, p.int(2)?);
                    let grid = p.ref_grid(3)?;
                    let net = self.net(sid, &grid)?;
                    let ku = self.knots(sid, ud, net.len(), &p.ints(8)?, &p.reals(10)?)?;
                    let kv = self.knots(sid, vd, net[0].len(), &p.ints(9)?, &p.reals(11)?)?;
                    let s = BSplineSurface::try_new((ku, kv), net)
                        .map_err(|e| fail(format!("B_SPLINE_SURFACE_WITH_KNOTS #{sid}: {e}")))?;
                    return Ok((Surface::BSplineSurface(s), Analytic::Freeform));
                }
                _ => {}
            }
        } else if e.has("B_SPLINE_SURFACE") && e.has("B_SPLINE_SURFACE_WITH_KNOTS") {
            return self.bspline_surface_complex(sid);
        }
        Err(fail(format!(
            "unsupported surface entity {} #{sid}",
            e.type_name()
        )))
    }

    fn net(&self, id: u64, grid: &[Vec<u64>]) -> Result<Vec<Vec<Point3>>, StepError> {
        let cols = grid.first().map(|r| r.len()).unwrap_or(0);
        if grid.is_empty() || cols == 0 || grid.iter().any(|r| r.len() != cols) {
            return Err(fail(format!("B-spline surface #{id}: ragged control net")));
        }
        grid.iter().map(|r| self.points(r)).collect()
    }

    fn bspline_surface_complex(&self, sid: u64) -> Result<(Surface, Analytic), StepError> {
        let e = self.idx.get(sid)?;
        let b = P::new(
            sid,
            "B_SPLINE_SURFACE",
            e.part("B_SPLINE_SURFACE").unwrap_or(&[]),
        );
        let k = P::new(
            sid,
            "B_SPLINE_SURFACE_WITH_KNOTS",
            e.part("B_SPLINE_SURFACE_WITH_KNOTS").unwrap_or(&[]),
        );
        // B_SPLINE_SURFACE(ud, vd, ((pts)), form, uc, vc, si)
        // B_SPLINE_SURFACE_WITH_KNOTS((um), (vm), (uk), (vk), spec)
        let (ud, vd) = (b.int(0)?, b.int(1)?);
        let net = self.net(sid, &b.ref_grid(2)?)?;
        let ku = self.knots(sid, ud, net.len(), &k.ints(0)?, &k.reals(2)?)?;
        let kv = self.knots(sid, vd, net[0].len(), &k.ints(1)?, &k.reals(3)?)?;
        if let Some(w) = e.part("RATIONAL_B_SPLINE_SURFACE") {
            let w = P::new(sid, "RATIONAL_B_SPLINE_SURFACE", w).real_grid(0)?;
            if w.len() != net.len()
                || w.iter().zip(&net).any(|(a, b)| a.len() != b.len())
                || w.iter().flatten().any(|&x| !(x > 0.0))
            {
                return Err(fail(format!(
                    "rational B-spline surface #{sid}: weights must be positive, one per control point"
                )));
            }
            let hom = net
                .iter()
                .zip(&w)
                .map(|(row, wr)| {
                    row.iter()
                        .zip(wr)
                        .map(|(p, &w)| Vector4::new(p.x * w, p.y * w, p.z * w, w))
                        .collect()
                })
                .collect();
            let s = BSplineSurface::try_new((ku, kv), hom)
                .map_err(|e| fail(format!("rational B-spline surface #{sid}: {e}")))?;
            Ok((
                Surface::NurbsSurface(NurbsSurface::new(s)),
                Analytic::Freeform,
            ))
        } else {
            let s = BSplineSurface::try_new((ku, kv), net)
                .map_err(|e| fail(format!("B-spline surface #{sid}: {e}")))?;
            Ok((Surface::BSplineSurface(s), Analytic::Freeform))
        }
    }
}

type KnotVecParts = (Vec<i64>, Vec<f64>);

/// Surface of revolution of `generatrix` about the placement axis, oriented so that its
/// normal matches the STEP convention for the analytic surface (outward: away from
/// the axis for cylinders and cones, from the centre for spheres, from the tube
/// centre for tori). The orientation is checked numerically at an interior point.
pub(super) fn revolved(
    generatrix: Curve,
    pl: &Placement,
    a: Analytic,
) -> Result<Surface, StepError> {
    // The normal of a revolved curve is d/dcurve x d/dangle; when it points inward the
    // generatrix is reversed (exact) rather than wrapping the surface in an inverted
    // Processor (Truck's tessellation was observed to mis-mesh inverted revolved
    // surfaces touching the axis, e.g. sphere caps).
    let mut generatrix = generatrix;
    for attempt in 0..2 {
        let s = Processor::new(RevolutedCurve::by_revolution(
            generatrix.clone(),
            pl.o,
            pl.z,
        ));
        let (t0, t1) = generatrix.range_tuple();
        // Interior parameter away from sphere poles and the seam.
        let (u, v) = (t0 + 0.37 * (t1 - t0), 1.0);
        let p = s.subs(u, v);
        let n = s.normal(u, v);
        let want =
            expected_normal(&a, p).ok_or_else(|| fail("degenerate surface sample".into()))?;
        if !n.x.is_finite() || n.dot(want).abs() < 0.5 {
            return Err(fail("surface of revolution normal check failed".into()));
        }
        if n.dot(want) > 0.0 {
            return Ok(Surface::RevolutedCurve(s));
        }
        if attempt == 0 {
            generatrix.invert();
        }
    }
    Err(fail(
        "surface of revolution orientation check failed".into(),
    ))
}

/// STEP's outward normal of an analytic surface at `p` (unit), if defined.
pub(super) fn expected_normal(a: &Analytic, p: Point3) -> Option<Vector3> {
    let radial = |o: Point3, z: Vector3| {
        let d = p - o;
        let r = d - z * d.dot(z);
        let m = r.magnitude();
        (m > 1e-12).then(|| r / m)
    };
    match *a {
        Analytic::Cylinder { o, z, .. } => radial(o, z),
        Analytic::Cone { o, z, semi, .. } => radial(o, z).map(|r| r * semi.cos() - z * semi.sin()),
        Analytic::Sphere { o, .. } => {
            let d = p - o;
            let m = d.magnitude();
            (m > 1e-12).then(|| d / m)
        }
        Analytic::Torus { o, z, big, .. } => radial(o, z).and_then(|r| {
            let d = p - (o + r * big);
            let m = d.magnitude();
            (m > 1e-12).then(|| d / m)
        }),
        Analytic::Plane | Analytic::Freeform => None,
    }
}

/// Exact distance from `p` to an analytic surface (None for planes/free-form, which
/// are checked through Truck's own projection).
pub(super) fn analytic_distance(a: &Analytic, p: Point3) -> Option<f64> {
    let cyl = |o: Point3, z: Vector3| {
        let d = p - o;
        let h = d.dot(z);
        ((d - z * h).magnitude(), h)
    };
    match *a {
        Analytic::Cylinder { o, z, r } => Some((cyl(o, z).0 - r).abs()),
        Analytic::Cone { o, z, r, semi } => {
            let (rho, h) = cyl(o, z);
            Some(((rho - r) * semi.cos() - h * semi.sin()).abs())
        }
        Analytic::Sphere { o, r, .. } => Some(((p - o).magnitude() - r).abs()),
        Analytic::Torus { o, z, big, small } => {
            let (rho, h) = cyl(o, z);
            Some(((rho - big).hypot(h) - small).abs())
        }
        Analytic::Plane | Analytic::Freeform => None,
    }
}

/// Exact rational quadratic arc of the conic c + a cos t x + b sin t y for
/// t in [t0, t0 + sweep], sweep in (0, 2 pi], as one multi-span curve (spans of at
/// most a quarter turn, double interior knots). An ellipse is the affine image of a
/// circle, and the control points are mapped by that affine map, so the curve is
/// exact.
pub(crate) fn conic_arc(
    c: Point3,
    x: Vector3,
    y: Vector3,
    a: f64,
    b: f64,
    t0: f64,
    sweep: f64,
) -> Result<NurbsCurve<Vector4>, StepError> {
    if !(sweep > 0.0 && sweep <= 2.0 * PI + 1e-12) {
        return Err(fail(format!("invalid arc sweep {sweep}")));
    }
    let n = ((sweep / FRAC_PI_2 - 1e-9).ceil() as usize).max(1);
    let phi = sweep / n as f64;
    let w = (phi / 2.0).cos();
    let at = |t: f64, s: f64| c + x * (a * s * t.cos()) + y * (b * s * t.sin());
    let hom = |p: Point3, w: f64| Vector4::new(p.x * w, p.y * w, p.z * w, w);
    let mut ctrl = Vec::with_capacity(2 * n + 1);
    let mut knots = vec![0.0; 3];
    ctrl.push(hom(at(t0, 1.0), 1.0));
    for j in 0..n {
        let ta = t0 + phi * j as f64;
        ctrl.push(hom(at(ta + phi / 2.0, 1.0 / w), w));
        ctrl.push(hom(at(ta + phi, 1.0), 1.0));
        let kk = (j + 1) as f64;
        if j + 1 < n {
            knots.extend([kk, kk]);
        } else {
            knots.extend([kk; 3]);
        }
    }
    let bs = BSplineCurve::try_new(KnotVec::from(knots), ctrl)
        .map_err(|e| fail(format!("arc construction failed: {e}")))?;
    Ok(NurbsCurve::new(bs))
}

/// Edge on a circle/ellipse (placement `pl`, semi axes a along x, b along y) from
/// `start` to `end` in the conic's positive sense; a full period when `closed`.
fn conic_edge(
    pl: &Placement,
    a: f64,
    b: f64,
    start: Point3,
    end: Point3,
    closed: bool,
) -> Result<Curve, StepError> {
    let ang = |p: Point3| {
        let d = p - pl.o;
        (d.dot(pl.y) / b).atan2(d.dot(pl.x) / a)
    };
    let t0 = ang(start);
    let mut sweep = (ang(end) - t0).rem_euclid(2.0 * PI);
    if closed || sweep < 1e-12 {
        sweep = 2.0 * PI;
    }
    Ok(Curve::NurbsCurve(conic_arc(
        pl.o, pl.x, pl.y, a, b, t0, sweep,
    )?))
}

/// Trim a bounded curve to the sub-range between `start` and `end` (exact knot
/// insertion). The whole curve is kept when its ends already are the vertices.
fn trim_curve(
    mut c: Curve,
    start: Point3,
    end: Point3,
    closed: bool,
    id: u64,
    ty: &str,
) -> Result<Curve, StepError> {
    let (t0, t1) = c.range_tuple();
    let ends_match = c.subs(t0).distance(start) < 1e-9 && c.subs(t1).distance(end) < 1e-9;
    if ends_match || closed {
        return Ok(c);
    }
    let find = |p: Point3| {
        c.search_nearest_parameter(p, None, 100)
            .ok_or_else(|| fail(format!("{ty} #{id}: edge vertex not on curve")))
    };
    let (a, b) = (find(start)?, find(end)?);
    if !(a < b) {
        return Err(fail(format!(
            "{ty} #{id}: edge vertices out of parameter order (periodic B-spline edges unsupported)"
        )));
    }
    let span = t1 - t0;
    if b < t1 - 1e-12 * span {
        let _ = c.cut(b);
    }
    if a > t0 + 1e-12 * span {
        c = c.cut(a);
    }
    Ok(c)
}
