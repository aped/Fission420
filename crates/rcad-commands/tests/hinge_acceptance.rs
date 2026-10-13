//! Run-7 acceptance project 1 ("reusable hinge assembly"), scoped to the run-6
//! flat assembly model (docs/ASSEMBLIES.md): a grounded leaf, a moving leaf on a
//! revolute about the pin hole's axis, a pin, and a fastener component with three
//! occurrences. Editing the fastener definition changes all three occurrences; an
//! independent copy does not. Revolute limits constrain the actual transforms;
//! nested placements follow (leaf A placed off the origin → everything follows);
//! BOM quantities; a conflicting joint and a loop are refused without changing
//! the document; undo/redo and save/reopen. Expected values are closed-form.
mod support;
use rcad_commands::assembly::AssemblyOut;
use rcad_commands::{Response, Session};
use rcad_kernel::DirectFace;
use serde_json::{Value, json};
use support::tray;

fn ok(s: &mut Session, v: Value) -> Response {
    tray::ok(s, &v)
}

fn out(r: Response) -> AssemblyOut {
    r.assembly.expect("assembly output")
}

fn block(s: &mut Session, w: f64, h: f64, d: f64, c: [f64; 2]) -> (u32, u32) {
    let f = s.summary().features.len() as u32 + 1;
    ok(
        s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": w, "height": h, "center": c}),
    );
    ok(s, json!({"cmd": "extrude", "sketch": f, "distance": d}));
    (s.summary().bodies.last().unwrap().id.0, f + 1)
}

fn world_point(s: &Session, occ: u32, p: [f64; 3]) -> [f64; 3] {
    let o = s
        .summary()
        .assembly
        .occurrences
        .into_iter()
        .find(|o| o.id.0 == occ)
        .expect("occurrence");
    std::array::from_fn(|i| (0..3).map(|k| o.world.r[i][k] * p[k]).sum::<f64>() + o.world.t[i])
}

fn near3(a: [f64; 3], b: [f64; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= 1e-9)
}

fn create(s: &mut Session, name: &str, body: u32) -> (u32, u32) {
    let o = out(ok(
        s,
        json!({"cmd": "create_component", "name": name, "bodies": [body],
               "part_number": format!("HG-{name}")}),
    ));
    (o.component.unwrap().0, o.occurrence.unwrap().0)
}

fn occurrence_volume(s: &mut Session, occ: u32) -> f64 {
    let r = ok(s, json!({"cmd": "mass_properties"}))
        .mass_properties
        .unwrap();
    r.occurrences
        .iter()
        .find(|o| o.occurrence.0 == occ)
        .and_then(|o| o.volume)
        .expect("occurrence volume")
}

#[test]
fn hinge_assembly() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    // Leaf A: x [-40, 0], y [-15, 15], z [0, 4], Ø4 pin hole at (-5, 0).
    let (a, a_ext) = block(&mut s, 40.0, 30.0, 4.0, [-20.0, 0.0]);
    let f = s.summary().features.len() as u32 + 1;
    ok(
        &mut s,
        json!({"cmd": "create_sketch",
               "plane": {"Face": {"feature": a_ext, "role": "ExtrudeEndCap"}},
               "template": "centered_circle", "diameter": 4.0, "center": [-5.0, 0.0]}),
    );
    ok(&mut s, json!({"cmd": "cut_through_all", "sketch": f}));
    // Leaf B: x [0, 40]; pin: 3 × 3 × 10 post at (-5, 0); screw: 4 × 4 × 6 at (20, 0).
    let (b, _) = block(&mut s, 40.0, 30.0, 4.0, [20.0, 0.0]);
    let (pin, _) = block(&mut s, 3.0, 3.0, 10.0, [-5.0, 0.0]);
    let (screw, screw_ext) = block(&mut s, 4.0, 4.0, 6.0, [20.0, 0.0]);
    let (_, oa) = create(&mut s, "LeafA", a);
    let (_, ob) = create(&mut s, "LeafB", b);
    let (_, op) = create(&mut s, "Pin", pin);
    let (cs, os1) = create(&mut s, "Screw", screw);
    let os2 = out(ok(
        &mut s,
        json!({"cmd": "add_occurrence", "component": cs, "translate": [10.0, 8.0, 0.0]}),
    ))
    .occurrence
    .unwrap()
    .0;
    let os3 = out(ok(
        &mut s,
        json!({"cmd": "add_occurrence", "component": cs, "translate": [10.0, -8.0, 0.0]}),
    ))
    .occurrence
    .unwrap()
    .0;
    // An independent copy of the screw body (not an occurrence).
    ok(
        &mut s,
        json!({"cmd": "move_body", "body": screw, "translate": [0.0, 40.0, 0.0], "copy": true}),
    );
    // Leaf A placed off the origin and grounded: everything jointed follows.
    ok(
        &mut s,
        json!({"cmd": "set_occurrence_transform", "occurrence": oa, "translate": [100.0, 0.0, 0.0]}),
    );
    ok(
        &mut s,
        json!({"cmd": "set_grounded", "occurrence": oa, "grounded": true}),
    );
    // Hinge: revolute about the pin hole's axis (exact from the cylinder face), 0–180°.
    let n = s
        .summary()
        .bodies
        .iter()
        .find(|x| x.id.0 == a)
        .unwrap()
        .measurements
        .as_ref()
        .unwrap()
        .face_count as u32;
    let hole = (0..n)
        .find(|&i| {
            tray::exec(
                &mut s,
                &json!({"cmd": "measure_faces", "a": {"body": a, "face": i}}),
            )
            .face_measure
            .is_some_and(|r| matches!(r.a, DirectFace::Cylinder { .. }))
        })
        .expect("hole face");
    // Before the joint, leaf B sits next to leaf A's definition; move it next to the
    // placed leaf A so the hinge starts closed-flat.
    ok(
        &mut s,
        json!({"cmd": "set_occurrence_transform", "occurrence": ob, "translate": [100.0, 0.0, 0.0]}),
    );
    let hinge = out(ok(
        &mut s,
        json!({"cmd": "add_joint", "name": "Hinge", "kind": "revolute", "parent": oa, "child": ob,
               "axis_from": {"cylinder": {"face": {"body": a, "face": hole}}},
               "min": 0.0, "max": 180.0}),
    ))
    .joint
    .unwrap()
    .0;
    ok(
        &mut s,
        json!({"cmd": "set_occurrence_transform", "occurrence": op, "translate": [100.0, 0.0, 0.0]}),
    );
    ok(
        &mut s,
        json!({"cmd": "add_joint", "kind": "rigid", "parent": oa, "child": op}),
    );
    for o in [os1, os2, os3] {
        let t = s
            .summary()
            .assembly
            .occurrences
            .iter()
            .find(|x| x.id.0 == o)
            .unwrap()
            .translate;
        ok(
            &mut s,
            json!({"cmd": "set_occurrence_transform", "occurrence": o,
                   "translate": [t[0] + 100.0, t[1], t[2]]}),
        );
        ok(
            &mut s,
            json!({"cmd": "add_joint", "kind": "rigid", "parent": ob, "child": o}),
        );
    }
    // DOF: A grounded 0, B revolute 1, pin rigid 0, screws rigid 0.
    assert_eq!(s.summary().assembly.total_dof, 1);
    // Rotate 90°: leaf B's far corner (40, 0, 0) is 45 from the axis at world
    // (95, 0); it lands at (95, ±45, 0). Screw #2's centre (definition (20, 0, 3),
    // placed +(110, 8, 0) → world (130, 8, 3) before rotation) follows: distance
    // from the axis preserved, height unchanged.
    let s2_before = world_point(&s, os2, [20.0, 0.0, 3.0]);
    assert!(near3(s2_before, [130.0, 8.0, 3.0]), "{s2_before:?}");
    ok(
        &mut s,
        json!({"cmd": "set_joint_value", "joint": hinge, "value": 90.0}),
    );
    let corner = world_point(&s, ob, [40.0, 0.0, 0.0]);
    assert!(
        near3(corner, [95.0, 45.0, 0.0]) || near3(corner, [95.0, -45.0, 0.0]),
        "{corner:?}"
    );
    let s2 = world_point(&s, os2, [20.0, 0.0, 3.0]);
    let r0 = ((s2_before[0] - 95.0).powi(2) + s2_before[1].powi(2)).sqrt();
    let r1 = ((s2[0] - 95.0).powi(2) + s2[1].powi(2)).sqrt();
    assert!(
        (r0 - r1).abs() < 1e-9 && (s2[2] - 3.0).abs() < 1e-9,
        "{s2:?}"
    );
    // The pin stays with leaf A.
    assert!(near3(
        world_point(&s, op, [-5.0, 0.0, 0.0]),
        [95.0, 0.0, 0.0]
    ));
    // Limits enforced: 200° refused, document unchanged.
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "set_joint_value", "joint": hinge, "value": 200.0}),
    );
    assert!(!r.ok);
    assert_eq!(s.model().unwrap().document(), &before);
    // Conflicting joint (a second joint on leaf B) and a loop (leaf A as the child
    // of the pin, which descends from A) are refused, document unchanged.
    for bad in [
        json!({"cmd": "add_joint", "kind": "rigid", "parent": op, "child": ob}),
        json!({"cmd": "add_joint", "kind": "rigid", "parent": op, "child": oa}),
    ] {
        let r = tray::exec(&mut s, &bad);
        assert!(!r.ok, "{bad}");
        assert_eq!(s.model().unwrap().document(), &before, "{bad}");
    }
    // BOM: LeafA, LeafB, Pin × 1, Screw × 3 and the loose copy × 1.
    let bom = out(ok(&mut s, json!({"cmd": "bom"}))).bom.unwrap();
    let q: Vec<(String, usize)> = bom.iter().map(|r| (r.name.clone(), r.quantity)).collect();
    assert_eq!(
        q[..4],
        [
            ("LeafA".to_string(), 1),
            ("LeafB".to_string(), 1),
            ("Pin".to_string(), 1),
            ("Screw".to_string(), 3)
        ]
    );
    assert_eq!(bom.len(), 5);
    assert_eq!(bom[3].part_number, "HG-Screw");
    // Edit the screw definition (taller: 6 → 9 mm): all three occurrences change,
    // the copy (made before by move_body copy) follows this upstream edit too, so
    // demonstrate independence with an edit after the copy: a 2 × 2 × 2 pocket.
    ok(
        &mut s,
        json!({"cmd": "set_parameter", "feature": screw_ext, "name": "distance", "value": 9.0}),
    );
    for o in [os1, os2, os3] {
        assert!((occurrence_volume(&mut s, o) - 144.0).abs() < 1e-6);
    }
    let f = s.summary().features.len() as u32 + 1;
    ok(
        &mut s,
        json!({"cmd": "create_sketch",
               "plane": {"Face": {"feature": screw_ext, "role": "ExtrudeEndCap"}},
               "template": "centered_rectangle", "width": 2.0, "height": 2.0,
               "center": [20.0, 0.0]}),
    );
    ok(
        &mut s,
        json!({"cmd": "extrude", "sketch": f, "distance": 2.0, "op": "cut", "target": screw}),
    );
    for o in [os1, os2, os3] {
        assert!((occurrence_volume(&mut s, o) - 136.0).abs() < 1e-6);
    }
    let copy = s.summary().bodies.last().unwrap().clone();
    match copy.measurements.unwrap().volume {
        rcad_kernel::VolumeResult::Value { volume, .. } => {
            assert!(
                (volume - 144.0).abs() < 1e-6,
                "the copy is independent: {volume}"
            )
        }
        v => panic!("{v:?}"),
    }
    // Undo / redo the pocket; save and reopen with the joint state.
    ok(&mut s, json!({"cmd": "undo"}));
    assert!((occurrence_volume(&mut s, os3) - 144.0).abs() < 1e-6);
    ok(&mut s, json!({"cmd": "redo"}));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hinge.rcad.json");
    ok(&mut s, json!({"cmd": "save", "path": path}));
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert!(near3(world_point(&t, ob, [40.0, 0.0, 0.0]), corner));
    assert_eq!(t.summary().assembly.total_dof, 1);
}
