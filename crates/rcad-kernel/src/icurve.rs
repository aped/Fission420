//! Fast, bit-identical evaluation of Truck boolean intersection curves.
//!
//! `IntersectionCurve::search_triple` projects a leader point onto both
//! surfaces by Newton's method. For a B-spline or NURBS surface without a hint,
//! Truck first runs `presearch`: it evaluates the surface on a fixed
//! (`PRESEARCH_DIVISION` + 1)² parameter grid and starts Newton from the grid
//! point nearest the query. That grid depends only on the surface, yet it is
//! rebuilt for every evaluated point, which made curve sampling (bounds,
//! tessellation, face loops, volumes) dominate cut regeneration.
//!
//! [`search_triple`] computes the same grid once per surface (cached by the
//! surface's exact bits), picks the nearest grid point with the same
//! comparisons in the same order, and hands it to Truck's own
//! `double_projection` as the hint. Every floating-point operation that decides
//! the result is the one Truck performs, so the returned triple is bitwise
//! identical to `search_triple` (checked in the tests below); only the
//! repeated surface evaluations are gone.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use truck_modeling::*;

/// Truck-geometry's private `PRESEARCH_DIVISION` (truck-geometry 0.5.0
/// `lib.rs`); the tests compare against Truck's own evaluation, so a change
/// there shows up as a test failure, not as silently different geometry.
const PRESEARCH_DIVISION: usize = 50;

/// Most cached grids (each ~2601 points); the cache is cleared beyond this.
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

    /// The grid parameter nearest `point` (first minimum, strict `<`, as
    /// `presearch` does; `(0, 0)` if no distance compares).
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

/// Exact bit pattern of a surface's knots and control points (with lengths),
/// the cache key: equal keys give equal `subs` everywhere.
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

static GRIDS: Mutex<Option<HashMap<Vec<u64>, Arc<Grid>>>> = Mutex::new(None);

fn cached(key: Vec<u64>, build: impl FnOnce() -> Grid) -> Arc<Grid> {
    if let Ok(g) = GRIDS.lock()
        && let Some(hit) = g.as_ref().and_then(|m| m.get(&key))
    {
        return hit.clone();
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

/// The hint Truck's `search_nearest_parameter(point, None, _)` would compute,
/// for the surfaces whose unhinted search is a plain presearch; `None` (Truck
/// computes its own) for the rest.
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

/// `ic.search_triple(t)`, bit for bit, without re-evaluating the presearch
/// grids for every point.
pub(crate) fn search_triple(
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Truck's raw booleans (no exact edge rebuild): a blind hole in a plate
    /// (plane–cylinder curves) and two overlapping upright cylinders
    /// (cylinder–cylinder curves, NURBS on both sides).
    fn raw_booleans() -> Vec<Solid> {
        let cyl = |c: [f64; 2], r: f64, z0: f64, h: f64| {
            let c = crate::Profile::Circle {
                center: c,
                radius: r,
            };
            crate::extrude(&c, &crate::Frame::XY.offset(z0), h)
                .unwrap()
                .solid
        };
        let p = crate::Profile::Polygon(vec![[0.0, 0.0], [60.0, 0.0], [60.0, 40.0], [0.0, 40.0]]);
        let plate = crate::extrude(&p, &crate::Frame::XY, 10.0).unwrap().solid;
        let mut hole = cyl([20.0, 20.0], 6.0, 4.0, 10.0);
        hole.not();
        let a = cyl([27.0, 22.0], 5.0, -1.0, 8.0);
        let b = cyl([20.0, 20.0], 6.0, -2.0, 12.0);
        [
            truck_shapeops::and(&plate, &hole, crate::tol::BOOLEAN),
            truck_shapeops::and(&a, &b, crate::tol::BOOLEAN),
        ]
        .into_iter()
        .map(|s| s.expect("fixture boolean"))
        .collect()
    }

    #[test]
    fn matches_truck_search_triple_bit_for_bit() {
        let (mut compared, mut both_nurbs) = (0, 0);
        for e in raw_booleans().iter().flat_map(|s| s.edge_iter()) {
            let Curve::IntersectionCurve(ic) = e.curve() else {
                continue;
            };
            let nurbs = |s: &Surface| matches!(s, Surface::NurbsSurface(_));
            if nurbs(ic.surface0()) && nurbs(ic.surface1()) {
                both_nurbs += 1;
            }
            let (t0, t1) = ic.range_tuple();
            for k in 0..=40 {
                let t = t0 + (t1 - t0) * k as f64 / 40.0;
                let fast = search_triple(&ic, t);
                let slow = ic.search_triple(t);
                let bits = |x: Option<(Point3, Point2, Point2)>| {
                    x.map(|(p, a, b)| [p.x, p.y, p.z, a.x, a.y, b.x, b.y].map(f64::to_bits))
                };
                assert_eq!(bits(fast), bits(slow), "t = {t}");
                compared += 1;
            }
        }
        eprintln!("compared {compared} points, {both_nurbs} NURBS-NURBS curves");
        assert!(compared > 0 && both_nurbs > 0, "{compared} points, {both_nurbs} NURBS-NURBS curves");
    }
}
