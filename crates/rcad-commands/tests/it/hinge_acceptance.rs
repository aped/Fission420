//! Acceptance project "reusable hinge assembly" on the flat assembly model
//! (docs/ASSEMBLIES.md): a grounded leaf, a moving leaf on a
//! revolute about the pin hole's axis, a pin, and a fastener component with three
//! occurrences. Editing the fastener definition changes all three occurrences; an
//! independent copy does not. Revolute limits constrain the actual transforms;
//! nested placements follow (leaf A placed off the origin → everything follows);
//! BOM quantities; a conflicting joint and a loop are refused without changing
//! the document; undo/redo and save/reopen. Expected values are closed-form.
use crate::support;
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

fn instance_point(s: &Session, path: &str, p: [f64; 3]) -> [f64; 3] {
    let i = s
        .summary()
        .assembly
        .instances
        .into_iter()
        .find(|i| i.path == path)
        .unwrap_or_else(|| panic!("instance {path}"));
    std::array::from_fn(|k| (0..3).map(|j| i.world.r[k][j] * p[j]).sum::<f64>() + i.world.t[k])
}

fn instance_volume(s: &mut Session, path: &str) -> f64 {
    ok(s, json!({"cmd": "mass_properties"}))
        .mass_properties
        .unwrap()
        .occurrences
        .iter()
        .find(|o| o.path == path)
        .and_then(|o| o.volume)
        .unwrap_or_else(|| panic!("volume of {path}"))
}

fn refused(s: &mut Session, v: Value) -> String {
    let before = s.model().unwrap().document().clone();
    let r = tray::exec(s, &v);
    assert!(!r.ok, "{v} should be refused");
    assert_eq!(
        s.model().unwrap().document(),
        &before,
        "{v} changed the document"
    );
    format!("{:?}", r.error)
}

/// The hinge with a nested moving sub-assembly. Leaf A is grounded at
/// the origin with the pin rigidly attached; the "Wing" sub-assembly holds
/// leaf B and three linked occurrences of the screw; the hinge (revolute about
/// the exact pin-hole axis, 0–180°) drives Wing#1, so the nested screws follow
/// through the composed transforms. A fourth screw is made an independent
/// copy. Closed-form positions, structured / flattened BOM, refusals (over-
/// constraint, loop, cross-level joint, limits) with the document unchanged,
/// definition edits, undo/redo and save/reopen.
#[test]
fn hinge_nested_sub_assembly() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let (a, a_ext) = block(&mut s, 40.0, 30.0, 4.0, [-20.0, 0.0]);
    let f = s.summary().features.len() as u32 + 1;
    ok(
        &mut s,
        json!({"cmd": "create_sketch",
               "plane": {"Face": {"feature": a_ext, "role": "ExtrudeEndCap"}},
               "template": "centered_circle", "diameter": 4.0, "center": [-5.0, 0.0]}),
    );
    ok(&mut s, json!({"cmd": "cut_through_all", "sketch": f}));
    let (b, _) = block(&mut s, 40.0, 30.0, 4.0, [20.0, 0.0]);
    let (pin, _) = block(&mut s, 3.0, 3.0, 10.0, [-5.0, 0.0]);
    let (screw, screw_ext) = block(&mut s, 4.0, 4.0, 6.0, [20.0, 0.0]);
    let (_, oa) = create(&mut s, "LeafA", a);
    let (_, ob) = create(&mut s, "LeafB", b);
    let (_, op) = create(&mut s, "Pin", pin);
    let (cs, os1) = create(&mut s, "Screw", screw);
    let mut screws = vec![os1];
    for t in [[10.0, 8.0, 0.0], [10.0, -8.0, 0.0], [0.0, 40.0, 0.0]] {
        screws.push(
            out(ok(
                &mut s,
                json!({"cmd": "add_occurrence", "component": cs, "translate": t}),
            ))
            .occurrence
            .unwrap()
            .0,
        );
    }
    let (os2, os4) = (screws[1], screws[3]);
    // The fourth screw becomes an independent copy (its own component).
    let ccopy = out(ok(
        &mut s,
        json!({"cmd": "make_independent", "occurrence": os4}),
    ))
    .component
    .unwrap()
    .0;
    ok(
        &mut s,
        json!({"cmd": "set_grounded", "occurrence": oa, "grounded": true}),
    );
    ok(
        &mut s,
        json!({"cmd": "add_joint", "kind": "rigid", "parent": oa, "child": op}),
    );
    // Wing = leaf B + three linked screws (nothing moves).
    let wing = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "Wing", "part_number": "HG-WING",
               "occurrences": [ob, screws[0], screws[1], screws[2]]}),
    ));
    let ow = wing.occurrence.unwrap().0;
    let p2 = format!("O{ow}/O{os2}");
    let near = |x: [f64; 3], y: [f64; 3]| (0..3).all(|i| (x[i] - y[i]).abs() <= 1e-9);
    // Screw#2 centre: definition (20, 0, 3) + (10, 8, 0) in Wing, Wing at identity.
    assert!(near(
        instance_point(&s, &p2, [20.0, 0.0, 3.0]),
        [30.0, 8.0, 3.0]
    ));
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
    let hinge = out(ok(
        &mut s,
        json!({"cmd": "add_joint", "name": "Hinge", "kind": "revolute", "parent": oa, "child": ow,
               "axis_from": {"cylinder": {"face": {"body": a, "face": hole}}},
               "min": 0.0, "max": 180.0}),
    ))
    .joint
    .unwrap()
    .0;
    // DOF: A grounded 0, pin rigid 0, Wing revolute 1, copy free 6; nested
    // screws/leaf B are fixed placements inside Wing (6 each are reported per
    // definition occurrence, they are not driven by any joint).
    let a_sum = s.summary().assembly;
    let wing_occ = a_sum.occurrences.iter().find(|o| o.id.0 == ow).unwrap();
    assert_eq!(wing_occ.dof, 1);
    // 90°: the axis is vertical through (-5, 0); leaf B's far corner (40, 0, 0)
    // is 45 from it → (-5, ±45, 0) depending on the hole axis sign; Screw#2's
    // centre (30, 8, 3) relative (35, 8) turns by the same ±90°.
    ok(
        &mut s,
        json!({"cmd": "set_joint_value", "joint": hinge, "value": 90.0}),
    );
    let corner = instance_point(&s, &format!("O{ow}/O{ob}"), [40.0, 0.0, 0.0]);
    let sign = if near(corner, [-5.0, 45.0, 0.0]) {
        1.0
    } else {
        assert!(near(corner, [-5.0, -45.0, 0.0]), "{corner:?}");
        -1.0
    };
    // R(±90°)(35, 8) = (∓8, ±35).
    let want = [-5.0 - sign * 8.0, sign * 35.0, 3.0];
    assert!(
        near(instance_point(&s, &p2, [20.0, 0.0, 3.0]), want),
        "{:?} vs {want:?}",
        instance_point(&s, &p2, [20.0, 0.0, 3.0])
    );
    // Limits, over-constraint, loop and cross-level joints: refused, unchanged.
    refused(
        &mut s,
        json!({"cmd": "set_joint_value", "joint": hinge, "value": 180.5}),
    );
    refused(
        &mut s,
        json!({"cmd": "add_joint", "kind": "rigid", "parent": op, "child": ow}),
    );
    let e = refused(
        &mut s,
        json!({"cmd": "add_joint", "kind": "rigid", "parent": oa, "child": os2}),
    );
    assert!(e.contains("same owner"), "{e}");
    ok(
        &mut s,
        json!({"cmd": "set_grounded", "occurrence": oa, "grounded": false}),
    );
    let e = refused(
        &mut s,
        json!({"cmd": "add_joint", "kind": "rigid", "parent": ow, "child": oa}),
    );
    assert!(e.contains("loop"), "{e}");
    ok(&mut s, json!({"cmd": "undo"}));
    assert!(s.summary().assembly.occurrences[0].grounded);
    // BOM: flattened totals and the indented structure.
    let bom = out(ok(&mut s, json!({"cmd": "bom"})));
    let flat: Vec<(String, usize)> = bom
        .bom
        .unwrap()
        .iter()
        .map(|r| (r.name.clone(), r.quantity))
        .collect();
    assert_eq!(
        flat,
        [
            ("LeafA", 1),
            ("LeafB", 1),
            ("Pin", 1),
            ("Screw", 3),
            ("Screw copy", 1)
        ]
        .map(|(n, q)| (n.to_string(), q))
    );
    let tree: Vec<(String, String, usize)> = bom
        .structured_bom
        .unwrap()
        .iter()
        .map(|r| (r.item.clone(), r.name.clone(), r.quantity))
        .collect();
    assert_eq!(
        tree,
        [
            ("1", "LeafA", 1),
            ("2", "Pin", 1),
            ("3", "Screw copy", 1),
            ("4", "Wing", 1),
            ("4.1", "LeafB", 1),
            ("4.2", "Screw", 3)
        ]
        .map(|(i, n, q)| (i.to_string(), n.to_string(), q))
    );
    // Definition edit upstream of the copy point (screw height 6 → 9): the
    // linked screws change; the copy is a history-based copy made after the
    // screw's extrusion, so it follows this upstream edit too (documented).
    ok(
        &mut s,
        json!({"cmd": "set_parameter", "feature": screw_ext, "name": "distance", "value": 9.0}),
    );
    for o in &screws[..3] {
        assert!((instance_volume(&mut s, &format!("O{ow}/O{o}")) - 144.0).abs() < 1e-6);
    }
    assert!((instance_volume(&mut s, &format!("O{os4}")) - 144.0).abs() < 1e-6);
    // An edit of the definition after the copy (2 × 2 × 2 pocket): the three
    // linked screws lose 8 mm³, the independent copy does not.
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
    for o in &screws[..3] {
        assert!((instance_volume(&mut s, &format!("O{ow}/O{o}")) - 136.0).abs() < 1e-6);
    }
    assert!((instance_volume(&mut s, &format!("O{os4}")) - 144.0).abs() < 1e-6);
    let copy = s
        .summary()
        .assembly
        .components
        .into_iter()
        .find(|c| c.id.0 == ccopy)
        .unwrap();
    assert_eq!(copy.derived_from.map(|c| c.0), Some(cs));
    // Undo / redo the pocket; save / reopen with the nested joint state.
    ok(&mut s, json!({"cmd": "undo"}));
    ok(&mut s, json!({"cmd": "undo"}));
    assert!((instance_volume(&mut s, &p2) - 144.0).abs() < 1e-6);
    ok(&mut s, json!({"cmd": "redo"}));
    ok(&mut s, json!({"cmd": "redo"}));
    assert!((instance_volume(&mut s, &p2) - 136.0).abs() < 1e-6);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hinge_nested.rcad.json");
    ok(&mut s, json!({"cmd": "save", "path": path}));
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert!(near(instance_point(&t, &p2, [20.0, 0.0, 3.0]), want));
}

/// The committed replayable example `examples/hinge_assembly_workflow.json`
/// (replayed by `cargo xtask check` through `rcad-cli run`): replayed here with
/// its file paths redirected, every step as expected, closed-form placement of
/// a nested screw at 90°, no interference in the seated hinge, and the
/// committed `examples/hinge_assembly.rcad.json` equal to the replayed result.
#[test]
fn committed_hinge_example() {
    let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = std::fs::read_to_string(repo.join("examples/hinge_assembly_workflow.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut steps: Vec<Value> = serde_json::from_str(&text).unwrap();
    for st in &mut steps {
        if st["cmd"] != "set_explode_offset"
            && let Some(p) = st.get("path").and_then(Value::as_str)
        {
            let name = std::path::Path::new(p).file_name().unwrap().to_owned();
            st["path"] = json!(dir.path().join(name));
        }
    }
    let mut s = Session::new();
    for (i, st) in steps.iter().enumerate() {
        let expect = st.get("expect_ok").and_then(Value::as_bool).unwrap_or(true);
        let mut c = st.clone();
        if let Some(o) = c.as_object_mut() {
            o.remove("expect_ok");
        }
        let r = tray::exec(&mut s, &c);
        assert_eq!(r.ok, expect, "step {i} {st}: {:?}", r.error);
    }
    // Screw#2 (O5) in Wing#1 (O8): definition centre (20, 0, 3) + (10, 8, 8)
    // → (30, 8, 11); 90° about the vertical axis through (-5, 0): relative
    // (35, 8) → (-8, 35) → (-13, 35, 11).
    let p = instance_point(&s, "O8/O5", [20.0, 0.0, 3.0]);
    assert!(
        (0..3).all(|i| (p[i] - [-13.0, 35.0, 11.0][i]).abs() <= 1e-9),
        "{p:?}"
    );
    let i = out(ok(&mut s, json!({"cmd": "interference"})))
        .interference
        .unwrap();
    assert!(i.is_empty(), "{i:?}");
    let committed = repo.join("examples/hinge_assembly.rcad.json");
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "open", "path": committed}));
    assert_eq!(
        t.model().unwrap().document(),
        s.model().unwrap().document(),
        "examples/hinge_assembly.rcad.json is stale: rerun the workflow with rcad-cli"
    );
}

/// The hierarchical STEP export of the hinge example imports back with
/// its structure (components per product, Wing as a sub-assembly holding three
/// linked occurrences of one Screw definition), the same flattened BOM, and
/// the closed-form placement of a nested screw; undo removes it in one step;
/// save/reopen keeps it.
#[test]
fn hinge_step_assembly_round_trip_preserves_hierarchy() {
    let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    ok(
        &mut s,
        json!({"cmd": "open", "path": repo.join("examples/hinge_assembly.rcad.json")}),
    );
    let step = dir.path().join("hinge_tree.step");
    ok(
        &mut s,
        json!({"cmd": "export_assembly", "path": step, "format": "step_assembly"}),
    );
    let qty = |s: &mut Session| {
        let mut q: Vec<(String, usize)> = out(ok(s, json!({"cmd": "bom"})))
            .bom
            .unwrap()
            .into_iter()
            .map(|r| (r.name, r.quantity))
            .collect();
        q.sort();
        q
    };
    let want = qty(&mut s);
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "new_document"}));
    let r = ok(&mut t, json!({"cmd": "import_step_assembly", "path": step}));
    assert!(r.message.unwrap().contains("product structure"));
    let a = t.summary().assembly;
    let names: Vec<&str> = a.components.iter().map(|c| c.name.as_str()).collect();
    for n in [
        "Assembly",
        "LeafA",
        "Pin",
        "Screw copy",
        "Wing",
        "LeafB",
        "Screw",
    ] {
        assert!(names.contains(&n), "{n} in {names:?}");
    }
    let screw = a.components.iter().find(|c| c.name == "Screw").unwrap();
    let wing = a.components.iter().find(|c| c.name == "Wing").unwrap();
    let screws: Vec<_> = a
        .occurrences
        .iter()
        .filter(|o| o.component == screw.id)
        .collect();
    assert_eq!(screws.len(), 3);
    assert!(screws.iter().all(|o| o.owner == Some(wing.id)));
    assert_eq!(screw.bodies.len(), 1, "the definition is written once");
    assert_eq!(qty(&mut t), want);
    // Screw#2 centre (definition (20, 0, 4)) at the hinge pose: (-13, 35, 12).
    let i = a
        .instances
        .iter()
        .find(|i| i.path_name.ends_with("Wing#1/Screw#2"))
        .expect("nested screw");
    let p: [f64; 3] = std::array::from_fn(|k| {
        (0..3)
            .map(|j| i.world.r[k][j] * [20.0, 0.0, 4.0][j])
            .sum::<f64>()
            + i.world.t[k]
    });
    assert!(
        (0..3).all(|k| (p[k] - [-13.0, 35.0, 12.0][k]).abs() < 1e-6),
        "{p:?}"
    );
    let path = dir.path().join("imported.rcad.json");
    ok(&mut t, json!({"cmd": "save", "path": path}));
    let mut u = Session::new();
    ok(&mut u, json!({"cmd": "open", "path": path}));
    assert_eq!(u.model().unwrap().document(), t.model().unwrap().document());
    ok(&mut t, json!({"cmd": "undo"}));
    assert!(t.summary().assembly.components.is_empty());
    // A file without assembly structure is refused (use import_step).
    let flat = dir.path().join("flat.step");
    ok(
        &mut s,
        json!({"cmd": "export_assembly", "path": flat, "format": "step"}),
    );
    let r = tray::exec(
        &mut t,
        &json!({"cmd": "import_step_assembly", "path": flat}),
    );
    assert!(!r.ok);
}

/// An OpenCascade-written assembly (FreeCAD, independent writer) imports with
/// its two products as components under the root.
#[test]
fn occt_step_assembly_imports_with_structure() {
    let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "new_document"}));
    ok(
        &mut t,
        json!({"cmd": "import_step_assembly",
               "path": repo.join("crates/rcad-kernel/tests/fixtures/step/two_boxes_assembly_mm.step")}),
    );
    let a = t.summary().assembly;
    let names: Vec<&str> = a.components.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&"BoxA") && names.contains(&"BoxB"),
        "{names:?}"
    );
    // Volumes 1000 each (README of the fixture).
    let m = ok(&mut t, json!({"cmd": "mass_properties"}))
        .mass_properties
        .unwrap();
    assert_eq!(m.occurrences.len(), 2);
    for o in &m.occurrences {
        assert!((o.volume.unwrap() - 1000.0).abs() < 1e-6);
    }
}
