//! Format v9 is required exactly by its additions: a plain sketch stays at
//! the old version, a driven (measuring) sketch dimension needs v9 (a v8
//! reader would take it as driving).
use rcad_document::{FeatureKind, Model, SketchPlane, required_version};
use rcad_sketch::Sketch;

#[test]
fn a_driven_dimension_needs_v9() {
    let mut m = Model::new();
    m.transact("sketch", |d| {
        d.push(
            "Sketch1",
            FeatureKind::Sketch {
                plane: SketchPlane::WorldXY,
                sketch: Sketch::centered_rectangle(40.0, 20.0).unwrap(),
                links: Vec::new(),
            },
        );
        Ok(())
    })
    .unwrap();
    let doc = m.document().clone();
    assert!(required_version(&doc) < 9);
    let text = serde_json::to_string(&doc).unwrap();
    assert!(text.contains("\"name\":\"width\""), "{text}");
    let driven = text.replacen(
        "\"name\":\"width\",\"value\":40.0",
        "\"name\":\"width\",\"value\":40.0,\"driven\":true",
        1,
    );
    assert_ne!(driven, text);
    let doc: rcad_document::Document = serde_json::from_str(&driven).unwrap();
    assert_eq!(required_version(&doc), 9);
}

#[test]
fn document_units_round_trip_and_need_v9() {
    let mut m = Model::new();
    assert!(required_version(m.document()) < 9);
    m.transact("units", |d| {
        d.units = rcad_document::DocumentUnits::In;
        Ok(())
    })
    .unwrap();
    let doc = m.document().clone();
    assert_eq!(required_version(&doc), 9);
    let text = serde_json::to_string(&doc).unwrap();
    assert!(text.contains("\"units\":\"in\""), "{text}");
    let back: rcad_document::Document = serde_json::from_str(&text).unwrap();
    assert_eq!(back, doc);
    assert_eq!(rcad_document::DocumentUnits::In.mm_per_unit(), 25.4);
}
