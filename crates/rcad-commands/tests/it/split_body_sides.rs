//! Split Body piece identity per side (ADR 0003): when an upstream edit
//! changes how many pieces fall behind the plane, the ids follow their
//! pieces instead of shifting along a combined positional list. Oracle:
//! which side of the plane each body lies on (its bounds) and closed-form
//! volumes.
use crate::support;
use rcad_commands::Session;
use serde_json::json;
use support::tray;

fn body(s: &Session, id: u32) -> Option<([f64; 3], [f64; 3], f64)> {
    let b = s.summary().bodies.into_iter().find(|b| b.id.0 == id)?;
    let bb = b.bounds.unwrap();
    let v = match b.measurements.expect("measured").volume {
        rcad_kernel::VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    };
    Some((bb.min, bb.max, v))
}

/// An H standing on World XY: bars x 0..5 and 35..40 (y -10..0, z 0..40)
/// joined by a crossbar z 10..15, split by the plane z = `h` (feature 3).
fn split_h(s: &mut Session, h: f64) {
    let outline = [
        [0.0, 0.0],
        [5.0, 0.0],
        [5.0, 10.0],
        [35.0, 10.0],
        [35.0, 0.0],
        [40.0, 0.0],
        [40.0, 40.0],
        [35.0, 40.0],
        [35.0, 15.0],
        [5.0, 15.0],
        [5.0, 40.0],
        [0.0, 40.0],
    ];
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXZ", "template": "polygon",
               "vertices": outline}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
        json!({"cmd": "split_body", "body": 1,
               "plane": {"Offset": {"base": "WorldXY", "distance": h}}}),
    ] {
        tray::ok(s, &v);
    }
}

#[test]
fn a_side_whose_pieces_rejoin_keeps_the_other_side_ids() {
    let mut s = Session::new();
    split_h(&mut s, 5.0);
    // Behind (below z = 5): the two feet; Body1 the left one, Body3 the
    // right one. In front: the rest of the H, Body2.
    let (lo, hi, v) = body(&s, 1).unwrap();
    assert!(lo[0].abs() < 1e-9 && (hi[0] - 5.0).abs() < 1e-9 && (hi[2] - 5.0).abs() < 1e-9);
    assert!((v - 250.0).abs() < 1e-6, "{v}");
    let (lo, _, v) = body(&s, 3).unwrap();
    assert!((lo[0] - 35.0).abs() < 1e-9, "{lo:?}");
    assert!((v - 250.0).abs() < 1e-6);
    let (lo, _, v) = body(&s, 2).unwrap();
    assert!((lo[2] - 5.0).abs() < 1e-9);
    assert!((v - (5500.0 - 500.0)).abs() < 1e-6, "{v}");

    // Raise the plane above the crossbar: behind is one U, in front two
    // bar tops. Body1 is the U (it holds the left foot); the right foot's
    // id Body3 rejoined Body1 and makes no body, never a bar top; Body2 is a
    // bar top and the other top gets a new id.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 3, "name": "plane_offset", "value": 25.0}),
    );
    let (lo, hi, v) = body(&s, 1).unwrap();
    assert!(
        lo[2].abs() < 1e-9 && (hi[2] - 25.0).abs() < 1e-9,
        "{lo:?} {hi:?}"
    );
    assert!((v - 4000.0).abs() < 1e-6, "{v}");
    assert!(
        body(&s, 3).is_none(),
        "the right foot's id moved to {:?}",
        body(&s, 3)
    );
    let tops: Vec<u32> = s
        .summary()
        .bodies
        .iter()
        .map(|b| b.id.0)
        .filter(|&id| id != 1)
        .collect();
    assert_eq!(tops.len(), 2, "{tops:?}");
    assert!(tops.contains(&2), "{tops:?}");
    for id in tops {
        let (lo, hi, v) = body(&s, id).unwrap();
        assert!((lo[2] - 25.0).abs() < 1e-9 && (hi[2] - 40.0).abs() < 1e-9);
        assert!((v - 750.0).abs() < 1e-6, "{v}");
    }
}
