//! Native format compatibility evidence for docs/FORMAT_SUPPORT.md: every
//! committed example document (format v2 from run 2, v3 from run 3) and the
//! v1 fixture opens through `Command::Open`, regenerates to the analytic
//! volume, re-saves as v3 and reopens to an identical document.
use f420_commands::{Command, Session};
use f420_kernel::VolumeResult;
use std::f64::consts::PI;
use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn version(p: &std::path::Path) -> u64 {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    v["version"].as_u64().unwrap()
}

/// Total volume of all B-rep bodies.
fn total_volume(s: &Session) -> f64 {
    s.summary()
        .bodies
        .iter()
        .filter_map(|b| b.measurements.as_ref())
        .map(|m| match m.volume {
            VolumeResult::Value { volume, .. } => volume,
            ref v => panic!("{v:?}"),
        })
        .sum()
}

fn check(rel: &str, want_version: u64, expected: f64) {
    let path = repo().join(rel);
    assert_eq!(version(&path), want_version, "{rel}");
    let mut s = Session::new();
    let r = s.execute(&Command::Open { path: path.clone() });
    assert!(r.ok, "{rel}: {:?}", r.error);
    let v = total_volume(&s);
    assert!(
        (v - expected).abs() <= 0.001f64.max(expected * 1e-7),
        "{rel}: {v} vs {expected}"
    );
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("resaved.f420.json");
    assert!(s.execute(&Command::Save { path: out.clone() }).ok);
    assert_eq!(version(&out), 3);
    let mut t = Session::new();
    assert!(t.execute(&Command::Open { path: out }).ok);
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert_eq!(total_volume(&t), v);
}

// Analytic volumes (same derivations as tools/external_validation.sh).

#[test]
fn v1_plate_fixture() {
    // 100 x 50 x 10 plate (width/height/distance/diameter in the file), centred Ø10 through hole.
    check(
        "crates/f420-document/tests/fixtures/plate_v1.f420.json",
        1,
        100.0 * 50.0 * 10.0 - 250.0 * PI,
    );
}

#[test]
fn v2_plate_example() {
    // 100 x 50 x 10, centred Ø10 through hole.
    check("examples/plate.f420.json", 2, 50000.0 - 250.0 * PI);
}

#[test]
fn v2_scenario_a_example() {
    // 100 x 50 x 12, centred Ø10 through hole.
    check("examples/scenario_a.f420.json", 2, 60000.0 - 300.0 * PI);
}

#[test]
fn v2_scenario_b_example() {
    // 5 mm prism, area 60*40 - 0.5*10*sqrt(12^2 - 10^2).
    check(
        "examples/scenario_b.f420.json",
        2,
        5.0 * (2400.0 - 5.0 * 44f64.sqrt()),
    );
}

#[test]
fn v2_scenario_r_example() {
    // 80 x 50 rounded rectangle r 12, 10 thick, centred Ø10 through hole.
    check(
        "examples/scenario_r.f420.json",
        2,
        10.0 * (80.0 * 50.0 - (4.0 - PI) * 144.0 - 25.0 * PI),
    );
}

#[test]
fn v3_imported_step_hole_example() {
    check(
        "examples/imported_step_hole.f420.json",
        3,
        12000.0 - 90.0 * PI,
    );
}

#[test]
fn v3_dxf_stl_import_example() {
    // B-rep notch plate only (the STL body is a mesh: no B-rep measurements).
    check(
        "examples/dxf_stl_import.f420.json",
        3,
        3.0 * (800.0 - 12.5 * PI),
    );
}
