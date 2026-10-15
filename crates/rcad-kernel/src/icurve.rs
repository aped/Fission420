//! Fast, bit-identical evaluation of Truck boolean intersection curves.
//!
//! `IntersectionCurve::search_triple` re-evaluates a 51 x 51 presearch grid
//! of each B-spline/NURBS surface for every point, which made sampling the
//! implicit edges of boolean results (bounds, face loops, volume, check
//! tessellation) dominate regeneration. The truck-shapeops fork's
//! `fast_curve::search_triple` caches each surface's grid and returns the
//! same triple bit for bit (see third_party/truck-shapeops/R-CAD-CHANGES.md);
//! every kernel call site samples intersection curves through it.
pub(crate) use truck_shapeops::fast_curve::search_triple;

#[cfg(test)]
mod tests {
    use super::*;
    use truck_modeling::*;

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
        assert!(
            compared > 0 && both_nurbs > 0,
            "{compared} points, {both_nurbs} NURBS-NURBS curves"
        );
    }
}
