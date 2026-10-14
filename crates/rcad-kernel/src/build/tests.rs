//! Boolean adapter tests that need crate internals: exact replacement of intersection
//! edges (`crate::exact`) and the preserved fragile-boolean regression.
//! Expected values are derived analytically here, not from kernel output.
use super::*;
use crate::exact::{VERIFY_TOL, exact_edge};
use crate::{VolumeResult, check_mesh, measure::measure, tessellate};
use std::collections::HashSet;
use std::f64::consts::PI;

fn rect(w: f64, h: f64) -> Profile {
    Profile::Polygon(vec![
        [-w / 2.0, -h / 2.0],
        [w / 2.0, -h / 2.0],
        [w / 2.0, h / 2.0],
        [-w / 2.0, h / 2.0],
    ])
}

fn circle(center: [f64; 2], radius: f64) -> Profile {
    Profile::Circle { center, radius }
}

/// Unique edges of a solid.
fn edges(body: &Body) -> Vec<Edge> {
    let mut seen = HashSet::new();
    body.solid
        .edge_iter()
        .filter(|e| seen.insert(e.id()))
        .collect()
}

fn count_intersection_edges(body: &Body) -> usize {
    edges(body)
        .iter()
        .filter(|e| matches!(e.curve(), Curve::IntersectionCurve(_)))
        .count()
}

/// One through-all boolean attempt with the first seam and policy tolerance, without
/// retries and without exact-edge replacement (Truck's raw output).
fn raw_cut(target: &Body, profile: &Profile, frame: &Frame) -> Result<Body> {
    let (start, length) = through_all_cutter(target, frame)?;
    subtract_attempt(
        target,
        profile,
        &[],
        &start,
        length,
        0.0,
        tol::BOOLEAN,
        "cut",
        false,
    )
}

fn volume_of(body: &Body) -> f64 {
    match measure(body).unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        VolumeResult::Unavailable { reason } => panic!("volume unavailable: {reason}"),
    }
}

/// Samples (33 per edge) of every non-line edge of `body`.
fn curved_edge_samples(body: &Body) -> Vec<Vec<Point3>> {
    edges(body)
        .iter()
        .filter(|e| !matches!(e.curve(), Curve::Line(_)))
        .map(|e| {
            let c = e.curve();
            let (t0, t1) = c.range_tuple();
            (0..=32)
                .map(|i| c.subs(t0 + (t1 - t0) * i as f64 / 32.0))
                .collect()
        })
        .collect()
}

#[test]
fn hole_edges_become_exact_circles() {
    let (w, h, d, r) = (80.0, 50.0, 10.0, 5.0);
    let plate = extrude(&rect(w, h), &Frame::XY, d).unwrap();
    let raw = raw_cut(&plate, &circle([0.0, 0.0], r), &Frame::XY.offset(d)).unwrap();
    // Truck's boolean output: the hole rims are implicit intersection curves.
    assert_eq!(count_intersection_edges(&raw), 6);

    let (exact, stats) = exact::exactify(&raw.solid);
    assert_eq!(stats.replaced, 6);
    assert_eq!(stats.kept, 0);
    let exact = Body {
        solid: exact.unwrap(),
    };
    assert_eq!(count_intersection_edges(&exact), 0);
    // Topology unchanged.
    assert_eq!(edges(&exact).len(), edges(&raw).len());
    assert_eq!(
        exact.solid.face_iter().count(),
        raw.solid.face_iter().count()
    );
    check_single_closed(&exact, "test").unwrap();

    // Independent geometry: every curved edge lies on the circle x^2 + y^2 = r^2 at
    // z = 0 or z = d (the cutter's own arcs are outside the plate and are gone).
    let samples = curved_edge_samples(&exact);
    assert_eq!(samples.len(), 6);
    for pts in samples {
        let z = pts[0].z;
        assert!(z.abs() < 1e-12 || (z - d).abs() < 1e-12, "rim at z = {z}");
        for p in pts {
            assert!((p.x.hypot(p.y) - r).abs() < 1e-9, "{p:?} off the circle");
            assert!((p.z - z).abs() < 1e-12);
        }
    }

    // Exact B-rep volume vs the analytic value, and agreement with the volume
    // integrated over the original intersection curves.
    let expected = w * h * d - PI * r * r * d;
    let v_exact = volume_of(&exact);
    assert!((v_exact - expected).abs() < 1e-6, "{v_exact} vs {expected}");
    let v_raw = volume_of(&raw);
    assert!((v_raw - expected).abs() < 1e-3, "{v_raw} vs {expected}");
}

#[test]
fn exact_edge_rejects_wrong_vertices() {
    let plate = extrude(&rect(40.0, 30.0), &Frame::XY, 5.0).unwrap();
    let raw = raw_cut(&plate, &circle([3.0, -2.0], 4.0), &Frame::XY.offset(5.0)).unwrap();
    let cs = raw.solid.compress();
    let shell = &cs.boundaries[0];
    let mut checked = 0;
    for e in &shell.edges {
        let Curve::IntersectionCurve(ic) = &e.curve else {
            continue;
        };
        let (a, b) = (shell.vertices[e.vertices.0], shell.vertices[e.vertices.1]);
        assert!(exact_edge(ic, a, b).is_ok());
        // Reversed direction: the rebuilt curve would run against Truck's edge.
        assert!(exact_edge(ic, b, a).is_err());
        // A vertex off the curve by more than the verification tolerance.
        let off = a + Vector3::new(0.0, 0.0, 1.0e3 * VERIFY_TOL);
        assert!(exact_edge(ic, off, b).is_err());
        let off = a + (a - Point3::new(3.0, -2.0, a.z)).normalize() * (1.0e3 * VERIFY_TOL);
        assert!(exact_edge(ic, off, b).is_err());
        checked += 1;
    }
    assert!(checked > 0);
}

/// A through hole drilled from a side face (frame normal -Y): the plane/cylinder
/// intersections are circles in the planes y = +-h/2 about the axis x = 0, z = d/2.
#[test]
fn side_hole_edges_are_exact() {
    let (w, h, d, r) = (80.0, 50.0, 10.0, 3.0);
    let plate = extrude(&rect(w, h), &Frame::XY, d).unwrap();
    // Frame::XZ: local x = world X, local y = world Z, normal -Y.
    let side = Frame::XZ.offset(h / 2.0);
    let body = cut_through_all(&plate, &circle([0.0, d / 2.0], r), &side).unwrap();
    assert_eq!(count_intersection_edges(&body), 0);
    let samples = curved_edge_samples(&body);
    assert!(!samples.is_empty());
    for pts in samples {
        let y = pts[0].y;
        assert!((y.abs() - h / 2.0).abs() < 1e-12, "rim at y = {y}");
        for p in pts {
            assert!((p.x.hypot(p.z - d / 2.0) - r).abs() < 1e-9, "{p:?}");
            assert!((p.y - y).abs() < 1e-12);
        }
    }
    let mesh = check_mesh(&tessellate(&body, 0.01).unwrap());
    assert!(mesh.watertight);
    let expected = w * h * d - PI * r * r * h;
    assert!(
        (mesh.mesh_volume - expected).abs() / expected < 1e-3,
        "mesh volume {} vs {expected}",
        mesh.mesh_volume
    );
    // No implicit curves remain, so STEP export (which refuses them) succeeds.
    let step = crate::export_step(&body, "test").unwrap();
    assert!(!step.contains("INTERSECTION_CURVE"));
}

/// Cylinder/cylinder intersections are not plane/extrusion cases: they must keep
/// Truck's implicit intersection curve (correct, only slower), while the plane/
/// cylinder rims of the same boolean are replaced.
#[test]
fn unsupported_intersections_keep_original_curve() {
    let (w, h, d) = (80.0, 50.0, 10.0);
    let plate = extrude(&rect(w, h), &Frame::XY, d).unwrap();
    let holed = cut_through_all(&plate, &circle([0.0, 0.0], 5.0), &Frame::XY.offset(d)).unwrap();
    let side = Frame::XZ.offset(h / 2.0);
    let raw = raw_cut(&holed, &circle([0.0, d / 2.0], 2.0), &side).unwrap();
    let (exact, stats) = exact::exactify(&raw.solid);
    assert!(stats.kept > 0, "{stats:?}");
    assert!(stats.replaced > 0, "{stats:?}");
    let exact = Body {
        solid: exact.unwrap(),
    };
    assert_eq!(count_intersection_edges(&exact), stats.kept);
    check_single_closed(&exact, "test").unwrap();
    assert!(check_mesh(&tessellate(&exact, 0.01).unwrap()).watertight);
}

/// Preserved fragile case: on a 120 x 60 x 8 plate, a corner
/// Ø6.5 hole at (50, 20) followed by a centred Ø20 hole. With Truck 0.6,
/// `truck_shapeops::and` returns None for the second cut with the first seam angle at
/// every tolerance tried; the adapter's retries (other seam
/// angles, same exact geometry) succeed. The correctness contract is the public API's
/// result (exact volume, no implicit edges, watertight); how many first-seam attempts
/// fail is printed as a diagnostic only, so an upstream fix cannot break this test.
#[test]
fn fragile_corner_then_centre_hole_is_cut_exactly() {
    let (w, h, d) = (120.0, 60.0, 8.0);
    let top = Frame::XY.offset(d);
    let plate = extrude(&rect(w, h), &Frame::XY, d).unwrap();
    let corner = circle([50.0, 20.0], 3.25);
    let centre = circle([0.0, 0.0], 10.0);
    let first_raw = raw_cut(&plate, &corner, &top).unwrap();
    let first_exact = with_exact_edges(first_raw.clone());
    assert_eq!(count_intersection_edges(&first_exact), 0);
    for (label, first) in [("raw", &first_raw), ("exact", &first_exact)] {
        let (start, length) = through_all_cutter(first, &top).unwrap();
        let failed = BOOLEAN_TOLS
            .iter()
            .filter(|&&btol| {
                subtract_attempt(first, &centre, &[], &start, length, 0.0, btol, "cut", false)
                    .is_err()
            })
            .count();
        eprintln!(
            "diagnostic: {label} first hole, first seam: {failed}/{} boolean attempts failed \
             (Truck 0.6 measured: all)",
            BOOLEAN_TOLS.len()
        );
    }
    // With retries (the public API) it succeeds, exactly.
    let first = cut_through_all(&plate, &corner, &top).unwrap();
    let body = cut_through_all(&first, &centre, &top).unwrap();
    assert_eq!(count_intersection_edges(&body), 0);
    let expected = w * h * d - PI * (3.25f64.powi(2) + 10.0f64.powi(2)) * d;
    let v = volume_of(&body);
    assert!((v - expected).abs() < 1e-6, "{v} vs {expected}");
    assert!(check_mesh(&tessellate(&body, 0.01).unwrap()).watertight);
}
