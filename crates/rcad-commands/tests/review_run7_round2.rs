//! Review run 7, round 2 regressions (assembly import, external components,
//! joints and sweeps, BOM CSV, load-time cleanup). Each test names its row.
mod support;
use rcad_commands::assembly::{AssemblyOut, AxisFrom};
use rcad_commands::measure_faces::FacePick;
use rcad_commands::mesh_repair::RepairMeshArgs;
use rcad_commands::{Command, Response, Session};
use rcad_document::BodyId;
use rcad_document::assembly::{JointKind, OccurrenceId};
use serde_json::{Value, json};
use support::tray;

fn ok(s: &mut Session, v: Value) -> Response {
    tray::ok(s, &v)
}

fn refused(s: &mut Session, v: Value) -> String {
    let r = tray::exec(s, &v);
    assert!(!r.ok, "expected a refusal: {v}");
    format!("{:?}", r.error)
}

fn out(r: Response) -> AssemblyOut {
    r.assembly.expect("assembly output")
}

fn block(s: &mut Session, w: f64, h: f64, d: f64, c: [f64; 2]) -> u32 {
    let f = s.summary().features.len() as u32 + 1;
    ok(
        s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": w, "height": h, "center": c}),
    );
    ok(s, json!({"cmd": "extrude", "sketch": f, "distance": d}));
    s.summary().bodies.last().unwrap().id.0
}

/// Component with `bodies` and one occurrence: (component, occurrence).
fn component(s: &mut Session, name: &str, bodies: &[u32]) -> (u32, u32) {
    let o = out(ok(
        s,
        json!({"cmd": "create_component", "name": name, "bodies": bodies}),
    ));
    (o.component.unwrap().0, o.occurrence.unwrap().0)
}

fn fixture_text() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../rcad-kernel/tests/fixtures/step/two_boxes_assembly_mm.step"),
    )
    .unwrap()
}

/// `text` without the entity instances `ids`.
fn drop_entities(text: &str, ids: &[u64]) -> String {
    let mut s = text.to_string();
    for id in ids {
        let key = format!("\n#{id} = ");
        let a = s.find(&key).unwrap_or_else(|| panic!("#{id}"));
        let b = a + s[a..].find(";\n").unwrap() + 1;
        s.replace_range(a..b, "");
    }
    s
}

fn write(dir: &tempfile::TempDir, name: &str, text: &str) -> std::path::PathBuf {
    let p = dir.path().join(name);
    std::fs::write(&p, text).unwrap();
    p
}

/// #2: a usage without placement is fatal only for the structured import;
/// the plain import (and the reopen that regenerates it) still works.
#[test]
fn plain_step_import_ignores_an_unresolvable_structure() {
    let dir = tempfile::tempdir().unwrap();
    let step = write(
        &dir,
        "unplaced.step",
        &drop_entities(&fixture_text(), &[192]),
    );
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    ok(&mut s, json!({"cmd": "import_step", "path": step}));
    assert_eq!(s.summary().bodies.len(), 2);
    let saved = dir.path().join("doc.rcad.json");
    ok(&mut s, json!({"cmd": "save", "path": saved}));
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "open", "path": saved}));
    assert_eq!(t.summary().bodies.len(), 2);
    let e = refused(&mut t, json!({"cmd": "import_step_assembly", "path": step}));
    assert!(e.contains("no placement"), "{e}");
}

/// #7: a free-standing part product beside an assembly becomes a component
/// with a top-level occurrence.
#[test]
fn free_standing_part_product_becomes_a_component() {
    let dir = tempfile::tempdir().unwrap();
    let step = write(
        &dir,
        "loose_part.step",
        &drop_entities(&fixture_text(), &[361, 362, 363, 364, 365]),
    );
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    ok(&mut s, json!({"cmd": "import_step_assembly", "path": step}));
    let a = s.summary().assembly;
    let boxb = a
        .components
        .iter()
        .find(|c| c.name == "BoxB")
        .expect("BoxB component");
    let o = a
        .occurrences
        .iter()
        .find(|o| o.component == boxb.id)
        .expect("BoxB occurrence");
    assert_eq!(o.owner, None, "top level");
    let m = ok(&mut s, json!({"cmd": "mass_properties"}))
        .mass_properties
        .unwrap();
    assert_eq!(m.occurrences.len(), 2);
}

/// #8 / #36: solids outside the product shapes are imported (folded into
/// their part, or under an extra root) and the import's notes are reported.
#[test]
fn solids_outside_product_shapes_are_imported_and_reported() {
    let mut text = fixture_text();
    text = text.replace(
        "#198 = SHAPE_DEFINITION_REPRESENTATION(#199,#205);",
        "#198 = SHAPE_DEFINITION_REPRESENTATION(#199,#900);",
    );
    text = text.replace(
        "#362 = ( REPRESENTATION_RELATIONSHIP('','',#205,#10)",
        "#362 = ( REPRESENTATION_RELATIONSHIP('','',#900,#10)",
    );
    text = text.replace(
        "ENDSEC;\nEND-ISO",
        "#900 = SHAPE_REPRESENTATION('',(#11),#356);\n\
         #901 = ( REPRESENTATION_RELATIONSHIP('','',#205,#900) \
         REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#902) \
         SHAPE_REPRESENTATION_RELATIONSHIP() );\n\
         #902 = ITEM_DEFINED_TRANSFORMATION('','',#11,#11);\n\
         #950 = MANIFOLD_SOLID_BREP('stray',#38);\nENDSEC;\nEND-ISO",
    );
    let dir = tempfile::tempdir().unwrap();
    let step = write(&dir, "outside.step", &text);
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let r = ok(&mut s, json!({"cmd": "import_step_assembly", "path": step}));
    let msg = r.message.unwrap();
    assert!(
        msg.contains("note(s)") && msg.contains("Unstructured solids") && msg.contains("BoxB"),
        "{msg}"
    );
    // Three placed solids of 1000 mm³ each, as the flat import has.
    let m = ok(&mut s, json!({"cmd": "mass_properties"}))
        .mass_properties
        .unwrap();
    assert_eq!(m.occurrences.len(), 3);
    for o in &m.occurrences {
        assert!((o.volume.unwrap() - 1000.0).abs() < 1e-6);
    }
}

/// #35: names from STEP are cut to the document's name rules.
#[test]
fn step_names_are_sanitized() {
    let long = "N".repeat(200);
    let text = fixture_text()
        .replace(
            "#33 = PRODUCT('BoxA','BoxA','',(#34));",
            &format!("#33 = PRODUCT('{long}','{long}','',(#34));"),
        )
        .replace(
            "NEXT_ASSEMBLY_USAGE_OCCURRENCE('1','BoxA'",
            &format!("NEXT_ASSEMBLY_USAGE_OCCURRENCE('1','{long}'"),
        );
    assert!(text.contains(&long));
    let dir = tempfile::tempdir().unwrap();
    let step = write(&dir, "long.step", &text);
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    ok(&mut s, json!({"cmd": "import_step_assembly", "path": step}));
    let a = s.summary().assembly;
    for c in &a.components {
        assert!(c.name.chars().count() <= 64, "{}", c.name);
    }
    for o in &a.occurrences {
        assert!(o.name.chars().count() <= 64, "{}", o.name);
    }
    assert!(a.components.iter().any(|c| c.name.starts_with("NNNN")));
}

/// #42: the kernel's structure depth is the document's.
#[test]
fn structure_depth_limits_agree() {
    assert_eq!(
        rcad_kernel::STEP_MAX_ASSEMBLY_DEPTH,
        rcad_document::assembly::MAX_DEPTH
    );
}

/// Closed 20 mm cube as an ASCII STL.
fn cube_stl() -> String {
    let p = [
        [0.0, 0.0, 0.0],
        [20.0, 0.0, 0.0],
        [20.0, 20.0, 0.0],
        [0.0, 20.0, 0.0],
        [0.0, 0.0, 20.0],
        [20.0, 0.0, 20.0],
        [20.0, 20.0, 20.0],
        [0.0, 20.0, 20.0],
    ];
    let t: [[usize; 3]; 12] = [
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ];
    let mut s = String::from("solid cube\n");
    for tr in t {
        s.push_str("  facet normal 0 0 0\n    outer loop\n");
        for i in tr {
            let v = p[i];
            s.push_str(&format!("      vertex {} {} {}\n", v[0], v[1], v[2]));
        }
        s.push_str("    endloop\n  endfacet\n");
    }
    s.push_str("endsolid cube\n");
    s
}

/// #3: deleting the repair whose body replaced its source in a component
/// gives the source its place back.
#[test]
fn deleting_a_derived_body_restores_its_source_in_the_component() {
    let dir = tempfile::tempdir().unwrap();
    let stl = write(&dir, "cube.stl", &cube_stl());
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    ok(
        &mut s,
        json!({"cmd": "import_stl", "path": stl, "units_to_mm": 1.0}),
    );
    let (c, _) = component(&mut s, "C", &[1]);
    let r = s.execute(&Command::RepairMesh(RepairMeshArgs {
        body: BodyId(1),
        options: Default::default(),
        preview: false,
        body_name: None,
        hide_source: false,
    }));
    assert!(r.ok, "{:?}", r.error);
    let comp = |s: &Session| {
        s.summary()
            .assembly
            .components
            .into_iter()
            .find(|x| x.id.0 == c)
            .unwrap()
            .bodies
    };
    assert_eq!(comp(&s), vec![BodyId(2)], "the repair replaced the source");
    let repair = s.summary().features.last().unwrap().id;
    ok(&mut s, json!({"cmd": "delete_feature", "feature": repair}));
    assert_eq!(comp(&s), vec![BodyId(1)], "the source is back");
    ok(&mut s, json!({"cmd": "undo"}));
    assert_eq!(comp(&s), vec![BodyId(2)]);
}

/// #40: hidden loose bodies of an external source are embedded like hidden
/// occurrences (visibility does not change the solid count).
#[test]
fn external_source_includes_hidden_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.rcad.json");
    let mut a = Session::new();
    ok(&mut a, json!({"cmd": "new_document"}));
    block(&mut a, 10.0, 10.0, 10.0, [0.0, 0.0]);
    let b = block(&mut a, 4.0, 4.0, 4.0, [20.0, 0.0]);
    ok(
        &mut a,
        json!({"cmd": "set_body_visible", "body": b, "visible": false}),
    );
    ok(&mut a, json!({"cmd": "save", "path": src}));
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    ok(
        &mut s,
        json!({"cmd": "insert_external_component", "path": src, "linked": true}),
    );
    let comp = s.summary().assembly.components[0].clone();
    assert_eq!(comp.bodies.len(), 2, "the hidden body is embedded too");
    // Unhiding it in the source keeps the solid count: update accepted.
    ok(
        &mut a,
        json!({"cmd": "set_body_visible", "body": b, "visible": true}),
    );
    ok(&mut a, json!({"cmd": "save", "path": src}));
    ok(
        &mut s,
        json!({"cmd": "update_external_component", "component": comp.id}),
    );
}

/// #41: an open surface body in an external source is refused by name.
#[test]
fn external_source_with_a_surface_body_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("surf.rcad.json");
    let mut a = Session::new();
    ok(&mut a, json!({"cmd": "new_document"}));
    ok(
        &mut a,
        json!({"cmd": "import_surfaces", "path": std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../rcad-kernel/tests/fixtures/step/open_box_mm.step")}),
    );
    ok(&mut a, json!({"cmd": "save", "path": src}));
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let e = refused(
        &mut s,
        json!({"cmd": "insert_external_component", "path": src}),
    );
    assert!(e.contains("open surface body"), "{e}");
}

/// #16: the native loader reads at most the size limit, even from a file
/// whose size is not known up front.
#[cfg(unix)]
#[test]
fn native_load_is_bounded() {
    let p = std::path::Path::new("/dev/zero");
    if !p.exists() {
        return;
    }
    let e = rcad_document::load_file(p).unwrap_err().to_string();
    assert!(e.contains("too large"), "{e}");
}

fn slider(s: &mut Session, parent: u32, child: u32, axis: [f64; 3], lim: (f64, f64)) -> u32 {
    let r = s.execute(&Command::AddJoint {
        name: None,
        kind: JointKind::Slider,
        parent: OccurrenceId(parent),
        child: OccurrenceId(child),
        origin: [0.0; 3],
        axis: Some(axis),
        axis_from: None,
        value: 0.0,
        min: Some(lim.0),
        max: Some(lim.1),
        extra: vec![],
        dir2: None,
        child_axis_from: None,
        flip: false,
    });
    assert!(r.ok, "{:?}", r.error);
    r.assembly.unwrap().joint.unwrap().0
}

/// #5: the sweep checks only pairs where a part moves with the joint; a
/// static overlap elsewhere is not reported at every sample.
#[test]
fn sweep_ignores_static_pairs() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let a = block(&mut s, 10.0, 10.0, 10.0, [0.0, 0.0]);
    let b = block(&mut s, 4.0, 4.0, 4.0, [20.0, 0.0]);
    // Two static parts overlapping each other, away from the slider.
    let p = block(&mut s, 6.0, 6.0, 6.0, [0.0, 50.0]);
    let q = block(&mut s, 6.0, 6.0, 7.0, [3.0, 50.5]);
    let (_, oa) = component(&mut s, "A", &[a]);
    let (_, ob) = component(&mut s, "B", &[b]);
    let (_, op) = component(&mut s, "P", &[p]);
    let (_, oq) = component(&mut s, "Q", &[q]);
    ok(
        &mut s,
        json!({"cmd": "set_occurrence_transform", "occurrence": ob, "translate": [0.0, 0.0, 1.0]}),
    );
    let j = slider(&mut s, oa, ob, [-1.0, 0.0, 0.0], (0.0, 20.0));
    // The static overlap exists.
    let all = out(ok(&mut s, json!({"cmd": "interference"})))
        .interference
        .unwrap();
    assert!(
        all.iter()
            .any(|i| [i.a.0, i.b.0] == [op, oq] || [i.a.0, i.b.0] == [oq, op]),
        "{all:?}"
    );
    let sw = out(ok(
        &mut s,
        json!({"cmd": "joint_sweep", "joint": j, "from": 0.0, "to": 20.0, "steps": 21}),
    ))
    .sweep
    .unwrap();
    // B overlaps A for travel > 13: samples 14..20 only.
    let hits: Vec<f64> = sw
        .iter()
        .filter(|x| !x.interference.is_empty())
        .map(|x| x.value)
        .collect();
    assert_eq!(hits, (14..=20).map(f64::from).collect::<Vec<_>>());
    for x in &sw {
        for i in &x.interference {
            assert!(i.a.0 == ob || i.b.0 == ob, "{i:?}");
        }
    }
}

/// #29: the last sample is the end exactly (no rounding past a limit), and
/// a range wider than f64 can subtract does not give NaN samples.
#[test]
fn sweep_end_points_are_exact_and_wide_ranges_work() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let a = block(&mut s, 10.0, 10.0, 10.0, [0.0, 0.0]);
    let b = block(&mut s, 4.0, 4.0, 4.0, [20.0, 0.0]);
    let (_, oa) = component(&mut s, "A", &[a]);
    let (_, ob) = component(&mut s, "B", &[b]);
    let max = 9007199254740994.0; // 2^53 + 2
    let j = slider(&mut s, oa, ob, [1.0, 0.0, 0.0], (-1.0, max));
    let sw = out(ok(
        &mut s,
        json!({"cmd": "joint_sweep", "joint": j, "from": -1.0, "to": max, "steps": 2}),
    ))
    .sweep
    .unwrap();
    assert_eq!(sw.last().unwrap().value, max);
    ok(
        &mut s,
        json!({"cmd": "set_joint_limits", "joint": j, "min": -1e308, "max": 1e308}),
    );
    let sw = out(ok(
        &mut s,
        json!({"cmd": "joint_sweep", "joint": j, "from": -1e308, "to": 1e308, "steps": 3}),
    ))
    .sweep
    .unwrap();
    let v: Vec<f64> = sw.iter().map(|x| x.value).collect();
    assert_eq!(v, vec![-1e308, 0.0, 1e308]);
}

/// #31 / #32: omitted axis fields keep the joint's; a zero axis is refused
/// for every kind; a child-axis face on the wrong part names the child.
#[test]
fn set_joint_axis_keeps_omitted_fields_and_refuses_zero_axis() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let a = block(&mut s, 10.0, 10.0, 10.0, [0.0, 0.0]);
    let b = block(&mut s, 4.0, 4.0, 4.0, [20.0, 0.0]);
    let c = block(&mut s, 4.0, 4.0, 4.0, [40.0, 0.0]);
    let (_, oa) = component(&mut s, "A", &[a]);
    let (_, ob) = component(&mut s, "B", &[b]);
    let (_, oc) = component(&mut s, "C", &[c]);
    let r = s.execute(&Command::AddJoint {
        name: None,
        kind: JointKind::Planar,
        parent: OccurrenceId(oa),
        child: OccurrenceId(ob),
        origin: [0.0; 3],
        axis: Some([0.0, 1.0, 0.0]),
        axis_from: None,
        value: 0.0,
        min: None,
        max: None,
        extra: vec![],
        dir2: Some([1.0, 0.0, 0.0]),
        child_axis_from: None,
        flip: false,
    });
    assert!(r.ok, "{:?}", r.error);
    let j = r.assembly.unwrap().joint.unwrap().0;
    ok(
        &mut s,
        json!({"cmd": "set_joint_axis", "joint": j, "origin": [1.0, 0.0, 0.0]}),
    );
    let jj = s.summary().assembly.joints[0].clone();
    assert_eq!(jj.origin, [1.0, 0.0, 0.0]);
    assert_eq!(jj.axis, [0.0, 1.0, 0.0], "axis kept");
    assert_eq!(jj.dir2, Some([1.0, 0.0, 0.0]), "dir2 kept");
    // A rigid joint with a zero axis is refused too.
    let r = s.execute(&Command::AddJoint {
        name: None,
        kind: JointKind::Rigid,
        parent: OccurrenceId(oa),
        child: OccurrenceId(oc),
        origin: [0.0; 3],
        axis: None,
        axis_from: None,
        value: 0.0,
        min: None,
        max: None,
        extra: vec![],
        dir2: None,
        child_axis_from: None,
        flip: false,
    });
    assert!(r.ok, "{:?}", r.error);
    let rj = r.assembly.unwrap().joint.unwrap().0;
    let e = refused(
        &mut s,
        json!({"cmd": "set_joint_axis", "joint": rj, "axis": [0.0, 0.0, 0.0]}),
    );
    assert!(e.contains("zero length"), "{e}");
    ok(&mut s, json!({"cmd": "delete_joint", "joint": rj}));
    // The child-axis face must be on the child: the message says so.
    let r = s.execute(&Command::AddJoint {
        name: None,
        kind: JointKind::Revolute,
        parent: OccurrenceId(oa),
        child: OccurrenceId(oc),
        origin: [0.0; 3],
        axis: Some([0.0, 0.0, 1.0]),
        axis_from: None,
        value: 0.0,
        min: None,
        max: None,
        extra: vec![],
        dir2: None,
        child_axis_from: Some(AxisFrom::Cylinder {
            face: FacePick {
                body: BodyId(a),
                face: 0,
            },
        }),
        flip: false,
    });
    let e = format!("{:?}", r.error);
    assert!(!r.ok && e.contains("child's component"), "{e}");
    // A zero joint axis with a child snap: a clear refusal.
    let r = s.execute(&Command::AddJoint {
        name: None,
        kind: JointKind::Revolute,
        parent: OccurrenceId(oa),
        child: OccurrenceId(oc),
        origin: [0.0; 3],
        axis: Some([0.0; 3]),
        axis_from: None,
        value: 0.0,
        min: None,
        max: None,
        extra: vec![],
        dir2: None,
        child_axis_from: Some(AxisFrom::Cylinder {
            face: FacePick {
                body: BodyId(c),
                face: 0,
            },
        }),
        flip: false,
    });
    let e = format!("{:?}", r.error);
    assert!(!r.ok && e.contains("zero length"), "{e}");
}

/// #50: an oversized rigid group is refused by count before any joint is
/// checked.
#[test]
fn oversized_rigid_group_is_refused_up_front() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let ids: Vec<u32> = (1..=10_002).collect();
    let t = std::time::Instant::now();
    let e = refused(
        &mut s,
        json!({"cmd": "create_rigid_group", "occurrences": ids}),
    );
    assert!(e.contains("more than 10000 joints"), "{e}");
    assert!(t.elapsed().as_secs_f64() < 5.0);
}

/// #17 / #38: BOM CSV text that starts like a formula is neutralized;
/// materials are listed once each.
#[test]
fn bom_csv_guards_formulas_and_lists_materials_once() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let b: Vec<u32> = (0..3)
        .map(|i| block(&mut s, 4.0, 4.0, 4.0, [10.0 * i as f64, 0.0]))
        .collect();
    for (body, m) in b.iter().zip(["Steel", "Al", "Steel"]) {
        ok(
            &mut s,
            json!({"cmd": "set_body_material", "body": body, "density": 7.0, "name": m}),
        );
    }
    component(&mut s, "=HYPERLINK(\"http://x\",\"y\")", &b);
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("bom.csv");
    ok(&mut s, json!({"cmd": "export_bom", "path": p}));
    let t = std::fs::read_to_string(&p).unwrap();
    let row = t.lines().nth(1).unwrap();
    assert!(
        row.starts_with("1,\"'=HYPERLINK(\"\"http://x\"\",\"\"y\"\")\","),
        "{row}"
    );
    assert!(row.contains(",\"Steel, Al\","), "{row}");
}

/// #19 / #30: stale configuration entries are cleaned on open with their
/// own notice, and the document is marked changed so saving keeps it.
#[test]
fn stale_configuration_entries_are_cleaned_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cfg.rcad.json");
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    ok(
        &mut s,
        json!({"cmd": "set_user_parameter", "name": "w", "expression": "5", "unit": "mm"}),
    );
    block(&mut s, 10.0, 10.0, 10.0, [0.0, 0.0]);
    ok(
        &mut s,
        json!({"cmd": "set_configuration", "name": "A", "values": {"w": "6"}}),
    );
    ok(&mut s, json!({"cmd": "save", "path": path}));
    // An older build left a value for a renamed parameter and an exclusion
    // of a deleted occurrence.
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let c = &mut v["document"]["configurations"][0];
    c["values"]["gone"] = json!("1");
    c["excluded_occurrences"] = json!([99]);
    std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
    let mut t = Session::new();
    let msg = ok(&mut t, json!({"cmd": "open", "path": path}))
        .message
        .unwrap();
    assert!(msg.contains("NOTE") && msg.contains("gone"), "{msg}");
    assert!(msg.contains("deleted occurrences"), "{msg}");
    assert!(!msg.contains("expressions do not evaluate"), "{msg}");
    assert!(t.summary().dirty, "the cleanup is an unsaved change");
    ok(
        &mut t,
        json!({"cmd": "activate_configuration", "name": "A"}),
    );
    // Saved and reopened: clean, no notice.
    ok(&mut t, json!({"cmd": "save", "path": path}));
    let mut u = Session::new();
    let msg = ok(&mut u, json!({"cmd": "open", "path": path}))
        .message
        .unwrap();
    assert!(!msg.contains("NOTE"), "{msg}");
}
