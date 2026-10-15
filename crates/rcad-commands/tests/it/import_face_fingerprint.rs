//! References into an imported STEP body keep a print of the face they
//! name (plane and area, format v9). If the same bytes later import with a
//! different face at that index (e.g. a newer importer orders faces
//! differently, simulated here by a print that no longer matches), the
//! reference is broken and reported, never bound to the other face.
use rcad_commands::{Command, Session};
use rcad_document::{FaceRef, FeatureId, HealthState, Model, SketchPlane};
use rcad_kernel::DirectFace;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rcad-kernel/tests/fixtures/step/direct_edit_block_mm.step")
}

fn ok(s: &mut Session, c: Command) -> rcad_commands::Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

/// Import the block and sketch a circle on its first planar face (F2).
fn sketched() -> (Session, FaceRef) {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let f = ok(
        &mut s,
        Command::ImportStep {
            path: fixture(),
            units_to_mm: None,
        },
    )
    .feature
    .unwrap();
    let body = s
        .summary()
        .features
        .into_iter()
        .find(|x| x.id == f)
        .unwrap()
        .creates[0];
    let r = ok(&mut s, Command::ImportedFaces { body });
    let v: Vec<serde_json::Value> = serde_json::from_str(&r.message.unwrap()).unwrap();
    let face: FaceRef = v
        .iter()
        .find(|x| {
            matches!(
                serde_json::from_value::<DirectFace>(x["kind"].clone()),
                Ok(DirectFace::Plane { .. })
            )
        })
        .map(|x| serde_json::from_value(x["face"].clone()).unwrap())
        .expect("a planar face");
    ok(
        &mut s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::Face(face),
            template: rcad_commands::SketchTemplate::CenteredCircle {
                diameter: 4.0,
                center: None,
            },
        },
    );
    (s, face)
}

#[test]
fn a_referenced_imported_face_is_printed_and_checked() {
    let (s, face) = sketched();
    let doc = s.model().unwrap().document().clone();
    let prints = &doc.feature(FeatureId(1)).unwrap().face_prints;
    assert_eq!(prints.len(), 1, "{prints:?}");
    let rcad_document::FaceRole::Imported { body, face: index } = face.role else {
        panic!("{face:?}")
    };
    assert_eq!((prints[0].body, prints[0].face), (body, index));
    assert_eq!(rcad_document::required_version(&doc), 9);
    // Reopened as saved: healthy, same prints.
    let (m, _) = Model::open_document(doc.clone(), false).unwrap();
    assert!(m.evaluation().is_healthy());
    assert_eq!(m.document(), &doc);

    // The face at that index is now another plane: the sketch on it is a
    // broken reference; the import itself and nothing else changes.
    let mut moved = doc.clone();
    let p = &mut moved
        .features
        .iter_mut()
        .find(|f| f.id == FeatureId(1))
        .unwrap()
        .face_prints[0];
    p.normal = p.normal.map(|x| -x);
    let (m, report) = Model::open_document(moved, false).unwrap();
    let r = m
        .evaluation()
        .features
        .iter()
        .find(|r| r.id == FeatureId(2))
        .unwrap();
    assert_eq!(r.health.state, HealthState::Error);
    let msg = r.health.message.clone().unwrap_or_default();
    assert!(msg.contains("no longer the face"), "{msg}");
    assert_eq!(report.failures.len(), 1);
    assert_eq!(
        m.evaluation().features[0].health.state,
        HealthState::Healthy
    );
}
