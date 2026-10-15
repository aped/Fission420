//! Associative projection of single body edges and body sections
//! ("intersect") into sketches: a parallel edge keeps its line or circle, a
//! circle on a face oblique to the sketch becomes the exact ellipse, a
//! section yields the body's outline and holes in the sketch plane.
//! Upstream edits move the geometry, a vanished edge breaks the link by name,
//! unlink freezes, refusals name the reason, undo and save/reopen. Edges a
//! cut leaves in another feature's face follow that cut by identity.
//! Expected geometry and volumes are closed-form.
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
    // Save / reopen: same document. Format v9: the hole rim is bound to the
    // cut feature that made it (a v8 reader would bind it by position).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edges.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(9));
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

/// Regression: a projected top edge must stay the top edge when a large
/// upstream edit moves it farther than another edge's distance to the old
/// pins (here the right edge would be nearer). The edge keeps its identity
/// (the generating sketch line), so it is the new top edge, y = 30.
#[test]
fn projected_edge_keeps_its_identity_under_large_edits() {
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 20.0, "height": 10.0, "center": [10.0, 5.0]}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 5.0}),
            json!({"cmd": "create_sketch",
                   "plane": {"Offset": {"base": "WorldXY", "distance": 20.0}},
                   "template": "polygon", "vertices": [[80.0, 80.0], [81.0, 80.0], [80.0, 81.0]]}),
            json!({"cmd": "project", "sketch": 3,
                   "source": {"Edge": {"feature": 2, "role": "ExtrudeEndCap"}},
                   "at": [10.0, 9.5, 5.0]}),
        ],
    );
    let top = |s: &Session| {
        let sk = solved(s, 2);
        lines(&sk)
            .into_iter()
            .filter(|(a, _)| a[0] < 50.0)
            .collect::<Vec<_>>()
    };
    let l = top(&s);
    assert_eq!(l.len(), 1);
    assert!(
        (l[0].0[1] - 10.0).abs() < 1e-9 && (l[0].1[1] - 10.0).abs() < 1e-9,
        "{l:?}"
    );
    // 0..10 → 0..30: the top edge moves 20 mm; the right edge's ends would be
    // nearer to the old pins.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 1, "name": "height", "value": 30.0}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 1, "name": "center_y", "value": 15.0}),
    );
    let l = top(&s);
    assert_eq!(l.len(), 1);
    assert!(
        (l[0].0[1] - 30.0).abs() < 1e-9 && (l[0].1[1] - 30.0).abs() < 1e-9,
        "the projection must follow the top edge: {l:?}"
    );
}

/// Edges projected one at a time share their end points where they meet, so
/// four projected rectangle edges form a closed profile that extrudes.
#[test]
fn edges_projected_one_by_one_join_into_a_profile() {
    let mut s = Session::new();
    holed_block(
        &mut s,
        json!({"Offset": {"base": "WorldXY", "distance": 20.0}}),
    );
    for at in [
        [0.0, -14.5, 10.0],
        [19.5, 5.0, 10.0],
        [0.0, 14.5, 10.0],
        [-19.5, 0.0, 10.0],
    ] {
        tray::ok(&mut s, &top_edge(at));
    }
    let sk = solved(&s, 4);
    let regions = sk.regions().unwrap();
    let big = regions
        .into_iter()
        .max_by(|a, b| a.area.total_cmp(&b.area))
        .unwrap();
    assert!((big.area - 1200.0).abs() < 1e-6, "{}", big.area);
    tray::ok(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 5, "distance": 5.0, "region": big.id}),
    );
    assert!(
        (body_vol(&s, 2) - 6000.0).abs() < 1e-6,
        "{}",
        body_vol(&s, 2)
    );
    // Still associative: a wider block widens the projected rectangle.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 1, "name": "width", "value": 50.0}),
    );
    assert!(
        (body_vol(&s, 2) - 7500.0).abs() < 1e-6,
        "{}",
        body_vol(&s, 2)
    );
}

/// A face oblique to the sketch with an arc edge: its straight edges still
/// project (lines stay lines); only picking the arc itself is refused.
#[test]
fn oblique_face_with_an_arc_projects_its_straight_edges() {
    let th = 30f64.to_radians();
    let profile = json!({
        "points": [
            {"id": 1, "x": -20.0, "y": -15.0}, {"id": 2, "x": 20.0, "y": -15.0},
            {"id": 3, "x": 20.0, "y": 15.0}, {"id": 4, "x": -20.0, "y": 15.0},
            {"id": 5, "x": 20.0, "y": 0.0}
        ],
        "entities": [
            {"Line": {"id": 1, "a": 1, "b": 2}},
            {"Arc": {"id": 2, "center": 5, "start": 2, "end": 3}},
            {"Line": {"id": 3, "a": 3, "b": 4}},
            {"Line": {"id": 4, "a": 4, "b": 1}}
        ],
        "constraints": []
    });
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "custom",
                   "sketch": profile}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            json!({"cmd": "create_sketch",
                   "plane": {"Offset": {"base": "WorldXY", "distance": 0.0, "angle": th}},
                   "template": "polygon", "vertices": [[80.0, 80.0], [81.0, 80.0], [80.0, 81.0]]}),
        ],
    );
    let edge = |at: [f64; 3]| {
        json!({"cmd": "project", "sketch": 3,
               "source": {"Edge": {"feature": 2, "role": "ExtrudeEndCap"}}, "at": at})
    };
    // The left edge x = -20 (local y' = y cos θ + 10 sin θ).
    tray::ok(&mut s, &edge([-19.5, 0.0, 10.0]));
    let sk = solved(&s, 2);
    let l: Vec<_> = lines(&sk)
        .into_iter()
        .filter(|(a, _)| a[0] < 50.0)
        .collect();
    assert_eq!(l.len(), 1, "{l:?}");
    let ys = [
        15.0 * th.cos() + 10.0 * th.sin(),
        -15.0 * th.cos() + 10.0 * th.sin(),
    ];
    for p in [l[0].0, l[0].1] {
        assert!((p[0] + 20.0).abs() < 1e-9, "{p:?}");
        assert!(ys.iter().any(|y| (p[1] - y).abs() < 1e-9), "{p:?}");
    }
    // Picking the arc: refused by name, nothing changes.
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(&mut s, &edge([34.5, 0.0, 10.0]));
    assert!(!r.ok);
    assert!(
        format!("{:?}", r.error).contains("elliptical arc"),
        "{:?}",
        r.error
    );
    assert_eq!(s.model().unwrap().document(), &before);
}

/// The tool identity stored for a projected curve: (`from`, `from_tool`).
fn link_identity(s: &Session, sketch: u32) -> Vec<(Option<u32>, Option<u32>)> {
    let doc = s.model().unwrap().document();
    let f = doc
        .features
        .iter()
        .find(|f| f.id.0 == sketch)
        .expect("sketch");
    match &f.kind {
        rcad_document::FeatureKind::Sketch { links, .. } => links
            .iter()
            .flat_map(|l| l.loops.iter().flatten())
            .map(|c| (c.from.map(|e| e.0), c.from_tool.map(|t| t.0)))
            .collect(),
        _ => panic!("not a sketch"),
    }
}

/// The rim a through cut leaves in the block's top face carries the cut
/// feature and its circle entity; resizing and moving the hole moves the
/// projected circle by that identity.
#[test]
fn hole_rim_follows_its_cut_by_identity() {
    let mut s = Session::new();
    holed_block(
        &mut s,
        json!({"Offset": {"base": "WorldXY", "distance": 20.0}}),
    );
    tray::ok(&mut s, &top_edge([14.5, 0.0, 10.0]));
    let ids = link_identity(&s, 5);
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0].1, Some(4), "bound to the cut feature: {ids:?}");
    assert!(ids[0].0.is_some());
    for (name, value) in [("diameter", 6.0), ("center_x", -8.0), ("center_y", 5.0)] {
        tray::ok(
            &mut s,
            &json!({"cmd": "set_parameter", "feature": 3, "name": name, "value": value}),
        );
    }
    let c = circles(&solved(&s, 4));
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(
        near(c[0].0, [-8.0, 5.0]) && (c[0].1 - 3.0).abs() < 1e-9,
        "{c:?}"
    );
}

/// Two holes; the projected rim of one follows its own hole when that hole
/// moves past the other (position alone cannot tell them apart: the edge
/// moved farther than its size, which the nearness fallback refuses).
#[test]
fn projected_rim_follows_its_own_hole_past_another() {
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 60.0, "height": 30.0}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
                   "diameter": 6.0, "center": [15.0, 0.0]}),
            json!({"cmd": "extrude", "sketch": 3, "op": "cut", "target": 1, "through_all": true}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
                   "diameter": 6.0, "center": [0.0, 0.0]}),
            json!({"cmd": "extrude", "sketch": 5, "op": "cut", "target": 1, "through_all": true}),
            json!({"cmd": "create_sketch", "plane": {"Offset": {"base": "WorldXY", "distance": 20.0}},
                   "template": "polygon", "vertices": [[80.0, 80.0], [81.0, 80.0], [80.0, 81.0]]}),
        ],
    );
    // Project the rim of the first hole (cut feature 4).
    tray::ok(
        &mut s,
        &json!({"cmd": "project", "sketch": 7,
                "source": {"Edge": {"feature": 2, "role": "ExtrudeEndCap"}}, "at": [18.5, 0.0, 10.0]}),
    );
    assert_eq!(link_identity(&s, 7)[0].1, Some(4));
    let rim = |s: &Session| {
        let c = circles(&solved(s, 6));
        assert_eq!(c.len(), 1, "{c:?}");
        c[0].0
    };
    assert!(near(rim(&s), [15.0, 0.0]));
    // Move the first hole across the second, to the far side.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "center_x", "value": -15.0}),
    );
    assert!(near(rim(&s), [-15.0, 0.0]), "{:?}", rim(&s));
    // And the other hole moving does not drag it.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 5, "name": "center_x", "value": 10.0}),
    );
    assert!(near(rim(&s), [-15.0, 0.0]), "{:?}", rim(&s));
    // Undo restores the earlier binding.
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert!(near(rim(&s), [15.0, 0.0]));
}

/// A blind pocket's rim in the top face also carries its cut (the face lies
/// at the end of the tool's span); its diameter edit is followed.
#[test]
fn blind_pocket_rim_follows_its_cut() {
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 40.0, "height": 30.0}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            json!({"cmd": "create_sketch", "plane": {"Offset": {"base": "WorldXY", "distance": 10.0}},
                   "template": "centered_circle", "diameter": 8.0, "center": [10.0, 0.0]}),
            json!({"cmd": "extrude", "sketch": 3, "op": "cut", "target": 1, "distance": 4.0}),
            json!({"cmd": "create_sketch", "plane": {"Offset": {"base": "WorldXY", "distance": 20.0}},
                   "template": "polygon", "vertices": [[80.0, 80.0], [81.0, 80.0], [80.0, 81.0]]}),
        ],
    );
    tray::ok(&mut s, &top_edge([14.5, 0.0, 10.0]));
    assert_eq!(link_identity(&s, 5)[0].1, Some(4));
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "diameter", "value": 12.0}),
    );
    let c = circles(&solved(&s, 4));
    assert!(
        near(c[0].0, [10.0, 0.0]) && (c[0].1 - 6.0).abs() < 1e-9,
        "{c:?}"
    );
}
