//! The committed replayable workflows `examples/scenario_{a,b,r}_workflow.json`
//! are generated from `rcad_commands::scenarios`. This test fails if they are
//! out of date; regenerate with
//! `RCAD_WRITE_EXAMPLES=1 cargo test -p rcad-commands --test it scenario_examples::`.
//! It also replays each workflow (into a temp dir) and checks every step's
//! outcome matches its `expect_ok`.
use rcad_commands::scenarios::{self, Step};
use rcad_commands::{Command, Session};
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sync(name: &str, steps: &[Step]) {
    let json = scenarios::workflow_json(steps).unwrap();
    let path = repo().join("examples").join(name);
    let text = serde_json::to_string_pretty(&json).unwrap() + "\n";
    if std::env::var_os("RCAD_WRITE_EXAMPLES").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    let committed: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap();
    assert_eq!(
        committed, json,
        "{name} is stale; regenerate with RCAD_WRITE_EXAMPLES=1"
    );
    // The committed file parses as the CLI's step list.
    let cmds: Vec<Command> = serde_json::from_value(committed).unwrap();
    assert_eq!(cmds.len(), steps.len());
}

fn replay(steps: &[Step]) -> Session {
    let mut s = Session::new();
    for (i, st) in steps.iter().enumerate() {
        let r = s.execute(&st.cmd);
        assert_eq!(
            r.ok,
            st.expect_ok,
            "step {i} {:?}: {:?}",
            st.cmd.name(),
            r.error
        );
    }
    s
}

#[test]
fn scenario_a_example_is_current_and_replays() {
    let doc = Path::new("examples/scenario_a.rcad.json");
    sync(
        "scenario_a_workflow.json",
        &scenarios::scenario_a_workflow(doc, Path::new("artifacts")).unwrap(),
    );
    let dir = tempfile::tempdir().unwrap();
    let steps =
        scenarios::scenario_a_workflow(&dir.path().join("a.rcad.json"), dir.path()).unwrap();
    let s = replay(&steps);
    let sum = s.summary();
    assert_eq!(sum.features.len(), 4);
    assert!(dir.path().join("scenario_a.step").exists());
}

#[test]
fn scenario_b_example_is_current_and_replays() {
    let doc = Path::new("examples/scenario_b.rcad.json");
    sync(
        "scenario_b_workflow.json",
        &scenarios::scenario_b_workflow(doc, Path::new("artifacts")).unwrap(),
    );
    let dir = tempfile::tempdir().unwrap();
    let steps =
        scenarios::scenario_b_workflow(&dir.path().join("b.rcad.json"), dir.path()).unwrap();
    let s = replay(&steps);
    assert_eq!(s.summary().features.len(), 2);
    assert!(dir.path().join("scenario_b.stl").exists());
}

#[test]
fn scenario_r_example_is_current_and_replays() {
    let doc = Path::new("examples/scenario_r.rcad.json");
    sync(
        "scenario_r_workflow.json",
        &scenarios::scenario_r_workflow(doc, Path::new("artifacts")).unwrap(),
    );
    let dir = tempfile::tempdir().unwrap();
    let steps =
        scenarios::scenario_r_workflow(&dir.path().join("r.rcad.json"), dir.path()).unwrap();
    let s = replay(&steps);
    // Last document: the two refused-profile sketches (kept, not extruded).
    let sum = s.summary();
    assert_eq!(sum.features.len(), 2);
    assert!(sum.features.iter().all(|f| f.profile_error.is_some()));
    for f in ["scenario_r.step", "scenario_r.stl", "scenario_r_slot.step"] {
        assert!(dir.path().join(f).exists(), "{f}");
    }
}
