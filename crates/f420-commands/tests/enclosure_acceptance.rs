//! SESSION_4 §6.A / §6.B: the parametric enclosure and lid, baseline and
//! variant, checked against hand-derived geometry: volumes, bounds, point
//! probes (walls, floor, open cavity, lip, opening), exact wall/floor/clearance
//! distances from the regenerated face planes, connectivity; then undo/redo of
//! a parameter change, save/reopen, illegal edits, and a notch variant.
use f420_commands::printables::{
    ENCLOSURE_BASELINE as BASE, ENCLOSURE_VARIANT as VAR, Enclosure, enclosure_ids as ids,
    enclosure_workflow,
};
use f420_commands::{Command, ExtrudeOp, Response, Session, SketchTemplate};
use f420_document::{BodyId, DocError, FaceRef, FaceRole, FeatureId, SketchPlane};
use f420_kernel::VolumeResult;

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn build() -> (Session, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::new();
    for st in enclosure_workflow(&dir.path().join("enclosure.f420.json")) {
        let r = s.execute(&st.cmd);
        assert_eq!(r.ok, st.expect_ok, "{:?}: {:?}", st.cmd, r.error);
    }
    (s, dir)
}

fn set(s: &mut Session, p: &Enclosure) {
    for c in p.set_parameters() {
        ok(s, c);
    }
}

fn measurements(s: &Session, b: u32) -> f420_kernel::Measurements {
    s.summary()
        .bodies
        .into_iter()
        .find(|x| x.id == BodyId(b))
        .unwrap()
        .measurements
        .unwrap()
}

fn vol(s: &Session, b: u32) -> f64 {
    match measurements(s, b).volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn probe(s: &Session, b: u32, pts: &[[f64; 3]]) -> Vec<Option<bool>> {
    let m = s.model().unwrap();
    let body = m
        .evaluation()
        .body_by_id(BodyId(b))
        .unwrap()
        .geometry
        .brep()
        .unwrap();
    f420_kernel::point_membership(body, pts).unwrap()
}

/// Planes of a feature's side faces: (outward normal, signed offset n·origin).
fn side_planes(s: &Session, f: u32) -> Vec<([f64; 3], f64)> {
    let fs = s
        .summary()
        .features
        .into_iter()
        .find(|x| x.id == FeatureId(f))
        .unwrap();
    fs.faces
        .iter()
        .filter(|x| matches!(x.reference.role, FaceRole::ExtrudeSide { .. }))
        .map(|x| {
            let n = x.frame.z;
            let o = x.frame.origin;
            (n, n[0] * o[0] + n[1] * o[1] + n[2] * o[2])
        })
        .collect()
}

/// Distance between the two side planes with normals along `axis` (0 = x).
fn span(planes: &[([f64; 3], f64)], axis: usize) -> f64 {
    let v: Vec<f64> = planes
        .iter()
        .filter(|(n, _)| n[axis].abs() > 0.999)
        .map(|(n, d)| d * n[axis])
        .collect();
    assert_eq!(v.len(), 2, "{planes:?}");
    (v[0] - v[1]).abs()
}

const D: f64 = 0.01;

fn check(s: &Session, p: &Enclosure) {
    let (l, w, h, t, wall, fl) = (
        p.outer_length,
        p.outer_width,
        p.outer_height,
        p.lid_thickness,
        p.wall,
        p.floor,
    );
    let tol = 1e-6;
    // Volumes (analytic, SESSION_4 §6.B formulas).
    assert!(
        (vol(s, 1) - p.base_volume()).abs() < tol,
        "base {}",
        vol(s, 1)
    );
    assert!(
        (vol(s, 2) - p.lid_volume()).abs() < tol,
        "lid {}",
        vol(s, 2)
    );
    // Connected closed solids.
    for b in [1, 2] {
        let m = measurements(s, b);
        assert_eq!((m.shell_count, m.all_shells_closed), (1, true), "body {b}");
    }
    // Bounds.
    let mb = measurements(s, 1).bounds;
    for (got, want) in mb.min.iter().chain(&mb.max).zip([0.0, 0.0, 0.0, l, w, h]) {
        assert!((got - want).abs() < 1e-9, "base bounds {mb:?}");
    }
    let ly = -(w / 2.0 + p.lid_gap);
    let ml = measurements(s, 2).bounds;
    let want = [0.0, ly - w / 2.0, -p.lip_depth, l, ly + w / 2.0, t];
    for (got, want) in ml.min.iter().chain(&ml.max).zip(want) {
        assert!(
            (got - want).abs() < 1e-9,
            "lid bounds {ml:?}, want {want} got {got}"
        );
    }
    // Exact wall / floor / cavity sizes from the regenerated face planes.
    let cav = side_planes(s, ids::CAVITY);
    let (cl, cw) = p.cavity();
    assert!((span(&cav, 0) - cl).abs() < 1e-9);
    assert!((span(&cav, 1) - cw).abs() < 1e-9);
    for (n, d) in &cav {
        // Cavity walls face into the cavity; the wall is between the plane
        // and the matching outer face.
        let axis = if n[0].abs() > 0.5 { 0 } else { 1 };
        let x = d * n[axis];
        let outer = [l, w][axis];
        let thick = x.min(outer - x);
        assert!((thick - wall).abs() < 1e-9, "wall {thick} on axis {axis}");
    }
    let floor_z = s
        .summary()
        .features
        .into_iter()
        .find(|x| x.id == FeatureId(ids::CAVITY))
        .unwrap()
        .faces
        .iter()
        .find(|x| x.reference.role == FaceRole::ExtrudeEndCap)
        .map(|x| x.frame.origin[2])
        .unwrap();
    assert!((floor_z - fl).abs() < 1e-9, "floor at {floor_z}");
    // Lip: outer size and per-side clearance against the cavity.
    let lip = side_planes(s, ids::LIP);
    let ((ol, ow), (il, iw)) = p.lip();
    assert!((span(&lip, 0) - ol).abs() < 1e-9 && (span(&lip, 1) - ow).abs() < 1e-9);
    let cx = (span(&cav, 0) - span(&lip, 0)) / 2.0;
    let cy = (span(&cav, 1) - span(&lip, 1)) / 2.0;
    assert!((cx - p.fit_clearance).abs() < 1e-9 && (cy - p.fit_clearance).abs() < 1e-9);
    let opening = side_planes(s, ids::OPENING);
    assert!((span(&opening, 0) - il).abs() < 1e-9 && (span(&opening, 1) - iw).abs() < 1e-9);
    // Probes: base.
    let (mx, my, mz) = (l / 2.0, w / 2.0, h / 2.0);
    let base = probe(
        s,
        1,
        &[
            [mx, my, mz],           // cavity: empty
            [mx, my, h - D],        // open top: empty
            [mx, my, fl - D],       // floor: material
            [mx, my, fl + D],       // just above the floor: empty
            [wall - D, my, mz],     // -X wall
            [wall + D, my, mz],     // inside -X wall: empty
            [l - wall + D, my, mz], // +X wall
            [mx, wall - D, mz],     // -Y wall
            [mx, w - wall + D, mz], // +Y wall
            [mx, w - wall - D, mz], // inside +Y wall: empty
            [-D, my, mz],           // outside
        ],
    );
    let want = [
        false, false, true, false, true, false, true, true, true, false, false,
    ];
    assert_eq!(base, want.map(Some).to_vec(), "base probes");
    // Probes: lid (plate z [0, t], lip below). Lip outer at mx ± ol/2.
    let z_lip = -p.lip_depth / 2.0;
    let lid = probe(
        s,
        2,
        &[
            [mx, ly, t / 2.0],                           // plate
            [mx, ly, z_lip],                             // lip opening: empty
            [mx - ol / 2.0 + D, ly, z_lip],              // lip wall
            [mx - ol / 2.0 - D, ly, z_lip],              // outside the lip
            [mx - ol / 2.0 + wall - D, ly, z_lip],       // lip wall, inner side
            [mx - ol / 2.0 + wall + D, ly, z_lip],       // opening
            [mx, ly - ow / 2.0 + D, z_lip],              // lip wall (Y)
            [1.0, ly, -D],                               // under the plate's rim: empty
            [mx - ol / 2.0 + 1.0, ly, -p.lip_depth - D], // below the lip: empty
        ],
    );
    let want = [true, false, true, false, true, false, true, false, false];
    assert_eq!(lid, want.map(Some).to_vec(), "lid probes");
}

#[test]
fn baseline_then_variant_with_undo_redo_and_reopen() {
    let (mut s, dir) = build();
    check(&s, &BASE);
    let rev0 = s.summary().revision;
    // One parameter change is one undo step across the regenerated model.
    ok(
        &mut s,
        Command::SetUserParameter {
            name: "wall".into(),
            expression: "2.4".into(),
            unit: None,
            comment: None,
        },
    );
    assert_eq!(s.summary().revision, rev0 + 1);
    let mut w24 = BASE;
    w24.wall = 2.4;
    assert!((vol(&s, 1) - w24.base_volume()).abs() < 1e-6);
    assert!((vol(&s, 2) - w24.lid_volume()).abs() < 1e-6);
    ok(&mut s, Command::Undo);
    check(&s, &BASE);
    ok(&mut s, Command::Redo);
    assert!((vol(&s, 2) - w24.lid_volume()).abs() < 1e-6);
    set(&mut s, &VAR);
    check(&s, &VAR);
    // Save / reopen: identical expressions and geometry.
    let path = dir.path().join("variant.f420.json");
    ok(&mut s, Command::Save { path: path.clone() });
    let mut t = Session::new();
    ok(&mut t, Command::Open { path: path.clone() });
    assert_eq!(t.model().unwrap().document(), s.model().unwrap().document());
    assert_eq!(t.summary().parameters, s.summary().parameters);
    check(&t, &VAR);
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw["version"], 4);
    assert_eq!(
        raw["document"]["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "lip_length")
            .unwrap()["expression"],
        "cavity_length - 2 * fit_clearance"
    );
    // Back to the baseline by parameters only.
    set(&mut t, &BASE);
    check(&t, &BASE);
}

/// Illegal edits leave the last valid document and name the failing
/// parameter or feature slot.
#[test]
fn illegal_parameter_edits_are_refused_unchanged() {
    let (mut s, _dir) = build();
    let before = s.model().unwrap().document().clone();
    let rev = s.summary().revision;
    let try_set = |s: &mut Session, name: &str, expr: &str| -> DocError {
        let r = s.execute(&Command::SetUserParameter {
            name: name.into(),
            expression: expr.into(),
            unit: None,
            comment: None,
        });
        assert!(!r.ok, "{name} = {expr} was accepted");
        assert_eq!(s.model().unwrap().document(), &before, "{name} = {expr}");
        assert_eq!(s.summary().revision, rev);
        r.error.unwrap()
    };
    // Walls meeting in the middle: the cavity sketch would get a negative width.
    match try_set(&mut s, "wall", "45") {
        DocError::Expression { name, detail } => {
            assert!(name.contains("Cavity outline"), "{name}: {detail}");
        }
        e => panic!("{e:?}"),
    }
    // Floor as thick as the box: cavity depth 0.
    match try_set(&mut s, "floor", "25") {
        DocError::Expression { name, detail } => {
            assert!(name.contains("Cavity.distance"), "{name}: {detail}")
        }
        e => panic!("{e:?}"),
    }
    // A clearance that leaves no lip.
    match try_set(&mut s, "fit_clearance", "40") {
        DocError::Expression { name, .. } => assert!(name.contains("Lip"), "{name}"),
        e => panic!("{e:?}"),
    }
    // Lip wall consuming the whole lip: the opening would be negative.
    match try_set(&mut s, "wall", "13") {
        DocError::Expression { name, .. } => assert!(name.contains("outline"), "{name}"),
        e => panic!("{e:?}"),
    }
    // Cycle.
    match try_set(&mut s, "outer_length", "cavity_length + 2 * wall") {
        DocError::Expression { detail, .. } => {
            assert!(detail.contains("circular reference"), "{detail}");
            assert!(
                detail.contains("outer_length -> cavity_length -> outer_length"),
                "{detail}"
            );
        }
        e => panic!("{e:?}"),
    }
    // Division by zero.
    match try_set(&mut s, "wall", "outer_length / (floor - floor)") {
        DocError::Expression { name, detail } => {
            assert_eq!(name, "wall");
            assert!(detail.contains("division by zero"), "{detail}");
        }
        e => panic!("{e:?}"),
    }
    // Unit mismatch and unknown names.
    match try_set(&mut s, "wall", "outer_length * floor") {
        DocError::Expression { detail, .. } => assert!(detail.contains("mm^2"), "{detail}"),
        e => panic!("{e:?}"),
    }
    match try_set(&mut s, "wall", "wal + 1") {
        DocError::Expression { detail, .. } => {
            assert!(detail.contains("unknown parameter 'wal'"), "{detail}")
        }
        e => panic!("{e:?}"),
    }
    // A bound slot cannot be overwritten with a raw value behind its expression.
    let r = s.execute(&Command::SetParameter {
        feature: FeatureId(ids::BASE),
        name: "distance".into(),
        value: 40.0,
    });
    match r.error {
        Some(DocError::Validation(m)) => assert!(m.contains("bound to the expression"), "{m}"),
        e => panic!("{e:?}"),
    }
    // Delete of a used parameter is refused with the users named; rename
    // rewrites every dependent expression.
    let r = s.execute(&Command::DeleteUserParameter {
        name: "fit_clearance".into(),
    });
    match r.error {
        Some(DocError::Expression { detail, .. }) => {
            assert!(detail.contains("parameter lip_length"), "{detail}")
        }
        e => panic!("{e:?}"),
    }
    ok(
        &mut s,
        Command::RenameUserParameter {
            name: "fit_clearance".into(),
            new_name: "clearance".into(),
        },
    );
    let p = s.summary().parameters;
    let lip = p.iter().find(|x| x.name == "lip_length").unwrap();
    assert_eq!(lip.expression, "cavity_length - 2 * clearance");
    check(&s, &BASE);
}

/// Feature composition: a cable notch through the +X wall (sketched on the
/// base's side face, crossing the top edge), with its size and depth bound to
/// parameters; then a size change regenerates it.
#[test]
fn notch_variant_regenerates_with_size_changes() {
    let (mut s, _dir) = build();
    for (n, e) in [("notch_width", "10"), ("notch_depth", "8")] {
        ok(
            &mut s,
            Command::SetUserParameter {
                name: n.into(),
                expression: e.into(),
                unit: None,
                comment: None,
            },
        );
    }
    // +X side face of the base: entity 2 of the rectangle; local x = world Y
    // from y = 0, local y = world Z from z = 0, normal +X.
    let sk = ok(
        &mut s,
        Command::CreateSketch {
            name: Some("Notch outline".into()),
            plane: SketchPlane::Face(FaceRef {
                feature: FeatureId(ids::BASE),
                role: FaceRole::ExtrudeSide { entity: 2 },
            }),
            template: SketchTemplate::CenteredRectangle {
                width: 10.0,
                height: 16.0,
                center: Some([25.0, 25.0]),
            },
        },
    )
    .feature
    .unwrap();
    for (slot, e) in [
        ("width", "notch_width"),
        ("height", "2 * notch_depth"),
        ("center_x", "outer_width / 2"),
        ("center_y", "outer_height"),
    ] {
        ok(
            &mut s,
            Command::SetExpression {
                feature: sk,
                name: slot.into(),
                expression: e.into(),
            },
        );
    }
    let cut = ok(
        &mut s,
        Command::Extrude {
            name: Some("Notch".into()),
            sketch: sk,
            distance: 2.0,
            op: ExtrudeOp::Cut,
            target: Some(BodyId(1)),
            through_all: false,
            reverse: false,
            body_name: None,
        },
    )
    .feature
    .unwrap();
    ok(
        &mut s,
        Command::SetExpression {
            feature: cut,
            name: "distance".into(),
            expression: "wall".into(),
        },
    );
    let notch = |p: &Enclosure| 10.0 * 8.0 * p.wall;
    assert!(
        (vol(&s, 1) - (BASE.base_volume() - notch(&BASE))).abs() < 1e-6,
        "{}",
        vol(&s, 1)
    );
    // The notch opens the +X wall at the top centre and leaves the rest.
    let (l, w, h) = (BASE.outer_length, BASE.outer_width, BASE.outer_height);
    let p = probe(
        &s,
        1,
        &[
            [l - 1.0, w / 2.0, h - 4.0],       // inside the notch: empty
            [l - 1.0, w / 2.0, h - 9.0],       // below the notch: wall
            [l - 1.0, w / 2.0 + 6.0, h - 4.0], // beside the notch: wall
        ],
    );
    assert_eq!(p, vec![Some(false), Some(true), Some(true)]);
    set(&mut s, &VAR);
    assert!(
        (vol(&s, 1) - (VAR.base_volume() - notch(&VAR))).abs() < 1e-6,
        "{}",
        vol(&s, 1)
    );
    assert!((vol(&s, 2) - VAR.lid_volume()).abs() < 1e-6);
}
