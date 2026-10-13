//! Offset construction planes (SESSION_4 §7 item 6): a sketch on a world plane
//! or a face moved along its normal, with a bindable "plane_offset" parameter;
//! deterministic regeneration, BrokenReference for a missing base face,
//! save/reopen, JSON shape. Bounds are analytic.
use rcad_commands::{Command, ExtrudeOp, Response, Session, SketchTemplate};
use rcad_document::{
    BodyId, DocError, FaceRef, FaceRole, FeatureId, ParamUnit, PlaneBase, SketchPlane,
};

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn rect(w: f64, h: f64) -> SketchTemplate {
    SketchTemplate::CenteredRectangle {
        width: w,
        height: h,
        center: None,
    }
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

fn z_range(s: &Session, body: u32) -> (f64, f64) {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id == BodyId(body))
        .unwrap();
    let m = b.measurements.unwrap();
    (m.bounds.min[2], m.bounds.max[2])
}

fn near(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
}

fn set(s: &mut Session, feature: FeatureId, name: &str, value: f64) {
    ok(
        s,
        Command::SetParameter {
            feature,
            name: name.into(),
            value,
        },
    );
}

#[test]
fn world_offset_plane_and_its_parameter() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let plane = SketchPlane::Offset {
        base: PlaneBase::WorldXY,
        distance: 7.0,
        angle: 0.0,
    };
    let sk = sketch(&mut s, plane, rect(20.0, 10.0));
    ok(&mut s, Command::extrude(None, sk, 3.0));
    assert!(near(z_range(&s, 1), (7.0, 10.0)), "{:?}", z_range(&s, 1));
    let f = s.summary().features[0].clone();
    assert!(f.parameters.contains(&("plane_offset".to_string(), 7.0)));
    set(&mut s, sk, "plane_offset", 12.0);
    assert!(near(z_range(&s, 1), (12.0, 15.0)));
    set(&mut s, sk, "plane_offset", -5.0);
    assert!(near(z_range(&s, 1), (-5.0, -2.0)));
    // An XZ offset plane moves along its normal (−Y).
    let xz = SketchPlane::Offset {
        base: PlaneBase::WorldXZ,
        distance: 4.0,
        angle: 0.0,
    };
    let sk2 = sketch(&mut s, xz, rect(6.0, 6.0));
    ok(&mut s, Command::extrude(None, sk2, 2.0));
    let b = s.summary().bodies[1].measurements.clone().unwrap().bounds;
    assert!(
        (b.max[1] + 4.0).abs() < 1e-9 && (b.min[1] + 6.0).abs() < 1e-9,
        "{b:?}"
    );
}

#[test]
fn offset_from_the_cavity_floor_bound_to_a_parameter() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let base = sketch(&mut s, SketchPlane::WorldXY, rect(80.0, 50.0));
    let e = ok(&mut s, Command::extrude(None, base, 25.0))
        .feature
        .unwrap();
    let top = SketchPlane::Face(FaceRef {
        feature: e,
        role: FaceRole::ExtrudeEndCap,
    });
    let cav = sketch(&mut s, top, rect(76.0, 46.0));
    let cut = ok(
        &mut s,
        Command::Extrude {
            name: None,
            sketch: cav,
            distance: 23.0,
            op: ExtrudeOp::Cut,
            target: None,
            through_all: false,
            reverse: false,
            body_name: None,
            region: None,
        },
    )
    .feature
    .unwrap();
    // A standoff sketch `standoff` above the cavity floor (end cap of the cut,
    // outward normal +Z at z = 2).
    ok(
        &mut s,
        Command::SetUserParameter {
            name: "standoff".into(),
            expression: "5".into(),
            unit: Some(ParamUnit::Mm),
            comment: None,
        },
    );
    let floor = PlaneBase::Face(FaceRef {
        feature: cut,
        role: FaceRole::ExtrudeEndCap,
    });
    let boss = sketch(
        &mut s,
        SketchPlane::Offset {
            base: floor,
            distance: 5.0,
            angle: 0.0,
        },
        rect(10.0, 10.0),
    );
    ok(
        &mut s,
        Command::SetExpression {
            feature: boss,
            name: "plane_offset".into(),
            expression: "standoff".into(),
        },
    );
    ok(&mut s, Command::extrude(None, boss, 4.0));
    assert!(near(z_range(&s, 2), (7.0, 11.0)), "{:?}", z_range(&s, 2));
    // The parameter moves the plane; the floor moving moves it too.
    ok(
        &mut s,
        Command::SetUserParameter {
            name: "standoff".into(),
            expression: "8".into(),
            unit: None,
            comment: None,
        },
    );
    assert!(near(z_range(&s, 2), (10.0, 14.0)), "{:?}", z_range(&s, 2));
    set(&mut s, cut, "distance", 20.0);
    assert!(near(z_range(&s, 2), (13.0, 17.0)), "{:?}", z_range(&s, 2));
    // Dependency on the base face's feature: the cut cannot be deleted.
    let r = s.execute(&Command::DeleteFeature { feature: cut });
    assert!(!r.ok, "{r:?}");

    // Save / reopen keeps the plane and its binding.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plane.rcad.json");
    ok(&mut s, Command::Save { path: path.clone() });
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"Offset\""));
    let mut t = Session::new();
    ok(&mut t, Command::Open { path });
    assert_eq!(t.summary().features, s.summary().features);
    assert!(near(z_range(&t, 2), (13.0, 17.0)));
}

#[test]
fn missing_base_face_is_a_broken_reference() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let base = sketch(&mut s, SketchPlane::WorldXY, rect(20.0, 10.0));
    let e = ok(&mut s, Command::extrude(None, base, 5.0))
        .feature
        .unwrap();
    let rev = s.summary().revision;
    let r = s.execute(&Command::CreateSketch {
        name: None,
        plane: SketchPlane::Offset {
            base: PlaneBase::Face(FaceRef {
                feature: e,
                role: FaceRole::ExtrudeSide { entity: 99 },
            }),
            distance: 2.0,
            angle: 0.0,
        },
        template: rect(4.0, 4.0),
    });
    assert!(
        matches!(r.error, Some(DocError::BrokenReference { .. })),
        "{r:?}"
    );
    assert_eq!(s.summary().revision, rev);
    // Non-finite distance: refused.
    let r = s.execute(&Command::CreateSketch {
        name: None,
        plane: SketchPlane::Offset {
            base: PlaneBase::WorldXY,
            distance: f64::NAN,
            angle: 0.0,
        },
        template: rect(4.0, 4.0),
    });
    assert!(!r.ok);
    assert_eq!(s.summary().revision, rev);
}

#[test]
fn plane_json_shape_is_additive() {
    let p: SketchPlane =
        serde_json::from_str(r#"{"Offset":{"base":"WorldXY","distance":7.0}}"#).unwrap();
    assert_eq!(
        p,
        SketchPlane::Offset {
            base: PlaneBase::WorldXY,
            distance: 7.0,
            angle: 0.0,
        }
    );
    let f: SketchPlane = serde_json::from_str(
        r#"{"Offset":{"base":{"Face":{"feature":2,"role":"ExtrudeEndCap"}},"distance":-1.5}}"#,
    )
    .unwrap();
    assert!(matches!(
        f,
        SketchPlane::Offset {
            base: PlaneBase::Face(_),
            ..
        }
    ));
    // Old forms unchanged.
    assert_eq!(
        serde_json::to_string(&SketchPlane::WorldXY).unwrap(),
        r#""WorldXY""#
    );
}
