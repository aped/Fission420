//! Components, occurrences, joints, BOM, interference and assembly export
//! (run 6 pull-forward of Run-7 §1-2 and acceptance projects 1/2/4; values
//! closed-form).
mod support;
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

/// A box `w` × `h` × `d` with its base centred at `c` (z from 0); returns its body id.
fn block(s: &mut Session, w: f64, h: f64, d: f64, c: [f64; 2]) -> u32 {
    let f = s.summary().features.len() as u32 + 1;
    ok(
        s,
        json!({"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
               "width": w, "height": h, "center": c}),
    );
    ok(s, json!({"cmd": "extrude", "sketch": f, "distance": d}));
    s.summary().bodies.last().unwrap().id.0
}

fn world_point(s: &Session, occ: u32, p: [f64; 3]) -> [f64; 3] {
    let o = s
        .summary()
        .assembly
        .occurrences
        .into_iter()
        .find(|o| o.id.0 == occ)
        .expect("occurrence");
    let r = o.world.r;
    std::array::from_fn(|i| (0..3).map(|k| r[i][k] * p[k]).sum::<f64>() + o.world.t[i])
}

fn near3(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= tol)
}

#[test]
fn linked_occurrences_follow_the_definition_and_copies_do_not() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let b = block(&mut s, 20.0, 30.0, 40.0, [10.0, 15.0]);
    ok(
        &mut s,
        json!({"cmd": "set_body_material", "body": b, "density": 1.2}),
    );
    let o = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "Block", "bodies": [b], "part_number": "BLK-1"}),
    ));
    let (c, o1) = (o.component.unwrap(), o.occurrence.unwrap());
    let o2 = out(ok(
        &mut s,
        json!({"cmd": "add_occurrence", "component": c, "translate": [100.0, 0.0, 0.0]}),
    ))
    .occurrence
    .unwrap();
    let o3 = out(ok(
        &mut s,
        json!({"cmd": "add_occurrence", "component": c, "translate": [0.0, 100.0, 0.0],
               "angle_deg": 90.0}),
    ))
    .occurrence
    .unwrap();
    // An independent copy (a new body with its own history from here on).
    ok(
        &mut s,
        json!({"cmd": "move_body", "body": b, "translate": [0.0, -100.0, 0.0], "copy": true}),
    );
    let sum = s.summary().assembly;
    assert_eq!(sum.occurrences.len(), 3);
    assert_eq!(sum.occurrences[0].name, "Block#1");
    assert_eq!(sum.total_dof, 18);
    // Mass: 3 occurrences + the copy, 28.8 g each; centre of mass = mean of the four.
    let r = ok(&mut s, json!({"cmd": "mass_properties"}))
        .mass_properties
        .unwrap();
    assert_eq!(r.occurrences.len(), 3);
    assert_eq!(
        r.bodies.len(),
        1,
        "the copy is a loose body; Block is counted per occurrence"
    );
    let centres = [
        [10.0, 15.0, 20.0],
        [110.0, 15.0, 20.0],
        [-15.0, 110.0, 20.0],
        [10.0, -85.0, 20.0],
    ];
    for (oc, want) in r.occurrences.iter().zip(&centres) {
        assert!(near3(oc.center_of_mass.unwrap(), *want, 1e-9), "{oc:?}");
        assert!((oc.mass.unwrap() - 28.8).abs() < 1e-9);
    }
    let t = r.total.unwrap();
    assert!((t.mass - 4.0 * 28.8).abs() < 1e-9);
    let mean: [f64; 3] = std::array::from_fn(|i| centres.iter().map(|c| c[i]).sum::<f64>() / 4.0);
    assert!(
        near3(t.center_of_mass, mean, 1e-9),
        "{:?}",
        t.center_of_mass
    );
    // BOM: Block × 3 (BLK-1) and the loose copy × 1.
    let bom = out(ok(&mut s, json!({"cmd": "bom"}))).bom.unwrap();
    assert_eq!(bom.len(), 2);
    assert_eq!((bom[0].name.as_str(), bom[0].quantity), ("Block", 3));
    assert_eq!(bom[0].part_number, "BLK-1");
    assert_eq!(bom[1].quantity, 1);
    // Edit the definition after the copy: a 10 mm deep 10 × 10 pocket on the
    // top. All three occurrences change; the copy does not.
    let f = s.summary().features.len() as u32 + 1;
    ok(
        &mut s,
        json!({"cmd": "create_sketch", "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
               "template": "centered_rectangle", "width": 10.0, "height": 10.0,
               "center": [10.0, 15.0]}),
    );
    ok(
        &mut s,
        json!({"cmd": "extrude", "sketch": f, "distance": 10.0, "op": "cut", "target": b}),
    );
    let r = ok(&mut s, json!({"cmd": "mass_properties"}))
        .mass_properties
        .unwrap();
    for oc in &r.occurrences {
        assert!((oc.volume.unwrap() - 23_000.0).abs() < 1e-6, "{oc:?}");
    }
    let copy = r.bodies[0].properties.as_ref().unwrap();
    assert!((copy.volume - 24_000.0).abs() < 1e-6);
    // Undo the pocket, the sketch and the copy; then the third occurrence.
    for _ in 0..3 {
        ok(&mut s, json!({"cmd": "undo"}));
    }
    ok(&mut s, json!({"cmd": "undo"}));
    assert_eq!(s.summary().assembly.occurrences.len(), 2);
    ok(&mut s, json!({"cmd": "redo"}));
    // Save / reopen (format v6).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("asm.rcad.json");
    ok(&mut s, json!({"cmd": "save", "path": path}));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["version"], json!(6));
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "open", "path": path}));
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert_eq!(t.summary().assembly, s.summary().assembly);
    let _ = (o1, o2, o3);
}

#[test]
fn ownership_and_edit_refusals() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let b = block(&mut s, 20.0, 30.0, 40.0, [10.0, 15.0]);
    let o = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "Block", "bodies": [b]}),
    ));
    let o1 = o.occurrence.unwrap().0;
    let before = s.model().unwrap().document().clone();
    for bad in [
        json!({"cmd": "create_component", "name": "Again", "bodies": [b]}),
        json!({"cmd": "create_component", "name": "Block", "bodies": [b]}),
        // (Run 7: an empty component is allowed (active-component editing);
        // tests/nested_assembly.rs covers it.)
        json!({"cmd": "create_component", "name": "Ghost", "bodies": [9]}),
        json!({"cmd": "add_occurrence", "component": 7}),
        json!({"cmd": "set_occurrence_transform", "occurrence": 9, "translate": [1.0, 0.0, 0.0]}),
        // Deleting the definition body's creator is refused while a component uses it.
        json!({"cmd": "delete_feature", "feature": 2}),
    ] {
        let r = tray::exec(&mut s, &bad);
        assert!(!r.ok, "{bad}");
        assert_eq!(s.model().unwrap().document(), &before, "{bad}");
    }
    ok(
        &mut s,
        json!({"cmd": "set_grounded", "occurrence": o1, "grounded": true}),
    );
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "set_occurrence_transform", "occurrence": o1, "translate": [1.0, 0.0, 0.0]}),
    );
    assert!(!r.ok, "a grounded occurrence cannot be moved");
    assert_eq!(s.summary().assembly.total_dof, 0);
}

#[test]
fn interference_and_assembly_export() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let b = block(&mut s, 20.0, 30.0, 40.0, [10.0, 15.0]);
    let c = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "Block", "bodies": [b]}),
    ))
    .component
    .unwrap()
    .0;
    // #2 overlaps #1 by 10 × 30 × 40 (shifted 10 mm in X, 3 mm in Y and 5 mm in Z
    // so no faces coincide: overlap 10 × 27 × 35); #3 is far away.
    ok(
        &mut s,
        json!({"cmd": "add_occurrence", "component": c, "translate": [10.0, 3.0, 5.0]}),
    );
    ok(
        &mut s,
        json!({"cmd": "add_occurrence", "component": c, "translate": [200.0, 0.0, 0.0]}),
    );
    let i = out(ok(&mut s, json!({"cmd": "interference"})))
        .interference
        .unwrap();
    assert_eq!(i.len(), 1, "{i:?}");
    assert_eq!(
        (i[0].a_name.as_str(), i[0].b_name.as_str()),
        ("Block#1", "Block#2")
    );
    match &i[0].status {
        InterferenceStatus::Overlap { volume, exact, .. } => {
            assert!((volume - 10.0 * 27.0 * 35.0).abs() < 1e-6, "{volume}");
            assert!(*exact);
        }
        other => panic!("{other:?}"),
    }
    // Export: one STEP solid / 3MF object per occurrence, named Block#n.
    let dir = tempfile::tempdir().unwrap();
    let step = dir.path().join("asm.step");
    let r = ok(
        &mut s,
        json!({"cmd": "export_assembly", "path": step, "format": "step"}),
    );
    let msg = r.message.unwrap();
    assert!(msg.starts_with("3 objects"), "{msg}");
    let text = std::fs::read_to_string(&step).unwrap();
    for n in ["Block#1", "Block#2", "Block#3"] {
        assert!(text.contains(n), "{n} missing in STEP");
    }
    assert_eq!(text.matches("MANIFOLD_SOLID_BREP").count(), 3);
    let tmf = dir.path().join("asm.3mf");
    ok(
        &mut s,
        json!({"cmd": "export_assembly", "path": tmf, "format": "3mf"}),
    );
    let imp = rcad_exchange::threemf::read(
        &std::fs::read(&tmf).unwrap(),
        &rcad_exchange::threemf::ThreeMfLimits::default(),
    )
    .unwrap();
    assert_eq!(imp.objects.len(), 3);
    // Placed: the third object's x range is 200..220.
    let xs: Vec<f64> = imp.objects[2].positions.iter().map(|p| p[0]).collect();
    let (lo, hi) = (
        xs.iter().copied().fold(f64::INFINITY, f64::min),
        xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    );
    assert!(
        (lo - 200.0).abs() < 1e-6 && (hi - 220.0).abs() < 1e-6,
        "{lo} {hi}"
    );
}

/// Run-7 acceptance 2 (slider fixture): rail 200 × 20 × 10 grounded, carriage
/// 30 × 20 × 10 on top; slider along X with limits 0–40, then travel 60.
#[test]
fn slider_fixture_limits_travel_and_reopen() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let rail = block(&mut s, 200.0, 20.0, 10.0, [100.0, 0.0]);
    let car = block(&mut s, 30.0, 20.0, 10.0, [15.0, 0.0]);
    ok(
        &mut s,
        json!({"cmd": "move_body", "body": car, "translate": [0.0, 0.0, 10.0]}),
    );
    let (or, oc) = (
        out(ok(
            &mut s,
            json!({"cmd": "create_component", "name": "Rail", "bodies": [rail]}),
        ))
        .occurrence
        .unwrap()
        .0,
        out(ok(
            &mut s,
            json!({"cmd": "create_component", "name": "Carriage", "bodies": [car]}),
        ))
        .occurrence
        .unwrap()
        .0,
    );
    ok(
        &mut s,
        json!({"cmd": "set_grounded", "occurrence": or, "grounded": true}),
    );
    let j = out(ok(
        &mut s,
        json!({"cmd": "add_joint", "kind": "slider", "parent": or, "child": oc,
               "axis": [1.0, 0.0, 0.0], "value": 0.0, "min": 0.0, "max": 40.0}),
    ))
    .joint
    .unwrap()
    .0;
    assert_eq!(s.summary().assembly.total_dof, 1);
    // The carriage's corner (0, -10, 10) moves by the joint value along X.
    ok(
        &mut s,
        json!({"cmd": "set_joint_value", "joint": j, "value": 25.0}),
    );
    assert!(near3(
        world_point(&s, oc, [0.0, -10.0, 10.0]),
        [25.0, -10.0, 10.0],
        1e-12
    ));
    // The child cannot be moved directly.
    assert!(!tray::exec(
        &mut s,
        &json!({"cmd": "set_occurrence_transform", "occurrence": oc, "translate": [1.0, 0.0, 0.0]})
    )
    .ok);
    // Outside the limits: refused, unchanged.
    let before = s.model().unwrap().document().clone();
    for v in [-1.0, 50.0] {
        let r = tray::exec(
            &mut s,
            &json!({"cmd": "set_joint_value", "joint": j, "value": v}),
        );
        assert!(!r.ok, "{v}");
        assert_eq!(s.model().unwrap().document(), &before);
    }
    // Travel 60: 50 is now allowed.
    ok(
        &mut s,
        json!({"cmd": "set_joint_limits", "joint": j, "min": 0.0, "max": 60.0}),
    );
    ok(
        &mut s,
        json!({"cmd": "set_joint_value", "joint": j, "value": 50.0}),
    );
    assert!(near3(
        world_point(&s, oc, [0.0, -10.0, 10.0]),
        [50.0, -10.0, 10.0],
        1e-12
    ));
    // Limits that exclude the current value are refused.
    assert!(
        !tray::exec(
            &mut s,
            &json!({"cmd": "set_joint_limits", "joint": j, "min": 0.0, "max": 40.0})
        )
        .ok
    );
    // Reopen.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slider.rcad.json");
    ok(&mut s, json!({"cmd": "save", "path": path}));
    let mut t = Session::new();
    ok(&mut t, json!({"cmd": "open", "path": path}));
    assert!(near3(
        world_point(&t, oc, [0.0, -10.0, 10.0]),
        [50.0, -10.0, 10.0],
        1e-12
    ));
    let jt = t.summary().assembly.joints[0].clone();
    assert_eq!((jt.value, jt.min, jt.max), (50.0, Some(0.0), Some(60.0)));
    assert_eq!(t.summary().assembly.total_dof, 1);
    // A deliberately overlapping second carriage (sunk 5 mm into the rail):
    // the static interference check reports 30 × 20 × 5 minus nothing... the
    // overlap box is x 100..130, y -10..10 (rail and carriage share y faces, so
    // offset it 2 mm in y to avoid coincident faces): 30 × 18 × 5.
    let c2 = s.summary().assembly.components[1].id.0;
    let o3 = out(ok(
        &mut s,
        json!({"cmd": "add_occurrence", "component": c2, "name": "Intruder",
               "translate": [100.0, 2.0, -5.0]}),
    ))
    .occurrence
    .unwrap()
    .0;
    let i = out(ok(
        &mut s,
        json!({"cmd": "interference", "occurrences": [or, o3]}),
    ))
    .interference
    .unwrap();
    assert_eq!(i.len(), 1);
    match &i[0].status {
        InterferenceStatus::Overlap { volume, .. } => {
            assert!((volume - 30.0 * 18.0 * 5.0).abs() < 1e-6, "{volume}");
        }
        other => panic!("{other:?}"),
    }
}

/// Hinge-like revolute with limits, a loop refusal and joint deletion keeping
/// the pose.
#[test]
fn revolute_hinge_limits_and_loop_refusal() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let a = block(&mut s, 40.0, 30.0, 4.0, [-20.0, 0.0]);
    let b = block(&mut s, 40.0, 30.0, 4.0, [20.0, 0.0]);
    let oa = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "LeafA", "bodies": [a]}),
    ))
    .occurrence
    .unwrap()
    .0;
    let ob = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "LeafB", "bodies": [b]}),
    ))
    .occurrence
    .unwrap()
    .0;
    ok(
        &mut s,
        json!({"cmd": "set_grounded", "occurrence": oa, "grounded": true}),
    );
    let j = out(ok(
        &mut s,
        json!({"cmd": "add_joint", "kind": "revolute", "parent": oa, "child": ob,
               "origin": [0.0, 0.0, 0.0], "axis": [0.0, 1.0, 0.0], "min": -90.0, "max": 90.0}),
    ))
    .joint
    .unwrap()
    .0;
    ok(
        &mut s,
        json!({"cmd": "set_joint_value", "joint": j, "value": 90.0}),
    );
    // +90° about +Y: (40, 0, 0) → (0, 0, −40).
    assert!(near3(
        world_point(&s, ob, [40.0, 0.0, 0.0]),
        [0.0, 0.0, -40.0],
        1e-12
    ));
    assert!(
        !tray::exec(
            &mut s,
            &json!({"cmd": "set_joint_value", "joint": j, "value": 120.0})
        )
        .ok
    );
    // A grounded child, a second parent and a loop are refused.
    for bad in [
        json!({"cmd": "add_joint", "kind": "rigid", "parent": ob, "child": oa}),
        json!({"cmd": "add_joint", "kind": "rigid", "parent": oa, "child": ob}),
    ] {
        assert!(!tray::exec(&mut s, &bad).ok, "{bad}");
    }
    let b3 = block(&mut s, 5.0, 5.0, 5.0, [100.0, 0.0]);
    let oc = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "Pin", "bodies": [b3]}),
    ))
    .occurrence
    .unwrap()
    .0;
    ok(
        &mut s,
        json!({"cmd": "add_joint", "kind": "rigid", "parent": ob, "child": oc}),
    );
    // ob → oc exists; oc as the parent of ob would close a loop (ob already has a
    // parent too), and an ungrounded free root jointed to its own descendant:
    ok(
        &mut s,
        json!({"cmd": "set_grounded", "occurrence": oa, "grounded": false}),
    );
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "add_joint", "kind": "rigid", "parent": oc, "child": oa}),
    );
    assert!(!r.ok, "loop must be refused");
    assert!(format!("{:?}", r.error).contains("loop"), "{:?}", r.error);
    // DOF: oa free 6, ob revolute 1, oc rigid 0.
    assert_eq!(s.summary().assembly.total_dof, 7);
    // Deleting the joint keeps ob where it is.
    let p = world_point(&s, ob, [40.0, 0.0, 0.0]);
    ok(&mut s, json!({"cmd": "delete_joint", "joint": j}));
    assert!(near3(world_point(&s, ob, [40.0, 0.0, 0.0]), p, 1e-9));
}

/// Face index (current evaluation) of body `b` matching `pick`.
fn face_of(s: &mut Session, b: u32, pick: impl Fn(&rcad_kernel::DirectFace) -> bool) -> u32 {
    let n = s
        .summary()
        .bodies
        .iter()
        .find(|x| x.id.0 == b)
        .unwrap()
        .measurements
        .as_ref()
        .unwrap()
        .face_count as u32;
    (0..n)
        .find(|&i| {
            tray::exec(
                s,
                &json!({"cmd": "measure_faces", "a": {"body": b, "face": i}}),
            )
            .face_measure
            .is_some_and(|r| pick(&r.a))
        })
        .expect("face")
}

/// Revolute axes taken from the parent's faces: the edge line of two planar
/// faces, and a hole's cylinder axis.
#[test]
fn joint_axis_from_an_edge_and_from_a_hole() {
    use rcad_kernel::DirectFace;
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let a = block(&mut s, 40.0, 30.0, 4.0, [-20.0, 0.0]);
    let b = block(&mut s, 40.0, 30.0, 4.0, [20.0, 0.0]);
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
    let plane = |n: [f64; 3]| move |f: &DirectFace| matches!(f, DirectFace::Plane { normal, .. } if near3(*normal, n, 1e-9));
    let side = face_of(&mut s, a, plane([1.0, 0.0, 0.0]));
    let top = face_of(&mut s, a, plane([0.0, 0.0, 1.0]));
    let j = out(ok(
        &mut s,
        json!({"cmd": "add_joint", "kind": "revolute", "parent": oa, "child": ob,
               "axis_from": {"edge": {"a": {"body": a, "face": side}, "b": {"body": a, "face": top}}}}),
    ))
    .joint
    .unwrap()
    .0;
    let jt = s.summary().assembly.joints[0].clone();
    // Edge line x = 0, z = 4 along −Y (n_side × n_top = X × Z = −Y).
    assert!(near3(jt.axis, [0.0, -1.0, 0.0], 1e-12), "{:?}", jt.axis);
    assert!(jt.origin[0].abs() < 1e-12 && (jt.origin[2] - 4.0).abs() < 1e-12);
    ok(
        &mut s,
        json!({"cmd": "set_joint_value", "joint": j, "value": 90.0}),
    );
    // +90° about −Y through (0, ·, 4): (40, 0, 4) → (0, 0, 44).
    assert!(near3(
        world_point(&s, ob, [40.0, 0.0, 4.0]),
        [0.0, 0.0, 44.0],
        1e-9
    ));
    // A face of another component is refused.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "add_joint", "kind": "revolute", "parent": oa, "child": ob,
                "axis_from": {"cylinder": {"face": {"body": b, "face": 0}}}}),
    );
    assert!(!r.ok);
    ok(&mut s, json!({"cmd": "delete_joint", "joint": j}));
    ok(
        &mut s,
        json!({"cmd": "set_occurrence_transform", "occurrence": ob}),
    );
    // A Ø6 hole through A at (−20, 0): revolute about its axis.
    let f = s.summary().features.len() as u32 + 1;
    ok(
        &mut s,
        json!({"cmd": "create_sketch", "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
               "template": "centered_circle", "diameter": 6.0, "center": [-20.0, 0.0]}),
    );
    ok(&mut s, json!({"cmd": "cut_through_all", "sketch": f}));
    let hole = face_of(&mut s, a, |f| matches!(f, DirectFace::Cylinder { .. }));
    let j = out(ok(
        &mut s,
        json!({"cmd": "add_joint", "kind": "revolute", "parent": oa, "child": ob,
               "axis_from": {"cylinder": {"face": {"body": a, "face": hole}}}}),
    ))
    .joint
    .unwrap()
    .0;
    ok(
        &mut s,
        json!({"cmd": "set_joint_value", "joint": j, "value": 90.0}),
    );
    // (40, 0, 0) is 60 from the axis (−20, 0): a quarter turn puts it at (−20, ±60, 0).
    let p = world_point(&s, ob, [40.0, 0.0, 0.0]);
    assert!(
        near3(p, [-20.0, 60.0, 0.0], 1e-9) || near3(p, [-20.0, -60.0, 0.0], 1e-9),
        "{p:?}"
    );
}

/// Run-7 §3 pull-forward: a configuration excludes a component occurrence;
/// BOM quantity and assembly mass follow the active configuration.
#[test]
fn configurations_include_or_exclude_occurrences() {
    let mut s = Session::new();
    ok(&mut s, json!({"cmd": "new_document"}));
    let b = block(&mut s, 20.0, 30.0, 40.0, [10.0, 15.0]);
    ok(
        &mut s,
        json!({"cmd": "set_body_material", "body": b, "density": 1.2}),
    );
    let o = out(ok(
        &mut s,
        json!({"cmd": "create_component", "name": "Block", "bodies": [b]}),
    ));
    let c = o.component.unwrap();
    let o2 = out(ok(
        &mut s,
        json!({"cmd": "add_occurrence", "component": c, "translate": [100.0, 0.0, 0.0]}),
    ))
    .occurrence
    .unwrap();
    ok(
        &mut s,
        json!({"cmd": "set_configuration", "name": "Pair", "part_number": "P-2"}),
    );
    ok(
        &mut s,
        json!({"cmd": "set_configuration", "name": "Single", "part_number": "P-1",
                      "excluded_occurrences": [o2]}),
    );
    let qty = |s: &mut Session| out(ok(s, json!({"cmd": "bom"}))).bom.unwrap()[0].quantity;
    ok(
        &mut s,
        json!({"cmd": "activate_configuration", "name": "Single"}),
    );
    assert_eq!(qty(&mut s), 1);
    ok(
        &mut s,
        json!({"cmd": "activate_configuration", "name": "Pair"}),
    );
    assert_eq!(qty(&mut s), 2);
    // Direct suppression, one undo step.
    ok(
        &mut s,
        json!({"cmd": "set_occurrence_suppressed", "occurrence": o2, "suppressed": true}),
    );
    assert_eq!(qty(&mut s), 1);
    ok(&mut s, json!({"cmd": "undo"}));
    assert_eq!(qty(&mut s), 2);
    // Unknown occurrence in a configuration: refused.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "set_configuration", "name": "Bad",
                                       "excluded_occurrences": [999]}),
    );
    assert!(!r.ok);
}
