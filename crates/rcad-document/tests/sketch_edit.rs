//! Run-2 document behaviour: editor sketches (origin point, free vertices),
//! optional profiles, dimension edits storing solved geometry, format v2
//! migration and float round-trip exactness.
use rcad_document::*;
use rcad_kernel::VolumeResult;
use rcad_sketch::{Constraint, Dimension, PointId, Sketch};
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

/// Frozen copy of the run-1 `examples/plate.rcad.json` (format v1). The
/// example itself is re-saved (as v2) by `examples/plate_workflow.json`.
fn v1_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plate_v1.rcad.json")
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
        .transact("extrude", |d| Ok(d.push_extrude("E", sid, 5.0)))
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
    let tol = rcad_sketch::solve_tolerance(stored.scale());
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
fn v1_file_migrates_to_bodies_saves_as_v3_and_reloads_equal() {
    let src = v1_fixture();
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&src).unwrap()).unwrap();
    assert_eq!(raw["version"], 1, "fixture must stay a v1 file");
    let nf = load_file(&src).unwrap();
    assert_eq!(nf.version, 1);
    // Migration (ADR 0003): the base extrusion creates Body1, cuts target it.
    assert_eq!(nf.document.bodies.len(), 1);
    assert_eq!(nf.document.bodies[0].name, "Body1");
    for f in &nf.document.features {
        if let FeatureKind::Cut { target, .. } | FeatureKind::Pocket { target, .. } = &f.kind {
            assert_eq!(*target, nf.document.bodies[0].id);
        }
    }
    let m = Model::from_document(nf.document.clone()).unwrap();
    // v1 template sketches have no origin and keep their center parameters.
    for f in &m.document().features {
        if let FeatureKind::Sketch { sketch, .. } = &f.kind {
            assert_eq!(sketch.origin, None);
            assert!(f.parameters().iter().any(|(n, _)| n == "center_x"));
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("plate_v3.rcad.json");
    save_file(&out, m.document(), Some(m.evaluation())).unwrap();
    let raw2: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(raw2["version"], 4);
    assert_eq!(FORMAT_VERSION, 5);
    let nf2 = load_file(&out).unwrap();
    assert_eq!(nf2.document, nf.document);
    let m2 = Model::from_document(nf2.document).unwrap();
    assert_eq!(volume(&m2), volume(&m));
}

#[test]
fn newer_format_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("v4.rcad.json");
    let doc = Document::new();
    let mut nf = NativeFile::new(&doc, None);
    nf.version = FORMAT_VERSION + 1;
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
    let p = dir.path().join("f.rcad.json");
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

/// Quarter disc: lines origin -> (r,0) and (0,r) -> origin, three-point arc
/// (r,0) -> (0,r) whose centre is coincident with the origin, Radius dimension
/// "radius". 8 unknowns, 8 equations (fixed origin 2, coincident 2, H, V,
/// radius, arc-implied equal radius).
fn quarter_disc(r: f64) -> Sketch {
    let mut s = Sketch::default();
    let o = s.ensure_origin();
    let a = s.add_point(r, 0.0);
    let b = s.add_point(0.0, r);
    let h = s.add_line(o, a);
    let v = s.add_line(b, o);
    let k = r * std::f64::consts::FRAC_1_SQRT_2;
    let arc = s.add_arc_3pt(a, b, [k, k]).unwrap();
    let Some(rcad_sketch::Entity::Arc { center, .. }) = s.entity(arc).cloned() else {
        panic!("arc")
    };
    s.constraints.extend([
        Constraint::Coincident { a: o, b: center },
        Constraint::Horizontal { line: h },
        Constraint::Vertical { line: v },
        Constraint::Radius {
            entity: arc,
            dim: Dimension::new("radius", r),
        },
    ]);
    s
}

#[test]
fn arc_profile_extrudes_regenerates_and_round_trips() {
    let mut m = Model::new();
    let sid = add_sketch(&mut m, quarter_disc(10.0));
    let solve = |m: &Model| {
        m.evaluation()
            .features
            .iter()
            .find(|f| f.id == sid)
            .and_then(|f| f.solve.clone())
            .unwrap()
    };
    assert_eq!(solve(&m).status, rcad_sketch::SolveStatus::FullyConstrained);
    m.transact("extrude", |d| Ok(d.push_extrude("E", sid, 4.0)))
        .unwrap();
    // Quarter cylinder: pi r^2 h / 4.
    let want = |r: f64| std::f64::consts::PI * r * r * 4.0 / 4.0;
    let tol = |v: f64| (1e-7 * v).max(1e-3);
    assert!(
        (volume(&m) - want(10.0)).abs() <= tol(want(10.0)),
        "{}",
        volume(&m)
    );
    m.set_parameter(sid, "radius", 6.0).unwrap();
    assert!(
        (volume(&m) - want(6.0)).abs() <= tol(want(6.0)),
        "{}",
        volume(&m)
    );
    let b = m.evaluation().measurements.as_ref().unwrap().bounds;
    assert!(b.min[0].abs() < 1e-6 && (b.max[0] - 6.0).abs() < 1e-6);
    assert!(b.min[1].abs() < 1e-6 && (b.max[1] - 6.0).abs() < 1e-6);

    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("arc.rcad.json");
    save_file(&p, m.document(), Some(m.evaluation())).unwrap();
    let back = load_file(&p).unwrap().document;
    assert_eq!(&back, m.document());
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(text.contains("\"Arc\"") && text.contains("\"Radius\""));
}
