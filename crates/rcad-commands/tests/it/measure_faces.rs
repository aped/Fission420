//! Exact face measurements: cylinder radius/diameter, distance
//! between parallel axes, plane-to-axis distance, plane distance/angle — from the
//! B-rep surfaces, against the modelled dimensions; they follow regeneration.
use crate::support;
use rcad_commands::measure_faces::FaceMeasureReport;
use rcad_commands::{Response, Session};
use rcad_kernel::{DirectFace, FacePairMeasure};
use serde_json::{Value, json};
use support::tray;

fn ok(s: &mut Session, v: Value) -> Response {
    tray::ok(s, &v)
}

fn measure(s: &mut Session, a: u32, b: Option<u32>) -> FaceMeasureReport {
    let v = match b {
        Some(b) => json!({"cmd": "measure_faces", "a": {"body": 1, "face": a},
                          "b": {"body": 1, "face": b}}),
        None => json!({"cmd": "measure_faces", "a": {"body": 1, "face": a}}),
    };
    ok(s, v).face_measure.expect("report")
}

/// Every face of body 1 with its exact geometry.
fn faces(s: &mut Session) -> Vec<(u32, DirectFace)> {
    let n = s.summary().bodies[0]
        .measurements
        .as_ref()
        .unwrap()
        .face_count as u32;
    (0..n)
        .filter_map(|i| {
            let r = tray::exec(
                s,
                &json!({"cmd": "measure_faces", "a": {"body": 1, "face": i}}),
            );
            r.face_measure.map(|m| (i, m.a))
        })
        .collect()
}

fn cyl(fs: &[(u32, DirectFace)], r: f64) -> u32 {
    fs.iter()
        .find(
            |(_, f)| matches!(f, DirectFace::Cylinder { radius, .. } if (radius - r).abs() < 1e-9),
        )
        .map(|(i, _)| *i)
        .expect("cylinder")
}

fn plane(fs: &[(u32, DirectFace)], n: [f64; 3], off: f64) -> u32 {
    fs.iter()
        .find(|(_, f)| match f {
            DirectFace::Plane { origin, normal } => {
                (0..3).all(|k| (normal[k] - n[k]).abs() < 1e-9)
                    && ((0..3).map(|k| origin[k] * n[k]).sum::<f64>() - off).abs() < 1e-9
            }
            _ => false,
        })
        .map(|(i, _)| *i)
        .expect("plane")
}

fn dist(p: &FacePairMeasure) -> f64 {
    match p {
        FacePairMeasure::PlaneDistance { distance }
        | FacePairMeasure::AxisDistance { distance }
        | FacePairMeasure::PlaneAxisDistance { distance } => *distance,
        other => panic!("{other:?}"),
    }
}

/// 60 × 40 × 20 block at the origin corner, Ø10 hole at (15, 10), Ø6 at (45, 30).
fn plate(s: &mut Session) {
    ok(s, json!({"cmd": "new_document"}));
    ok(
        s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 60.0, "height": 40.0, "center": [30.0, 20.0]}),
    );
    ok(s, json!({"cmd": "extrude", "sketch": 1, "distance": 20.0}));
    for (d, c, f) in [(10.0, [15.0, 10.0], 3), (6.0, [45.0, 30.0], 5)] {
        ok(
            s,
            json!({"cmd": "create_sketch", "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
                   "template": "centered_circle", "diameter": d, "center": c}),
        );
        ok(s, json!({"cmd": "cut_through_all", "sketch": f}));
    }
}

#[test]
fn radius_axis_and_plane_measurements_are_exact() {
    let mut s = Session::new();
    plate(&mut s);
    let fs = faces(&mut s);
    let (h1, h2) = (cyl(&fs, 5.0), cyl(&fs, 3.0));
    let r = measure(&mut s, h1, None);
    assert!((r.radius.unwrap() - 5.0).abs() < 1e-9 && (r.diameter.unwrap() - 10.0).abs() < 1e-9);
    assert!(matches!(r.a, DirectFace::Cylinder { hole: true, .. }));
    // Parallel axes: hypot(30, 20).
    let d = dist(&measure(&mut s, h1, Some(h2)).pair.unwrap());
    assert!((d - 30f64.hypot(20.0)).abs() < 1e-9, "{d}");
    let (top, bottom) = (
        plane(&fs, [0.0, 0.0, 1.0], 20.0),
        plane(&fs, [0.0, 0.0, -1.0], 0.0),
    );
    let side = plane(&fs, [-1.0, 0.0, 0.0], 0.0);
    assert!((dist(&measure(&mut s, top, Some(bottom)).pair.unwrap()) - 20.0).abs() < 1e-9);
    // Side plane x = 0 to the Ø10 axis at x = 15.
    assert!((dist(&measure(&mut s, side, Some(h1)).pair.unwrap()) - 15.0).abs() < 1e-9);
    match measure(&mut s, top, Some(h1)).pair.unwrap() {
        FacePairMeasure::PlaneAxisAngle { degrees } => assert!((degrees - 90.0).abs() < 1e-9),
        p => panic!("{p:?}"),
    }
    match measure(&mut s, top, Some(side)).pair.unwrap() {
        FacePairMeasure::PlaneAngle { degrees } => assert!((degrees - 90.0).abs() < 1e-9),
        p => panic!("{p:?}"),
    }
    // Readouts follow regeneration: a Ø12 first hole.
    ok(
        &mut s,
        json!({"cmd": "set_parameter", "feature": 3, "name": "diameter", "value": 12.0}),
    );
    let fs = faces(&mut s);
    let h1 = cyl(&fs, 6.0);
    assert!((measure(&mut s, h1, None).diameter.unwrap() - 12.0).abs() < 1e-9);
    // Unknown face / body are refused.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "measure_faces", "a": {"body": 1, "face": 999}}),
    );
    assert!(!r.ok);
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "measure_faces", "a": {"body": 9, "face": 0}}),
    );
    assert!(!r.ok);
}

#[test]
fn rotated_body_axis_distance_is_invariant() {
    let mut s = Session::new();
    plate(&mut s);
    ok(
        &mut s,
        json!({"cmd": "move_body", "body": 1, "angle_deg": 33.0, "axis": [1.0, 1.0, 0.0],
               "translate": [5.0, -7.0, 2.0]}),
    );
    let fs = faces(&mut s);
    let (h1, h2) = (cyl(&fs, 5.0), cyl(&fs, 3.0));
    let d = dist(&measure(&mut s, h1, Some(h2)).pair.unwrap());
    assert!((d - 30f64.hypot(20.0)).abs() < 1e-9, "{d}");
}

/// Exact planar face area (holes subtracted), following an edit.
#[test]
fn planar_face_area_is_exact_and_follows_edits() {
    let mut s = Session::new();
    plate(&mut s);
    let fs = faces(&mut s);
    let top = plane(&fs, [0.0, 0.0, 1.0], 20.0);
    let front = plane(&fs, [0.0, -1.0, 0.0], 0.0);
    let pi = std::f64::consts::PI;
    let a = measure(&mut s, top, None).area.unwrap();
    assert!((a - (2400.0 - pi * (25.0 + 9.0))).abs() < 1e-9, "{a}");
    assert!((measure(&mut s, front, None).area.unwrap() - 1200.0).abs() < 1e-9);
    // Cylinders report no area (planar faces only).
    let hole = cyl(&fs, 5.0);
    assert!(measure(&mut s, hole, None).area.is_none());
    // Hole Ø10 -> Ø12.
    ok(
        &mut s,
        json!({"cmd": "set_parameter", "feature": 3, "name": "diameter", "value": 12.0}),
    );
    let fs = faces(&mut s);
    let top = plane(&fs, [0.0, 0.0, 1.0], 20.0);
    let a = measure(&mut s, top, None).area.unwrap();
    assert!((a - (2400.0 - pi * (36.0 + 9.0))).abs() < 1e-9, "{a}");
}
