//! Join/Cut with revolved tools through the product path (`combine`: Truck boolean, then the
//! independent membership check), checked against closed-form volumes with the exact boundary
//! integral. Before the truck-shapeops fork fixes (third_party/truck-shapeops/R-CAD-CHANGES.md)
//! every case here failed in Truck: None, or a panic, which the adapter turns into a refusal.

use rcad_kernel::*;
use std::f64::consts::PI;

/// Revolve frame about world Z with the profile half-plane at angle `seam` from +X:
/// local x = radial, local y = world Z.
fn about_z(seam: f64) -> Frame {
    let (s, c) = seam.sin_cos();
    Frame {
        origin: [0.0, 0.0, 0.0],
        x: [c, s, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [s, -c, 0.0],
    }
}

/// Closed revolution of the (r, z) polygon about world Z.
fn ring(pts: &[(f64, f64)], seam: f64) -> Body {
    let p = Profile::Polygon(pts.iter().map(|&(r, z)| [r, z]).collect());
    revolve(&p, &about_z(seam), 2.0 * PI).unwrap()
}

fn rect_rz(r0: f64, r1: f64, z0: f64, z1: f64) -> Vec<(f64, f64)> {
    vec![(r0, z0), (r1, z0), (r1, z1), (r0, z1)]
}

fn cylinder(r: f64, h: f64) -> Body {
    let c = Profile::Circle {
        center: [0.0, 0.0],
        radius: r,
    };
    extrude(&c, &Frame::XY, h).unwrap()
}

/// Axis-aligned block centred on Z, `hx` x `hy` half sizes, z0..z1.
fn block(hx: f64, hy: f64, z0: f64, z1: f64) -> Body {
    let p = Profile::Polygon(vec![[-hx, -hy], [hx, -hy], [hx, hy], [-hx, hy]]);
    extrude(&p, &Frame::XY.offset(z0), z1 - z0).unwrap()
}

fn check(name: &str, target: &Body, tool: &Body, op: CombineOp, want: f64, shells: usize) {
    let (out, report) =
        combine(target, std::slice::from_ref(tool), op).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(report.passed(), "{name}: membership {report:?}");
    let m = measure(&out).unwrap();
    assert!(m.all_shells_closed, "{name}: open shell");
    assert_eq!(m.shell_count, shells, "{name}: shells");
    let VolumeResult::Value { volume: v, .. } = m.volume else {
        panic!("{name}: volume unavailable");
    };
    assert!(
        ((v - want) / want).abs() < 1e-6,
        "{name}: volume {v}, closed form {want}"
    );
}

#[test]
fn coaxial_groove_on_every_seam() {
    // Slow (~25 s per seam: exact volume and the membership check); four seams.
    for seam in [0.0, PI / 4.0, PI, 1.234] {
        check(
            &format!("groove seam {seam}"),
            &cylinder(10.0, 30.0),
            &ring(&rect_rz(9.0, 11.0, 10.0, 12.0), seam),
            CombineOp::Cut,
            3000.0 * PI - 38.0 * PI,
            1,
        );
    }
}

#[test]
fn coaxial_collar_join() {
    check(
        "collar",
        &cylinder(10.0, 30.0),
        &ring(&rect_rz(8.0, 14.0, 10.0, 14.0), 0.3),
        CombineOp::Join,
        3000.0 * PI + 384.0 * PI,
        1,
    );
}

/// Truck now builds the block plus a void shell (fork: PR #134); R-CAD's one-body rule then
/// refuses the two-shell result. Before the fork Truck panicked.
#[test]
fn ring_cut_inside_a_block_is_refused_by_the_one_body_rule() {
    let target = block(20.0, 20.0, 0.0, 30.0);
    let tool = ring(&rect_rz(8.0, 12.0, 10.0, 12.0), 0.3);
    let raw = boolean(&target, &tool, CombineOp::Cut);
    let e = raw
        .expect_err("one body must stay one connected solid")
        .to_string();
    assert!(e.contains("2 disconnected solids"), "{e}");
}

#[test]
fn countersink_cone_cut_into_a_block() {
    // Tool: triangle (1, 1), (1, -4.5), (6.5, 1) revolved; inside the block (z <= 0) it removes
    // r in [1, z + 5.5] for z in [-4.5, 0]: pi * ((5.5^3 - 1) / 3 - 4.5) = 50.625 pi.
    check(
        "countersink",
        &block(10.0, 10.0, -10.0, 0.0),
        &ring(&[(1.0, 1.0), (1.0, -4.5), (6.5, 1.0)], 0.3),
        CombineOp::Cut,
        4000.0 - 50.625 * PI,
        1,
    );
}

#[test]
fn ring_boss_joined_through_a_face() {
    check(
        "boss",
        &block(20.0, 20.0, 0.0, 30.0),
        &ring(&rect_rz(8.0, 12.0, 28.0, 34.0), 0.3),
        CombineOp::Join,
        48000.0 + 320.0 * PI,
        1,
    );
}

/// A tube cut through a block leaves a free central rod: two solids, refused by the one-body
/// rule, not by Truck.
#[test]
fn tube_cut_through_a_block_is_refused_by_the_one_body_rule() {
    let target = block(10.0, 10.0, -10.0, 0.0);
    let tool = ring(&rect_rz(1.0, 3.0, -15.0, 5.0), 0.3);
    let e = boolean(&target, &tool, CombineOp::Cut)
        .expect_err("two solids")
        .to_string();
    assert!(e.contains("2 disconnected solids"), "{e}");
}
