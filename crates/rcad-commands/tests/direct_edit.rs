//! Direct editing of an imported STEP body through the command API (run 6,
//! ADR 0010). Input: the independent FreeCAD fixture `direct_edit_block_mm.step`
//! (block [0,60]x[0,40]x[0,20], Ø8 through hole at (15,20), Ø6 x 8 blind hole
//! at (30,12), Ø10 x 5 boss at (45,25); provenance in the fixtures README).
//! Expected values are analytic.
use rcad_commands::{Command, ExtrudeOp, Response, Session, SketchTemplate};
use rcad_document::{BodyId, DirectOp, DocError, FaceRef, FeatureId, SketchPlane};
use rcad_kernel::{DirectFace, VolumeResult};
use std::f64::consts::PI;
use std::path::PathBuf;

const V0: f64 = 48000.0 - 267.0 * PI;
const TOP_AREA: f64 = 2400.0 - 50.0 * PI;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rcad-kernel/tests/fixtures/step/direct_edit_block_mm.step")
}

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn err(s: &mut Session, c: Command) -> DocError {
    let before = s.summary();
    let r = s.execute(&c);
    assert!(!r.ok, "{c:?} should fail");
    let after = s.summary();
    assert_eq!(before.features.len(), after.features.len(), "unchanged");
    r.error.unwrap()
}

fn setup() -> (Session, BodyId) {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let r = ok(
        &mut s,
        Command::ImportStep {
            path: fixture(),
            units_to_mm: None,
        },
    );
    let f = r.feature.unwrap();
    let body = s
        .summary()
        .features
        .into_iter()
        .find(|x| x.id == f)
        .unwrap()
        .creates[0];
    (s, body)
}

fn faces(s: &mut Session, body: BodyId) -> Vec<(FaceRef, DirectFace)> {
    let r = ok(s, Command::ImportedFaces { body });
    let v: Vec<serde_json::Value> = serde_json::from_str(&r.message.unwrap()).unwrap();
    v.into_iter()
        .map(|x| {
            (
                serde_json::from_value(x["face"].clone()).unwrap(),
                serde_json::from_value(x["kind"].clone()).unwrap(),
            )
        })
        .collect()
}

fn plane(s: &mut Session, body: BodyId, n: [f64; 3], level: f64) -> FaceRef {
    let hits: Vec<FaceRef> = faces(s, body)
        .into_iter()
        .filter_map(|(r, k)| match k {
            DirectFace::Plane { origin, normal }
                if (0..3).all(|i| (normal[i] - n[i]).abs() < 1e-9)
                    && ((0..3).map(|i| origin[i] * n[i]).sum::<f64>() - level).abs() < 1e-9 =>
            {
                Some(r)
            }
            _ => None,
        })
        .collect();
    assert_eq!(hits.len(), 1, "{n:?} at {level}");
    hits[0]
}

fn cylinder(s: &mut Session, body: BodyId, x: f64, y: f64) -> FaceRef {
    faces(s, body)
        .into_iter()
        .find_map(|(r, k)| match k {
            DirectFace::Cylinder { origin, .. }
                if (origin[0] - x).abs() < 1e-6 && (origin[1] - y).abs() < 1e-6 =>
            {
                Some(r)
            }
            _ => None,
        })
        .unwrap()
}

fn volume(s: &Session, body: BodyId) -> f64 {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id == body)
        .unwrap();
    match b.measurements.expect("measured").volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn assert_volume(s: &Session, body: BodyId, want: f64) {
    let v = volume(s, body);
    assert!(
        (v - want).abs() <= 1e-6 * want,
        "volume {v} vs {want} (diff {})",
        v - want
    );
}

fn edit(s: &mut Session, face: FaceRef, op: DirectOp) -> FeatureId {
    ok(
        s,
        Command::DirectEdit {
            name: None,
            face,
            op,
        },
    )
    .feature
    .unwrap()
}

#[test]
fn offset_resize_remove_with_parameters_undo_and_reopen() {
    let (mut s, body) = setup();
    assert_volume(&s, body, V0);
    let top = plane(&mut s, body, [0.0, 0.0, 1.0], 20.0);
    // Offset bound to a user parameter.
    ok(
        &mut s,
        Command::SetUserParameter {
            name: "lift".into(),
            expression: "3 mm".into(),
            unit: None,
            comment: None,
        },
    );
    let off = edit(&mut s, top, DirectOp::OffsetFace { distance: 1.0 });
    ok(
        &mut s,
        Command::SetExpression {
            feature: off,
            name: "distance".into(),
            expression: "lift".into(),
        },
    );
    assert_volume(&s, body, V0 + 3.0 * TOP_AREA);
    // The same imported face reference now names the moved face.
    assert_eq!(plane(&mut s, body, [0.0, 0.0, 1.0], 23.0), top);
    let hole = cylinder(&mut s, body, 15.0, 20.0);
    edit(&mut s, hole, DirectOp::ResizeCylinder { diameter: 12.0 });
    let after_resize = V0 + 3.0 * TOP_AREA - PI * (36.0 - 16.0) * 23.0;
    assert_volume(&s, body, after_resize);
    let blind = cylinder(&mut s, body, 30.0, 12.0);
    let floor = plane(&mut s, body, [0.0, 0.0, 1.0], 12.0);
    let rm = edit(&mut s, blind, DirectOp::RemoveCylinder);
    let after_remove = after_resize + PI * 9.0 * 11.0;
    assert_volume(&s, body, after_remove);

    // Upstream parameter edit: everything regenerates from the embedded STEP.
    ok(
        &mut s,
        Command::SetUserParameter {
            name: "lift".into(),
            expression: "4 mm".into(),
            unit: None,
            comment: None,
        },
    );
    let lifted = V0 + 4.0 * TOP_AREA - PI * 20.0 * 24.0 + PI * 9.0 * 12.0;
    assert_volume(&s, body, lifted);
    // A value that collapses the 5 mm boss is refused; nothing changes.
    let e = s.execute(&Command::SetUserParameter {
        name: "lift".into(),
        expression: "5 mm".into(),
        unit: None,
        comment: None,
    });
    assert!(!e.ok);
    assert!(
        format!("{:?}", e.error).contains("collapse"),
        "{:?}",
        e.error
    );
    assert_volume(&s, body, lifted);

    // The removed floor's reference is broken, never another face.
    let e = err(
        &mut s,
        Command::DirectEdit {
            name: None,
            face: floor,
            op: DirectOp::OffsetFace { distance: -1.0 },
        },
    );
    assert!(
        matches!(&e, DocError::BrokenReference { detail, .. } if detail.contains("no longer exists")),
        "{e:?}"
    );

    // Undo the parameter change, then the removal; redo both.
    ok(&mut s, Command::Undo);
    assert_volume(&s, body, after_remove);
    ok(&mut s, Command::Undo);
    assert_volume(&s, body, after_resize);
    ok(&mut s, Command::Redo);
    assert_volume(&s, body, after_remove);
    ok(&mut s, Command::Redo);
    assert_volume(&s, body, lifted);
    assert!(s.summary().features.iter().any(|f| f.id == rm));

    // Save and reopen: rebuilt from the embedded STEP bytes (the source path
    // is not used), same volume; file format v6.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("direct_edit");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("direct_edit.rcad.json");
    ok(&mut s, Command::Save { path: path.clone() });
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"version\": 6"), "v6 file");
    // A file labelled v5 holding a direct edit is refused, not misread.
    let v5 = dir.join("direct_edit_as_v5.rcad.json");
    std::fs::write(&v5, text.replace("\"version\": 6", "\"version\": 5")).unwrap();
    let r = Session::new().execute(&Command::Open { path: v5 });
    assert!(
        !r.ok && format!("{:?}", r.error).contains("v6 feature"),
        "{:?}",
        r.error
    );
    let mut s2 = Session::new();
    ok(&mut s2, Command::Open { path: path.clone() });
    assert_volume(&s2, body, lifted);

    // STEP export of the edited body.
    let out = dir.join("direct_edit_export.step");
    ok(
        &mut s2,
        Command::ExportStep {
            path: out.clone(),
            bodies: None,
        },
    );
    assert!(
        std::fs::read_to_string(&out)
            .unwrap()
            .contains("MANIFOLD_SOLID_BREP")
    );
    if let Ok(evidence) = std::env::var("RCAD_EVIDENCE_DIR") {
        std::fs::create_dir_all(&evidence).unwrap();
        std::fs::copy(&out, PathBuf::from(evidence).join("direct_cmd_lift4.step")).unwrap();
    }
}

#[test]
fn refusals_leave_the_document_unchanged() {
    let (mut s, body) = setup();
    let hole = cylinder(&mut s, body, 15.0, 20.0);
    let e = err(
        &mut s,
        Command::DirectEdit {
            name: None,
            face: hole,
            op: DirectOp::ResizeCylinder { diameter: 29.0 },
        },
    );
    assert!(
        matches!(&e, DocError::ProductLimit(m) if m.contains("would reach another edge")),
        "{e:?}"
    );
    let top = plane(&mut s, body, [0.0, 0.0, 1.0], 20.0);
    let e = err(
        &mut s,
        Command::DirectEdit {
            name: None,
            face: top,
            op: DirectOp::OffsetFace { distance: -8.0 },
        },
    );
    assert!(
        matches!(&e, DocError::ProductLimit(m) if m.contains("collapse")),
        "{e:?}"
    );
    // Offsetting a cylinder / resizing a plane: named refusals.
    assert!(matches!(
        err(
            &mut s,
            Command::DirectEdit {
                name: None,
                face: hole,
                op: DirectOp::OffsetFace { distance: 1.0 },
            },
        ),
        DocError::ProductLimit(_)
    ));
    assert_volume(&s, body, V0);
}

#[test]
fn direct_edit_after_a_modeled_feature_is_refused() {
    let (mut s, body) = setup();
    let top = plane(&mut s, body, [0.0, 0.0, 1.0], 20.0);
    // A modeled pocket on the imported top face changes the body's topology.
    let sk = ok(
        &mut s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::Face(top),
            template: SketchTemplate::CenteredRectangle {
                width: 6.0,
                height: 4.0,
                center: None,
            },
        },
    )
    .feature
    .unwrap();
    let frame = s
        .summary()
        .features
        .into_iter()
        .find(|f| f.id == sk)
        .unwrap()
        .frame
        .unwrap();
    assert!((frame.origin[2] - 20.0).abs() < 1e-9);
    ok(
        &mut s,
        Command::Extrude {
            name: None,
            sketch: sk,
            distance: 2.0,
            op: ExtrudeOp::Cut,
            target: Some(body),
            through_all: false,
            reverse: false,
            body_name: None,
            region: None,
        },
    );
    let hole_face = FaceRef {
        role: rcad_document::FaceRole::Imported { body, face: 0 },
        ..top
    };
    let e = err(
        &mut s,
        Command::DirectEdit {
            name: None,
            face: hole_face,
            op: DirectOp::RemoveCylinder,
        },
    );
    assert!(
        matches!(&e, DocError::BrokenReference { detail, .. } if detail.contains("can no longer be identified")),
        "{e:?}"
    );
    assert!(s.execute(&Command::ImportedFaces { body }).error.is_some());
}

#[test]
fn sketch_on_offset_face_follows_it() {
    let (mut s, body) = setup();
    let top = plane(&mut s, body, [0.0, 0.0, 1.0], 20.0);
    let off = edit(&mut s, top, DirectOp::OffsetFace { distance: 2.5 });
    let sk = ok(
        &mut s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::Face(top),
            template: SketchTemplate::CenteredCircle {
                diameter: 4.0,
                center: None,
            },
        },
    )
    .feature
    .unwrap();
    let z = |s: &Session| {
        s.summary()
            .features
            .into_iter()
            .find(|f| f.id == sk)
            .unwrap()
            .frame
            .unwrap()
            .origin[2]
    };
    assert!((z(&s) - 22.5).abs() < 1e-9);
    ok(
        &mut s,
        Command::SetParameter {
            feature: off,
            name: "distance".into(),
            value: -1.5,
        },
    );
    assert!((z(&s) - 18.5).abs() < 1e-9);
    assert_volume(&s, body, V0 - 1.5 * TOP_AREA);
}

#[test]
fn documented_json_form_parses() {
    // docs/COMMANDS.md example.
    let c: Command = serde_json::from_str(
        r#"{"cmd": "direct_edit", "face": {"feature": 1, "role": {"Imported": {"body": 1, "face": 4}}},
            "op": {"OffsetFace": {"distance": 3.0}}}"#,
    )
    .unwrap();
    assert!(matches!(
        c,
        Command::DirectEdit {
            op: DirectOp::OffsetFace { distance: 3.0 },
            ..
        }
    ));
    let r: Command = serde_json::from_str(r#"{"cmd": "imported_faces", "body": 1}"#).unwrap();
    assert!(matches!(r, Command::ImportedFaces { .. }));
}
