//! R-CAD fork: fast, bit-identical evaluation of truck-modeling intersection curves.
//!
//! `IntersectionCurve::search_triple` projects a leader point onto both surfaces by Newton's
//! method. For a B-spline or NURBS surface without a hint, Truck first runs `presearch`: it
//! evaluates the surface on a fixed (`PRESEARCH_DIVISION` + 1)² parameter grid and starts Newton
//! from the grid point nearest the query. That grid depends only on the surface, yet it was rebuilt
//! for every evaluated point, so dividing the implicit edges of an earlier boolean result (and any
//! other sampling of them) dominated the next boolean.
//!
//! [`search_triple`] computes the same grid once per surface (cached by the surface's exact bits),
//! picks the nearest grid point with the same comparisons in the same order, and hands it to
//! Truck's own `double_projection` as the hint. Every floating-point operation that decides the
//! result is the one Truck performs, so the result is bitwise identical to
//! `IntersectionCurve::search_triple`; only the repeated surface evaluations are gone.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use truck_modeling::*;

/// truck-geometry's private `PRESEARCH_DIVISION` (truck-geometry 0.5.0 `lib.rs`). The tests compare
/// against Truck's own evaluation, so a change there shows up as a test failure, not as silently
/// different geometry.
const PRESEARCH_DIVISION: usize = 50;

/// Most cached grids (each 2601 points); the cache is cleared beyond this.
const MAX_GRIDS: usize = 256;

/// Presearch grid of one surface: points in Truck's (i, j) loop order.
struct Grid {
    params: Vec<(f64, f64)>,
    points: Vec<Point3>,
}

impl Grid {
    fn build<S>(surface: &S) -> Self
    where
        S: BoundedSurface<Point = Point3>,
    {
        let ((u0, u1), (v0, v1)) = surface.range_tuple();
        let n = PRESEARCH_DIVISION;
        let mut params = Vec::with_capacity((n + 1) * (n + 1));
        let mut points = Vec::with_capacity((n + 1) * (n + 1));
        for i in 0..=n {
            for j in 0..=n {
                // Same expressions as `truck_geotrait::algo::surface::presearch`.
                let p = i as f64 / n as f64;
                let q = j as f64 / n as f64;
                let u = u0 * (1.0 - p) + u1 * p;
                let v = v0 * (1.0 - q) + v1 * q;
                params.push((u, v));
                points.push(surface.subs(u, v));
            }
        }
        Self { params, points }
    }

    /// The grid parameter nearest `point` (first minimum, strict `<`, as `presearch` does;
    /// `(0, 0)` if no distance compares).
    fn nearest(&self, point: Point3) -> (f64, f64) {
        let mut res = (0.0, 0.0);
        let mut min = f64::INFINITY;
        for (uv, p) in self.params.iter().zip(&self.points) {
            let dist = p.distance2(point);
            if dist < min {
                min = dist;
                res = *uv;
            }
        }
        res
    }
}

/// Exact bit pattern of a surface's knots and control points (with lengths), the cache key: equal
/// keys give equal `subs` everywhere.
fn key_of<P, const N: usize>(
    tag: u64,
    knots: &(KnotVec, KnotVec),
    ctrl: &[Vec<P>],
    coords: impl Fn(&P) -> [f64; N],
) -> Vec<u64> {
    let mut k = vec![
        tag,
        knots.0.len() as u64,
        knots.1.len() as u64,
        ctrl.len() as u64,
    ];
    k.extend(knots.0.iter().chain(knots.1.iter()).map(|x| x.to_bits()));
    for row in ctrl {
        k.push(row.len() as u64);
        for p in row {
            k.extend(coords(p).map(f64::to_bits));
        }
    }
    k
}

type GridCache = HashMap<Vec<u64>, Arc<Grid>>;
static GRIDS: Mutex<Option<GridCache>> = Mutex::new(None);

fn cached(key: Vec<u64>, build: impl FnOnce() -> Grid) -> Arc<Grid> {
    if let Ok(g) = GRIDS.lock() {
        if let Some(hit) = g.as_ref().and_then(|m| m.get(&key)) {
            return hit.clone();
        }
    }
    let grid = Arc::new(build());
    if let Ok(mut g) = GRIDS.lock() {
        let m = g.get_or_insert_with(HashMap::new);
        if m.len() >= MAX_GRIDS {
            m.clear();
        }
        m.insert(key, grid.clone());
    }
    grid
}

/// The hint Truck's `search_nearest_parameter(point, None, _)` computes, for the surfaces whose
/// unhinted search is a plain presearch; `None` (Truck computes its own) for the rest.
fn presearch_hint(surface: &Surface, point: Point3) -> Option<(f64, f64)> {
    match surface {
        Surface::NurbsSurface(s) => {
            let key = key_of(1, s.knot_vecs(), s.control_points(), |p| {
                [p.x, p.y, p.z, p.w]
            });
            Some(cached(key, || Grid::build(s)).nearest(point))
        }
        Surface::BSplineSurface(s) => {
            let key = key_of(2, s.knot_vecs(), s.control_points(), |p| [p.x, p.y, p.z]);
            Some(cached(key, || Grid::build(s)).nearest(point))
        }
        Surface::Plane(_) | Surface::RevolutedCurve(_) => None,
    }
}

/// `ic.search_triple(t)`, bit for bit, without re-evaluating the presearch grids for every point.
pub fn search_triple(
    ic: &IntersectionCurve<Leader, Surface>,
    t: f64,
) -> Option<(Point3, Point2, Point2)> {
    let point = ic.leader().subs(t);
    let (s0, s1) = (ic.surface0(), ic.surface1());
    double_projection(
        s0,
        presearch_hint(s0, point),
        s1,
        presearch_hint(s1, point),
        point,
        ic.leader().der(t),
        100,
    )
}

/// An intersection curve whose `subs` and `der` use [`search_triple`]: the same values as the
/// wrapped curve's, so Truck's generic algorithms (e.g. `algo::curve::parameter_division`) give
/// bit-identical results on it.
#[derive(Clone, Copy, Debug)]
pub struct FastIntersection<'a>(pub &'a IntersectionCurve<Leader, Surface>);

impl ParametricCurve for FastIntersection<'_> {
    type Point = Point3;
    type Vector = Vector3;
    fn subs(&self, t: f64) -> Point3 {
        // `IntersectionCurve::subs` unwraps too.
        search_triple(self.0, t).unwrap().0
    }
    fn der(&self, t: f64) -> Vector3 {
        // `IntersectionCurve::der`, with the triple from `search_triple`.
        let n = self.0.leader().der(t);
        let (_, p0, p1) = search_triple(self.0, t).unwrap();
        let d = self
            .0
            .surface0()
            .normal(p0.x, p0.y)
            .cross(self.0.surface1().normal(p1.x, p1.y))
            .normalize();
        d * (n.dot(n) / d.dot(n))
    }
    fn der2(&self, t: f64) -> Vector3 {
        self.0.der2(t)
    }
    fn parameter_range(&self) -> ParameterRange {
        self.0.parameter_range()
    }
}

/// The hint that makes `surface.search_nearest_parameter(point, hint, n)` equal to the unhinted
/// search bit for bit: the cached presearch result for a truck-modeling B-spline/NURBS
/// [`Surface`], `None` for any other surface (Truck then searches as before).
pub fn nearest_hint<S: 'static>(surface: &S, point: Point3) -> Option<(f64, f64)> {
    let any: &dyn std::any::Any = surface;
    any.downcast_ref::<Surface>()
        .and_then(|s| presearch_hint(s, point))
}

/// `curve.parameter_division(range, tol)`, bit for bit; a truck-modeling intersection curve is
/// divided through [`FastIntersection`].
pub fn parameter_division<C>(curve: &C, range: (f64, f64), tol: f64) -> (Vec<f64>, Vec<Point3>)
where
    C: ParameterDivision1D<Point = Point3> + 'static,
{
    let any: &dyn std::any::Any = curve;
    if let Some(Curve::IntersectionCurve(ic)) = any.downcast_ref::<Curve>() {
        return algo::curve::parameter_division(&FastIntersection(ic), range, tol);
    }
    curve.parameter_division(range, tol)
}

/// A truck-modeling [`Curve`] whose evaluation and division go through [`FastIntersection`] for
/// intersection curves: the same values bit for bit, so a shape mapped to it tessellates exactly
/// like the original (truck-meshalgo divides every edge with `parameter_division`).
#[derive(Clone, Debug)]
pub struct FastCurve(pub Curve);

impl ParametricCurve for FastCurve {
    type Point = Point3;
    type Vector = Vector3;
    fn subs(&self, t: f64) -> Point3 {
        match &self.0 {
            Curve::IntersectionCurve(ic) => FastIntersection(ic).subs(t),
            c => c.subs(t),
        }
    }
    fn der(&self, t: f64) -> Vector3 {
        match &self.0 {
            Curve::IntersectionCurve(ic) => FastIntersection(ic).der(t),
            c => c.der(t),
        }
    }
    fn der2(&self, t: f64) -> Vector3 {
        self.0.der2(t)
    }
    fn parameter_range(&self) -> ParameterRange {
        self.0.parameter_range()
    }
    fn period(&self) -> Option<f64> {
        self.0.period()
    }
}

impl BoundedCurve for FastCurve {}

impl ParameterDivision1D for FastCurve {
    type Point = Point3;
    fn parameter_division(&self, range: (f64, f64), tol: f64) -> (Vec<f64>, Vec<Point3>) {
        parameter_division(&self.0, range, tol)
    }
}

/// `shell` with its curves wrapped in [`FastCurve`] (same topology, order and geometry), for
/// tessellating it.
pub fn fast_shell(shell: &Shell) -> truck_topology::Shell<Point3, FastCurve, Surface> {
    shell.mapped(Point3::clone, |c| FastCurve(c.clone()), Surface::clone)
}

/// `solid` with its curves wrapped in [`FastCurve`] (same topology, order and geometry), for
/// tessellating it.
pub fn fast_solid(solid: &Solid) -> truck_topology::Solid<Point3, FastCurve, Surface> {
    solid.mapped(Point3::clone, |c| FastCurve(c.clone()), Surface::clone)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Truck's raw booleans: a blind hole in a block (plane–cylinder curves) and two overlapping
    /// upright cylinders (cylinder–cylinder curves, NURBS on both sides).
    fn raw_booleans() -> Vec<Solid> {
        let cyl = |c: Point3, r: f64, h: f64| {
            let v = builder::vertex(c + Vector3::new(r, 0.0, 0.0));
            let w = builder::rsweep(&v, c, Vector3::unit_z(), Rad(7.0));
            let f = builder::try_attach_plane(&[w]).unwrap();
            builder::tsweep(&f, Vector3::new(0.0, 0.0, h))
        };
        let v = builder::vertex(Point3::origin());
        let e = builder::tsweep(&v, Vector3::new(60.0, 0.0, 0.0));
        let f = builder::tsweep(&e, Vector3::new(0.0, 40.0, 0.0));
        let block = builder::tsweep(&f, Vector3::new(0.0, 0.0, 10.0));
        let mut hole = cyl(Point3::new(20.0, 20.0, 4.0), 6.0, 10.0);
        hole.not();
        let a = cyl(Point3::new(27.0, 22.0, -1.0), 5.0, 8.0);
        let b = cyl(Point3::new(20.0, 20.0, -2.0), 6.0, 12.0);
        vec![
            crate::and(&block, &hole, 0.05).expect("block minus hole"),
            crate::and(&a, &b, 0.05).expect("cylinder and cylinder"),
        ]
    }

    #[test]
    fn matches_truck_bit_for_bit() {
        let bits = |p: Point3| [p.x, p.y, p.z].map(f64::to_bits);
        let (mut compared, mut both_nurbs) = (0, 0);
        for solid in raw_booleans() {
            for e in solid.edge_iter() {
                let curve = e.curve();
                let Curve::IntersectionCurve(ic) = &curve else {
                    continue;
                };
                let nurbs = |s: &Surface| matches!(s, Surface::NurbsSurface(_));
                if nurbs(ic.surface0()) && nurbs(ic.surface1()) {
                    both_nurbs += 1;
                }
                let (t0, t1) = ic.range_tuple();
                for k in 0..=20 {
                    let t = t0 + (t1 - t0) * k as f64 / 20.0;
                    let tri = |x: Option<(Point3, Point2, Point2)>| {
                        x.map(|(p, a, b)| [p.x, p.y, p.z, a.x, a.y, b.x, b.y].map(f64::to_bits))
                    };
                    assert_eq!(tri(search_triple(ic, t)), tri(ic.search_triple(t)));
                    let (fast, slow) = (FastIntersection(ic), ic);
                    assert_eq!(bits(fast.subs(t)), bits(slow.subs(t)));
                    let d = |v: Vector3| [v.x, v.y, v.z].map(f64::to_bits);
                    assert_eq!(d(fast.der(t)), d(slow.der(t)));
                    // The hinted projection `IntersectionCurveWithParameters::try_new` uses.
                    let (s0, s1) = (ic.surface0(), ic.surface1());
                    let (p, n) = (
                        ic.leader().subs(t) + Vector3::new(1e-3, -2e-3, 1e-3),
                        Vector3::unit_z(),
                    );
                    let hinted = double_projection(
                        s0,
                        nearest_hint(s0, p),
                        s1,
                        nearest_hint(s1, p),
                        p,
                        n,
                        100,
                    );
                    assert_eq!(
                        tri(hinted),
                        tri(double_projection(s0, None, s1, None, p, n, 100))
                    );
                    compared += 1;
                }
                let (pa, qa) = parameter_division(&curve, (t0, t1), 0.01);
                let (pb, qb) = curve.parameter_division((t0, t1), 0.01);
                assert_eq!(
                    pa.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                    pb.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
                );
                assert_eq!(
                    qa.into_iter().map(bits).collect::<Vec<_>>(),
                    qb.into_iter().map(bits).collect::<Vec<_>>()
                );
            }
        }
        assert!(
            compared > 0 && both_nurbs > 0,
            "{compared} points, {both_nurbs} NURBS-NURBS curves"
        );
    }

    #[test]
    fn fast_curves_tessellate_identically() {
        use truck_meshalgo::tessellation::MeshableShape;
        let bits =
            |s: &truck_topology::Solid<Point3, _, Option<truck_meshalgo::prelude::PolygonMesh>>| {
                s.face_iter()
                    .map(|f| {
                        let m = f.surface().expect("face tessellated");
                        let p: Vec<[u64; 3]> = m
                            .positions()
                            .iter()
                            .map(|p| [p.x, p.y, p.z].map(f64::to_bits))
                            .collect();
                        (p, format!("{:?}", m.faces()))
                    })
                    .collect::<Vec<_>>()
            };
        for solid in raw_booleans() {
            let a = solid.triangulation(0.01);
            let b = fast_solid(&solid).triangulation(0.01);
            assert_eq!(bits(&a), bits(&b));
        }
    }
}
