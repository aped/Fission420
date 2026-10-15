//! Extrusion cap roles do not depend on the profile's winding: for a
//! rectangle drawn clockwise or counter-clockwise, the end cap is the face
//! at the extrusion distance and the start cap the one on the sketch plane,
//! and a sketch on the end cap lies on top.
use rcad_commands::{Command, Session, SketchTemplate};
use rcad_document::{FaceRef, FaceRole, FeatureId, SketchPlane};
use rcad_sketch::Sketch;

fn rect(clockwise: bool) -> Sketch {
    let mut pts = vec![[0.0, 0.0], [40.0, 0.0], [40.0, 20.0], [0.0, 20.0]];
    if clockwise {
        pts.reverse();
    }
    let mut s = Sketch::default();
    let p: Vec<_> = pts.iter().map(|q| s.add_point(q[0], q[1])).collect();
    for i in 0..4 {
        s.add_line(p[i], p[(i + 1) % 4]);
    }
    s
}

#[test]
fn end_cap_is_on_top_for_either_winding() {
    for clockwise in [false, true] {
        let mut s = Session::new();
        for c in [
            Command::NewDocument,
            Command::CreateSketch {
                name: None,
                plane: SketchPlane::WorldXY,
                template: SketchTemplate::Custom {
                    sketch: rect(clockwise),
                },
            },
            Command::extrude(None, FeatureId(1), 10.0),
            Command::CreateSketch {
                name: None,
                plane: SketchPlane::Face(FaceRef {
                    feature: FeatureId(2),
                    role: FaceRole::ExtrudeEndCap,
                }),
                template: SketchTemplate::CenteredCircle {
                    diameter: 4.0,
                    center: Some([20.0, 10.0]),
                },
            },
        ] {
            let r = s.execute(&c);
            assert!(r.ok, "{clockwise}: {c:?}: {:?}", r.error);
        }
        let ev = s.model().unwrap().evaluation();
        let f = ev.features.iter().find(|r| r.id == FeatureId(3)).unwrap();
        let frame = f.frame.unwrap();
        assert!(
            (frame.origin[2] - 10.0).abs() < 1e-9,
            "{clockwise}: {frame:?}"
        );
        assert!((frame.z[2] - 1.0).abs() < 1e-9, "{clockwise}: {frame:?}");
        // The face table the viewport maps picks through agrees.
        for c in ev
            .faces
            .iter()
            .filter(|c| c.reference.feature == FeatureId(2))
        {
            let z = c.frame.origin[2];
            match c.reference.role {
                FaceRole::ExtrudeEndCap => {
                    assert!(
                        (z - 10.0).abs() < 1e-9 && c.frame.z[2] > 0.99,
                        "{clockwise}: {c:?}"
                    )
                }
                FaceRole::ExtrudeStartCap => {
                    assert!(z.abs() < 1e-9 && c.frame.z[2] < -0.99, "{clockwise}: {c:?}")
                }
                _ => {}
            }
        }
    }
}
