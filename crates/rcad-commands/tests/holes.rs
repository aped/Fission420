//! SESSION_4 §7.6: Hole feature with counterbore (through / blind), refusals,
//! patterns and parameter bindings. Expected volumes are cylinder arithmetic
//! (the solids are Z-ruled, so the measured volume is the exact boundary
//! integral).
use rcad_commands::{Command, ExtrudeOp, Response, Session, SketchTemplate};
use rcad_document::{DocError, FaceRef, FaceRole, FeatureId, SketchPlane};
use rcad_kernel::VolumeResult;
use std::f64::consts::PI;

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn vol(s: &Session) -> f64 {
    match s.summary().bodies[0].measurements.clone().unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

/// 40 x 30 x 10 plate on x [0, 40], y [0, 30]; returns the extrusion id.
fn plate(s: &mut Session) -> FeatureId {
    ok(s, Command::NewDocument);
    let sk = ok(
        s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 40.0,
                height: 30.0,
                center: Some([20.0, 15.0]),
            },
        },
    )
    .feature
    .unwrap();
    ok(s, Command::extrude(None, sk, 10.0)).feature.unwrap()
}

fn top(e: FeatureId) -> SketchPlane {
    SketchPlane::Face(FaceRef {
        feature: e,
        role: FaceRole::ExtrudeEndCap,
    })
}

fn hole(e: FeatureId, c: [f64; 2], depth: Option<f64>, cb: Option<[f64; 2]>) -> Command {
    Command::Hole {
        name: None,
        plane: top(e),
        center: c,
        diameter: 5.5,
        depth,
        counterbore: cb,
        target: None,
    }
}

/// Removed volume of a Ø5.5 hole `h` deep with a Ø10 x `cd` counterbore.
fn removed(h: f64, cd: f64) -> f64 {
    PI * 2.75 * 2.75 * h + PI * (25.0 - 2.75 * 2.75) * cd
}

#[test]
fn through_and_blind_counterbored_holes_are_exact() {
    let mut s = Session::new();
    let e = plate(&mut s);
    ok(&mut s, hole(e, [20.0, 15.0], None, Some([10.0, 5.0])));
    assert!(
        (vol(&s) - (12000.0 - removed(10.0, 5.0))).abs() < 1e-6,
        "{}",
        vol(&s)
    );
    // Probes: counterbore and hole are open, material between them stays.
    let m = s.model().unwrap();
    let b = m.evaluation().bodies[0].geometry.brep().unwrap();
    let p = rcad_kernel::point_membership(
        b,
        &[
            [20.0, 15.0, 7.0], // counterbore
            [20.0, 15.0, 2.0], // hole
            [24.0, 15.0, 2.0], // below the counterbore, outside the hole
            [24.0, 15.0, 7.0], // inside the counterbore radius
            [26.0, 15.0, 7.0], // outside the counterbore
        ],
    )
    .unwrap();
    assert_eq!(
        p,
        vec![
            Some(false),
            Some(false),
            Some(true),
            Some(false),
            Some(true)
        ]
    );
    // Blind hole with a counterbore.
    let mut s = Session::new();
    let e = plate(&mut s);
    ok(&mut s, hole(e, [20.0, 15.0], Some(8.0), Some([10.0, 3.0])));
    assert!(
        (vol(&s) - (12000.0 - removed(8.0, 3.0))).abs() < 1e-6,
        "{}",
        vol(&s)
    );
    // Plain through hole.
    let mut s = Session::new();
    let e = plate(&mut s);
    ok(&mut s, hole(e, [10.0, 10.0], None, None));
    assert!((vol(&s) - (12000.0 - removed(10.0, 0.0))).abs() < 1e-6);
}

#[test]
fn invalid_holes_are_refused_unchanged() {
    let mut s = Session::new();
    let e = plate(&mut s);
    let rev = s.summary().revision;
    for c in [
        hole(e, [20.0, 15.0], None, Some([5.0, 3.0])), // counterbore not wider
        hole(e, [20.0, 15.0], Some(3.0), Some([10.0, 3.0])), // not shallower
        hole(e, [20.0, 15.0], Some(-1.0), None),
    ] {
        let r = s.execute(&c);
        assert!(!r.ok, "{c:?}");
        assert!(
            matches!(r.error, Some(DocError::Validation(_))),
            "{:?}",
            r.error
        );
        assert_eq!(s.summary().revision, rev);
    }
}

#[test]
fn patterned_counterbored_holes_and_parameters() {
    let mut s = Session::new();
    let e = plate(&mut s);
    let h = ok(&mut s, hole(e, [10.0, 15.0], None, Some([10.0, 4.0])))
        .feature
        .unwrap();
    // Spacing 10: the Ø10 counterbores touch tangentially; Truck's result is
    // wrong there and the result check refuses it (document unchanged).
    let rev = s.summary().revision;
    let r = s.execute(&Command::LinearPattern {
        name: None,
        feature: h,
        count: 3,
        spacing: 10.0,
        direction: [1.0, 0.0],
    });
    assert!(
        matches!(r.error, Some(DocError::GeometryCheck { .. })),
        "{:?}",
        r.error
    );
    assert_eq!(s.summary().revision, rev);
    // Spacing 12: separate counterbores, exact.
    ok(
        &mut s,
        Command::LinearPattern {
            name: None,
            feature: h,
            count: 3,
            spacing: 12.0,
            direction: [1.0, 0.0],
        },
    );
    assert!(
        (vol(&s) - (12000.0 - 3.0 * removed(10.0, 4.0))).abs() < 1e-6,
        "{}",
        vol(&s)
    );
    // Bind the hole sizes to parameters; the pattern follows.
    for (n, v) in [("screw_clear", "4.5"), ("head_d", "8"), ("head_h", "3")] {
        ok(
            &mut s,
            Command::SetUserParameter {
                name: n.into(),
                expression: v.into(),
                unit: None,
                comment: None,
            },
        );
    }
    for (slot, e) in [
        ("diameter", "screw_clear"),
        ("cbore_diameter", "head_d"),
        ("cbore_depth", "head_h"),
    ] {
        ok(
            &mut s,
            Command::SetExpression {
                feature: h,
                name: slot.into(),
                expression: e.into(),
            },
        );
    }
    let one = PI * 2.25 * 2.25 * 10.0 + PI * (16.0 - 2.25 * 2.25) * 3.0;
    assert!(
        (vol(&s) - (12000.0 - 3.0 * one)).abs() < 1e-6,
        "{}",
        vol(&s)
    );
    // Mirror of the hole across a face-free world plane is refused only if
    // its copy misses; across YZ it lands outside the plate (x < 0): a cut
    // that removes nothing is refused, the document unchanged.
    let rev = s.summary().revision;
    let r = s.execute(&Command::Mirror {
        name: None,
        feature: h,
        plane: SketchPlane::WorldYZ,
    });
    assert!(!r.ok);
    assert_eq!(s.summary().revision, rev);
    let _ = ExtrudeOp::Cut;
}

/// Review run 4 #3: "through all" goes into the material only. A hole on the
/// lower jaw of a C-channel must not drill the upper jaw above the face.
#[test]
fn through_hole_does_not_drill_material_behind_the_face() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    // C profile in XZ (x right, z up), extruded 30 along -Y.
    let c = ok(
        &mut s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXZ,
            template: SketchTemplate::Polygon {
                vertices: vec![
                    [0.0, 0.0],
                    [40.0, 0.0],
                    [40.0, 5.0],
                    [5.0, 5.0],
                    [5.0, 15.0],
                    [40.0, 15.0],
                    [40.0, 20.0],
                    [0.0, 20.0],
                ],
            },
        },
    )
    .feature
    .unwrap();
    let e = ok(&mut s, Command::extrude(None, c, 30.0)).feature.unwrap();
    let v0 = vol(&s);
    // The lower jaw's top face: outward +Z at z = 5.
    let f = s
        .summary()
        .features
        .into_iter()
        .find(|f| f.id == e)
        .unwrap()
        .faces
        .into_iter()
        .find(|f| f.frame.z[2] > 0.99 && (f.frame.origin[2] - 5.0).abs() < 1e-9)
        .expect("lower jaw top face");
    // Hole centre at world (20, -15): local x along the face frame's x axis.
    let d = [20.0 - f.frame.origin[0], -15.0 - f.frame.origin[1], 0.0];
    let dot = |a: [f64; 3]| a[0] * d[0] + a[1] * d[1] + a[2] * d[2];
    ok(
        &mut s,
        Command::Hole {
            name: None,
            plane: SketchPlane::Face(f.reference),
            center: [dot(f.frame.x), dot(f.frame.y)],
            diameter: 6.0,
            depth: None,
            counterbore: None,
            target: None,
        },
    );
    assert!(
        (v0 - vol(&s) - PI * 9.0 * 5.0).abs() < 1e-6,
        "{}",
        v0 - vol(&s)
    );
    let m = s.model().unwrap();
    let b = m.evaluation().bodies[0].geometry.brep().unwrap();
    let p = rcad_kernel::point_membership(b, &[[20.0, -15.0, 17.5], [20.0, -15.0, 2.5]]).unwrap();
    assert_eq!(
        p,
        vec![Some(true), Some(false)],
        "upper jaw intact, lower drilled"
    );
}

/// Review run 4 #4: a hole's depth and counterbore depth bound to one
/// parameter change together; the final combination is what is validated.
#[test]
fn coupled_hole_depths_change_together() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let sk = ok(
        &mut s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 40.0,
                height: 30.0,
                center: Some([20.0, 15.0]),
            },
        },
    )
    .feature
    .unwrap();
    let e = ok(&mut s, Command::extrude(None, sk, 40.0))
        .feature
        .unwrap();
    ok(
        &mut s,
        Command::SetUserParameter {
            name: "h".into(),
            expression: "9".into(),
            unit: None,
            comment: None,
        },
    );
    let h = ok(&mut s, hole(e, [20.0, 15.0], Some(9.0), Some([10.0, 3.0])))
        .feature
        .unwrap();
    for (slot, x) in [("depth", "h"), ("cbore_depth", "h / 3")] {
        ok(
            &mut s,
            Command::SetExpression {
                feature: h,
                name: slot.into(),
                expression: x.into(),
            },
        );
    }
    ok(
        &mut s,
        Command::SetUserParameter {
            name: "h".into(),
            expression: "30".into(),
            unit: None,
            comment: None,
        },
    );
    let want = 40.0 * 30.0 * 40.0 - PI * 2.75 * 2.75 * 30.0 - PI * (25.0 - 2.75 * 2.75) * 10.0;
    assert!((vol(&s) - want).abs() < 1e-6, "{} vs {want}", vol(&s));
}
