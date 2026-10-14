//! Cap-loop fillet through the command API (`fillet_loop`,
//! `FeatureKind::FilletLoop` over `rcad_kernel::fillet_cap_loop`): the
//! common print workflow of a box whose vertical edges are rounded R and whose
//! top edge loop is then rounded r. Expected volumes come from the
//! dimensions alone: the removed spandrel has area A = r²(1 − π/4) and first
//! moment M = r³(5/6 − π/4) about a wall; along the straight sides it removes
//! A per unit length, around each quarter-turn corner of radius R it removes
//! (π/2)(A·R − M) (Pappus). The measured volume of torus/sphere faces is a
//! tessellation estimate; it must agree within its own reported error bound
//! (and that bound must be small).
mod support;
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
    assert!(error_estimate <= 1e-3 * v, "error bound {error_estimate}");
    assert!(
        (volume - v).abs() <= 3.0 * error_estimate + 1e-9 * v,
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
