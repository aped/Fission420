//! Acceptance project "two product configurations": an enclosure
//! assembly (tray body, lid, optional fan) in an 80 mm and a 100 mm variant.
//! The basic variant excludes the fan occurrence and suppresses the drain hole;
//! the variants have different part numbers. For each: assembly mass
//! properties, the drawing's BOM and dimensions, and exports named by variant.
//! A shared dimension (wall height) is edited once and both variants and the
//! drawing follow. Closed-form expected values throughout; the configuration
//! table command edits both variants in one undo step.
mod support;
use rcad_commands::Session;
use rcad_document::drawing::DrawingId;
use serde_json::{Value, json};
use std::f64::consts::PI;
use support::tray;

fn run(s: &mut Session, steps: &[Value]) {
    for v in steps {
        tray::ok(s, v);
    }
}

const DENSITY: f64 = 1.2; // g/cm³ for every body (user density)

/// Tray: L × 60 × h box with an (L − 4) × 56 pocket 2 mm above the floor (open
/// top), plus (full only) a Ø6 drain hole through the 2 mm floor.
fn tray_volume(l: f64, h: f64, drain: bool) -> f64 {
    l * 60.0 * h - (l - 4.0) * 56.0 * (h - 2.0) - if drain { PI * 9.0 * 2.0 } else { 0.0 }
}
fn lid_volume(l: f64) -> f64 {
    l * 60.0 * 3.0
}
const FAN: f64 = 20.0 * 20.0 * 10.0;

fn build(s: &mut Session) {
    run(
        s,
        &[
            json!({"cmd": "new_document"}),
            json!({"cmd": "set_user_parameter", "name": "length", "expression": "100", "unit": "mm"}),
            json!({"cmd": "set_user_parameter", "name": "height", "expression": "30", "unit": "mm"}),
            // F1/F2 tray box, F3/F4 pocket, F5/F6 drain hole.
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 100.0, "height": 60.0}),
            json!({"cmd": "set_expression", "feature": 1, "name": "width", "expression": "length"}),
            json!({"cmd": "extrude", "sketch": 1, "distance": 30.0, "body_name": "Tray"}),
            json!({"cmd": "set_expression", "feature": 2, "name": "distance", "expression": "height"}),
            json!({"cmd": "create_sketch",
                   "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
                   "template": "centered_rectangle", "width": 96.0, "height": 56.0}),
            json!({"cmd": "set_expression", "feature": 3, "name": "width",
                   "expression": "length - 4"}),
            json!({"cmd": "extrude", "sketch": 3, "distance": 28.0, "op": "cut", "target": 1}),
            json!({"cmd": "set_expression", "feature": 4, "name": "distance",
                   "expression": "height - 2"}),
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
                   "diameter": 6.0}),
            json!({"cmd": "extrude", "sketch": 5, "op": "cut", "target": 1, "through_all": true}),
            // F7/F8 lid (L × 60 × 3 at z 0..3, placed on top by its occurrence).
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 100.0, "height": 60.0, "center": [0.0, 100.0]}),
            json!({"cmd": "set_expression", "feature": 7, "name": "width", "expression": "length"}),
            json!({"cmd": "extrude", "sketch": 7, "distance": 3.0, "body_name": "Lid"}),
            // F9/F10 optional fan 20 × 20 × 10 at the side.
            json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
                   "width": 20.0, "height": 20.0, "center": [0.0, 200.0]}),
            json!({"cmd": "extrude", "sketch": 9, "distance": 10.0, "body_name": "Fan"}),
        ],
    );
    for b in 1..=3 {
        tray::ok(
            s,
            &json!({"cmd": "set_body_material", "body": b, "density": DENSITY,
                    "name": "Resin 1.2"}),
        );
    }
    run(
        s,
        &[
            json!({"cmd": "create_component", "name": "Tray", "bodies": [1],
                   "part_number": "TR-01"}),
            json!({"cmd": "create_component", "name": "Lid", "bodies": [2],
                   "part_number": "LD-01"}),
            json!({"cmd": "create_component", "name": "Fan", "bodies": [3],
                   "part_number": "FAN-20"}),
            // Lid on top of the tray (z = height … height + 3), fan inside.
            json!({"cmd": "set_occurrence_transform", "occurrence": 2,
                   "translate": [0.0, -100.0, 30.0]}),
            json!({"cmd": "set_occurrence_transform", "occurrence": 3,
                   "translate": [0.0, -200.0, 2.0]}),
            json!({"cmd": "set_configuration", "name": "Basic", "part_number": "ENC-80",
                   "values": {"length": "80"}, "suppressed": [5, 6],
                   "excluded_occurrences": [3]}),
            json!({"cmd": "set_configuration", "name": "Pro", "part_number": "ENC-100",
                   "values": {"length": "100"}}),
            json!({"cmd": "create_drawing", "name": "Enclosure", "assembly": true,
                   "title": "Enclosure"}),
        ],
    );
}

/// (total volume, total mass, occurrence count) of the evaluated assembly.
fn totals(s: &mut Session) -> (f64, f64, usize) {
    let r = tray::ok(s, &json!({"cmd": "mass_properties"}))
        .mass_properties
        .unwrap();
    let t = r.total.expect("total");
    (t.volume, t.mass, r.occurrences.len())
}

/// BOM rows (name, part number, quantity) and the horizontal/vertical
/// dimensions of the front view, from a fresh SVG export.
fn sheet(s: &mut Session, name: &str) -> (Vec<(String, String, usize)>, Vec<f64>) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../artifacts/product_configurations");
    std::fs::create_dir_all(&dir).unwrap();
    let r = tray::ok(
        s,
        &json!({"cmd": "export_drawing", "drawing": 1, "path": dir.join(name)}),
    );
    let rep = r.drawing_report.unwrap();
    assert!(rep.issues.is_empty(), "{:?}", rep.issues);
    let bom = rep
        .bom
        .iter()
        .map(|b| (b.name.clone(), b.part_number.clone(), b.quantity))
        .collect();
    let dims = rep.views[0]
        .dimensions
        .iter()
        .filter(|d| d.kind == "horizontal" || d.kind == "vertical")
        .filter_map(|d| d.value.map(|v| (v * 1e6).round() / 1e6))
        .collect();
    (bom, dims)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * b.abs().max(1.0)
}

fn check_variant(s: &mut Session, l: f64, h: f64, basic: bool, file: &str) {
    let want_v = tray_volume(l, h, !basic) + lid_volume(l) + if basic { 0.0 } else { FAN };
    let (v, m, n) = totals(s);
    assert!(close(v, want_v), "{file}: volume {v} vs {want_v}");
    assert!(
        close(m, want_v * DENSITY / 1000.0),
        "{file}: mass {m} g vs {}",
        want_v * DENSITY / 1000.0
    );
    assert_eq!(n, if basic { 2 } else { 3 });
    let (bom, dims) = sheet(s, file);
    let mut want = vec![
        ("Tray".to_string(), "TR-01".to_string(), 1),
        ("Lid".to_string(), "LD-01".to_string(), 1),
    ];
    if !basic {
        want.push(("Fan".into(), "FAN-20".into(), 1));
    }
    assert_eq!(bom, want, "{file}");
    // Front view: overall width L, overall height h + 3 (lid on top).
    assert_eq!(dims, vec![l, h + 3.0], "{file}");
}

#[test]
fn two_product_configurations_with_assembly_properties_bom_and_drawings() {
    let mut s = Session::new();
    build(&mut s);
    tray::ok(
        &mut s,
        &json!({"cmd": "activate_configuration", "name": "Basic"}),
    );
    check_variant(&mut s, 80.0, 30.0, true, "basic_h30.svg");
    tray::ok(
        &mut s,
        &json!({"cmd": "activate_configuration", "name": "Pro"}),
    );
    check_variant(&mut s, 100.0, 30.0, false, "pro_h30.svg");

    // Shared dimension (not configured): height 30 -> 36 once. The lid
    // occurrence's placement is not driven by the parameter (no joint), so
    // move it too; then both variants follow.
    run(
        &mut s,
        &[
            json!({"cmd": "set_user_parameter", "name": "height", "expression": "36", "unit": "mm"}),
            json!({"cmd": "set_occurrence_transform", "occurrence": 2,
                   "translate": [0.0, -100.0, 36.0]}),
        ],
    );
    check_variant(&mut s, 100.0, 36.0, false, "pro_h36.svg");
    tray::ok(
        &mut s,
        &json!({"cmd": "activate_configuration", "name": "Basic"}),
    );
    check_variant(&mut s, 80.0, 36.0, true, "basic_h36.svg");

    // Variant exports named by configuration and part number; an existing
    // file is refused without overwrite.
    let dir = tempfile::tempdir().unwrap();
    tray::ok(
        &mut s,
        &json!({"cmd": "export_configurations", "dir": dir.path(), "stem": "enclosure",
                "step": true}),
    );
    for f in ["enclosure-Basic-ENC-80.step", "enclosure-Pro-ENC-100.step"] {
        assert!(dir.path().join(f).is_file(), "{f}");
    }
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "export_configurations", "dir": dir.path(), "stem": "enclosure",
                "step": true}),
    );
    assert!(!r.ok, "existing files must not be overwritten silently");

    // Table edit (one undo step): Basic becomes 90 mm "ENC-90" and keeps the
    // fan; Pro unchanged. Duplicate names are refused unchanged.
    let mut table = s.model().unwrap().document().configurations.clone();
    table[0].values.insert("length".into(), "90".into());
    table[0].part_number = "ENC-90".into();
    table[0].excluded_occurrences.clear();
    let mut dup = table.clone();
    dup[1].name = "Basic".into();
    let rev = s.summary().revision;
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "set_configuration_table", "configurations": dup}),
    );
    assert!(!r.ok && s.summary().revision == rev);
    tray::ok(
        &mut s,
        &json!({"cmd": "set_configuration_table", "configurations": table}),
    );
    // Basic is active and re-applied: 90 mm with the fan, drain suppressed.
    let want = tray_volume(90.0, 36.0, false) + lid_volume(90.0) + FAN;
    assert!(close(totals(&mut s).0, want));
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    check_variant(&mut s, 80.0, 36.0, true, "basic_undo.svg");

    // Save / reopen keeps both variants, the drawing and the active one.
    let path = dir.path().join("enclosure.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": path}));
    assert_eq!(t.summary().active_configuration.as_deref(), Some("Basic"));
    check_variant(&mut t, 80.0, 36.0, true, "basic_reopened.svg");
    tray::ok(
        &mut t,
        &json!({"cmd": "activate_configuration", "name": "Pro"}),
    );
    check_variant(&mut t, 100.0, 36.0, false, "pro_reopened.svg");
    let _ = DrawingId(1);
}
