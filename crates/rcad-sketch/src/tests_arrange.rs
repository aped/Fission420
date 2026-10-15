//! Regions of crossing, touching and T-joined curves (`arrange.rs`).
//! Areas are closed forms: the lens of two circles of radius r with centres
//! d apart is 2r²·acos(d/2r) − (d/2)√(4r² − d²); rectangles split by lines.
use super::*;
use std::f64::consts::PI;

const AREA_TOL: f64 = 1e-6;

fn line(s: &mut Sketch, a: [f64; 2], b: [f64; 2]) -> EntityId {
    let p = s.add_point(a[0], a[1]);
    let q = s.add_point(b[0], b[1]);
    s.add_line(p, q)
}

/// Closed 40 x 20 rectangle with shared corners.
fn rectangle(s: &mut Sketch) -> Vec<EntityId> {
    let p: Vec<PointId> = [[0.0, 0.0], [40.0, 0.0], [40.0, 20.0], [0.0, 20.0]]
        .iter()
        .map(|q| s.add_point(q[0], q[1]))
        .collect();
    (0..4).map(|i| s.add_line(p[i], p[(i + 1) % 4])).collect()
}

fn areas(s: &Sketch) -> Vec<f64> {
    let mut a: Vec<f64> = s.regions().unwrap().iter().map(|r| r.area).collect();
    a.sort_by(f64::total_cmp);
    a
}

#[test]
fn two_overlapping_circles_give_a_lens_and_two_crescents() {
    let mut s = Sketch::default();
    let c1 = s.add_point(0.0, 0.0);
    let c2 = s.add_point(10.0, 0.0);
    let k1 = s.add_circle(c1, 10.0);
    let k2 = s.add_circle(c2, 10.0);
    let (r, d) = (10.0f64, 10.0f64);
    let lens = 2.0 * r * r * (d / (2.0 * r)).acos() - 0.5 * d * (4.0 * r * r - d * d).sqrt();
    let crescent = PI * r * r - lens;
    let a = areas(&s);
    assert_eq!(a.len(), 3, "{a:?}");
    assert!((a[0] - lens).abs() < AREA_TOL, "{} vs {lens}", a[0]);
    assert!((a[1] - crescent).abs() < AREA_TOL && (a[2] - crescent).abs() < AREA_TOL);
    // Identities: all three are bounded by both circles; the sides differ.
    let rs = s.regions().unwrap();
    for r in &rs {
        assert_eq!(r.id.outer, vec![k1, k2]);
    }
    let lens_id = s.region_at([5.0, 0.0]).unwrap().expect("lens picked");
    assert!(lens_id.outside.is_empty(), "inside both circles");
    let left = s.region_at([-5.0, 0.0]).unwrap().unwrap();
    assert_eq!(left.outside, vec![k2]);
    let right = s.region_at([15.0, 0.0]).unwrap().unwrap();
    assert_eq!(right.outside, vec![k1]);
    // Exact arcs, no polygonisation.
    let lr = s.region_by_ref(&lens_id).unwrap().unwrap();
    assert!(matches!(lr.outer, Profile::Loop(ref v) if v.len() == 2
        && v.iter().all(|x| matches!(x, LoopSegment::Arc { .. }))));
}

#[test]
fn a_line_across_a_rectangle_splits_it_and_overhangs_are_ignored() {
    let mut s = Sketch::default();
    rectangle(&mut s);
    line(&mut s, [15.0, -5.0], [15.0, 25.0]);
    assert_eq!(areas(&s), vec![300.0, 500.0]);
    let rs = s.regions().unwrap();
    for r in &rs {
        assert!(matches!(r.region.outer, Profile::Polygon(ref v) if v.len() == 4));
    }
    assert!(s.region_at([5.0, 10.0]).unwrap().is_some());
    assert!(
        s.region_at([15.0, 10.0]).unwrap().is_none(),
        "on the split line"
    );
}

#[test]
fn t_junctions_split_and_a_dangling_stem_is_ignored() {
    // A line from edge to edge, ending exactly on them (T-junctions).
    let mut s = Sketch::default();
    rectangle(&mut s);
    line(&mut s, [25.0, 0.0], [25.0, 20.0]);
    assert_eq!(areas(&s), vec![300.0, 500.0]);
    // A stem from the bottom edge into the interior: one region, 800 mm²,
    // its bottom edge one segment again.
    let mut t = Sketch::default();
    rectangle(&mut t);
    line(&mut t, [20.0, 0.0], [20.0, 10.0]);
    let rs = t.regions().unwrap();
    assert_eq!(rs.len(), 1);
    assert!((rs[0].area - 800.0).abs() < AREA_TOL);
    assert!(matches!(rs[0].region.outer, Profile::Polygon(ref v) if v.len() == 4));
}

#[test]
fn a_circle_inside_a_split_rectangle_is_a_hole_of_its_face() {
    let mut s = Sketch::default();
    rectangle(&mut s);
    line(&mut s, [15.0, 0.0], [15.0, 20.0]);
    let c = s.add_point(30.0, 10.0);
    s.add_circle(c, 4.0);
    let a = areas(&s);
    assert_eq!(a.len(), 3, "{a:?}");
    let want = [PI * 16.0, 300.0, 500.0 - PI * 16.0];
    let mut want = want.to_vec();
    want.sort_by(f64::total_cmp);
    for (g, w) in a.iter().zip(&want) {
        assert!((g - w).abs() < AREA_TOL, "{a:?} vs {want:?}");
    }
}

#[test]
fn ellipses_in_crossing_sketches_are_refused_by_name() {
    let mut s = Sketch::default();
    rectangle(&mut s);
    line(&mut s, [15.0, -5.0], [15.0, 25.0]);
    s.add_ellipse([30.0, 10.0], [34.0, 10.0], 2.0).unwrap();
    let err = s.regions().unwrap_err();
    assert!(err.to_string().contains("ellipse or spline"), "{err}");
}
