//! Nested assemblies (docs/ASSEMBLIES.md): sub-assembly components,
//! instance paths, world = composition along the path, cyclic containment
//! refused, structured / flattened BOM with reuse and suppression, mass
//! aggregated through nesting, joints inside a sub-assembly, active-component
//! editing, hidden / exploded display state, independent copies, alignment,
//! more joint types, undo/redo and v7 save/reopen. Expected values are
//! closed-form (hand-computed below), not read back from the implementation.
mod support;
use rcad_commands::assembly::{AssemblyOut, InterferenceStatus};
use rcad_commands::{Response, Session};
use serde_json::{Value, json};
use support::tray;

fn ok(s: &mut Session, v: Value) -> Response {
    tray::ok(s, &v)
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
    r.error.map(|e| format!("{e:?}")).unwrap_or_default()
}

fn out(r: Response) -> AssemblyOut {
    r.assembly.expect("assembly output")
}

/// A box `w` × `h` × `d` with its base centred at `c` (z from 0); (body, extrude feature).
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

fn near3(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= tol)
}

fn flat_qty(s: &mut Session) -> Vec<(String, usize)> {
    out(ok(s, json!({"cmd": "bom"})))
        .bom
        .unwrap()
        .into_iter()
        .map(|r| (r.name, r.quantity))
        .collect()
}

fn structured(s: &mut Session) -> Vec<(String, usize, String, usize)> {
    out(ok(s, json!({"cmd": "bom"})))
        .structured_bom
        .unwrap()
        .into_iter()
        .map(|r| (r.item, r.level, r.name, r.quantity))
        .collect()
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

fn version(p: &std::path::Path) -> u64 {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    v["version"].as_u64().unwrap()
}

/// Plate 40 × 20 × 4 at the origin (V 3200), bolt 2 × 2 × 10 at (30, 0) (V 40).
/// Module = {Plate#1, Bolt#1, Bolt#2 (+10 x)}; Module#1 at identity, Module#2
/// rotated 90° about Z and moved +100 y.
struct Fixture {
    s: Session,
    plate_body: u32,
    bolt_ext: u32,
    plate: u32,
    bolt: u32,
    module: u32,
    o_plate: u32,
    o_bolt1: u32,
    o_bolt2: u32,
    o_mod1: u32,
    o_mod2: u32,
}

fn fixture() -> Fixture {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let (plate_body, _) = block(&mut s, 40.0, 20.0, 4.0, [0.0, 0.0]);
    let (bolt_body, bolt_ext) = block(&mut s, 2.0, 2.0, 10.0, [30.0, 0.0]);
    for b in [plate_body, bolt_body] {
        ok(
            &mut s,
            json!({"cmd": "set_body_material", "body": b, "density": 1.2}),
        );
    }
    let o = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "Plate", "bodies": [plate_body], "part_number": "PL-1"}),
    ));
    let (plate, o_plate) = (o.component.unwrap().0, o.occurrence.unwrap().0);
    let o = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "Bolt", "bodies": [bolt_body], "part_number": "BT-1"}),
    ));
    let (bolt, o_bolt1) = (o.component.unwrap().0, o.occurrence.unwrap().0);
    // Group the two into a sub-assembly: nothing moves.
    let o = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "Module", "occurrences": [o_plate, o_bolt1],
               "part_number": "MOD-1"}),
    ));
    let (module, o_mod1) = (o.component.unwrap().0, o.occurrence.unwrap().0);
    let o_bolt2 = out(ok(
        &mut s,
        json!({"cmd": "add_occurrence", "component": bolt, "owner": module,
               "translate": [10.0, 0.0, 0.0]}),
    ))
    .occurrence
    .unwrap()
    .0;
    let o_mod2 = out(ok(
        &mut s,
        json!({"cmd": "add_occurrence", "component": module, "translate": [0.0, 100.0, 0.0],
               "angle_deg": 90.0}),
    ))
    .occurrence
    .unwrap()
    .0;
    Fixture {
        s,
        plate_body,
        bolt_ext,
        plate,
        bolt,
        module,
        o_plate,
        o_bolt1,
        o_bolt2,
        o_mod1,
        o_mod2,
    }
}

#[test]
fn nested_transforms_bom_and_mass() {
    let mut f = fixture();
    let s = &mut f.s;
    let (m1, m2, b1, b2) = (f.o_mod1, f.o_mod2, f.o_bolt1, f.o_bolt2);
    // Six leaf instances, two sub-assembly instances.
    let inst = s.summary().assembly.instances;
    assert_eq!(inst.len(), 8);
    let paths: Vec<String> = inst.iter().map(|i| i.path.clone()).collect();
    for p in [
        format!("O{m1}"),
        format!("O{m1}/O{}", f.o_plate),
        format!("O{m1}/O{b1}"),
        format!("O{m1}/O{b2}"),
        format!("O{m2}/O{b2}"),
    ] {
        assert!(paths.contains(&p), "{p} in {paths:?}");
    }
    let n2 = inst
        .iter()
        .find(|i| i.path == format!("O{m2}/O{b2}"))
        .unwrap();
    assert_eq!(n2.path_name, "Module#2/Bolt#2");
    assert_eq!(n2.level, 1);
    // Bolt top centre (30, 0, 10): in Module (40, 0, 10); Module#1 identity;
    // Module#2: rotate 90° about Z → (0, 40, 10), +100 y → (0, 140, 10).
    assert!(near3(
        instance_point(s, &format!("O{m1}/O{b2}"), [30.0, 0.0, 10.0]),
        [40.0, 0.0, 10.0],
        1e-9
    ));
    assert!(near3(
        instance_point(s, &format!("O{m2}/O{b2}"), [30.0, 0.0, 10.0]),
        [0.0, 140.0, 10.0],
        1e-9
    ));
    // Flattened: Plate 2, Bolt 4 (2 per module × 2 modules); no row for the
    // body-less Module.
    assert_eq!(
        flat_qty(s),
        vec![("Plate".to_string(), 2), ("Bolt".to_string(), 4)]
    );
    // Structured: Module ×2 at the top, per module Plate ×1, Bolt ×2.
    assert_eq!(
        structured(s),
        vec![
            ("1".into(), 0, "Module".into(), 2),
            ("1.1".into(), 1, "Plate".into(), 1),
            ("1.2".into(), 1, "Bolt".into(), 2),
        ]
    );
    // Mass through nesting, density 1.2 g/cm³ = 0.0012 g/mm³.
    // Volumes: 2 × (3200 + 40 + 40) = 6560 mm³ → 7.872 g.
    // Centroids: M1 plate (0,0,2), bolts (30,0,5), (40,0,5);
    //            M2 plate (0,100,2), bolts (0,130,5), (0,140,5).
    let mp = ok(s, json!({"cmd": "mass_properties"}))
        .mass_properties
        .unwrap();
    assert_eq!(mp.occurrences.len(), 6);
    let t = mp.total.expect("total");
    assert!((t.volume - 6560.0).abs() < 1e-6, "{}", t.volume);
    assert!((t.mass - 7.872).abs() < 1e-9, "{}", t.mass);
    let want = [
        2800.0 / 6560.0,
        (3200.0 * 100.0 + 40.0 * 130.0 + 40.0 * 140.0) / 6560.0,
        13600.0 / 6560.0,
    ];
    assert!(
        near3(t.center_of_mass, want, 1e-9),
        "{:?}",
        t.center_of_mass
    );
    // Suppressing Bolt#2 in the Module definition removes it from both modules.
    ok(
        s,
        json!({"cmd": "set_occurrence_suppressed", "occurrence": b2, "suppressed": true}),
    );
    assert_eq!(
        flat_qty(s),
        vec![("Plate".to_string(), 2), ("Bolt".to_string(), 2)]
    );
    assert_eq!(structured(s)[2], ("1.2".into(), 1, "Bolt".into(), 1));
    // Suppressing Module#2 removes its whole subtree.
    ok(
        s,
        json!({"cmd": "set_occurrence_suppressed", "occurrence": m2, "suppressed": true}),
    );
    assert_eq!(
        flat_qty(s),
        vec![("Plate".to_string(), 1), ("Bolt".to_string(), 1)]
    );
    assert_eq!(structured(s)[0], ("1".into(), 0, "Module".into(), 1));
    ok(s, json!({"cmd": "undo"}));
    ok(s, json!({"cmd": "undo"}));
    assert_eq!(
        flat_qty(s),
        vec![("Plate".to_string(), 2), ("Bolt".to_string(), 4)]
    );
    // Upstream edit of the bolt definition (10 → 12 mm): all four instances.
    ok(
        s,
        json!({"cmd": "set_parameter", "feature": f.bolt_ext, "name": "distance", "value": 12.0}),
    );
    for p in [
        format!("O{m1}/O{b1}"),
        format!("O{m1}/O{b2}"),
        format!("O{m2}/O{b1}"),
        format!("O{m2}/O{b2}"),
    ] {
        assert!((instance_volume(s, &p) - 48.0).abs() < 1e-6, "{p}");
    }
}

#[test]
fn cyclic_containment_and_cross_level_joints_are_refused() {
    let mut f = fixture();
    let s = &mut f.s;
    // Module inside Plate (Module contains Plate) and Module inside itself.
    let e = refused(
        s,
        json!({"cmd": "add_occurrence", "component": f.module, "owner": f.plate}),
    );
    assert!(e.contains("cyclic"), "{e}");
    refused(
        s,
        json!({"cmd": "add_occurrence", "component": f.module, "owner": f.module}),
    );
    // A joint between a top-level occurrence and a nested one.
    let e = refused(
        s,
        json!({"cmd": "add_joint", "kind": "rigid", "parent": f.o_mod1, "child": f.o_bolt1}),
    );
    assert!(e.contains("same owner"), "{e}");
    // Grouping occurrences with different owners.
    refused(
        s,
        json!({"cmd": "create_component", "name": "Bad", "occurrences": [f.o_mod2, f.o_bolt1]}),
    );
    // Deleting a component used inside another one.
    refused(s, json!({"cmd": "delete_component", "component": f.bolt}));
    // A bad exploded path.
    refused(
        s,
        json!({"cmd": "set_explode_offset", "path": format!("O{}/O{}", f.o_bolt1, f.o_mod1),
               "offset": [0.0, 0.0, 5.0]}),
    );
}

#[test]
fn joints_inside_a_sub_assembly_drive_every_instance() {
    let mut f = fixture();
    let s = &mut f.s;
    // Slider Plate#1 → Bolt#2 along +x inside the Module definition, 0–15 mm.
    let j = out(ok(
        s,
        json!({"cmd": "add_joint", "kind": "slider", "parent": f.o_plate, "child": f.o_bolt2,
               "axis": [1.0, 0.0, 0.0], "min": 0.0, "max": 15.0}),
    ))
    .joint
    .unwrap()
    .0;
    ok(
        s,
        json!({"cmd": "set_joint_value", "joint": j, "value": 5.0}),
    );
    // Module#1: (30,0,10) → (45, 0, 10); Module#2: rotate → (0, 45, 10) + 100 y.
    assert!(near3(
        instance_point(
            s,
            &format!("O{}/O{}", f.o_mod1, f.o_bolt2),
            [30.0, 0.0, 10.0]
        ),
        [45.0, 0.0, 10.0],
        1e-9
    ));
    assert!(near3(
        instance_point(
            s,
            &format!("O{}/O{}", f.o_mod2, f.o_bolt2),
            [30.0, 0.0, 10.0]
        ),
        [0.0, 145.0, 10.0],
        1e-9
    ));
    refused(
        s,
        json!({"cmd": "set_joint_value", "joint": j, "value": 16.0}),
    );
    // Moving the top-level module moves its nested slider child with it.
    ok(
        s,
        json!({"cmd": "set_occurrence_transform", "occurrence": f.o_mod1,
               "translate": [0.0, 0.0, 7.0]}),
    );
    assert!(near3(
        instance_point(
            s,
            &format!("O{}/O{}", f.o_mod1, f.o_bolt2),
            [30.0, 0.0, 10.0]
        ),
        [45.0, 0.0, 17.0],
        1e-9
    ));
    // Save as v7 and reopen identical.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested.rcad.json");
    ok(s, json!({"cmd": "save", "path": path}));
    assert_eq!(version(&path), 7);
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert!(near3(
        instance_point(
            &t,
            &format!("O{}/O{}", f.o_mod2, f.o_bolt2),
            [30.0, 0.0, 10.0]
        ),
        [0.0, 145.0, 10.0],
        1e-9
    ));
    // Undo back past the slider: Bolt#2 returns to +10 x.
    ok(s, json!({"cmd": "undo"}));
    ok(s, json!({"cmd": "undo"}));
    assert!(near3(
        instance_point(
            s,
            &format!("O{}/O{}", f.o_mod1, f.o_bolt2),
            [30.0, 0.0, 10.0]
        ),
        [40.0, 0.0, 10.0],
        1e-9
    ));
    ok(s, json!({"cmd": "redo"}));
    assert!(near3(
        instance_point(
            s,
            &format!("O{}/O{}", f.o_mod1, f.o_bolt2),
            [30.0, 0.0, 10.0]
        ),
        [45.0, 0.0, 10.0],
        1e-9
    ));
}

#[test]
fn active_component_hidden_exploded_and_independent_copy() {
    let mut f = fixture();
    let s = &mut f.s;
    // Activate Plate and model a 10 × 10 × 2 boss: it joins Plate's definition
    // and appears in both modules.
    ok(
        s,
        json!({"cmd": "activate_component", "component": f.plate}),
    );
    let (boss, _) = block(s, 10.0, 10.0, 2.0, [-10.0, 0.0]);
    ok(
        s,
        json!({"cmd": "set_body_material", "body": boss, "density": 1.2}),
    );
    ok(s, json!({"cmd": "activate_component", "component": null}));
    let a = s.summary().assembly;
    let plate = a.components.iter().find(|c| c.id.0 == f.plate).unwrap();
    assert_eq!(
        plate.bodies.iter().map(|b| b.0).collect::<Vec<_>>(),
        vec![f.plate_body, boss]
    );
    assert_eq!(a.active, None);
    for m in [f.o_mod1, f.o_mod2] {
        assert!((instance_volume(s, &format!("O{m}/O{}", f.o_plate)) - 3400.0).abs() < 1e-6);
    }
    // Hidden: display only; BOM unchanged.
    ok(
        s,
        json!({"cmd": "set_occurrence_hidden", "occurrence": f.o_mod2, "hidden": true}),
    );
    let inst = s.summary().assembly.instances;
    for i in &inst {
        assert_eq!(
            i.visible,
            !i.path.starts_with(&format!("O{}", f.o_mod2)),
            "{}",
            i.path
        );
    }
    assert_eq!(flat_qty(s)[1], ("Bolt".to_string(), 4));
    // Exploded view: Module#2 +50 z, its Bolt#2 +20 x more; geometry unchanged.
    let before = instance_point(s, &format!("O{}/O{}", f.o_mod2, f.o_bolt2), [0.0; 3]);
    ok(
        s,
        json!({"cmd": "set_explode_offset", "path": format!("O{}", f.o_mod2), "offset": [0.0, 0.0, 50.0]}),
    );
    ok(
        s,
        json!({"cmd": "set_explode_offset", "path": format!("O{}/O{}", f.o_mod2, f.o_bolt2),
               "offset": [20.0, 0.0, 0.0]}),
    );
    let ex = |s: &Session, p: String| {
        s.summary()
            .assembly
            .instances
            .into_iter()
            .find(|i| i.path == p)
            .unwrap()
            .explode
    };
    // Stored but not shown yet.
    assert_eq!(ex(s, format!("O{}/O{}", f.o_mod2, f.o_bolt2)), [0.0; 3]);
    ok(s, json!({"cmd": "set_exploded", "exploded": true}));
    assert_eq!(
        ex(s, format!("O{}/O{}", f.o_mod2, f.o_bolt2)),
        [20.0, 0.0, 50.0]
    );
    assert_eq!(
        ex(s, format!("O{}/O{}", f.o_mod2, f.o_plate)),
        [0.0, 0.0, 50.0]
    );
    assert_eq!(ex(s, format!("O{}/O{}", f.o_mod1, f.o_plate)), [0.0; 3]);
    assert!(near3(
        instance_point(s, &format!("O{}/O{}", f.o_mod2, f.o_bolt2), [0.0; 3]),
        before,
        0.0
    ));
    let mp = ok(s, json!({"cmd": "mass_properties"}))
        .mass_properties
        .unwrap();
    assert!((mp.total.unwrap().volume - (6560.0 + 400.0)).abs() < 1e-6);
    // Independent copy of Bolt#2 (the definition occurrence used in both modules).
    let c = out(ok(
        s,
        json!({"cmd": "make_independent", "occurrence": f.o_bolt2}),
    ))
    .component
    .unwrap();
    let a = s.summary().assembly;
    let copy = a.components.iter().find(|x| x.id == c).unwrap();
    assert_eq!(copy.name, "Bolt copy");
    assert_eq!(copy.derived_from.map(|x| x.0), Some(f.bolt));
    assert_eq!(
        flat_qty(s),
        vec![
            ("Plate".to_string(), 2),
            ("Bolt".to_string(), 2),
            ("Bolt copy".to_string(), 2)
        ]
    );
    // Edit the bolt definition after the copy point (a 1 × 1 × 2 pocket):
    // linked Bolt#1 instances lose 2 mm³, the independent copies do not.
    let bolt_body = a
        .components
        .iter()
        .find(|x| x.id.0 == f.bolt)
        .unwrap()
        .bodies[0]
        .0;
    let g = s.summary().features.len() as u32 + 1;
    ok(
        s,
        json!({"cmd": "create_sketch",
               "plane": {"Face": {"feature": f.bolt_ext, "role": "ExtrudeEndCap"}},
               "template": "centered_rectangle", "width": 1.0, "height": 1.0,
               "center": [30.0, 0.0]}),
    );
    ok(
        s,
        json!({"cmd": "extrude", "sketch": g, "distance": 2.0, "op": "cut", "target": bolt_body}),
    );
    for m in [f.o_mod1, f.o_mod2] {
        assert!((instance_volume(s, &format!("O{m}/O{}", f.o_bolt1)) - 38.0).abs() < 1e-6);
        assert!((instance_volume(s, &format!("O{m}/O{}", f.o_bolt2)) - 40.0).abs() < 1e-6);
    }
    // Everything persists (v7) and reopens identical.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("display.rcad.json");
    ok(s, json!({"cmd": "save", "path": path}));
    assert_eq!(version(&path), 7);
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert!(t.summary().assembly.exploded);
}

#[test]
fn rename_align_rigid_group_and_interference_paths() {
    let mut f = fixture();
    let s = &mut f.s;
    ok(
        s,
        json!({"cmd": "rename_component", "component": f.module, "name": "Bracket"}),
    );
    ok(
        s,
        json!({"cmd": "rename_occurrence", "occurrence": f.o_mod2, "name": "Right"}),
    );
    refused(
        s,
        json!({"cmd": "rename_occurrence", "occurrence": f.o_mod1, "name": "Right"}),
    );
    assert_eq!(structured(s)[0].2, "Bracket");
    // Interference: within a module the bolts (x 29..31, 39..41) clear the
    // plate (x -20..20). Module#2 rotated 90° and moved +35 x: its plate spans
    // x 25..45, y -20..20 and, lifted 1 mm (z 1..5, no coplanar faces), overlaps
    // each of Module#1's bolts (y -1..1, z 0..10) in 2 × 2 × 4 = 16 mm³.
    ok(
        s,
        json!({"cmd": "set_occurrence_transform", "occurrence": f.o_mod2,
               "translate": [35.0, 0.0, 1.0], "angle_deg": 90.0}),
    );
    let list = out(ok(s, json!({"cmd": "interference"})))
        .interference
        .unwrap();
    let pairs: Vec<(String, String)> = list
        .iter()
        .map(|i| (i.a_path.clone(), i.b_path.clone()))
        .collect();
    let pl2 = format!("O{}/O{}", f.o_mod2, f.o_plate);
    for b in [f.o_bolt1, f.o_bolt2] {
        let a = format!("O{}/O{b}", f.o_mod1);
        let i = list
            .iter()
            .find(|i| i.a_path == a && i.b_path == pl2)
            .unwrap_or_else(|| panic!("{a} ∩ {pl2} in {pairs:?}"));
        match &i.status {
            InterferenceStatus::Overlap { volume, .. } => {
                assert!((volume - 16.0).abs() < 1e-6, "{volume}")
            }
            st => panic!("{st:?}"),
        }
        assert!(i.b_name.starts_with("Right/"), "{}", i.b_name);
    }
    ok(
        s,
        json!({"cmd": "set_occurrence_transform", "occurrence": f.o_mod2,
               "translate": [0.0, 100.0, 0.0]}),
    );
    // Rigid group of the two modules: one parent, one child, 0 DOF for it.
    ok(
        s,
        json!({"cmd": "create_rigid_group", "occurrences": [f.o_mod1, f.o_mod2]}),
    );
    let a = s.summary().assembly;
    let o2 = a.occurrences.iter().find(|o| o.id.0 == f.o_mod2).unwrap();
    assert_eq!(o2.dof, 0);
    assert!(o2.driven_by.is_some());
    refused(
        s,
        json!({"cmd": "create_rigid_group", "occurrences": [f.o_mod1, f.o_mod2]}),
    );
}

/// Planar face of `body` whose outward normal is `n` (exact classification).
fn face_with_normal(s: &mut Session, body: u32, n: [f64; 3]) -> u32 {
    let count = s
        .summary()
        .bodies
        .iter()
        .find(|x| x.id.0 == body)
        .unwrap()
        .measurements
        .as_ref()
        .unwrap()
        .face_count as u32;
    (0..count)
        .find(|&i| {
            tray::exec(
                s,
                &json!({"cmd": "measure_faces", "a": {"body": body, "face": i}}),
            )
            .face_measure
            .is_some_and(|r| match r.a {
                rcad_kernel::DirectFace::Plane { normal, .. } => near3(normal, n, 1e-12),
                _ => false,
            })
        })
        .expect("face")
}

#[test]
fn align_occurrence_face_to_face() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    // A: 10 × 10 × 10 at the origin; B: 4 × 4 × 2 at (50, 0).
    let (a, _) = block(&mut s, 10.0, 10.0, 10.0, [0.0, 0.0]);
    let (b, _) = block(&mut s, 4.0, 4.0, 2.0, [50.0, 0.0]);
    let oa = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "A", "bodies": [a]}),
    ))
    .occurrence
    .unwrap()
    .0;
    let ob = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "B", "bodies": [b]}),
    ))
    .occurrence
    .unwrap()
    .0;
    let (top, side) = (
        face_with_normal(&mut s, a, [0.0, 0.0, 1.0]),
        face_with_normal(&mut s, a, [1.0, 0.0, 0.0]),
    );
    let bottom = face_with_normal(&mut s, b, [0.0, 0.0, -1.0]);
    let world = |s: &Session, p: [f64; 3]| instance_point(s, &format!("O{ob}"), p);
    // B's bottom on A's top with a 1 mm gap: same orientation, z 0 → 11.
    ok(
        &mut s,
        json!({"cmd": "align_occurrence", "occurrence": ob, "from": {"body": b, "face": bottom},
               "target": oa, "to": {"body": a, "face": top}, "offset": 1.0}),
    );
    assert!(near3(world(&s, [50.0, 0.0, 0.0]), [50.0, 0.0, 11.0], 1e-9));
    // B's bottom onto A's +x side (x = 5): bottom normal turns to −x, the
    // bottom face's centre point stays in its plane and lands on x = 5.
    ok(
        &mut s,
        json!({"cmd": "align_occurrence", "occurrence": ob, "from": {"body": b, "face": bottom},
               "target": oa, "to": {"body": a, "face": side}}),
    );
    let p0 = world(&s, [50.0, 0.0, 0.0]);
    let p1 = world(&s, [50.0, 0.0, 2.0]);
    assert!((p0[0] - 5.0).abs() < 1e-9, "{p0:?}");
    // The +z direction of B now points to +x (away from A).
    assert!(near3(
        [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]],
        [2.0, 0.0, 0.0],
        1e-9
    ));
    // Not planar / grounded / self refused.
    refused(
        &mut s,
        json!({"cmd": "align_occurrence", "occurrence": ob, "from": {"body": b, "face": bottom},
               "target": ob, "to": {"body": b, "face": bottom}}),
    );
    ok(
        &mut s,
        json!({"cmd": "set_grounded", "occurrence": ob, "grounded": true}),
    );
    refused(
        &mut s,
        json!({"cmd": "align_occurrence", "occurrence": ob, "from": {"body": b, "face": bottom},
               "target": oa, "to": {"body": a, "face": top}}),
    );
}

#[test]
fn cylindrical_pin_slot_planar_and_ball_joints() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let (a, _) = block(&mut s, 10.0, 10.0, 10.0, [0.0, 0.0]);
    let (b, _) = block(&mut s, 2.0, 2.0, 2.0, [20.0, 0.0]);
    let oa = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "A", "bodies": [a]}),
    ))
    .occurrence
    .unwrap()
    .0;
    let ob = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "B", "bodies": [b]}),
    ))
    .occurrence
    .unwrap()
    .0;
    ok(
        &mut s,
        json!({"cmd": "set_grounded", "occurrence": oa, "grounded": true}),
    );
    let p = |s: &Session| instance_point(s, &format!("O{ob}"), [20.0, 0.0, 0.0]);
    // Cylindrical about Z through the origin: angle 0–90°, slide 0–30 mm.
    let j = out(ok(
        &mut s,
        json!({"cmd": "add_joint", "kind": "cylindrical", "parent": oa, "child": ob,
               "axis": [0.0, 0.0, 1.0], "min": 0.0, "max": 90.0,
               "extra": [{"value": 0.0, "min": 0.0, "max": 30.0}]}),
    ))
    .joint
    .unwrap()
    .0;
    assert_eq!(s.summary().assembly.total_dof, 2);
    ok(
        &mut s,
        json!({"cmd": "set_joint_values", "joint": j, "values": [90.0, 12.0]}),
    );
    // (20, 0, 0) → rotate 90° → (0, 20, 0) → +12 z.
    assert!(near3(p(&s), [0.0, 20.0, 12.0], 1e-9));
    // Second value outside its own limits; wrong count; NaN — all refused.
    refused(
        &mut s,
        json!({"cmd": "set_joint_value", "joint": j, "index": 1, "value": 31.0}),
    );
    refused(
        &mut s,
        json!({"cmd": "set_joint_values", "joint": j, "values": [10.0]}),
    );
    refused(
        &mut s,
        json!({"cmd": "set_joint_value", "joint": j, "index": 2, "value": 1.0}),
    );
    // Narrowing a limit below the current value is refused (no drift).
    refused(
        &mut s,
        json!({"cmd": "set_joint_limits", "joint": j, "index": 1, "min": 0.0, "max": 10.0}),
    );
    ok(
        &mut s,
        json!({"cmd": "set_joint_limits", "joint": j, "index": 1, "min": 0.0, "max": 60.0}),
    );
    ok(
        &mut s,
        json!({"cmd": "set_joint_value", "joint": j, "index": 1, "value": 55.0}),
    );
    assert!(near3(p(&s), [0.0, 20.0, 55.0], 1e-9));
    ok(&mut s, json!({"cmd": "delete_joint", "joint": j}));
    // The child keeps its pose as a free placement.
    assert!(near3(p(&s), [0.0, 20.0, 55.0], 1e-6));
    ok(
        &mut s,
        json!({"cmd": "set_occurrence_transform", "occurrence": ob, "translate": [0.0, 0.0, 0.0]}),
    );
    // Pin-slot: pin along Z at the origin, slot along +x: rotate 90°, slide 5.
    let j = out(ok(
        &mut s,
        json!({"cmd": "add_joint", "kind": "pin_slot", "parent": oa, "child": ob,
               "axis": [0.0, 0.0, 1.0], "dir2": [1.0, 0.0, 0.0]}),
    ))
    .joint
    .unwrap()
    .0;
    ok(
        &mut s,
        json!({"cmd": "set_joint_values", "joint": j, "values": [90.0, 5.0]}),
    );
    assert!(near3(p(&s), [5.0, 20.0, 0.0], 1e-9));
    ok(&mut s, json!({"cmd": "undo"}));
    ok(&mut s, json!({"cmd": "undo"}));
    // A slot direction parallel to the pin is singular: refused.
    let e = refused(
        &mut s,
        json!({"cmd": "add_joint", "kind": "pin_slot", "parent": oa, "child": ob,
               "axis": [0.0, 0.0, 1.0], "dir2": [0.0, 0.0, 2.0]}),
    );
    assert!(e.contains("parallel"), "{e}");
    // Planar (normal Z, u = x): (3, 4) in plane, 0° → +3 x, +4 y.
    let j = out(ok(
        &mut s,
        json!({"cmd": "add_joint", "kind": "planar", "parent": oa, "child": ob,
               "axis": [0.0, 0.0, 1.0], "dir2": [1.0, 0.0, 0.0]}),
    ))
    .joint
    .unwrap()
    .0;
    assert_eq!(s.summary().assembly.total_dof, 3);
    ok(
        &mut s,
        json!({"cmd": "set_joint_values", "joint": j, "values": [3.0, 4.0, 0.0]}),
    );
    assert!(near3(p(&s), [23.0, 4.0, 0.0], 1e-9));
    ok(&mut s, json!({"cmd": "undo"}));
    ok(&mut s, json!({"cmd": "undo"}));
    // Ball at the origin: 90° about u = x takes (20, 0, 0) nowhere; about
    // the axis Z it goes to (0, 20, 0).
    let j = out(ok(
        &mut s,
        json!({"cmd": "add_joint", "kind": "ball", "parent": oa, "child": ob,
               "axis": [0.0, 0.0, 1.0], "dir2": [1.0, 0.0, 0.0],
               "extra": [{"min": -45.0, "max": 45.0}, {}]}),
    ))
    .joint
    .unwrap()
    .0;
    ok(
        &mut s,
        json!({"cmd": "set_joint_values", "joint": j, "values": [90.0, 30.0, 0.0]}),
    );
    // R_z(90°) ∘ R_x(30°): (20,0,0) is on the x axis → unchanged by R_x → (0,20,0).
    assert!(near3(p(&s), [0.0, 20.0, 0.0], 1e-9));
    refused(
        &mut s,
        json!({"cmd": "set_joint_values", "joint": j, "values": [0.0, 50.0, 0.0]}),
    );
    // Save (v7) / reopen.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("joints.rcad.json");
    ok(&mut s, json!({"cmd": "save", "path": path}));
    assert_eq!(version(&path), 7);
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert!(near3(p(&t), [0.0, 20.0, 0.0], 1e-9));
}

#[test]
fn step_assembly_export_keeps_the_product_structure() {
    let mut f = fixture();
    let s = &mut f.s;
    let dir = tempfile::tempdir().unwrap();
    let tree = dir.path().join("tree.step");
    let r = ok(
        s,
        json!({"cmd": "export_assembly", "path": tree, "format": "step_assembly"}),
    );
    // Products: top, Module, Plate, Bolt; usages: 2 modules + plate + 2 bolts.
    assert!(
        r.message
            .as_deref()
            .unwrap()
            .contains("4 products, 5 usages"),
        "{:?}",
        r.message
    );
    let text = std::fs::read_to_string(&tree).unwrap();
    assert_eq!(text.matches("NEXT_ASSEMBLY_USAGE_OCCURRENCE(").count(), 5);
    assert_eq!(text.matches("MANIFOLD_SOLID_BREP(").count(), 2);
    for n in ["'Module#2'", "'Bolt#2'", "'Module'", "'Plate'"] {
        assert!(text.contains(n), "{n}");
    }
    // A suppressed sub-assembly occurrence is not exported.
    ok(
        s,
        json!({"cmd": "set_occurrence_suppressed", "occurrence": f.o_mod2, "suppressed": true}),
    );
    let r = ok(
        s,
        json!({"cmd": "export_assembly", "path": dir.path().join("tree2.step"), "format": "step_assembly"}),
    );
    assert!(r.message.unwrap().contains("4 products, 4 usages"));
    // Re-read (R-CAD's importer, flattens): 6 placed solids and an explicit
    // flatten note (FreeCAD is the independent check: tools/check_step_assembly.sh).
    let r = ok(s, json!({"cmd": "import_step", "path": tree}));
    let msg = r.message.unwrap_or_default();
    assert!(msg.contains("6 solid(s)"), "{msg}");
    assert!(msg.contains("assembly structure flattened"), "{msg}");
}

/// `set_joint_axis` redefines the axis without moving the child; later values
/// move it about the new axis (closed form).
#[test]
fn joint_axis_redefinition_keeps_the_pose() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let (a, _) = block(&mut s, 10.0, 10.0, 10.0, [0.0, 0.0]);
    let (b, _) = block(&mut s, 2.0, 2.0, 2.0, [20.0, 0.0]);
    let oa = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "A", "bodies": [a]}),
    ))
    .occurrence
    .unwrap()
    .0;
    let ob = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "B", "bodies": [b]}),
    ))
    .occurrence
    .unwrap()
    .0;
    let j = out(ok(
        &mut s,
        json!({"cmd": "add_joint", "kind": "revolute", "parent": oa, "child": ob,
               "axis": [0.0, 0.0, 1.0], "value": 0.0}),
    ))
    .joint
    .unwrap()
    .0;
    ok(
        &mut s,
        json!({"cmd": "set_joint_value", "joint": j, "value": 90.0}),
    );
    let p = |s: &Session| instance_point(s, &format!("O{ob}"), [20.0, 0.0, 0.0]);
    assert!(near3(p(&s), [0.0, 20.0, 0.0], 1e-12));
    ok(
        &mut s,
        json!({"cmd": "set_joint_axis", "joint": j, "axis": [1.0, 0.0, 0.0]}),
    );
    assert!(near3(p(&s), [0.0, 20.0, 0.0], 1e-12), "nothing moves");
    // Back to 0: a −90° turn about +x takes (0, 20, 0) to (0, 0, −20).
    ok(
        &mut s,
        json!({"cmd": "set_joint_value", "joint": j, "value": 0.0}),
    );
    assert!(near3(p(&s), [0.0, 0.0, -20.0], 1e-12), "{:?}", p(&s));
    // A zero axis is refused; undo restores the old axis and pose.
    refused(
        &mut s,
        json!({"cmd": "set_joint_axis", "joint": j, "axis": [0.0, 0.0, 0.0]}),
    );
    ok(&mut s, json!({"cmd": "undo"}));
    ok(&mut s, json!({"cmd": "undo"}));
    assert_eq!(s.summary().assembly.joints[0].axis, [0.0, 0.0, 1.0]);
    assert!(near3(p(&s), [0.0, 20.0, 0.0], 1e-12));
}

/// Regressions: configuration exclusions of deleted occurrences, an
/// interference request naming an occurrence without instances, an empty
/// nested sub-assembly in the hierarchical export, alignment to a joint
/// descendant, surplus joint values, and add_derived_body semantics.
#[test]
fn assembly_edge_case_regressions() {
    let mut f = fixture();
    let s = &mut f.s;
    // #6: a configuration excluding Module#2; deleting it drops the id.
    ok(
        s,
        json!({"cmd": "set_configuration", "name": "A", "excluded_occurrences": [f.o_mod2]}),
    );
    ok(
        s,
        json!({"cmd": "delete_occurrence", "occurrence": f.o_mod2}),
    );
    let cfg = &s.model().unwrap().document().configurations[0];
    assert!(cfg.excluded_occurrences.is_empty());
    // The configuration can still be edited afterwards.
    ok(
        s,
        json!({"cmd": "set_configuration", "name": "A", "part_number": "X"}),
    );
    ok(s, json!({"cmd": "undo"}));
    ok(s, json!({"cmd": "undo"}));
    // A saved file with a dangling exclusion (an older build) opens; the id is
    // dropped and reported.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dangling.rcad.json");
    ok(s, json!({"cmd": "save", "path": path}));
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v["document"]["configurations"][0]["excluded_occurrences"] = json!([f.o_mod2, 999]);
    std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
    let mut t = Session::new();
    let r = ok(&mut t, json!({"cmd": "open", "path": path}));
    let _ = r;
    let ex = &t.model().unwrap().document().configurations[0].excluded_occurrences;
    assert_eq!(ex.len(), 1, "{ex:?}");
    // #22: an occurrence without any evaluated instance is an error.
    ok(
        s,
        json!({"cmd": "set_occurrence_suppressed", "occurrence": f.o_mod2, "suppressed": true}),
    );
    let e = refused(s, json!({"cmd": "interference", "occurrences": [f.o_mod2]}));
    assert!(e.contains("no evaluated instance"), "{e}");
    ok(s, json!({"cmd": "undo"}));
    // #21: an empty sub-assembly inside an empty sub-assembly is left out of
    // the hierarchical export instead of refusing it.
    let e1 = out(ok(s, json!({"cmd": "create_component", "name": "Empty1"})))
        .component
        .unwrap()
        .0;
    let e2 = out(ok(s, json!({"cmd": "create_component", "name": "Empty2"})))
        .component
        .unwrap()
        .0;
    ok(
        s,
        json!({"cmd": "add_occurrence", "component": e2, "owner": e1}),
    );
    let r = ok(
        s,
        json!({"cmd": "export_assembly", "path": dir.path().join("t.step"), "format": "step_assembly"}),
    );
    let msg = r.message.unwrap();
    assert!(msg.contains("empty occurrences left out"), "{msg}");
    // Surplus extra values are refused (cylindrical has 2 values).
    let e = refused(
        s,
        json!({"cmd": "add_joint", "kind": "cylindrical", "parent": f.o_mod1, "child": f.o_mod2,
               "extra": [{}, {}]}),
    );
    assert!(e.contains("at most 1 extra"), "{e}");
}

#[test]
fn align_to_a_joint_descendant_is_refused() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let (a, _) = block(&mut s, 10.0, 10.0, 10.0, [0.0, 0.0]);
    let (b, _) = block(&mut s, 4.0, 4.0, 2.0, [50.0, 0.0]);
    let oa = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "A", "bodies": [a]}),
    ))
    .occurrence
    .unwrap()
    .0;
    let ob = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "B", "bodies": [b]}),
    ))
    .occurrence
    .unwrap()
    .0;
    ok(
        &mut s,
        json!({"cmd": "add_joint", "kind": "slider", "parent": oa, "child": ob, "axis": [1.0, 0.0, 0.0]}),
    );
    let top = face_with_normal(&mut s, a, [0.0, 0.0, 1.0]);
    let bottom = face_with_normal(&mut s, b, [0.0, 0.0, -1.0]);
    let e = refused(
        &mut s,
        json!({"cmd": "align_occurrence", "occurrence": oa, "from": {"body": a, "face": top},
               "target": ob, "to": {"body": b, "face": bottom}}),
    );
    assert!(e.contains("would move with it"), "{e}");
}

/// Hole axes recovered from the B-rep faces (`measure_faces` classification of
/// Truck's rational cylinder surfaces) agree with the sketch circle centres to
/// ~1e-9 mm; the snap itself is exact algebra on those axes.
const AXIS_TOL: f64 = 1e-8;

/// Joint with a child-axis snap: the child's hole axis is brought onto the
/// parent's hole axis (closed form), then the revolute turns about it.
#[test]
fn joint_snaps_the_child_axis_onto_the_parent_axis() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    // Parent: 20 × 20 × 4 plate at the origin with a Ø4 hole at (5, 0).
    let (a, a_ext) = block(&mut s, 20.0, 20.0, 4.0, [0.0, 0.0]);
    let f = s.summary().features.len() as u32 + 1;
    ok(
        &mut s,
        json!({"cmd": "create_sketch",
               "plane": {"Face": {"feature": a_ext, "role": "ExtrudeEndCap"}},
               "template": "centered_circle", "diameter": 4.0, "center": [5.0, 0.0]}),
    );
    ok(&mut s, json!({"cmd": "cut_through_all", "sketch": f}));
    // Child: 10 × 10 × 2 link at (50, 30) with a Ø4 hole at (52, 30).
    let (b, b_ext) = block(&mut s, 10.0, 10.0, 2.0, [50.0, 30.0]);
    let f = s.summary().features.len() as u32 + 1;
    ok(
        &mut s,
        json!({"cmd": "create_sketch",
               "plane": {"Face": {"feature": b_ext, "role": "ExtrudeEndCap"}},
               "template": "centered_circle", "diameter": 4.0, "center": [52.0, 30.0]}),
    );
    ok(&mut s, json!({"cmd": "cut_through_all", "sketch": f}));
    let oa = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "A", "bodies": [a]}),
    ))
    .occurrence
    .unwrap()
    .0;
    let ob = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "B", "bodies": [b]}),
    ))
    .occurrence
    .unwrap()
    .0;
    let cyl = |s: &mut Session, body: u32| {
        let n = s
            .summary()
            .bodies
            .iter()
            .find(|x| x.id.0 == body)
            .unwrap()
            .measurements
            .as_ref()
            .unwrap()
            .face_count as u32;
        (0..n)
            .find(|&i| {
                tray::exec(
                    s,
                    &json!({"cmd": "measure_faces", "a": {"body": body, "face": i}}),
                )
                .face_measure
                .is_some_and(|r| matches!(r.a, rcad_kernel::DirectFace::Cylinder { .. }))
            })
            .unwrap()
    };
    let (ha, hb) = (cyl(&mut s, a), cyl(&mut s, b));
    let j = out(ok(
        &mut s,
        json!({"cmd": "add_joint", "kind": "revolute", "parent": oa, "child": ob,
               "axis_from": {"cylinder": {"face": {"body": a, "face": ha}}},
               "child_axis_from": {"cylinder": {"face": {"body": b, "face": hb}}}}),
    ))
    .joint
    .unwrap()
    .0;
    // The child hole axis (52, 30, z) now lies on x = 5, y = 0; the axial
    // (z) position is kept, the hole axes are parallel (no rotation needed,
    // or a half turn when the face axes point opposite ways).
    let p = instance_point(&s, &format!("O{ob}"), [52.0, 30.0, 1.0]);
    assert!(
        (p[0] - 5.0).abs() < AXIS_TOL && p[1].abs() < AXIS_TOL,
        "{p:?}"
    );
    // Turning the joint keeps the child's hole on the axis.
    ok(
        &mut s,
        json!({"cmd": "set_joint_value", "joint": j, "value": 37.0}),
    );
    let p = instance_point(&s, &format!("O{ob}"), [52.0, 30.0, 1.0]);
    assert!(
        (p[0] - 5.0).abs() < AXIS_TOL && p[1].abs() < AXIS_TOL,
        "{p:?}"
    );
    // One undo step removes the joint and restores the child placement.
    ok(&mut s, json!({"cmd": "undo"}));
    ok(&mut s, json!({"cmd": "undo"}));
    assert!(s.summary().assembly.joints.is_empty());
    let p = instance_point(&s, &format!("O{ob}"), [52.0, 30.0, 1.0]);
    assert!(near3(p, [52.0, 30.0, 1.0], 1e-12));
}

/// BOM CSV export (flattened and structured), RFC 4180 quoting.
#[test]
fn bom_csv_export() {
    let mut f = fixture();
    let s = &mut f.s;
    ok(
        s,
        json!({"cmd": "set_part_number", "component": f.bolt, "part_number": "BT-1, \"M3\""}),
    );
    let dir = tempfile::tempdir().unwrap();
    let (p1, p2) = (dir.path().join("flat.csv"), dir.path().join("tree.csv"));
    ok(s, json!({"cmd": "export_bom", "path": p1}));
    ok(
        s,
        json!({"cmd": "export_bom", "path": p2, "structured": true}),
    );
    let flat = std::fs::read_to_string(&p1).unwrap();
    let tree = std::fs::read_to_string(&p2).unwrap();
    let fl: Vec<&str> = flat.lines().collect();
    assert_eq!(fl[0], "item,name,part_number,quantity,material,occurrences");
    assert!(fl[1].starts_with("1,Plate,PL-1,2,"), "{}", fl[1]);
    assert!(
        fl[2].starts_with("2,Bolt,\"BT-1, \"\"M3\"\"\",4,"),
        "{}",
        fl[2]
    );
    let tl: Vec<&str> = tree.lines().collect();
    assert_eq!(
        tl[0],
        "item,level,name,part_number,quantity,assembly,material"
    );
    // Item numbers are quoted text ("1.10" is not 1.1).
    assert_eq!(tl[1], "\"1\",0,Module,MOD-1,2,true,—");
    assert!(tl[3].starts_with("\"1.2\",1,Bolt,"), "{}", tl[3]);
    assert_eq!(tl.len(), 4);
}
