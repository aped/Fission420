//! Import/export edge cases through the command API (`Session::execute` only),
//! complementing acceptance scenarios C and D. Inputs are the independently
//! generated fixtures (provenance in their READMEs); expected values are
//! analytic and derived in the comments.
use rcad_commands::{
    BodySummary, Command, ExtrudeOp, FaceSummary, Response, Session, SketchTemplate,
};
use rcad_document::{BodyId, FaceRole, FeatureId, SketchPlane};
use rcad_kernel::{VolumeMethod, VolumeResult};
use std::f64::consts::PI;
use std::path::PathBuf;

fn fixture(dir: &str, file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(dir)
        .join(file)
}
fn step(file: &str) -> PathBuf {
    fixture("rcad-kernel/tests/fixtures/step", file)
}
fn stl(file: &str) -> PathBuf {
    fixture("rcad-exchange/tests/fixtures/stl", file)
}
fn dxf(file: &str) -> PathBuf {
    fixture("rcad-exchange/tests/fixtures/dxf", file)
}

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn import_step(s: &mut Session, file: &str) -> (FeatureId, Vec<BodyId>) {
    let r = ok(
        s,
        Command::ImportStep {
            path: step(file),
            units_to_mm: None,
        },
    );
    let f = r.feature.unwrap();
    let created = feature(s, f).creates;
    (f, created)
}

fn feature(s: &Session, id: FeatureId) -> rcad_commands::FeatureSummary {
    s.summary()
        .features
        .into_iter()
        .find(|f| f.id == id)
        .unwrap()
}

fn body(s: &Session, id: BodyId) -> BodySummary {
    s.summary().bodies.into_iter().find(|b| b.id == id).unwrap()
}

fn volume(s: &Session, id: BodyId) -> f64 {
    match body(s, id).measurements.expect("measured").volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

/// Exact (boundary-integral) volumes: GOAL.md tolerance. A labelled
/// tessellation estimate (no axis-ruled integral applies, e.g. holes along two
/// axes): the analytic value must lie within the estimate's stated error.
fn assert_volume(s: &Session, id: BodyId, exp: f64) {
    match body(s, id).measurements.expect("measured").volume {
        VolumeResult::Value {
            volume,
            error_estimate,
            method: VolumeMethod::TessellationEstimate,
        } => assert!(
            (volume - exp).abs() <= error_estimate,
            "estimate {volume} ± {error_estimate} vs analytic {exp}"
        ),
        VolumeResult::Value { volume, method, .. } => {
            assert!(vol_close(volume, exp), "{volume} ({method:?}) vs {exp}")
        }
        v => panic!("{v:?}"),
    }
}

fn bounds(s: &Session, id: BodyId) -> ([f64; 3], [f64; 3]) {
    let m = body(s, id).measurements.expect("measured");
    (m.bounds.min, m.bounds.max)
}

/// GOAL.md volume tolerance.
fn vol_close(v: f64, exp: f64) -> bool {
    (v - exp).abs() <= 0.001f64.max(exp * 1e-7)
}

fn close3(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= 1e-6)
}

/// The imported planar face of `body` with outward normal `n` through `p`.
fn face(s: &Session, body: BodyId, n: [f64; 3], p: [f64; 3]) -> FaceSummary {
    let v: Vec<FaceSummary> = s
        .summary()
        .features
        .into_iter()
        .flat_map(|f| f.faces)
        .filter(|f| {
            matches!(f.reference.role, FaceRole::Imported { body: b, .. } if b == body)
                && close3(f.frame.z, n)
                && (0..3)
                    .map(|i| (p[i] - f.frame.origin[i]) * n[i])
                    .sum::<f64>()
                    .abs()
                    <= 1e-6
        })
        .collect();
    assert_eq!(v.len(), 1, "{v:?}");
    v.into_iter().next().unwrap()
}

/// World point -> the face's local sketch coordinates.
fn local(f: &FaceSummary, p: [f64; 3]) -> [f64; 2] {
    let d = [
        p[0] - f.frame.origin[0],
        p[1] - f.frame.origin[1],
        p[2] - f.frame.origin[2],
    ];
    let dot = |a: [f64; 3]| d[0] * a[0] + d[1] * a[1] + d[2] * a[2];
    [dot(f.frame.x), dot(f.frame.y)]
}

fn circle_on(s: &mut Session, f: &FaceSummary, world: [f64; 3], d: f64) -> FeatureId {
    ok(
        s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::Face(f.reference),
            template: SketchTemplate::CenteredCircle {
                diameter: d,
                center: Some(local(f, world)),
            },
        },
    )
    .feature
    .unwrap()
}

/// Extrude along the sketch normal (`reverse`: against it; a blind cut from a
/// face sketch must be reversed to go into the material, ADR 0003).
fn extrude(
    s: &mut Session,
    sketch: FeatureId,
    op: ExtrudeOp,
    target: Option<BodyId>,
    distance: f64,
    through_all: bool,
    reverse: bool,
) -> Response {
    s.execute(&Command::Extrude {
        name: None,
        sketch,
        distance,
        op,
        target,
        through_all,
        reverse,
        body_name: None,
        region: None,
        options: Default::default(),
    })
}

#[test]
fn imported_box_join_boss_and_side_face_blind_cut() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let (_, b) = import_step(&mut s, "box_40x30x10_mm.step");
    let b = b[0];
    // Boss Ø10 x 5 on the top face at (10, 10): V = 12000 + 125 pi.
    let top = face(&s, b, [0.0, 0.0, 1.0], [0.0, 0.0, 10.0]);
    let sk = circle_on(&mut s, &top, [10.0, 10.0, 10.0], 10.0);
    let r = extrude(&mut s, sk, ExtrudeOp::Join, Some(b), 5.0, false, false);
    assert!(r.ok, "{:?}", r.error);
    assert!(vol_close(volume(&s, b), 12000.0 + 125.0 * PI));
    let (lo, hi) = bounds(&s, b);
    assert!(
        close3(lo, [0.0; 3]) && close3(hi, [40.0, 30.0, 15.0]),
        "{lo:?} {hi:?}"
    );
    // Blind cut Ø4, 6 deep, into the side face x = 40 at (y, z) = (15, 5):
    // V -= 4 pi * 6. The side face reference is the imported one (normal +X).
    let side = face(&s, b, [1.0, 0.0, 0.0], [40.0, 0.0, 0.0]);
    let sk = circle_on(&mut s, &side, [40.0, 15.0, 5.0], 4.0);
    let r = extrude(&mut s, sk, ExtrudeOp::Cut, Some(b), 6.0, false, false);
    assert!(r.ok, "{:?}", r.error);
    assert_volume(&s, b, 12000.0 + 125.0 * PI - 24.0 * PI);
    assert_eq!(s.summary().bodies.len(), 1);
}

#[test]
fn import_undo_redo_keeps_body_ids_and_geometry() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let (_, ids) = import_step(&mut s, "two_boxes_assembly_mm.step");
    assert_eq!(ids.len(), 2);
    let before: Vec<_> = s
        .summary()
        .bodies
        .iter()
        .map(|b| (b.id, b.name.clone()))
        .collect();
    ok(&mut s, Command::Undo);
    assert!(s.summary().bodies.is_empty());
    ok(&mut s, Command::Redo);
    let after: Vec<_> = s
        .summary()
        .bodies
        .iter()
        .map(|b| (b.id, b.name.clone()))
        .collect();
    assert_eq!(before, after);
    for id in ids {
        assert!(vol_close(volume(&s, id), 1000.0));
    }
}

#[test]
fn declared_step_unit_wins_over_override_and_bad_overrides_are_refused() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    // The inch file declares INCH: an override of 1 (mm) must be ignored.
    let r = ok(
        &mut s,
        Command::ImportStep {
            path: step("box_40x30x10_inch.step"),
            units_to_mm: Some(1.0),
        },
    );
    let b = feature(&s, r.feature.unwrap()).creates[0];
    let (_, hi) = bounds(&s, b);
    assert!(close3(hi, [40.0, 30.0, 10.0]), "{hi:?}");
    let rev = s.summary().revision;
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let r = s.execute(&Command::ImportStep {
            path: step("missing_unit.step"),
            units_to_mm: Some(bad),
        });
        assert!(!r.ok, "units_to_mm {bad} accepted");
        let r = s.execute(&Command::ImportStl {
            path: stl("box_freecad_ascii.stl"),
            units_to_mm: Some(bad),
        });
        assert!(!r.ok, "STL units_to_mm {bad} accepted");
    }
    assert_eq!(s.summary().revision, rev);
}

#[test]
fn reimport_gets_unique_names_and_delete_prunes_embedded_source() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let (f1, a) = import_step(&mut s, "box_40x30x10_mm.step");
    let (f2, b) = import_step(&mut s, "box_40x30x10_mm.step");
    let names = (body(&s, a[0]).name, body(&s, b[0]).name);
    assert_eq!(
        names,
        ("box_40x30x10_mm".into(), "box_40x30x10_mm (2)".into())
    );
    assert_eq!(s.model().unwrap().document().blobs.len(), 2);
    ok(&mut s, Command::DeleteFeature { feature: f2 });
    assert_eq!(s.model().unwrap().document().blobs.len(), 1);
    assert_eq!(s.summary().bodies.len(), 1);
    // The remaining import is still used: deleting it is allowed (no dependents)
    // and leaves an empty, blob-free document.
    ok(&mut s, Command::DeleteFeature { feature: f1 });
    assert!(s.model().unwrap().document().blobs.is_empty());
    assert!(s.summary().bodies.is_empty());
}

#[test]
fn imported_face_with_dependents_cannot_be_deleted() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let (f, b) = import_step(&mut s, "plate_hole_mm.step");
    let top = face(&s, b[0], [0.0, 0.0, 1.0], [0.0, 0.0, 8.0]);
    let sk = circle_on(&mut s, &top, [10.0, 10.0, 8.0], 6.0);
    // Second hole through the imported plate (which already has a cylindrical
    // face): V = 19200 - 200 pi - 72 pi.
    let r = extrude(&mut s, sk, ExtrudeOp::Cut, Some(b[0]), 0.0, true, false);
    assert!(r.ok, "{:?}", r.error);
    assert!(vol_close(volume(&s, b[0]), 19200.0 - 272.0 * PI));
    let rev = s.summary().revision;
    let r = s.execute(&Command::DeleteFeature { feature: f });
    assert!(!r.ok);
    assert_eq!(s.summary().revision, rev);
}

#[test]
fn dxf_contour_onto_imported_face_cuts_the_imported_body() {
    // lwpoly_bulge.dxf (hand-written, tools/fixtures/dxf/README.md): closed
    // LWPOLYLINE with two bulge arcs, bounds (0,0)-(30,20), area 500. Imported
    // onto the FreeCAD box's top face (40 x 30, frame origin = its centroid),
    // recentred on the face origin by an edit (-15, -10) so it lies strictly
    // inside the face, then pocketed 4 mm: V = 12000 - 4 * 500.
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let (_, b) = import_step(&mut s, "box_40x30x10_mm.step");
    let top = face(&s, b[0], [0.0, 0.0, 1.0], [0.0, 0.0, 10.0]);
    let r = ok(
        &mut s,
        Command::ImportDxf {
            path: dxf("lwpoly_bulge.dxf"),
            plane: SketchPlane::Face(top.reference),
            name: None,
            units_to_mm: Some(1.0),
            allow_partial: false,
            layers: None,
        },
    );
    let sk = r.feature.unwrap();
    let mut sketch = feature(&s, sk).sketch.unwrap();
    for p in &mut sketch.points {
        if Some(p.id) != sketch.origin {
            p.x -= 15.0;
            p.y -= 10.0;
        }
    }
    ok(
        &mut s,
        Command::EditSketch {
            feature: sk,
            sketch,
        },
    );
    let r = extrude(&mut s, sk, ExtrudeOp::Cut, Some(b[0]), 4.0, false, false);
    assert!(r.ok, "{:?}", r.error);
    assert_volume(&s, b[0], 12000.0 - 4.0 * 500.0);
    let (lo, hi) = bounds(&s, b[0]);
    assert!(close3(lo, [0.0; 3]) && close3(hi, [40.0, 30.0, 10.0]));
}

#[test]
fn mesh_body_rename_hide_and_explicit_export_selection() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let r = ok(
        &mut s,
        Command::ImportStl {
            path: stl("tet_solid_header.stl"),
            units_to_mm: Some(1.0),
        },
    );
    let m = feature(&s, r.feature.unwrap()).creates[0];
    // Tetrahedron with legs 10: V = 1000 / 6.
    let d = body(&s, m).mesh.unwrap();
    assert!(d.closed && (d.signed_volume.unwrap() - 1000.0 / 6.0).abs() < 1e-9);
    ok(
        &mut s,
        Command::RenameBody {
            body: m,
            name: "Tet".into(),
        },
    );
    ok(
        &mut s,
        Command::SetBodyVisible {
            body: m,
            visible: false,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    // Nothing visible: the default selection is empty and refused...
    let r = s.execute(&Command::ExportStl {
        path: dir.path().join("none.stl"),
        chord: None,
        bodies: None,
        print: Default::default(),
    });
    assert!(!r.ok);
    // ...but an explicit selection exports the hidden mesh body.
    let p = dir.path().join("tet.stl");
    ok(
        &mut s,
        Command::ExportStl {
            path: p.clone(),
            chord: None,
            bodies: Some(vec![m]),
            print: Default::default(),
        },
    );
    assert_eq!(std::fs::metadata(&p).unwrap().len(), 84 + 50 * 4);
    assert_eq!(body(&s, m).name, "Tet");
}

/// Direction semantics (ADR 0003): a blind cut sketched on a face of its target
/// goes into the material by default; `reverse` makes it point away, which is
/// refused with an explicit "clear 'reverse'" diagnostic (not a generic miss).
#[test]
fn blind_cut_from_face_goes_into_material_and_reverse_is_explained() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let (_, b) = import_step(&mut s, "box_40x30x10_mm.step");
    let top = face(&s, b[0], [0.0, 0.0, 1.0], [0.0, 0.0, 10.0]);
    let sk = circle_on(&mut s, &top, [20.0, 15.0, 10.0], 6.0);
    let rev = s.model().unwrap().revision;
    let r = extrude(&mut s, sk, ExtrudeOp::Cut, Some(b[0]), 4.0, false, true);
    assert!(!r.ok);
    let msg = r.error.map(|e| e.to_string()).unwrap_or_default();
    assert!(
        msg.contains("points away") && msg.contains("reverse"),
        "{msg}"
    );
    assert_eq!(s.model().unwrap().revision, rev);
    let r = extrude(&mut s, sk, ExtrudeOp::Cut, Some(b[0]), 4.0, false, false);
    assert!(r.ok, "{:?}", r.error);
}

/// ExportStep's documented default is "all visible B-rep bodies" (lib.rs doc
/// comment on `Command::ExportStep`). With a visible mesh body next to a solid,
/// the default export should write the solid; it currently refuses the whole
/// export because the default selection includes the mesh body.
#[test]
fn export_step_default_skips_mesh_bodies_as_documented() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    import_step(&mut s, "box_40x30x10_mm.step");
    ok(
        &mut s,
        Command::ImportStl {
            path: stl("box_freecad_ascii.stl"),
            units_to_mm: Some(1.0),
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let r = s.execute(&Command::ExportStep {
        path: dir.path().join("mixed.step"),
        bodies: None,
    });
    assert!(r.ok, "{:?}", r.error);
}

#[test]
fn import_commands_round_trip_as_json() {
    let text = r#"[
      {"cmd":"import_step","path":"a.step"},
      {"cmd":"import_step","path":"a.step","units_to_mm":25.4},
      {"cmd":"import_stl","path":"a.stl","units_to_mm":1},
      {"cmd":"import_dxf","path":"a.dxf","plane":"WorldXY"},
      {"cmd":"import_dxf","path":"a.dxf","plane":{"Face":{"feature":1,"role":{"Imported":{"body":1,"face":5}}}},"units_to_mm":1,"allow_partial":true,"layers":["A"]},
      {"cmd":"export_dxf","path":"a.dxf","sketch":2},
      {"cmd":"export_step","path":"a.step","bodies":[1]},
      {"cmd":"export_stl","path":"a.stl","bodies":[1,2]},
      {"cmd":"extrude","sketch":2,"op":"cut","target":1,"through_all":true}
    ]"#;
    let cmds: Vec<Command> = serde_json::from_str(text).unwrap();
    let back: Vec<Command> = serde_json::from_str(&serde_json::to_string(&cmds).unwrap()).unwrap();
    assert_eq!(cmds, back);
}

const INCH: f64 = 25.4;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Replay a committed workflow (`rcad-cli run` format): repository-relative
/// inputs (`crates/...`) resolve against the repository, outputs go to `out`.
fn replay_workflow(name: &str, out: &std::path::Path) -> Session {
    let text = std::fs::read_to_string(repo().join("examples").join(name)).unwrap();
    let mut steps: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    let mut s = Session::new();
    for (i, st) in steps.iter_mut().enumerate() {
        let expect_ok = st
            .as_object_mut()
            .unwrap()
            .remove("expect_ok")
            .is_none_or(|v| v.as_bool().unwrap());
        if let Some(p) = st.get_mut("path") {
            let rel = p.as_str().unwrap().to_string();
            let fixed = if rel.starts_with("crates/") {
                repo().join(&rel)
            } else {
                out.join(std::path::Path::new(&rel).file_name().unwrap())
            };
            *p = serde_json::Value::String(fixed.to_string_lossy().into_owned());
        }
        let cmd: Command = serde_json::from_value(st.clone()).unwrap();
        let r = s.execute(&cmd);
        assert_eq!(r.ok, expect_ok, "{name} step {i} {cmd:?}: {:?}", r.error);
    }
    s
}

fn open_example(file: &str) -> Session {
    let mut e = Session::new();
    ok(
        &mut e,
        Command::Open {
            path: repo().join("examples").join(file),
        },
    );
    e
}

#[test]
fn imported_step_hole_workflow_replays_and_matches_the_example() {
    let dir = tempfile::tempdir().unwrap();
    let s = replay_workflow("imported_step_hole_workflow.json", dir.path());
    // 40 x 30 x 10 box with a centred Ø6 through hole: 12000 - 90 pi.
    assert_volume(&s, BodyId(1), 12000.0 - 90.0 * PI);
    assert!(dir.path().join("imported_step_hole.step").exists());
    // The committed example (saved by `rcad-cli run`) opens without the source
    // file and is the same document with the same geometry.
    let e = open_example("imported_step_hole.rcad.json");
    assert_volume(&e, BodyId(1), 12000.0 - 90.0 * PI);
    assert_eq!(e.model().unwrap().document(), s.model().unwrap().document());
}

#[test]
fn dxf_stl_import_workflow_replays_and_matches_the_example() {
    let dir = tempfile::tempdir().unwrap();
    let s = replay_workflow("dxf_stl_import_workflow.json", dir.path());
    // Notch plate (40 x 20 - 12.5 pi) x 3; the STL box 20 x 10 x 5 read in inches.
    assert_volume(&s, BodyId(1), 3.0 * (800.0 - 12.5 * PI));
    let v = body(&s, BodyId(2)).mesh.unwrap().signed_volume.unwrap();
    let exp = 1000.0 * INCH.powi(3);
    assert!((v - exp).abs() <= 1e-9 * exp, "{v} vs {exp}");
    let e = open_example("dxf_stl_import.rcad.json");
    assert_eq!(e.model().unwrap().document(), s.model().unwrap().document());
    assert_eq!(body(&e, BodyId(2)).mesh, body(&s, BodyId(2)).mesh);
    assert_volume(&e, BodyId(1), 3.0 * (800.0 - 12.5 * PI));
}
