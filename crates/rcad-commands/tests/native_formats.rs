//! Native format compatibility evidence for docs/FORMAT_SUPPORT.md.
//!
//! * v1: `crates/rcad-document/tests/fixtures/plate_v1.rcad.json` (run 1).
//! * v2: `tests/fixtures/native_v2/*.rcad.json`, frozen copies of the run-2 example
//!   documents (the `examples/*_workflow.json` replays in `cargo xtask check` re-save the
//!   examples themselves as v3, so they cannot serve as v2 evidence).
//! * v3: the committed run-3 examples, and whatever version the other examples are in.
//! * v4: `tests/fixtures/native_v4/*.rcad.json`, frozen copies of the run-4 printable
//!   examples (named parameters and expressions); the committed examples are re-saved by
//!   the xtask replays, so they are checked separately.
//!
//! Each opens through `Command::Open`, regenerates to the analytic volume, re-saves as v3
//! and reopens to an identical document.
use rcad_commands::printables::{COUPON, COUPON_CLEARANCES, ENCLOSURE_BASELINE, ENCLOSURE_VARIANT};
use rcad_commands::{Command, Session};
use rcad_kernel::{VolumeMethod, VolumeResult};
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
    let out = dir.path().join("resaved.rcad.json");
    assert!(s.execute(&Command::Save { path: out.clone() }).ok);
    assert_eq!(version(&out), u64::from(rcad_document::FORMAT_VERSION));
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
        "crates/rcad-document/tests/fixtures/plate_v1.rcad.json",
        Some(1),
        PLATE,
    );
}

#[test]
fn v2_run2_documents() {
    let d = "crates/rcad-commands/tests/fixtures/native_v2";
    check(&format!("{d}/plate.rcad.json"), Some(2), PLATE);
    check(&format!("{d}/scenario_a.rcad.json"), Some(2), SCENARIO_A);
    check(&format!("{d}/scenario_b.rcad.json"), Some(2), scenario_b());
    check(&format!("{d}/scenario_r.rcad.json"), Some(2), SCENARIO_R);
    check(&format!("{d}/bracket.rcad.json"), Some(2), BRACKET);
}

#[test]
fn committed_examples() {
    check("examples/plate.rcad.json", None, PLATE);
    check("examples/scenario_a.rcad.json", None, SCENARIO_A);
    check("examples/scenario_b.rcad.json", None, scenario_b());
    check("examples/scenario_r.rcad.json", None, SCENARIO_R);
    check("examples/bracket.rcad.json", None, BRACKET);
}

/// Frozen copies of the run-3 (v3) documents: they load into v4 unchanged.
#[test]
fn v3_run3_examples() {
    let d = "crates/rcad-commands/tests/fixtures/native_v3";
    for (name, v) in [
        ("plate", PLATE),
        ("scenario_a", SCENARIO_A),
        ("scenario_b", scenario_b()),
        ("scenario_r", SCENARIO_R),
        ("bracket", BRACKET),
    ] {
        check(&format!("{d}/{name}.rcad.json"), Some(3), v);
    }
    // Imported FreeCAD box with a centred Ø6 through hole.
    check(
        &format!("{d}/imported_step_hole.rcad.json"),
        Some(3),
        12000.0 - 90.0 * PI,
    );
    // DXF notch plate (the STL body is a mesh: no B-rep measurements).
    check(
        &format!("{d}/dxf_stl_import.rcad.json"),
        Some(3),
        3.0 * (800.0 - 12.5 * PI),
    );
}

/// Run-4 enclosure base + lid (`Enclosure::{base,lid}_volume` restate the box formulas).
fn enclosure(p: &rcad_commands::printables::Enclosure) -> f64 {
    p.base_volume() + p.lid_volume()
}

/// Coupon plate 70 x 20 x t with four clearance holes, plus the Ø6 x h peg.
fn coupon() -> f64 {
    let (d, t, h, sp) = COUPON;
    let len = sp * COUPON_CLEARANCES.len() as f64 + 10.0;
    let holes: f64 = COUPON_CLEARANCES
        .iter()
        .map(|c| PI * ((d + 2.0 * c) / 2.0).powi(2) * t)
        .sum();
    len * 20.0 * t - holes + PI * (d / 2.0).powi(2) * h
}

/// Mounting bracket: 60 x 40 x 5 foot + 50 x 5 x 35 upright, 1 mm x 60 mm chamfer,
/// four Ø5.5 holes through 5 mm.
fn mounting_bracket() -> f64 {
    60.0 * 40.0 * 5.0 + 50.0 * 5.0 * 35.0 - 0.5 * 60.0 - 4.0 * PI * (5.5f64 / 2.0).powi(2) * 5.0
}

/// The bracket's holes run along Z and along Y, so the kernel reports a tessellation
/// estimate; it must hold within 3x its own error estimate (as in printable_examples.rs).
fn check_estimate(rel: &str, want_version: Option<u64>, expected: f64) {
    let path = repo().join(rel);
    if let Some(v) = want_version {
        assert_eq!(version(&path), v, "{rel}");
    }
    let mut s = Session::new();
    let r = s.execute(&Command::Open { path });
    assert!(r.ok, "{rel}: {:?}", r.error);
    let bodies = s.summary().bodies;
    assert_eq!(bodies.len(), 1, "{rel}");
    match bodies[0].measurements.as_ref().unwrap().volume {
        VolumeResult::Value {
            volume,
            method: VolumeMethod::TessellationEstimate,
            error_estimate,
        } => assert!(
            (volume - expected).abs() <= (3.0 * error_estimate).max(1e-3),
            "{rel}: {volume} vs {expected} (estimate error {error_estimate})"
        ),
        ref v => panic!("{rel}: {v:?}"),
    }
}

/// Frozen run-4 (v4) documents: parameters and expressions load, regenerate to the
/// analytic volume and re-save identically.
#[test]
fn v4_run4_printable_documents() {
    let d = "crates/rcad-commands/tests/fixtures/native_v4";
    check(
        &format!("{d}/enclosure.rcad.json"),
        Some(4),
        enclosure(&ENCLOSURE_BASELINE),
    );
    check(
        &format!("{d}/clearance_coupon.rcad.json"),
        Some(4),
        coupon(),
    );
    check_estimate(
        &format!("{d}/mounting_bracket.rcad.json"),
        Some(4),
        mounting_bracket(),
    );
}

/// The expressions in a frozen v4 file are live after loading: driving its user
/// parameters to the variant regenerates to the variant's analytic volume.
#[test]
fn v4_parameters_drive_a_loaded_document() {
    let path = repo().join("crates/rcad-commands/tests/fixtures/native_v4/enclosure.rcad.json");
    let mut s = Session::new();
    assert!(s.execute(&Command::Open { path }).ok);
    for c in ENCLOSURE_VARIANT.set_parameters() {
        let r = s.execute(&c);
        assert!(r.ok, "{c:?}: {:?}", r.error);
    }
    let (v, want) = (total_volume(&s), enclosure(&ENCLOSURE_VARIANT));
    assert!((v - want).abs() <= want * 1e-7, "{v} vs {want}");
}

#[test]
fn committed_printable_examples() {
    check(
        "examples/enclosure.rcad.json",
        None,
        enclosure(&ENCLOSURE_BASELINE),
    );
    check("examples/clearance_coupon.rcad.json", None, coupon());
    check_estimate(
        "examples/mounting_bracket.rcad.json",
        None,
        mounting_bracket(),
    );
}
