//! Native format compatibility evidence for docs/FORMAT_SUPPORT.md.
//!
//! * v1: `crates/f420-document/tests/fixtures/plate_v1.f420.json` (run 1).
//! * v2: `tests/fixtures/native_v2/*.f420.json`, frozen copies of the run-2 example
//!   documents (the `examples/*_workflow.json` replays in `cargo xtask check` re-save the
//!   examples themselves as v3, so they cannot serve as v2 evidence).
//! * v3: the committed run-3 examples, and whatever version the other examples are in.
//!
//! Each opens through `Command::Open`, regenerates to the analytic volume, re-saves as v3
//! and reopens to an identical document.
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

/// `want_version`: exact version required (None: any supported version).
fn check(rel: &str, want_version: Option<u64>, expected: f64) {
    let path = repo().join(rel);
    if let Some(v) = want_version {
        assert_eq!(version(&path), v, "{rel}");
    }
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
const PLATE: f64 = 100.0 * 50.0 * 10.0 - 250.0 * PI; // 100 x 50 x 10, centred Ø10
const SCENARIO_A: f64 = 60000.0 - 300.0 * PI; // 100 x 50 x 12, centred Ø10
fn scenario_b() -> f64 {
    5.0 * (2400.0 - 5.0 * 44f64.sqrt()) // 5 mm prism, 60x40 minus a corner triangle
}
const SCENARIO_R: f64 = 10.0 * (80.0 * 50.0 - (4.0 - PI) * 144.0 - 25.0 * PI); // r 12, Ø10
/// 120 x 60 x 6 with four Ø6.5 and one Ø20 through holes: 43200 - 6 pi (4*3.25^2 + 10^2).
const BRACKET: f64 = 43200.0 - 6.0 * PI * (4.0 * 3.25 * 3.25 + 100.0);

#[test]
fn v1_plate_fixture() {
    check(
        "crates/f420-document/tests/fixtures/plate_v1.f420.json",
        Some(1),
        PLATE,
    );
}

#[test]
fn v2_run2_documents() {
    let d = "crates/f420-commands/tests/fixtures/native_v2";
    check(&format!("{d}/plate.f420.json"), Some(2), PLATE);
    check(&format!("{d}/scenario_a.f420.json"), Some(2), SCENARIO_A);
    check(&format!("{d}/scenario_b.f420.json"), Some(2), scenario_b());
    check(&format!("{d}/scenario_r.f420.json"), Some(2), SCENARIO_R);
    check(&format!("{d}/bracket.f420.json"), Some(2), BRACKET);
}

#[test]
fn committed_examples() {
    check("examples/plate.f420.json", None, PLATE);
    check("examples/scenario_a.f420.json", None, SCENARIO_A);
    check("examples/scenario_b.f420.json", None, scenario_b());
    check("examples/scenario_r.f420.json", None, SCENARIO_R);
    check("examples/bracket.f420.json", None, BRACKET);
}

#[test]
fn v3_run3_examples() {
    // Imported FreeCAD box with a centred Ø6 through hole.
    check(
        "examples/imported_step_hole.f420.json",
        Some(3),
        12000.0 - 90.0 * PI,
    );
    // DXF notch plate (the STL body is a mesh: no B-rep measurements).
    check(
        "examples/dxf_stl_import.f420.json",
        Some(3),
        3.0 * (800.0 - 12.5 * PI),
    );
}
