//! Surfaces and open-shell repair through the command API
//! (docs/SURFACES.md). STEP fixtures were made by
//! FreeCAD (tools/fixtures/step/open_shells.py); expected values are closed
//! form: the open box is [0,40]×[0,30]×[0,20] without its top (V = 24000).
mod support;
use rcad_commands::Session;
use rcad_document::BodyId;
use rcad_kernel::VolumeResult;
use serde_json::json;
use std::f64::consts::PI;
use support::tray;

fn fixture(name: &str) -> String {
    format!(
        "{}/../rcad-kernel/tests/fixtures/step/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn body(s: &Session, id: u32) -> rcad_commands::BodySummary {
    s.summary()
        .bodies
        .into_iter()
        .find(|b| b.id == BodyId(id))
        .unwrap_or_else(|| panic!("body {id}"))
}

fn volume(s: &Session, id: u32) -> Option<f64> {
    match body(s, id).measurements.expect("measured").volume {
        VolumeResult::Value { volume, .. } => Some(volume),
        VolumeResult::Unavailable { .. } => None,
    }
}

fn import(s: &mut Session, name: &str) {
    tray::ok(s, &json!({"cmd": "new_document"}));
    tray::ok(s, &json!({"cmd": "import_surfaces", "path": fixture(name)}));
}

#[test]
fn open_box_stays_open_then_fills_to_a_solid() {
    let mut s = Session::new();
    import(&mut s, "open_box_mm.step");
    let b = body(&s, 1);
    let m = b.measurements.clone().unwrap();
    assert!(
        !m.all_shells_closed,
        "an open shell must not be reported closed"
    );
    assert_eq!(volume(&s, 1), None, "an open shell has no volume");
    // Solid-only operations refuse the surface body.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "shell_face", "face": {"feature": 1, "role": {"Imported": {"body": 1, "face": 0}}}, "thickness": 1.0}),
    );
    assert!(!r.ok);
    // A plain STEP import points at import_surfaces.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "import_step", "path": fixture("open_box_mm.step")}),
    );
    assert!(!r.ok);
    assert!(format!("{:?}", r.error).contains("import_surfaces"));
    // Analysis before repair.
    let r = tray::ok(&mut s, &json!({"cmd": "analyze_surface", "body": 1}));
    let a = r.surface_analysis.unwrap();
    assert!(!a.closed);
    assert_eq!(
        (a.free_edges.len(), a.gaps.len(), a.boundary_loops.len()),
        (4, 0, 1)
    );
    assert!(a.orientation_conflicts.is_empty());
    // Fill (new body; the source is kept, hidden).
    let r = tray::ok(&mut s, &json!({"cmd": "fill_surface", "body": 1}));
    assert!(r.message.unwrap().contains("closed solid"));
    assert!((volume(&s, 2).unwrap() - 24000.0).abs() < 1e-6);
    assert_eq!(body(&s, 2).kind, "brep");
    assert!(!body(&s, 1).visible);
    assert_eq!(volume(&s, 1), None);
    // Undo / redo / save / reopen.
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    assert_eq!(s.summary().bodies.len(), 1);
    assert!(body(&s, 1).visible);
    tray::ok(&mut s, &json!({"cmd": "redo"}));
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("box.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": p}));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": p}));
    assert!((volume(&t, 2).unwrap() - 24000.0).abs() < 1e-6);
    assert_eq!(volume(&t, 1), None);
    // The open source body exports as a surface model (never as a solid),
    // and reads back as the same open shell: 5 faces, one boundary loop of
    // 4 free edges.
    let open = dir.path().join("open.step");
    tray::ok(
        &mut t,
        &json!({"cmd": "export_step", "path": open, "bodies": [1]}),
    );
    let text = std::fs::read_to_string(&open).unwrap();
    assert!(text.contains("SHELL_BASED_SURFACE_MODEL") && text.contains("OPEN_SHELL"));
    assert!(!text.contains("MANIFOLD_SOLID_BREP"));
    assert!(text.contains("MANIFOLD_SURFACE_SHAPE_REPRESENTATION"));
    assert!(!text.contains("ADVANCED_BREP_SHAPE_REPRESENTATION"));
    let mut u = Session::new();
    tray::ok(&mut u, &json!({"cmd": "new_document"}));
    tray::ok(&mut u, &json!({"cmd": "import_surfaces", "path": open}));
    let a = tray::ok(&mut u, &json!({"cmd": "analyze_surface", "body": 1}))
        .surface_analysis
        .unwrap();
    assert!(!a.closed);
    assert_eq!((a.free_edges.len(), a.boundary_loops.len()), (4, 1));
    // Solids and surfaces in one file: refused with the reason.
    let r = tray::exec(
        &mut t,
        &json!({"cmd": "export_step", "path": dir.path().join("mixed.step"), "bodies": [1, 2]}),
    );
    assert!(!r.ok);
    assert!(
        format!("{:?}", r.error).contains("separately"),
        "{:?}",
        r.error
    );
    // The repaired solid exports as a STEP solid.
    let out = dir.path().join("box.step");
    tray::ok(
        &mut t,
        &json!({"cmd": "export_step", "path": out, "bodies": [2]}),
    );
    assert!(
        std::fs::read_to_string(&out)
            .unwrap()
            .contains("MANIFOLD_SOLID_BREP")
    );
}

#[test]
fn loose_faces_gap_report_stitch_fill() {
    let mut s = Session::new();
    import(&mut s, "loose_faces_mm.step");
    let a = tray::ok(
        &mut s,
        &json!({"cmd": "analyze_surface", "body": 1, "search": 1e-6}),
    )
    .surface_analysis
    .unwrap();
    assert_eq!((a.shells, a.free_edges.len(), a.gaps.len()), (5, 20, 8));
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "stitch_surface", "body": 1, "tolerance": 1e-6}),
    );
    assert!(r.message.unwrap().contains("open shell, 4 free edge(s)"));
    tray::ok(&mut s, &json!({"cmd": "fill_surface", "body": 2}));
    assert!((volume(&s, 3).unwrap() - 24000.0).abs() < 1e-6);
}

#[test]
fn gap_box_heals_within_the_displayed_tolerance_and_refuses_below_it() {
    let mut s = Session::new();
    import(&mut s, "gap_box_mm.step");
    let a = tray::ok(
        &mut s,
        &json!({"cmd": "analyze_surface", "body": 1, "search": 0.1}),
    )
    .surface_analysis
    .unwrap();
    let biggest = a.gaps.iter().map(|g| g.size).fold(0.0, f64::max);
    assert!((biggest - 0.02).abs() < 1e-9);
    // Stitch at 0.05 mm (shown in the notes), then fill.
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "stitch_surface", "body": 1, "tolerance": 0.05}),
    );
    let msg = r.message.unwrap();
    assert!(msg.contains("healing tolerance 0.05 mm"), "{msg}");
    tray::ok(
        &mut s,
        &json!({"cmd": "fill_surface", "body": 2, "tolerance": 0.05}),
    );
    let v = volume(&s, 3).unwrap();
    assert!((v - 24000.0).abs() <= 0.02 * 600.0 + 1e-6, "{v}");
    // Upstream edit: a tolerance below the gap leaves the stitch open and the
    // fill impossible; the edit is refused and the model is unchanged.
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "tolerance", "value": 0.01}),
    );
    assert!(!r.ok);
    assert!((volume(&s, 3).unwrap() - v).abs() < 1e-12);
}

#[test]
fn thicken_plate_and_half_tube_with_collapse_refusal() {
    let mut s = Session::new();
    import(&mut s, "plate_mm.step");
    tray::ok(
        &mut s,
        &json!({"cmd": "thicken_surface", "body": 1, "distance": 2.0}),
    );
    assert!((volume(&s, 2).unwrap() - 2000.0).abs() < 1e-6);
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "distance", "value": 3.0}),
    );
    assert!((volume(&s, 2).unwrap() - 3000.0).abs() < 1e-6);
    let mut t = Session::new();
    import(&mut t, "half_tube_mm.step");
    tray::ok(
        &mut t,
        &json!({"cmd": "thicken_surface", "body": 1, "distance": 2.0}),
    );
    let out = volume(&t, 2).unwrap();
    // FreeCAD's face normal points away from the axis: outward thicken.
    let want_out = 0.5 * PI * (144.0 - 100.0) * 30.0;
    assert!((out - want_out).abs() < 1e-6, "{out}");
    let b = body(&t, 2).measurements.unwrap().bounds;
    assert!((b.max[0] - 12.0).abs() < 1e-9 && (b.min[1] + 12.0).abs() < 1e-9);
    // Toward the axis by >= the radius: offset collapse, refused.
    let r = tray::exec(
        &mut t,
        &json!({"cmd": "thicken_surface", "body": 1, "distance": 10.0, "reverse": true}),
    );
    assert!(!r.ok);
    assert!(format!("{:?}", r.error).contains("collapses"));
    let r = tray::ok(
        &mut t,
        &json!({"cmd": "thicken_surface", "body": 1, "distance": 4.0, "reverse": true}),
    );
    assert!(r.ok);
    let inn = volume(&t, 3).unwrap();
    assert!(
        (inn - 0.5 * PI * (100.0 - 36.0) * 30.0).abs() < 1e-6,
        "{inn}"
    );
}

/// With one solid and one surface body, a cut without a target
/// goes to the solid (surface bodies are not default targets).
#[test]
fn surface_bodies_are_not_default_targets() {
    let mut s = Session::new();
    import(&mut s, "plate_mm.step");
    let r = s.execute(&rcad_commands::Command::CreateSketch {
        name: None,
        plane: rcad_document::SketchPlane::WorldXY,
        template: rcad_commands::SketchTemplate::CenteredRectangle {
            width: 10.0,
            height: 10.0,
            center: Some([100.0, 0.0]),
        },
    });
    assert!(r.ok);
    tray::ok(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 2, "distance": 10.0}),
    );
    let r = s.execute(&rcad_commands::Command::CreateSketch {
        name: None,
        plane: rcad_document::SketchPlane::WorldXY,
        template: rcad_commands::SketchTemplate::CenteredCircle {
            diameter: 4.0,
            center: Some([100.0, 0.0]),
        },
    });
    assert!(r.ok);
    tray::ok(
        &mut s,
        &json!({"cmd": "extrude", "sketch": 4, "op": "cut", "through_all": true}),
    );
    let v = volume(&s, 2).unwrap();
    assert!((v - (1000.0 - PI * 4.0 * 10.0)).abs() < 1e-6, "{v}");
}

/// With a component active, the flat-pattern display body joins
/// no component and a stitch/fill output replaces its source only when the
/// source is in a component (here: none, so they stay loose).
#[test]
fn derived_and_display_bodies_do_not_join_the_active_component() {
    let mut s = Session::new();
    import(&mut s, "open_box_mm.step");
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "create_component", "name": "Holder"}),
    );
    let comp = r.assembly.and_then(|a| a.component).expect("component id");
    tray::ok(
        &mut s,
        &json!({"cmd": "activate_component", "component": comp}),
    );
    tray::ok(&mut s, &json!({"cmd": "fill_surface", "body": 1}));
    let c = s
        .summary()
        .assembly
        .components
        .into_iter()
        .find(|c| c.id == comp)
        .unwrap();
    assert!(
        !c.bodies.contains(&BodyId(2)),
        "fill output joined the active component"
    );
    // A source inside a component is replaced by its derived body there.
    let mut t = Session::new();
    import(&mut t, "open_box_mm.step");
    let r = tray::ok(
        &mut t,
        &json!({"cmd": "create_component", "name": "Box", "bodies": [1]}),
    );
    let comp = r.assembly.and_then(|a| a.component).expect("component id");
    tray::ok(&mut t, &json!({"cmd": "fill_surface", "body": 1}));
    let c = t
        .summary()
        .assembly
        .components
        .into_iter()
        .find(|c| c.id == comp)
        .unwrap();
    assert_eq!(c.bodies, vec![BodyId(2)]);
}

fn sketch(s: &mut Session, sk: rcad_sketch::Sketch) -> u32 {
    let r = s.execute(&rcad_commands::Command::CreateSketch {
        name: None,
        plane: rcad_document::SketchPlane::WorldXY,
        template: rcad_commands::SketchTemplate::Custom { sketch: sk },
    });
    assert!(r.ok, "{:?}", r.error);
    r.feature.unwrap().0
}

/// Surfaces from sketches: a ruled (extruded) rectangle tube, filled into the
/// box; an upstream distance edit regenerates the fill; an open L chain makes
/// a non-planar boundary that fill refuses; a planar patch with a hole
/// thickens to the exact plate.
#[test]
fn surfaces_from_sketches() {
    let mut s = Session::new();
    tray::ok(&mut s, &json!({"cmd": "new_document"}));
    let r = s.execute(&rcad_commands::Command::CreateSketch {
        name: None,
        plane: rcad_document::SketchPlane::WorldXY,
        template: rcad_commands::SketchTemplate::CenteredRectangle {
            width: 40.0,
            height: 30.0,
            center: Some([20.0, 15.0]),
        },
    });
    assert!(r.ok);
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "surface_extrude", "sketch": 1, "distance": 20.0}),
    );
    let ext = r.feature.unwrap();
    assert_eq!(body(&s, 1).kind, "surface");
    let a = tray::ok(&mut s, &json!({"cmd": "analyze_surface", "body": 1}))
        .surface_analysis
        .unwrap();
    assert_eq!(
        (a.faces, a.free_edges.len(), a.boundary_loops.len()),
        (4, 8, 2)
    );
    tray::ok(&mut s, &json!({"cmd": "fill_surface", "body": 1}));
    assert!((volume(&s, 2).unwrap() - 24000.0).abs() < 1e-6);
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": ext, "name": "distance", "value": 10.0}),
    );
    assert!((volume(&s, 2).unwrap() - 12000.0).abs() < 1e-6);
    // Open L chain: a bent strip whose boundary is not planar.
    let mut sk = rcad_sketch::Sketch::default();
    let p: Vec<_> = [[100.0, 0.0], [130.0, 0.0], [130.0, 20.0]]
        .iter()
        .map(|q| sk.add_point(q[0], q[1]))
        .collect();
    sk.add_line(p[0], p[1]);
    sk.add_line(p[1], p[2]);
    let l = sketch(&mut s, sk);
    tray::ok(
        &mut s,
        &json!({"cmd": "surface_extrude", "sketch": l, "distance": 5.0}),
    );
    let lb = s.summary().bodies.last().unwrap().id.0;
    assert_eq!(body(&s, lb).kind, "surface");
    let r = tray::exec(&mut s, &json!({"cmd": "fill_surface", "body": lb}));
    assert!(!r.ok);
    assert!(format!("{:?}", r.error).contains("not planar"));
    // Planar patch 40 x 30 with a Ø10 hole, thickened 2 mm.
    let mut sk = rcad_sketch::Sketch::default();
    let p: Vec<_> = [[0.0, 100.0], [40.0, 100.0], [40.0, 130.0], [0.0, 130.0]]
        .iter()
        .map(|q| sk.add_point(q[0], q[1]))
        .collect();
    for i in 0..4 {
        sk.add_line(p[i], p[(i + 1) % 4]);
    }
    let c = sk.add_point(20.0, 115.0);
    sk.add_circle(c, 5.0);
    let ps = sketch(&mut s, sk);
    tray::ok(&mut s, &json!({"cmd": "surface_patch", "sketch": ps}));
    let pb = s.summary().bodies.last().unwrap().id.0;
    assert_eq!(volume(&s, pb), None);
    tray::ok(
        &mut s,
        &json!({"cmd": "thicken_surface", "body": pb, "distance": 2.0}),
    );
    let tb = s.summary().bodies.last().unwrap().id.0;
    let want = 2.0 * (1200.0 - PI * 25.0);
    assert!(
        (volume(&s, tb).unwrap() - want).abs() < 1e-6,
        "{:?}",
        volume(&s, tb)
    );
    // Save / reopen (format v7).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": path}));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": path}));
    assert!((volume(&t, tb).unwrap() - want).abs() < 1e-6);
    assert!((volume(&t, 2).unwrap() - 12000.0).abs() < 1e-6);
}

/// An open chain's ruled surface faces the same way
/// however its lines were drawn (entity order and line directions), so a
/// downstream thicken stays on its side; `reverse` moves the surface below
/// the sketch plane without flipping it. Convention (docs/SURFACES.md): the
/// chain runs from its end with the smaller x; the normal is to the right
/// of that direction (here −y for a chain along +x).
#[test]
fn ruled_surface_side_does_not_depend_on_how_lines_were_drawn() {
    let mut s = Session::new();
    tray::ok(&mut s, &json!({"cmd": "new_document"}));
    // Three drawings of the chain (0,0)-(10,0)-(20,0): forward, the lines
    // created in the other order, and both lines drawn backwards.
    let drawings: [&[([f64; 2], [f64; 2])]; 3] = [
        &[([0.0, 0.0], [10.0, 0.0]), ([10.0, 0.0], [20.0, 0.0])],
        &[([10.0, 0.0], [20.0, 0.0]), ([0.0, 0.0], [10.0, 0.0])],
        &[([20.0, 0.0], [10.0, 0.0]), ([10.0, 0.0], [0.0, 0.0])],
    ];
    let mut boxes = Vec::new();
    for (k, lines) in drawings.iter().enumerate() {
        for reverse in [false, true] {
            let mut sk = rcad_sketch::Sketch::default();
            for (a, b) in lines.iter() {
                let pa = sk.add_point(a[0], a[1]);
                let pb = sk.add_point(b[0], b[1]);
                sk.add_line(pa, pb);
            }
            let id = sketch(&mut s, sk);
            tray::ok(
                &mut s,
                &json!({"cmd": "surface_extrude", "sketch": id, "distance": 5.0,
                        "reverse": reverse}),
            );
            let sb = s.summary().bodies.last().unwrap().id.0;
            // Every face of the two-face strip faces −y (thicken takes one
            // face, so the single-line case below checks the solid's side).
            let m = s.model().unwrap();
            let brep = m
                .evaluation()
                .body_by_id(BodyId(sb))
                .unwrap()
                .geometry
                .brep()
                .unwrap()
                .clone();
            for f in rcad_kernel::classify_faces(&brep).unwrap() {
                let rcad_kernel::DirectFace::Plane { normal, .. } = f else {
                    panic!("{f:?}")
                };
                assert!(
                    (normal[1] + 1.0).abs() < 1e-12,
                    "drawing {k} reverse {reverse}: normal {normal:?}"
                );
            }
            boxes.push((k, reverse, body(&s, sb).measurements.unwrap().bounds));
        }
    }
    for (k, reverse, b) in &boxes {
        let z = if *reverse { [-5.0, 0.0] } else { [0.0, 5.0] };
        assert!(
            (b.min[2] - z[0]).abs() < 1e-9 && (b.max[2] - z[1]).abs() < 1e-9,
            "drawing {k}: {b:?}"
        );
    }
    // A single line drawn either way, thickened 2 mm: the solid is on −y.
    for (a, b) in [([0.0, 50.0], [10.0, 50.0]), ([10.0, 50.0], [0.0, 50.0])] {
        let mut sk = rcad_sketch::Sketch::default();
        let pa = sk.add_point(a[0], a[1]);
        let pb = sk.add_point(b[0], b[1]);
        sk.add_line(pa, pb);
        let id = sketch(&mut s, sk);
        tray::ok(
            &mut s,
            &json!({"cmd": "surface_extrude", "sketch": id, "distance": 5.0}),
        );
        let sb = s.summary().bodies.last().unwrap().id.0;
        tray::ok(
            &mut s,
            &json!({"cmd": "thicken_surface", "body": sb, "distance": 2.0}),
        );
        let tb = s.summary().bodies.last().unwrap().id.0;
        let bb = body(&s, tb).measurements.unwrap().bounds;
        assert!(
            (bb.min[1] - 48.0).abs() < 1e-9 && (bb.max[1] - 50.0).abs() < 1e-9,
            "{a:?}->{b:?}: {bb:?}"
        );
        assert!((volume(&s, tb).unwrap() - 100.0).abs() < 1e-9);
    }
}
