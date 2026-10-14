//! Ellipse and spline sketches through the command API: New Body extrusion
//! (volume πab·h), an elliptical through cut and a boss join checked against
//! closed-form volumes, an upstream minor-radius edit, undo / redo,
//! save / reopen (format v8), a closed-spline prism whose volume is the
//! Green's-theorem area (independent quadrature) times the height, and
//! refusals that leave the document unchanged.
mod support;
use rcad_commands::{Response, Session};
use serde_json::{Value, json};
use std::f64::consts::PI;
use support::tray;

fn ok(s: &mut Session, v: Value) -> Response {
    tray::ok(s, &v)
}

fn vol(s: &Session, id: u32) -> f64 {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id.0 == id)
        .expect("body");
    match b.measurements.expect("measured").volume {
        rcad_kernel::VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

/// Volume tolerance: the boundary-integral volume is exact to ~1e-9
/// relative; 1e-6 relative leaves margin.
fn close(v: f64, want: f64) -> bool {
    (v - want).abs() <= 1e-6 * want.abs().max(1.0)
}

#[test]
fn ellipse_new_body_upstream_edit_undo_and_reopen() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    ok(
        &mut s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_ellipse",
               "major_radius": 20.0, "minor_radius": 10.0, "center": [5.0, -3.0]}),
    );
    ok(
        &mut s,
        json!({"cmd": "extrude", "sketch": 1, "distance": 15.0}),
    );
    let v0 = PI * 20.0 * 10.0 * 15.0;
    assert!(close(vol(&s, 1), v0), "{}", vol(&s, 1));
    let b = s.summary().bodies[0].measurements.clone().unwrap().bounds;
    for (got, want) in b.min.iter().zip([-15.0, -13.0, 0.0]) {
        assert!((got - want).abs() < 1e-6, "{b:?}");
    }
    // Upstream edit: the minor radius drives the volume.
    ok(
        &mut s,
        json!({"cmd": "set_parameter", "feature": 1, "name": "minor_radius", "value": 6.0}),
    );
    let v1 = PI * 20.0 * 6.0 * 15.0;
    assert!(close(vol(&s, 1), v1), "{}", vol(&s, 1));
    // A non-positive radius is refused; the document is unchanged.
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 1, "name": "minor_radius", "value": -1.0}),
    );
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().document(), &before);
    ok(&mut s, json!({"cmd": "undo"}));
    assert!(close(vol(&s, 1), v0));
    ok(&mut s, json!({"cmd": "redo"}));
    assert!(close(vol(&s, 1), v1));
    // Save / reopen: format v8 (sketch ellipses), same document and volume.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ellipse.rcad.json");
    ok(&mut s, json!({"cmd": "save", "path": path}));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(8));
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert!(close(vol(&t, 1), v1));
    // A file claiming an older version cannot hold sketch ellipses: refused
    // with the reason instead of being misread.
    let mut old = v.clone();
    old["version"] = json!(7);
    let old_path = dir.path().join("ellipse_v7.rcad.json");
    std::fs::write(&old_path, serde_json::to_string(&old).unwrap()).unwrap();
    let mut u = Session::new();
    let r = tray::exec(&mut u, &json!({"cmd": "open", "path": old_path}));
    assert!(!r.ok);
    assert!(
        format!("{:?}", r.error).contains("sketch ellipses and splines"),
        "{:?}",
        r.error
    );
}

#[test]
fn elliptical_cut_and_boss_on_a_block() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    ok(
        &mut s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 60.0, "height": 40.0}),
    );
    ok(
        &mut s,
        json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
    );
    ok(
        &mut s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_ellipse",
               "major_radius": 12.0, "minor_radius": 6.0, "center": [-10.0, 0.0]}),
    );
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 3, "op": "cut", "target": 1, "through_all": true}),
    );
    assert!(r.ok, "elliptical through cut: {:?}", r.error);
    let v_cut = 24_000.0 - PI * 12.0 * 6.0 * 10.0;
    assert!(close(vol(&s, 1), v_cut), "{}", vol(&s, 1));
    // Boss on the top face: an ellipse joined 5 mm up.
    ok(
        &mut s,
        json!({"cmd": "create_sketch",
               "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
               "template": "centered_ellipse", "major_radius": 8.0, "minor_radius": 5.0,
               "center": [15.0, 0.0]}),
    );
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 5, "distance": 5.0, "op": "join", "target": 1}),
    );
    assert!(r.ok, "elliptical boss: {:?}", r.error);
    let v_boss = v_cut + PI * 8.0 * 5.0 * 5.0;
    assert!(close(vol(&s, 1), v_boss), "{}", vol(&s, 1));
    // Membership: inside the boss, inside the cut, solid corner.
    let p = tray::probe(
        &s,
        &[[15.0, 0.0, 12.0], [-10.0, 0.0, 5.0], [28.0, 18.0, 5.0]],
    );
    assert_eq!(p, vec![Some(true), Some(false), Some(true)]);
}

/// Area of the closed uniform cubic B-spline with control points `c` by
/// Green's theorem, evaluated independently of the implementation: each
/// knot span is x(t), y(t) in the uniform cubic B-spline basis
/// (1/6)[(1−t)³, 3t³−6t²+4, −3t³+3t²+3t+1, t³]; x·y' − y·x' is a degree-5
/// polynomial per span, integrated exactly by 3-point Gauss–Legendre.
fn spline_area(c: &[[f64; 2]]) -> f64 {
    let n = c.len();
    let basis = |t: f64| {
        let s = 1.0 - t;
        [
            s * s * s / 6.0,
            (3.0 * t * t * t - 6.0 * t * t + 4.0) / 6.0,
            (-3.0 * t * t * t + 3.0 * t * t + 3.0 * t + 1.0) / 6.0,
            t * t * t / 6.0,
        ]
    };
    let dbasis = |t: f64| {
        let s = 1.0 - t;
        [
            -s * s / 2.0,
            (9.0 * t * t - 12.0 * t) / 6.0,
            (-9.0 * t * t + 6.0 * t + 3.0) / 6.0,
            t * t / 2.0,
        ]
    };
    let g = [
        (0.5 - 0.5 * 0.6f64.sqrt(), 5.0 / 18.0),
        (0.5, 8.0 / 18.0),
        (0.5 + 0.5 * 0.6f64.sqrt(), 5.0 / 18.0),
    ];
    let mut a = 0.0;
    for i in 0..n {
        let p = [c[i], c[(i + 1) % n], c[(i + 2) % n], c[(i + 3) % n]];
        for (t, w) in g {
            let (b, db) = (basis(t), dbasis(t));
            let (mut x, mut y, mut dx, mut dy) = (0.0, 0.0, 0.0, 0.0);
            for k in 0..4 {
                x += b[k] * p[k][0];
                y += b[k] * p[k][1];
                dx += db[k] * p[k][0];
                dy += db[k] * p[k][1];
            }
            a += w * 0.5 * (x * dy - y * dx);
        }
    }
    a.abs()
}

#[test]
fn closed_spline_prism_volume_and_refusals() {
    let ctrl = [
        [0.0, 0.0],
        [20.0, -4.0],
        [30.0, 10.0],
        [18.0, 25.0],
        [2.0, 20.0],
    ];
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    ok(
        &mut s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "spline",
               "points": ctrl, "closed": true}),
    );
    ok(
        &mut s,
        json!({"cmd": "extrude", "sketch": 1, "distance": 7.0}),
    );
    let want = spline_area(&ctrl) * 7.0;
    assert!(close(vol(&s, 1), want), "{} vs {want}", vol(&s, 1));
    // An open spline alone is not a closed profile: refused, nothing added.
    ok(
        &mut s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "spline",
               "points": [[50.0, 0.0], [55.0, 10.0], [60.0, 10.0], [65.0, 0.0]]}),
    );
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 3, "distance": 5.0}),
    );
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().document(), &before);
    // Too few control points: refused by name.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "spline",
               "points": [[0.0, 0.0], [1.0, 1.0], [2.0, 0.0]]}),
    );
    assert!(!r.ok);
    assert!(
        format!("{:?}", r.error).contains("control points"),
        "{:?}",
        r.error
    );
}

#[test]
fn elliptical_cut_patterns_and_mirrors_and_ellipse_revolve_refusal() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    ok(
        &mut s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 80.0, "height": 40.0}),
    );
    ok(
        &mut s,
        json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
    );
    ok(
        &mut s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_ellipse",
               "major_radius": 6.0, "minor_radius": 4.0, "center": [-25.0, 0.0]}),
    );
    ok(
        &mut s,
        json!({"cmd": "extrude", "sketch": 3, "op": "cut", "target": 1, "through_all": true}),
    );
    let hole = PI * 6.0 * 4.0 * 10.0;
    assert!(close(vol(&s, 1), 32_000.0 - hole));
    // Linear pattern of the elliptical cut (centres x = -25, -10, 5; the mirror
    // adds x = 25, clear of them): three holes in all, or refused
    // unchanged; never a wrong volume.
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "linear_pattern", "feature": 4, "count": 3, "spacing": 15.0,
                "direction": [1.0, 0.0]}),
    );
    if r.ok {
        assert!(close(vol(&s, 1), 32_000.0 - 3.0 * hole), "{}", vol(&s, 1));
    } else {
        assert_eq!(s.model().unwrap().document(), &before, "{:?}", r.error);
    }
    let before = s.model().unwrap().document().clone();
    let v_before = vol(&s, 1);
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "mirror", "feature": 4, "plane": "WorldYZ"}),
    );
    if r.ok {
        assert!(close(vol(&s, 1), v_before - hole), "{}", vol(&s, 1));
    } else {
        assert_eq!(s.model().unwrap().document(), &before, "{:?}", r.error);
    }
    // Revolve of an ellipse profile: refused by name (its revolved faces
    // cannot be tessellated reliably), the document unchanged.
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "new_document"}));
    ok(
        &mut t,
        json!({"cmd": "create_sketch", "plane": "WorldXZ", "template": "centered_ellipse",
               "major_radius": 10.0, "minor_radius": 5.0, "center": [60.0, 0.0]}),
    );
    let before = t.model().unwrap().document().clone();
    let r = tray::exec(
        &mut t,
        &json!({"cmd": "revolve", "sketch": 1, "angle_deg": 360.0}),
    );
    assert!(!r.ok);
    assert!(
        format!("{:?}", r.error).contains("revolving ellipse or spline"),
        "{:?}",
        r.error
    );
    assert_eq!(t.model().unwrap().document(), &before);
}
