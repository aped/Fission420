//! Angled construction planes: `SketchPlane::Offset`
//! with `angle` tilts the base about its local X axis through its origin
//! before the offset. Bounds are analytic; the angle is a bindable parameter
//! (`plane_angle_deg`); out-of-range values are refused unchanged; the file is
//! v5 and reopens identically.
use crate::support;
use rcad_commands::Session;
use rcad_document::BodyId;
use serde_json::json;
use support::tray;

fn bounds(s: &Session) -> ([f64; 3], [f64; 3]) {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id == BodyId(1))
        .unwrap()
        .measurements
        .unwrap()
        .bounds;
    (b.min, b.max)
}

fn near(a: [f64; 3], b: [f64; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9)
}

/// Box of a 20 x 10 rectangle centred on the plane origin, extruded 5 along
/// the plane normal, plane = XY tilted `deg` about X then offset `d`.
fn expected(deg: f64, d: f64) -> ([f64; 3], [f64; 3]) {
    let (s, c) = deg.to_radians().sin_cos();
    // local y -> (0, c, s), normal -> (0, -s, c); corners y = ±5, h = d..d+5.
    let mut lo = [-10.0, f64::INFINITY, f64::INFINITY];
    let mut hi = [10.0, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for y in [-5.0, 5.0] {
        for h in [d, d + 5.0] {
            let p = [y * c - h * s, y * s + h * c];
            for k in 0..2 {
                lo[k + 1] = lo[k + 1].min(p[k]);
                hi[k + 1] = hi[k + 1].max(p[k]);
            }
        }
    }
    (lo, hi)
}

#[test]
fn tilted_plane_bounds_parameter_and_persistence() {
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch",
               "plane": {"Offset": {"base": "WorldXY", "distance": 0.0, "angle": 30f64.to_radians()}},
               "template": "centered_rectangle", "width": 20.0, "height": 10.0, "center": [0.0, 0.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 5.0}),
    ] {
        tray::ok(&mut s, &v);
    }
    assert!((tray::volume(&s) - 1000.0).abs() < 1e-6);
    let (lo, hi) = bounds(&s);
    let (elo, ehi) = expected(30.0, 0.0);
    assert!(
        near(lo, elo) && near(hi, ehi),
        "{lo:?} {hi:?} vs {elo:?} {ehi:?}"
    );
    // The angle and offset are parameters, bindable.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_user_parameter", "name": "tilt", "expression": "45",
                             "unit": "deg"}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "set_expression", "feature": 1, "name": "plane_angle_deg",
                             "expression": "tilt"}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 1, "name": "plane_offset",
                             "value": 3.0}),
    );
    let (lo, hi) = bounds(&s);
    let (elo, ehi) = expected(45.0, 3.0);
    assert!(
        near(lo, elo) && near(hi, ehi),
        "{lo:?} {hi:?} vs {elo:?} {ehi:?}"
    );
    // Out of range: refused unchanged.
    let before = s.model().unwrap().document().clone();
    for bad in [180.0, -200.0] {
        let r = tray::exec(
            &mut s,
            &json!({"cmd": "set_user_parameter", "name": "tilt",
                                           "expression": format!("{bad}"), "unit": "deg"}),
        );
        assert!(!r.ok, "{bad}");
        assert_eq!(s.model().unwrap().document(), &before);
    }
    // Save / reopen: v5, identical.
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("t.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": p}));
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(v["version"], json!(5));
    let mut s2 = Session::new();
    tray::ok(&mut s2, &json!({"cmd": "open", "path": p}));
    assert_eq!(
        s2.model().unwrap().document(),
        s.model().unwrap().document()
    );
    let (lo2, hi2) = bounds(&s2);
    assert!(near(lo2, lo) && near(hi2, hi));
    // Undo back to the untilted-offset state.
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    let (lo, hi) = bounds(&s);
    let (elo, ehi) = expected(45.0, 0.0);
    assert!(near(lo, elo) && near(hi, ehi));
}

#[test]
fn tilted_plane_on_a_face_has_the_rotated_frame() {
    // 40 x 30 x 20 block; its top face's frame tilted -90° about the face's X
    // axis (world X through (0, 0, 20)): y' = c·y + s·z = (0, 0, -1),
    // z' = -s·y + c·z = (0, 1, 0), origin unchanged.
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 40.0, "height": 30.0, "center": [20.0, 15.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 20.0}),
    ] {
        tray::ok(&mut s, &v);
    }
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "create_sketch",
                "plane": {"Offset": {"base": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
                                      "distance": 0.0, "angle": -90f64.to_radians()}},
                "template": "centered_rectangle", "width": 10.0, "height": 4.0,
                "center": [20.0, -10.0]}),
    );
    let f = s
        .summary()
        .features
        .into_iter()
        .find(|x| Some(x.id) == r.feature)
        .unwrap()
        .frame
        .unwrap();
    assert!(
        (f.z[1] - 1.0).abs() < 1e-12 && (f.y[2] + 1.0).abs() < 1e-12,
        "{f:?}"
    );
    assert!((f.origin[2] - 20.0).abs() < 1e-12);
}
