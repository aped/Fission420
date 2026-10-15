//! Edge and vertex picking bound to semantic references (ADR 0015, stage 1
//! display side).
//!
//! The kernel's edge and vertex tables (`rcad_kernel::edge_polylines`,
//! `rcad_kernel::vertex_points`) key each edge by the provisional indices of
//! its two faces and each vertex by the faces around it. Here those faces are
//! mapped to their `FaceRef`s (feature + role) through the document's
//! referenceable faces, so an edge's reference is "the edge between these two
//! roles" and a corner's "the corner of these three roles". The viewport picks
//! them through its BVH with a pixel tolerance. On a box the 12 edges and 8
//! corners get distinct references, and the same physical edges and corners
//! keep the same references after the box's length and height are edited.
use rcad_commands::{Command, Session, SketchTemplate};
use rcad_document::{BodyId, FaceRef, SketchPlane};
use rcad_viewport::{Pick, PickEdge, PickScene, PickVertex, PixelTolerance, Ray, RenderMesh};

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// A semantic reference: the sorted face references (Debug text) of the
/// faces an edge or corner lies on, plus the edge component.
type Key = (Vec<String>, u32);

/// The pickable scene of body 1 with the semantic key of every edge and
/// vertex (indexed by their pick ids).
struct Scene {
    scene: PickScene,
    edge_keys: Vec<Key>,
    vertex_keys: Vec<Key>,
    xmin: f64,
    xmax: f64,
}

fn scene(s: &Session) -> Scene {
    let model = s.model().unwrap();
    let ev = model.evaluation();
    let body = ev.body_by_id(BodyId(1)).unwrap().geometry.brep().unwrap();
    let mesh = rcad_kernel::tessellate_display(body, 0.05).unwrap();
    let edges = rcad_kernel::edge_polylines(body, 0.05).unwrap();
    let verts = rcad_kernel::vertex_points(body).unwrap();
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
    let key = |faces: &[u32], component: u32| -> Key {
        let mut v: Vec<String> = faces.iter().map(|&f| format!("{:?}", role(f))).collect();
        v.sort();
        v.dedup();
        (v, component)
    };
    let edge_keys = edges.iter().map(|e| key(&e.faces, e.component)).collect();
    let vertex_keys = verts.iter().map(|v| key(&v.faces, 0)).collect();
    let rm = RenderMesh {
        positions: mesh.positions.iter().map(|p| p.map(|c| c as f32)).collect(),
        normals: Vec::new(),
        indices: mesh.triangles.iter().flatten().copied().collect(),
        tri_group: mesh.tri_face.clone(),
    };
    let pick_edges: Vec<PickEdge> = edges
        .iter()
        .enumerate()
        .map(|(i, e)| PickEdge {
            id: i as u32,
            points: e.points.iter().map(|p| p.map(|c| c as f32)).collect(),
        })
        .collect();
    let pick_verts: Vec<PickVertex> = verts
        .iter()
        .enumerate()
        .map(|(i, v)| PickVertex {
            id: i as u32,
            pos: v.position.map(|c| c as f32),
        })
        .collect();
    let xs = mesh.positions.iter().map(|p| p[0]);
    Scene {
        scene: PickScene::new(&rm, &pick_edges, &pick_verts).unwrap(),
        edge_keys,
        vertex_keys,
        xmin: xs.clone().fold(f64::INFINITY, f64::min),
        xmax: xs.fold(f64::NEG_INFINITY, f64::max),
    }
}

/// Pick at `at`, viewed from 200 mm outside the box along `at - centre`.
fn pick_at(sc: &Scene, at: [f64; 3], centre: [f64; 3]) -> Option<Pick> {
    let out = sub(at, centre);
    let l = dot(out, out).sqrt();
    let eye = [0, 1, 2].map(|k| at[k] + 200.0 * out[k] / l);
    let dir = sub(at, eye).map(|c| c / 200.0);
    let ray = Ray {
        origin: eye.map(|c| c as f32),
        dir: dir.map(|c| c as f32),
    };
    // 5 px at a 0.8 rad field of view in an 800 px viewport.
    sc.scene
        .pick(&ray, PixelTolerance::perspective(5.0, 0.8, 800.0))
}

fn edge_at(sc: &Scene, at: [f64; 3], centre: [f64; 3]) -> Key {
    match pick_at(sc, at, centre) {
        Some(Pick::Edge { id, .. }) => sc.edge_keys[id as usize].clone(),
        other => panic!("expected an edge at {at:?}, got {other:?}"),
    }
}

fn vertex_at(sc: &Scene, at: [f64; 3], centre: [f64; 3]) -> Key {
    match pick_at(sc, at, centre) {
        Some(Pick::Vertex { id, .. }) => sc.vertex_keys[id as usize].clone(),
        other => panic!("expected a vertex at {at:?}, got {other:?}"),
    }
}

fn distinct(keys: &[Key]) -> Vec<Key> {
    let mut k = keys.to_vec();
    k.sort();
    k.dedup();
    k
}

/// A 40 x 30 x 10 box at the origin; returns the session, sketch and extrude.
fn box_doc() -> (Session, rcad_document::FeatureId, rcad_document::FeatureId) {
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
    (s, sk, ex)
}

/// Length (sketch width 40 -> 60, centred) and height (10 -> 16) edit.
fn edit(s: &mut Session, sk: rcad_document::FeatureId, ex: rcad_document::FeatureId) {
    for (feature, name, value) in [(ex, "distance", 16.0), (sk, "width", 60.0)] {
        let r = s.execute(&Command::SetParameter {
            feature,
            name: name.into(),
            value,
        });
        assert!(r.ok, "{name}: {:?}", r.error);
    }
}

#[test]
fn pick_edge_returns_semantic_ref() {
    let (mut s, sk, ex) = box_doc();
    let before = scene(&s);
    assert_eq!(before.edge_keys.len(), 12);
    let keys = distinct(&before.edge_keys);
    assert_eq!(keys.len(), 12, "distinct edge references");
    assert!(keys.iter().all(|k| k.0.len() == 2), "{keys:?}");
    // Top front edge (y = 0, z = 10) and a vertical edge (x = 40, y = 30).
    let c0 = [20.0, 15.0, 5.0];
    let top_front = edge_at(&before, [20.0, 0.0, 10.0], c0);
    let vertical = edge_at(&before, [40.0, 30.0, 5.0], c0);
    assert_ne!(top_front, vertical);
    edit(&mut s, sk, ex);
    let after = scene(&s);
    assert_eq!(
        distinct(&after.edge_keys),
        keys,
        "the same 12 references after the edit"
    );
    // The same physical edges, now elsewhere, give the same references.
    assert!((after.xmax - after.xmin - 60.0).abs() < 1e-6);
    let mid_x = 0.5 * (after.xmin + after.xmax);
    let c1 = [mid_x, 15.0, 8.0];
    assert_eq!(edge_at(&after, [mid_x, 0.0, 16.0], c1), top_front);
    assert_eq!(edge_at(&after, [after.xmax, 30.0, 8.0], c1), vertical);
}

#[test]
fn pick_vertex_returns_semantic_ref() {
    let (mut s, sk, ex) = box_doc();
    let before = scene(&s);
    assert_eq!(before.vertex_keys.len(), 8);
    let keys = distinct(&before.vertex_keys);
    assert_eq!(keys.len(), 8, "distinct corner references");
    assert!(keys.iter().all(|k| k.0.len() == 3), "{keys:?}");
    let c0 = [20.0, 15.0, 5.0];
    let top_corner = vertex_at(&before, [40.0, 30.0, 10.0], c0);
    let bottom_corner = vertex_at(&before, [0.0, 0.0, 0.0], c0);
    assert_ne!(top_corner, bottom_corner);
    edit(&mut s, sk, ex);
    let after = scene(&s);
    assert_eq!(distinct(&after.vertex_keys), keys);
    let c1 = [0.5 * (after.xmin + after.xmax), 15.0, 8.0];
    assert_eq!(vertex_at(&after, [after.xmax, 30.0, 16.0], c1), top_corner);
    assert_eq!(vertex_at(&after, [after.xmin, 0.0, 0.0], c1), bottom_corner);
}
