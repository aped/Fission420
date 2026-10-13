//! Run-6 acceptance oracles for edge finishing, lofts and sweeps
//! (SESSION_6 acceptance projects 2–4). Expected values are derived from the
//! dimensions alone, never from the kernel's own output:
//!
//! * fillet: a 40×30×10 prism with its four vertical edges rounded r has
//!   V = (40·30 − (4 − π)·r²)·10;
//! * loft between centred aligned rectangles w0×h0 and (w0+dw)×(h0+dh) at
//!   separation H: V = H·(w0·h0 + (w0·dh + h0·dw)/2 + dw·dh/3), and the section
//!   at height z is the linear interpolation;
//! * sweep of a radius-3 circle along a quarter circle of radius 25:
//!   V = π·3²·(π·25/2) (Pappus; the centroid runs on the path).
mod support;
use rcad_commands::{Command, Session, SketchTemplate};
use rcad_document::SketchPlane;
use rcad_sketch::{Entity, Sketch};
use serde_json::json;
use std::f64::consts::PI;
use support::tray;

fn new_box(s: &mut Session, w: f64, h: f64, d: f64) {
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": w, "height": h, "center": [w / 2.0, h / 2.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": d}),
    ] {
        tray::ok(s, &v);
    }
}

fn side(e: u32) -> serde_json::Value {
    json!({"feature": 2, "role": {"ExtrudeSide": {"entity": e}}})
}

fn fillet(a: serde_json::Value, b: serde_json::Value, r: f64) -> serde_json::Value {
    json!({"cmd": "fillet", "a": a, "b": b, "radius": r})
}

fn rounded_box_volume(r: f64) -> f64 {
    (40.0 * 30.0 - (4.0 - PI) * r * r) * 10.0
}

/// The four vertical edges of the rectangle's extrusion (consecutive sides).
const VERTICAL_EDGES: [(u32, u32); 4] = [(1, 2), (2, 3), (3, 4), (4, 1)];

fn round_vertical_edges(s: &mut Session, r: f64) {
    for (a, b) in VERTICAL_EDGES {
        tray::ok(s, &fillet(side(a), side(b), r));
    }
}

#[test]
fn fillet_reference_prism_four_vertical_edges() {
    let mut s = Session::new();
    new_box(&mut s, 40.0, 30.0, 10.0);
    round_vertical_edges(&mut s, 3.0);
    let v = tray::volume(&s);
    assert!(
        (v - rounded_box_volume(3.0)).abs() < 1e-6,
        "{v} vs {}",
        rounded_box_volume(3.0)
    );
    // Exact arc boundaries: on the arc's bisector, just inside / outside the
    // radius-3 arc centred (3, 3); the sharp corner is air.
    let d = 3.0 - 3.0 * std::f64::consts::FRAC_1_SQRT_2;
    let p = tray::probe(
        &s,
        &[
            [d + 0.01, d + 0.01, 5.0],
            [d - 0.01, d - 0.01, 5.0],
            [0.1, 0.1, 5.0],
            [40.0 - d - 0.01, 30.0 - d - 0.01, 5.0],
            [40.0 - d + 0.01, 30.0 - d + 0.01, 5.0],
        ],
    );
    assert_eq!(
        p,
        vec![Some(true), Some(false), Some(false), Some(true), Some(false)]
    );
    // Topology: one solid, 4 planar sides + 4 cylinders + 2 caps.
    let summ = s.summary();
    assert_eq!(summ.bodies.len(), 1);
    // Upstream edit of the radius of every fillet sweeps the volume formula.
    for r in [1.0, 5.0, 7.5, 14.0] {
        for f in 3..=6 {
            tray::ok(
                &mut s,
                &json!({"cmd": "set_parameter", "feature": f, "name": "radius", "value": r}),
            );
        }
        let v = tray::volume(&s);
        assert!(
            (v - rounded_box_volume(r)).abs() < 1e-6,
            "r={r}: {v} vs {}",
            rounded_box_volume(r)
        );
    }
    // Upstream edit of the box: the fillets follow.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "distance", "value": 20.0}),
    );
    assert!((tray::volume(&s) - 2.0 * rounded_box_volume(14.0)).abs() < 1e-6);
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert!((tray::volume(&s) - rounded_box_volume(14.0)).abs() < 1e-6);
}

#[test]
fn fillet_radius_sweep_invalid_cases_are_refused_unchanged() {
    let mut s = Session::new();
    new_box(&mut s, 40.0, 30.0, 10.0);
    round_vertical_edges(&mut s, 3.0);
    let before = s.model().unwrap().document().clone();
    // A radius that cannot fit beside its neighbours (28 + 3 > 30 on the short
    // side), one that eats a whole face, and non-positive radii are refused.
    for (f, r) in [(3, 31.0), (3, 0.0), (3, -1.0), (4, 28.0)] {
        let resp = tray::exec(
            &mut s,
            &json!({"cmd": "set_parameter", "feature": f, "name": "radius", "value": r}),
        );
        assert!(!resp.ok, "fillet {f} radius {r} accepted");
        assert_eq!(s.model().unwrap().document(), &before);
    }
    // Radius 16 on every edge: 32 > 30 on the short sides.
    for f in 3..=6 {
        let r = tray::exec(
            &mut s,
            &json!({"cmd": "set_parameter", "feature": f, "name": "radius", "value": 16.0}),
        );
        if !r.ok {
            assert!(f > 3, "the first radius-16 fillet alone fits");
            assert!(r.error.is_some());
            return;
        }
    }
    panic!("four radius-16 fillets on a 40x30 prism were accepted");
}

#[test]
fn concave_fillet_of_an_inside_corner() {
    // L-shaped bracket: 40 long (X), 30 tall leg, 6 thick; extruded 20.
    // The inside corner at (6, 6) is concave; a radius-4 fillet adds
    // (r² − πr²/4)·20 of material.
    let mut s = Session::new();
    tray::ok(&mut s, &json!({"cmd": "new_document"}));
    tray::ok(
        &mut s,
        &json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "polygon", "vertices":
            [[0.0, 0.0], [40.0, 0.0], [40.0, 6.0], [6.0, 6.0], [6.0, 30.0], [0.0, 30.0]]}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 1, "distance": 20.0}),
    );
    let base = (40.0 * 6.0 + 6.0 * 24.0) * 20.0;
    assert!((tray::volume(&s) - base).abs() < 1e-6);
    // Polygon entities are numbered along the vertex list: 3 = (40,6)-(6,6),
    // 4 = (6,6)-(6,30).
    let r = tray::exec(&mut s, &fillet(side(3), side(4), 4.0));
    assert!(r.ok, "concave fillet: {:?}", r.error);
    let want = base + (16.0 - PI * 4.0) * 20.0;
    assert!((tray::volume(&s) - want).abs() < 1e-6, "{}", tray::volume(&s));
    let d = 4.0 - 4.0 * std::f64::consts::FRAC_1_SQRT_2;
    let p = tray::probe(
        &s,
        &[
            [6.0 + d - 0.01, 6.0 + d - 0.01, 10.0],
            [6.0 + d + 0.01, 6.0 + d + 0.01, 10.0],
        ],
    );
    assert_eq!(p, vec![Some(true), Some(false)]);
    // Too large for the 24 mm leg.
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "radius", "value": 25.0}),
    );
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().document(), &before);
}

#[test]
fn fillet_meeting_rounded_corners_is_refused_by_name() {
    // Corner case: after the vertical edges are rounded, the top-front edge
    // runs into the cylinders; blending it needs a corner patch (not
    // supported). It must be refused with a reason, document unchanged.
    let mut s = Session::new();
    new_box(&mut s, 40.0, 30.0, 10.0);
    round_vertical_edges(&mut s, 3.0);
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &fillet(json!({"feature": 2, "role": "ExtrudeEndCap"}), side(1), 2.0),
    );
    assert!(!r.ok, "corner blend unexpectedly accepted");
    assert!(r.error.is_some());
    assert_eq!(s.model().unwrap().document(), &before);
}

fn rect_on(z: f64, w: f64, h: f64) -> serde_json::Value {
    let plane = if z == 0.0 {
        json!("WorldXY")
    } else {
        json!({"Offset": {"base": "WorldXY", "distance": z}})
    };
    json!({"cmd": "create_sketch", "plane": plane, "template": "centered_rectangle",
           "width": w, "height": h, "center": [0.0, 0.0]})
}

fn taper_volume(h: f64, (w0, h0): (f64, f64), (w1, h1): (f64, f64)) -> f64 {
    let (dw, dh) = (w1 - w0, h1 - h0);
    h * (w0 * h0 + (w0 * dh + h0 * dw) / 2.0 + dw * dh / 3.0)
}

#[test]
fn loft_reference_sections_and_volume() {
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        rect_on(0.0, 40.0, 30.0),
        rect_on(20.0, 60.0, 50.0),
        json!({"cmd": "loft", "a": 1, "b": 2, "body_name": "Taper"}),
    ] {
        tray::ok(&mut s, &v);
    }
    let want = taper_volume(20.0, (40.0, 30.0), (60.0, 50.0));
    assert!((tray::volume(&s) - want).abs() < 1e-6, "{}", tray::volume(&s));
    // Sections at z = 5, 10, 15: half-sizes (20 + z/2, 15 + z/2). Probe just
    // inside / outside each side: no twist, linear interpolation.
    let mut pts = vec![];
    let mut expect = vec![];
    for z in [5.0, 10.0, 15.0] {
        let (hx, hy) = (20.0 + z / 2.0, 15.0 + z / 2.0);
        for (p, inside) in [
            ([hx - 0.01, 0.0, z], true),
            ([hx + 0.01, 0.0, z], false),
            ([0.0, hy - 0.01, z], true),
            ([0.0, hy + 0.01, z], false),
            ([-hx + 0.01, -hy + 0.01, z], true),
            ([-hx - 0.01, -hy - 0.01, z], false),
        ] {
            pts.push(p);
            expect.push(Some(inside));
        }
    }
    assert_eq!(tray::probe(&s, &pts), expect);
    let b = s.summary().bodies[0].bounds.unwrap();
    for (got, want) in b.min.iter().chain(b.max.iter()).zip([-30.0, -25.0, 0.0, 30.0, 25.0, 20.0]) {
        assert!((got - want).abs() < 1e-6, "bounds {b:?}");
    }
}

/// Quarter circle of radius `r` on World XZ from the origin, leaving along +Z
/// (the World XY profile's normal) and ending at (−r, 0, r) heading −X.
fn quarter(r: f64) -> Sketch {
    let mut s = Sketch::default();
    let p0 = s.add_point(0.0, 0.0);
    let c = s.add_point(-r, 0.0);
    let p1 = s.add_point(-r, r);
    let id = s.next_entity_id();
    s.entities.push(Entity::Arc {
        id,
        center: c,
        start: p0,
        end: p1,
    });
    s
}

fn custom(plane: SketchPlane, sketch: Sketch) -> Command {
    Command::CreateSketch {
        name: None,
        plane,
        template: SketchTemplate::Custom { sketch },
    }
}

#[test]
fn sweep_reference_circle_along_quarter_circle() {
    let mut s = Session::new();
    tray::ok(&mut s, &json!({"cmd": "new_document"}));
    tray::ok(
        &mut s,
        &json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
                "diameter": 6.0}),
    );
    let r = s.execute(&custom(SketchPlane::WorldXZ, quarter(25.0)));
    assert!(r.ok, "{:?}", r.error);
    tray::ok(
        &mut s,
        &json!({"cmd": "sweep", "profile": 1, "path": 2, "body_name": "Elbow"}),
    );
    let want = PI * 9.0 * (PI * 25.0 / 2.0);
    let v = tray::volume(&s);
    assert!((v - want).abs() < 1e-6, "{v} vs {want}");
    let b = s.summary().bodies[0].bounds.unwrap();
    for (got, want) in b.min.iter().chain(b.max.iter()).zip([-25.0, -3.0, 0.0, 3.0, 3.0, 28.0]) {
        assert!((got - want).abs() < 1e-6, "bounds {b:?}");
    }
    // Sections: at 45° along the bend the tube is centred on
    // (−25 + 25cos45°, 0, 25 sin45°); the end cap lies in the plane x = −25.
    let (c, sn) = (25.0 * (1.0 - std::f64::consts::FRAC_1_SQRT_2), 25.0 * std::f64::consts::FRAC_1_SQRT_2);
    let p = tray::probe(
        &s,
        &[
            [-c, 2.99, sn],
            [-c, 3.01, sn],
            [-25.01, 0.0, 25.0],
            [-24.9, 0.0, 27.9],
            [-24.9, 0.0, 28.1],
            [0.0, 0.0, -0.01],
        ],
    );
    assert_eq!(
        p,
        vec![Some(true), Some(false), Some(false), Some(true), Some(false), Some(false)]
    );
}

#[test]
fn sweep_noncircular_profile_keeps_its_orientation() {
    // Rectangle 8 (X) × 4 (Y) on World XY swept along the quarter circle: the
    // X extent turns into Z at the end, the Y extent stays Y (no twist).
    let mut s = Session::new();
    tray::ok(&mut s, &json!({"cmd": "new_document"}));
    tray::ok(
        &mut s,
        &json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                "width": 8.0, "height": 4.0}),
    );
    let r = s.execute(&custom(SketchPlane::WorldXZ, quarter(25.0)));
    assert!(r.ok, "{:?}", r.error);
    tray::ok(
        &mut s,
        &json!({"cmd": "sweep", "profile": 1, "path": 2, "body_name": "Bar"}),
    );
    let want = 32.0 * (PI * 25.0 / 2.0);
    assert!((tray::volume(&s) - want).abs() < 1e-6, "{}", tray::volume(&s));
    let p = tray::probe(
        &s,
        &[
            [-24.9, 1.9, 25.0 + 3.9],
            [-24.9, 2.1, 25.0],
            [-24.9, 0.0, 25.0 + 4.1],
            [-24.9, 0.0, 25.0 - 3.9],
        ],
    );
    assert_eq!(p, vec![Some(true), Some(false), Some(false), Some(true)]);
}
