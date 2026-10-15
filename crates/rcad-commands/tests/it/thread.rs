//! Modeled threads through the command API (ADR 0011): Ø20 x 2 rod and
//! mating square nut as new bodies, faceted with a declared chord tolerance.
//! Reference volumes: the kernel test checks the rod against the ideal thread
//! integral, and FreeCAD read the same rod as 3215.09586 mm³ and the nut as
//! 7515.00173 mm³ with an empty rod ∩ nut.
use rcad_commands::{Command, Response, Session};
use serde_json::{Value, json};

fn exec(s: &mut Session, v: Value) -> Response {
    let c: Command = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{v}: {e}"));
    s.execute(&c)
}

fn ok(s: &mut Session, v: Value) -> Response {
    let r = exec(s, v.clone());
    assert!(r.ok, "{v}: {:?}", r.error);
    r
}

fn vol(s: &Session, body: u32) -> f64 {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id.0 == body)
        .unwrap();
    match b.measurements.expect("measured").volume {
        rcad_kernel::VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

#[test]
fn rod_and_nut_bodies_with_parameters_and_reopen() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let rod = ok(
        &mut s,
        json!({"cmd": "thread", "plane": "WorldXY", "diameter": 20.0, "pitch": 2.0, "length": 12.0}),
    )
    .feature
    .unwrap();
    assert!((vol(&s, 1) - 3215.09586).abs() < 1e-3, "{}", vol(&s, 1));
    let f = s
        .summary()
        .features
        .into_iter()
        .find(|f| f.id == rod)
        .unwrap();
    assert_eq!(f.kind, "thread");
    assert!(
        f.notes.iter().any(|n| n.contains("faceted")),
        "{:?}",
        f.notes
    );
    ok(
        &mut s,
        json!({"cmd": "thread", "plane": "WorldXY", "diameter": 20.0, "pitch": 2.0,
               "length": 12.0, "nut_width": 30.0}),
    );
    assert!((vol(&s, 2) - 7515.00173).abs() < 1e-3, "{}", vol(&s, 2));
    // Shorter rod: two turns fewer of full profile (the lead-in/runout stay).
    ok(
        &mut s,
        json!({"cmd": "set_parameter", "feature": rod.0, "name": "length", "value": 8.0}),
    );
    let v8 = vol(&s, 1);
    assert!(
        v8 < 3215.09586 && v8 > 3215.09586 * 8.0 / 12.0 - 100.0,
        "{v8}"
    );
    // Not a whole number of pitches: refused, unchanged.
    let r = exec(
        &mut s,
        json!({"cmd": "set_parameter", "feature": rod.0, "name": "length", "value": 9.0}),
    );
    assert!(
        !r.ok && format!("{:?}", r.error).contains("whole number"),
        "{:?}",
        r.error
    );
    assert!((vol(&s, 1) - v8).abs() < 1e-9);
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("thread");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("thread.rcad.json");
    ok(&mut s, json!({"cmd": "save", "path": path}));
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"version\": 6")
    );
    let mut s2 = Session::new();
    ok(&mut s2, json!({"cmd": "open", "path": path}));
    assert!((vol(&s2, 1) - v8).abs() < 1e-9);
    assert!((vol(&s2, 2) - 7515.00173).abs() < 1e-3);
}

#[test]
fn left_hand_and_refusals() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    ok(
        &mut s,
        json!({"cmd": "thread", "plane": "WorldXY", "diameter": 20.0, "pitch": 2.0,
               "length": 12.0, "left_hand": true}),
    );
    assert!((vol(&s, 1) - 3215.09586).abs() < 1e-3);
    for bad in [
        json!({"cmd": "thread", "plane": "WorldXY", "diameter": 20.0, "pitch": 2.0, "length": 11.0}),
        json!({"cmd": "thread", "plane": "WorldXY", "diameter": 3.0, "pitch": 2.0, "length": 12.0}),
        json!({"cmd": "thread", "plane": "WorldXY", "diameter": 20.0, "pitch": 2.0,
               "length": 12.0, "nut_width": 21.0}),
    ] {
        let r = exec(&mut s, bad.clone());
        assert!(!r.ok, "{bad}");
    }
    assert_eq!(s.summary().bodies.len(), 1);
}

/// Changing the pitch regenerates the thread (finer
/// pitch: shallower profile, so more material, still between the core and
/// major cylinders); an impossible pitch for the diameter is refused.
#[test]
fn pitch_change_and_impossible_profile() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let rod = ok(
        &mut s,
        json!({"cmd": "thread", "plane": "WorldXY", "diameter": 20.0, "pitch": 2.0, "length": 12.0}),
    )
    .feature
    .unwrap();
    let v2 = vol(&s, 1);
    ok(
        &mut s,
        json!({"cmd": "set_parameter", "feature": rod.0, "name": "pitch", "value": 1.5}),
    );
    let v15 = vol(&s, 1);
    let depth = 5.0 / 8.0 * 1.5 * 3f64.sqrt() / 2.0;
    let core = std::f64::consts::PI * (10.0 - depth).powi(2) * 12.0;
    let major = std::f64::consts::PI * 100.0 * 12.0;
    assert!(
        v15 > v2 && v15 > core && v15 < major,
        "{v2} {v15} {core} {major}"
    );
    // A pitch whose profile depth reaches the axis: refused, unchanged.
    let r = exec(
        &mut s,
        json!({"cmd": "set_parameter", "feature": rod.0, "name": "pitch", "value": 12.0}),
    );
    assert!(!r.ok, "pitch 12 on Ø20 accepted");
    assert!((vol(&s, 1) - v15).abs() < 1e-9);
}
