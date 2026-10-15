//! Edge picking bound to semantic references (ADR 0015, stage 1 display side).
//!
//! The kernel's edge table (`rcad_kernel::edge_polylines`) keys each edge by
//! the provisional indices of its two faces; here those faces are mapped to
//! their `FaceRef`s (feature + role) through the document's referenceable
//! faces, so an edge's reference is "the edge between these two roles". The
//! viewport picks the edge through its BVH with a pixel tolerance. On a box
//! the 12 edges get 12 distinct references, and the same physical edges keep
//! the same references after the box's length and height are edited.
use rcad_commands::{Command, Session, SketchTemplate};
use rcad_document::{BodyId, FaceRef, SketchPlane};
use rcad_viewport::{Pick, PickEdge, PickScene, PixelTolerance, Ray, RenderMesh};

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// For every edge of body 1: (pair of face references, component, polyline).
type EdgeRefs = Vec<((String, String), u32, Vec<[f64; 3]>)>;

/// Edge references of body 1, and the display mesh with picking edges.
fn edges(s: &Session) -> (EdgeRefs, RenderMesh, Vec<PickEdge>) {
    let model = s.model().unwrap();
    let ev = model.evaluation();
    let body = ev.body_by_id(BodyId(1)).unwrap().geometry.brep().unwrap();
    let mesh = rcad_kernel::tessellate_display(body, 0.05).unwrap();
    let table = rcad_kernel::edge_polylines(body, 0.05).unwrap();
    // Provisional face index -> FaceRef: the unique referenceable face whose
    // plane and outward normal hold that face's triangles.
    let role = |fi: u32| -> FaceRef {
        let t = mesh
            .tri_face
            .iter()
            .position(|&f| f == fi)
            .expect("face has triangles");
        let [a, b, c] = mesh.triangles[t].map(|i| mesh.positions[i as usize]);
        let (u, v) = (sub(b, a), sub(c, a));
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let l = dot(n, n).sqrt();
        let n = n.map(|x| x / l);
        let hits: Vec<&rcad_document::FaceCandidate> = ev
            .faces
            .iter()
            .filter(|f| {
                f.body == BodyId(1)
                    && dot(f.frame.z, n) > 1.0 - 1e-9
                    && dot(sub(a, f.frame.origin), f.frame.z).abs() < 1e-6
            })
            .collect();
        assert_eq!(hits.len(), 1, "face {fi}: {hits:?}");
        hits[0].reference
    };
    let refs: EdgeRefs = table
        .iter()
        .map(|e| {
            let mut pair = [
                format!("{:?}", role(e.faces[0])),
                format!("{:?}", role(e.faces[1])),
            ];
            pair.sort();
            let [a, b] = pair;
            ((a, b), e.component, e.points.clone())
        })
        .collect();
    let rm = RenderMesh {
        positions: mesh.positions.iter().map(|p| p.map(|c| c as f32)).collect(),
        normals: Vec::new(),
        indices: mesh.triangles.iter().flatten().copied().collect(),
        tri_group: mesh.tri_face.clone(),
    };
    let pick_edges = table
        .iter()
        .enumerate()
        .map(|(i, e)| PickEdge {
            id: i as u32,
            points: e.points.iter().map(|p| p.map(|c| c as f32)).collect(),
        })
        .collect();
    (refs, rm, pick_edges)
}

/// Pick the edge whose midpoint is nearest `at`, viewed from outside the box
/// along the bisector of its two faces; returns its reference.
fn pick_ref(s: &Session, at: [f64; 3], centre: [f64; 3]) -> (String, String) {
    let (refs, rm, pe) = edges(s);
    let scene = PickScene::new(&rm, &pe, &[]).unwrap();
    let out = sub(at, centre);
    let l = dot(out, out).sqrt();
    let eye = [0, 1, 2].map(|k| at[k] + 200.0 * out[k] / l);
    let dir = sub(at, eye).map(|c| c / 200.0);
    let ray = Ray {
        origin: eye.map(|c| c as f32),
        dir: dir.map(|c| c as f32),
    };
    // 5 px at a 0.8 rad field of view in an 800 px viewport.
    let tol = PixelTolerance::perspective(5.0, 0.8, 800.0);
    match scene.pick(&ray, tol) {
        Some(Pick::Edge { id, .. }) => refs[id as usize].0.clone(),
        other => panic!("expected an edge at {at:?}, got {other:?}"),
    }
}

#[test]
fn pick_edge_returns_semantic_ref() {
    let mut s = Session::new();
    s.execute(&Command::NewDocument);
    let sk = s
        .execute(&Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 40.0,
                height: 30.0,
                center: Some([20.0, 15.0]),
            },
        })
        .feature
        .unwrap();
    let ex = s
        .execute(&Command::extrude(None, sk, 10.0))
        .feature
        .unwrap();
    let (before, _, _) = edges(&s);
    assert_eq!(before.len(), 12);
    let mut keys: Vec<_> = before.iter().map(|e| (e.0.clone(), e.1)).collect();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), 12, "distinct edge references");
    // Top front edge (y = 0, z = 10) and a vertical edge (x = 40, y = 30).
    let top_front = pick_ref(&s, [20.0, 0.0, 10.0], [20.0, 15.0, 5.0]);
    let vertical = pick_ref(&s, [40.0, 30.0, 5.0], [20.0, 15.0, 5.0]);
    assert_ne!(top_front, vertical);
    // Edit length (sketch width) and height (extrude distance).
    let r = s.execute(&Command::SetParameter {
        feature: ex,
        name: "distance".into(),
        value: 16.0,
    });
    assert!(r.ok, "{:?}", r.error);
    let r = s.execute(&Command::SetParameter {
        feature: sk,
        name: "width".into(),
        value: 60.0,
    });
    assert!(r.ok, "{:?}", r.error);
    let (after, _, _) = edges(&s);
    let mut keys_after: Vec<_> = after.iter().map(|e| (e.0.clone(), e.1)).collect();
    keys_after.sort();
    keys_after.dedup();
    assert_eq!(keys_after, keys, "the same 12 references after the edit");
    // The same physical edges, now elsewhere, give the same references.
    let centre = [20.0, 15.0, 8.0];
    let bbox_x = after
        .iter()
        .flat_map(|e| e.2.iter().map(|p| p[0]))
        .fold(f64::NEG_INFINITY, f64::max);
    let bbox_x0 = after
        .iter()
        .flat_map(|e| e.2.iter().map(|p| p[0]))
        .fold(f64::INFINITY, f64::min);
    let mid_x = 0.5 * (bbox_x + bbox_x0);
    assert!(
        (bbox_x - bbox_x0 - 60.0).abs() < 1e-6,
        "{bbox_x0}..{bbox_x}"
    );
    let c = [mid_x, centre[1], centre[2]];
    assert_eq!(pick_ref(&s, [mid_x, 0.0, 16.0], c), top_front);
    assert_eq!(pick_ref(&s, [bbox_x, 30.0, 8.0], c), vertical);
}
