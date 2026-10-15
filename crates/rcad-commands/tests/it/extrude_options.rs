//! Extrusion options: symmetric,
//! two sides, start offset, up to a plane (following that plane upstream),
//! thin walls, taper, Intersect, several regions in one feature; undo/redo,
//! save/reopen as format v6 and refusal of v6 content in an older-labelled
//! file. Expected values are closed-form from the dimensions.
use crate::support;
use rcad_commands::Session;
use rcad_sketch::Sketch;
use serde_json::{Value, json};
use std::f64::consts::PI;
use support::tray;

fn bounds(s: &Session, body: u32) -> ([f64; 3], [f64; 3]) {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id.0 == body)
        .expect("body");
    let bb = b.bounds.expect("bounds");
    (bb.min, bb.max)
}

fn vol(s: &Session, body: u32) -> f64 {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id.0 == body)
        .expect("body");
    match b.measurements.expect("measured").volume {
        rcad_kernel::VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn close(a: [f64; 3], b: [f64; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < 1e-6)
}

fn rect(plane: Value, w: f64, h: f64, c: [f64; 2]) -> Value {
    json!({"cmd": "create_sketch", "plane": plane, "template": "centered_rectangle",
           "width": w, "height": h, "center": c})
}

fn circle(plane: Value, d: f64, c: [f64; 2]) -> Value {
    json!({"cmd": "create_sketch", "plane": plane, "template": "centered_circle",
           "diameter": d, "center": c})
}

fn run(s: &mut Session, steps: &[Value]) {
    for v in steps {
        tray::ok(s, v);
    }
}

#[test]
fn symmetric_two_sides_and_start_offset_on_two_orientations() {
    // World XY (normal +Z) and World XZ (normal −Y): the same spans.
    for (plane, axis, sign) in [("WorldXY", 2usize, 1.0), ("WorldXZ", 1usize, -1.0)] {
        for (opts, d, lo, hi) in [
            (json!({"symmetric": true}), 10.0, -5.0, 5.0),
            (json!({"distance2": 3.0}), 8.0, -3.0, 8.0),
            (json!({"start_offset": 2.0}), 5.0, 2.0, 7.0),
            (
                json!({"start_offset": -4.0, "symmetric": true}),
                6.0,
                -7.0,
                -1.0,
            ),
        ] {
            let mut s = Session::new();
            run(
                &mut s,
                &[
                    json!({"cmd": "new_document"}),
                    rect(json!(plane), 20.0, 10.0, [0.0, 0.0]),
                    json!({"cmd": "extrude", "sketch": 1, "distance": d, "options": opts}),
                ],
            );
            let (mn, mx) = bounds(&s, 1);
            let (a, b) = if sign > 0.0 { (lo, hi) } else { (-hi, -lo) };
            assert!(
                (mn[axis] - a).abs() < 1e-9 && (mx[axis] - b).abs() < 1e-9,
                "{plane} {opts}: {mn:?} {mx:?}"
            );
            assert!((vol(&s, 1) - 200.0 * (hi - lo)).abs() < 1e-6);
        }
    }
}

#[test]
fn to_plane_follows_the_face_it_runs_to() {
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            rect(json!("WorldXY"), 40.0, 30.0, [0.0, 0.0]),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            // A post sketched 30 mm up, joined down to the block's top face.
            circle(
                json!({"Offset": {"base": "WorldXY", "distance": 30.0}}),
                8.0,
                [0.0, 0.0],
            ),
            json!({"cmd": "extrude", "sketch": 3, "op": "join", "target": 1,
                   "options": {"to": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}}}}),
        ],
    );
    let post = |top: f64| 1200.0 * top + 16.0 * PI * (30.0 - top);
    assert!((vol(&s, 1) - post(10.0)).abs() < 1e-6, "{}", vol(&s, 1));
    assert!(close(bounds(&s, 1).1, [20.0, 15.0, 30.0]));
    // Upstream edit: the block grows to 15; the post now starts at 15.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "distance", "value": 15.0}),
    );
    assert!((vol(&s, 1) - post(15.0)).abs() < 1e-6);
    // The to-plane is a dependency: the Join lists the block's extrusion.
    let deps = s.summary().features[3].depends_on.clone();
    assert!(deps.iter().any(|f| f.0 == 2), "{deps:?}");
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert!((vol(&s, 1) - post(10.0)).abs() < 1e-6);
    tray::ok(&mut s, &json!({"cmd": "redo"}));
    assert!((vol(&s, 1) - post(15.0)).abs() < 1e-6);
    // A to-plane with an offset (2 mm past the face, i.e. into the block:
    // the face's outward normal is +Z, so −2).
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 4, "name": "to_offset", "value": -2.0}),
    );
    assert!(
        (vol(&s, 1) - post(15.0)).abs() < 1e-6,
        "offset into material adds nothing"
    );
}

#[test]
fn to_plane_refusals_leave_the_document_unchanged() {
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            rect(json!("WorldXY"), 40.0, 30.0, [0.0, 0.0]),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            circle(
                json!({"Offset": {"base": "WorldXY", "distance": 30.0}}),
                8.0,
                [0.0, 0.0],
            ),
        ],
    );
    let before = s.model().unwrap().document().clone();
    for opts in [
        // Not parallel to the sketch plane.
        json!({"to": "WorldXZ"}),
        // The sketch plane itself.
        json!({"to": {"Offset": {"base": "WorldXY", "distance": 30.0}}}),
        // Contradictory extents.
        json!({"to": "WorldXY", "symmetric": true}),
        json!({"symmetric": true, "distance2": 2.0}),
    ] {
        let r = tray::exec(
            &mut s,
            &json!({"cmd": "extrude", "sketch": 3, "distance": 5.0, "op": "join", "target": 1,
                    "options": opts}),
        );
        assert!(!r.ok, "{opts} accepted");
        assert_eq!(s.model().unwrap().document(), &before);
    }
}

#[test]
fn thin_extrusion_and_its_limits() {
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            rect(json!("WorldXY"), 40.0, 30.0, [0.0, 0.0]),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0, "options": {"thin": 2.0}}),
        ],
    );
    assert!((vol(&s, 1) - (1200.0 - 36.0 * 26.0) * 10.0).abs() < 1e-6);
    let p = tray::probe(&s, &[[0.0, 0.0, 5.0], [19.0, 0.0, 5.0], [17.9, 0.0, 5.0]]);
    assert_eq!(p, vec![Some(false), Some(true), Some(false)]);
    // Thickness edit, then one that does not fit (15 + 15 = 30).
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "thin", "value": 3.0}),
    );
    assert!((vol(&s, 1) - (1200.0 - 34.0 * 24.0) * 10.0).abs() < 1e-6);
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "thin", "value": 15.0}),
    );
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().document(), &before);
    // A thin circle: a tube.
    let mut t = Session::new();
    run(
        &mut t,
        &[
            json!({"cmd": "new_document"}),
            circle(json!("WorldXY"), 20.0, [0.0, 0.0]),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0, "options": {"thin": 1.5}}),
        ],
    );
    assert!((vol(&t, 1) - PI * (100.0 - 8.5 * 8.5) * 10.0).abs() < 1e-6);
}

#[test]
fn tapered_extrusion_matches_the_loft_formula() {
    // 40 x 30 grows to 60 x 50 over 20: offset +10 = −20·tan(taper), so
    // taper = −atan(1/2) (negative: the profile grows).
    let mut s = Session::new();
    let taper = -(0.5f64).atan();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            rect(json!("WorldXY"), 40.0, 30.0, [0.0, 0.0]),
            json!({"cmd": "extrude", "sketch": 1, "distance": 20.0, "options": {"taper": taper}}),
        ],
    );
    let want = 20.0 * (1200.0 + (40.0 * 20.0 + 30.0 * 20.0) / 2.0 + 400.0 / 3.0);
    assert!((vol(&s, 1) - want).abs() < 1e-6, "{}", vol(&s, 1));
    assert!(close(bounds(&s, 1).1, [30.0, 25.0, 20.0]));
    // Draft inward 10°: the circle frustum.
    let mut t = Session::new();
    run(
        &mut t,
        &[
            json!({"cmd": "new_document"}),
            circle(json!("WorldXY"), 20.0, [0.0, 0.0]),
            json!({"cmd": "extrude", "sketch": 1, "distance": 15.0,
                   "options": {"taper": 10f64.to_radians()}}),
        ],
    );
    let r2 = 10.0 - 15.0 * 10f64.to_radians().tan();
    let want = PI * 15.0 / 3.0 * (100.0 + 10.0 * r2 + r2 * r2);
    assert!((vol(&t, 1) - want).abs() < 1e-6, "{} vs {want}", vol(&t, 1));
    // The taper is a parameter (degrees); too steep a taper is refused.
    tray::ok(
        &mut t,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "taper_deg", "value": 5.0}),
    );
    let before = t.model().unwrap().document().clone();
    let r = tray::exec(
        &mut t,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "taper_deg", "value": 40.0}),
    );
    assert!(
        !r.ok,
        "a taper closing the circle before 15 mm was accepted"
    );
    assert_eq!(t.model().unwrap().document(), &before);
}

#[test]
fn intersect_keeps_the_common_material() {
    // Box 40 x 30 x 10 ∩ Ø20 cylinder from World XY, 10 deep: a disc.
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            rect(json!("WorldXY"), 40.0, 30.0, [20.0, 15.0]),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            circle(json!("WorldXY"), 20.0, [20.0, 15.0]),
            json!({"cmd": "extrude", "sketch": 3, "distance": 10.0, "op": "intersect",
                   "target": 1}),
        ],
    );
    assert!((vol(&s, 1) - 1000.0 * PI).abs() < 1e-6, "{}", vol(&s, 1));
    // Another orientation: a triangle on World XZ (the box's front face,
    // normal −Y) intersected 30 deep into the material: a wedge.
    let mut t = Session::new();
    run(
        &mut t,
        &[
            json!({"cmd": "new_document"}),
            rect(json!("WorldXY"), 40.0, 30.0, [20.0, 15.0]),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            json!({"cmd": "create_sketch", "plane": "WorldXZ", "template": "polygon",
                   "vertices": [[0.0, 0.0], [40.0, 0.0], [0.0, 10.0]]}),
            json!({"cmd": "extrude", "sketch": 3, "distance": 30.0, "op": "intersect",
                   "target": 1}),
        ],
    );
    assert!((vol(&t, 1) - 6000.0).abs() < 1e-6, "{}", vol(&t, 1));
    let p = tray::probe(&t, &[[5.0, 15.0, 2.0], [35.0, 15.0, 5.0]]);
    assert_eq!(p, vec![Some(true), Some(false)]);
    // Shorter intersect: only 12 of the 30 mm.
    tray::ok(
        &mut t,
        &json!({"cmd": "set_parameter", "feature": 4, "name": "distance", "value": 12.0}),
    );
    assert!((vol(&t, 1) - 200.0 * 12.0).abs() < 1e-6);
    // No common material: refused, unchanged.
    let before = t.model().unwrap().document().clone();
    let r = tray::exec(
        &mut t,
        &json!({"cmd": "create_sketch", "plane": {"Offset": {"base": "WorldXY",
                "distance": 50.0}}, "template": "centered_circle", "diameter": 5.0,
                "center": [20.0, 15.0]}),
    );
    assert!(r.ok);
    let r = tray::exec(
        &mut t,
        &json!({"cmd": "extrude", "sketch": 5, "distance": 5.0, "op": "intersect",
                "target": 1}),
    );
    assert!(!r.ok, "an empty intersection was accepted");
    tray::ok(&mut t, &json!({"cmd": "undo"}));
    assert_eq!(t.model().unwrap().document(), &before);
}

#[test]
fn symmetric_join_and_two_sided_cut_on_a_block() {
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            rect(json!("WorldXY"), 40.0, 30.0, [0.0, 0.0]),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
            // Boss on the top face, symmetric 6: 3 above the face.
            circle(
                json!({"Face": {"feature": 2, "role": "ExtrudeEndCap"}}),
                8.0,
                [0.0, 0.0],
            ),
            json!({"cmd": "extrude", "sketch": 3, "distance": 6.0, "op": "join", "target": 1,
                   "options": {"symmetric": true}}),
            // A Ø6 blind hole from below up to z = 7, from a mid plane at 5
            // (2 up, 6 down: through the bottom face).
            circle(
                json!({"Offset": {"base": "WorldXY", "distance": 5.0}}),
                6.0,
                [10.0, 0.0],
            ),
            json!({"cmd": "extrude", "sketch": 5, "distance": 2.0, "op": "cut", "target": 1,
                   "options": {"distance2": 6.0}}),
        ],
    );
    let want = 12000.0 + 16.0 * PI * 3.0 - 9.0 * PI * 7.0;
    assert!((vol(&s, 1) - want).abs() < 1e-6, "{} vs {want}", vol(&s, 1));
    let p = tray::probe(
        &s,
        &[
            [10.0, 0.0, 0.5],
            [10.0, 0.0, 6.9],
            [10.0, 0.0, 7.1],
            [0.0, 0.0, 12.9],
        ],
    );
    assert_eq!(p, vec![Some(false), Some(false), Some(true), Some(true)]);
    // Fully inside (z 2..7 from the mid plane): an enclosed void, refused.
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 6, "name": "distance2", "value": 3.0}),
    );
    assert!(!r.ok);
    assert!(format!("{:?}", r.error).contains("void"), "{:?}", r.error);
    assert_eq!(s.model().unwrap().document(), &before);
}

#[test]
fn several_regions_cut_in_one_feature() {
    // Two separate rectangles on the top face cut in one feature.
    let mut sk = Sketch::default();
    for (x0, x1) in [(-15.0, -5.0), (5.0, 15.0)] {
        let p: Vec<_> = [[x0, -5.0], [x1, -5.0], [x1, 5.0], [x0, 5.0]]
            .iter()
            .map(|q| sk.add_point(q[0], q[1]))
            .collect();
        for i in 0..4 {
            sk.add_line(p[i], p[(i + 1) % 4]);
        }
    }
    let regions: Vec<_> = sk.regions().unwrap().into_iter().map(|r| r.id).collect();
    assert_eq!(regions.len(), 2);
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            rect(json!("WorldXY"), 40.0, 30.0, [0.0, 0.0]),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
        ],
    );
    let r = s.execute(&rcad_commands::Command::CreateSketch {
        name: None,
        plane: serde_json::from_value(json!({"Face": {"feature": 2, "role": "ExtrudeEndCap"}}))
            .unwrap(),
        template: rcad_commands::SketchTemplate::Custom { sketch: sk },
    });
    assert!(r.ok, "{:?}", r.error);
    tray::ok(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 3, "distance": 4.0, "op": "cut", "target": 1,
                "region": regions[0], "options": {"regions": [regions[1]]}}),
    );
    assert!((vol(&s, 1) - (12000.0 - 2.0 * 100.0 * 4.0)).abs() < 1e-6);
    // New Body with several regions: separate solids, refused.
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 3, "distance": 4.0,
                "region": regions[0], "options": {"regions": [regions[1]]}}),
    );
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().document(), &before);
}

#[test]
fn options_save_as_v6_and_older_labels_are_refused() {
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            rect(json!("WorldXY"), 20.0, 10.0, [0.0, 0.0]),
            json!({"cmd": "extrude", "sketch": 1, "distance": 10.0,
                   "options": {"symmetric": true}}),
        ],
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("o.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(6));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert!((vol(&t, 1) - 2000.0).abs() < 1e-6);
    // The same content labelled v5 is refused (a v5 reader would ignore
    // the options and build a different solid).
    v["version"] = json!(5);
    let old = dir.path().join("o5.rcad.json");
    std::fs::write(&old, serde_json::to_string(&v).unwrap()).unwrap();
    let r = tray::exec(&mut t, &json!({"cmd": "open", "path": old}));
    assert!(!r.ok);
    let e = format!("{:?}", r.error);
    assert!(e.contains("v6 feature"), "{e}");
}

#[test]
fn thin_open_chain_ribs() {
    // Plate 60 x 40 x 5; an L-shaped rib (centre line (10,10)-(30,10)-(30,30),
    // 2 thick, 10 high) joined on the top face: a mitred band of area
    // length × thickness = 40 × 2.
    let mut s = Session::new();
    run(
        &mut s,
        &[
            json!({"cmd": "new_document"}),
            rect(json!("WorldXY"), 60.0, 40.0, [30.0, 20.0]),
            json!({"cmd": "extrude", "sketch": 1, "distance": 5.0}),
        ],
    );
    let mut sk = Sketch::default();
    let p: Vec<_> = [[10.0, 10.0], [30.0, 10.0], [30.0, 30.0]]
        .iter()
        .map(|q| sk.add_point(q[0], q[1]))
        .collect();
    sk.add_line(p[0], p[1]);
    sk.add_line(p[1], p[2]);
    let r = s.execute(&rcad_commands::Command::CreateSketch {
        name: None,
        plane: serde_json::from_value(json!({"Face": {"feature": 2, "role": "ExtrudeEndCap"}}))
            .unwrap(),
        template: rcad_commands::SketchTemplate::Custom { sketch: sk },
    });
    assert!(r.ok, "{:?}", r.error);
    tray::ok(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 3, "distance": 10.0, "op": "join", "target": 1,
                "options": {"thin": 2.0}}),
    );
    assert!(
        (vol(&s, 1) - (12000.0 + 800.0)).abs() < 1e-6,
        "{}",
        vol(&s, 1)
    );
    let p = tray::probe(
        &s,
        &[
            [20.0, 10.9, 10.0],
            [20.0, 11.1, 10.0],
            [30.9, 29.0, 10.0],
            [31.1, 9.1, 10.0],
        ],
    );
    assert_eq!(p, vec![Some(true), Some(false), Some(true), Some(false)]);
    // Rib thickness is the `thin` parameter.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 4, "name": "thin", "value": 3.0}),
    );
    assert!((vol(&s, 1) - (12000.0 + 1200.0)).abs() < 1e-6);
}
