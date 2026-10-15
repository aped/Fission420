//! A document whose history no longer fully regenerates still opens: the
//! failing feature and its dependents are flagged in their health, unrelated
//! bodies are evaluated exactly, safe mode rolls back to before the first
//! failure, and save/reopen and undo/redo keep working.
//!
//! Fixture `fixtures/open_with_errors.rcad.json` (frozen): six features.
//! F1 sketch 40x20, F2 extrude 10 (Body1), F3 sketch on a face that does not
//! exist (a side of a line F1 does not have), F4 cut through all with F3,
//! F5 sketch 10x10 centred at (100, 0), F6 extrude 5 (Body2).
//! Regenerate it with `RCAD_WRITE_FIXTURE=1`.
use rcad_commands::{Command, Session, SketchTemplate, end_cap_of};
use rcad_document::{FeatureId, HealthState, SketchPlane};
use rcad_kernel::VolumeResult;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/open_with_errors.rcad.json")
}

fn rect(w: f64, h: f64, center: Option<[f64; 2]>) -> Command {
    Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: w,
            height: h,
            center,
        },
    }
}

/// Build the healthy document, then break F3's face reference in the saved
/// JSON (as an upstream change in another build could).
fn write_fixture(path: &std::path::Path) {
    let mut s = Session::new();
    for c in [
        Command::NewDocument,
        rect(40.0, 20.0, None),
        Command::extrude(None, FeatureId(1), 10.0),
        Command::CreateSketch {
            name: None,
            plane: end_cap_of(FeatureId(2)),
            template: SketchTemplate::CenteredCircle {
                diameter: 6.0,
                center: None,
            },
        },
        Command::CutThroughAll {
            name: None,
            sketch: FeatureId(3),
        },
        rect(10.0, 10.0, Some([100.0, 0.0])),
        Command::extrude(None, FeatureId(5), 5.0),
    ] {
        let r = s.execute(&c);
        assert!(r.ok, "{c:?}: {:?}", r.error);
    }
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().join("ok.rcad.json");
    assert!(s.execute(&Command::Save { path: tmp.clone() }).ok);
    let mut j: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&tmp).unwrap()).unwrap();
    let f3 = &mut j["document"]["features"][2];
    assert_eq!(f3["id"], 3);
    f3["kind"]["plane"] = serde_json::json!({
        "Face": { "feature": 2, "role": { "ExtrudeSide": { "entity": 99 } } }
    });
    std::fs::write(path, serde_json::to_string_pretty(&j).unwrap() + "\n").unwrap();
}

fn volume(s: &Session, body: u32) -> f64 {
    let ev = s.model().unwrap().evaluation();
    let b = ev
        .bodies
        .iter()
        .find(|b| b.id.0 == body)
        .unwrap_or_else(|| panic!("body {body} missing"));
    match b.measurements.as_ref().map(|m| &m.volume) {
        Some(VolumeResult::Value { volume, .. }) => *volume,
        other => panic!("body {body} volume {other:?}"),
    }
}

fn open(path: PathBuf, safe_mode: bool) -> (Session, rcad_commands::Response) {
    let mut s = Session::new();
    let r = s.execute(&Command::Open { path, safe_mode });
    (s, r)
}

#[test]
fn opens_with_the_failure_flagged() {
    if std::env::var_os("RCAD_WRITE_FIXTURE").is_some() {
        write_fixture(&fixture());
    }
    let (s, r) = open(fixture(), false);
    assert!(r.ok, "{:?}", r.error);
    let msg = r.message.unwrap();
    assert!(msg.contains("ERRORS: 2 feature(s)"), "{msg}");
    let sum = s.summary();
    let state = |id: u32| {
        sum.features
            .iter()
            .find(|f| f.id == FeatureId(id))
            .unwrap()
            .health
            .clone()
    };
    for id in [1, 2, 5, 6] {
        assert_eq!(state(id).state, HealthState::Healthy, "F{id}");
    }
    let f3 = state(3);
    assert_eq!(f3.state, HealthState::Error);
    assert!(f3.error.is_some() && f3.hint.is_some(), "{f3:?}");
    let f4 = state(4);
    assert_eq!(f4.state, HealthState::Error);
    assert_eq!(f4.upstream, Some(FeatureId(3)));
    // Unrelated body exact; Body1 is the state before the failed cut.
    assert!((volume(&s, 2) - 500.0).abs() < 1e-9, "{}", volume(&s, 2));
    assert!((volume(&s, 1) - 8000.0).abs() < 1e-9, "{}", volume(&s, 1));
}

#[test]
fn geometry_exports_are_refused_while_unhealthy() {
    let (mut s, _) = open(fixture(), false);
    let dir = tempfile::tempdir().unwrap();
    let r = s.execute(&Command::ExportStl {
        path: dir.path().join("x.stl"),
        chord: None,
        bodies: None,
        print: Default::default(),
    });
    assert!(!r.ok);
    let e = r.error.unwrap().to_string();
    assert!(
        e.contains("does not fully regenerate") && e.contains("F3"),
        "{e}"
    );
}

#[test]
fn edits_work_on_an_unhealthy_document_and_save_reopen_is_stable() {
    let (mut s, _) = open(fixture(), false);
    // An unrelated edit commits (the existing failures do not block it).
    let r = s.execute(&Command::SetParameter {
        feature: FeatureId(6),
        name: "distance".into(),
        value: 8.0,
    });
    assert!(r.ok, "{:?}", r.error);
    assert!((volume(&s, 2) - 800.0).abs() < 1e-9);
    // Undo and redo always succeed.
    assert!(s.execute(&Command::Undo).ok);
    assert!((volume(&s, 2) - 500.0).abs() < 1e-9);
    assert!(s.execute(&Command::Redo).ok);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("resaved.rcad.json");
    assert!(s.execute(&Command::Save { path: out.clone() }).ok);
    let (t, r) = open(out, false);
    assert!(r.ok);
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert_eq!(t.model().unwrap().evaluation().failures().count(), 2);
    assert!((volume(&t, 2) - 800.0).abs() < 1e-9);
}

#[test]
fn an_edit_that_breaks_a_healthy_feature_is_still_refused() {
    let (mut s, _) = open(fixture(), false);
    let rev = s.model().unwrap().revision;
    // Body2's extrude cannot be zero length: refused, nothing changes.
    let r = s.execute(&Command::SetParameter {
        feature: FeatureId(6),
        name: "distance".into(),
        value: 0.0,
    });
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().revision, rev);
}

#[test]
fn safe_mode_rolls_back_to_before_the_first_failure() {
    let (s, r) = open(fixture(), true);
    assert!(r.ok, "{:?}", r.error);
    let m = s.model().unwrap();
    assert!(m.evaluation().is_healthy());
    assert_eq!(m.document().rollback, Some(FeatureId(2)));
    assert!(m.dirty);
    assert!(r.message.unwrap().contains("SAFE MODE"));
    assert!((volume(&s, 1) - 8000.0).abs() < 1e-9);
}
