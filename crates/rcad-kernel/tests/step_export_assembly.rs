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
