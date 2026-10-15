//! Acceptance project "slider fixture": a parameterized
//! rail (grounded) and carriage on a slider joint limited to 0–40 mm. Positions
//! inside the limits move the carriage by exactly the commanded distance;
//! positions outside are refused with the document unchanged. The travel is
//! then raised to 60 mm and the document reopened. The displacement and the
//! remaining degree of freedom are checked independently of the joint value:
//! the world pose of definition points (closed form) and the observed motion
//! (rotation fixed, translation only along x). A rail-length edit upstream
//! regenerates without moving the carriage. A deliberately overlapping second
//! carriage is reported by the static interference check with both occurrences
//! identified and the closed-form overlap volume; the touching carriage/rail
//! contact is not reported. Static interference is not collision avoidance.
use crate::support;
use rcad_commands::assembly::{AssemblyOut, InterferenceStatus};
use rcad_commands::{Response, Session};
use serde_json::{Value, json};
use support::tray;

fn ok(s: &mut Session, v: Value) -> Response {
    tray::ok(s, &v)
}

fn out(r: Response) -> AssemblyOut {
    r.assembly.expect("assembly output")
}

fn refused(s: &mut Session, v: Value) {
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(s, &v);
    assert!(!r.ok, "{v} should be refused");
    assert_eq!(
        s.model().unwrap().document(),
        &before,
        "{v} changed the document"
    );
}

fn block(s: &mut Session, w: f64, h: f64, d: f64, c: [f64; 2]) -> (u32, u32, u32) {
    let f = s.summary().features.len() as u32 + 1;
    ok(
        s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": w, "height": h, "center": c}),
    );
    ok(s, json!({"cmd": "extrude", "sketch": f, "distance": d}));
    (s.summary().bodies.last().unwrap().id.0, f, f + 1)
}

fn pose(s: &Session, occ: u32) -> rcad_document::assembly::Pose {
    s.summary()
        .assembly
        .occurrences
        .into_iter()
        .find(|o| o.id.0 == occ)
        .expect("occurrence")
        .world
}

fn world(s: &Session, occ: u32, p: [f64; 3]) -> [f64; 3] {
    let w = pose(s, occ);
    std::array::from_fn(|i| (0..3).map(|k| w.r[i][k] * p[k]).sum::<f64>() + w.t[i])
}

const I3: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Carriage corner (0, -10, 0) in its definition; at slider value `v` it sits
/// at (v, -10, 10) (placed on top of the rail, then moved along +x).
fn check_at(s: &Session, carriage: u32, v: f64) {
    let p = world(s, carriage, [0.0, -10.0, 0.0]);
    let want = [v, -10.0, 10.0];
    assert!(
        (0..3).all(|i| (p[i] - want[i]).abs() <= 1e-12),
        "{p:?} vs {want:?}"
    );
    // Remaining freedom observed: the rotation never changes.
    assert_eq!(pose(s, carriage).r, I3);
}

fn rail_volume(s: &mut Session, occ: u32) -> f64 {
    ok(s, json!({"cmd": "mass_properties"}))
        .mass_properties
        .unwrap()
        .occurrences
        .iter()
        .find(|o| o.occurrence.0 == occ)
        .and_then(|o| o.volume)
        .unwrap()
}

#[test]
fn slider_fixture() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    // Rail: 100 × 10 × 10 along x from 0 to 100. Carriage: 20 × 20 × 8 with
    // its definition at x 0..20, y -10..10, z 0..8.
    let (rail, rail_sketch, _) = block(&mut s, 100.0, 10.0, 10.0, [50.0, 0.0]);
    let (carriage, _, _) = block(&mut s, 20.0, 20.0, 8.0, [10.0, 0.0]);
    let o = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "Rail", "bodies": [rail], "part_number": "SF-RAIL"}),
    ));
    let (_, orail) = (o.component.unwrap().0, o.occurrence.unwrap().0);
    let o = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "Carriage", "bodies": [carriage],
               "part_number": "SF-CAR"}),
    ));
    let (ccar, ocar) = (o.component.unwrap().0, o.occurrence.unwrap().0);
    ok(
        &mut s,
        json!({"cmd": "set_grounded", "occurrence": orail, "grounded": true}),
    );
    // Seat the carriage on the rail (z 10), then the slider along +x, 0–40 mm.
    ok(
        &mut s,
        json!({"cmd": "set_occurrence_transform", "occurrence": ocar, "translate": [0.0, 0.0, 10.0]}),
    );
    let j = out(ok(
        &mut s,
        json!({"cmd": "add_joint", "name": "Slide", "kind": "slider", "parent": orail,
               "child": ocar, "axis": [1.0, 0.0, 0.0], "min": 0.0, "max": 40.0}),
    ))
    .joint
    .unwrap()
    .0;
    // DOF: rail grounded (0) + carriage on a slider (1).
    assert_eq!(s.summary().assembly.total_dof, 1);
    check_at(&s, ocar, 0.0);
    for v in [12.5, 40.0, 0.0, 25.0] {
        ok(
            &mut s,
            json!({"cmd": "set_joint_value", "joint": j, "value": v}),
        );
        check_at(&s, ocar, v);
    }
    // Outside the limits: refused, pose unchanged (no drift, no clamping).
    for v in [40.000001, -0.5, 60.0, f64::MAX] {
        refused(
            &mut s,
            json!({"cmd": "set_joint_value", "joint": j, "value": v}),
        );
        check_at(&s, ocar, 25.0);
    }
    // The carriage cannot be placed directly (its only freedom is the joint),
    // and the grounded rail cannot move.
    refused(
        &mut s,
        json!({"cmd": "set_occurrence_transform", "occurrence": ocar, "translate": [5.0, 5.0, 5.0]}),
    );
    refused(
        &mut s,
        json!({"cmd": "set_occurrence_transform", "occurrence": orail, "translate": [5.0, 0.0, 0.0]}),
    );
    // Travel → 60 mm.
    ok(
        &mut s,
        json!({"cmd": "set_joint_limits", "joint": j, "min": 0.0, "max": 60.0}),
    );
    ok(
        &mut s,
        json!({"cmd": "set_joint_value", "joint": j, "value": 55.0}),
    );
    check_at(&s, ocar, 55.0);
    refused(
        &mut s,
        json!({"cmd": "set_joint_value", "joint": j, "value": 60.5}),
    );
    // Narrowing the travel below the current position is refused.
    refused(
        &mut s,
        json!({"cmd": "set_joint_limits", "joint": j, "min": 0.0, "max": 40.0}),
    );
    // Upstream edit: rail length 100 → 140 (centred at x 50): the rail's
    // volume follows (140 × 10 × 10), the carriage stays at 55.
    assert!((rail_volume(&mut s, orail) - 10000.0).abs() < 1e-6);
    ok(
        &mut s,
        json!({"cmd": "set_parameter", "feature": rail_sketch, "name": "width", "value": 140.0}),
    );
    assert!((rail_volume(&mut s, orail) - 14000.0).abs() < 1e-6);
    check_at(&s, ocar, 55.0);
    // Undo / redo the length edit.
    ok(&mut s, json!({"cmd": "undo"}));
    assert!((rail_volume(&mut s, orail) - 10000.0).abs() < 1e-6);
    ok(&mut s, json!({"cmd": "redo"}));
    // Save, reopen: limits, value, pose and DOF survive.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slider.rcad.json");
    ok(&mut s, json!({"cmd": "save", "path": path}));
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    check_at(&t, ocar, 55.0);
    assert_eq!(t.summary().assembly.total_dof, 1);
    let jt = t.summary().assembly.joints[0].clone();
    assert_eq!((jt.min, jt.max, jt.value), (Some(0.0), Some(60.0), 55.0));
    refused(
        &mut t,
        json!({"cmd": "set_joint_value", "joint": j, "value": 61.0}),
    );
    // Interference: the carriage touches the rail (z = 10) — not reported.
    let list = out(ok(&mut t, json!({"cmd": "interference"})))
        .interference
        .unwrap();
    assert!(list.is_empty(), "{list:?}");
    // A second carriage deliberately overlapping: at value 10 carriage #1 spans
    // x 10..30, y -10..10, z 10..18; carriage #2 placed at (25, 3, 12) spans
    // x 25..45, y -7..13, z 12..20. Overlap 5 × 17 × 6 = 510 mm³.
    ok(
        &mut t,
        json!({"cmd": "set_joint_value", "joint": j, "value": 10.0}),
    );
    let ocar2 = out(ok(
        &mut t,
        json!({"cmd": "add_occurrence", "component": ccar, "translate": [25.0, 3.0, 12.0]}),
    ))
    .occurrence
    .unwrap()
    .0;
    let list = out(ok(&mut t, json!({"cmd": "interference"})))
        .interference
        .unwrap();
    assert_eq!(list.len(), 1, "{list:?}");
    let i = &list[0];
    assert_eq!((i.a.0, i.b.0), (ocar, ocar2));
    assert_eq!(
        (i.a_name.as_str(), i.b_name.as_str()),
        ("Carriage#1", "Carriage#2")
    );
    assert_eq!(
        (i.a_path.clone(), i.b_path.clone()),
        (format!("O{ocar}"), format!("O{ocar2}"))
    );
    match &i.status {
        InterferenceStatus::Overlap { volume, exact, .. } => {
            assert!((volume - 510.0).abs() < 1e-6, "{volume}");
            assert!(*exact);
        }
        st => panic!("{st:?}"),
    }
    // Sliding the first carriage back to 0 clears it (x 0..20 vs 25..45).
    ok(
        &mut t,
        json!({"cmd": "set_joint_value", "joint": j, "value": 0.0}),
    );
    let list = out(ok(&mut t, json!({"cmd": "interference"})))
        .interference
        .unwrap();
    assert!(list.is_empty(), "{list:?}");
}

/// Sampled motion check over the slider travel: carriage 1 (x 0..20 + value)
/// against a fixed second carriage at x 25..45, y -7..13, z 12..20: they
/// overlap exactly when value > 5, in (min(v + 20, 45) − max(v, 25)) × 17 × 6 mm³
/// (closed form); the document is unchanged; a range beyond the limits and
/// fewer than 2 samples are refused.
#[test]
fn slider_sweep_reports_where_it_collides() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let (rail, _, _) = block(&mut s, 100.0, 10.0, 10.0, [50.0, 0.0]);
    let (car, _, _) = block(&mut s, 20.0, 20.0, 8.0, [10.0, 0.0]);
    let orail = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "Rail", "bodies": [rail]}),
    ))
    .occurrence
    .unwrap()
    .0;
    let o = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "Carriage", "bodies": [car]}),
    ));
    let (ccar, ocar) = (o.component.unwrap().0, o.occurrence.unwrap().0);
    ok(
        &mut s,
        json!({"cmd": "set_grounded", "occurrence": orail, "grounded": true}),
    );
    ok(
        &mut s,
        json!({"cmd": "set_occurrence_transform", "occurrence": ocar, "translate": [0.0, 0.0, 10.0]}),
    );
    let j = out(ok(
        &mut s,
        json!({"cmd": "add_joint", "kind": "slider", "parent": orail, "child": ocar,
               "axis": [1.0, 0.0, 0.0], "min": 0.0, "max": 40.0}),
    ))
    .joint
    .unwrap()
    .0;
    // A fixed obstacle carriage at x 25..45, y -7..13, z 12..20.
    ok(
        &mut s,
        json!({"cmd": "add_occurrence", "component": ccar, "translate": [25.0, 3.0, 12.0]}),
    );
    let before = s.model().unwrap().document().clone();
    let sw = out(ok(
        &mut s,
        // Samples 2, 8, …, 38 (no sample puts faces exactly coplanar, where
        // Truck's intersection is unreliable).
        json!({"cmd": "joint_sweep", "joint": j, "from": 2.0, "to": 38.0, "steps": 7}),
    ))
    .sweep
    .unwrap();
    assert_eq!(s.model().unwrap().document(), &before);
    assert_eq!(sw.len(), 7);
    for st in &sw {
        // Carriage 1 spans x v..v+20, the obstacle 25..45: the x overlap is
        // min(v + 20, 45) − max(v, 25).
        let ox = (st.value + 20.0).min(45.0) - st.value.max(25.0);
        let hit = st
            .interference
            .iter()
            .find(|i| i.a.0 == ocar && i.b.0 != orail);
        if ox > 0.0 {
            let i = hit.unwrap_or_else(|| panic!("no hit at {}", st.value));
            match &i.status {
                InterferenceStatus::Overlap { volume, .. } => {
                    assert!(
                        (volume - ox * 17.0 * 6.0).abs() < 1e-6,
                        "{} {volume}",
                        st.value
                    )
                }
                x => panic!("{x:?}"),
            }
        } else {
            assert!(hit.is_none(), "{}: {hit:?}", st.value);
        }
    }
    refused(
        &mut s,
        json!({"cmd": "joint_sweep", "joint": j, "from": 0.0, "to": 50.0, "steps": 5}),
    );
    refused(
        &mut s,
        json!({"cmd": "joint_sweep", "joint": j, "from": 0.0, "to": 40.0, "steps": 1}),
    );
}
