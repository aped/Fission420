//! Piece identity of a cut that separates a body (ADR 0003): each body id
//! keeps the piece that contains its stored seed point across upstream
//! edits, including an edit that reorders the pieces' bounding boxes (the
//! old positional rule would have swapped the ids). Seeds persist (format
//! v9); seeds no piece contains are a broken reference, never a guess.
use crate::support;
use rcad_commands::Session;
use rcad_document::{BodyId, FeatureId, HealthState, Model};
use serde_json::json;
use support::tray;

fn bounds(s: &Session, id: u32) -> ([f64; 3], [f64; 3]) {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id.0 == id)
        .unwrap_or_else(|| panic!("body {id}"))
        .bounds
        .unwrap();
    (b.min, b.max)
}

fn vol(s: &Session, id: u32) -> f64 {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id.0 == id)
        .unwrap();
    match b.measurements.expect("measured").volume {
        rcad_kernel::VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

/// Plate 40 x 30 x 10 (x 0..40, y 0..30, z 0..10) joined with a tab
/// 12 x 10 x 10 (x -10..2, z -5..5) centred on y = `tab_y` (feature 3 is its
/// sketch), then a slot y 14..16 cut through everything (feature 6): two
/// pieces, below and above the slot.
fn tabbed_plate(s: &mut Session, tab_y: f64) {
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 40.0, "height": 30.0, "center": [20.0, 15.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 12.0, "height": 10.0, "center": [-4.0, tab_y]}),
        json!({"cmd": "extrude", "sketch": 3, "distance": 10.0, "op": "join", "target": 1,
               "options": {"symmetric": true}}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 60.0, "height": 2.0, "center": [20.0, 15.0]}),
        json!({"cmd": "extrude", "sketch": 5, "op": "cut", "target": 1, "through_all": true}),
    ] {
        tray::ok(s, &v);
    }
}

// Top or bottom piece, each 40 x 14 x 10 = 5600, plus 1100 with the tab
// (1200 less the 2 x 10 x 5 overlap with the plate).
const PLAIN: f64 = 5600.0;
const TABBED: f64 = 6700.0;

#[test]
fn ids_follow_their_pieces_when_the_bounding_box_order_flips() {
    let mut s = Session::new();
    tabbed_plate(&mut s, 25.0);
    // Fresh cut: positional order, lowest corner first: the top piece
    // (with the tab, x from -10) stays Body1, the bottom one is Body2.
    let (lo1, hi1) = bounds(&s, 1);
    assert!((lo1[1] - 16.0).abs() < 1e-9 && (hi1[1] - 30.0).abs() < 1e-9);
    assert!((lo1[0] + 10.0).abs() < 1e-9);
    let (lo2, hi2) = bounds(&s, 2);
    assert!(lo2[1].abs() < 1e-9 && (hi2[1] - 14.0).abs() < 1e-9);
    assert!((vol(&s, 1) - TABBED).abs() < 1e-6, "{}", vol(&s, 1));
    assert!((vol(&s, 2) - PLAIN).abs() < 1e-6, "{}", vol(&s, 2));
    let seeds = s
        .model()
        .unwrap()
        .document()
        .feature(FeatureId(6))
        .unwrap()
        .piece_seeds
        .clone();
    assert_eq!(
        seeds.keys().copied().collect::<Vec<_>>(),
        [BodyId(1), BodyId(2)]
    );

    // Move the tab below the slot: the bottom piece now has the lowest
    // corner. Body1 is still the top piece, Body2 the bottom one.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "center_y", "value": 5.0}),
    );
    let (lo1, hi1) = bounds(&s, 1);
    assert!(
        (lo1[1] - 16.0).abs() < 1e-9 && (hi1[1] - 30.0).abs() < 1e-9,
        "{lo1:?} {hi1:?}"
    );
    assert!(lo1[0].abs() < 1e-9, "the tab left the top piece: {lo1:?}");
    let (lo2, hi2) = bounds(&s, 2);
    assert!(
        lo2[1].abs() < 1e-9 && (hi2[1] - 14.0).abs() < 1e-9,
        "{lo2:?} {hi2:?}"
    );
    assert!((lo2[0] + 10.0).abs() < 1e-9);
    assert!((vol(&s, 1) - PLAIN).abs() < 1e-6, "{}", vol(&s, 1));
    assert!((vol(&s, 2) - TABBED).abs() < 1e-6, "{}", vol(&s, 2));

    // Undo and redo restore the same identities.
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert!((bounds(&s, 1).0[0] + 10.0).abs() < 1e-9);
    tray::ok(&mut s, &json!({"cmd": "redo"}));
    assert!(bounds(&s, 1).0[0].abs() < 1e-9);
}

#[test]
fn seeds_persist_and_a_seed_in_no_piece_is_a_broken_reference() {
    let mut s = Session::new();
    tabbed_plate(&mut s, 25.0);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tabbed.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw["version"], 9);

    // Reopen: same identities.
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": path}));
    assert!((bounds(&t, 1).0[0] + 10.0).abs() < 1e-9);
    assert!(!t.model().unwrap().dirty);

    // A seed that no piece contains: the cut fails with a broken reference
    // (the document still opens, with the cut flagged), nothing is guessed.
    let mut nf = rcad_document::load_file(&path).unwrap();
    let f = nf
        .document
        .features
        .iter_mut()
        .find(|f| f.id == FeatureId(6))
        .unwrap();
    // (Both seeds: with one left over, the pairing would be forced.)
    f.piece_seeds.insert(BodyId(1), vec![[500.0, 500.0, 500.0]]);
    f.piece_seeds
        .insert(BodyId(2), vec![[-500.0, 500.0, 500.0]]);
    let (m, report) = Model::open_document(nf.document, false).unwrap();
    let r = &m
        .evaluation()
        .features
        .iter()
        .find(|r| r.id == FeatureId(6))
        .unwrap();
    assert_eq!(r.health.state, HealthState::Error);
    let msg = r.health.message.clone().unwrap_or_default();
    assert!(msg.contains("can no longer be identified"), "{msg}");
    assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
}

#[test]
fn a_document_without_seeds_gets_them_on_open_without_becoming_unsaved() {
    let mut s = Session::new();
    tabbed_plate(&mut s, 25.0);
    let mut doc = s.model().unwrap().document().clone();
    for f in &mut doc.features {
        f.piece_seeds.clear();
    }
    assert_eq!(rcad_document::required_version(&doc), 8);
    let (m, _) = Model::open_document(doc, false).unwrap();
    assert!(!m.dirty);
    assert_eq!(
        m.document()
            .feature(FeatureId(6))
            .unwrap()
            .piece_seeds
            .len(),
        2
    );
}

#[test]
fn moving_the_cut_far_keeps_each_id_on_its_side() {
    // Block x 0..40 with a 4 mm slot across it at x 18..22: Body1 left,
    // Body2 right. Move the slot to x 32..36, then to x 4..8: each time the
    // deep point of one piece ends up in the other, but its point far from
    // the cut does not.
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 40.0, "height": 30.0, "center": [20.0, 15.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 4.0, "height": 32.0, "center": [20.0, 15.0]}),
        json!({"cmd": "extrude", "sketch": 3, "op": "cut", "target": 1, "through_all": true}),
    ] {
        tray::ok(&mut s, &v);
    }
    let x_range = |s: &Session, id| {
        let (lo, hi) = bounds(s, id);
        (lo[0], hi[0])
    };
    let near = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9;
    assert!(near(x_range(&s, 1), (0.0, 18.0)));
    assert!(near(x_range(&s, 2), (22.0, 40.0)));
    for (cx, left, right) in [
        (34.0, (0.0, 32.0), (36.0, 40.0)),
        (6.0, (0.0, 4.0), (8.0, 40.0)),
    ] {
        tray::ok(
            &mut s,
            &json!({"cmd": "set_parameter", "feature": 3, "name": "center_x", "value": cx}),
        );
        assert!(near(x_range(&s, 1), left), "{cx}: {:?}", x_range(&s, 1));
        assert!(near(x_range(&s, 2), right), "{cx}: {:?}", x_range(&s, 2));
        assert!((vol(&s, 1) - (left.1 - left.0) * 300.0).abs() < 1e-6);
    }
}

#[test]
fn a_join_on_piece_two_stays_on_its_physical_piece() {
    // A Ø4 x 5 boss joined to Body2 (the bottom piece) from World XY, which
    // holds that piece's bottom face, so it grows downward (ADR 0003). The
    // tab then moves below the slot, flipping the bounding-box order: the
    // boss stays on the bottom piece. (Under the positional rule Body2 would
    // become the top piece and the boss would join it far from its body.)
    let mut s = Session::new();
    tabbed_plate(&mut s, 25.0);
    for v in [
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_circle",
               "diameter": 4.0, "center": [20.0, 5.0]}),
        json!({"cmd": "extrude", "sketch": 7, "distance": 5.0, "op": "join", "target": 2}),
    ] {
        tray::ok(&mut s, &v);
    }
    let boss = std::f64::consts::PI * 4.0 * 5.0;
    assert!((vol(&s, 2) - (PLAIN + boss)).abs() < 1e-6, "{}", vol(&s, 2));
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "center_y", "value": 5.0}),
    );
    let (lo2, hi2) = bounds(&s, 2);
    assert!(
        lo2[1].abs() < 1e-9 && (hi2[1] - 14.0).abs() < 1e-9,
        "{lo2:?} {hi2:?}"
    );
    assert!(
        (vol(&s, 2) - (TABBED + boss)).abs() < 1e-6,
        "{}",
        vol(&s, 2)
    );
    assert!((vol(&s, 1) - PLAIN).abs() < 1e-6, "{}", vol(&s, 1));
}
