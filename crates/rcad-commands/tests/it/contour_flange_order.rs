//! A contour flange does not depend on how its sketch lines were numbered
//! or drawn: the chain starts at its geometric start (the end with the
//! smaller sketch x). Drawing the U channel's lines in reverse order gives
//! the same folded part (closed-form volume, material probes). A flange made
//! before this rule keeps its entity-order start until it is edited.
use crate::support;
use rcad_commands::{Command, Session, SketchTemplate};
use rcad_document::{FeatureId, FeatureKind, SketchPlane};
use rcad_sketch::Sketch;
use serde_json::json;
use std::f64::consts::PI;
use support::tray;

/// U channel inner surface (0,22)-(0,0)-(64,0)-(64,22), lines in the given
/// point order.
fn u_sketch(reversed: bool) -> Sketch {
    let mut pts = [[0.0, 22.0], [0.0, 0.0], [64.0, 0.0], [64.0, 22.0]];
    if reversed {
        pts.reverse();
    }
    let mut s = Sketch::default();
    let p: Vec<_> = pts.iter().map(|q| s.add_point(q[0], q[1])).collect();
    for i in 0..3 {
        s.add_line(p[i], p[i + 1]);
    }
    s
}

fn flange(reversed: bool) -> Session {
    let mut s = Session::new();
    tray::ok(&mut s, &json!({"cmd": "new_document"}));
    let r = s.execute(&Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::Custom {
            sketch: u_sketch(reversed),
        },
    });
    assert!(r.ok, "{:?}", r.error);
    tray::ok(
        &mut s,
        &json!({"cmd": "sheet_metal", "sketch": 1, "thickness": 1.0, "radius": 2.0,
                "k_factor": 0.4, "width": 40.0}),
    );
    s
}

/// Material outside the U (below the base, left of the left leg).
const PROBES: [[f64; 3]; 3] = [[32.0, -0.5, 20.0], [32.0, 0.5, 20.0], [-0.5, 15.0, 20.0]];
const OUTSIDE: [Option<bool>; 3] = [Some(true), Some(false), Some(true)];

#[test]
fn redrawing_the_lines_in_reverse_keeps_the_flange() {
    // 100 mm of straights, two 90° bends at R 2, t 1, w 40.
    let want = 40.0 * 100.0 + 40.0 * 2.0 * (PI / 4.0) * (9.0 - 4.0);
    for reversed in [false, true] {
        let s = flange(reversed);
        assert!(
            (tray::volume(&s) - want).abs() < 1e-6,
            "{reversed}: {}",
            tray::volume(&s)
        );
        assert_eq!(tray::probe(&s, &PROBES), OUTSIDE, "reversed: {reversed}");
    }
}

#[test]
fn a_flange_made_before_the_rule_keeps_its_side_until_edited() {
    let mut s = flange(true);
    let mut k = s
        .model()
        .unwrap()
        .document()
        .feature(FeatureId(2))
        .unwrap()
        .kind
        .clone();
    let FeatureKind::SheetMetal {
        geometric_order, ..
    } = &mut k
    else {
        panic!()
    };
    assert!(*geometric_order);
    // The entity-order rule (documents before format v9) starts at
    // (64, 22): the material is inside the U.
    *geometric_order = false;
    let r = s.execute(&Command::RedefineFeature {
        feature: FeatureId(2),
        kind: Box::new(k),
        name: None,
        expressions: None,
    });
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(
        tray::probe(&s, &PROBES),
        [Some(false), Some(true), Some(false)]
    );
}
