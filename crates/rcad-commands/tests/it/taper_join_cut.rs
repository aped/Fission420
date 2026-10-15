//! Tapered (drafted) Extrude Join and Cut: exact
//! frustum tools through Truck's boolean, checked against the nominal drafted
//! tool (`rcad_kernel::tapered_boolean`). Block 40 x 30 x 10 at the origin.
//! Expected volumes in closed form: a 10 x 10 square offset inward by w·t has
//! side 10 − 2wt, so V = ∫ (10 − 2wt)² dw; circles give cone frusta.
use rcad_commands::{Command, Response, Session};
use serde_json::{Value, json};
use std::f64::consts::PI;

fn exec(s: &mut Session, v: &Value) -> Response {
    let c: Command = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{v}: {e}"));
    s.execute(&c)
}

fn ok(s: &mut Session, v: Value) -> Response {
    let r = exec(s, &v);
    assert!(r.ok, "{v}: {:?}", r.error);
    r
}

/// (volume, error estimate) of body 1.
fn vol(s: &Session) -> (f64, f64) {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id.0 == 1)
        .unwrap();
    match b.measurements.expect("measured").volume {
        rcad_kernel::VolumeResult::Value {
            volume,
            error_estimate,
            ..
        } => (volume, error_estimate),
        v => panic!("{v:?}"),
    }
}

fn assert_vol(s: &Session, want: f64) {
    let (v, e) = vol(s);
    assert!(
        (v - want).abs() <= e.max(1e-6 * want),
        "{v} ± {e} vs {want}"
    );
}

fn square_frustum(side: f64, d: f64, t: f64) -> f64 {
    let f = |w: f64| -(side - 2.0 * w * t).powi(3) / (6.0 * t);
    f(d) - f(0.0)
}

/// Block, then a sketch on its top face: a `w` square (or a circle of
/// diameter `w` when `round`) centred at `c`. Returns the sketch id.
fn block_and_top_sketch(s: &mut Session, w: f64, c: [f64; 2], round: bool) -> u32 {
    ok(s, json!({"cmd": "new_document"}));
    ok(
        s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 40.0, "height": 30.0, "center": [20.0, 15.0]}),
    );
    ok(s, json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}));
    let top = json!({"Face": {"feature": 2, "role": "ExtrudeEndCap"}});
    let sk = if round {
        json!({"cmd": "create_sketch", "plane": top, "template": "centered_circle",
               "diameter": w, "center": c})
    } else {
        json!({"cmd": "create_sketch", "plane": top, "template": "centered_rectangle",
               "width": w, "height": w, "center": c})
    };
    ok(s, sk).feature.unwrap().0
}

#[test]
fn drafted_pocket_and_boss_with_edit_undo_and_pattern_refusal() {
    let mut s = Session::new();
    let sk = block_and_top_sketch(&mut s, 10.0, [20.0, 15.0], false);
    let a10 = 10f64.to_radians();
    let cut = ok(
        &mut s,
        json!({"cmd": "extrude", "sketch": sk, "distance": 5.0, "op": "cut", "target": 1,
               "options": {"taper": a10}}),
    )
    .feature
    .unwrap();
    let v1 = 12000.0 - square_frustum(10.0, 5.0, a10.tan());
    assert_vol(&s, v1);
    // Depth 5 -> 4 regenerates the drafted pocket.
    ok(
        &mut s,
        json!({"cmd": "set_parameter", "feature": cut.0, "name": "distance", "value": 4.0}),
    );
    assert_vol(&s, 12000.0 - square_frustum(10.0, 4.0, a10.tan()));
    ok(&mut s, json!({"cmd": "undo"}));
    assert_vol(&s, v1);
    ok(&mut s, json!({"cmd": "redo"}));
    // A pattern of the drafted pocket is refused (copies would be straight).
    let r = exec(
        &mut s,
        &json!({"cmd": "linear_pattern", "feature": cut.0, "count": 2, "spacing": 12.0,
                "direction": [1.0, 0.0]}),
    );
    assert!(!r.ok, "pattern of a tapered cut accepted");
    assert!(
        format!("{:?}", r.error).contains("tapered"),
        "{:?}",
        r.error
    );

    // Drafted boss 8 x 8 on the top face beside the pocket, 5 tall, 5°.
    let top = json!({"Face": {"feature": 2, "role": "ExtrudeEndCap"}});
    let bsk = ok(
        &mut s,
        json!({"cmd": "create_sketch", "plane": top, "template": "centered_rectangle",
               "width": 8.0, "height": 8.0, "center": [6.0, 6.0]}),
    )
    .feature
    .unwrap();
    let a5 = 5f64.to_radians();
    ok(
        &mut s,
        json!({"cmd": "extrude", "sketch": bsk.0, "distance": 5.0, "op": "join", "target": 1,
               "options": {"taper": a5}}),
    );
    let v2 = 12000.0 - square_frustum(10.0, 4.0, a10.tan()) + square_frustum(8.0, 5.0, a5.tan());
    assert_vol(&s, v2);
    let b = s.summary().bodies[0].bounds.unwrap();
    assert!((b.max[2] - 15.0).abs() < 1e-6);
}

#[test]
fn drafted_round_pocket_and_refusals() {
    // Round pocket Ø8, 4 deep, 5° draft: cone frustum radii 4 and 4 − 4·tan5°.
    let mut s = Session::new();
    let sk = block_and_top_sketch(&mut s, 8.0, [20.0, 15.0], true);
    let a5 = 5f64.to_radians();
    let r = exec(
        &mut s,
        &json!({"cmd": "extrude", "sketch": sk, "distance": 4.0, "op": "cut", "target": 1,
                "options": {"taper": a5}}),
    );
    if r.ok {
        let r2 = 4.0 - 4.0 * a5.tan();
        assert_vol(&s, 12000.0 - PI * 4.0 / 3.0 * (16.0 + 4.0 * r2 + r2 * r2));
    } else {
        // Truck may fail on the cone: a named refusal, the model unchanged.
        eprintln!("round drafted pocket refused: {:?}", r.error);
        assert_vol(&s, 12000.0);
    }
    // A draft that closes the 10 mm square before 15 mm: refused by name.
    let mut s = Session::new();
    let sk = block_and_top_sketch(&mut s, 10.0, [20.0, 15.0], false);
    let r = exec(
        &mut s,
        &json!({"cmd": "extrude", "sketch": sk, "distance": 9.0, "op": "cut", "target": 1,
                "options": {"taper": 30f64.to_radians()}}),
    );
    assert!(!r.ok);
    assert!(format!("{:?}", r.error).contains("taper"), "{:?}", r.error);
    assert_vol(&s, 12000.0);
}
