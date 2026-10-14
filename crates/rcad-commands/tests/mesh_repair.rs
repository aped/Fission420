//! Mesh repair (docs/MESH.md): an ASCII STL of
//! a 20 mm cube with the top face missing, one side triangle flipped, a
//! duplicate triangle and a degenerate sliver. Diagnosis first; preview changes
//! nothing; apply makes a new mesh body (the source is kept) that is closed and
//! outward with the analytic volume 8000 mm³; undo/redo; the weld tolerance is
//! an editable parameter; save/reopen (v7); exact bodies and bad options are
//! refused.
use rcad_commands::mesh_repair::RepairMeshArgs;
use rcad_commands::{Command, Response, Session};
use rcad_document::BodyId;
use rcad_exchange::mesh_repair::RepairOptions;

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

/// Cube [0, 20]³ as outward triangles, minus the two top triangles; side
/// triangle 6 flipped; triangle 0 repeated; a zero-area sliver on an edge.
fn broken_cube_stl() -> String {
    let p = [
        [0.0, 0.0, 0.0],
        [20.0, 0.0, 0.0],
        [20.0, 20.0, 0.0],
        [0.0, 20.0, 0.0],
        [0.0, 0.0, 20.0],
        [20.0, 0.0, 20.0],
        [20.0, 20.0, 20.0],
        [0.0, 20.0, 20.0],
    ];
    let mut t: Vec<[usize; 3]> = vec![
        [0, 2, 1],
        [0, 3, 2],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ];
    t[6] = [2, 7, 3]; // flipped
    t.push([0, 2, 1]); // duplicate
    t.push([0, 1, 1]); // degenerate (repeated corner)
    let mut s = String::from("solid broken\n");
    for tr in t {
        s.push_str("  facet normal 0 0 0\n    outer loop\n");
        for i in tr {
            let v = p[i];
            s.push_str(&format!("      vertex {} {} {}\n", v[0], v[1], v[2]));
        }
        s.push_str("    endloop\n  endfacet\n");
    }
    s.push_str("endsolid broken\n");
    s
}

fn mesh_volume(s: &mut Session, body: u32) -> f64 {
    let r = ok(
        s,
        serde_json::from_value(serde_json::json!({"cmd": "mass_properties", "bodies": [body]}))
            .unwrap(),
    );
    r.mass_properties.unwrap().bodies[0]
        .properties
        .as_ref()
        .expect("mesh volume")
        .volume
}

#[test]
fn diagnose_preview_apply_undo_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let stl = dir.path().join("broken_cube.stl");
    std::fs::write(&stl, broken_cube_stl()).unwrap();
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    ok(
        &mut s,
        Command::ImportStl {
            path: stl,
            units_to_mm: Some(1.0),
        },
    );
    let d = s.summary().bodies[0]
        .mesh
        .clone()
        .expect("mesh diagnostics");
    assert!(!d.closed);
    assert!(d.boundary_edges > 0 && d.degenerate_triangles == 1, "{d:?}");
    let opts = RepairOptions {
        fill_holes_max_edges: 8,
        ..Default::default()
    };
    // Preview: report, no change.
    let rev = s.summary().revision;
    let r = ok(
        &mut s,
        Command::RepairMesh(RepairMeshArgs {
            body: BodyId(1),
            options: opts,
            preview: true,
            body_name: None,
            hide_source: false,
        }),
    );
    assert_eq!(s.summary().revision, rev);
    let rep = r.mesh_repair.unwrap();
    assert_eq!(rep.removed_degenerate, 1);
    assert_eq!(rep.removed_duplicates, 1);
    assert_eq!(rep.flipped_triangles, 1);
    assert_eq!((rep.filled_holes, rep.fill_triangles), (1, 2));
    assert!(rep.after.closed && !rep.before.closed);
    assert!((rep.after.signed_volume.unwrap() - 8000.0).abs() < 1e-9);
    // Apply: new body 2; body 1 kept, unchanged.
    ok(
        &mut s,
        Command::RepairMesh(RepairMeshArgs {
            body: BodyId(1),
            options: opts,
            preview: false,
            body_name: Some("Cube fixed".into()),
            hide_source: true,
        }),
    );
    let sum = s.summary();
    assert_eq!(sum.bodies.len(), 2);
    assert!(!sum.bodies[0].visible);
    assert!(!sum.bodies[0].mesh.as_ref().unwrap().closed);
    let fixed = sum.bodies[1].mesh.clone().unwrap();
    assert!(fixed.closed, "{fixed:?}");
    assert_eq!(fixed.triangles, 12);
    assert!((mesh_volume(&mut s, 2) - 8000.0).abs() < 1e-9);
    // Undo / redo.
    ok(&mut s, Command::Undo);
    assert_eq!(s.summary().bodies.len(), 1);
    ok(&mut s, Command::Redo);
    assert_eq!(s.summary().bodies.len(), 2);
    // The weld tolerance is a parameter of the feature; invalid values refused.
    let f = s.summary().features.last().unwrap().id;
    ok(
        &mut s,
        Command::SetParameter {
            feature: f,
            name: "weld_tolerance".into(),
            value: 0.01,
        },
    );
    let rev = s.summary().revision;
    let r = s.execute(&Command::SetParameter {
        feature: f,
        name: "weld_tolerance".into(),
        value: 5.0,
    });
    assert!(!r.ok && s.summary().revision == rev);
    // Save / reopen (format v7): regenerates the same repaired mesh.
    let path = dir.path().join("repaired.rcad.json");
    ok(&mut s, Command::Save { path: path.clone() });
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], 7);
    let mut t = Session::new();
    ok(&mut t, Command::Open { path });
    assert!(t.summary().bodies[1].mesh.as_ref().unwrap().closed);
    assert!((mesh_volume(&mut t, 2) - 8000.0).abs() < 1e-9);
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
    for (body, opts) in [
        (BodyId(1), RepairOptions::default()),
        (BodyId(9), RepairOptions::default()),
    ] {
        let r = s.execute(&Command::RepairMesh(RepairMeshArgs {
            body,
            options: opts,
            preview: false,
            body_name: None,
            hide_source: false,
        }));
        assert!(!r.ok, "{body:?}");
    }
    let r = s.execute(&Command::RepairMesh(RepairMeshArgs {
        body: BodyId(1),
        options: RepairOptions {
            weld_tolerance: -1.0,
            ..Default::default()
        },
        preview: true,
        body_name: None,
        hide_source: false,
    }));
    assert!(!r.ok);
    assert_eq!(s.summary().revision, rev);
}
