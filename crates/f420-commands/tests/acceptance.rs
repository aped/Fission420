//! The full GOAL.md workflow as a test (same code path as `f420-cli acceptance`).
#[test]
fn acceptance_workflow_passes() {
    let dir = tempfile::tempdir().unwrap();
    let report = f420_commands::acceptance::run(dir.path());
    for c in report.checks.iter().filter(|c| !c.pass) {
        eprintln!(
            "FAIL [{}] {}: expected {} | actual {}",
            c.step, c.name, c.expected, c.actual
        );
    }
    assert!(report.ok(), "{} checks failed", report.failed);
}

#[test]
fn commands_round_trip_as_json() {
    use f420_commands::Command;
    let text = r#"[{"cmd":"new_document"},
      {"cmd":"create_sketch","plane":"WorldXY","template":"centered_rectangle","width":80,"height":50},
      {"cmd":"extrude","sketch":1,"distance":10},
      {"cmd":"create_sketch","plane":{"Face":{"feature":2,"role":"ExtrudeEndCap"}},"template":"centered_circle","diameter":10},
      {"cmd":"cut_through_all","sketch":3},
      {"cmd":"set_parameter","feature":1,"name":"width","value":100}]"#;
    let cmds: Vec<Command> = serde_json::from_str(text).unwrap();
    let back: Vec<Command> = serde_json::from_str(&serde_json::to_string(&cmds).unwrap()).unwrap();
    assert_eq!(cmds, back);
}

#[test]
fn commands_without_document_fail_cleanly() {
    use f420_commands::{Command, Session};
    let mut s = Session::new();
    for c in [
        Command::Undo,
        Command::Measure,
        Command::Close,
        Command::Extrude {
            name: None,
            sketch: f420_document::FeatureId(1),
            distance: 1.0,
        },
    ] {
        let r = s.execute(&c);
        assert!(!r.ok && r.error.is_some(), "{c:?}");
    }
    let r = s.execute(&Command::Open {
        path: "/nonexistent/x.f420.json".into(),
    });
    assert!(!r.ok);
}
