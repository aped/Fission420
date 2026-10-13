//! Feature suppression (SESSION_5 §9 item 1): a suppressed feature stays in
//! history and is not evaluated; suppressing a consumed feature is refused
//! (document unchanged) unless its dependents are suppressed with it; restore
//! needs the dependencies restored; the result after restore is identical
//! (deterministic regeneration); undo/redo and save/reopen keep the state.
mod support;
use rcad_commands::Session;
use rcad_document::{DocError, FeatureId};
use serde_json::json;
use support::tray::{self, Tray};

fn suppress(s: &mut Session, f: u32, on: bool, cascade: bool) -> rcad_commands::Response {
    tray::exec(
        s,
        &json!({"cmd": "set_suppressed", "feature": f, "suppressed": on,
                "with_dependents": cascade}),
    )
}

fn suppressed(s: &Session) -> Vec<u32> {
    s.summary()
        .features
        .iter()
        .filter(|f| f.suppressed)
        .map(|f| f.id.0)
        .collect()
}

#[test]
fn suppress_restore_with_dependency_rules() {
    let t = Tray::BASELINE;
    let mut s = tray::build(&t, true);
    let full = tray::volume(&s);
    assert!((full - t.notched_volume()).abs() < 1e-6);
    let before = s.model().unwrap().document().clone();

    // A leaf (the notch cut): plain tray.
    assert!(suppress(&mut s, tray::NOTCH, true, false).ok);
    assert!((tray::volume(&s) - t.plain_volume()).abs() < 1e-6);
    assert_eq!(suppressed(&s), vec![tray::NOTCH]);
    assert!(suppress(&mut s, tray::NOTCH, false, false).ok);
    assert_eq!(tray::volume(&s), full, "restore regenerates the same solid");

    // A consumed feature (the compartment the pattern copies): refused alone.
    let r = suppress(&mut s, tray::COMPARTMENT, true, false);
    assert!(!r.ok);
    match r.error {
        Some(DocError::BrokenReference { from, detail }) => {
            assert_eq!(from, FeatureId(tray::COMPARTMENTS));
            assert!(detail.contains("suppressed"), "{detail}");
        }
        e => panic!("{e:?}"),
    }
    assert_eq!(s.model().unwrap().document(), &before);

    // With dependents: compartment + pattern (+ nothing else); notch stays.
    assert!(suppress(&mut s, tray::COMPARTMENT, true, true).ok);
    assert_eq!(suppressed(&s), vec![tray::COMPARTMENT, tray::COMPARTMENTS]);
    let blank_with_notch = t.l * t.w * t.h - t.notch_w * t.notch_d * t.wall;
    assert!((tray::volume(&s) - blank_with_notch).abs() < 1e-6);
    // Restoring the pattern alone: its seed is still suppressed -> refused.
    let r = suppress(&mut s, tray::COMPARTMENTS, false, false);
    assert!(!r.ok, "{:?}", r.message);
    // Undo/redo of the cascaded change is one step.
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert_eq!(tray::volume(&s), full);
    assert!(suppressed(&s).is_empty());
    tray::ok(&mut s, &json!({"cmd": "redo"}));
    assert_eq!(suppressed(&s), vec![tray::COMPARTMENT, tray::COMPARTMENTS]);

    // Save/reopen keeps the suppressed state; restore with dependents.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tray.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let mut s2 = Session::new();
    tray::ok(&mut s2, &json!({"cmd": "open", "path": path}));
    assert_eq!(suppressed(&s2), vec![tray::COMPARTMENT, tray::COMPARTMENTS]);
    assert!(suppress(&mut s2, tray::COMPARTMENT, false, true).ok);
    assert_eq!(tray::volume(&s2), full);
    tray::assert_occupancy(&s2, &t, true);

    // The blank with dependents: everything suppressed, no body evaluated.
    assert!(suppress(&mut s2, tray::BLANK, true, true).ok);
    assert!(s2.model().unwrap().evaluation().bodies.is_empty());
    assert!(suppress(&mut s2, tray::BLANK, false, true).ok);
    assert_eq!(tray::volume(&s2), full);
}

/// Format: run-5 features make the file v5 (older builds refuse it instead of
/// evaluating suppressed features); without them it stays v4, byte-identical
/// to a run-4 save. A v4-labelled file containing a v5 feature is refused.
#[test]
fn v5_only_when_needed() {
    let t = Tray::BASELINE;
    let mut s = tray::build(&t, false);
    let dir = tempfile::tempdir().unwrap();
    let (p4, p5) = (
        dir.path().join("a.rcad.json"),
        dir.path().join("b.rcad.json"),
    );
    tray::ok(&mut s, &json!({"cmd": "save", "path": p4}));
    let version = |p: &std::path::Path| -> serde_json::Value {
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(p).unwrap()).unwrap()
            ["version"]
            .clone()
    };
    assert_eq!(version(&p4), json!(4));
    assert!(suppress(&mut s, tray::COMPARTMENTS, true, false).ok);
    tray::ok(&mut s, &json!({"cmd": "save", "path": p5}));
    assert_eq!(version(&p5), json!(5));
    // Relabelled as v4: refused, naming the feature.
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p5).unwrap()).unwrap();
    v["version"] = json!(4);
    std::fs::write(&p5, serde_json::to_vec(&v).unwrap()).unwrap();
    let r = tray::exec(&mut Session::new(), &json!({"cmd": "open", "path": p5}));
    assert!(!r.ok);
    assert!(
        format!("{:?}", r.error).contains("suppressed"),
        "{:?}",
        r.error
    );
}

/// Frozen v4 files (examples at 8f2b797, before any run-5 schema change)
/// still load, regenerate to their saved volume, and re-save as v4.
#[test]
fn frozen_v4_fixtures_load_unchanged() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/native_v4");
    let mut n = 0;
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(raw["version"], json!(4), "{p:?}");
        let mut s = Session::new();
        tray::ok(&mut s, &json!({"cmd": "open", "path": p}));
        let want = raw["cache"]["volume"].as_f64().expect("cached volume");
        let got: f64 = s
            .summary()
            .bodies
            .iter()
            .filter_map(|b| match b.measurements.as_ref()?.volume {
                rcad_kernel::VolumeResult::Value { volume, .. } => Some(volume),
                _ => None,
            })
            .sum();
        // The cache records the last body's (or the only body's) volume.
        let last = s
            .summary()
            .bodies
            .last()
            .and_then(|b| match b.measurements.as_ref()?.volume {
                rcad_kernel::VolumeResult::Value { volume, .. } => Some(volume),
                _ => None,
            })
            .unwrap_or(got);
        // The bracket's cached volume was a tessellation estimate (holes along two
        // axes; 20245.18 vs analytic 20244.834); its volume is now exact, so the
        // frozen cache is only good to the estimate's accuracy.
        let rel = if p.ends_with("mounting_bracket.rcad.json") {
            1e-4
        } else {
            1e-6
        };
        assert!(
            (last - want).abs() <= rel * want.abs().max(1.0)
                || (got - want).abs() <= rel * want.abs().max(1.0),
            "{p:?}: {last} / {got} vs cached {want}"
        );
        let tmp = tempfile::tempdir().unwrap();
        let o = tmp.path().join("re.rcad.json");
        tray::ok(&mut s, &json!({"cmd": "save", "path": o}));
        let re: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&o).unwrap()).unwrap();
        assert_eq!(re["version"], json!(4));
        assert_eq!(re["document"], raw["document"], "{p:?}: document unchanged");
        n += 1;
    }
    assert_eq!(n, 5);
}

/// Review run 5 #8: a pattern of count 1 (divider_count 0) is a run-5 range:
/// saved as v5 so a run-4 build says "newer version" instead of a misleading
/// count error.
#[test]
fn count_one_pattern_is_saved_as_v5() {
    let t = Tray {
        n: 0,
        ..Tray::BASELINE
    };
    let mut s = tray::build(&t, false);
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("t.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": p}));
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(v["version"], json!(5));
}
