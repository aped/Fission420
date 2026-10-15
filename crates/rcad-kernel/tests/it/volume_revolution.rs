//! Exact surface-of-revolution volume (`BrepBoundaryIntegralRevolution`): lofted
//! frusta, revolved cones and tori, swept circular channels, against analytic values
//! (frustum formula, Pappus) written independently here.
use rcad_kernel::*;
use std::f64::consts::PI;

fn value(b: &Body) -> (f64, f64, VolumeMethod) {
    let m = measure(b).unwrap();
    assert_eq!(m.shell_count, 1);
    assert!(m.all_shells_closed);
    match m.volume {
        VolumeResult::Value {
            volume,
            error_estimate,
            method,
        } => (volume, error_estimate, method),
        VolumeResult::Unavailable { ref reason } => panic!("volume unavailable: {reason}"),
    }
}

/// Exact within 1e-9 relative, reported error estimate within the same bound.
fn check(b: &Body, want: f64, method: VolumeMethod, label: &str) {
    let (v, est, got) = value(b);
    eprintln!(
        "{label}: V = {v:.12}, want {want:.12}, rel err {:.3e}, est {est:.3e}, {got:?}",
        (v - want) / want
    );
    assert_eq!(got, method, "{label}");
    assert!((v - want).abs() <= 1e-9 * want, "{label}: {v} vs {want}");
    assert!(est <= 1e-9 * want, "{label}: estimate {est}");
}

fn circle(cx: f64, cy: f64, r: f64) -> Profile {
    Profile::Circle {
        center: [cx, cy],
        radius: r,
    }
}

#[test]
fn loft_cone_frustum_is_exact() {
    // Ø40 -> Ø30 over 30 mm.
    let b = loft(
        &circle(0.0, 0.0, 20.0),
        &Frame::XY,
        &circle(0.0, 0.0, 15.0),
        &Frame::XY.offset(30.0),
    )
    .unwrap();
    let want = PI * 30.0 / 3.0 * (400.0 + 300.0 + 225.0);
    check(
        &b,
        want,
        VolumeMethod::BrepBoundaryIntegralRevolution,
        "frustum",
    );
    // Expanding frustum off the origin, along a tilted axis.
    let f = Frame {
        origin: [3.0, -4.0, 5.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.8, -0.6],
        z: [0.0, 0.6, 0.8],
    };
    let b = loft(
        &circle(2.0, 1.0, 5.0),
        &f,
        &circle(2.0, 1.0, 12.0),
        &f.offset(17.0),
    )
    .unwrap();
    let want = PI * 17.0 / 3.0 * (25.0 + 60.0 + 144.0);
    check(
        &b,
        want,
        VolumeMethod::BrepBoundaryIntegralRevolution,
        "tilted frustum",
    );
}

#[test]
fn equal_radius_loft_is_an_exact_cylinder() {
    let b = loft(
        &circle(0.0, 0.0, 10.0),
        &Frame::XY,
        &circle(0.0, 0.0, 10.0),
        &Frame::XY.offset(25.0),
    )
    .unwrap();
    let (v, _, method) = value(&b);
    let want = PI * 100.0 * 25.0;
    // Z-ruled: the original axis method still applies first.
    assert_eq!(method, VolumeMethod::BrepBoundaryIntegralZ);
    assert!((v - want).abs() <= 1e-9 * want, "{v} vs {want}");
}

/// Line 10 along +Z, 90° arc R 15 about the axis through (15, 0, 10) along +Y,
/// line 10 along +X (the ADR 0009 handle path).
fn handle_path() -> Vec<PathSegment> {
    vec![
        PathSegment::Line {
            end: [0.0, 0.0, 10.0],
        },
        PathSegment::Arc {
            center: [15.0, 0.0, 10.0],
            end: [15.0, 0.0, 25.0],
        },
        PathSegment::Line {
            end: [25.0, 0.0, 25.0],
        },
    ]
}

#[test]
fn swept_circular_channel_matches_pappus() {
    let b = sweep(&circle(0.0, 0.0, 3.0), &Frame::XY, &handle_path()).unwrap();
    // Area x centroid path length: 9 pi (10 + 15 pi / 2 + 10).
    let want = 9.0 * PI * (20.0 + 7.5 * PI);
    check(
        &b,
        want,
        VolumeMethod::BrepBoundaryIntegralRevolution,
        "channel",
    );
    // Off-centre disc, 1 mm away from the arc centre: its centroid path on the arc
    // has radius 16 (Pappus).
    let b = sweep(&circle(-1.0, 0.0, 2.0), &Frame::XY, &handle_path()).unwrap();
    let want = 4.0 * PI * (20.0 + 16.0 * PI / 2.0);
    check(
        &b,
        want,
        VolumeMethod::BrepBoundaryIntegralRevolution,
        "offset channel",
    );
}

#[test]
fn revolved_cone_and_torus_are_exact() {
    // Triangle (2,0)-(10,0)-(2,20) in XZ revolved about Z (revolve keeps profiles off
    // the axis): a cone face (apex beyond the face), a cylinder and an annulus.
    // Pappus: area 80, centroid x = 14/3.
    let tri = Profile::Polygon(vec![[2.0, 0.0], [10.0, 0.0], [2.0, 20.0]]);
    let b = revolve(&tri, &Frame::XZ, 2.0 * PI).unwrap();
    check(
        &b,
        2.0 * PI * 14.0 / 3.0 * 80.0,
        VolumeMethod::BrepBoundaryIntegralRevolution,
        "revolved cone",
    );
    // Trapezoid off the axis, partial turn: Pappus with the polygon centroid.
    let trap = Profile::Polygon(vec![[5.0, 0.0], [12.0, 0.0], [9.0, 6.0], [5.0, 6.0]]);
    let b = revolve(&trap, &Frame::XZ, 1.25 * PI).unwrap();
    // Area 33 (rect 4x6 + triangle 3x6/2), centroid x = (24 * 7 + 9 * 10) / 33.
    let want = 1.25 * PI * (24.0 * 7.0 + 9.0 * 10.0);
    check(
        &b,
        want,
        VolumeMethod::BrepBoundaryIntegralRevolution,
        "revolved trapezoid",
    );
    // Torus R 40, r 10 (was the tessellation-estimate example).
    let b = revolve(&circle(40.0, 0.0, 10.0), &Frame::XZ, 2.0 * PI).unwrap();
    check(
        &b,
        2.0 * PI * PI * 40.0 * 100.0,
        VolumeMethod::BrepBoundaryIntegralRevolution,
        "torus",
    );
}

#[test]
fn frustum_with_holes_is_exact() {
    let frustum = loft(
        &circle(0.0, 0.0, 20.0),
        &Frame::XY,
        &circle(0.0, 0.0, 15.0),
        &Frame::XY.offset(30.0),
    )
    .unwrap();
    let full = PI * 30.0 / 3.0 * (400.0 + 300.0 + 225.0);
    // Coaxial Ø6 bore and an off-axis Ø4 hole parallel to it (a cylinder about
    // another axis), both trimmed by the planar caps (boolean edges).
    let b = cut_through_all(&frustum, &circle(0.0, 0.0, 3.0), &Frame::XY).unwrap();
    let b = cut_through_all(&b, &circle(9.0, 0.0, 2.0), &Frame::XY).unwrap();
    let want = full - PI * 9.0 * 30.0 - PI * 4.0 * 30.0;
    check(
        &b,
        want,
        VolumeMethod::BrepBoundaryIntegralRevolution,
        "frustum with holes",
    );
}
