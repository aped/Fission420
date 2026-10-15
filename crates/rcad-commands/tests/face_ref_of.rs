//! `Evaluation::face_ref_of`: a picked face (B-rep face index, the display
//! mesh's `tri_face`) maps to its semantic reference. On a box every face maps
//! to its own role (caps by normal, sides by sketch line); a through hole's
//! curved wall maps to nothing; indices out of range map to nothing; after an
//! upstream edit the same physical faces map to the same references.
use rcad_commands::{Command, ExtrudeOp, Session, SketchTemplate};
use rcad_document::{BodyId, FaceRef, FaceRole, SketchPlane};

fn face_normals(s: &Session) -> Vec<Option<[f64; 3]>> {
    let model = s.model().unwrap();
    let b = model
        .evaluation()
        .body_by_id(BodyId(1))
        .unwrap()
        .geometry
        .brep()
        .unwrap();
    (0..rcad_kernel::face_count(b))
        .map(|i| match rcad_kernel::face_geometry(b, i).unwrap() {
            rcad_kernel::DirectFace::Plane { normal, .. } => Some(normal),
            _ => None,
        })
        .collect()
}

/// (outward normal rounded, reference) of every face, sorted by normal.
fn refs_by_normal(s: &Session) -> Vec<([i64; 3], Option<FaceRef>)> {
    let model = s.model().unwrap();
    let ev = model.evaluation();
    let mut v: Vec<_> = face_normals(s)
        .into_iter()
        .enumerate()
        .filter_map(|(i, n)| Some((n?.map(|c| c.round() as i64), ev.face_ref_of(BodyId(1), i))))
        .collect();
    v.sort_by_key(|x| x.0);
    v
}

#[test]
fn picked_faces_map_to_their_references() {
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
    let before = refs_by_normal(&s);
    assert_eq!(before.len(), 6);
    assert!(before.iter().all(|(_, r)| r.is_some()), "{before:?}");
    for (i, a) in before.iter().enumerate() {
        for b in &before[i + 1..] {
            assert_ne!(a.1, b.1, "two faces with one reference");
        }
    }
    let role_of = |n: [i64; 3]| before.iter().find(|x| x.0 == n).unwrap().1.unwrap();
    assert_eq!(
        role_of([0, 0, 1]),
        FaceRef {
            feature: ex,
            role: FaceRole::ExtrudeEndCap
        }
    );
    assert_eq!(
        role_of([0, 0, -1]),
        FaceRef {
            feature: ex,
            role: FaceRole::ExtrudeStartCap
        }
    );
    for n in [[1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0]] {
        assert!(
            matches!(role_of(n).role, FaceRole::ExtrudeSide { .. }),
            "{n:?}: {:?}",
            role_of(n)
        );
    }
    {
        let model = s.model().unwrap();
        assert_eq!(model.evaluation().face_ref_of(BodyId(1), 99), None);
        assert_eq!(model.evaluation().face_ref_of(BodyId(7), 0), None);
    }
    // Upstream edit: the same physical faces keep their references.
    let r = s.execute(&Command::SetParameter {
        feature: ex,
        name: "distance".into(),
        value: 14.0,
    });
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(refs_by_normal(&s), before);
    // A through hole: its curved wall has no reference.
    let hole = s
        .execute(&Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredCircle {
                diameter: 6.0,
                center: Some([20.0, 15.0]),
            },
        })
        .feature
        .unwrap();
    let r = s.execute(&Command::Extrude {
        name: None,
        sketch: hole,
        distance: 0.0,
        op: ExtrudeOp::Cut,
        target: Some(BodyId(1)),
        through_all: true,
        reverse: false,
        body_name: None,
        region: None,
        options: Default::default(),
    });
    assert!(r.ok, "{:?}", r.error);
    let normals = face_normals(&s);
    let model = s.model().unwrap();
    let ev = model.evaluation();
    let curved: Vec<usize> = (0..normals.len())
        .filter(|&i| normals[i].is_none())
        .collect();
    assert!(!curved.is_empty());
    for i in curved {
        assert_eq!(ev.face_ref_of(BodyId(1), i), None, "curved face {i}");
    }
}
