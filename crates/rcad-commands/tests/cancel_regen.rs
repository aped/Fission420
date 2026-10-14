//! Cooperative cancellation of a regeneration: with cancellation requested,
//! an edit stops before its first re-evaluated feature with
//! `DocError::Cancelled`, nothing is committed (document, revision and undo
//! stack unchanged); after clearing the request the same edit applies. Own
//! test binary: the cancellation flag is process-wide.
use rcad_commands::{Command, Session, SketchTemplate};
use rcad_document::{DocError, FeatureId, SketchPlane};

#[test]
fn a_cancelled_edit_changes_nothing() {
    let mut s = Session::new();
    assert!(s.execute(&Command::NewDocument).ok);
    let sk = s
        .execute(&Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 40.0,
                height: 30.0,
                center: None,
            },
        })
        .feature
        .unwrap();
    let e = s
        .execute(&Command::extrude(None, sk, 10.0))
        .feature
        .unwrap();
    let before = s.model().unwrap().document().clone();
    let rev = s.summary().revision;
    rcad_document::request_cancel();
    let r = s.execute(&Command::SetParameter {
        feature: e,
        name: "distance".into(),
        value: 12.0,
    });
    rcad_document::clear_cancel();
    assert!(
        matches!(r.error, Some(DocError::Cancelled(FeatureId(_)))),
        "{:?}",
        r.error
    );
    assert_eq!(s.model().unwrap().document(), &before);
    assert_eq!(s.summary().revision, rev);
    let r = s.execute(&Command::SetParameter {
        feature: e,
        name: "distance".into(),
        value: 12.0,
    });
    assert!(r.ok, "{:?}", r.error);
    // One undo step: back to 10 mm (the cancelled edit left no entry).
    assert!(s.execute(&Command::Undo).ok);
    assert_eq!(s.model().unwrap().document(), &before);
}
