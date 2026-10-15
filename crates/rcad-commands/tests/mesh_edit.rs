//! Mesh edits through the command API (docs/MESH.md): a 40 mm cube STL made of
//! 10 mm cells (192 triangles). Inspection (diagnosis + self-intersection);
//! transform with the inch → mm preset; a two-sided plane cut; decimation with a
//! measured deviation; preview changes nothing; undo/redo; parameter edits and
//! refusals; save (format v8) / reopen. Expected volumes are analytic (cube
//! volume × scale³, slab volumes), not taken from the implementation.
use rcad_commands::mesh_edit::{EditMeshArgs, InspectMeshArgs};
use rcad_commands::{Command, Response, Session};
use rcad_document::BodyId;
use rcad_exchange::mesh_edit::{CutKeep, Decimate, MeshEditOp, MeshTransform, PlaneCut};

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

/// ASCII STL of the cube [0, 40]³ with each face split into 4 × 4 squares.
fn cube_stl() -> String {
    let mut s = String::from("solid cube\n");
    let mut tri = |a: [f64; 3], b: [f64; 3], c: [f64; 3]| {
        s.push_str("facet normal 0 0 0\nouter loop\n");
        for v in [a, b, c] {
            s.push_str(&format!("vertex {} {} {}\n", v[0], v[1], v[2]));
        }
        s.push_str("endloop\nendfacet\n");
    };
    for axis in 0..3 {
        for side in [0.0, 40.0] {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for i in 0..4 {
                for j in 0..4 {
                    let pt = |di: f64, dj: f64| {
                        let mut p = [0.0; 3];
                        p[axis] = side;
                        p[u] = 10.0 * (i as f64 + di);
                        p[v] = 10.0 * (j as f64 + dj);
                        p
                    };
                    let (a, b, c, d) = (pt(0.0, 0.0), pt(1.0, 0.0), pt(1.0, 1.0), pt(0.0, 1.0));
                    if side > 0.0 {
                        tri(a, b, c);
                        tri(a, c, d);
                    } else {
                        tri(a, c, b);
                        tri(a, d, c);
                    }
                }
            }
        }
    }
    s.push_str("endsolid cube\n");
    s
}

fn volume(s: &mut Session, body: u32) -> f64 {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id == BodyId(body))
        .unwrap();
    let d = b.mesh.expect("mesh body");
    assert!(d.closed, "body {body} not closed: {d:?}");
    d.signed_volume.unwrap()
}

fn edit_cmd(body: u32, op: MeshEditOp, preview: bool) -> Command {
    Command::EditMesh(EditMeshArgs {
        body: BodyId(body),
        op,
        preview,
        body_name: None,
        hide_source: false,
    })
}

#[test]
fn inspect_transform_cut_decimate_undo_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let stl = dir.path().join("cube.stl");
    std::fs::write(&stl, cube_stl()).unwrap();
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    ok(
        &mut s,
        Command::ImportStl {
            path: stl,
            units_to_mm: Some(1.0),
        },
    );
    assert!((volume(&mut s, 1) - 64000.0).abs() < 1e-6);

    // Inspection: read only.
    let rev = s.summary().revision;
    let r = ok(
        &mut s,
        Command::InspectMesh(InspectMeshArgs {
            body: BodyId(1),
            limits: None,
        }),
    );
    let ins = r.mesh_inspection.unwrap();
    assert!(ins.self_intersection.complete);
    assert_eq!(ins.self_intersection.intersecting_pairs, 0);
    assert_eq!(ins.diagnostics.duplicate_triangles, 0);
    assert_eq!(ins.diagnostics.component_details.len(), 1);
    assert!(r.message.unwrap().contains("not an exact proof"));
    assert_eq!(s.summary().revision, rev);

    // Transform preview (no change), then apply: inch -> mm.
    let inch = MeshEditOp::Transform(MeshTransform::unit_conversion("inch").unwrap());
    let r = ok(&mut s, edit_cmd(1, inch, true));
    assert_eq!(s.summary().revision, rev);
    let rep = r.mesh_edit.unwrap();
    let want = 64000.0 * 25.4f64.powi(3);
    assert!((rep.after[0].signed_volume.unwrap() - want).abs() < 1e-6 * want);
    ok(&mut s, edit_cmd(1, inch, false));
    assert!((volume(&mut s, 2) - want).abs() < 1e-6 * want);
    assert!((volume(&mut s, 1) - 64000.0).abs() < 1e-6, "source kept");

    // Two-sided plane cut at z = 10: 16000 + 48000.
    let cut = MeshEditOp::PlaneCut(PlaneCut {
        origin: [0.0, 0.0, 10.0],
        normal: [0.0, 0.0, 1.0],
        offset: 0.0,
        keep: CutKeep::Both,
        cap: true,
    });
    let r = ok(&mut s, edit_cmd(1, cut, false));
    let cut_feature = r.feature.unwrap();
    assert!((volume(&mut s, 3) - 16000.0).abs() < 1e-6);
    assert!((volume(&mut s, 4) - 48000.0).abs() < 1e-6);
    let names: Vec<String> = s.summary().bodies.iter().map(|b| b.name.clone()).collect();
    assert_eq!(names.len(), 4, "{names:?}");

    // Decimation: flat faces collapse with zero deviation.
    let dec = MeshEditOp::Decimate(Decimate {
        target_triangles: 0,
        max_deviation: 0.001,
    });
    let r = ok(&mut s, edit_cmd(1, dec, false));
    let rep = r.mesh_edit.unwrap();
    assert!(rep.deviation.as_ref().unwrap().max <= 0.001);
    assert!(rep.after[0].triangles < 80, "{}", rep.after[0].triangles);
    assert!((volume(&mut s, 5) - 64000.0).abs() < 1e-6);
    let dec_feature = r.feature.unwrap();

    // Undo / redo of the decimation.
    ok(&mut s, Command::Undo);
    assert_eq!(s.summary().bodies.len(), 4);
    ok(&mut s, Command::Redo);
    assert_eq!(s.summary().bodies.len(), 5);

    // Parameter edit: the cut plane moves to z = 15.
    ok(
        &mut s,
        Command::SetParameter {
            feature: cut_feature,
            name: "offset".into(),
            value: 5.0,
        },
    );
    assert!((volume(&mut s, 3) - 24000.0).abs() < 1e-6);
    assert!((volume(&mut s, 4) - 40000.0).abs() < 1e-6);
    // Refused edits leave the revision unchanged.
    let rev = s.summary().revision;
    for (f, name, value) in [
        (dec_feature, "max_deviation", 0.0),
        (dec_feature, "target_triangles", 1.5),
        (cut_feature, "angle_deg", 3.0),
    ] {
        let r = s.execute(&Command::SetParameter {
            feature: f,
            name: name.into(),
            value,
        });
        assert!(!r.ok, "{name}");
    }
    // A plane outside the mesh fails regeneration and is rolled back.
    let r = s.execute(&Command::SetParameter {
        feature: cut_feature,
        name: "offset".into(),
        value: 100.0,
    });
    assert!(!r.ok);
    assert_eq!(s.summary().revision, rev);

    // Save (v8) / reopen: the same bodies regenerate.
    let path = dir.path().join("edited.rcad.json");
    ok(&mut s, Command::Save { path: path.clone() });
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], 8);
    let mut t = Session::new();
    ok(
        &mut t,
        Command::Open {
            path,
            safe_mode: false,
        },
    );
    assert_eq!(t.summary().bodies.len(), 5);
    assert!((volume(&mut t, 2) - want).abs() < 1e-6 * want);
    assert!((volume(&mut t, 3) - 24000.0).abs() < 1e-6);
    assert!((volume(&mut t, 5) - 64000.0).abs() < 1e-6);
}

#[test]
fn refusals() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let sk = ok(
        &mut s,
        serde_json::from_value(
            serde_json::json!({"cmd": "create_sketch", "plane": "WorldXY",
            "template": "centered_rectangle", "width": 10.0, "height": 10.0}),
        )
        .unwrap(),
    )
    .feature
    .unwrap();
    ok(&mut s, Command::extrude(None, sk, 5.0));
    let rev = s.summary().revision;
    let t = MeshEditOp::Transform(MeshTransform::default());
    for c in [
        edit_cmd(1, t, false),
        edit_cmd(9, t, false),
        Command::InspectMesh(InspectMeshArgs {
            body: BodyId(1),
            limits: None,
        }),
    ] {
        let r = s.execute(&c);
        assert!(!r.ok, "{c:?}");
    }
    // JSON form of the command (CLI / scripts).
    let c: Command = serde_json::from_value(serde_json::json!({
        "cmd": "edit_mesh", "body": 1,
        "op": {"op": "decimate", "max_deviation": 0.1}, "preview": true
    }))
    .unwrap();
    assert!(!s.execute(&c).ok);
    assert_eq!(s.summary().revision, rev);
}

/// Deleting a mesh edit made with "hide source" shows the source again (the
/// derived body is gone, so the source is the geometry); undo restores both.
#[test]
fn deleting_a_hiding_edit_shows_the_source_again() {
    let dir = tempfile::tempdir().unwrap();
    let stl = dir.path().join("cube.stl");
    std::fs::write(&stl, cube_stl()).unwrap();
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    ok(
        &mut s,
        Command::ImportStl {
            path: stl,
            units_to_mm: Some(1.0),
        },
    );
    let visible = |s: &Session, id: u32| {
        s.summary()
            .bodies
            .iter()
            .find(|b| b.id == BodyId(id))
            .map(|b| b.visible)
    };
    let r = ok(
        &mut s,
        Command::EditMesh(EditMeshArgs {
            body: BodyId(1),
            op: MeshEditOp::Transform(MeshTransform {
                scale: [2.0; 3],
                ..Default::default()
            }),
            preview: false,
            body_name: None,
            hide_source: true,
        }),
    );
    let feature = r.feature.unwrap();
    assert_eq!(visible(&s, 1), Some(false));
    assert_eq!(visible(&s, 2), Some(true));
    ok(&mut s, Command::DeleteFeature { feature });
    assert_eq!(visible(&s, 1), Some(true), "source shown again");
    assert_eq!(visible(&s, 2), None, "derived body gone");
    ok(&mut s, Command::Undo);
    assert_eq!(visible(&s, 1), Some(false));
    assert_eq!(visible(&s, 2), Some(true));
}
