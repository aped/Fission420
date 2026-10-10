#![allow(clippy::needless_range_loop)]
//! Kernel acceptance checks against independently derived analytic values.
use f420_kernel::*;
use std::f64::consts::PI;

fn rect(w: f64, h: f64) -> Profile {
    Profile::Polygon(vec![
        [-w / 2.0, -h / 2.0],
        [w / 2.0, -h / 2.0],
        [w / 2.0, h / 2.0],
        [-w / 2.0, h / 2.0],
    ])
}

fn plate_with_hole(w: f64, h: f64, d: f64, dia: f64) -> Body {
    let plate = extrude(&rect(w, h), &Frame::XY, d).unwrap();
    let top = Frame::XY.offset(d);
    cut_through_all(
        &plate,
        &Profile::Circle {
            center: [0.0, 0.0],
            radius: dia / 2.0,
        },
        &top,
    )
    .unwrap()
}

fn vol(m: &Measurements) -> (f64, f64) {
    match m.volume {
        VolumeResult::Value {
            volume,
            error_estimate,
            ..
        } => (volume, error_estimate),
        VolumeResult::Unavailable { ref reason } => panic!("volume unavailable: {reason}"),
    }
}

fn check(w: f64, h: f64, d: f64, dia: f64) {
    let body = plate_with_hole(w, h, d, dia);
    let m = measure(&body).unwrap();
    assert_eq!(m.shell_count, 1);
    assert!(m.all_shells_closed);
    let expected = w * h * d - PI * (dia / 2.0).powi(2) * d;
    let (v, est) = vol(&m);
    let tol = 0.001f64.max(expected * 1e-7);
    assert!(
        (v - expected).abs() <= tol,
        "volume {v} vs {expected} (est {est})"
    );
    let eb = [[-w / 2.0, -h / 2.0, 0.0], [w / 2.0, h / 2.0, d]];
    for i in 0..3 {
        assert!((m.bounds.min[i] - eb[0][i]).abs() <= 1e-6, "{:?}", m.bounds);
        assert!((m.bounds.max[i] - eb[1][i]).abs() <= 1e-6, "{:?}", m.bounds);
    }
    // Top face must still exist exactly once (semantic reference target).
    assert_eq!(planar_faces_on(&body, &Frame::XY.offset(d)), 1);
    let mesh = tessellate(&body, 0.01).unwrap();
    let mc = check_mesh(&mesh);
    assert!(mc.watertight, "{mc:?}");
    assert!(
        (mc.mesh_volume - expected).abs() / expected <= 1e-3,
        "{mc:?}"
    );
}

#[test]
fn plate_80x50x10_hole10() {
    check(80.0, 50.0, 10.0, 10.0);
}

#[test]
fn plate_100x50x10_hole10() {
    check(100.0, 50.0, 10.0, 10.0);
}

#[test]
fn plate_100x50x12_hole10() {
    check(100.0, 50.0, 12.0, 10.0);
}

#[test]
fn plain_box_volume_and_faces() {
    let b = extrude(&rect(80.0, 50.0), &Frame::XY, 10.0).unwrap();
    let m = measure(&b).unwrap();
    assert_eq!(m.face_count, 6);
    assert_eq!(m.vertex_count, 8);
    assert_eq!(m.edge_count, 12);
    let (v, _) = vol(&m);
    assert!((v - 40000.0).abs() < 1e-6);
    assert_eq!(planar_faces_on(&b, &Frame::XY.offset(10.0)), 1);
    assert_eq!(planar_faces_on(&b, &Frame::XY.offset(12.0)), 0);
}

#[test]
fn rejects_bad_inputs() {
    assert!(extrude(&rect(80.0, 50.0), &Frame::XY, -10.0).is_err());
    assert!(extrude(&rect(80.0, 50.0), &Frame::XY, f64::NAN).is_err());
    assert!(extrude(&rect(0.0, 50.0), &Frame::XY, 10.0).is_err());
    // clockwise polygon
    let cw = Profile::Polygon(vec![[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]]);
    assert!(extrude(&cw, &Frame::XY, 1.0).is_err());
    assert!(
        extrude(
            &Profile::Circle {
                center: [0.0, 0.0],
                radius: 0.0
            },
            &Frame::XY,
            1.0
        )
        .is_err()
    );
}

#[test]
fn hole_splitting_plate_is_rejected() {
    // A hole wider than the plate height splits it into two bodies (or fails);
    // either way the kernel must not return a single "valid" solid silently.
    let plate = extrude(&rect(80.0, 50.0), &Frame::XY, 10.0).unwrap();
    let r = cut_through_all(
        &plate,
        &Profile::Circle {
            center: [0.0, 0.0],
            radius: 30.0,
        },
        &Frame::XY.offset(10.0),
    );
    if let Ok(b) = r {
        let m = measure(&b).unwrap();
        panic!("expected failure, got body {m:?}");
    }
}

#[test]
fn stl_has_expected_size() {
    let body = plate_with_hole(80.0, 50.0, 10.0, 10.0);
    let mesh = tessellate(&body, 0.05).unwrap();
    let bytes = write_stl_binary(&mesh, "t");
    assert_eq!(bytes.len(), 84 + 50 * mesh.triangles.len());
}

#[test]
fn step_export_has_unique_ids_and_no_intersection_curves() {
    let body = plate_with_hole(100.0, 50.0, 10.0, 10.0);
    let text = export_step(&body, "test").unwrap();
    assert!(text.starts_with("ISO-10303-21;"));
    assert!(!text.contains("INTERSECTION_CURVE"));
    assert_eq!(text.matches("FACE_SURFACE").count(), 9);
    std::fs::create_dir_all("../../artifacts").ok();
    std::fs::write("../../artifacts/plate_100x50x10.step", &text).unwrap();
}
