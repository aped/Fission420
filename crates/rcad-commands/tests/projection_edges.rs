//! Associative projection of single body edges and body sections
//! ("intersect") into sketches: a parallel edge keeps its line or circle, a
//! circle on a face oblique to the sketch becomes the exact ellipse, a
//! section yields the body's outline and holes in the sketch plane.
//! Upstream edits move the geometry, a vanished edge breaks the link by name,
//! unlink freezes, refusals name the reason, undo and save/reopen (format
//! v8). Expected geometry and volumes are closed-form.
mod support;
use rcad_commands::Session;
use rcad_sketch::Entity;
use serde_json::{Value, json};
use std::f64::consts::PI;
use support::tray;

fn run(s: &mut Session, steps: &[Value]) {
    for v in steps {
        tray::ok(s, v);
    }
}

fn body_vol(s: &Session, id: u32) -> f64 {
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

/// F1 rect 40 x 30 centred, F2 extrude 10, F3 circle Ø8 at (10, 0), F4 cut
/// through all; F5 a sketch on `plane` holding a small triangle far away.
fn holed_block(s: &mut Session, plane: Value) {
    run(
        s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 40.0, "height": 30.0}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
                   "diameter": 8.0, "center": [10.0, 0.0]}),
            json!({"cmd": "extrude", "sketch": 3, "op": "cut", "target": 1, "through_all": true}),
            json!({"cmd": "create_sketch", "plane": plane,
                   "template": "polygon", "vertices": [[80.0, 80.0], [81.0, 80.0], [80.0, 81.0]]}),
        ],
    );
}

fn top_edge(at: [f64; 3]) -> Value {
    json!({"cmd": "project", "sketch": 5,
           "source": {"Edge": {"feature": 2, "role": "ExtrudeEndCap"}}, "at": at})
}

fn solved(s: &Session, f: usize) -> rcad_sketch::Sketch {
    s.summary().features[f].sketch.clone().unwrap()
}

fn circles(sk: &rcad_sketch::Sketch) -> Vec<([f64; 2], f64)> {
    sk.entities
        .iter()
        .filter_map(|e| match e {
            Entity::Circle { center, radius, .. } => Some((sk.point_xy(*center)?, *radius)),
            _ => None,
        })
        .collect()
}

fn lines(sk: &rcad_sketch::Sketch) -> Vec<([f64; 2], [f64; 2])> {
    sk.entities
        .iter()
        .filter_map(|e| match e {
            Entity::Line { a, b, .. } => Some((sk.point_xy(*a)?, sk.point_xy(*b)?)),
            _ => None,
        })
        .collect()
}

fn near(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-7
}

#[test]
fn parallel_edges_follow_upstream_edits_undo_and_reopen() {
    let mut s = Session::new();
    holed_block(
        &mut s,
        json!({"Offset": {"base": "WorldXY", "distance": 20.0}}),
    );
    // The hole's rim (click just outside it on the top face) and the right edge.
    tray::ok(&mut s, &top_edge([14.5, 0.0, 10.0]));
    tray::ok(&mut s, &top_edge([19.0, 5.0, 10.0]));
    let sk = solved(&s, 4);
    let c = circles(&sk);
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(
        near(c[0].0, [10.0, 0.0]) && (c[0].1 - 4.0).abs() < 1e-9,
        "{c:?}"
    );
    let right: Vec<_> = lines(&sk)
        .into_iter()
        .filter(|(a, b)| (a[0] - 20.0).abs() < 1e-9 && (b[0] - 20.0).abs() < 1e-9)
        .collect();
    assert_eq!(right.len(), 1, "only the picked edge is projected");
    assert_eq!(lines(&sk).len(), 3 + 1, "triangle + one projected line");
    // Upstream edits: hole diameter and block width.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "diameter", "value": 10.0}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 1, "name": "width", "value": 50.0}),
    );
    let sk = solved(&s, 4);
    assert!((circles(&sk)[0].1 - 5.0).abs() < 1e-9);
    assert!(
        lines(&sk)
            .iter()
            .any(|(a, b)| (a[0] - 25.0).abs() < 1e-9 && (b[0] - 25.0).abs() < 1e-9),
        "{:?}",
        lines(&sk)
    );
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    let sk = solved(&s, 4);
    assert!(lines(&sk).iter().any(|(a, _)| (a[0] - 20.0).abs() < 1e-9));
    // Save / reopen: format v8, same document.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edges.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(8));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    // The removed hole breaks the circle's link by name.
    let r = tray::exec(
        &mut t,
        &json!({"cmd": "set_suppressed", "feature": 4, "suppressed": true}),
    );
    let msg = format!("{:?} {:?}", r.error, t.summary().features[4].profile_error);
    assert!(
        !r.ok || msg.contains("projection"),
        "suppressing the hole must break or refuse: {msg}"
    );
}

#[test]
fn circle_on_an_oblique_face_projects_to_the_exact_ellipse() {
    let th = 30f64.to_radians();
    let mut s = Session::new();
    holed_block(
        &mut s,
        json!({"Offset": {"base": "WorldXY", "distance": 0.0, "angle": th}}),
    );
    tray::ok(&mut s, &top_edge([14.5, 0.0, 10.0]));
    let sk = solved(&s, 4);
    let el: Vec<_> = sk
        .entities
        .iter()
        .filter_map(|e| match e {
            Entity::Ellipse {
                center,
                major,
                minor,
                ..
            } => Some((sk.point_xy(*center)?, sk.point_xy(*major)?, *minor)),
            _ => None,
        })
        .collect();
    assert_eq!(el.len(), 1, "{:?}", sk.entities);
    let (c, m, b) = el[0];
    // Tilt about the x axis: local y' = (0, cos θ, sin θ); the hole centre
    // (10, 0, 10) lands at (10, 10 sin θ); semi-axes 4 (along x) and 4 cos θ.
    assert!(near(c, [10.0, 10.0 * th.sin()]), "{c:?}");
    assert!(((m[0] - c[0]).hypot(m[1] - c[1]) - 4.0).abs() < 1e-9);
    assert!((m[1] - c[1]).abs() < 1e-9, "major axis along x: {m:?}");
    assert!((b - 4.0 * th.cos()).abs() < 1e-9, "{b}");
    // No user dimension appears for the driven minor radius.
    assert!(sk.dimensions().iter().all(|(n, _)| !n.is_empty()));
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "diameter", "value": 10.0}),
    );
    let sk = solved(&s, 4);
    let b2 = sk
        .entities
        .iter()
        .find_map(|e| match e {
            Entity::Ellipse { minor, .. } => Some(*minor),
            _ => None,
        })
        .unwrap();
    assert!((b2 - 5.0 * th.cos()).abs() < 1e-9, "{b2}");
    // A sketch perpendicular to the face would flatten the circle: refused.
    let mut p = Session::new();
    holed_block(&mut p, json!("WorldXZ"));
    let before = p.model().unwrap().document().clone();
    let r = tray::exec(&mut p, &top_edge([14.5, 0.0, 10.0]));
    assert!(!r.ok);
    assert!(
        format!("{:?}", r.error).contains("segment"),
        "{:?}",
        r.error
    );
    assert_eq!(p.model().unwrap().document(), &before);
    // Missing pick point: refused by name.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "project", "sketch": 5,
                "source": {"Edge": {"feature": 2, "role": "ExtrudeEndCap"}}}),
    );
    assert!(format!("{:?}", r.error).contains("`at`"), "{:?}", r.error);
}

#[test]
fn section_of_the_body_is_associative_and_unlink_freezes_it() {
    let mut s = Session::new();
    holed_block(
        &mut s,
        json!({"Offset": {"base": "WorldXY", "distance": 4.0}}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "project", "sketch": 5,
                "source": {"Section": {"body": 1, "after": 4}}}),
    );
    let sk = solved(&s, 4);
    let big = sk
        .regions()
        .unwrap()
        .into_iter()
        .max_by(|a, b| a.area.total_cmp(&b.area))
        .unwrap();
    let want = |w: f64| w * 30.0 - 16.0 * PI;
    assert!((big.area - want(40.0)).abs() < 1e-6, "{}", big.area);
    tray::ok(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 5, "distance": 5.0, "region": big.id}),
    );
    assert!((body_vol(&s, 2) - 5.0 * want(40.0)).abs() < 1e-6);
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 1, "name": "width", "value": 50.0}),
    );
    assert!(
        (body_vol(&s, 2) - 5.0 * want(50.0)).abs() < 1e-6,
        "{}",
        body_vol(&s, 2)
    );
    // Unlink: the section stays where it is (width 50) when the block changes.
    tray::ok(
        &mut s,
        &json!({"cmd": "unlink_projection", "sketch": 5, "link": 0}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 1, "name": "width", "value": 44.0}),
    );
    assert!(
        (body_vol(&s, 2) - 5.0 * want(50.0)).abs() < 1e-6,
        "{}",
        body_vol(&s, 2)
    );
    // A plane that misses the body: refused, nothing changes.
    let mut p = Session::new();
    holed_block(
        &mut p,
        json!({"Offset": {"base": "WorldXY", "distance": 20.0}}),
    );
    let before = p.model().unwrap().document().clone();
    let r = tray::exec(
        &mut p,
        &json!({"cmd": "project", "sketch": 5,
                "source": {"Section": {"body": 1, "after": 4}}}),
    );
    assert!(!r.ok);
    assert!(
        format!("{:?}", r.error).contains("does not cross"),
        "{:?}",
        r.error
    );
    assert_eq!(p.model().unwrap().document(), &before);
}

#[test]
fn face_with_an_elliptical_hole_projects_the_exact_ellipse() {
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 40.0, "height": 30.0}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_ellipse",
                   "major_radius": 8.0, "minor_radius": 5.0, "center": [-5.0, 0.0]}),
            json!({"cmd": "extrude", "sketch": 3, "op": "cut", "target": 1, "through_all": true}),
            json!({"cmd": "create_sketch",
                   "plane": {"Offset": {"base": "WorldXY", "distance": 20.0}},
                   "template": "polygon", "vertices": [[80.0, 80.0], [81.0, 80.0], [80.0, 81.0]]}),
            json!({"cmd": "project", "sketch": 5,
                   "source": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}}}),
        ],
    );
    let area = |s: &Session| {
        solved(s, 4)
            .regions()
            .unwrap()
            .into_iter()
            .map(|r| r.area)
            .fold(0.0, f64::max)
    };
    assert!(
        (area(&s) - (1200.0 - 40.0 * PI)).abs() < 1e-6,
        "{}",
        area(&s)
    );
    assert!(
        solved(&s, 4)
            .entities
            .iter()
            .any(|e| matches!(e, Entity::Ellipse { .. }))
    );
    // Upstream edit of the hole's minor radius: the projection follows.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "minor_radius", "value": 4.0}),
    );
    assert!(
        (area(&s) - (1200.0 - 32.0 * PI)).abs() < 1e-6,
        "{}",
        area(&s)
    );
}
