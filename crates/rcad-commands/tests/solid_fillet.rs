//! Exact solid edge fillet (run 5, `FeatureKind::Fillet` over
//! `rcad_kernel::fillet_edge`): a right-angle edge of radius r removes
//! (r² − πr²/4) per unit length; the fillet follows upstream edits, undo/redo
//! and save/reopen (format v5); refusals leave the document unchanged; the
//! organizer tray's four outer vertical edges round exactly.
mod support;
use rcad_commands::Session;
use serde_json::json;
use std::f64::consts::PI;
use support::tray::{self, Tray};

fn corner_area(r: f64) -> f64 {
    r * r - PI * r * r / 4.0
}

fn block(s: &mut Session, w: f64) {
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": w, "height": 30.0, "center": [w / 2.0, 15.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 20.0}),
    ] {
        tray::ok(s, &v);
    }
}

fn top_front(r: f64) -> serde_json::Value {
    json!({"cmd": "fillet", "a": {"feature": 2, "role": "ExtrudeEndCap"},
           "b": {"feature": 2, "role": {"ExtrudeSide": {"entity": 1}}}, "radius": r})
}

#[test]
fn edge_fillet_is_exact_follows_edits_and_persists() {
    let mut s = Session::new();
    block(&mut s, 40.0);
    tray::ok(&mut s, &top_front(3.0));
    let want = |w: f64, r: f64| w * 30.0 * 20.0 - corner_area(r) * w;
    assert!((tray::volume(&s) - want(40.0, 3.0)).abs() < 1e-6);
    // The rounded corner: air at the sharp corner, material inside the arc.
    let p = tray::probe(
        &s,
        &[[20.0, 0.2, 19.8], [20.0, 1.5, 18.5], [20.0, 3.5, 17.0]],
    );
    assert_eq!(p, vec![Some(false), Some(true), Some(true)]);
    // Upstream edit: a wider block, the fillet runs the new length.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 1, "name": "width", "value": 50.0}),
    );
    assert!((tray::volume(&s) - want(50.0, 3.0)).abs() < 1e-6);
    // Its radius is a parameter.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "radius", "value": 5.0}),
    );
    assert!((tray::volume(&s) - want(50.0, 5.0)).abs() < 1e-6);
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert!((tray::volume(&s) - want(50.0, 3.0)).abs() < 1e-6);
    tray::ok(&mut s, &json!({"cmd": "redo"}));
    // Save / reopen: v5, same document and volume.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(5));
    let mut s2 = Session::new();
    tray::ok(&mut s2, &json!({"cmd": "open", "path": path}));
    assert_eq!(
        s2.model().unwrap().document(),
        s.model().unwrap().document()
    );
    assert!((tray::volume(&s2) - want(50.0, 5.0)).abs() < 1e-6);
}

#[test]
fn edge_fillet_refusals_leave_the_document_unchanged() {
    let mut s = Session::new();
    block(&mut s, 40.0);
    let before = s.model().unwrap().document().clone();
    for bad in [
        top_front(25.0), // does not fit the 20 mm side
        top_front(0.0),
        json!({"cmd": "fillet", "a": {"feature": 2, "role": "ExtrudeEndCap"},
               "b": {"feature": 2, "role": "ExtrudeStartCap"}, "radius": 1.0}), // parallel
    ] {
        let r = tray::exec(&mut s, &bad);
        assert!(!r.ok, "{bad}");
        assert_eq!(s.model().unwrap().document(), &before, "{bad}");
    }
}

#[test]
fn tray_outer_vertical_edges_round_exactly() {
    let t = Tray::BASELINE;
    let mut s = tray::build(&t, true);
    let r = 4.0;
    for (i, (a, b)) in [(1, 2), (2, 3), (3, 4), (4, 1)].into_iter().enumerate() {
        tray::ok(
            &mut s,
            &json!({"cmd": "fillet", "name": format!("Corner {}", i + 1),
                    "a": {"feature": tray::BLANK, "role": {"ExtrudeSide": {"entity": a}}},
                    "b": {"feature": tray::BLANK, "role": {"ExtrudeSide": {"entity": b}}},
                    "radius": r}),
        );
    }
    let want = t.notched_volume() - 4.0 * corner_area(r) * t.h;
    assert!(
        (tray::volume(&s) - want).abs() < 1e-6,
        "{} vs {want}",
        tray::volume(&s)
    );
    tray::assert_occupancy(&s, &t, true);
    // And after a resize through the parameters.
    let v = Tray {
        l: 150.0,
        n: 3,
        ..t
    };
    tray::ok(&mut s, &serde_json::to_value(v.set_all()).unwrap());
    let want = v.notched_volume() - 4.0 * corner_area(r) * v.h;
    assert!((tray::volume(&s) - want).abs() < 1e-6);
}
