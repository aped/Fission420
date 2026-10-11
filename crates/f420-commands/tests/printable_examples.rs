//! SESSION_4 §4/§6.A/§6.B: the printable example workflows through the public
//! command path, with analytic expected geometry for the baseline and the
//! parameter-driven variant.
use f420_commands::printables::{self, ENCLOSURE_BASELINE, ENCLOSURE_VARIANT, Enclosure};
use f420_commands::{Command, Session};
use f420_document::BodyId;
use f420_kernel::VolumeResult;

fn replay(steps: &[f420_commands::scenarios::Step]) -> Session {
    let mut s = Session::new();
    for (i, st) in steps.iter().enumerate() {
        let r = s.execute(&st.cmd);
        assert_eq!(r.ok, st.expect_ok, "step {i} {:?}: {:?}", st.cmd, r.error);
    }
    s
}

fn vol(s: &Session, b: u32) -> f64 {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|x| x.id == BodyId(b))
        .unwrap();
    match b.measurements.unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn check_enclosure(s: &Session, p: &Enclosure) {
    let (vb, vl) = (vol(s, 1), vol(s, 2));
    assert!(
        (vb - p.base_volume()).abs() < 1e-6,
        "base {vb} vs {}",
        p.base_volume()
    );
    assert!(
        (vl - p.lid_volume()).abs() < 1e-6,
        "lid {vl} vs {}",
        p.lid_volume()
    );
}

#[test]
fn enclosure_baseline_and_variant() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = replay(&printables::enclosure_workflow(
        &dir.path().join("e.f420.json"),
    ));
    assert!((ENCLOSURE_BASELINE.base_volume() - 19592.0).abs() < 1e-9);
    assert!((ENCLOSURE_BASELINE.lid_volume() - 9404.0).abs() < 1e-9);
    check_enclosure(&s, &ENCLOSURE_BASELINE);
    for c in ENCLOSURE_VARIANT.set_parameters() {
        let r = s.execute(&c);
        assert!(r.ok, "{c:?}: {:?}", r.error);
    }
    check_enclosure(&s, &ENCLOSURE_VARIANT);
    let _ = Command::Inspect;
}

#[test]
fn coupon_and_bracket_build() {
    use std::f64::consts::PI;
    let dir = tempfile::tempdir().unwrap();
    let s = replay(&printables::coupon_workflow(
        &dir.path().join("c.f420.json"),
    ));
    let (d, t, h, _) = printables::COUPON;
    let holes: f64 = printables::COUPON_CLEARANCES
        .iter()
        .map(|c| PI * ((d + 2.0 * c) / 2.0).powi(2) * t)
        .sum();
    assert!(
        (vol(&s, 1) - (70.0 * 20.0 * t - holes)).abs() < 1e-3,
        "{}",
        vol(&s, 1)
    );
    assert!((vol(&s, 2) - PI * 9.0 * h).abs() < 1e-3, "{}", vol(&s, 2));
    let s = replay(&printables::bracket_workflow(
        &dir.path().join("b.f420.json"),
    ));
    let hole = PI * (5.5f64 / 2.0).powi(2);
    let want =
        60.0 * 40.0 * 5.0 + 50.0 * 5.0 * 35.0 - 2.0 * hole * 5.0 - 2.0 * hole * 5.0 - 0.5 * 60.0;
    // Holes along Z and along Y: no axis-ruled exact boundary integral applies,
    // so the kernel reports a tessellation estimate with its error estimate
    // (FreeCAD checks the exported STEP exactly: tools/print/check_examples.sh).
    let m = s.summary().bodies[0].measurements.clone().unwrap();
    match m.volume {
        VolumeResult::Value {
            volume,
            method: f420_kernel::VolumeMethod::TessellationEstimate,
            error_estimate,
        } => assert!(
            (volume - want).abs() <= (3.0 * error_estimate).max(1e-3),
            "{volume} vs {want} (estimate error {error_estimate})"
        ),
        v => panic!("{v:?}"),
    }
}

fn repo() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The committed workflow equals the builder's output (regenerate with
/// `F420_WRITE_EXAMPLES=1 cargo test -p f420-commands --test printable_examples`).
fn sync(name: &str, steps: &[f420_commands::scenarios::Step]) {
    let json = f420_commands::scenarios::workflow_json(steps).unwrap();
    let path = repo().join("examples").join(name);
    let text = serde_json::to_string_pretty(&json).unwrap() + "\n";
    if std::env::var_os("F420_WRITE_EXAMPLES").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    let committed: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap();
    assert_eq!(
        committed, json,
        "{name} is stale; regenerate with F420_WRITE_EXAMPLES=1"
    );
}

#[test]
fn committed_printable_workflows_are_current() {
    use std::path::Path;
    sync(
        "enclosure_workflow.json",
        &printables::enclosure_workflow(Path::new("examples/enclosure.f420.json")),
    );
    sync(
        "clearance_coupon_workflow.json",
        &printables::coupon_workflow(Path::new("examples/clearance_coupon.f420.json")),
    );
    sync(
        "mounting_bracket_workflow.json",
        &printables::bracket_workflow(Path::new("examples/mounting_bracket.f420.json")),
    );
}
