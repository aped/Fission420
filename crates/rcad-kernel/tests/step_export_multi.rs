//! Multi-body STEP export: structural checks in Rust plus an `#[ignore]`d writer for
//! the independent FreeCAD/OpenCascade check (`tools/check_step_multi.sh`).
use rcad_kernel::*;
use std::f64::consts::PI;
use truck_stepio::r#in::{Table, ruststep};

fn rect_at(cx: f64, cy: f64, w: f64, h: f64) -> Profile {
    Profile::Polygon(vec![
        [cx - w / 2.0, cy - h / 2.0],
        [cx + w / 2.0, cy - h / 2.0],
        [cx + w / 2.0, cy + h / 2.0],
        [cx - w / 2.0, cy + h / 2.0],
    ])
}

/// Three disjoint bodies; analytic volumes are restated in tools/check_step_multi.sh.
/// - "Block": 20 x 10 x 5 box centred at (0, 0): 1000
/// - "Plate 'B' Ø8": 30 x 20 x 10 box centred at (50, 0), centred Ø8 through hole:
///   6000 - 160*pi
/// - "Pin": Ø10 cylinder 12 high centred at (0, 40): 300*pi
fn bodies() -> Vec<(&'static str, Body, f64)> {
    let block = extrude(&rect_at(0.0, 0.0, 20.0, 10.0), &Frame::XY, 5.0).unwrap();
    let plate = extrude(&rect_at(50.0, 0.0, 30.0, 20.0), &Frame::XY, 10.0).unwrap();
    let plate = cut_through_all(
        &plate,
        &Profile::Circle {
            center: [50.0, 0.0],
            radius: 4.0,
        },
        &Frame::XY.offset(10.0),
    )
    .unwrap();
    let pin = extrude(
        &Profile::Circle {
            center: [0.0, 40.0],
            radius: 5.0,
        },
        &Frame::XY,
        12.0,
    )
    .unwrap();
    vec![
        ("Block", block, 1000.0),
        ("Plate 'B' Ø8", plate, 6000.0 - 160.0 * PI),
        ("Pin", pin, 300.0 * PI),
    ]
}

/// "Plate 'B' Ø8" as a STEP string literal (apostrophe and Ø as `\X2\` code units).
const PLATE_LABEL: &str = "('Plate \\X2\\0027\\X0\\B\\X2\\0027\\X0\\ \\X2\\00D8\\X0\\8', #";

fn export(b: &[(&'static str, Body, f64)]) -> String {
    let refs: Vec<(&str, &Body)> = b.iter().map(|(n, body, _)| (*n, body)).collect();
    export_step_bodies(&refs, "R-CAD test").unwrap()
}

fn solid_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|l| l.contains(" = MANIFOLD_SOLID_BREP(") || l.contains(" = BREP_WITH_VOIDS("))
        .collect()
}

fn assert_unique_ids(text: &str) {
    let mut seen = std::collections::HashSet::new();
    for line in text.lines() {
        if let Some((id, _)) = line.split_once(" = ")
            && id.starts_with('#')
        {
            assert!(seen.insert(id), "duplicate entity {id}");
        }
    }
}

fn parsed_shell_count(text: &str) -> usize {
    let exchange = ruststep::parser::parse(text).expect("truck-stepio reader parses the file");
    Table::from_data_section(&exchange.data[0]).shell.len()
}

#[test]
fn three_bodies_three_solids() {
    let b = bodies();
    // The kernel's own volumes agree with the analytic ones (sanity for the fixture).
    for (name, body, v) in &b {
        if let VolumeResult::Value { volume, .. } = measure(body).unwrap().volume {
            assert!(
                (volume - v).abs() <= 0.001f64.max(v * 1e-7),
                "{name}: {volume} vs {v}"
            );
        } else {
            panic!("{name}: volume unavailable");
        }
    }
    let text = export(&b);
    let solids = solid_lines(&text);
    assert_eq!(solids.len(), 3, "{solids:?}");
    assert!(solids.iter().all(|l| l.contains("MANIFOLD_SOLID_BREP(")));
    // Names in input order, STEP-escaped.
    assert!(solids[0].contains("('Block', #"), "{}", solids[0]);
    assert!(solids[1].contains(PLATE_LABEL), "{}", solids[1]);
    assert!(solids[2].contains("('Pin', #"), "{}", solids[2]);
    // The shape representation lists exactly these solids.
    let ids: Vec<&str> = solids
        .iter()
        .map(|l| l.split_once(" = ").unwrap().0)
        .collect();
    let rep = text
        .lines()
        .find(|l| l.contains("ADVANCED_BREP_SHAPE_REPRESENTATION"))
        .unwrap();
    assert!(rep.contains(&format!("({})", ids.join(", "))), "{rep}");
    assert_unique_ids(&text);
    // No implicit boolean edges survive (the hole rims are exact curves).
    assert!(!text.contains("INTERSECTION_CURVE"));
    assert_eq!(parsed_shell_count(&text), 3);
    // Exactly one STEP product (writer limitation, documented on export_step_bodies).
    assert_eq!(text.matches("= PRODUCT(").count(), 1);
}

#[test]
fn single_body_matches_export_step_structure() {
    let b = bodies();
    let one = &b[1..2];
    let multi = export(one);
    let single = export_step(&one[0].1, "R-CAD test").unwrap();
    // Same entities apart from the header timestamp and the solid label.
    let data = |t: &str| t.split_once("DATA;").unwrap().1.to_string();
    let relabelled = data(&multi).replacen(PLATE_LABEL, "('', #", 1);
    assert_eq!(relabelled, data(&single));
    assert_eq!(solid_lines(&multi).len(), 1);
    assert_eq!(parsed_shell_count(&multi), 1);
}

#[test]
fn empty_slice_is_invalid_input() {
    assert!(matches!(
        export_step_bodies(&[], "x"),
        Err(KernelError::InvalidInput(_))
    ));
}

#[test]
fn same_body_twice_keeps_ids_unique() {
    // The same Body may be exported twice (e.g. two instances); ids stay unique.
    let b = bodies();
    let text = export_step_bodies(&[("a", &b[0].1), ("b", &b[0].1)], "x").unwrap();
    assert_eq!(solid_lines(&text).len(), 2);
    assert_unique_ids(&text);
    assert_eq!(parsed_shell_count(&text), 2);
}

/// Writes artifacts/step_multi/multi.step for tools/check_step_multi.sh (FreeCAD).
#[test]
#[ignore = "writes artifacts/ for the external FreeCAD check; run via tools/check_step_multi.sh"]
fn write_multi_step_artifact() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/step_multi");
    std::fs::create_dir_all(&dir).unwrap();
    let b = bodies();
    std::fs::write(dir.join("multi.step"), export(&b)).unwrap();
}
