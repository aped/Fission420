//! Blind circular pocket from an extrusion cap: analytic volume, topology, and
//! the blind floor location.
use rcad_kernel::*;
use std::f64::consts::PI;

fn rect(w: f64, h: f64) -> Profile {
    Profile::Polygon(vec![
        [-w / 2.0, -h / 2.0],
        [w / 2.0, -h / 2.0],
        [w / 2.0, h / 2.0],
        [-w / 2.0, h / 2.0],
    ])
}

fn hole(d: f64) -> Profile {
    Profile::Circle {
        center: [0.0, 0.0],
        radius: d / 2.0,
    }
}

fn volume(b: &Body) -> f64 {
    match measure(b).unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        VolumeResult::Unavailable { reason } => panic!("{reason}"),
    }
}

#[test]
fn pocket_from_end_cap_has_analytic_volume_and_blind_floor() {
    let (w, h, t, dia, depth) = (80.0, 50.0, 10.0, 10.0, 4.0);
    let plate = extrude(&rect(w, h), &Frame::XY, t).unwrap();
    let top = Frame::XY.offset(t);
    let b = cut_blind(&plate, &hole(dia), &top, depth).unwrap();
    let m = measure(&b).unwrap();
    assert_eq!(m.shell_count, 1);
    assert!(m.all_shells_closed);
    // Box minus a cylinder of radius 5 and height 4.
    let exp = w * h * t - PI * 25.0 * depth;
    let v = volume(&b);
    assert!((v - exp).abs() <= 0.001f64.max(exp * 1e-7), "{v} vs {exp}");
    // Bounds unchanged (the pocket is interior).
    assert!((m.bounds.max[2] - t).abs() < 1e-6 && m.bounds.min[2].abs() < 1e-6);
    // Exactly one inner loop, on the top face; none on the bottom (it is blind).
    let loops = z_face_inner_loops(&b).unwrap();
    assert_eq!(loops.len(), 1, "{loops:?}");
    assert!((loops[0].z - t).abs() < 1e-6);
    assert!(
        loops[0].centroid_xy[0]
            .abs()
            .max(loops[0].centroid_xy[1].abs())
            < 1e-6
    );
    assert!((loops[0].equivalent_diameter - dia).abs() < 1e-6);
    // A planar floor face at z = t - depth.
    assert_eq!(planar_faces_on(&b, &Frame::XY.offset(t - depth)), 1);
    // The result tessellates watertight.
    let mesh = tessellate(&b, 0.01).unwrap();
    assert!(check_mesh(&mesh).watertight);
}

#[test]
fn pocket_from_start_cap_goes_up_into_the_body() {
    let plate = extrude(&rect(60.0, 40.0), &Frame::XY, 8.0).unwrap();
    // Start cap: outward normal -Z (y flipped to stay right-handed).
    let bottom = Frame {
        origin: [0.0, 0.0, 0.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, -1.0, 0.0],
        z: [0.0, 0.0, -1.0],
    };
    let b = cut_blind(&plate, &hole(6.0), &bottom, 3.0).unwrap();
    let exp = 60.0 * 40.0 * 8.0 - PI * 9.0 * 3.0;
    assert!((volume(&b) - exp).abs() <= 0.001f64.max(exp * 1e-7));
    // Floor at world z = 3 facing down into the pocket (outward normal -Z;
    // offset() moves along the frame normal, here -Z).
    assert_eq!(planar_faces_on(&b, &bottom.offset(-3.0)), 1);
    assert_eq!(planar_faces_on(&b, &Frame::XY.offset(3.0)), 0);
}

#[test]
fn pocket_rejects_bad_depth() {
    let plate = extrude(&rect(20.0, 20.0), &Frame::XY, 5.0).unwrap();
    let top = Frame::XY.offset(5.0);
    for d in [0.0, -1.0, f64::NAN, 1e-4] {
        assert!(matches!(
            cut_blind(&plate, &hole(4.0), &top, d),
            Err(KernelError::InvalidInput(_))
        ));
    }
}
