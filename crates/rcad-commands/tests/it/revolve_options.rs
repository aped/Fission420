//! Revolve axis and direction:
//! about the sketch Y axis (the default), the sketch X axis, a sketch line, a
//! profile on the negative side, partial angles in both directions; the axis
//! line follows sketch edits; a profile crossing the axis is refused;
//! save/reopen (format v6). Volumes by Pappus (area × 2π × centroid radius).
use crate::support;
use rcad_commands::{Command, Session, SketchTemplate};
use rcad_document::{RevolveAxis, SketchPlane};
use rcad_sketch::Sketch;
use serde_json::json;
use std::f64::consts::PI;
use support::tray;

fn rect_sketch(
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    axis_line: Option<([f64; 2], [f64; 2])>,
) -> Sketch {
    let mut s = Sketch::default();
    let p: Vec<_> = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
        .iter()
        .map(|q| s.add_point(q[0], q[1]))
        .collect();
    for i in 0..4 {
        s.add_line(p[i], p[(i + 1) % 4]);
    }
    if let Some((a, b)) = axis_line {
        let (pa, pb) = (s.add_point(a[0], a[1]), s.add_point(b[0], b[1]));
        let l = s.add_line(pa, pb);
        s.construction.insert(l);
    }
    s
}

fn revolve(
    s: &mut Session,
    sketch: Sketch,
    deg: f64,
    axis: RevolveAxis,
    reverse: bool,
) -> rcad_commands::Response {
    let r = s.execute(&Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::Custom { sketch },
    });
    assert!(r.ok, "{:?}", r.error);
    s.execute(&Command::Revolve {
        name: None,
        sketch: r.feature.unwrap(),
        angle_deg: deg,
        axis,
        reverse,
        op: Default::default(),
        target: None,
    })
}

fn fresh() -> Session {
    let mut s = Session::new();
    tray::ok(&mut s, &json!({"cmd": "new_document"}));
    s
}

/// Pappus: area × angle × centroid distance.
fn pappus(area: f64, r: f64, deg: f64) -> f64 {
    area * r * deg.to_radians()
}

#[test]
fn axes_sides_and_partial_angles() {
    // About the sketch X axis: rectangle x 0..30, y 5..15 → a tube along X.
    let mut s = fresh();
    let r = revolve(
        &mut s,
        rect_sketch(0.0, 5.0, 30.0, 15.0, None),
        360.0,
        RevolveAxis::SketchX,
        false,
    );
    assert!(r.ok, "{:?}", r.error);
    assert!((tray::volume(&s) - PI * (225.0 - 25.0) * 30.0).abs() < 1e-6);
    let b = s.summary().bodies[0].bounds.unwrap();
    assert!(
        (b.max[0] - 30.0).abs() < 1e-6
            && (b.max[1] - 15.0).abs() < 1e-6
            && (b.max[2] - 15.0).abs() < 1e-6
    );
    // About Y with the profile on the negative side.
    let mut s = fresh();
    let r = revolve(
        &mut s,
        rect_sketch(-20.0, 0.0, -10.0, 30.0, None),
        360.0,
        RevolveAxis::SketchY,
        false,
    );
    assert!(r.ok, "{:?}", r.error);
    assert!((tray::volume(&s) - PI * (400.0 - 100.0) * 30.0).abs() < 1e-6);
    // About a construction line x = -5 (vertical): radii 15..25 for x 10..20.
    let mut s = fresh();
    let sk = rect_sketch(10.0, 0.0, 20.0, 30.0, Some(([-5.0, 0.0], [-5.0, 1.0])));
    let axis_line = sk.entities.last().unwrap().id();
    let r = revolve(&mut s, sk, 360.0, RevolveAxis::Line(axis_line), false);
    assert!(r.ok, "{:?}", r.error);
    assert!((tray::volume(&s) - PI * (625.0 - 225.0) * 30.0).abs() < 1e-6);
    // Quarter turn about Y, both directions: the default leaves toward −Z,
    // reverse toward +Z.
    for (rev, z) in [(false, -1.0), (true, 1.0)] {
        let mut s = fresh();
        let r = revolve(
            &mut s,
            rect_sketch(10.0, 0.0, 20.0, 30.0, None),
            90.0,
            RevolveAxis::SketchY,
            rev,
        );
        assert!(r.ok, "{:?}", r.error);
        assert!((tray::volume(&s) - pappus(300.0, 15.0, 90.0)).abs() < 1e-6);
        // At 45° on the swept side: material; on the other side: none.
        let c = 15.0 * std::f64::consts::FRAC_1_SQRT_2;
        let p = tray::probe(&s, &[[c, 15.0, z * c], [c, 15.0, -z * c]]);
        assert_eq!(p, vec![Some(true), Some(false)], "reverse {rev}");
    }
}

#[test]
fn axis_line_follows_edits_and_refusals() {
    let mut s = fresh();
    let sk = rect_sketch(10.0, 0.0, 20.0, 30.0, Some(([-5.0, 0.0], [-5.0, 1.0])));
    let axis_line = sk.entities.last().unwrap().id();
    let r = revolve(&mut s, sk, 360.0, RevolveAxis::Line(axis_line), false);
    assert!(r.ok, "{:?}", r.error);
    // Save / reopen: v6.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("r.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(6));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": path}));
    assert!((tray::volume(&t) - PI * (625.0 - 225.0) * 30.0).abs() < 1e-6);
    // A profile crossing its axis is refused, nothing created.
    let mut u = fresh();
    let r = revolve(
        &mut u,
        rect_sketch(-5.0, 0.0, 5.0, 10.0, None),
        360.0,
        RevolveAxis::SketchY,
        false,
    );
    assert!(!r.ok);
    assert!(u.summary().bodies.is_empty());
    // An axis entity that is not a line of the sketch: refused.
    let mut w = fresh();
    let r = revolve(
        &mut w,
        rect_sketch(10.0, 0.0, 20.0, 30.0, None),
        360.0,
        RevolveAxis::Line(rcad_sketch::EntityId(99)),
        false,
    );
    assert!(!r.ok);
}

fn revolve_op(s: &mut Session, sketch: Sketch, op: &str) -> rcad_commands::Response {
    let r = s.execute(&Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXZ,
        template: SketchTemplate::Custom { sketch },
    });
    assert!(r.ok, "{:?}", r.error);
    let c: Command = serde_json::from_value(json!({"cmd": "revolve", "sketch": r.feature.unwrap(),
        "angle_deg": 360.0, "op": op, "target": 1}))
    .unwrap();
    s.execute(&c)
}

#[test]
fn revolve_cut_groove_is_exact() {
    // A coaxial ring groove (r 9..11, z 10..12) cut from a Ø20 x 30 cylinder. Truck 0.6 failed
    // here on every seam angle; the truck-shapeops fork fixes the periodic face division
    // (third_party/truck-shapeops/R-CAD-CHANGES.md). Exact volume: 3000π - 38π.
    let mut s = fresh();
    tray::ok(
        &mut s,
        &json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
                "diameter": 20.0}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 1, "distance": 30.0}),
    );
    let r = revolve_op(&mut s, rect_sketch(9.0, 10.0, 11.0, 12.0, None), "cut");
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(s.summary().bodies.len(), 1);
    assert!((tray::volume(&s) - (PI * 3000.0 - PI * 38.0)).abs() < 1e-6 * PI * 3000.0);
    // Undo restores the uncut cylinder; redo the groove.
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert!((tray::volume(&s) - PI * 3000.0).abs() < 1e-6 * PI * 3000.0);
    tray::ok(&mut s, &json!({"cmd": "redo"}));
    assert!((tray::volume(&s) - (PI * 3000.0 - PI * 38.0)).abs() < 1e-6 * PI * 3000.0);
}

/// A cup drawn the usual way, its section against the axis (outer R 25,
/// height 60, wall 2, floor 3), about the sketch Y axis and, mirrored, about
/// a sketch line on the −X side: exact volume π(25²·60 − 23²·57); an
/// upstream edit of the floor; save/reopen; STEP export; a section touching
/// the axis only at a point (cone tip) and a partial turn are refused.
#[test]
fn revolve_of_a_cup_section_touching_the_axis() {
    let cup = |floor: f64, sign: f64| {
        let mut s = Sketch::default();
        let p: Vec<_> = [
            [0.0, 0.0],
            [25.0, 0.0],
            [25.0, 60.0],
            [23.0, 60.0],
            [23.0, floor],
            [0.0, floor],
        ]
        .iter()
        .map(|q| s.add_point(sign * q[0], q[1]))
        .collect();
        for i in 0..6 {
            s.add_line(p[i], p[(i + 1) % 6]);
        }
        s
    };
    let want = |floor: f64| PI * (625.0 * 60.0 - 529.0 * (60.0 - floor));
    let mut s = fresh();
    let r = revolve(&mut s, cup(3.0, 1.0), 360.0, RevolveAxis::SketchY, false);
    assert!(r.ok, "{:?}", r.error);
    assert!((tray::volume(&s) - want(3.0)).abs() < 1e-6 * want(3.0));
    // About the sketch Y axis with the section on −X (mirrored).
    let mut m = fresh();
    let r = revolve(&mut m, cup(3.0, -1.0), 360.0, RevolveAxis::SketchY, false);
    assert!(r.ok, "{:?}", r.error);
    assert!((tray::volume(&m) - want(3.0)).abs() < 1e-6 * want(3.0));
    // Upstream edit: a 5 mm floor (replace the sketch through edit_sketch).
    let sk = s.summary().features[0].id;
    tray::ok(
        &mut s,
        &json!({"cmd": "edit_sketch", "feature": sk.0, "sketch": cup(5.0, 1.0)}),
    );
    assert!((tray::volume(&s) - want(5.0)).abs() < 1e-6 * want(5.0));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cup.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": path}));
    assert!((tray::volume(&t) - want(5.0)).abs() < 1e-6 * want(5.0));
    let step = dir.path().join("cup.step");
    tray::ok(&mut t, &json!({"cmd": "export_step", "path": step}));
    if let Some(ev) = std::env::var_os("RCAD_EVIDENCE_DIR") {
        let ev = std::path::PathBuf::from(ev);
        std::fs::create_dir_all(&ev).unwrap();
        std::fs::copy(&step, ev.join("revolved_cup_floor5.step")).unwrap();
    }
    // Cone tip and a partial turn: refused, nothing created.
    let mut c = fresh();
    let mut cone = Sketch::default();
    let p: Vec<_> = [[0.0, 0.0], [10.0, 0.0], [0.0, 20.0]]
        .iter()
        .map(|q| cone.add_point(q[0], q[1]))
        .collect();
    for i in 0..3 {
        cone.add_line(p[i], p[(i + 1) % 3]);
    }
    let r = revolve(&mut c, cone, 360.0, RevolveAxis::SketchY, false);
    assert!(!r.ok);
    assert!(
        format!("{:?}", r.error).contains("cone tip"),
        "{:?}",
        r.error
    );
    let mut h = fresh();
    let r = revolve(&mut h, cup(3.0, 1.0), 180.0, RevolveAxis::SketchY, false);
    assert!(!r.ok);
    assert!(h.summary().bodies.is_empty());
}
