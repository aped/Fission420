//! Document-level behaviour: regeneration, references, transactions, persistence.
use f420_document::*;
use f420_kernel::VolumeResult;
use f420_sketch::Sketch;
use std::f64::consts::PI;

fn plate(w: f64, h: f64, d: f64, hole: f64, role: FaceRole) -> (Model, [FeatureId; 4]) {
    let mut m = Model::new();
    let s1 = m
        .transact("s1", |doc| {
            Ok(doc.push(
                "S1",
                FeatureKind::Sketch {
                    plane: SketchPlane::WorldXY,
                    sketch: Sketch::centered_rectangle(w, h).unwrap(),
                },
            ))
        })
        .unwrap();
    let e1 = m
        .transact("e1", |doc| {
            Ok(doc.push(
                "E1",
                FeatureKind::Extrude {
                    sketch: s1,
                    distance: d,
                },
            ))
        })
        .unwrap();
    let s2 = m
        .transact("s2", |doc| {
            Ok(doc.push(
                "S2",
                FeatureKind::Sketch {
                    plane: SketchPlane::Face(FaceRef { feature: e1, role }),
                    sketch: Sketch::centered_circle(hole).unwrap(),
                },
            ))
        })
        .unwrap();
    let c1 = m
        .transact("c1", |doc| {
            Ok(doc.push(
                "C1",
                FeatureKind::Cut {
                    sketch: s2,
                    extent: CutExtent::ThroughAll,
                },
            ))
        })
        .unwrap();
    (m, [s1, e1, s2, c1])
}

fn volume(m: &Model) -> f64 {
    match m.evaluation().measurements.as_ref().unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        ref v => panic!("{v:?}"),
    }
}

#[test]
fn start_cap_reference_also_cuts_through() {
    let (m, [_, _, s2, _]) = plate(80.0, 50.0, 10.0, 10.0, FaceRole::ExtrudeStartCap);
    let exp = 40000.0 - 250.0 * PI;
    assert!((volume(&m) - exp).abs() < 0.004, "{}", volume(&m));
    let fr = m.evaluation().features.iter().find(|f| f.id == s2).unwrap();
    let frame = fr.frame.unwrap();
    assert_eq!(frame.origin[2], 0.0);
    assert_eq!(frame.z, [0.0, 0.0, -1.0]);
    assert_eq!(fr.resolved_face.as_ref().unwrap().matching_brep_faces, 1);
}

#[test]
fn failed_edit_rolls_back() {
    let (mut m, [s1, e1, s2, _]) = plate(80.0, 50.0, 10.0, 10.0, FaceRole::ExtrudeEndCap);
    let doc = m.document().clone();
    let rev = m.revision;
    let v = volume(&m);
    assert!(matches!(
        m.set_parameter(e1, "distance", -10.0),
        Err(DocError::Validation(_))
    ));
    assert!(matches!(
        m.set_parameter(e1, "distance", f64::NAN),
        Err(DocError::Validation(_))
    ));
    assert!(matches!(
        m.set_parameter(s2, "diameter", 55.0),
        Err(DocError::ProductLimit(_))
    ));
    assert!(matches!(
        m.set_parameter(s1, "height", 9.0),
        Err(DocError::ProductLimit(_))
    ));
    assert!(matches!(
        m.set_parameter(s1, "depth", 9.0),
        Err(DocError::UnknownParameter { .. })
    ));
    assert!(matches!(
        m.set_parameter(FeatureId(99), "width", 9.0),
        Err(DocError::UnknownFeature(_))
    ));
    assert_eq!(m.document(), &doc);
    assert_eq!(m.revision, rev);
    assert_eq!(volume(&m), v);
}

#[test]
fn undo_redo_restore_semantics() {
    let (mut m, [s1, ..]) = plate(80.0, 50.0, 10.0, 10.0, FaceRole::ExtrudeEndCap);
    let before = m.document().clone();
    m.set_parameter(s1, "width", 100.0).unwrap();
    let after = m.document().clone();
    assert!((volume(&m) - (50000.0 - 250.0 * PI)).abs() < 0.005);
    m.undo().unwrap();
    assert_eq!(m.document(), &before);
    assert!((volume(&m) - (40000.0 - 250.0 * PI)).abs() < 0.004);
    m.redo().unwrap();
    assert_eq!(m.document(), &after);
    // A new edit clears redo.
    m.undo().unwrap();
    m.set_parameter(s1, "height", 40.0).unwrap();
    assert!(matches!(m.redo(), Err(DocError::NothingTo(_))));
    let mut fresh = Model::new();
    assert!(matches!(fresh.undo(), Err(DocError::NothingTo(_))));
}

#[test]
fn broken_and_cyclic_references_are_reported() {
    // Sketch referencing a feature that does not exist.
    let mut d = Document::new();
    d.push(
        "S",
        FeatureKind::Sketch {
            plane: SketchPlane::Face(FaceRef {
                feature: FeatureId(42),
                role: FaceRole::ExtrudeEndCap,
            }),
            sketch: Sketch::centered_circle(5.0).unwrap(),
        },
    );
    assert!(matches!(
        regenerate(&d),
        Err(DocError::BrokenReference { .. })
    ));

    // Face reference to a sketch (not an extrusion).
    let mut d = Document::new();
    let s = d.push(
        "S",
        FeatureKind::Sketch {
            plane: SketchPlane::WorldXY,
            sketch: Sketch::centered_rectangle(10.0, 10.0).unwrap(),
        },
    );
    d.push(
        "S2",
        FeatureKind::Sketch {
            plane: SketchPlane::Face(FaceRef {
                feature: s,
                role: FaceRole::ExtrudeEndCap,
            }),
            sketch: Sketch::centered_circle(5.0).unwrap(),
        },
    );
    assert!(matches!(
        regenerate(&d),
        Err(DocError::BrokenReference { .. })
    ));

    // Cycle: sketch on the end cap of an extrusion of itself.
    let mut d = Document::new();
    d.next_id = 3;
    d.features.push(Feature {
        id: FeatureId(1),
        name: "S".into(),
        kind: FeatureKind::Sketch {
            plane: SketchPlane::Face(FaceRef {
                feature: FeatureId(2),
                role: FaceRole::ExtrudeEndCap,
            }),
            sketch: Sketch::centered_rectangle(10.0, 10.0).unwrap(),
        },
    });
    d.features.push(Feature {
        id: FeatureId(2),
        name: "E".into(),
        kind: FeatureKind::Extrude {
            sketch: FeatureId(1),
            distance: 1.0,
        },
    });
    match regenerate(&d) {
        Err(DocError::Cycle(ids)) => assert_eq!(ids, vec![FeatureId(1), FeatureId(2)]),
        other => panic!("{other:?}"),
    }
}

#[test]
fn regeneration_is_deterministic() {
    let (m, _) = plate(100.0, 50.0, 12.0, 10.0, FaceRole::ExtrudeEndCap);
    let a = regenerate(m.document()).unwrap();
    let b = regenerate(m.document()).unwrap();
    assert_eq!(a.measurements, b.measurements);
    assert_eq!(a.features, b.features);
}

#[test]
fn save_load_round_trip_and_corrupt_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let (m, _) = plate(100.0, 50.0, 10.0, 10.0, FaceRole::ExtrudeEndCap);
    let p = dir.path().join("a.f420.json");
    save_file(&p, m.document(), Some(m.evaluation())).unwrap();
    let nf = load_file(&p).unwrap();
    assert_eq!(&nf.document, m.document());
    assert_eq!(nf.version, FORMAT_VERSION);
    let m2 = Model::from_document(nf.document).unwrap();
    assert_eq!(volume(&m2), volume(&m));

    let write = |name: &str, s: &str| {
        let q = dir.path().join(name);
        std::fs::write(&q, s).unwrap();
        q
    };
    assert!(matches!(
        load_file(&write("x1", "not json")),
        Err(DocError::File(_))
    ));
    assert!(matches!(
        load_file(&write("x2", r#"{"format":"other","version":1}"#)),
        Err(DocError::File(_))
    ));
    assert!(matches!(
        load_file(&write(
            "x3",
            r#"{"format":"fission420","version":99,"document":{}}"#
        )),
        Err(DocError::File(_))
    ));
    assert!(matches!(
        load_file(&write(
            "x4",
            r#"{"format":"fission420","version":1,"document":{"features":"oops"}}"#
        )),
        Err(DocError::File(_))
    ));
    // Structurally valid JSON but with a dangling reference.
    let good = std::fs::read_to_string(&p).unwrap();
    let bad = good.replacen("\"sketch\": 3", "\"sketch\": 77", 1);
    assert_ne!(good, bad);
    assert!(load_file(&write("x5", &bad)).is_err());
    assert!(load_file(&dir.path().join("missing.json")).is_err());

    // Failed save must not clobber the existing file.
    let before = std::fs::read(&p).unwrap();
    assert!(save_file(&dir.path().join("no/such/dir/a.json"), m.document(), None).is_err());
    assert_eq!(std::fs::read(&p).unwrap(), before);
    // No temp files left behind.
    let stray: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
        .collect();
    assert!(stray.is_empty());
}

#[test]
fn second_base_extrusion_is_rejected() {
    let (mut m, [s1, ..]) = plate(80.0, 50.0, 10.0, 10.0, FaceRole::ExtrudeEndCap);
    let doc = m.document().clone();
    let r = m.transact("e2", |d| {
        Ok(d.push(
            "E2",
            FeatureKind::Extrude {
                sketch: s1,
                distance: 5.0,
            },
        ))
    });
    assert!(matches!(r, Err(DocError::ProductLimit(_))));
    assert_eq!(m.document(), &doc);
}

/// Move a template sketch's fixed origin point (the template's anchor) to (x, y).
fn anchored(mut s: Sketch, x: f64, y: f64) -> Sketch {
    for c in &mut s.constraints {
        if let f420_sketch::Constraint::FixedPoint { x: cx, y: cy, .. } = c {
            *cx = x;
            *cy = y;
        }
    }
    s
}

fn doc_with_cut(
    rect: Sketch,
    plane: impl Fn(FeatureId) -> SketchPlane,
    circle: Sketch,
) -> Document {
    let mut d = Document::new();
    let s1 = d.push(
        "S1",
        FeatureKind::Sketch {
            plane: SketchPlane::WorldXY,
            sketch: rect,
        },
    );
    let e1 = d.push(
        "E1",
        FeatureKind::Extrude {
            sketch: s1,
            distance: 5.0,
        },
    );
    let s2 = d.push(
        "S2",
        FeatureKind::Sketch {
            plane: plane(e1),
            sketch: circle,
        },
    );
    d.push(
        "C1",
        FeatureKind::Cut {
            sketch: s2,
            extent: CutExtent::ThroughAll,
        },
    );
    d
}

#[test]
fn review_start_cap_containment_uses_host_frame() {
    // Plate y in [-5, 15]. Start-cap frame has y flipped: local (0, 4) is world (0, -4).
    let rect = || anchored(Sketch::centered_rectangle(40.0, 20.0).unwrap(), 0.0, 5.0);
    let start = |e| {
        SketchPlane::Face(FaceRef {
            feature: e,
            role: FaceRole::ExtrudeStartCap,
        })
    };
    let outside = doc_with_cut(
        rect(),
        start,
        anchored(Sketch::centered_circle(4.0).unwrap(), 0.0, 4.0),
    );
    assert!(matches!(
        regenerate(&outside),
        Err(DocError::ProductLimit(_))
    ));
    let inside = doc_with_cut(
        rect(),
        start,
        anchored(Sketch::centered_circle(4.0).unwrap(), 0.0, -4.0),
    );
    let ev = regenerate(&inside).unwrap();
    let loops = f420_kernel::z_face_inner_loops(ev.body.as_ref().unwrap()).unwrap();
    assert_eq!(loops.len(), 2);
    assert!((loops[0].centroid_xy[1] - 4.0).abs() < 1e-6, "{loops:?}");
}

#[test]
fn review_world_xy_cut_is_also_contained() {
    let xy = |_| SketchPlane::WorldXY;
    let notch = doc_with_cut(
        Sketch::centered_rectangle(40.0, 20.0).unwrap(),
        xy,
        anchored(Sketch::centered_circle(4.0).unwrap(), 20.0, 0.0),
    );
    assert!(matches!(regenerate(&notch), Err(DocError::ProductLimit(_))));
    let tangent = doc_with_cut(
        Sketch::centered_rectangle(40.0, 20.0).unwrap(),
        xy,
        Sketch::centered_circle(20.0).unwrap(),
    );
    assert!(matches!(
        regenerate(&tangent),
        Err(DocError::ProductLimit(_))
    ));
    let ok = doc_with_cut(
        Sketch::centered_rectangle(40.0, 20.0).unwrap(),
        xy,
        Sketch::centered_circle(10.0).unwrap(),
    );
    assert!(regenerate(&ok).is_ok());
}

#[test]
fn review_feature_id_space_does_not_wrap() {
    let mut d = Document::new();
    d.next_id = u32::MAX - 1;
    let sk = || FeatureKind::Sketch {
        plane: SketchPlane::WorldXY,
        sketch: Sketch::centered_rectangle(1.0, 1.0).unwrap(),
    };
    assert_eq!(d.push("a", sk()), FeatureId(u32::MAX - 1));
    assert_eq!(d.next_id, u32::MAX);
    d.push("b", sk());
    assert!(
        d.evaluation_order().is_err(),
        "duplicate id must be rejected, not wrapped"
    );
}

#[test]
fn cached_regeneration_matches_uncached() {
    let (mut m, [s1, e1, s2, _]) = plate(80.0, 50.0, 10.0, 10.0, FaceRole::ExtrudeEndCap);
    let edits: [(FeatureId, &str, f64); 5] = [
        (s1, "width", 100.0),
        (e1, "distance", 12.0),
        (s2, "diameter", 8.0),
        (e1, "distance", 10.0),
        (s1, "width", 80.0),
    ];
    for (f, n, v) in edits {
        m.set_parameter(f, n, v).unwrap();
        let fresh = regenerate(m.document()).unwrap();
        assert_eq!(m.evaluation().features, fresh.features, "after {n}={v}");
        assert_eq!(
            m.evaluation().measurements,
            fresh.measurements,
            "after {n}={v}"
        );
    }
    m.undo().unwrap();
    m.undo().unwrap();
    let fresh = regenerate(m.document()).unwrap();
    assert_eq!(m.evaluation().measurements, fresh.measurements);
    let (hits, _) = m.cache_stats();
    assert!(hits >= 5, "cache was not used: {:?}", m.cache_stats());
}
