//! External-exchange qualification (docs/FORMAT_SUPPORT.md "Qualification
//! scenario: external exchange"), through the typed command API on files made by
//! another implementation (FreeCAD 1.1.4, `tools/fixtures/corpus/`) or written
//! by hand from the format specifications:
//!
//! 1. foreign exact solids (mm and an inch file with a rotated placement): volume
//!    and bounds against FreeCAD's own read-back, the unit stated in the reply;
//! 2. a foreign assembly with two nested levels of rotated + translated
//!    occurrences (and an inch assembly): flattened bounds and volume against
//!    FreeCAD, hierarchy kept by `import_step_assembly`, units stated;
//! 3. a broken STL repaired to a closed mesh (analytic 8000 mm³) and exported;
//! 4. vector sketches with holes and counters (DXF, SVG): whole-profile extrusion
//!    refused by name, every material region extruded to the closed-form area;
//! 5. reported losses: STEP colours, an unsupported DXF entity;
//! 6. malformed or unsupported files of every format, and files beyond the import
//!    size limit: refused with a bounded message, nothing imported, revision
//!    unchanged.
use rcad_commands::{Command, Response, Session};
use rcad_kernel::VolumeResult;
use serde_json::{Value, json};
use std::f64::consts::PI;
use std::path::PathBuf;

fn corpus(f: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/fixtures/corpus")
        .join(f)
}

fn reference(f: &str) -> Value {
    for r in ["freecad_reference.json", "freecad_reference_more.json"] {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(corpus(r)).unwrap()).unwrap();
        if v.get(f).is_some() {
            return v[f].clone();
        }
    }
    panic!("no FreeCAD reference for {f}")
}

fn exec(s: &mut Session, v: Value) -> Response {
    let c: Command = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{v}: {e}"));
    s.execute(&c)
}

fn ok(s: &mut Session, v: Value) -> Response {
    let r = exec(s, v.clone());
    assert!(r.ok, "{v}: {:?}", r.error);
    r
}

fn fresh() -> Session {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    s
}

/// Sum of exact volumes and union of bounds of every body.
fn totals(s: &Session) -> (f64, [f64; 6]) {
    let mut v = 0.0;
    let mut b = [f64::MAX, f64::MAX, f64::MAX, f64::MIN, f64::MIN, f64::MIN];
    for body in s.summary().bodies {
        let m = body.measurements.expect("measured");
        match m.volume {
            VolumeResult::Value { volume, .. } => v += volume,
            other => panic!("{other:?}"),
        }
        for i in 0..3 {
            b[i] = b[i].min(m.bounds.min[i]);
            b[i + 3] = b[i + 3].max(m.bounds.max[i]);
        }
    }
    (v, b)
}

fn bbox_err(got: [f64; 6], want: &Value) -> f64 {
    got.iter()
        .zip(want.as_array().unwrap())
        .map(|(g, w)| (g - w.as_f64().unwrap()).abs())
        .fold(0.0, f64::max)
}

#[test]
fn foreign_exact_solids_match_freecad_with_units_stated() {
    for (file, unit) in [
        ("p8_fillet_block_mm.step", "1 mm per unit"),
        ("p12_rotated_cylinder_inch.step", "25.4 mm per unit"),
    ] {
        let mut s = fresh();
        let r = ok(&mut s, json!({"cmd": "import_step", "path": corpus(file)}));
        let msg = r.message.unwrap_or_default();
        assert!(
            msg.contains(unit) && msg.contains("declared in file"),
            "{file}: {msg}"
        );
        let rf = reference(file);
        let (v, b) = totals(&s);
        assert_eq!(s.summary().bodies.len(), 1);
        assert!(
            (v - rf["volume"].as_f64().unwrap()).abs() < 1e-4,
            "{file}: {v} vs FreeCAD {}",
            rf["volume"]
        );
        assert!(
            bbox_err(b, &rf["bbox"]) < 1e-5,
            "{file}: bounds {b:?} vs {}",
            rf["bbox"]
        );
    }
}

#[test]
fn foreign_assemblies_keep_transforms_units_and_hierarchy() {
    for (file, unit) in [
        ("a5_nested_transforms_mm.step", "1 mm per unit"),
        ("a4_assembly_inch.step", "25.4 mm per unit"),
    ] {
        let rf = reference(file);
        // Flattened: one body per placed solid, nested transforms composed.
        let mut s = fresh();
        let r = ok(&mut s, json!({"cmd": "import_step", "path": corpus(file)}));
        assert!(r.message.unwrap_or_default().contains(unit), "{file}");
        let (v, b) = totals(&s);
        assert_eq!(
            s.summary().bodies.len() as u64,
            rf["solids"].as_u64().unwrap()
        );
        assert!(
            (v - rf["volume"].as_f64().unwrap()).abs() < 1e-3,
            "{file}: {v}"
        );
        assert!(
            bbox_err(b, &rf["bbox"]) < 1e-5,
            "{file}: bounds {b:?} vs {}",
            rf["bbox"]
        );
        // Hierarchy: products/usages as components and placed occurrences.
        let mut a = fresh();
        ok(
            &mut a,
            json!({"cmd": "import_step_assembly", "path": corpus(file)}),
        );
        let mp = ok(&mut a, json!({"cmd": "mass_properties"}))
            .mass_properties
            .expect("mass properties");
        assert_eq!(
            mp.occurrences.len() as u64,
            rf["solids"].as_u64().unwrap(),
            "{file}"
        );
        let total: f64 = mp.occurrences.iter().filter_map(|o| o.volume).sum();
        assert!(
            (total - rf["volume"].as_f64().unwrap()).abs() < 1e-3,
            "{file}: {total}"
        );
        assert!(
            !a.summary().assembly.components.is_empty(),
            "{file}: components kept"
        );
    }
}

#[test]
fn broken_stl_is_repaired_to_a_closed_mesh_and_exported() {
    let mut s = fresh();
    let stl =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/fixtures/stl/broken_cube.stl");
    ok(
        &mut s,
        json!({"cmd": "import_stl", "path": stl, "units_to_mm": 1.0}),
    );
    let d = s.summary().bodies[0].mesh.clone().unwrap();
    assert!(!d.closed && d.signed_volume.is_none());
    let r = ok(
        &mut s,
        json!({"cmd": "repair_mesh", "body": 1, "options": {"fill_holes_max_edges": 8},
               "hide_source": true}),
    );
    let rep = r.mesh_repair.unwrap();
    assert!(rep.after.closed);
    assert!((rep.after.signed_volume.unwrap() - 8000.0).abs() < 1e-9);
    let dir = tempfile::tempdir().unwrap();
    // The unrepaired source is refused by the strict export policy; the repair exports.
    assert!(
        !exec(
            &mut s,
            json!({"cmd": "export_stl", "path": dir.path().join("a.stl"), "bodies": [1]})
        )
        .ok
    );
    ok(
        &mut s,
        json!({"cmd": "export_stl", "path": dir.path().join("b.stl"), "bodies": [2]}),
    );
    let m = rcad_exchange::stl::read(
        &std::fs::read(dir.path().join("b.stl")).unwrap(),
        &Default::default(),
    )
    .unwrap();
    let dd = rcad_exchange::mesh::diagnose(&m.positions, &m.triangles);
    assert!(dd.closed && (dd.signed_volume.unwrap() - 8000.0).abs() < 1e-3);
}

#[test]
fn vector_sketches_with_holes_and_counters() {
    for (file, cmd, area) in [
        ("h7_counters_mm.dxf", "import_dxf", 4800.0 - 100.0 * PI),
        ("h8_counters_mm.svg", "import_svg", 1600.0 + 25.0 * PI),
    ] {
        let mut s = fresh();
        let sk = ok(
            &mut s,
            json!({"cmd": cmd, "path": corpus(file), "plane": "WorldXY"}),
        )
        .feature
        .unwrap();
        let r = exec(
            &mut s,
            json!({"cmd": "extrude", "sketch": sk.0, "distance": 5.0}),
        );
        assert!(
            !r.ok && format!("{:?}", r.error).contains("island"),
            "{file}: {:?}",
            r.error
        );
        let sketch = s
            .summary()
            .features
            .into_iter()
            .find(|f| f.id == sk)
            .and_then(|f| f.sketch)
            .unwrap();
        for rg in sketch
            .regions()
            .unwrap()
            .into_iter()
            .filter(|r| r.depth % 2 == 0)
        {
            ok(
                &mut s,
                json!({"cmd": "extrude", "sketch": sk.0, "distance": 5.0,
                       "region": serde_json::to_value(&rg.id).unwrap()}),
            );
        }
        let (v, _) = totals(&s);
        assert!(
            (v - 5.0 * area).abs() < 1e-6,
            "{file}: {v} vs {}",
            5.0 * area
        );
    }
}

#[test]
fn losses_are_reported() {
    let mut s = fresh();
    let r = ok(
        &mut s,
        json!({"cmd": "import_step", "path": corpus("c1_coloured_blocks_mm.step")}),
    );
    assert!(
        r.message
            .unwrap()
            .contains("colours/styles ignored: 2 STYLED_ITEM")
    );
    let mut s = fresh();
    let r = exec(
        &mut s,
        json!({"cmd": "import_dxf", "path": corpus("h4_spline_unsupported.dxf"), "plane": "WorldXY"}),
    );
    assert!(!r.ok && format!("{:?}", r.error).contains("SPLINE"));
    let r = ok(
        &mut s,
        json!({"cmd": "import_dxf", "path": corpus("h4_spline_unsupported.dxf"),
               "plane": "WorldXY", "allow_partial": true}),
    );
    assert!(
        r.message.unwrap_or_default().contains("SPLINE"),
        "partial import names the loss"
    );
}

#[test]
fn malformed_and_oversized_files_are_refused_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    // Sparse files just over the import limit (half the native file limit).
    let big = |name: &str| {
        let p = dir.path().join(name);
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(rcad_document::MAX_FILE_BYTES / 2 + 1).unwrap();
        p
    };
    let cases: Vec<(Value, &str)> = vec![
        (
            json!({"cmd": "import_step", "path": corpus("x1_truncated.step")}),
            "STEP",
        ),
        (
            json!({"cmd": "import_stl", "path": corpus("x2_bad_count.stl"), "units_to_mm": 1.0}),
            "STL",
        ),
        (
            json!({"cmd": "import_stl", "path": corpus("x6_truncated_ascii.stl"), "units_to_mm": 1.0}),
            "STL",
        ),
        (
            json!({"cmd": "import_stl", "path": corpus("x7_nan_binary.stl"), "units_to_mm": 1.0}),
            "non-finite",
        ),
        (
            json!({"cmd": "import_3mf", "path": corpus("x3_truncated.3mf")}),
            "3MF",
        ),
        (
            json!({"cmd": "import_3mf", "path": corpus("x8_unit_bogus.3mf")}),
            "unit",
        ),
        (
            json!({"cmd": "import_3mf", "path": corpus("x9_required_beamlattice.3mf")}),
            "extension",
        ),
        (
            json!({"cmd": "import_3mf", "path": corpus("x10_singular_transform.3mf")}),
            "transform",
        ),
        (
            json!({"cmd": "import_dxf", "path": corpus("x4_garbage.dxf"), "plane": "WorldXY"}),
            "DXF",
        ),
        (
            json!({"cmd": "import_dxf", "path": corpus("x11_truncated.dxf"), "plane": "WorldXY"}),
            "DXF",
        ),
        (
            json!({"cmd": "import_svg", "path": corpus("x5_unclosed.svg"), "plane": "WorldXY"}),
            "SVG",
        ),
        (
            json!({"cmd": "import_stl", "path": big("big.stl"), "units_to_mm": 1.0}),
            "import limit",
        ),
        (
            json!({"cmd": "import_step", "path": big("big.step")}),
            "import limit",
        ),
        (
            json!({"cmd": "import_3mf", "path": big("big.3mf")}),
            "import limit",
        ),
    ];
    for (c, want) in cases {
        // Into a document that already holds a body: nothing may change.
        let mut s = fresh();
        ok(
            &mut s,
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 10.0, "height": 10.0}),
        );
        ok(
            &mut s,
            json!({"cmd": "extrude", "sketch": 1, "distance": 5.0}),
        );
        let before = s.model().unwrap().document().clone();
        let rev = s.summary().revision;
        let t = std::time::Instant::now();
        let r = exec(&mut s, c.clone());
        let msg = format!("{:?}", r.error);
        assert!(!r.ok, "{c}: accepted");
        assert!(msg.contains(want), "{c}: {msg} lacks {want}");
        assert!(msg.len() < 2000 && !msg.contains("panicked"), "{c}: {msg}");
        assert_eq!(s.summary().revision, rev, "{c}");
        assert_eq!(s.model().unwrap().document(), &before, "{c}");
        assert!(t.elapsed().as_secs() < 30, "{c}: not bounded in time");
    }
}
