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

#[test]
fn nested_loops_make_a_plate_with_holes_in_one_feature() {
    use f420_document::FaceRole;
    use std::f64::consts::PI;
    let mut sk = f420_sketch::Sketch::default();
    sk.ensure_origin();
    let quad = |sk: &mut f420_sketch::Sketch, v: [[f64; 2]; 4]| {
        let p: Vec<_> = v.iter().map(|q| sk.add_point(q[0], q[1])).collect();
        (0..4)
            .map(|i| sk.add_line(p[i], p[(i + 1) % 4]))
            .collect::<Vec<_>>()
    };
    quad(
        &mut sk,
        [[0.0, 0.0], [80.0, 0.0], [80.0, 50.0], [0.0, 50.0]],
    );
    let c = sk.add_point(20.0, 25.0);
    sk.add_circle(c, 5.0);
    let slot = quad(
        &mut sk,
        [[50.0, 22.0], [60.0, 22.0], [60.0, 28.0], [50.0, 28.0]],
    );
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let sid = ok(
        &mut s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::Custom { sketch: sk },
        },
    )
    .feature
    .unwrap();
    let e = ok(&mut s, Command::extrude(None, sid, 10.0))
        .feature
        .unwrap();
    let b = s.summary().bodies[0].id;
    let want = (80.0 * 50.0 - 25.0 * PI - 60.0) * 10.0;
    assert!((body_volume(&s, b) - want).abs() < 1e-7 * want);
    // The slot's walls are referenceable side faces facing into the slot: the
    // wall on x = 50 has outward normal +X (material on the -X side).
    let faces = s
        .summary()
        .features
        .into_iter()
        .find(|f| f.id == e)
        .unwrap()
        .faces;
    let wall = faces.iter().find(|f| {
        matches!(f.reference.role, FaceRole::ExtrudeSide { entity } if slot.iter().any(|id| id.0 == entity))
            && (f.frame.origin[0] - 50.0).abs() < 1e-9
            && f.frame.x[1].abs() > 0.5
    });
    let wall = wall.expect("slot wall at x=50");
    assert!((wall.frame.z[0] - 1.0).abs() < 1e-12, "{:?}", wall.frame);
    // Join/Cut with inner loops: explicit product limit, model kept.
    let rev = s.model().unwrap().revision;
    let r = s.execute(&Command::Extrude {
        name: None,
        sketch: sid,
        distance: 5.0,
        op: ExtrudeOp::Cut,
        target: Some(b),
        through_all: false,
        reverse: false,
        body_name: None,
    });
    assert!(
        matches!(r.error, Some(DocError::ProductLimit(ref m)) if m.contains("inner loop")),
        "{r:?}"
    );
    assert_eq!(s.model().unwrap().revision, rev);
}

#[test]
fn threemf_import_and_multi_body_export_round_trip() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    // Independent fixture (FreeCAD): two 10 mm cubes, millimetres.
    ok(
        &mut s,
        Command::Import3mf {
            path: fixture("crates/f420-exchange/tests/fixtures/threemf/freecad_two_boxes.3mf"),
        },
    );
    let bodies = s.summary().bodies;
    assert_eq!(bodies.len(), 2);
    for b in &bodies {
        assert_eq!(b.kind, "mesh");
        let d = b.mesh.as_ref().unwrap();
        assert!(d.closed && (d.signed_volume.unwrap() - 1000.0).abs() < 1e-9);
    }
    // Add a B-rep body and export all three as 3MF (inch), then re-import.
    let sk = rect(&mut s, SketchPlane::WorldXY, 40.0, 20.0, [100.0, 0.0]);
    ok(&mut s, Command::extrude(None, sk, 5.0));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("all.3mf");
    let r = ok(
        &mut s,
        Command::Export3mf {
            path: out.clone(),
            chord: None,
            bodies: None,
            unit: Some(f420_exchange::threemf::ThreeMfUnit::Inch),
        },
    );
    assert!(r.message.unwrap().contains("3 objects"));
    let mut t = Session::new();
    ok(&mut t, Command::NewDocument);
    ok(&mut t, Command::Import3mf { path: out });
    let back = t.summary().bodies;
    assert_eq!(back.len(), 3);
    let vols: Vec<f64> = back
        .iter()
        .map(|b| b.mesh.as_ref().unwrap().signed_volume.unwrap())
        .collect();
    // Planar faces only: tessellation is exact, units round trip through inch.
    assert!(
        (vols[0] - 1000.0).abs() < 1e-6 && (vols[1] - 1000.0).abs() < 1e-6,
        "{vols:?}"
    );
    assert!((vols[2] - 4000.0).abs() < 1e-6, "{vols:?}");
    assert_eq!(back[2].name, s.summary().bodies[2].name);
}

#[test]
fn every_command_serializes_under_its_documented_name() {
    let p = PathBuf::from("x");
    for c in [
        Command::Import3mf { path: p.clone() },
        Command::Export3mf {
            path: p.clone(),
            chord: None,
            bodies: None,
            unit: None,
        },
        Command::ImportSvg {
            path: p.clone(),
            plane: SketchPlane::WorldXY,
            name: None,
            units_to_mm: None,
            allow_partial: false,
        },
        Command::ExportSvg {
            path: p.clone(),
            sketch: f420_document::FeatureId(1),
        },
        Command::CircularPattern {
            name: None,
            feature: f420_document::FeatureId(1),
            count: 2,
            angle_deg: 10.0,
            center: [0.0, 0.0],
        },
    ] {
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["cmd"], c.name(), "{v}");
        let back: Command = serde_json::from_value(v).unwrap();
        assert_eq!(back, c);
    }
}

#[test]
fn dxf_import_notes_persist_on_the_sketch_feature() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let id = ok(
        &mut s,
        Command::ImportDxf {
            path: fixture("crates/f420-exchange/tests/fixtures/dxf/slot_freecad_r14.dxf"),
            plane: SketchPlane::WorldXY,
            name: None,
            units_to_mm: Some(1.0),
            allow_partial: false,
            layers: None,
        },
    )
    .feature
    .unwrap();
    let notes = |s: &Session| {
        s.summary()
            .features
            .into_iter()
            .find(|f| f.id == id)
            .unwrap()
            .notes
    };
    assert!(
        notes(&s).iter().any(|n| n.starts_with("units:")),
        "{:?}",
        notes(&s)
    );
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("d.f420.json");
    ok(&mut s, Command::Save { path: p.clone() });
    ok(&mut s, Command::Open { path: p });
    assert!(notes(&s).iter().any(|n| n.starts_with("units:")));
}

#[test]
fn deferred_measurement_of_imported_bodies() {
    let mut s = Session::new();
    s.set_defer_measurements(true);
    ok(&mut s, Command::NewDocument);
    for f in ["two_boxes_compound_mm.step", "plate_hole_mm.step"] {
        ok(
            &mut s,
            Command::ImportStep {
                path: fixture(&format!("crates/f420-kernel/tests/fixtures/step/{f}")),
                units_to_mm: None,
            },
        );
    }
    let (doc, rev, bodies) = s.pending_measurement().expect("pending");
    assert_eq!(bodies.len(), 3);
    let m: Vec<_> = bodies
        .iter()
        .map(|(id, b)| (*id, f420_kernel::measure(b).unwrap()))
        .collect();
    assert!(s.adopt_measurements(doc, rev, m));
    assert!(s.summary().bodies.iter().all(|b| b.measurements.is_some()));
}
