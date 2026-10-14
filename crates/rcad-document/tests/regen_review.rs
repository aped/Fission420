//! Regeneration regressions in `regen.rs`. Expected volumes are derived by
//! hand from box arithmetic, not from the implementation's output.
use rcad_document::*;
use rcad_kernel::{Measurements, VolumeResult};
use rcad_sketch::Sketch;

/// Move a centred sketch (and its fixed anchor) to `(x, y)`.
fn anchored(mut s: Sketch, x: f64, y: f64) -> Sketch {
    for c in &mut s.constraints {
        if let rcad_sketch::Constraint::FixedPoint { x: cx, y: cy, .. } = c {
            *cx = x;
            *cy = y;
        }
    }
    for p in &mut s.points {
        p.x += x;
        p.y += y;
    }
    s
}

fn rect(w: f64, h: f64, x: f64, y: f64) -> Sketch {
    anchored(Sketch::centered_rectangle(w, h).unwrap(), x, y)
}

fn circ(d: f64, x: f64, y: f64) -> Sketch {
    anchored(Sketch::centered_circle(d).unwrap(), x, y)
}

fn sketch(d: &mut Document, plane: SketchPlane, s: Sketch) -> FeatureId {
    d.push(
        "S",
        FeatureKind::Sketch {
            plane,
            sketch: s,
            links: Vec::new(),
        },
    )
}

fn face(feature: FeatureId, role: FaceRole) -> SketchPlane {
    SketchPlane::Face(FaceRef { feature, role })
}

fn cut(d: &mut Document, s: FeatureId, dist: f64, target: BodyId) -> FeatureId {
    d.push(
        "C",
        FeatureKind::extrude(s, dist, Operation::Cut { target }),
    )
}

fn join(d: &mut Document, s: FeatureId, dist: f64, target: BodyId) -> FeatureId {
    d.push(
        "J",
        FeatureKind::extrude(s, dist, Operation::Join { target }),
    )
}

fn vol(m: &Measurements) -> f64 {
    match m.volume {
        VolumeResult::Value { volume, .. } => volume,
        ref v => panic!("{v:?}"),
    }
}

fn body_vol(e: &Evaluation) -> f64 {
    vol(e.bodies[0].measurements.as_ref().expect("measured"))
}

/// Volume tolerance for box-only models (exact planar arithmetic).
const VOL_TOL: f64 = 0.01;

// ----------------------------------------------------------- mesh re-import

fn tet_stl(s: f64) -> Vec<u8> {
    let v = [[0.0, 0.0, 0.0], [s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, s]];
    let tris = [[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]];
    let mut o = String::from("solid t\n");
    for t in tris {
        o += "facet normal 0 0 0\nouter loop\n";
        for i in t {
            o += &format!("vertex {} {} {}\n", v[i][0], v[i][1], v[i][2]);
        }
        o += "endloop\nendfacet\n";
    }
    o += "endsolid t\n";
    o.into_bytes()
}

fn import_stl(m: &mut Model, bytes: Vec<u8>) {
    m.transact("import", move |d| {
        let blob = d.add_blob("part.stl", &bytes);
        let body = d.add_body("part");
        let n = d.default_name("Import");
        Ok(d.push(
            n,
            FeatureKind::ImportMesh {
                blob,
                to_mm: 1.0,
                body,
            },
        ))
    })
    .unwrap();
}

fn max_x(e: &Evaluation) -> f64 {
    e.bodies[0]
        .geometry
        .mesh()
        .unwrap()
        .mesh
        .positions
        .iter()
        .fold(f64::NEG_INFINITY, |a, p| a.max(p[0]))
}

/// Undo an import and import a different file: feature, blob and body ids and
/// the name are reused, so only the blob contents tell the two apart.
#[test]
fn reimport_after_undo_shows_the_new_file() {
    let mut m = Model::new();
    import_stl(&mut m, tet_stl(10.0));
    assert_eq!(max_x(m.evaluation()), 10.0);
    m.undo().unwrap();
    import_stl(&mut m, tet_stl(50.0));
    assert_eq!(max_x(m.evaluation()), 50.0);
    // Undo/redo still hit the cache and show the right file.
    m.undo().unwrap();
    m.redo().unwrap();
    assert_eq!(max_x(m.evaluation()), 50.0);
}

/// Re-importing identical bytes is still served from the cache.
#[test]
fn reimport_of_identical_bytes_hits_the_cache() {
    let mut m = Model::new();
    import_stl(&mut m, tet_stl(10.0));
    m.undo().unwrap();
    let (hits, _) = m.cache_stats();
    import_stl(&mut m, tet_stl(10.0));
    assert_eq!(max_x(m.evaluation()), 10.0);
    assert!(m.cache_stats().0 > hits, "{:?}", m.cache_stats());
}

// ---------------------------------------------------- deferred measurements

/// 20x20x10 block (4000) with a 4x4x3 cut at x = -5 (-48 = 3952), then
/// `third`. Measurements are deferred and adopted after `third`; undoing it
/// must show the block-with-cut volume again, not the adopted one.
fn deferred_then_undo(third: impl FnOnce(&mut Document, FeatureId, FeatureId)) -> (f64, f64) {
    let mut m = Model::new();
    m.set_defer_measurements(true);
    let b = BodyId(1);
    let e1 = m
        .transact("a", |d| {
            let s1 = sketch(d, SketchPlane::WorldXY, rect(20.0, 20.0, 0.0, 0.0));
            Ok(d.push_extrude("E1", s1, 10.0))
        })
        .unwrap();
    let c = m
        .transact("b", |d| {
            let s2 = sketch(
                d,
                face(e1, FaceRole::ExtrudeEndCap),
                rect(4.0, 4.0, -5.0, 0.0),
            );
            Ok(cut(d, s2, 3.0, b))
        })
        .unwrap();
    m.transact("c", |d| {
        third(d, e1, c);
        Ok(())
    })
    .unwrap();
    assert!(m.measurements_pending());
    let meas = rcad_kernel::measure(m.evaluation().bodies[0].geometry.brep().unwrap()).unwrap();
    let after = vol(&meas);
    assert!(m.adopt_measurements(m.revision, vec![(b, meas)]));
    m.undo().unwrap();
    // The undone state was measured (or is pending); whatever is shown must be
    // its own volume.
    m.complete_measurements().unwrap();
    let shown = body_vol(m.evaluation());
    assert!((shown - 3952.0).abs() < VOL_TOL, "after undo: {shown}");
    (after, shown)
}

#[test]
fn deferred_measurement_after_pattern_is_not_reused_on_undo() {
    let (after, _) = deferred_then_undo(|d, _, c| {
        d.push(
            "P",
            FeatureKind::LinearPattern {
                seed: c,
                count: 2,
                spacing: 10.0,
                direction: [1.0, 0.0],
            },
        );
    });
    // Second 4x4x3 pocket at x = +5.
    assert!((after - 3904.0).abs() < VOL_TOL, "{after}");
}

#[test]
fn deferred_measurement_after_chamfer_is_not_reused_on_undo() {
    let (after, _) = deferred_then_undo(|d, e1, _| {
        d.push(
            "Ch",
            FeatureKind::Chamfer {
                a: FaceRef {
                    feature: e1,
                    role: FaceRole::ExtrudeEndCap,
                },
                b: FaceRef {
                    feature: e1,
                    role: FaceRole::ExtrudeSide { entity: 1 },
                },
                distance: 2.0,
            },
        );
    });
    // A 2 mm chamfer along a 20 mm edge removes 2*2/2*20 = 40.
    assert!((after - 3912.0).abs() < VOL_TOL, "{after}");
}

/// The adopted measurement is reused when the same state comes back (redo).
#[test]
fn adopted_measurement_is_reused_on_redo() {
    let mut m = Model::new();
    m.set_defer_measurements(true);
    let e1 = m
        .transact("a", |d| {
            let s1 = sketch(d, SketchPlane::WorldXY, rect(20.0, 20.0, 0.0, 0.0));
            Ok(d.push_extrude("E1", s1, 10.0))
        })
        .unwrap();
    m.transact("b", |d| {
        let s2 = sketch(
            d,
            face(e1, FaceRole::ExtrudeEndCap),
            rect(4.0, 4.0, -5.0, 0.0),
        );
        let c = cut(d, s2, 3.0, BodyId(1));
        Ok(d.push(
            "P",
            FeatureKind::LinearPattern {
                seed: c,
                count: 2,
                spacing: 10.0,
                direction: [1.0, 0.0],
            },
        ))
    })
    .unwrap();
    let meas = rcad_kernel::measure(m.evaluation().bodies[0].geometry.brep().unwrap()).unwrap();
    assert!(m.adopt_measurements(m.revision, vec![(BodyId(1), meas)]));
    m.undo().unwrap();
    m.redo().unwrap();
    assert!(!m.measurements_pending(), "adopted measurement not reused");
    assert!((body_vol(m.evaluation()) - 3904.0).abs() < VOL_TOL);
}

// ---------------------------------------------------- caps on partial faces

/// Truck 0.6 cannot cut with a cap coplanar to a partial face (a naive cut
/// removes `wrong` mm^3); the exact slab construction (ADR 0006) builds them,
/// checked by the result gate: the exact volume, and the material that must
/// stay (bounds along z) intact.
fn exact(r: Result<Evaluation>, want: f64, wrong: f64, z: (f64, f64)) {
    let e = r.unwrap_or_else(|e| panic!("expected the exact result {want}, got {e:?}"));
    let v = body_vol(&e);
    assert!(
        (v - want).abs() < VOL_TOL,
        "volume {v}, exact {want} (old wrong {wrong})"
    );
    let bb = &e.bodies[0].measurements.as_ref().unwrap().bounds;
    assert!(
        (bb.min[2] - z.0).abs() < 1e-9 && (bb.max[2] - z.1).abs() < 1e-9,
        "{bb:?}"
    );
}

/// A slot sketched on the top face running off the block's edge: the face
/// covers only part of the profile but nothing lies above the plane, so the
/// start margin is still used. Full-thickness blind depth: the bottom face also
/// covers only part of the profile, with nothing below, so the floor is still
/// extended. 20x10x10 (2000) - slot x 15..25 (5 inside), y 2..8, 10 deep (300).
#[test]
fn slot_running_off_the_edge_still_cuts_through() {
    let mut d = Document::new();
    let s1 = sketch(&mut d, SketchPlane::WorldXY, rect(20.0, 10.0, 10.0, 5.0));
    let e1 = d.push_extrude("E1", s1, 10.0);
    let s3 = sketch(
        &mut d,
        face(e1, FaceRole::ExtrudeEndCap),
        rect(10.0, 6.0, 20.0, 5.0),
    );
    cut(&mut d, s3, 10.0, BodyId(1));
    let v = body_vol(&regenerate(&d).unwrap());
    assert!((v - 1700.0).abs() < VOL_TOL, "{v}");
}

/// 20x10x10 base (2000) with a 9x8x10 step joined on top over x 10..19 (+720).
/// Returns the document, base extrusion and body.
fn stepped_base() -> (Document, FeatureId, BodyId) {
    let mut d = Document::new();
    let s1 = sketch(&mut d, SketchPlane::WorldXY, rect(20.0, 10.0, 10.0, 5.0));
    let e1 = d.push_extrude("E1", s1, 10.0);
    let b = BodyId(1);
    let s2 = sketch(
        &mut d,
        face(e1, FaceRole::ExtrudeEndCap),
        rect(9.0, 8.0, 14.5, 5.0),
    );
    join(&mut d, s2, 10.0, b);
    (d, e1, b)
}

/// A cut sketched on the lower face whose profile runs under the raised step
/// removes only the 10x6x3 pocket below the sketch plane.
#[test]
fn cut_from_face_does_not_gouge_material_above_the_plane() {
    let (mut d, e1, b) = stepped_base();
    let s3 = sketch(
        &mut d,
        face(e1, FaceRole::ExtrudeEndCap),
        rect(10.0, 6.0, 10.0, 5.0),
    );
    cut(&mut d, s3, 3.0, b);
    // 2720 - 10x6x3 = 2540; the raised step (to z = 20) is untouched.
    exact(regenerate(&d), 2540.0, 2495.0, (0.0, 20.0));
}

/// Normal case: a cut fully inside the open part of the face is unchanged.
#[test]
fn cut_from_face_inside_the_face_still_works() {
    let (mut d, e1, b) = stepped_base();
    let s3 = sketch(
        &mut d,
        face(e1, FaceRole::ExtrudeEndCap),
        rect(4.0, 6.0, 4.0, 5.0),
    );
    cut(&mut d, s3, 3.0, b);
    let v = body_vol(&regenerate(&d).unwrap());
    assert!((v - (2720.0 - 72.0)).abs() < VOL_TOL, "{v}");
}

/// An edge notch on the lower face (x -2..4, running off the
/// x = 0 edge) with the raised step elsewhere on the body. The air above the
/// notch is free, so the cut works: 2720 - 4x6x3 = 2648. The cut-probe slab in
/// air made Truck return an empty result, which was taken for material and
/// refused the cut.
#[test]
fn edge_notch_beside_a_raised_step_still_cuts() {
    let (mut d, e1, b) = stepped_base();
    let s3 = sketch(
        &mut d,
        face(e1, FaceRole::ExtrudeEndCap),
        rect(6.0, 6.0, 1.0, 5.0),
    );
    cut(&mut d, s3, 3.0, b);
    let v = body_vol(&regenerate(&d).expect("notch must cut, not be refused"));
    assert!((v - 2648.0).abs() < VOL_TOL, "{v}");
}

/// A Join on world XY coplanar with the block's bottom face
/// (outward normal -Z). Builds before the "Join goes away from the material"
/// rule needed `reverse: true` here and saved it; such files must still
/// regenerate, and `reverse` makes no difference. 20x20x10 + 4x4x3 = 4048,
/// boss below z = 0.
#[test]
fn join_on_a_face_ignores_reverse_so_old_files_still_load() {
    for reverse in [false, true] {
        let mut d = Document::new();
        let s1 = sketch(&mut d, SketchPlane::WorldXY, rect(20.0, 20.0, 0.0, 0.0));
        d.push_extrude("E1", s1, 10.0);
        let s2 = sketch(&mut d, SketchPlane::WorldXY, rect(4.0, 4.0, 0.0, 0.0));
        d.push(
            "J",
            FeatureKind::Extrude {
                sketch: s2,
                distance: 3.0,
                operation: Operation::Join { target: BodyId(1) },
                extent: Extent::Distance,
                reverse,
                region: None,
                options: Default::default(),
            },
        );
        let e = regenerate(&d).unwrap_or_else(|e| panic!("reverse={reverse}: {e}"));
        assert!((body_vol(&e) - 4048.0).abs() < VOL_TOL, "{}", body_vol(&e));
        let bb = &e.bodies[0].measurements.as_ref().unwrap().bounds;
        assert!(
            (bb.min[2] + 3.0).abs() < 1e-9 && (bb.max[2] - 10.0).abs() < 1e-9,
            "{bb:?}"
        );
    }
}

// --------------------------------------------------------- blind-cut floors

/// A blind cut whose floor lands on a bottom face that covers only part of the
/// profile (a boss continues below) must stop at the floor: 2000 + 9*8*5 boss
/// - 10*6*10 cut = 1760, with the boss intact.
#[test]
fn blind_cut_floor_on_a_partial_face_does_not_overcut() {
    let mut d = Document::new();
    let s1 = sketch(&mut d, SketchPlane::WorldXY, rect(20.0, 10.0, 10.0, 5.0));
    let e1 = d.push_extrude("E1", s1, 10.0);
    let b = BodyId(1);
    // Start-cap frame: local x = world X, local y = world -Y.
    let s2 = sketch(
        &mut d,
        face(e1, FaceRole::ExtrudeStartCap),
        rect(9.0, 8.0, 14.5, -5.0),
    );
    join(&mut d, s2, 5.0, b);
    let s3 = sketch(
        &mut d,
        face(e1, FaceRole::ExtrudeEndCap),
        rect(10.0, 6.0, 10.0, 5.0),
    );
    cut(&mut d, s3, 10.0, b);
    // 1760; the boss below the floor (to z = -5) is untouched.
    exact(regenerate(&d), 1760.0, 1719.6, (-5.0, 10.0));
}

/// Normal case: a full-thickness blind cut on a plain block still goes through
/// (the floor face covers the whole profile, so the floor is extended).
#[test]
fn blind_cut_through_full_thickness_still_works() {
    let mut d = Document::new();
    let s1 = sketch(&mut d, SketchPlane::WorldXY, rect(20.0, 10.0, 10.0, 5.0));
    let e1 = d.push_extrude("E1", s1, 10.0);
    let s3 = sketch(
        &mut d,
        face(e1, FaceRole::ExtrudeEndCap),
        rect(10.0, 6.0, 10.0, 5.0),
    );
    cut(&mut d, s3, 10.0, BodyId(1));
    let v = body_vol(&regenerate(&d).unwrap());
    assert!((v - 1400.0).abs() < VOL_TOL, "{v}");
}

/// Pattern copies re-derive the floor margin where they land: the seed is a
/// full-thickness pocket in open plate; the second copy lands over a boss
/// below the plate, where extending the floor would bite into the boss.
fn plate_with_boss_and_pattern(spacing: f64) -> Result<Evaluation> {
    let mut d = Document::new();
    // 40x10x10 plate, x 0..40, y 0..10.
    let s1 = sketch(&mut d, SketchPlane::WorldXY, rect(40.0, 10.0, 20.0, 5.0));
    let e1 = d.push_extrude("E1", s1, 10.0);
    let b = BodyId(1);
    // Boss under x 25..35, y 1..9, 5 deep (start-cap frame: y = -Y).
    let s2 = sketch(
        &mut d,
        face(e1, FaceRole::ExtrudeStartCap),
        rect(10.0, 8.0, 30.0, -5.0),
    );
    join(&mut d, s2, 5.0, b);
    // Seed pocket x 4..8, y 3..7, through the 10 mm thickness.
    let s3 = sketch(
        &mut d,
        face(e1, FaceRole::ExtrudeEndCap),
        rect(4.0, 4.0, 6.0, 5.0),
    );
    let c = cut(&mut d, s3, 10.0, b);
    d.push(
        "P",
        FeatureKind::LinearPattern {
            seed: c,
            count: 2,
            spacing,
            direction: [1.0, 0.0],
        },
    );
    regenerate(&d)
}

#[test]
fn pattern_copy_floor_is_rederived_where_it_lands() {
    // 4000 + 400 boss - 2 * 160.
    let v = body_vol(&plate_with_boss_and_pattern(10.0).unwrap());
    assert!((v - 4080.0).abs() < VOL_TOL, "{v}");
    // The copy at x 28..32 sits over the boss: exact (stop at z = 0) or refused.
    let v = body_vol(&plate_with_boss_and_pattern(24.0).expect("copy cuts"));
    assert!((v - 4080.0).abs() < VOL_TOL, "{v}");
}

// ------------------------------------------------------------- join overlap

/// A Join on a plate thinner than twice the overlap must not poke out of the
/// far side: 20x20x0.4 plate (160) + 4x4x5 boss (80).
#[test]
fn join_on_thin_plate_does_not_poke_through() {
    let mut d = Document::new();
    let s1 = sketch(&mut d, SketchPlane::WorldXY, rect(20.0, 20.0, 0.0, 0.0));
    let e1 = d.push_extrude("E1", s1, 0.4);
    let s2 = sketch(
        &mut d,
        face(e1, FaceRole::ExtrudeEndCap),
        rect(4.0, 4.0, 0.0, 0.0),
    );
    join(&mut d, s2, 5.0, BodyId(1));
    let e = regenerate(&d).unwrap();
    let m = e.bodies[0].measurements.as_ref().unwrap();
    assert!(m.bounds.min[2].abs() < 1e-9, "{:?}", m.bounds);
    assert!((m.bounds.max[2] - 5.4).abs() < 1e-9, "{:?}", m.bounds);
    assert!((vol(m) - 240.0).abs() < VOL_TOL, "{}", vol(m));
}

/// Same overlap problem over an internal cavity: a 0.3 mm skin left by a
/// pocket cut from the other side. The bounds do not change, so neither the
/// depth cap nor the bounds check sees it. 20x20x10 (4000) - 10x10x9.7 pocket
/// (970) + 4x4x5 boss (80) = 3110.
/// The overlap is sized from the clear material depth under the
/// profile (0.3 mm here), so the result is exact.
#[test]
fn join_over_thin_skin_does_not_poke_into_cavity() {
    let mut d = Document::new();
    let s1 = sketch(&mut d, SketchPlane::WorldXY, rect(20.0, 20.0, 0.0, 0.0));
    let e1 = d.push_extrude("E1", s1, 10.0);
    let s2 = sketch(
        &mut d,
        face(e1, FaceRole::ExtrudeStartCap),
        rect(10.0, 10.0, 0.0, 0.0),
    );
    cut(&mut d, s2, 9.7, BodyId(1));
    let s3 = sketch(
        &mut d,
        face(e1, FaceRole::ExtrudeEndCap),
        rect(4.0, 4.0, 0.0, 0.0),
    );
    join(&mut d, s3, 5.0, BodyId(1));
    let v = body_vol(&regenerate(&d).unwrap());
    assert!((v - 3110.0).abs() < VOL_TOL, "{v}");
}

// -------------------------------------------------------- circular patterns

fn circular(count: u32, angle_deg: f64) -> Result<Evaluation> {
    let mut d = Document::new();
    let s1 = sketch(&mut d, SketchPlane::WorldXY, rect(40.0, 40.0, 0.0, 0.0));
    let e1 = d.push_extrude("E1", s1, 5.0);
    let s2 = sketch(
        &mut d,
        face(e1, FaceRole::ExtrudeEndCap),
        circ(4.0, 10.0, 0.0),
    );
    let c = cut(&mut d, s2, 2.0, BodyId(1));
    d.push(
        "P",
        FeatureKind::CircularPattern {
            seed: c,
            count,
            angle: angle_deg.to_radians(),
            center: [0.0, 0.0],
        },
    );
    regenerate(&d)
}

/// Five instances 90° apart: the fifth lands on the seed.
#[test]
fn circular_pattern_full_turn_is_rejected() {
    match circular(5, 90.0) {
        Err(DocError::Validation(msg)) => assert!(msg.contains("full turn"), "{msg}"),
        other => panic!("expected a validation error, got {:?}", other.map(|_| ())),
    }
}

/// Four instances 90° apart fill the turn exactly once.
#[test]
fn circular_pattern_just_under_a_full_turn_works() {
    let v = body_vol(&circular(4, 90.0).unwrap());
    let exp = 40.0 * 40.0 * 5.0 - 4.0 * std::f64::consts::PI * 4.0 * 2.0;
    assert!((v - exp).abs() < 0.05, "{v} vs {exp}");
}
