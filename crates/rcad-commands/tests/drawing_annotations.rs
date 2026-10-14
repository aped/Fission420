//! Drawing annotations (docs/DRAWINGS.md "Dimensions, tolerances,
//! notes, BOM and balloons"): driven dimensions between semantic references
//! with tolerances, assembly views with a BOM table and balloons, and notes.
//! Values are checked against the model definition by hand; texts are read
//! back from the exported SVG with the small scanner below, not the writer's
//! types.
use rcad_commands::{Command, Response, Session, SketchTemplate};
use rcad_document::drawing::{DimRef, DrawingId, Tolerance, ViewId};
use rcad_document::{BodyId, FaceRef, FaceRole, FeatureId, SketchPlane};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn refused(s: &mut Session, c: Command) -> String {
    let rev = s.summary().revision;
    let r = s.execute(&c);
    assert!(!r.ok, "{c:?} should be refused");
    assert_eq!(s.summary().revision, rev, "refusal changed the document");
    format!("{:?}", r.error)
}

fn out_dir() -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/drawings/annotations");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn sketch_rect(s: &mut Session, w: f64, h: f64, c: [f64; 2]) -> FeatureId {
    ok(
        s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: w,
                height: h,
                center: Some(c),
            },
        },
    )
    .feature
    .unwrap()
}

/// Block x [0, 40], y [0, 30], z [0, 10] with a Ø8 hole at (20, 15).
fn block(s: &mut Session) -> (FeatureId, FeatureId) {
    ok(s, Command::NewDocument);
    let sk = sketch_rect(s, 40.0, 30.0, [20.0, 15.0]);
    let e = ok(s, Command::extrude(None, sk, 10.0)).feature.unwrap();
    let h = ok(
        s,
        Command::Hole {
            name: None,
            plane: SketchPlane::Face(FaceRef {
                feature: e,
                role: FaceRole::ExtrudeEndCap,
            }),
            center: [20.0, 15.0],
            diameter: 8.0,
            depth: None,
            counterbore: None,
            target: None,
        },
    )
    .feature
    .unwrap();
    (e, h)
}

/// Texts per SVG group id (`<g id="...">` ... `</g>`, groups not nested).
fn svg_texts(svg: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut rest = svg;
    while let Some(i) = rest.find("<g id=\"") {
        let after = &rest[i + 7..];
        let id = &after[..after.find('"').unwrap()];
        let end = after.find("</g>").unwrap();
        let body = &after[..end];
        let mut t = body;
        let list = out.entry(id.to_string()).or_default();
        while let Some(j) = t.find("<text") {
            let s2 = &t[j..];
            let open = s2.find('>').unwrap();
            let close = s2.find("</text>").unwrap();
            list.push(
                s2[open + 1..close]
                    .replace("&amp;", "&")
                    .replace("&lt;", "<")
                    .replace("&gt;", ">"),
            );
            t = &s2[close..];
        }
        rest = &after[end..];
    }
    out
}

fn export(s: &mut Session, d: DrawingId, name: &str) -> (Response, String) {
    let path = out_dir().join(name);
    let r = ok(
        s,
        Command::ExportDrawing {
            drawing: d,
            path: path.clone(),
            format: None,
        },
    );
    let text = if name.ends_with(".svg") {
        std::fs::read_to_string(&path).unwrap()
    } else {
        String::new()
    };
    (r, text)
}

/// (kind, value, text, dangling?) of the dimensions of view `v` of the last export.
fn view_dims(r: &Response, v: u32) -> Vec<(String, Option<f64>, String, bool)> {
    r.drawing_report
        .as_ref()
        .unwrap()
        .views
        .iter()
        .find(|x| x.id == ViewId(v))
        .unwrap()
        .dimensions
        .iter()
        .map(|d| {
            (
                d.kind.clone(),
                d.value,
                d.text.clone(),
                d.dangling.is_some(),
            )
        })
        .collect()
}

/// User dimensions with values rounded to 1e-9 mm (B-rep coordinates carry
/// ~1e-13 noise).
fn user_dims(r: &Response, v: u32) -> Vec<(String, Option<f64>, String, bool)> {
    view_dims(r, v)
        .into_iter()
        .map(|(k, v, t, d)| (k, v.map(|x| (x * 1e9).round() / 1e9), t, d))
        .filter(|d| d.0 == "distance" || d.0 == "hole_diameter")
        .collect()
}

fn side_face(s: &Session, e: FeatureId, normal: [f64; 3]) -> FaceRef {
    s.model()
        .unwrap()
        .evaluation()
        .faces
        .iter()
        .find(|c| {
            c.reference.feature == e
                && matches!(c.reference.role, FaceRole::ExtrudeSide { .. })
                && (0..3).all(|i| (c.frame.z[i] - normal[i]).abs() < 1e-9)
        })
        .expect("side face")
        .reference
}

#[test]
fn toleranced_dimensions_follow_the_model_and_dangle_when_invalid() {
    let mut s = Session::new();
    let (e, hole) = block(&mut s);
    let d = ok(
        &mut s,
        Command::CreateDrawing {
            name: Some("Block".into()),
            sheet: None,
            projection: None,
            title: None,
            part_number: None,
            material: None,
            bodies: vec![BodyId(1)],
            standard_views: None,
            assembly: false,
        },
    )
    .drawing
    .unwrap();
    let top = FaceRef {
        feature: e,
        role: FaceRole::ExtrudeEndCap,
    };
    let bottom = FaceRef {
        feature: e,
        role: FaceRole::ExtrudeStartCap,
    };
    let left = side_face(&s, e, [-1.0, 0.0, 0.0]);
    // Front view (V1): thickness top-bottom, vertical, ±0.1.
    ok(
        &mut s,
        Command::AddDrawingDimension {
            drawing: d,
            view: ViewId(1),
            a: DimRef::Face(bottom),
            b: Some(DimRef::Face(top)),
            tolerance: Tolerance::Symmetric { value: 0.1 },
        },
    );
    // Top view (V2): hole axis from the left face (x = 0), deviation; hole
    // diameter as limits.
    ok(
        &mut s,
        Command::AddDrawingDimension {
            drawing: d,
            view: ViewId(2),
            a: DimRef::Face(left),
            b: Some(DimRef::Hole(hole)),
            tolerance: Tolerance::Deviation {
                upper: 0.05,
                lower: -0.02,
            },
        },
    );
    ok(
        &mut s,
        Command::AddDrawingDimension {
            drawing: d,
            view: ViewId(2),
            a: DimRef::Hole(hole),
            b: None,
            tolerance: Tolerance::Limits {
                upper: 0.1,
                lower: 0.0,
            },
        },
    );
    // Refusals (document unchanged): faces not parallel; hole not end-on in
    // the front view; a dimension not seen in true length (top-bottom in the
    // top view); bad tolerances.
    let why = refused(
        &mut s,
        Command::AddDrawingDimension {
            drawing: d,
            view: ViewId(1),
            a: DimRef::Face(top),
            b: Some(DimRef::Face(left)),
            tolerance: Tolerance::None,
        },
    );
    assert!(why.contains("not parallel"), "{why}");
    let why = refused(
        &mut s,
        Command::AddDrawingDimension {
            drawing: d,
            view: ViewId(1),
            a: DimRef::Hole(hole),
            b: None,
            tolerance: Tolerance::None,
        },
    );
    assert!(why.contains("end-on"), "{why}");
    let why = refused(
        &mut s,
        Command::AddDrawingDimension {
            drawing: d,
            view: ViewId(2),
            a: DimRef::Face(bottom),
            b: Some(DimRef::Face(top)),
            tolerance: Tolerance::None,
        },
    );
    assert!(why.contains("true length"), "{why}");
    for t in [
        Tolerance::Symmetric { value: -0.1 },
        Tolerance::Deviation {
            upper: -0.1,
            lower: 0.1,
        },
        Tolerance::Symmetric { value: f64::NAN },
    ] {
        refused(
            &mut s,
            Command::AddDrawingDimension {
                drawing: d,
                view: ViewId(1),
                a: DimRef::Face(bottom),
                b: Some(DimRef::Face(top)),
                tolerance: t,
            },
        );
    }

    let (r, svg) = export(&mut s, d, "dims_0.svg");
    assert!(r.drawing_report.as_ref().unwrap().issues.is_empty());
    assert_eq!(
        user_dims(&r, 1),
        vec![("distance".into(), Some(10.0), "10 ±0.1".into(), false)]
    );
    let v2 = user_dims(&r, 2);
    assert_eq!(
        v2[0],
        (
            "distance".into(),
            Some(20.0),
            "20 +0.05/-0.02".into(),
            false
        )
    );
    assert_eq!(
        v2[1],
        ("hole_diameter".into(), Some(8.0), "Ø8.1/8".into(), false)
    );
    let t = svg_texts(&svg);
    assert!(
        t["dims-1"].contains(&"10 ±0.1".to_string()),
        "{:?}",
        t["dims-1"]
    );
    assert!(t["dims-2"].contains(&"20 +0.05/-0.02".to_string()));
    assert!(t["dims-2"].contains(&"Ø8.1/8".to_string()));

    // Upstream edits: hole Ø12 at (12, 10), block 10 -> 15 thick.
    for (n, v) in [("diameter", 12.0), ("center_x", 12.0), ("center_y", 10.0)] {
        ok(
            &mut s,
            Command::SetParameter {
                feature: hole,
                name: n.into(),
                value: v,
            },
        );
    }
    ok(
        &mut s,
        Command::SetParameter {
            feature: e,
            name: "distance".into(),
            value: 15.0,
        },
    );
    let (r, _) = export(&mut s, d, "dims_1.svg");
    assert!(r.drawing_report.as_ref().unwrap().issues.is_empty());
    assert_eq!(user_dims(&r, 1)[0].2, "15 ±0.1");
    let v2 = user_dims(&r, 2);
    assert_eq!((v2[0].1, v2[0].2.as_str()), (Some(12.0), "12 +0.05/-0.02"));
    assert_eq!((v2[1].1, v2[1].2.as_str()), (Some(12.0), "Ø12.1/12"));

    // Tolerance edit (one undo step).
    ok(
        &mut s,
        Command::SetDimensionTolerance {
            drawing: d,
            view: ViewId(1),
            index: 0,
            tolerance: Tolerance::None,
        },
    );
    let (r, _) = export(&mut s, d, "dims_2.svg");
    assert!(
        view_dims(&r, 1)
            .iter()
            .any(|x| x.0 == "distance" && x.2 == "15")
    );
    ok(&mut s, Command::Undo);

    // Delete the hole: both of its dimensions dangle (no value, red, reported);
    // the thickness stays valid. Undo restores them.
    ok(&mut s, Command::DeleteFeature { feature: hole });
    let (r, svg) = export(&mut s, d, "dims_deleted.svg");
    let issues = &r.drawing_report.as_ref().unwrap().issues;
    assert_eq!(issues.len(), 2, "{issues:?}");
    assert!(issues.iter().all(|i| i.contains("dangling")));
    let v2 = user_dims(&r, 2);
    assert_eq!(v2.len(), 2);
    assert!(v2.iter().all(|x| x.1.is_none() && x.3), "{v2:?}");
    assert!(!svg.contains("12 +0.05/-0.02"), "a stale value is drawn");
    assert_eq!(user_dims(&r, 1)[0].2, "15 ±0.1");
    ok(&mut s, Command::Undo);
    let (r, _) = export(&mut s, d, "dims_restored.svg");
    assert!(r.drawing_report.as_ref().unwrap().issues.is_empty());

    // Save / reopen (v7), identical report; removal of an annotation.
    let path = out_dir().join("dims.rcad.json");
    ok(&mut s, Command::Save { path: path.clone() });
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], 7);
    let mut t = Session::new();
    ok(&mut t, Command::Open { path });
    let (r2, _) = export(&mut t, d, "dims_reopened.svg");
    // Same texts and values (to 1e-9 mm: a fresh regeneration may differ
    // from the undo-restored geometry in the last bits).
    for v in 1..=3 {
        let (a, b) = (view_dims(&r, v), view_dims(&r2, v));
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!((&x.0, &x.2, x.3), (&y.0, &y.2, y.3));
            assert!((x.1.unwrap() - y.1.unwrap()).abs() < 1e-9);
        }
    }
    ok(
        &mut t,
        Command::RemoveDrawingAnnotation {
            drawing: d,
            view: ViewId(2),
            index: 0,
        },
    );
    refused(
        &mut t,
        Command::RemoveDrawingAnnotation {
            drawing: d,
            view: ViewId(2),
            index: 5,
        },
    );
}

#[test]
fn assembly_drawing_bom_balloons_and_notes() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    // Plate 60 x 40 x 5 and a 4 x 4 x 8 screw.
    let sk = sketch_rect(&mut s, 60.0, 40.0, [0.0, 0.0]);
    ok(&mut s, Command::extrude(None, sk, 5.0));
    let sk = sketch_rect(&mut s, 4.0, 4.0, [-20.0, 0.0]);
    ok(&mut s, Command::extrude(None, sk, 8.0));
    ok(
        &mut s,
        Command::SetBodyMaterial {
            body: BodyId(1),
            material: Some("pla".into()),
            density: None,
            density_unit: Default::default(),
            name: None,
        },
    );
    let json = |v: serde_json::Value| -> Command { serde_json::from_value(v).unwrap() };
    ok(
        &mut s,
        json(
            serde_json::json!({"cmd": "create_component", "name": "Plate",
        "bodies": [1], "part_number": "PL-60"}),
        ),
    );
    ok(
        &mut s,
        json(
            serde_json::json!({"cmd": "create_component", "name": "Screw",
        "bodies": [2], "part_number": "SC-M4"}),
        ),
    );
    for x in [20.0, 40.0] {
        ok(
            &mut s,
            json(serde_json::json!({"cmd": "add_occurrence", "component": 2,
            "translate": [x, 0.0, 0.0]})),
        );
    }
    let d = ok(
        &mut s,
        Command::CreateDrawing {
            name: Some("Assembly".into()),
            sheet: None,
            projection: None,
            title: Some("Plate assembly".into()),
            part_number: None,
            material: None,
            bodies: vec![],
            standard_views: None,
            assembly: true,
        },
    )
    .drawing
    .unwrap();
    ok(
        &mut s,
        Command::AddDrawingNote {
            drawing: d,
            text: "NOTES:\n1. BREAK SHARP EDGES".into(),
            position: [20.0, 60.0],
        },
    );
    let (r, svg) = export(&mut s, d, "assembly_0.svg");
    let rep = r.drawing_report.as_ref().unwrap();
    assert!(rep.issues.is_empty(), "{:?}", rep.issues);
    let rows: Vec<(usize, &str, &str, usize, &str)> = rep
        .bom
        .iter()
        .map(|b| {
            (
                b.item,
                b.name.as_str(),
                b.part_number.as_str(),
                b.quantity,
                b.material.as_str(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (1, "Plate", "PL-60", 1, "PLA"),
            (2, "Screw", "SC-M4", 3, "-")
        ]
    );
    let tx = svg_texts(&svg);
    for want in ["ITEM", "QTY", "PL-60", "SC-M4", "Screw", "3", "PLA"] {
        assert!(
            tx["bom"].contains(&want.to_string()),
            "{want}: {:?}",
            tx["bom"]
        );
    }
    assert_eq!(tx["notes"], vec!["NOTES:", "1. BREAK SHARP EDGES"]);
    let balloons: Vec<String> = view_dims(&r, 1)
        .into_iter()
        .filter(|x| x.0 == "balloon")
        .map(|x| x.2)
        .collect();
    assert_eq!(balloons, vec!["1", "2"]);
    assert!(
        tx["balloons-1"].contains(&"1".to_string()) && tx["balloons-1"].contains(&"2".to_string())
    );
    // The assembly views draw every occurrence: the front view is as wide as
    // the plate (the screws are within it) and 8 mm high (the screws stand
    // on z = 0 like the plate).
    let v1 = &rep.views[0];
    assert!(v1.visible_curves > 0);
    let h: Vec<f64> = v1
        .dimensions
        .iter()
        .filter(|x| x.kind == "horizontal" || x.kind == "vertical")
        .filter_map(|x| x.value)
        .collect();
    assert_eq!(h, vec![60.0, 8.0]);

    // Suppressing a screw occurrence: quantity 2. Undo: 3.
    ok(
        &mut s,
        json(
            serde_json::json!({"cmd": "set_occurrence_suppressed", "occurrence": 4,
        "suppressed": true}),
        ),
    );
    let (r, _) = export(&mut s, d, "assembly_suppressed.svg");
    assert_eq!(r.drawing_report.as_ref().unwrap().bom[1].quantity, 2);
    ok(&mut s, Command::Undo);
    let (r, _) = export(&mut s, d, "assembly_1.svg");
    assert_eq!(r.drawing_report.as_ref().unwrap().bom[1].quantity, 3);
    // Part number edit flows into the table.
    ok(
        &mut s,
        json(serde_json::json!({"cmd": "set_part_number", "component": 2,
            "part_number": "SC-M4-10"})),
    );
    let (_, svg) = export(&mut s, d, "assembly_2.svg");
    assert!(svg_texts(&svg)["bom"].contains(&"SC-M4-10".to_string()));
    // PDF and DXF exports of the same sheet.
    export(&mut s, d, "assembly.pdf");
    export(&mut s, d, "assembly.dxf");

    // Dimensions are refused in assembly views; notes outside the sheet and
    // empty notes are refused; balloons/BOM toggles and note removal undo.
    let why = refused(
        &mut s,
        Command::AddDrawingDimension {
            drawing: d,
            view: ViewId(1),
            a: DimRef::Face(FaceRef {
                feature: FeatureId(2),
                role: FaceRole::ExtrudeEndCap,
            }),
            b: Some(DimRef::Face(FaceRef {
                feature: FeatureId(2),
                role: FaceRole::ExtrudeStartCap,
            })),
            tolerance: Tolerance::None,
        },
    );
    assert!(why.contains("assembly views"), "{why}");
    for (text, at) in [("x", [500.0, 10.0]), ("  ", [20.0, 20.0])] {
        refused(
            &mut s,
            Command::AddDrawingNote {
                drawing: d,
                text: text.into(),
                position: at,
            },
        );
    }
    ok(
        &mut s,
        Command::SetBalloons {
            drawing: d,
            view: ViewId(1),
            show: false,
        },
    );
    ok(
        &mut s,
        Command::SetBomTable {
            drawing: d,
            show: false,
            position: None,
        },
    );
    ok(
        &mut s,
        Command::RemoveDrawingNote {
            drawing: d,
            index: 0,
        },
    );
    let (r, svg) = export(&mut s, d, "assembly_bare.svg");
    assert!(r.drawing_report.as_ref().unwrap().bom.is_empty());
    let tx = svg_texts(&svg);
    assert!(!tx.contains_key("bom") && !tx.contains_key("notes"));
    for _ in 0..3 {
        ok(&mut s, Command::Undo);
    }
    let (r, _) = export(&mut s, d, "assembly_back.svg");
    assert_eq!(r.drawing_report.as_ref().unwrap().bom.len(), 2);

    // Save / reopen keeps the assembly sheet (v7).
    let path = out_dir().join("assembly_drawing.rcad.json");
    ok(&mut s, Command::Save { path: path.clone() });
    let mut u = Session::new();
    ok(&mut u, Command::Open { path });
    let (r2, _) = export(&mut u, d, "assembly_reopened.svg");
    assert_eq!(
        r.drawing_report.as_ref().unwrap().bom,
        r2.drawing_report.as_ref().unwrap().bom
    );
}

/// Regression: a hole silhouette hidden behind a front face that is
/// triangulated with a diagonal crossing it was drawn with a 0.03 mm visible
/// speck where the diagonal crossed (each triangle was tested alone with the
/// outline margin). 80 × 50 × 10 plate, Ø8 at (20, 25) and Ø6 at (60, 25):
/// front and right views each show exactly four hidden lines, the full
/// plate thickness long.
#[test]
fn hidden_hole_silhouettes_have_no_visible_specks() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let sk = sketch_rect(&mut s, 80.0, 50.0, [40.0, 25.0]);
    let e = ok(&mut s, Command::extrude(None, sk, 10.0))
        .feature
        .unwrap();
    for (c, d) in [([20.0, 25.0], 8.0), ([60.0, 25.0], 6.0)] {
        ok(
            &mut s,
            Command::Hole {
                name: None,
                plane: SketchPlane::Face(FaceRef {
                    feature: e,
                    role: FaceRole::ExtrudeEndCap,
                }),
                center: c,
                diameter: d,
                depth: None,
                counterbore: None,
                target: None,
            },
        );
    }
    let d = ok(
        &mut s,
        Command::CreateDrawing {
            name: None,
            sheet: None,
            projection: None,
            title: None,
            part_number: None,
            material: None,
            bodies: vec![BodyId(1)],
            standard_views: None,
            assembly: false,
        },
    )
    .drawing
    .unwrap();
    let (r, _) = export(&mut s, d, "specks.svg");
    let views = &r.drawing_report.as_ref().unwrap().views;
    assert_eq!(views[0].hidden_curves, 4, "front: {:?}", views[0]);
    assert_eq!(views[2].hidden_curves, 4, "right: {:?}", views[2]);
    // Front: 4 outline edges + nothing else visible.
    assert_eq!(views[0].visible_curves, 4, "front: {:?}", views[0]);
}

/// A hole reference must not print a stale value after the
/// body moves (the hole's frame is where it was made): the dimension dangles.
#[test]
fn hole_dimension_dangles_after_the_body_moves() {
    let mut s = Session::new();
    let (e, hole) = block(&mut s);
    let d = ok(
        &mut s,
        Command::CreateDrawing {
            name: None,
            sheet: None,
            projection: None,
            title: None,
            part_number: None,
            material: None,
            bodies: vec![BodyId(1)],
            standard_views: None,
            assembly: false,
        },
    )
    .drawing
    .unwrap();
    let left = side_face(&s, e, [-1.0, 0.0, 0.0]);
    ok(
        &mut s,
        Command::AddDrawingDimension {
            drawing: d,
            view: ViewId(2),
            a: DimRef::Face(left),
            b: Some(DimRef::Hole(hole)),
            tolerance: Tolerance::None,
        },
    );
    let (r, _) = export(&mut s, d, "moved_0.svg");
    assert_eq!(user_dims(&r, 2)[0].1, Some(20.0));
    ok(
        &mut s,
        serde_json::from_value(serde_json::json!({"cmd": "move_body", "body": 1,
            "translate": [5.0, 0.0, 0.0]}))
        .unwrap(),
    );
    let (r, svg) = export(&mut s, d, "moved_1.svg");
    let dims = user_dims(&r, 2);
    assert!(dims[0].1.is_none() && dims[0].3, "{dims:?}");
    assert!(
        !svg.contains(">20<") && !svg.contains(">25<"),
        "a value is drawn"
    );
}

/// A file whose detail views are each other's parent (or
/// any invalid drawing) is refused on load instead of recursing later.
#[test]
fn invalid_drawings_are_refused_on_load() {
    let mut s = Session::new();
    block(&mut s);
    ok(
        &mut s,
        Command::CreateDrawing {
            name: None,
            sheet: None,
            projection: None,
            title: None,
            part_number: None,
            material: None,
            bodies: vec![BodyId(1)],
            standard_views: Some(false),
            assembly: false,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("d.rcad.json");
    ok(&mut s, Command::Save { path: path.clone() });
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let views = v["document"]["drawings"][0]["views"]
        .as_array_mut()
        .unwrap();
    let detail = |id: u32, parent: u32| {
        serde_json::json!({"id": id, "kind": {"detail": {"parent": parent, "center": [0.0, 0.0],
            "radius": 5.0, "scale": 2.0, "position": [100.0, 100.0],
            "label": format!("D{id}")}}})
    };
    views.push(detail(7, 8));
    views.push(detail(8, 7));
    v["document"]["drawings"][0]["next_view"] = serde_json::json!(9);
    v["version"] = serde_json::json!(7);
    std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    let mut t = Session::new();
    let r = t.execute(&Command::Open { path });
    assert!(!r.ok, "cyclic detail views loaded");
    assert!(
        format!("{:?}", r.error).contains("drawing"),
        "{:?}",
        r.error
    );
}

/// An occurrence whose definition body is not evaluated
/// (here: rolled back) flags the assembly view instead of vanishing silently.
#[test]
fn unevaluated_occurrence_bodies_flag_the_assembly_view() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let sk = sketch_rect(&mut s, 60.0, 40.0, [0.0, 0.0]);
    ok(&mut s, Command::extrude(None, sk, 5.0));
    let sk = sketch_rect(&mut s, 4.0, 4.0, [-20.0, 0.0]);
    ok(&mut s, Command::extrude(None, sk, 8.0));
    let json = |v: serde_json::Value| -> Command { serde_json::from_value(v).unwrap() };
    ok(
        &mut s,
        json(serde_json::json!({"cmd": "create_component", "name": "Plate", "bodies": [1]})),
    );
    ok(
        &mut s,
        json(serde_json::json!({"cmd": "create_component", "name": "Screw", "bodies": [2]})),
    );
    let d = ok(
        &mut s,
        Command::CreateDrawing {
            name: None,
            sheet: None,
            projection: None,
            title: None,
            part_number: None,
            material: None,
            bodies: vec![],
            standard_views: None,
            assembly: true,
        },
    )
    .drawing
    .unwrap();
    let (r, _) = export(&mut s, d, "rollback_0.svg");
    assert!(r.drawing_report.as_ref().unwrap().issues.is_empty());
    // Roll back to after the plate: the screw's body is not evaluated.
    let r0 = s.execute(&json(serde_json::json!({"cmd": "set_rollback", "at": 2})));
    // The precondition must hold (no early pass).
    assert!(r0.ok, "rollback refused: {:?}", r0.error);
    let (r, _) = export(&mut s, d, "rollback_1.svg");
    let issues = &r.drawing_report.as_ref().unwrap().issues;
    assert!(
        issues.iter().any(|i| i.contains("B2")),
        "missing occurrence body not reported: {issues:?}"
    );
}

/// A 180° rotation swaps two equal holes, so a cylinder of
/// the same size sits on the first hole's old axis; the dimension must still
/// dangle (body moved after the hole), not print a value.
#[test]
fn hole_dimension_dangles_after_a_rotation_that_swaps_holes() {
    let mut s = Session::new();
    let (e, hole) = block(&mut s);
    // A second Ø8 hole mirrored about x = 20: the first is at (20, 15), move
    // it to (10, 15) and add one at (30, 15).
    ok(
        &mut s,
        Command::SetParameter {
            feature: hole,
            name: "center_x".into(),
            value: 10.0,
        },
    );
    ok(
        &mut s,
        Command::Hole {
            name: None,
            plane: SketchPlane::Face(FaceRef {
                feature: e,
                role: FaceRole::ExtrudeEndCap,
            }),
            center: [30.0, 15.0],
            diameter: 8.0,
            depth: None,
            counterbore: None,
            target: None,
        },
    );
    let d = ok(
        &mut s,
        Command::CreateDrawing {
            name: None,
            sheet: None,
            projection: None,
            title: None,
            part_number: None,
            material: None,
            bodies: vec![BodyId(1)],
            standard_views: None,
            assembly: false,
        },
    )
    .drawing
    .unwrap();
    let left = side_face(&s, e, [-1.0, 0.0, 0.0]);
    ok(
        &mut s,
        Command::AddDrawingDimension {
            drawing: d,
            view: ViewId(2),
            a: DimRef::Face(left),
            b: Some(DimRef::Hole(hole)),
            tolerance: Tolerance::None,
        },
    );
    let (r, _) = export(&mut s, d, "swap_0.svg");
    assert_eq!(user_dims(&r, 2)[0].1, Some(10.0));
    ok(
        &mut s,
        serde_json::from_value(serde_json::json!({"cmd": "move_body", "body": 1,
            "origin": [20.0, 15.0, 0.0], "axis": [0.0, 0.0, 1.0], "angle_deg": 180.0}))
        .unwrap(),
    );
    let (r, _) = export(&mut s, d, "swap_1.svg");
    let dims = user_dims(&r, 2);
    assert!(dims[0].1.is_none() && dims[0].3, "{dims:?}");
}

/// Centre lines are for holes and bosses (full circles),
/// not for fillets: a block with one rounded vertical edge has no centre line
/// or mark anywhere on the sheet.
#[test]
fn fillets_get_no_centre_lines() {
    let mut s = Session::new();
    let (e, hole) = block(&mut s);
    ok(&mut s, Command::DeleteFeature { feature: hole });
    let right = side_face(&s, e, [1.0, 0.0, 0.0]);
    let back = side_face(&s, e, [0.0, 1.0, 0.0]);
    ok(
        &mut s,
        Command::Fillet {
            name: None,
            a: right,
            b: back,
            radius: 4.0,
        },
    );
    let d = ok(
        &mut s,
        Command::CreateDrawing {
            name: None,
            sheet: None,
            projection: None,
            title: None,
            part_number: None,
            material: None,
            bodies: vec![BodyId(1)],
            standard_views: None,
            assembly: false,
        },
    )
    .drawing
    .unwrap();
    let (_, svg) = export(&mut s, d, "fillet_centres.svg");
    assert_eq!(
        svg.matches("class=\"center\"").count(),
        0,
        "centre lines on a fillet"
    );
}
