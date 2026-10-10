//! Run-2 document behaviour: editor sketches (origin point, free vertices),
//! optional profiles, dimension edits storing solved geometry, format v2
//! migration and float round-trip exactness.
use f420_document::*;
use f420_kernel::VolumeResult;
use f420_sketch::{Constraint, Dimension, PointId, Sketch};
use std::path::PathBuf;

/// Editor-style centred rectangle: origin + 4 free corners, H/V, symmetric
/// about the origin, width/height dimensions. Corner seeds are deliberately
/// off so the stored definition only becomes exact after a solve.
fn editor_rect(w: f64, h: f64) -> (Sketch, [PointId; 4]) {
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let c = [
        s.add_point(-w / 2.0 + 0.3, -h / 2.0 - 0.2),
        s.add_point(w / 2.0 - 0.1, -h / 2.0 + 0.4),
        s.add_point(w / 2.0 + 0.2, h / 2.0),
        s.add_point(-w / 2.0, h / 2.0 + 0.1),
    ];
    let l = [
        s.add_line(c[0], c[1]),
        s.add_line(c[1], c[2]),
        s.add_line(c[2], c[3]),
        s.add_line(c[3], c[0]),
    ];
    s.constraints.extend([
        Constraint::Horizontal { line: l[0] },
        Constraint::Horizontal { line: l[2] },
        Constraint::Vertical { line: l[1] },
        Constraint::Vertical { line: l[3] },
        Constraint::SymmetricAboutPoint {
            a: c[0],
            b: c[2],
            center: o,
        },
        Constraint::HorizontalDistance {
            a: c[0],
            b: c[1],
            dim: Dimension::new("width", w),
        },
        Constraint::VerticalDistance {
            a: c[0],
            b: c[3],
            dim: Dimension::new("height", h),
        },
    ]);
    (s, c)
}

fn add_sketch(m: &mut Model, s: Sketch) -> FeatureId {
    m.transact("sketch", |d| {
        Ok(d.push(
            "S",
            FeatureKind::Sketch {
                plane: SketchPlane::WorldXY,
                sketch: s,
            },
        ))
    })
    .unwrap()
}

fn stored_sketch(m: &Model, id: FeatureId) -> Sketch {
    match &m.document().feature(id).unwrap().kind {
        FeatureKind::Sketch { sketch, .. } => sketch.clone(),
        k => panic!("not a sketch: {k:?}"),
    }
}

fn volume(m: &Model) -> f64 {
    match m.evaluation().measurements.as_ref().unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        ref v => panic!("{v:?}"),
    }
}

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

#[test]
fn open_profile_sketch_regenerates_but_cannot_be_extruded() {
    let mut m = Model::new();
    // Open polyline: three lines, not closed.
    let mut s = Sketch::default();
    s.ensure_origin();
    let p: Vec<_> = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
        .iter()
        .map(|v| s.add_point(v[0], v[1]))
        .collect();
    for i in 0..3 {
        s.add_line(p[i], p[i + 1]);
    }
    let sid = add_sketch(&mut m, s);
    let r = m
        .evaluation()
        .features
        .iter()
        .find(|f| f.id == sid)
        .unwrap();
    assert!(r.profile_error.is_some(), "open loop must be reported");
    assert!(r.solve.as_ref().unwrap().is_success());

    let (doc, rev) = (m.document().clone(), m.revision);
    let err = m
        .transact("extrude", |d| {
            Ok(d.push(
                "E",
                FeatureKind::Extrude {
                    sketch: sid,
                    distance: 5.0,
                },
            ))
        })
        .unwrap_err();
    assert!(
        matches!(err, DocError::Validation(ref t) if t.contains("no usable profile")),
        "{err:?}"
    );
    assert_eq!(m.document(), &doc, "rolled back");
    assert_eq!(m.revision, rev);
    assert!(m.evaluation().body.is_none());
    assert_eq!(m.undo_label(), Some("sketch"));
}

#[test]
fn dimension_edit_stores_solved_coordinates() {
    let mut m = Model::new();
    let (s, c) = editor_rect(80.0, 50.0);
    let sid = add_sketch(&mut m, s);
    m.set_parameter(sid, "width", 100.0).unwrap();
    let stored = stored_sketch(&m, sid);
    let tol = f420_sketch::solve_tolerance(stored.scale());
    let res = stored.residual().unwrap();
    assert!(res <= tol, "definition residual {res:e} > {tol:e}");
    // Centred 100 x 50: corners at (+-50, +-25).
    let want = [[-50.0, -25.0], [50.0, -25.0], [50.0, 25.0], [-50.0, 25.0]];
    for (p, w) in c.iter().zip(want) {
        let q = stored.point_xy(*p).unwrap();
        assert!(
            (q[0] - w[0]).abs() < 1e-8 && (q[1] - w[1]).abs() < 1e-8,
            "{q:?} vs {w:?}"
        );
    }
    // Editor sketches expose no center_x/center_y and must not move the origin.
    assert!(
        !m.document()
            .feature(sid)
            .unwrap()
            .parameters()
            .iter()
            .any(|(n, _)| n == "center_x")
    );
    let before = m.document().clone();
    assert!(matches!(
        m.set_parameter(sid, "center_x", 5.0),
        Err(DocError::UnknownParameter { .. })
    ));
    assert_eq!(m.document(), &before);
}

#[test]
fn conflicting_dimension_edit_rolls_back_and_names_culprits() {
    let mut m = Model::new();
    let (mut s, c) = editor_rect(80.0, 50.0);
    // Diagonal of the 80 x 50 rectangle: sqrt(80^2 + 50^2).
    let diag = (80.0f64 * 80.0 + 50.0 * 50.0).sqrt();
    s.constraints.push(Constraint::Distance {
        a: c[0],
        b: c[2],
        dim: Dimension::new("diag", diag),
    });
    let sid = add_sketch(&mut m, s);
    let (doc, rev) = (m.document().clone(), m.revision);
    // width 100 contradicts the fixed diagonal with height 50.
    let err = m.set_parameter(sid, "width", 100.0).unwrap_err();
    match &err {
        DocError::SketchSolve { feature, detail } => {
            assert_eq!(*feature, sid);
            assert!(
                detail.contains("width") || detail.contains("diag"),
                "{detail}"
            );
        }
        e => panic!("{e:?}"),
    }
    assert_eq!(m.document(), &doc);
    assert_eq!(m.revision, rev);
}

#[test]
fn v1_example_loads_saves_as_v2_and_reloads_equal() {
    let src = examples_dir().join("plate.f420.json");
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&src).unwrap()).unwrap();
    assert_eq!(raw["version"], 1, "fixture must stay a v1 file");
    let nf = load_file(&src).unwrap();
    assert_eq!(nf.version, 1);
    let m = Model::from_document(nf.document.clone()).unwrap();
    // v1 template sketches have no origin and keep their center parameters.
    for f in &m.document().features {
        if let FeatureKind::Sketch { sketch, .. } = &f.kind {
            assert_eq!(sketch.origin, None);
            assert!(f.parameters().iter().any(|(n, _)| n == "center_x"));
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("plate_v2.f420.json");
    save_file(&out, m.document(), Some(m.evaluation())).unwrap();
    let raw2: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(raw2["version"], FORMAT_VERSION);
    assert_eq!(FORMAT_VERSION, 2);
    let nf2 = load_file(&out).unwrap();
    assert_eq!(nf2.document, nf.document);
    let m2 = Model::from_document(nf2.document).unwrap();
    assert_eq!(volume(&m2), volume(&m));
}

#[test]
fn newer_format_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("v3.f420.json");
    let doc = Document::new();
    let mut nf = NativeFile::new(&doc, None);
    nf.version = 3;
    std::fs::write(&p, serde_json::to_vec(&nf).unwrap()).unwrap();
    match load_file(&p) {
        Err(DocError::File(t)) => assert!(t.contains("newer"), "{t}"),
        r => panic!("{r:?}"),
    }
    nf.version = 0;
    std::fs::write(&p, serde_json::to_vec(&nf).unwrap()).unwrap();
    assert!(matches!(load_file(&p), Err(DocError::File(_))));
}

#[test]
fn stored_solved_coordinates_round_trip_bit_exactly() {
    let mut m = Model::new();
    // Dimensions with non-terminating binary expansions so the solved
    // coordinates are "ugly" doubles.
    let (s, c) = editor_rect(100.0 / 3.0, 0.1 + 0.2);
    let sid = add_sketch(&mut m, s);
    m.set_parameter(sid, "width", 200.0 / 7.0).unwrap();
    let stored = stored_sketch(&m, sid);
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("f.f420.json");
    save_file(&p, m.document(), Some(m.evaluation())).unwrap();
    let back = load_file(&p).unwrap().document;
    let FeatureKind::Sketch { sketch: loaded, .. } = &back.feature(sid).unwrap().kind else {
        panic!()
    };
    for (a, b) in stored.points.iter().zip(&loaded.points) {
        assert_eq!(a.x.to_bits(), b.x.to_bits(), "x of {:?}", a.id);
        assert_eq!(a.y.to_bits(), b.y.to_bits(), "y of {:?}", a.id);
    }
    assert_eq!(&back, m.document());
    // The reloaded definition is still a solved state (no drift on reload).
    let corner = loaded.point_xy(c[1]).unwrap();
    assert!((corner[0] - 100.0 / 7.0).abs() < 1e-8);
}
