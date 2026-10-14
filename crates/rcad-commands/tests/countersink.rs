//! Countersink feature through the command API: an exact cone
//! built into the topology at a Hole's entry (`rcad_kernel::countersink`). Plate
//! 60 x 40 x 10 with a Ø6 hole at (30, 20). Volumes are analytic: a countersink of
//! diameter 2R and included angle a on a hole of radius r removes the frustum of height
//! h = (R - r) / tan(a / 2) minus the cylinder it replaces.
use rcad_commands::{Command, Response, Session, SketchTemplate};
use rcad_document::{DocError, FaceRef, FaceRole, FeatureId, SketchPlane};
use rcad_kernel::VolumeResult;
use std::f64::consts::PI;
use std::path::PathBuf;

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn removed(r: f64, big: f64, deg: f64) -> f64 {
    let h = (big - r) / (deg.to_radians() / 2.0).tan();
    PI * h / 3.0 * (big * big + big * r + r * r) - PI * r * r * h
}

/// Volume (exact, or a labelled estimate containing `want` within its error).
fn assert_volume(s: &Session, want: f64) {
    let b = &s.summary().bodies[0];
    match b.measurements.as_ref().expect("measured").volume {
        VolumeResult::Value {
            volume,
            error_estimate,
            method,
        } => assert!(
            (volume - want).abs() <= error_estimate.max(1e-6 * want),
            "{method:?}: {volume} ± {error_estimate} vs {want}"
        ),
        ref v => panic!("{v:?}"),
    }
}

/// Plate and a hole (`depth` None = through); returns the hole feature.
fn plate_with_hole(s: &mut Session, depth: Option<f64>, cbore: Option<[f64; 2]>) -> FeatureId {
    ok(s, Command::NewDocument);
    let sk = ok(
        s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 60.0,
                height: 40.0,
                center: Some([30.0, 20.0]),
            },
        },
    )
    .feature
    .unwrap();
    let ext = ok(s, Command::extrude(None, sk, 10.0)).feature.unwrap();
    ok(
        s,
        Command::Hole {
            name: None,
            plane: SketchPlane::Face(FaceRef {
                feature: ext,
                role: FaceRole::ExtrudeEndCap,
            }),
            center: [30.0, 20.0],
            diameter: 6.0,
            depth,
            counterbore: cbore,
            target: None,
        },
    )
    .feature
    .unwrap()
}

fn csink(hole: FeatureId, diameter: f64, angle_deg: f64) -> Command {
    Command::Countersink {
        name: None,
        hole,
        diameter,
        angle_deg,
    }
}

#[test]
fn countersink_parameters_undo_and_reopen() {
    let mut s = Session::new();
    let hole = plate_with_hole(&mut s, None, None);
    let v_hole = 24000.0 - PI * 9.0 * 10.0;
    assert_volume(&s, v_hole);
    let cs = ok(&mut s, csink(hole, 12.0, 90.0)).feature.unwrap();
    assert_volume(&s, v_hole - removed(3.0, 6.0, 90.0));
    assert!((removed(3.0, 6.0, 90.0) - 36.0 * PI).abs() < 1e-9);
    // Parameter edits regenerate exactly.
    ok(
        &mut s,
        Command::SetParameter {
            feature: cs,
            name: "angle_deg".into(),
            value: 82.0,
        },
    );
    assert_volume(&s, v_hole - removed(3.0, 6.0, 82.0));
    // Upstream edit: a Ø8 hole keeps the Ø12 countersink.
    ok(
        &mut s,
        Command::SetParameter {
            feature: hole,
            name: "diameter".into(),
            value: 8.0,
        },
    );
    let v8 = 24000.0 - PI * 16.0 * 10.0 - removed(4.0, 6.0, 82.0);
    assert_volume(&s, v8);
    // A countersink no wider than the hole is refused; nothing changes.
    let r = s.execute(&Command::SetParameter {
        feature: hole,
        name: "diameter".into(),
        value: 12.0,
    });
    assert!(!r.ok);
    assert_volume(&s, v8);
    ok(&mut s, Command::Undo);
    assert_volume(&s, v_hole - removed(3.0, 6.0, 82.0));
    ok(&mut s, Command::Redo);
    assert_volume(&s, v8);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("countersink");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("csink.rcad.json");
    ok(&mut s, Command::Save { path: path.clone() });
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"version\": 6")
    );
    let mut s2 = Session::new();
    ok(&mut s2, Command::Open { path });
    assert_volume(&s2, v8);
    if let Ok(evidence) = std::env::var("RCAD_EVIDENCE_DIR") {
        std::fs::create_dir_all(&evidence).unwrap();
        ok(
            &mut s2,
            Command::ExportStep {
                path: PathBuf::from(evidence).join("csink_plate_d8_d12_82.step"),
                bodies: None,
            },
        );
    }
}

#[test]
fn countersink_refusals() {
    let mut s = Session::new();
    let hole = plate_with_hole(&mut s, None, None);
    // Ø42 at (30,20) leaves the 40 mm wide face.
    let r = s.execute(&csink(hole, 42.0, 90.0));
    assert!(
        matches!(&r.error, Some(DocError::ProductLimit(m)) if m.contains("does not fit")),
        "{:?}",
        r.error
    );
    // 90°, Ø30: 12 mm deep in a 10 mm plate.
    let r = s.execute(&csink(hole, 30.0, 90.0));
    assert!(
        matches!(&r.error, Some(DocError::ProductLimit(m)) if m.contains("end of the hole wall")),
        "{:?}",
        r.error
    );
    assert!(!s.execute(&csink(hole, 12.0, 180.0)).ok);
    assert!(!s.execute(&csink(hole, -1.0, 90.0)).ok);
    assert_eq!(s.summary().features.len(), 3, "nothing added");
    // Blind 2 mm hole: a 3 mm deep countersink is refused.
    let mut s = Session::new();
    let hole = plate_with_hole(&mut s, Some(2.0), None);
    assert!(!s.execute(&csink(hole, 12.0, 90.0)).ok);
    let r = ok(&mut s, csink(hole, 9.0, 90.0));
    assert!(r.feature.is_some(), "1.5 mm deep fits a 2 mm hole");
    assert_volume(&s, 24000.0 - PI * 9.0 * 2.0 - removed(3.0, 4.5, 90.0));
    // Counterbored hole: refused by name.
    let mut s = Session::new();
    let hole = plate_with_hole(&mut s, None, Some([10.0, 3.0]));
    let r = s.execute(&csink(hole, 12.0, 90.0));
    assert!(
        matches!(&r.error, Some(DocError::ProductLimit(m)) if m.contains("counterbored")),
        "{:?}",
        r.error
    );
}

/// A frame-shaped boss (outer 16 x 20, opening 8 x 12) joined to the top of
/// a plate whose countersink cone makes it non-prismatic: the exact slab
/// construction does not apply, so the holed tool goes through the general
/// boolean and the result check; the frame adds exactly (320 − 96) · 2. A
/// frame cut 1 mm deep removes (320 − 96) · 1.
#[test]
fn holed_region_join_and_cut_on_a_countersunk_plate() {
    let mut s = Session::new();
    let hole = plate_with_hole(&mut s, None, None);
    ok(&mut s, csink(hole, 12.0, 90.0));
    let base = 24000.0 - PI * 9.0 * 10.0 - removed(3.0, 6.0, 90.0);
    assert_volume(&s, base);
    let frame = |s: &mut Session| {
        let mut sk = rcad_sketch::Sketch::default();
        for (x0, y0, x1, y1) in [(40.0, 10.0, 56.0, 30.0), (44.0, 14.0, 52.0, 26.0)] {
            let p: Vec<_> = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
                .iter()
                .map(|q| sk.add_point(q[0], q[1]))
                .collect();
            for i in 0..4 {
                sk.add_line(p[i], p[(i + 1) % 4]);
            }
        }
        ok(
            s,
            Command::CreateSketch {
                name: None,
                plane: SketchPlane::Face(FaceRef {
                    feature: FeatureId(2),
                    role: FaceRole::ExtrudeEndCap,
                }),
                template: SketchTemplate::Custom { sketch: sk },
            },
        )
        .feature
        .unwrap()
    };
    let sk = frame(&mut s);
    let join = |sk, distance, op| Command::Extrude {
        name: None,
        sketch: sk,
        distance,
        op,
        target: Some(rcad_document::BodyId(1)),
        through_all: false,
        reverse: false,
        body_name: None,
        region: None,
        options: Default::default(),
    };
    ok(&mut s, join(sk, 2.0, rcad_commands::ExtrudeOp::Join));
    assert_volume(&s, base + 224.0 * 2.0);
    ok(&mut s, Command::Undo);
    let r = s.execute(&join(sk, 1.0, rcad_commands::ExtrudeOp::Cut));
    assert!(r.ok, "{:?}", r.error);
    assert_volume(&s, base - 224.0);
}
