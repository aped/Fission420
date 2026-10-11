//! The full GOAL.md workflow as a test (same code path as `f420-cli acceptance`).
#[test]
fn acceptance_workflow_passes() {
    let dir = tempfile::tempdir().unwrap();
    let report = f420_commands::acceptance::run(dir.path());
    for c in report.checks.iter().filter(|c| !c.pass) {
        eprintln!(
            "FAIL [{}] {}: expected {} | actual {}",
            c.step, c.name, c.expected, c.actual
        );
    }
    assert!(report.ok(), "{} checks failed", report.failed);
}

#[test]
fn commands_round_trip_as_json() {
    use f420_commands::Command;
    let text = r#"[{"cmd":"new_document"},
      {"cmd":"create_sketch","plane":"WorldXY","template":"centered_rectangle","width":80,"height":50},
      {"cmd":"extrude","sketch":1,"distance":10},
      {"cmd":"create_sketch","plane":{"Face":{"feature":2,"role":"ExtrudeEndCap"}},"template":"centered_circle","diameter":10},
      {"cmd":"cut_through_all","sketch":3},
      {"cmd":"set_parameter","feature":1,"name":"width","value":100}]"#;
    let cmds: Vec<Command> = serde_json::from_str(text).unwrap();
    let back: Vec<Command> = serde_json::from_str(&serde_json::to_string(&cmds).unwrap()).unwrap();
    assert_eq!(cmds, back);
}

#[test]
fn commands_without_document_fail_cleanly() {
    use f420_commands::{Command, Session};
    let mut s = Session::new();
    for c in [
        Command::Undo,
        Command::Measure,
        Command::Close,
        Command::extrude(None, f420_document::FeatureId(1), 1.0),
    ] {
        let r = s.execute(&c);
        assert!(!r.ok && r.error.is_some(), "{c:?}");
    }
    let r = s.execute(&Command::Open {
        path: "/nonexistent/x.f420.json".into(),
    });
    assert!(!r.ok);
}

#[test]
fn off_center_hole_is_placed_editable_and_contained() {
    use f420_commands::{Command, Session, SketchTemplate, end_cap_of};
    use f420_document::{FeatureId, SketchPlane};
    let mut s = Session::new();
    let ok = |s: &mut Session, c: Command| {
        let r = s.execute(&c);
        assert!(r.ok, "{c:?}: {:?}", r.error);
        r
    };
    ok(&mut s, Command::NewDocument);
    let rect = SketchTemplate::CenteredRectangle {
        width: 100.0,
        height: 50.0,
        center: None,
    };
    ok(
        &mut s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: rect,
        },
    );
    ok(&mut s, Command::extrude(None, FeatureId(1), 10.0));
    let circle = SketchTemplate::CenteredCircle {
        diameter: 10.0,
        center: Some([30.0, -10.0]),
    };
    ok(
        &mut s,
        Command::CreateSketch {
            name: None,
            plane: end_cap_of(FeatureId(2)),
            template: circle,
        },
    );
    ok(
        &mut s,
        Command::CutThroughAll {
            name: None,
            sketch: FeatureId(3),
        },
    );
    let centroid = |s: &Session| {
        let body = s.model().unwrap().evaluation().body.as_ref().unwrap();
        let l = f420_kernel::z_face_inner_loops(body).unwrap();
        assert_eq!(l.len(), 2);
        l[1].centroid_xy
    };
    let c = centroid(&s);
    assert!(
        (c[0] - 30.0).abs() < 1e-6 && (c[1] + 10.0).abs() < 1e-6,
        "{c:?}"
    );
    ok(
        &mut s,
        Command::SetParameter {
            feature: FeatureId(3),
            name: "center_x".into(),
            value: -20.0,
        },
    );
    let c = centroid(&s);
    assert!((c[0] + 20.0).abs() < 1e-6, "{c:?}");
    // Moving the hole so it breaks out of the plate is a product-limit rejection.
    let r = s.execute(&Command::SetParameter {
        feature: FeatureId(3),
        name: "center_x".into(),
        value: 47.0,
    });
    assert!(
        !r.ok && matches!(r.error, Some(f420_document::DocError::ProductLimit(_))),
        "{r:?}"
    );
    // Non-finite centre at creation is rejected.
    let bad = SketchTemplate::CenteredCircle {
        diameter: 4.0,
        center: Some([f64::NAN, 0.0]),
    };
    assert!(
        !s.execute(&Command::CreateSketch {
            name: None,
            plane: end_cap_of(FeatureId(2)),
            template: bad
        })
        .ok
    );
}

/// Revolve stretch feature: washer from an off-axis rectangle on WorldXZ.
/// Expected values by Pappus' theorem (V = A * angle * x_centroid), not the kernel.
#[test]
fn revolve_washer_edit_undo_and_limits() {
    use f420_commands::{Command, Session, SketchTemplate};
    use f420_document::{DocError, FeatureId, SketchPlane};
    use std::f64::consts::PI;
    let mut s = Session::new();
    let vol = |s: &Session| match s
        .model()
        .unwrap()
        .evaluation()
        .measurements
        .as_ref()
        .unwrap()
        .volume
    {
        f420_kernel::VolumeResult::Value { volume, .. } => volume,
        ref v => panic!("{v:?}"),
    };
    assert!(s.execute(&Command::NewDocument).ok);
    let rect = SketchTemplate::CenteredRectangle {
        width: 20.0,
        height: 10.0,
        center: Some([30.0, 5.0]),
    };
    assert!(
        s.execute(&Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXZ,
            template: rect
        })
        .ok
    );
    let r = s.execute(&Command::Revolve {
        name: None,
        sketch: FeatureId(1),
        angle_deg: 360.0,
    });
    assert!(r.ok, "{:?}", r.error);
    assert!(
        (vol(&s) - 200.0 * 2.0 * PI * 30.0).abs() < 0.004,
        "{}",
        vol(&s)
    );
    assert!(
        s.execute(&Command::SetParameter {
            feature: FeatureId(2),
            name: "angle_deg".into(),
            value: 90.0
        })
        .ok
    );
    assert!(
        (vol(&s) - 200.0 * PI / 2.0 * 30.0).abs() < 0.001,
        "{}",
        vol(&s)
    );
    // Moving the profile across the axis is rejected and rolled back.
    let r = s.execute(&Command::SetParameter {
        feature: FeatureId(1),
        name: "center_x".into(),
        value: 5.0,
    });
    assert!(
        !r.ok && matches!(r.error, Some(DocError::Validation(_))),
        "{r:?}"
    );
    assert!((vol(&s) - 200.0 * PI / 2.0 * 30.0).abs() < 0.001);
    assert!(
        !s.execute(&Command::SetParameter {
            feature: FeatureId(2),
            name: "angle_deg".into(),
            value: 400.0
        })
        .ok
    );
    assert!(s.execute(&Command::Undo).ok);
    assert!((vol(&s) - 200.0 * 2.0 * PI * 30.0).abs() < 0.004);
    // Cuts on revolved bodies are outside the product scope.
    let circle = SketchTemplate::CenteredCircle {
        diameter: 2.0,
        center: Some([30.0, 0.0]),
    };
    assert!(
        s.execute(&Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: circle
        })
        .ok
    );
    let r = s.execute(&Command::CutThroughAll {
        name: None,
        sketch: FeatureId(3),
    });
    assert!(
        !r.ok && matches!(r.error, Some(DocError::ProductLimit(_))),
        "{r:?}"
    );
    // STL of the washer is watertight.
    let dir = tempfile::tempdir().unwrap();
    let r = s.execute(&Command::ExportStl {
        path: dir.path().join("w.stl"),
        chord: Some(0.01),
        bodies: None,
        print: Default::default(),
    });
    assert!(r.ok && r.mesh_check.unwrap().watertight);
}

/// Response.superseded / elapsed_ms are front-end annotations: Session never
/// sets them, they are omitted from JSON when unset, and older JSON without
/// them still parses.
#[test]
fn response_front_end_fields_are_optional() {
    use f420_commands::{Command, Response, Session};
    let mut s = Session::new();
    let r = s.execute(&Command::NewDocument);
    assert!(r.ok && !r.superseded && r.elapsed_ms.is_none());
    let json = serde_json::to_value(&r).unwrap();
    assert!(json.get("superseded").is_none() && json.get("elapsed_ms").is_none());
    let old: Response = serde_json::from_str(r#"{"ok":true,"command":"inspect"}"#).unwrap();
    assert!(!old.superseded && old.elapsed_ms.is_none());
    let marked = Response {
        superseded: true,
        elapsed_ms: Some(1.5),
        ..r
    };
    let back: Response = serde_json::from_value(serde_json::to_value(&marked).unwrap()).unwrap();
    assert_eq!(back, marked);
}
