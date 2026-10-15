//! A committed mesh edit computes the edit once: the command's report and the
//! regeneration of the new feature share one result
//! (`rcad_document::mesh_edit::edit_cached`). Counted with the process-wide
//! counter of computed edits, so this file holds a single test.
use rcad_commands::mesh_edit::EditMeshArgs;
use rcad_commands::{Command, Session};
use rcad_document::BodyId;
use rcad_document::mesh_edit::edit_runs;
use rcad_exchange::mesh_edit::{Decimate, MeshEditOp};

/// ASCII STL of the cube [0, 20]³, each face split into 2 x 2 squares.
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
        for side in [0.0, 20.0] {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for i in 0..2 {
                for j in 0..2 {
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

#[test]
fn a_committed_decimation_runs_once() {
    let dir = tempfile::tempdir().unwrap();
    let stl = dir.path().join("cube.stl");
    std::fs::write(&stl, cube_stl()).unwrap();
    let mut s = Session::new();
    assert!(s.execute(&Command::NewDocument).ok);
    let r = s.execute(&Command::ImportStl {
        path: stl,
        units_to_mm: Some(1.0),
    });
    assert!(r.ok, "{:?}", r.error);
    let dec = |preview| {
        Command::EditMesh(EditMeshArgs {
            body: BodyId(1),
            op: MeshEditOp::Decimate(Decimate {
                target_triangles: 0,
                max_deviation: 0.001,
            }),
            preview,
            body_name: None,
            hide_source: false,
        })
    };
    let before = edit_runs();
    let r = s.execute(&dec(false));
    assert!(r.ok, "{:?}", r.error);
    assert!(r.mesh_edit.is_some());
    assert_eq!(edit_runs() - before, 1, "command + regeneration: one edit");
    // A preview then the same edit committed: still one computation.
    let r = s.execute(&Command::Undo);
    assert!(r.ok);
    let before = edit_runs();
    assert!(s.execute(&dec(true)).ok);
    assert!(s.execute(&dec(false)).ok);
    assert_eq!(edit_runs() - before, 0, "served from the earlier result");
}
