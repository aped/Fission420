//! Sketches on faces made by patterns, mirrors and chamfers (SESSION_5 §5):
//! semantic references `FaceRole::Copy { instance, tool, of }` and
//! `FaceRole::Chamfer`; coplanar faces told apart by the generating tool's
//! footprint (never by index or nearest centroid); occurrence identity across
//! count changes; an occurrence that disappears is a BrokenReference with the
//! document unchanged; save/reopen and undo/redo. Expected values are analytic.
mod support;
use rcad_commands::Session;
use rcad_document::{DocError, FaceRef, FaceRole, FeatureId, SketchPlane};
use serde_json::json;
use support::tray::{self, Tray};

fn sketch_frame(s: &Session, id: u32) -> rcad_kernel::Frame {
    s.summary()
        .features
        .into_iter()
        .find(|f| f.id == FeatureId(id))
        .and_then(|f| f.frame)
        .expect("sketch frame")
}

fn floor_of(instance: u32) -> serde_json::Value {
    json!({"Face": {"feature": tray::COMPARTMENTS,
                    "role": {"Copy": {"instance": instance, "of": "EndCap"}}}})
}

#[test]
fn sketch_on_a_pattern_occurrence_floor_follows_the_occurrence() {
    let t = Tray::BASELINE;
    let mut s = tray::build(&t, false);
    assert!((tray::volume(&s) - t.plain_volume()).abs() < 1e-6);
    // The three floors are coplanar (z = floor): resolved by footprint.
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "create_sketch", "name": "Recess outline", "plane": floor_of(2),
                "template": "centered_rectangle", "width": 10.0, "height": 10.0,
                "center": [t.compartment() / 2.0, t.w / 2.0 - t.wall]}),
    );
    let sk = r.feature.unwrap().0;
    let f = sketch_frame(&s, sk);
    // Occurrence 3's floor frame: the seed floor frame moved by 2 pitches.
    let pitch = t.compartment() + t.dt;
    assert!((f.origin[0] - 2.0 * pitch).abs() < 1e-9, "{f:?}");
    assert!((f.origin[2] - t.floor).abs() < 1e-9 && (f.z[2] - 1.0).abs() < 1e-12);
    tray::ok(
        &mut s,
        &json!({"cmd": "extrude", "sketch": sk, "distance": 0.6, "op": "cut", "target": 1,
                "name": "Recess"}),
    );
    let recess = 10.0 * 10.0 * 0.6;
    assert!((tray::volume(&s) - (t.plain_volume() - recess)).abs() < 1e-6);
    // Recess in compartment 3's floor, not in 1 or 2.
    let c = |i| [t.compartment_center(i), t.w / 2.0, t.floor - 0.3];
    assert_eq!(
        tray::probe(&s, &[c(0), c(1), c(2)]),
        vec![Some(true), Some(true), Some(false)]
    );

    // More dividers: occurrence 3 survives and the recess moves with it.
    let t4 = Tray { n: 3, ..t };
    tray::ok(&mut s, &serde_json::to_value(t4.set_all()).unwrap());
    assert!((tray::volume(&s) - (t4.plain_volume() - recess)).abs() < 1e-6);
    let c4 = |i| [t4.compartment_center(i), t4.w / 2.0, t4.floor - 0.3];
    assert_eq!(
        tray::probe(&s, &[c4(1), c4(2), c4(3)]),
        vec![Some(true), Some(false), Some(true)]
    );

    // Fewer: occurrence 3 is gone -> BrokenReference, document unchanged.
    let before = s.model().unwrap().document().clone();
    let t1 = Tray { n: 1, ..t };
    let r = tray::exec(&mut s, &serde_json::to_value(t1.set_all()).unwrap());
    assert!(!r.ok);
    match r.error.as_ref() {
        Some(DocError::BrokenReference { detail, .. }) => {
            assert!(detail.contains("occurrence 3"), "{detail}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(s.model().unwrap().document(), &before);
    assert!((tray::volume(&s) - (t4.plain_volume() - recess)).abs() < 1e-6);

    // Undo/redo of the parameter change, then save/reopen.
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert!((tray::volume(&s) - (t.plain_volume() - recess)).abs() < 1e-6);
    tray::ok(&mut s, &json!({"cmd": "redo"}));
    assert!((tray::volume(&s) - (t4.plain_volume() - recess)).abs() < 1e-6);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tray.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let mut s2 = Session::new();
    tray::ok(&mut s2, &json!({"cmd": "open", "path": path}));
    assert_eq!(
        s2.model().unwrap().document(),
        s.model().unwrap().document()
    );
    assert!((tray::volume(&s2) - (t4.plain_volume() - recess)).abs() < 1e-6);
    let plane = match &s2
        .model()
        .unwrap()
        .document()
        .feature(FeatureId(sk))
        .unwrap()
        .kind
    {
        rcad_document::FeatureKind::Sketch { plane, .. } => *plane,
        k => panic!("{k:?}"),
    };
    assert_eq!(
        plane,
        SketchPlane::Face(FaceRef {
            feature: FeatureId(tray::COMPARTMENTS),
            role: FaceRole::Copy {
                instance: 2,
                tool: 0,
                of: rcad_document::CopiedRole::EndCap
            }
        })
    );
}

#[test]
fn the_seed_floor_and_occurrence_walls_resolve_among_coplanar_faces() {
    let t = Tray::BASELINE;
    let s = tray::build(&t, false);
    // Seed floor (coplanar with both copies) and the back wall of occurrence 2
    // (coplanar with the back walls of 1 and 3): one face each.
    let faces: Vec<_> = s
        .summary()
        .features
        .into_iter()
        .flat_map(|f| f.faces)
        .collect();
    let has = |r: FaceRole, feature: u32| {
        faces
            .iter()
            .any(|f| f.reference.role == r && f.reference.feature == FeatureId(feature))
    };
    assert!(has(FaceRole::ExtrudeEndCap, tray::COMPARTMENT));
    for i in 1..=2 {
        for of in [
            rcad_document::CopiedRole::EndCap,
            rcad_document::CopiedRole::Side { entity: 1 },
            rcad_document::CopiedRole::Side { entity: 3 },
        ] {
            assert!(
                has(
                    FaceRole::Copy {
                        instance: i,
                        tool: 0,
                        of
                    },
                    tray::COMPARTMENTS
                ),
                "occurrence {i} {of:?}"
            );
        }
    }
    // Every candidate carries a sample inside its own face when its plane is shared.
    let floors: Vec<_> = faces
        .iter()
        .filter(|f| (f.frame.origin[2] - t.floor).abs() < 1e-9 && f.frame.z[2] > 0.5)
        .collect();
    assert_eq!(floors.len(), 3);
    assert!(floors.iter().all(|f| f.sample.is_some()));
}

#[test]
fn a_footprint_spanning_two_faces_stays_ambiguous() {
    // A block whose top is split in two by a full-width slot: the block's end
    // cap reference names a plane with two faces, both under its footprint.
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 40.0, "height": 20.0, "center": [20.0, 10.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
        json!({"cmd": "create_sketch", "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
               "template": "centered_rectangle", "width": 4.0, "height": 30.0,
               "center": [20.0, 10.0]}),
        json!({"cmd": "extrude", "sketch": 3, "distance": 3.0, "op": "cut", "target": 1}),
    ] {
        tray::ok(&mut s, &v);
    }
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "create_sketch", "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
                "template": "centered_rectangle", "width": 2.0, "height": 2.0,
                "center": [5.0, 5.0]}),
    );
    assert!(!r.ok);
    match r.error.as_ref() {
        Some(DocError::BrokenReference { detail, .. }) => {
            assert!(detail.contains("ambiguous"), "{detail}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(s.model().unwrap().document(), &before);
}

#[test]
fn sketch_on_a_mirrored_boss_and_on_a_chamfer_face() {
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 60.0, "height": 30.0, "center": [0.0, 0.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 8.0}),
        json!({"cmd": "create_sketch", "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
               "template": "centered_rectangle", "width": 10.0, "height": 10.0,
               "center": [15.0, 0.0]}),
        json!({"cmd": "extrude", "sketch": 3, "distance": 5.0, "op": "join", "target": 1}),
        json!({"cmd": "mirror", "feature": 4, "plane": "WorldYZ"}),
    ] {
        tray::ok(&mut s, &v);
    }
    let base = 60.0 * 30.0 * 8.0 + 2.0 * 500.0;
    assert!((tray::volume(&s) - base).abs() < 1e-6);
    // The mirrored boss's top (z = 13) is coplanar with the seed boss's top.
    // Occurrence frames are the seed's reflected: local x = 15 is world x = -15.
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "create_sketch", "name": "On mirrored boss",
                "plane": {"Face": {"feature": 5,
                                   "role": {"Copy": {"instance": 1, "of": "EndCap"}}}},
                "template": "centered_circle", "diameter": 4.0, "center": [15.0, 0.0]}),
    );
    let sk = r.feature.unwrap().0;
    let f = sketch_frame(&s, sk);
    assert!(
        (f.origin[2] - 13.0).abs() < 1e-9 && (f.z[2] - 1.0).abs() < 1e-12,
        "{f:?}"
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "extrude", "sketch": sk, "distance": 2.0, "op": "cut", "target": 1}),
    );
    let hole = std::f64::consts::PI * 4.0 * 2.0;
    // The hole is under the mirrored boss (x < 0), not the seed's.
    let p = tray::probe(&s, &[[-15.0, 0.0, 12.0], [15.0, 0.0, 12.0]]);
    assert_eq!(p, vec![Some(false), Some(true)]);
    let v = tray::volume(&s);
    assert!((v - (base - hole)).abs() < 0.05 * hole, "{v}");

    // Chamfer the base's front top edge, then sketch on the chamfer face.
    tray::ok(
        &mut s,
        &json!({"cmd": "chamfer", "a": {"feature": 2, "role": "ExtrudeEndCap"},
                "b": {"feature": 2, "role": {"ExtrudeSide": {"entity": 1}}}, "distance": 1.5}),
    );
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "create_sketch", "name": "On chamfer",
                "plane": {"Face": {"feature": 8, "role": "Chamfer"}},
                "template": "centered_rectangle", "width": 2.0, "height": 1.0,
                "center": [0.0, 0.0]}),
    );
    let f = sketch_frame(&s, r.feature.unwrap().0);
    let h = std::f64::consts::FRAC_1_SQRT_2;
    assert!(
        (f.z[1] + h).abs() < 1e-12 && (f.z[2] - h).abs() < 1e-12,
        "{f:?}"
    );
    // The chamfer plane passes through (y = -15, z = 8 - 1.5) and (y = -13.5, z = 8).
    let on = |p: [f64; 3]| {
        (0..3)
            .map(|i| (p[i] - f.origin[i]) * f.z[i])
            .sum::<f64>()
            .abs()
    };
    assert!(on([0.0, -15.0, 6.5]) < 1e-9 && on([3.0, -13.5, 8.0]) < 1e-9);
    // Upstream edit: a thicker base moves the chamfer face; the sketch follows.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "distance", "value": 10.0}),
    );
    let f2 = sketch_frame(&s, r.feature.unwrap().0);
    assert!((f2.origin[2] - f.origin[2] - 2.0).abs() < 1e-9, "{f2:?}");
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    let f3 = sketch_frame(&s, r.feature.unwrap().0);
    assert!((f3.origin[2] - f.origin[2]).abs() < 1e-12);
}

#[test]
fn count_one_is_the_seed_only() {
    let t = Tray {
        n: 0,
        ..Tray::BASELINE
    };
    let s = tray::build(&t, false);
    assert!((tray::volume(&s) - t.plain_volume()).abs() < 1e-6);
    tray::assert_occupancy(&s, &t, false);
}

/// Review run 5 #2: when an occurrence's floor is cut away the plane may
/// still hold one face (the seed's floor); the occurrence reference must not
/// attach to it.
#[test]
fn occurrence_reference_never_attaches_to_another_occurrences_face() {
    let mut s = Session::new();
    for v in [
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": 60.0, "height": 20.0, "center": [30.0, 10.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": 10.0}),
        json!({"cmd": "create_sketch", "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
               "template": "centered_rectangle", "width": 10.0, "height": 10.0,
               "center": [15.0, 10.0]}),
        json!({"cmd": "extrude", "sketch": 3, "distance": 3.0, "op": "cut", "target": 1}),
        json!({"cmd": "linear_pattern", "feature": 4, "count": 2, "spacing": 30.0,
               "direction": [1.0, 0.0]}),
        // A deeper cut over occurrence 2 only (x 40..50): its 3 mm floor is gone.
        json!({"cmd": "create_sketch", "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
               "template": "centered_rectangle", "width": 12.0, "height": 12.0,
               "center": [45.0, 10.0]}),
        json!({"cmd": "extrude", "sketch": 6, "distance": 5.0, "op": "cut", "target": 1}),
    ] {
        tray::ok(&mut s, &v);
    }
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "create_sketch",
                "plane": {"Face": {"feature": 5, "role": {"Copy": {"instance": 1, "of": "EndCap"}}}},
                "template": "centered_rectangle", "width": 2.0, "height": 2.0,
                "center": [15.0, 10.0]}),
    );
    assert!(!r.ok, "attached to some face: {:?}", r.message);
    match r.error {
        Some(DocError::BrokenReference { detail, .. }) => {
            assert!(detail.contains("no longer exists"), "{detail}")
        }
        e => panic!("{e:?}"),
    }
    assert_eq!(s.model().unwrap().document(), &before);
    // The seed's own floor still resolves.
    tray::ok(
        &mut s,
        &json!({"cmd": "create_sketch",
                "plane": {"Face": {"feature": 4, "role": "ExtrudeEndCap"}},
                "template": "centered_rectangle", "width": 2.0, "height": 2.0,
                "center": [15.0, 10.0]}),
    );
}

/// Review run 5 #5: a chamfer between faces sharing their plane with other
/// faces (pattern occurrences) is refused up front with the reason.
#[test]
fn chamfer_between_shared_plane_faces_is_refused() {
    let t = Tray::BASELINE;
    let mut s = tray::build(&t, false);
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "chamfer",
                "a": {"feature": tray::COMPARTMENTS, "role": {"Copy": {"instance": 1, "of": "EndCap"}}},
                "b": {"feature": tray::COMPARTMENTS, "role": {"Copy": {"instance": 1, "of": {"Side": {"entity": 1}}}}},
                "distance": 0.5}),
    );
    assert!(!r.ok);
    assert!(
        format!("{:?}", r.error).contains("share their plane"),
        "{:?}",
        r.error
    );
    assert_eq!(s.model().unwrap().document(), &before);
}
