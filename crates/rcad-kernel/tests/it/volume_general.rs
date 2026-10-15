//! Generalized B-rep volume: X/Y boundary integrals, tilted planar faces, the
//! surface-of-revolution integral and the tessellation fallback, against analytic
//! values.
use rcad_kernel::*;
use std::f64::consts::PI;

fn value(m: &Measurements) -> (f64, f64, VolumeMethod) {
    match m.volume {
        VolumeResult::Value {
            volume,
            error_estimate,
            method,
        } => (volume, error_estimate, method),
        VolumeResult::Unavailable { ref reason } => panic!("volume unavailable: {reason}"),
    }
}

/// Exact-path acceptance tolerance (same as the plate tests).
fn exact_tol(v: f64) -> f64 {
    0.001f64.max(v.abs() * 1e-7)
}

fn check_exact(body: &Body, expected: f64, method: VolumeMethod, label: &str) {
    let m = measure(body).unwrap();
    assert_eq!(m.shell_count, 1, "{label}");
    assert!(m.all_shells_closed, "{label}");
    let (v, est, got) = value(&m);
    eprintln!(
        "{label}: V = {v:.9}, expected {expected:.9}, err {:.3e}, est {est:.3e}, {got:?}",
        v - expected
    );
    assert_eq!(got, method, "{label}");
    assert!(
        (v - expected).abs() <= exact_tol(expected),
        "{label}: {v} vs {expected} (est {est})"
    );
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Profile {
    Profile::Polygon(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
}

fn hole(r: f64) -> Profile {
    Profile::Circle {
        center: [0.0, 0.0],
        radius: r,
    }
}

/// Frame at `origin` whose normal is +X (local x = Y, local y = Z).
fn frame_x(origin: [f64; 3]) -> Frame {
    Frame {
        origin,
        x: [0.0, 1.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [1.0, 0.0, 0.0],
    }
}

/// Frame at `origin` whose normal is +Y (local x = Z, local y = X).
fn frame_y(origin: [f64; 3]) -> Frame {
    Frame {
        origin,
        x: [0.0, 0.0, 1.0],
        y: [1.0, 0.0, 0.0],
        z: [0.0, 1.0, 0.0],
    }
}

fn box_60x40x20() -> Body {
    extrude(&rect(0.0, 0.0, 60.0, 40.0), &Frame::XY, 20.0).unwrap()
}

#[test]
fn side_hole_along_x() {
    let body = cut_through_all(&box_60x40x20(), &hole(3.0), &frame_x([60.0, 20.0, 10.0])).unwrap();
    check_exact(
        &body,
        48000.0 - 9.0 * PI * 60.0,
        VolumeMethod::BrepBoundaryIntegralX,
        "hole along X",
    );
}

#[test]
fn side_hole_along_y() {
    let body = cut_through_all(&box_60x40x20(), &hole(3.0), &frame_y([30.0, 40.0, 10.0])).unwrap();
    check_exact(
        &body,
        48000.0 - 9.0 * PI * 40.0,
        VolumeMethod::BrepBoundaryIntegralY,
        "hole along Y",
    );
}

/// Prism extruded along -Y from a trapezoid in XZ (x 0..60, z 0..20 at x = 60 rising
/// to 30 at x = 0): its top face is tilted, so the planar-face path of any orientation
/// is exercised.
fn wedge() -> Body {
    let trap = Profile::Polygon(vec![[0.0, 0.0], [60.0, 0.0], [60.0, 20.0], [0.0, 30.0]]);
    extrude(&trap, &Frame::XZ, 40.0).unwrap()
}
const WEDGE_V: f64 = 60.0 * 25.0 * 40.0;

#[test]
fn tilted_faces_plain_wedge_z() {
    check_exact(
        &wedge(),
        WEDGE_V,
        VolumeMethod::BrepBoundaryIntegralZ,
        "wedge",
    );
}

#[test]
fn tilted_face_with_vertical_hole_z() {
    // Ø6 along Z at x = 30, y = -20 through the tilted top z = 30 - x / 6: the removed
    // volume is the disc area times the top height at the disc centroid (linear in x).
    let top = Frame::XY.offset(30.0);
    let mut f = top;
    f.origin = [30.0, -20.0, 30.0];
    let body = cut_through_all(&wedge(), &hole(3.0), &f).unwrap();
    check_exact(
        &body,
        WEDGE_V - 9.0 * PI * 25.0,
        VolumeMethod::BrepBoundaryIntegralZ,
        "wedge, hole along Z",
    );
}

#[test]
fn tilted_face_with_side_hole_x() {
    let body = cut_through_all(&wedge(), &hole(3.0), &frame_x([60.0, -20.0, 10.0])).unwrap();
    check_exact(
        &body,
        WEDGE_V - 9.0 * PI * 60.0,
        VolumeMethod::BrepBoundaryIntegralX,
        "wedge, hole along X",
    );
}

#[test]
fn torus_is_exact_by_revolution() {
    let (big_r, r) = (40.0, 10.0);
    let prof = Profile::Circle {
        center: [big_r, 0.0],
        radius: r,
    };
    let body = revolve(&prof, &Frame::XZ, 2.0 * PI).unwrap();
    check_exact(
        &body,
        2.0 * PI * PI * big_r * r * r,
        VolumeMethod::BrepBoundaryIntegralRevolution,
        "torus",
    );
}

#[test]
fn tori_about_two_axes_are_exact_by_the_parametric_integral() {
    // Ø6 disc swept along an S-bend: line 10, then 90° arcs R 15 about two different
    // axes (parallel, distinct): no common revolution axis; the parametric
    // boundary integral (split at the sweep's knots) gives Pappus' value.
    let disc = Profile::Circle {
        center: [0.0, 0.0],
        radius: 3.0,
    };
    let path = [
        PathSegment::Line {
            end: [0.0, 0.0, 10.0],
        },
        PathSegment::Arc {
            center: [15.0, 0.0, 10.0],
            end: [15.0, 0.0, 25.0],
        },
        PathSegment::Arc {
            center: [15.0, 0.0, 40.0],
            end: [30.0, 0.0, 40.0],
        },
    ];
    let t = std::time::Instant::now();
    let body = sweep(&disc, &Frame::XY, &path).unwrap();
    let m = measure(&body).unwrap();
    let dt = t.elapsed();
    let expected = 9.0 * PI * (10.0 + 15.0 * PI);
    let (v, est, method) = value(&m);
    eprintln!(
        "S-bend: V = {v:.6}, expected {expected:.6}, err {:.3e} ({:.2e} rel), est {est:.3e}, {method:?}, {dt:?}",
        v - expected,
        (v - expected) / expected
    );
    assert_eq!(method, VolumeMethod::BrepBoundaryIntegralParametric);
    assert!(
        (v - expected).abs() <= 1e-9 * expected && est <= 1e-9 * expected,
        "S-bend: {v} vs {expected}, estimate {est}"
    );
}

#[test]
fn volume_method_deserializes_by_variant_name() {
    use serde::Deserialize;
    use serde::de::value::{Error, StrDeserializer};
    for (name, m) in [
        ("BrepBoundaryIntegralZ", VolumeMethod::BrepBoundaryIntegralZ),
        ("BrepBoundaryIntegralX", VolumeMethod::BrepBoundaryIntegralX),
        ("BrepBoundaryIntegralY", VolumeMethod::BrepBoundaryIntegralY),
        (
            "BrepBoundaryIntegralRevolution",
            VolumeMethod::BrepBoundaryIntegralRevolution,
        ),
        (
            "BrepBoundaryIntegralParametric",
            VolumeMethod::BrepBoundaryIntegralParametric,
        ),
        ("TessellationEstimate", VolumeMethod::TessellationEstimate),
    ] {
        // A JSON unit variant is the bare string, exactly what this deserializer feeds.
        let got = VolumeMethod::deserialize(StrDeserializer::<Error>::new(name)).unwrap();
        assert_eq!(got, m);
    }
}
