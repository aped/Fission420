//! Cap-loop fillet through the command API (`fillet_loop`,
//! `FeatureKind::FilletLoop` over `rcad_kernel::fillet_cap_loop`): the
//! common print workflow of a box whose vertical edges are rounded R and whose
//! top edge loop is then rounded r. Expected volumes come from the
//! dimensions alone: the removed spandrel has area A = r²(1 − π/4) and first
//! moment M = r³(5/6 − π/4) about a wall; along the straight sides it removes
//! A per unit length, around each quarter-turn corner of radius R it removes
//! (π/2)(A·R − M) (Pappus). The measured volume is the exact parametric
//! boundary integral and must agree to 1e-9 relative.
use crate::support;
use rcad_commands::Session;
use serde_json::json;
use std::f64::consts::PI;
use support::tray;

fn side(e: u32) -> serde_json::Value {
    json!({"feature": 2, "role": {"ExtrudeSide": {"entity": e}}})
}

fn top() -> serde_json::Value {
    json!({"feature": 2, "role": "ExtrudeEndCap"})
}

/// Box w × 30 × d with its vertical edges rounded `big` (features 3..=6).
fn rounded_box(s: &mut Session, w: f64, d: f64, big: f64) {
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": w, "height": 30.0, "center": [w / 2.0, 15.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": d}),
    ] {
        tray::ok(s, &v);
    }
    for (a, b) in [(1, 2), (2, 3), (3, 4), (4, 1)] {
        tray::ok(
            s,
            &json!({"cmd": "fillet", "a": side(a), "b": side(b), "radius": big}),
        );
    }
}

fn want(w: f64, d: f64, big: f64, r: f64) -> f64 {
    let (a, m) = (r * r * (1.0 - PI / 4.0), r.powi(3) * (5.0 / 6.0 - PI / 4.0));
    let area = w * 30.0 - (4.0 - PI) * big * big;
    let straight = 2.0 * (w - 2.0 * big) + 2.0 * (30.0 - 2.0 * big);
    area * d - a * straight - 2.0 * PI * (a * big - m)
}

fn close(s: &Session, v: f64) {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id == rcad_document::BodyId(1))
        .expect("body");
    let rcad_kernel::VolumeResult::Value {
        volume,
        error_estimate,
        ..
    } = b.measurements.expect("measured").volume
    else {
        panic!("no volume");
    };
    assert!(error_estimate <= 1e-9 * v, "error bound {error_estimate}");
    assert!(
        (volume - v).abs() <= 1e-9 * v,
        "volume {volume} (±{error_estimate}) vs closed form {v}"
    );
}

fn set(s: &mut Session, feature: u32, name: &str, value: f64) -> rcad_commands::Response {
    tray::exec(
        s,
        &json!({"cmd": "set_parameter", "feature": feature, "name": name, "value": value}),
    )
}

#[test]
fn top_loop_fillet_follows_edits_undo_and_persists() {
    let mut s = Session::new();
    rounded_box(&mut s, 40.0, 10.0, 4.0);
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "fillet_loop", "face": top(), "radius": 2.0}),
    );
    assert_eq!(r.feature.map(|f| f.0), Some(7));
    close(&s, want(40.0, 10.0, 4.0, 2.0));
    // Torus at the (0,0) corner, 45° around and 45° up: air just outside,
    // material just inside.
    let k = 45f64.to_radians();
    let tube = 4.0 - 2.0 + 2.0 * k.cos();
    let p = [
        4.0 - tube * k.cos(),
        4.0 - tube * k.sin(),
        8.0 + 2.0 * k.sin(),
    ];
    let n = [-k.cos() * k.cos(), -k.cos() * k.sin(), k.sin()];
    let at = |e: f64| [p[0] + e * n[0], p[1] + e * n[1], p[2] + e * n[2]];
    assert_eq!(
        tray::probe(&s, &[at(0.02), at(-0.02)]),
        vec![Some(false), Some(true)]
    );
    // Upstream edits: block width, height, the loop radius, R = r (sphere corners).
    assert!(set(&mut s, 1, "width", 50.0).ok);
    close(&s, want(50.0, 10.0, 4.0, 2.0));
    assert!(set(&mut s, 2, "distance", 14.0).ok);
    close(&s, want(50.0, 14.0, 4.0, 2.0));
    assert!(set(&mut s, 7, "radius", 4.0).ok);
    close(&s, want(50.0, 14.0, 4.0, 4.0));
    // Undo / redo restore the radius.
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    close(&s, want(50.0, 14.0, 4.0, 2.0));
    tray::ok(&mut s, &json!({"cmd": "redo"}));
    close(&s, want(50.0, 14.0, 4.0, 4.0));
    // Save / reopen: format v8, same document and geometry.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loop.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(8));
    let mut s2 = Session::new();
    tray::ok(&mut s2, &json!({"cmd": "open", "path": path}));
    assert_eq!(
        s2.model().unwrap().document(),
        s.model().unwrap().document()
    );
    close(&s2, want(50.0, 14.0, 4.0, 4.0));
}

#[test]
fn refused_edits_and_commands_leave_the_document_unchanged() {
    let mut s = Session::new();
    rounded_box(&mut s, 40.0, 10.0, 3.0);
    tray::ok(
        &mut s,
        &json!({"cmd": "fillet_loop", "face": top(), "radius": 2.0}),
    );
    let before = s.model().unwrap().document().clone();
    // r > R: the corner torus does not exist (needs a corner blend).
    let r = set(&mut s, 7, "radius", 3.5);
    assert!(!r.ok);
    let e = r.error.unwrap().to_string();
    assert!(e.contains("larger than the convex arc radius"), "{e}");
    assert_eq!(s.model().unwrap().document(), &before);
    // Upstream R below r: refused the same way, nothing changes.
    assert!(!set(&mut s, 3, "radius", 1.0).ok);
    assert_eq!(s.model().unwrap().document(), &before);
    // The same loop again: the body under the top face is no longer a prism.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "fillet_loop", "face": top(), "radius": 1.0}),
    );
    assert!(!r.ok);
    assert!(
        r.error
            .unwrap()
            .to_string()
            .contains("not a straight prism")
    );
    assert_eq!(s.model().unwrap().document(), &before);
    // Non-positive radius.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "fillet_loop", "face": top(), "radius": 0.0}),
    );
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().document(), &before);
}

#[test]
fn downstream_features_on_the_top_face_keep_resolving() {
    // A Ø8 through hole sketched on the top face after the loop fillet: the
    // top cap reference resolves to the shrunken top face and the cut follows
    // a height edit.
    let mut s = Session::new();
    rounded_box(&mut s, 40.0, 10.0, 4.0);
    tray::ok(
        &mut s,
        &json!({"cmd": "fillet_loop", "face": top(), "radius": 2.0}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "create_sketch", "plane": {"Face": top()},
                "template": "centered_circle", "diameter": 8.0, "center": [20.0, 15.0]}),
    );
    tray::ok(&mut s, &json!({"cmd": "cut_through_all", "sketch": 8}));
    let hole = |d: f64| PI * 16.0 * d;
    close(&s, want(40.0, 10.0, 4.0, 2.0) - hole(10.0));
    assert!(set(&mut s, 2, "distance", 12.0).ok);
    close(&s, want(40.0, 12.0, 4.0, 2.0) - hole(12.0));
    assert_eq!(
        tray::probe(&s, &[[20.0, 15.0, 11.0], [20.0, 10.0, 11.0]]),
        vec![Some(false), Some(true)]
    );
    // A shell from the (now holed) top face is out of scope: refused with a
    // reason (not a wrong body), the document unchanged.
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "shell_face", "face": top(), "thickness": 1.5}),
    );
    assert!(!r.ok && r.error.is_some());
    assert_eq!(s.model().unwrap().document(), &before);
}

/// Volume of body 1 (exact method required).
fn exact_volume(s: &Session) -> f64 {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id == rcad_document::BodyId(1))
        .expect("body");
    match b.measurements.expect("measured").volume {
        rcad_kernel::VolumeResult::Value { volume, method, .. } => {
            assert_ne!(method, rcad_kernel::VolumeMethod::TessellationEstimate);
            volume
        }
        v => panic!("{v:?}"),
    }
}

#[test]
fn chamfer_loop_of_a_box_and_loops_of_a_hexagon_prism() {
    // Box 40 × 30 × 10, top loop chamfered d: the band is the prismatoid
    // between the rectangle at depth d and its d-inset at the top.
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 40.0, "height": 30.0, "center": [20.0, 15.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
    ] {
        tray::ok(&mut s, &v);
    }
    let band = |a: f64, b: f64, d: f64| {
        d / 6.0 * (a * b + 4.0 * (a - d) * (b - d) + (a - 2.0 * d) * (b - 2.0 * d))
    };
    let want = |d: f64| 1200.0 * (10.0 - d) + band(40.0, 30.0, d);
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "chamfer_loop", "face": top(), "distance": 2.0}),
    );
    let f = r.feature.unwrap().0;
    assert!((exact_volume(&s) - want(2.0)).abs() < 1e-9 * want(2.0));
    assert!(set(&mut s, f, "distance", 3.0).ok);
    assert!((exact_volume(&s) - want(3.0)).abs() < 1e-9 * want(3.0));
    // Save / reopen keeps it.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chamfer.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let mut s2 = Session::new();
    tray::ok(&mut s2, &json!({"cmd": "open", "path": path}));
    assert_eq!(
        s2.model().unwrap().document(),
        s.model().unwrap().document()
    );
    assert!((exact_volume(&s2) - want(3.0)).abs() < 1e-9 * want(3.0));

    // Regular hexagon, apothem 4, height 20 (120° corners, mitered).
    let hex: Vec<[f64; 2]> = (0..6)
        .map(|k| {
            let t = PI / 3.0 * k as f64;
            let c = 8.0 / 3f64.sqrt();
            [c * t.cos(), c * t.sin()]
        })
        .collect();
    let area = |ap: f64| 2.0 * 3f64.sqrt() * ap * ap;
    let new_hex = |s: &mut Session| {
        for v in [
            json!({"cmd": "new_document"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "polygon",
                   "vertices": hex}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 20.0}),
        ] {
            tray::ok(s, &v);
        }
    };
    let mut s = Session::new();
    new_hex(&mut s);
    tray::ok(
        &mut s,
        &json!({"cmd": "chamfer_loop", "face": top(), "distance": 0.5}),
    );
    let want = area(4.0) * 19.5 + 0.5 / 6.0 * (area(4.0) + 4.0 * area(3.75) + area(3.5));
    assert!((exact_volume(&s) - want).abs() < 1e-9 * want);
    // Then a Ø3 hole down the axis from the (smaller) top face.
    tray::ok(
        &mut s,
        &json!({"cmd": "create_sketch", "plane": {"Face": top()},
                "template": "centered_circle", "diameter": 3.0, "center": [0.0, 0.0]}),
    );
    tray::ok(&mut s, &json!({"cmd": "cut_through_all", "sketch": 4}));
    let want = want - PI * 2.25 * 20.0;
    assert!((exact_volume(&s) - want).abs() < 1e-9 * want);
    // Fillet loop r = 1 instead: each of the 12 corner ends loses M·tan 30°.
    let mut s = Session::new();
    new_hex(&mut s);
    tray::ok(
        &mut s,
        &json!({"cmd": "fillet_loop", "face": top(), "radius": 1.0}),
    );
    let (a, m) = (1.0 - PI / 4.0, 5.0 / 6.0 - PI / 4.0);
    let side = 8.0 / 3f64.sqrt();
    let want = area(4.0) * 20.0 - a * 6.0 * side + 12.0 * m * (PI / 6.0).tan();
    assert!((exact_volume(&s) - want).abs() < 1e-9 * want);
}

#[test]
fn single_edge_blends_meeting_at_a_corner_are_refused_by_name() {
    // Box 60 × 40 × 30: chamfer the top/front edge, then the adjacent
    // top/right edge on its own. It is refused with a reason that points at the
    // loop command (it used to surface as a kernel panic), and the document is unchanged.
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 60.0, "height": 40.0, "center": [30.0, 20.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 30.0}),
        json!({"cmd": "chamfer", "a": top(), "b": side(1), "distance": 2.0}),
    ] {
        tray::ok(&mut s, &v);
    }
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "chamfer", "a": top(), "b": side(2), "distance": 2.0}),
    );
    assert!(!r.ok);
    let e = r.error.unwrap().to_string();
    assert!(!e.contains("panicked"), "{e}");
    assert!(
        e.contains("chamfer_loop") || e.contains("within reach"),
        "{e}"
    );
    assert_eq!(s.model().unwrap().document(), &before);
    // Fillets: after the top/front edge, the adjacent edge is refused and
    // the reason names the loop command.
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 60.0, "height": 40.0, "center": [30.0, 20.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 30.0}),
        json!({"cmd": "fillet", "a": top(), "b": side(1), "radius": 2.0}),
    ] {
        tray::ok(&mut s, &v);
    }
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "fillet", "a": top(), "b": side(2), "radius": 2.0}),
    );
    assert!(!r.ok);
    let e = r.error.unwrap().to_string();
    assert!(e.contains("fillet_loop"), "{e}");
}

#[test]
fn shelled_enclosure_rim_and_base_loops() {
    // 40 × 30 × 20 box, vertical edges R 5, shell from the top face (walls
    // and floor 2: cavity 36 × 26 with corner radius 3, 18 deep); then the
    // outer rim loop and the base loop rounded r 1.5. The cavity is kept.
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 40.0, "height": 30.0, "center": [20.0, 15.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 20.0}),
    ] {
        tray::ok(&mut s, &v);
    }
    for (a, b) in [(1, 2), (2, 3), (3, 4), (4, 1)] {
        tray::ok(
            &mut s,
            &json!({"cmd": "fillet", "a": side(a), "b": side(b), "radius": 5.0}),
        );
    }
    tray::ok(
        &mut s,
        &json!({"cmd": "shell_face", "face": top(), "thickness": 2.0}),
    );
    let rr = |l: f64, w: f64, r: f64| l * w - (4.0 - PI) * r * r;
    let shell = |t: f64| {
        rr(40.0, 30.0, 5.0) * 20.0 - rr(40.0 - 2.0 * t, 30.0 - 2.0 * t, 5.0 - t) * (20.0 - t)
    };
    let removed = |r: f64| {
        let (a, m) = (r * r * (1.0 - PI / 4.0), r.powi(3) * (5.0 / 6.0 - PI / 4.0));
        a * (2.0 * 30.0 + 2.0 * 20.0) + 2.0 * PI * (a * 5.0 - m)
    };
    tray::ok(
        &mut s,
        &json!({"cmd": "fillet_loop", "face": top(), "radius": 1.5}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "fillet_loop", "face": {"feature": 2, "role": "ExtrudeStartCap"},
                "radius": 1.5}),
    );
    let want = shell(2.0) - 2.0 * removed(1.5);
    assert!(
        (exact_volume(&s) - want).abs() < 1e-9 * want,
        "{} vs {want}",
        exact_volume(&s)
    );
    assert_eq!(
        tray::probe(
            &s,
            &[[20.0, 15.0, 10.0], [20.0, 15.0, 1.0], [0.05, 15.0, 19.95]]
        ),
        vec![Some(false), Some(true), Some(false)]
    );
    // Upstream edit: thicker walls; both loops follow.
    assert!(set(&mut s, 7, "thickness", 2.5).ok);
    let want = shell(2.5) - 2.0 * removed(1.5);
    assert!(
        (exact_volume(&s) - want).abs() < 1e-9 * want,
        "{} vs {want}",
        exact_volume(&s)
    );
    // Thinner than the rim radius: refused (the cavity is a hole within
    // reach of the rim loop), document unchanged.
    let before = s.model().unwrap().document().clone();
    assert!(!set(&mut s, 7, "thickness", 1.2).ok);
    assert_eq!(s.model().unwrap().document(), &before);
}

#[test]
fn face_edges_with_holes_through_the_command_path() {
    // 40 × 30 × 10 plate with a centred Ø10 through hole; chamfer every edge
    // of the top face (d 1): the box loop and a countersink-like ring at the
    // hole, 2π (R + d/3) d²/2 (Pappus). Then switch it to a fillet r 1.5.
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 40.0, "height": 30.0, "center": [20.0, 15.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
        json!({"cmd": "create_sketch", "plane": {"Face": top()},
               "template": "centered_circle", "diameter": 10.0, "center": [20.0, 15.0]}),
        json!({"cmd": "cut_through_all", "sketch": 3}),
    ] {
        tray::ok(&mut s, &v);
    }
    let plate = 12000.0 - PI * 25.0 * 10.0;
    tray::ok(
        &mut s,
        &json!({"cmd": "chamfer_loop", "face": top(), "distance": 1.0, "holes": true}),
    );
    let want = plate - (0.5 * 140.0 - 8.0 / 6.0) - 2.0 * PI * (5.0 + 1.0 / 3.0) * 0.5;
    assert!(
        (exact_volume(&s) - want).abs() < 1e-9 * want,
        "{} vs {want}",
        exact_volume(&s)
    );
    // Saved with the option; reopened identically.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("holes.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"holes\": true") || text.contains("\"holes\":true"));
    let mut s2 = Session::new();
    tray::ok(&mut s2, &json!({"cmd": "open", "path": path}));
    assert_eq!(
        s2.model().unwrap().document(),
        s.model().unwrap().document()
    );
    // Upstream: a larger hole (Ø12); the ring follows.
    assert!(set(&mut s, 3, "diameter", 12.0).ok);
    let plate = 12000.0 - PI * 36.0 * 10.0;
    let want = plate - (0.5 * 140.0 - 8.0 / 6.0) - 2.0 * PI * (6.0 + 1.0 / 3.0) * 0.5;
    assert!(
        (exact_volume(&s) - want).abs() < 1e-9 * want,
        "{} vs {want}",
        exact_volume(&s)
    );
}
