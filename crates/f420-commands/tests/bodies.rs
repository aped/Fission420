//! Multi-body command behaviour (ADR 0003): extrude options, mesh-body limits,
//! deletion pruning, embedded imports. Scenario-level checks live in the
//! acceptance runner (steps 3A/3B).
use f420_commands::{Command, ExtrudeOp, Response, Session, SketchTemplate};
use f420_document::{BodyId, DocError, SketchPlane};
use f420_kernel::VolumeResult;
use std::path::PathBuf;

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn rect(
    s: &mut Session,
    plane: SketchPlane,
    w: f64,
    h: f64,
    c: [f64; 2],
) -> f420_document::FeatureId {
    ok(
        s,
        Command::CreateSketch {
            name: None,
            plane,
            template: SketchTemplate::CenteredRectangle {
                width: w,
                height: h,
                center: Some(c),
            },
        },
    )
    .feature
    .unwrap()
}

fn extrude(
    sketch: f420_document::FeatureId,
    d: f64,
    op: ExtrudeOp,
    target: Option<BodyId>,
    through_all: bool,
    reverse: bool,
) -> Command {
    Command::Extrude {
        name: None,
        sketch,
        distance: d,
        op,
        target,
        through_all,
        reverse,
        body_name: None,
    }
}

fn body_volume(s: &Session, id: BodyId) -> f64 {
    let b = s.summary().bodies.into_iter().find(|b| b.id == id).unwrap();
    match b.measurements.unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

#[test]
fn reverse_through_all_and_invalid_options() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let sk = rect(&mut s, SketchPlane::WorldXY, 40.0, 20.0, [0.0, 0.0]);
    // Reverse: the body lies below the sketch plane.
    ok(
        &mut s,
        extrude(sk, 10.0, ExtrudeOp::NewBody, None, false, true),
    );
    let b = s.summary().bodies[0].clone();
    let m = b.measurements.unwrap();
    assert!(
        (m.bounds.min[2] + 10.0).abs() < 1e-9 && m.bounds.max[2].abs() < 1e-9,
        "{m:?}"
    );
    // Through-all is Cut-only; NewBody refuses it, document unchanged.
    let before = s.model().unwrap().revision;
    let r = s.execute(&extrude(sk, 0.0, ExtrudeOp::NewBody, None, true, false));
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().revision, before);
    // A through-all cut from a sketch plane at the body's top face (z = 0)
    // cuts both sides along the normal: the whole thickness.
    let hole = rect(&mut s, SketchPlane::WorldXY, 4.0, 4.0, [0.0, 0.0]);
    ok(
        &mut s,
        extrude(hole, 0.0, ExtrudeOp::Cut, None, true, false),
    );
    assert!((body_volume(&s, b.id) - (40.0 * 20.0 * 10.0 - 16.0 * 10.0)).abs() < 1e-6);
}

#[test]
fn mesh_bodies_are_display_and_export_only() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let stl = fixture("crates/f420-exchange/tests/fixtures/stl/box_freecad_binary.stl");
    // STL is unitless: the unit must be chosen.
    let r = s.execute(&Command::ImportStl {
        path: stl.clone(),
        units_to_mm: None,
    });
    assert!(
        matches!(r.error, Some(DocError::Validation(ref m)) if m.contains("no units")),
        "{r:?}"
    );
    let r = ok(
        &mut s,
        Command::ImportStl {
            path: stl,
            units_to_mm: Some(1.0),
        },
    );
    let imp = r.feature.unwrap();
    let mesh = s.summary().bodies[0].clone();
    assert_eq!(mesh.kind, "mesh");
    assert!(
        mesh.measurements.is_none(),
        "no exact measurements for meshes"
    );
    let diag = mesh.mesh.unwrap();
    assert!(diag.closed && diag.triangles > 0);
    // Exact operations on the mesh body are refused with an explanation.
    let sk = rect(&mut s, SketchPlane::WorldXY, 4.0, 4.0, [1.0, 1.0]);
    let r = s.execute(&extrude(
        sk,
        2.0,
        ExtrudeOp::Cut,
        Some(mesh.id),
        false,
        false,
    ));
    assert!(
        matches!(r.error, Some(DocError::ProductLimit(ref m)) if m.contains("mesh")),
        "{r:?}"
    );
    // STEP export refuses meshes; STL export writes them.
    let dir = tempfile::tempdir().unwrap();
    let r = s.execute(&Command::ExportStep {
        path: dir.path().join("m.step"),
        bodies: None,
    });
    assert!(!r.ok);
    ok(
        &mut s,
        Command::ExportStl {
            path: dir.path().join("m.stl"),
            chord: None,
            bodies: None,
        },
    );
    // Deleting the import removes its body and its embedded file.
    ok(&mut s, Command::DeleteFeature { feature: sk });
    ok(&mut s, Command::DeleteFeature { feature: imp });
    let d = s.model().unwrap().document();
    assert!(d.bodies.is_empty() && d.blobs.is_empty());
}

#[test]
fn imported_step_survives_moving_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("box.step");
    std::fs::copy(
        fixture("crates/f420-kernel/tests/fixtures/step/box_40x30x10_mm.step"),
        &src,
    )
    .unwrap();
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    ok(
        &mut s,
        Command::ImportStep {
            path: src.clone(),
            units_to_mm: None,
        },
    );
    let b = s.summary().bodies[0].clone();
    let v0 = body_volume(&s, b.id);
    assert!((v0 - 12000.0).abs() < 1e-6, "{v0}");
    let doc = dir.path().join("imported.f420.json");
    ok(&mut s, Command::Save { path: doc.clone() });
    std::fs::remove_file(&src).unwrap();
    ok(&mut s, Command::Close);
    ok(&mut s, Command::Open { path: doc });
    let b2 = s.summary().bodies[0].clone();
    assert_eq!((b2.id, b2.name), (b.id, b.name));
    assert!((body_volume(&s, b2.id) - v0).abs() < 1e-9);
}
