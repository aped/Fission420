//! Hierarchical STEP export (`export_step_assembly`, run 7): refusals, the
//! product-structure entities, and a re-read through R-CAD's importer (placements
//! from the NAUO/CDSR chain; not an independent check — the FreeCAD check is
//! tools/check_step_assembly.sh).
use rcad_kernel::*;

fn rect_at(cx: f64, cy: f64, w: f64, h: f64) -> Profile {
    Profile::Polygon(vec![
        [cx - w / 2.0, cy - h / 2.0],
        [cx + w / 2.0, cy - h / 2.0],
        [cx + w / 2.0, cy + h / 2.0],
        [cx - w / 2.0, cy + h / 2.0],
    ])
}

fn part(b: &Body, c: Vec<(usize, String, Rigid)>) -> StepProduct<'_> {
    StepProduct {
        name: "Part".into(),
        solids: vec![("part".into(), b)],
        children: c,
    }
}

fn asm<'a>(name: &str, c: Vec<(usize, String, Rigid)>) -> StepProduct<'a> {
    StepProduct {
        name: name.into(),
        solids: vec![],
        children: c,
    }
}

#[test]
fn refusals() {
    let b = extrude(&rect_at(0.0, 0.0, 2.0, 2.0), &Frame::XY, 1.0).unwrap();
    let id = Rigid::IDENTITY;
    let cyc = [
        asm("A", vec![(1, "x".into(), id)]),
        asm("B", vec![(0, "y".into(), id)]),
    ];
    assert!(export_step_assembly(&cyc, 0, "t").is_err());
    assert!(export_step_assembly(&[asm("A", vec![])], 0, "t").is_err());
    assert!(export_step_assembly(&[asm("A", vec![(5, "x".into(), id)])], 0, "t").is_err());
    let mut scaled = id;
    scaled.r[0][0] = 2.0;
    let p = [asm("A", vec![(1, "x".into(), scaled)]), part(&b, vec![])];
    assert!(export_step_assembly(&p, 0, "t").is_err());
    assert!(export_step_assembly(&p, 7, "t").is_err());
}

/// Root A uses sub-assembly S twice (identity and +100 y); S uses the part
/// (2 × 2 × 1 box centred at the origin, V 4) at identity and rotated 90°
/// about Z then +10 x. Four placed instances; the part is written once.
#[test]
fn two_level_structure_and_reread() {
    let b = extrude(&rect_at(1.0, 0.0, 2.0, 2.0), &Frame::XY, 1.0).unwrap();
    let id = Rigid::IDENTITY;
    let rot = Rigid::new(
        [10.0, 0.0, 0.0],
        [0.0; 3],
        [0.0, 0.0, 1.0],
        std::f64::consts::FRAC_PI_2,
    )
    .unwrap();
    let up = Rigid::new([0.0, 100.0, 0.0], [0.0; 3], [0.0, 0.0, 1.0], 0.0).unwrap();
    let p = [
        asm(
            "Top",
            vec![(1, "Sub#1".into(), id), (1, "Sub#2".into(), up)],
        ),
        asm(
            "Sub",
            vec![(2, "Part#1".into(), id), (2, "Part#2".into(), rot)],
        ),
        part(&b, vec![]),
    ];
    let text = export_step_assembly(&p, 0, "R-CAD").unwrap();
    assert_eq!(text.matches("NEXT_ASSEMBLY_USAGE_OCCURRENCE(").count(), 4);
    assert_eq!(
        text.matches("CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(")
            .count(),
        4
    );
    assert_eq!(text.matches("= PRODUCT(").count(), 3);
    assert_eq!(text.matches("MANIFOLD_SOLID_BREP(").count(), 1);
    assert_eq!(text.matches("APPLICATION_CONTEXT(").count(), 1);
    for n in ["'Top'", "'Sub'", "'Part'", "'Sub#2'", "'Part#2'"] {
        assert!(text.contains(n), "{n}");
    }
    let imp = import_step(&text, &StepImportOptions::default()).unwrap();
    assert_eq!(imp.solids.len(), 4);
    assert!(imp.warnings.iter().any(|w| w.contains("flattened")));
    // Part centroid (1, 0, 0.5): Part#2 → rotate 90° → (0, 1, 0.5) + 10 x.
    let mut c: Vec<[i64; 3]> = imp
        .solids
        .iter()
        .map(|s| {
            let bb = bounds(&s.body).unwrap();
            std::array::from_fn(|i| ((bb.min[i] + bb.max[i]) / 2.0 * 1e6).round() as i64)
        })
        .collect();
    c.sort();
    let mut want: Vec<[i64; 3]> = [
        [1.0, 0.0, 0.5],
        [10.0, 1.0, 0.5],
        [1.0, 100.0, 0.5],
        [10.0, 101.0, 0.5],
    ]
    .iter()
    .map(|p: &[f64; 3]| p.map(|x| (x * 1e6).round() as i64))
    .collect();
    want.sort();
    assert_eq!(c, want);
    for s in &imp.solids {
        assert_eq!(s.name.as_deref(), Some("Part"));
    }
}

fn with_structure() -> StepImportOptions {
    StepImportOptions {
        assembly: true,
        ..Default::default()
    }
}

fn centre(b: &Body) -> [f64; 3] {
    let bb = bounds(b).unwrap();
    std::array::from_fn(|i| (bb.min[i] + bb.max[i]) / 2.0)
}

/// The product structure is read back: definitions in their own coordinates
/// (written once), usages with placements; composing them reproduces the
/// flattened placements.
#[test]
fn structure_is_read_back() {
    let b = extrude(&rect_at(1.0, 0.0, 2.0, 2.0), &Frame::XY, 1.0).unwrap();
    let id = Rigid::IDENTITY;
    let rot = Rigid::new(
        [10.0, 0.0, 0.0],
        [0.0; 3],
        [0.0, 0.0, 1.0],
        std::f64::consts::FRAC_PI_2,
    )
    .unwrap();
    let up = Rigid::new([0.0, 100.0, 0.0], [0.0; 3], [0.0, 0.0, 1.0], 0.0).unwrap();
    let p = [
        asm(
            "Top",
            vec![(1, "Sub#1".into(), id), (1, "Sub#2".into(), up)],
        ),
        asm(
            "Sub",
            vec![(2, "Part#1".into(), id), (2, "Part#2".into(), rot)],
        ),
        part(&b, vec![]),
    ];
    let text = export_step_assembly(&p, 0, "R-CAD").unwrap();
    // Plain import: no structure is read (review run 7 #2).
    assert!(
        import_step(&text, &StepImportOptions::default())
            .unwrap()
            .assembly
            .is_none()
    );
    let imp = import_step(&text, &with_structure()).unwrap();
    // Review run 7 #36: the flattened view is not built with the structure.
    assert!(imp.solids.is_empty());
    assert_eq!(imp.stats.solids, 1);
    let a = imp.assembly.expect("structure");
    let by = |n: &str| a.products.iter().position(|p| p.name == n).unwrap();
    let (top, sub, prt) = (by("Top"), by("Sub"), by("Part"));
    assert_eq!(a.roots, vec![top]);
    assert_eq!(a.products[top].children.len(), 2);
    assert_eq!(a.products[sub].children.len(), 2);
    assert!(a.products[top].solids.is_empty() && a.products[sub].solids.is_empty());
    assert_eq!(a.products[prt].solids.len(), 1);
    let c = centre(&a.products[prt].solids[0].body);
    assert!(
        (0..3).all(|i| (c[i] - [1.0, 0.0, 0.5][i]).abs() < 1e-9),
        "{c:?}"
    );
    let names: Vec<&str> = a.products[sub]
        .children
        .iter()
        .map(|x| x.1.as_str())
        .collect();
    assert_eq!(names, ["Part#1", "Part#2"]);
    // Part#2 placement = rot (rotation about Z by 90°, +10 x).
    let t = a.products[sub].children[1].2;
    for i in 0..3 {
        for j in 0..3 {
            assert!((t.r[i][j] - rot.r[i][j]).abs() < 1e-9);
        }
        assert!((t.t[i] - rot.t[i]).abs() < 1e-9);
    }
}

/// An OpenCascade-written assembly (FreeCAD `Import.export`, independent of
/// R-CAD's writer): two products under the root, their placements composing to
/// the flattened solids.
#[test]
fn occt_assembly_structure() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/step/two_boxes_assembly_mm.step"),
    )
    .unwrap();
    let imp = import_step(&text, &with_structure()).unwrap();
    let flat_import = import_step(&text, &StepImportOptions::default()).unwrap();
    let a = imp.assembly.expect("structure");
    assert_eq!(a.roots.len(), 1);
    let root = &a.products[a.roots[0]];
    assert_eq!(root.children.len(), 2);
    let mut placed: Vec<[i64; 3]> = Vec::new();
    for (c, _, t) in &root.children {
        let p = &a.products[*c];
        assert_eq!(p.solids.len(), 1, "{}", p.name);
        let w = transform_body(&p.solids[0].body, t).unwrap();
        placed.push(centre(&w).map(|x| (x * 1e6).round() as i64));
    }
    let mut flat: Vec<[i64; 3]> = flat_import
        .solids
        .iter()
        .map(|s| centre(&s.body).map(|x| (x * 1e6).round() as i64))
        .collect();
    placed.sort();
    flat.sort();
    assert_eq!(placed, flat);
}

fn occt_fixture() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/step/two_boxes_assembly_mm.step"),
    )
    .unwrap()
}

/// `text` without the entity instances `ids` (each `#id = ...;`).
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

/// Review run 7 #2: a usage without a placement (no
/// CONTEXT_DEPENDENT_SHAPE_REPRESENTATION) does not stop a plain import (or
/// the regeneration of an embedded copy); only the structured import refuses.
#[test]
fn unplaced_usage_fails_only_the_structured_import() {
    let text = drop_entities(&occt_fixture(), &[192]);
    let plain = import_step(&text, &StepImportOptions::default()).unwrap();
    assert_eq!(plain.solids.len(), 2);
    assert!(plain.assembly.is_none());
    let e = import_step(&text, &with_structure())
        .unwrap_err()
        .to_string();
    assert!(e.contains("no placement"), "{e}");
}

/// Review run 7 #7: a free-standing part product (used by no assembly) is a
/// root of the structure, so its solid is not lost.
#[test]
fn free_standing_part_is_a_root() {
    // BoxB's usage removed: BoxB is a top-level part beside the assembly.
    let text = drop_entities(&occt_fixture(), &[361, 362, 363, 364, 365]);
    let plain = import_step(&text, &StepImportOptions::default()).unwrap();
    assert_eq!(plain.solids.len(), 2);
    let imp = import_step(&text, &with_structure()).unwrap();
    let a = imp.assembly.unwrap();
    let names: Vec<&str> = a
        .roots
        .iter()
        .map(|&r| a.products[r].name.as_str())
        .collect();
    assert!(names.contains(&"BoxB"), "{names:?}");
    let total: usize = a.products.iter().map(|p| p.solids.len()).sum();
    assert_eq!(total, 2);
}

/// Review run 7 #8: a solid whose representation is placed in its part's
/// shape representation by a relationship with transformation (not an
/// identity link) is folded into that part; a solid in no representation is
/// placed under an extra root. Both are reported.
#[test]
fn solids_outside_product_shapes_are_kept_and_reported() {
    let mut text = occt_fixture();
    // BoxB's SDR points at a new part SR #900; the B-rep representation #205
    // is placed in it by an identity-transform RRWT (#901/#902); the usage's
    // relationship now places #900 in the assembly.
    text = text.replace(
        "#198 = SHAPE_DEFINITION_REPRESENTATION(#199,#205);",
        "#198 = SHAPE_DEFINITION_REPRESENTATION(#199,#900);",
    );
    text = text.replace(
        "#362 = ( REPRESENTATION_RELATIONSHIP('','',#205,#10)",
        "#362 = ( REPRESENTATION_RELATIONSHIP('','',#900,#10)",
    );
    // A second solid on BoxA's shell outside every representation.
    text = text.replace(
        "ENDSEC;\nEND-ISO",
        "#900 = SHAPE_REPRESENTATION('',(#11),#356);\n\
         #901 = ( REPRESENTATION_RELATIONSHIP('','',#205,#900) \
         REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#902) \
         SHAPE_REPRESENTATION_RELATIONSHIP() );\n\
         #902 = ITEM_DEFINED_TRANSFORMATION('','',#11,#11);\n\
         #950 = MANIFOLD_SOLID_BREP('stray',#38);\nENDSEC;\nEND-ISO",
    );
    assert!(text.contains("#950 ="), "fixture layout changed");
    let plain = import_step(&text, &StepImportOptions::default()).unwrap();
    assert_eq!(plain.solids.len(), 3);
    let imp = import_step(&text, &with_structure()).unwrap();
    let a = imp.assembly.unwrap();
    let total: usize = a.products.iter().map(|p| p.solids.len()).sum();
    assert_eq!(total, 3, "every solid is in the structure");
    let boxb = a.products.iter().find(|p| p.name == "BoxB").unwrap();
    assert_eq!(boxb.solids.len(), 1, "folded into BoxB");
    let loose = a
        .roots
        .iter()
        .map(|&r| &a.products[r])
        .find(|p| p.name == "Unstructured solids")
        .expect("extra root");
    assert_eq!(loose.children.len(), 1);
    assert_eq!(a.products[loose.children[0].0].solids.len(), 1);
    let w = imp.warnings.join("\n");
    assert!(w.contains("#206") && w.contains("BoxB"), "{w}");
    assert!(
        w.contains("#950") && w.contains("Unstructured solids"),
        "{w}"
    );
    // The placed geometry matches the flattened import.
    let mut placed: Vec<[i64; 3]> = Vec::new();
    fn walk(a: &ImportedAssembly, p: usize, t: Rigid, out: &mut Vec<[i64; 3]>) {
        for s in &a.products[p].solids {
            let w = transform_body(&s.body, &t).unwrap();
            out.push(centre(&w).map(|x| (x * 1e6).round() as i64));
        }
        for (c, _, m) in &a.products[p].children {
            walk(a, *c, t.after(m), out);
        }
    }
    for &r in &a.roots {
        walk(&a, r, Rigid::IDENTITY, &mut placed);
    }
    let mut flat: Vec<[i64; 3]> = plain
        .solids
        .iter()
        .map(|s| centre(&s.body).map(|x| (x * 1e6).round() as i64))
        .collect();
    placed.sort();
    flat.sort();
    assert_eq!(placed, flat);
}

/// Review run 7 #42: the structure is refused past the document's nesting
/// limit before any solid is built; at the limit it is read.
#[test]
fn structure_depth_matches_the_document_limit() {
    let b = extrude(&rect_at(0.0, 0.0, 2.0, 2.0), &Frame::XY, 1.0).unwrap();
    let chain = |levels: usize| {
        // `levels` products in a chain (paths of `levels` usages incl. root).
        let mut p: Vec<StepProduct> = (0..levels - 1)
            .map(|i| {
                asm(
                    &format!("A{i}"),
                    vec![(i + 1, format!("u{i}"), Rigid::IDENTITY)],
                )
            })
            .collect();
        p.push(part(&b, vec![]));
        export_step_assembly(&p, 0, "t").unwrap()
    };
    let ok = import_step(&chain(STEP_MAX_ASSEMBLY_DEPTH), &with_structure()).unwrap();
    assert!(ok.assembly.is_some());
    let e = import_step(&chain(STEP_MAX_ASSEMBLY_DEPTH + 1), &with_structure())
        .unwrap_err()
        .to_string();
    assert!(e.contains("deeper than 16"), "{e}");
    // The plain (flattened) import is unaffected.
    import_step(
        &chain(STEP_MAX_ASSEMBLY_DEPTH + 1),
        &StepImportOptions::default(),
    )
    .unwrap();
}

/// Run 7 (corpus a1): R-CAD's writer stores revolved surfaces as
/// SURFACE_OF_REVOLUTION; an embedded copy of an imported cylinder, sphere or
/// torus must read back with the same volume (exact for the cylinder,
/// within the estimate otherwise).
#[test]
fn surfaces_of_revolution_written_by_r_cad_read_back() {
    for (name, want) in [
        (
            "cylinder_d20_h15_mm.step",
            std::f64::consts::PI * 100.0 * 15.0,
        ),
        (
            "torus_R20_r5_mm.step",
            2.0 * std::f64::consts::PI.powi(2) * 20.0 * 25.0,
        ),
    ] {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/step")
                .join(name),
        )
        .unwrap();
        let a = import_step(&text, &StepImportOptions::default()).unwrap();
        let body = &a.solids[0].body;
        let copy = export_step_bodies(&[("s", body)], "t").unwrap();
        assert!(copy.contains("SURFACE_OF_REVOLUTION"), "{name}");
        let b = import_step(&copy, &StepImportOptions::default())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(b.solids.len(), 1);
        let vol = |b: &Body| match measure(b).unwrap().volume {
            VolumeResult::Value {
                volume,
                error_estimate,
                ..
            } => (volume, error_estimate),
            VolumeResult::Unavailable { reason } => panic!("{name}: {reason}"),
        };
        let (v0, e0) = vol(body);
        let (v1, e1) = vol(&b.solids[0].body);
        let tol = (e0 + e1).max(1e-9 * want);
        assert!((v1 - v0).abs() <= tol, "{name}: {v0} vs {v1} (±{tol})");
        assert!(
            (v1 - want).abs() <= e1.max(1e-9 * want) + 1e-6 * want,
            "{name}: {v1} vs {want}"
        );
    }
}

/// Known limit (run 7): a sphere written by R-CAD (SURFACE_OF_REVOLUTION
/// with poles) does not read back yet; it must be refused with an error,
/// never imported wrongly. Replace with a volume check when supported.
#[test]
fn written_sphere_is_refused_not_misread() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/step/sphere_r10_mm.step"),
    )
    .unwrap();
    let a = import_step(&text, &StepImportOptions::default()).unwrap();
    let copy = export_step_bodies(&[("s", &a.solids[0].body)], "t").unwrap();
    if let Ok(b) = import_step(&copy, &StepImportOptions::default()) {
        let v = match measure(&b.solids[0].body).unwrap().volume {
            VolumeResult::Value {
                volume,
                error_estimate,
                ..
            } => (volume, error_estimate),
            VolumeResult::Unavailable { reason } => panic!("{reason}"),
        };
        let want = 4.0 / 3.0 * std::f64::consts::PI * 1000.0;
        assert!((v.0 - want).abs() <= v.1 + 1e-6 * want, "{v:?}");
    }
}
