//! Geometry gate, through the public command path: every known
//! wrong-geometry case either yields the exact intended solid or a specific
//! refusal that leaves the document unchanged, and each refusal has an adjacent
//! control that still works. Expected volumes are box/cylinder arithmetic.
use rcad_commands::{Command, ExtrudeOp, Response, Session, SketchTemplate};
use rcad_document::{BodyId, FaceRef, FaceRole, FeatureId, SketchPlane};
use rcad_kernel::VolumeResult;
use std::f64::consts::PI;

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn sketch(s: &mut Session, plane: SketchPlane, t: SketchTemplate) -> FeatureId {
    ok(
        s,
        Command::CreateSketch {
            name: None,
            plane,
            template: t,
        },
    )
    .feature
    .unwrap()
}

fn rect(w: f64, h: f64, c: [f64; 2]) -> SketchTemplate {
    SketchTemplate::CenteredRectangle {
        width: w,
        height: h,
        center: Some(c),
    }
}

fn circle(d: f64, c: [f64; 2]) -> SketchTemplate {
    SketchTemplate::CenteredCircle {
        diameter: d,
        center: Some(c),
    }
}

fn extrude(sketch: FeatureId, d: f64, op: ExtrudeOp, through_all: bool) -> Command {
    Command::Extrude {
        name: None,
        sketch,
        distance: d,
        op,
        target: Some(BodyId(1)),
        through_all,
        reverse: false,
        body_name: None,
        region: None,
        options: Default::default(),
    }
}

fn face(feature: FeatureId, role: FaceRole) -> SketchPlane {
    SketchPlane::Face(FaceRef { feature, role })
}

fn volume(s: &Session) -> f64 {
    let b = s.summary().bodies.into_iter().next().unwrap();
    match b.measurements.unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

// ------------------------------------------------- engulfed (swallowed) hole

/// 80x50x10 plate with a Ø10 through hole at (20, 25).
fn holed_plate(s: &mut Session) -> FeatureId {
    ok(s, Command::NewDocument);
    let sk = sketch(s, SketchPlane::WorldXY, rect(80.0, 50.0, [40.0, 25.0]));
    let e = ok(s, Command::extrude(None, sk, 10.0)).feature.unwrap();
    let h = sketch(
        s,
        face(e, FaceRole::ExtrudeEndCap),
        circle(10.0, [20.0, 25.0]),
    );
    ok(s, extrude(h, 0.0, ExtrudeOp::Cut, true));
    assert!((volume(s) - (40000.0 - PI * 25.0 * 10.0)).abs() < 1e-3);
    e
}

/// Truck 0.6 keeps the engulfed hole's walls (2010.6 removed instead of
/// 1225.2 with a "closed" shell); the result check refuses that. The exact
/// slab construction with arcs (ADR 0007) builds the cut instead, through
/// Extrude Cut through-all and blind alike, and the same result check passes
/// it. The legacy "Cut Through All" command (`legacy_cut`) has the same slab
/// fallback and is exact too.
#[test]
fn cut_swallowing_an_existing_hole_is_exact() {
    let want = 40000.0 - PI * 64.0 * 10.0;
    for (d, through) in [(0.0, true), (10.0, false)] {
        let mut s = Session::new();
        let e = holed_plate(&mut s);
        let big = sketch(
            &mut s,
            face(e, FaceRole::ExtrudeEndCap),
            circle(16.0, [20.0, 25.0]),
        );
        ok(&mut s, extrude(big, d, ExtrudeOp::Cut, through));
        assert!((volume(&s) - want).abs() < 1e-6 * want, "{}", volume(&s));
    }
    let mut s = Session::new();
    let e = holed_plate(&mut s);
    let big = sketch(
        &mut s,
        face(e, FaceRole::ExtrudeEndCap),
        circle(16.0, [20.0, 25.0]),
    );
    ok(
        &mut s,
        Command::CutThroughAll {
            name: None,
            sketch: big,
        },
    );
    assert!((volume(&s) - want).abs() < 1e-6 * want, "{}", volume(&s));
}

/// Control: the same Ø16 cut elsewhere on the plate is exact.
#[test]
fn control_cut_beside_an_existing_hole_is_exact() {
    let mut s = Session::new();
    let e = holed_plate(&mut s);
    let big = sketch(
        &mut s,
        face(e, FaceRole::ExtrudeEndCap),
        circle(16.0, [55.0, 25.0]),
    );
    ok(&mut s, extrude(big, 0.0, ExtrudeOp::Cut, true));
    let want = 40000.0 - PI * 25.0 * 10.0 - PI * 64.0 * 10.0;
    assert!((volume(&s) - want).abs() < 1e-3, "{}", volume(&s));
}

/// Control: enlarging the hole by editing its diameter is exact (the
/// recommended route instead of engulfing it).
#[test]
fn control_enlarging_the_hole_by_its_dimension_is_exact() {
    let mut s = Session::new();
    holed_plate(&mut s);
    let hole_sketch = s.summary().features[2].id;
    ok(
        &mut s,
        Command::SetParameter {
            feature: hole_sketch,
            name: "diameter".into(),
            value: 16.0,
        },
    );
    let want = 40000.0 - PI * 64.0 * 10.0;
    assert!((volume(&s) - want).abs() < 1e-3, "{}", volume(&s));
}

// ------------------------------------------------- joins over thin material

/// 40x10x10 block on x [-20, 20]; a 19x9 pocket from below over x [0, 19],
/// y [-4.5, 4.5] leaves a 0.3 mm skin on top there. 4000 - 19*9*9.7 = 2341.3.
/// (Inset from the walls: a pocket flush with the outer walls makes coplanar
/// boolean faces, which Truck 0.6 fails on; that is a refusal, not this test.)
const BASE: f64 = 4000.0 - 19.0 * 9.0 * 9.7;

fn flanged_block(s: &mut Session) -> FeatureId {
    ok(s, Command::NewDocument);
    let sk = sketch(s, SketchPlane::WorldXY, rect(40.0, 10.0, [0.0, 0.0]));
    let e = ok(s, Command::extrude(None, sk, 10.0)).feature.unwrap();
    // Start cap frame: x = world x, y = -world y, z = -Z.
    let p = sketch(
        s,
        face(e, FaceRole::ExtrudeStartCap),
        rect(19.0, 9.0, [9.5, 0.0]),
    );
    ok(s, extrude(p, 9.7, ExtrudeOp::Cut, false));
    assert!((volume(s) - BASE).abs() < 1e-6, "{}", volume(s));
    e
}

fn boss_on_top(s: &mut Session, e: FeatureId, x: f64) -> FeatureId {
    let b = sketch(
        s,
        face(e, FaceRole::ExtrudeEndCap),
        rect(4.0, 4.0, [x, 0.0]),
    );
    ok(s, extrude(b, 5.0, ExtrudeOp::Join, false))
        .feature
        .unwrap()
}

fn bounds_z(s: &Session) -> (f64, f64) {
    let b = s.summary().bodies.into_iter().next().unwrap();
    let m = b.measurements.unwrap();
    (m.bounds.min[2], m.bounds.max[2])
}

/// Thin-flange leak (repro 2161.6 vs 2160): the bounds do not change, so a
/// bounds check alone misses it. Exact: the overlap is sized from the 0.3 mm
/// of clear material.
#[test]
fn join_over_thin_flange_is_exact() {
    let mut s = Session::new();
    let e = flanged_block(&mut s);
    boss_on_top(&mut s, e, 10.0);
    assert!((volume(&s) - (BASE + 80.0)).abs() < 1e-6, "{}", volume(&s));
    assert_eq!(bounds_z(&s), (0.0, 15.0));
}

/// Control: the same boss over thick material.
#[test]
fn control_join_over_thick_material_is_exact() {
    let mut s = Session::new();
    let e = flanged_block(&mut s);
    boss_on_top(&mut s, e, -10.0);
    assert!((volume(&s) - (BASE + 80.0)).abs() < 1e-6, "{}", volume(&s));
}

/// Pattern copies re-derive the overlap per placement: the seed sits over
/// thick material, copy 2 straddles the step, copy 3 is over the flange.
#[test]
fn linear_pattern_join_over_flange_is_exact() {
    let mut s = Session::new();
    let e = flanged_block(&mut s);
    let seed = boss_on_top(&mut s, e, -10.0);
    ok(
        &mut s,
        Command::LinearPattern {
            name: None,
            feature: seed,
            count: 3,
            spacing: 10.0,
            direction: [1.0, 0.0],
            second: None,
        },
    );
    assert!(
        (volume(&s) - (BASE + 3.0 * 80.0)).abs() < 1e-6,
        "{}",
        volume(&s)
    );
    assert_eq!(bounds_z(&s), (0.0, 15.0));
}

#[test]
fn mirror_join_over_flange_is_exact() {
    let mut s = Session::new();
    let e = flanged_block(&mut s);
    let seed = boss_on_top(&mut s, e, -10.0);
    ok(
        &mut s,
        Command::Mirror {
            name: None,
            feature: seed,
            plane: SketchPlane::WorldYZ,
        },
    );
    assert!(
        (volume(&s) - (BASE + 2.0 * 80.0)).abs() < 1e-6,
        "{}",
        volume(&s)
    );
}

#[test]
fn circular_pattern_join_over_flange_is_exact() {
    let mut s = Session::new();
    let e = flanged_block(&mut s);
    let seed = boss_on_top(&mut s, e, -10.0);
    ok(
        &mut s,
        Command::CircularPattern {
            name: None,
            feature: seed,
            count: 2,
            angle_deg: 180.0,
            center: [0.0, 0.0],
        },
    );
    assert!(
        (volume(&s) - (BASE + 2.0 * 80.0)).abs() < 1e-6,
        "{}",
        volume(&s)
    );
}

/// A skin over a cavity, joined from the top (second regeneration case):
/// 20x20x10 - 10x10x9.7 + 4x4x5 = 3110, exact.
#[test]
fn join_over_skin_above_a_cavity_is_exact() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let sk = sketch(&mut s, SketchPlane::WorldXY, rect(20.0, 20.0, [0.0, 0.0]));
    let e = ok(&mut s, Command::extrude(None, sk, 10.0))
        .feature
        .unwrap();
    let p = sketch(
        &mut s,
        face(e, FaceRole::ExtrudeStartCap),
        rect(10.0, 10.0, [0.0, 0.0]),
    );
    ok(&mut s, extrude(p, 9.7, ExtrudeOp::Cut, false));
    boss_on_top(&mut s, e, 0.0);
    assert!((volume(&s) - 3110.0).abs() < 1e-6, "{}", volume(&s));
}
