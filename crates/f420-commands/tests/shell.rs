//! Scoped shell (SESSION_4 §7 item 4, docs/SHELL.md): a New Body extrusion of
//! one closed loop hollowed to walls + floor of `thickness`, open at one cap.
//! Analytic volumes, point probes of walls/floor/cavity, parameter edits,
//! start-cap opening, rounded profiles (offset area formula), refusals and
//! undo/redo, all through the public command path.
use f420_commands::{Command, ExtrudeOp, Response, Session, SketchTemplate};
use f420_document::{BodyId, DocError, FaceRole, FeatureId, SketchPlane};
use f420_kernel::VolumeResult;
use std::f64::consts::PI;

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn vol(s: &Session) -> f64 {
    match s.summary().bodies[0].measurements.clone().unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn probe(s: &Session, pts: &[[f64; 3]]) -> Vec<Option<bool>> {
    let m = s.model().unwrap();
    let body = m
        .evaluation()
        .body_by_id(BodyId(1))
        .unwrap()
        .geometry
        .brep()
        .unwrap();
    f420_kernel::point_membership(body, pts).unwrap()
}

fn sketch(s: &mut Session, t: SketchTemplate) -> FeatureId {
    ok(
        s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: t,
        },
    )
    .feature
    .unwrap()
}

fn rect(w: f64, h: f64) -> SketchTemplate {
    SketchTemplate::CenteredRectangle {
        width: w,
        height: h,
        center: None,
    }
}

fn shell(extrude: FeatureId, open: FaceRole, t: f64) -> Command {
    Command::Shell {
        name: None,
        extrude,
        open,
        thickness: t,
    }
}

/// Box w x h x d centred on the origin in XY, z = [0, d]; returns (sketch, extrude).
fn boxed(s: &mut Session, w: f64, h: f64, d: f64) -> (FeatureId, FeatureId) {
    ok(s, Command::NewDocument);
    let sk = sketch(s, rect(w, h));
    let e = ok(s, Command::extrude(None, sk, d)).feature.unwrap();
    (sk, e)
}

fn enclosure(l: f64, w: f64, h: f64, t: f64) -> f64 {
    l * w * h - (l - 2.0 * t) * (w - 2.0 * t) * (h - t)
}

const TOL: f64 = 1e-6;

#[test]
fn rectangle_shell_open_top_is_the_enclosure_base_and_follows_parameters() {
    let mut s = Session::new();
    let (sk, e) = boxed(&mut s, 80.0, 50.0, 25.0);
    let sh = ok(&mut s, shell(e, FaceRole::ExtrudeEndCap, 2.0))
        .feature
        .unwrap();
    assert!((vol(&s) - 19592.0).abs() < TOL, "{}", vol(&s));
    assert!((enclosure(80.0, 50.0, 25.0, 2.0) - 19592.0).abs() < 1e-9);
    // Walls (x = ±39, y = ±24 mid-height), floor, open cavity, open top.
    let p = probe(
        &s,
        &[
            [39.0, 0.0, 12.5],
            [-39.0, 0.0, 12.5],
            [0.0, 24.0, 12.5],
            [0.0, -24.0, 12.5],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 12.5],
            [37.5, 0.0, 12.5],
            [0.0, 0.0, 24.9],
            [39.0, 24.0, 24.9],
        ],
    );
    let want = [true, true, true, true, true, false, false, false, true];
    assert_eq!(p, want.map(Some).to_vec());
    let summary = s.summary();
    let f = summary.features.iter().find(|f| f.id == sh).unwrap();
    assert_eq!(f.kind, "shell");
    assert_eq!(f.parameters, vec![("thickness".to_string(), 2.0)]);
    // Parameter edits: the SESSION_4 B variant sizes.
    for (feature, name, value) in [
        (sk, "width", 100.0),
        (sk, "height", 65.0),
        (e, "distance", 30.0),
        (sh, "thickness", 2.4),
    ] {
        ok(
            &mut s,
            Command::SetParameter {
                feature,
                name: name.into(),
                value,
            },
        );
    }
    let want = enclosure(100.0, 65.0, 30.0, 2.4);
    assert!((vol(&s) - want).abs() < TOL, "{} vs {want}", vol(&s));
    let p = probe(
        &s,
        &[
            [48.8, 0.0, 15.0],
            [47.4, 0.0, 15.0],
            [0.0, 0.0, 2.3],
            [0.0, 0.0, 2.5],
        ],
    );
    assert_eq!(p, [true, false, true, false].map(Some).to_vec());
    // Undo / redo of the last edit and of the shell itself.
    ok(&mut s, Command::Undo);
    assert!((vol(&s) - enclosure(100.0, 65.0, 30.0, 2.0)).abs() < TOL);
    ok(&mut s, Command::Redo);
    assert!((vol(&s) - want).abs() < TOL);
}

#[test]
fn start_cap_opening_keeps_the_floor_at_the_far_end() {
    let mut s = Session::new();
    let (_, e) = boxed(&mut s, 40.0, 30.0, 10.0);
    ok(&mut s, shell(e, FaceRole::ExtrudeStartCap, 1.5));
    assert!((vol(&s) - enclosure(40.0, 30.0, 10.0, 1.5)).abs() < TOL);
    let p = probe(&s, &[[0.0, 0.0, 9.5], [0.0, 0.0, 0.5], [0.0, 0.0, 5.0]]);
    assert_eq!(p, [true, false, false].map(Some).to_vec());
}

#[test]
fn rounded_rectangle_shell_matches_the_offset_area_formula() {
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let (rr, _) = f420_commands::sketches::rounded_rectangle(60.0, 40.0, 6.0).unwrap();
    let sk = sketch(&mut s, SketchTemplate::Custom { sketch: rr });
    let e = ok(&mut s, Command::extrude(None, sk, 20.0))
        .feature
        .unwrap();
    ok(&mut s, shell(e, FaceRole::ExtrudeEndCap, 2.0));
    // Area of a w x h rectangle with corner radius r: w h − (4 − π) r²; the
    // inward offset by t has w − 2t, h − 2t, r − t.
    let area = |w: f64, h: f64, r: f64| w * h - (4.0 - PI) * r * r;
    let want = area(60.0, 40.0, 6.0) * 20.0 - area(56.0, 36.0, 4.0) * 18.0;
    let v = vol(&s);
    // Curved faces: the kernel's volume is exact or a bounded estimate.
    assert!((v - want).abs() < 1e-3, "{v} vs {want}");
    // Thickness past the corner radius: the inner corner arcs collapse.
    let r = s.execute(&Command::SetParameter {
        feature: FeatureId(3),
        name: "thickness".into(),
        value: 6.5,
    });
    assert!(
        matches!(&r.error, Some(DocError::ProductLimit(m)) if m.contains("does not fit")),
        "{r:?}"
    );
    assert!((vol(&s) - want).abs() < 1e-3, "unchanged");
}

#[test]
fn refusals_leave_the_document_unchanged() {
    let mut s = Session::new();
    let (_, e) = boxed(&mut s, 80.0, 50.0, 100.0);
    let rev = s.summary().revision;
    let check = |s: &mut Session, c: Command, text: &str| {
        let r = s.execute(&c);
        match &r.error {
            Some(DocError::ProductLimit(m)) => assert!(m.contains(text), "{m}"),
            other => panic!("expected ProductLimit '{text}', got {other:?}"),
        }
    };
    // Half the smallest width (25 mm) or more: the walls would meet.
    check(
        &mut s,
        shell(e, FaceRole::ExtrudeEndCap, 25.0),
        "does not fit",
    );
    assert_eq!(s.summary().revision, rev);
    // Thickness >= distance.
    let mut t = Session::new();
    let (_, e3) = boxed(&mut t, 80.0, 50.0, 3.0);
    check(
        &mut t,
        shell(e3, FaceRole::ExtrudeEndCap, 3.0),
        "less than the extrusion distance",
    );
    // Not a New Body extrusion: a cut.
    let hole = sketch(
        &mut s,
        SketchTemplate::CenteredCircle {
            diameter: 10.0,
            center: None,
        },
    );
    let cut = ok(
        &mut s,
        Command::Extrude {
            name: None,
            sketch: hole,
            distance: 5.0,
            op: ExtrudeOp::Cut,
            target: None,
            through_all: false,
            reverse: false,
            body_name: None,
            region: None,
        },
    )
    .feature
    .unwrap();
    let rev2 = s.summary().revision;
    check(
        &mut s,
        shell(cut, FaceRole::ExtrudeEndCap, 1.0),
        "not a New Body",
    );
    assert_eq!(s.summary().revision, rev2);
    // A profile with an inner loop (island).
    let mut u = Session::new();
    ok(&mut u, Command::NewDocument);
    let mut ring = f420_sketch::Sketch::default();
    for (w, h) in [(40.0, 30.0), (20.0, 10.0)] {
        let p: Vec<_> = [
            [-w / 2.0, -h / 2.0],
            [w / 2.0, -h / 2.0],
            [w / 2.0, h / 2.0],
            [-w / 2.0, h / 2.0],
        ]
        .iter()
        .map(|q: &[f64; 2]| ring.add_point(q[0], q[1]))
        .collect();
        for i in 0..4 {
            ring.add_line(p[i], p[(i + 1) % 4]);
        }
    }
    let sk = sketch(&mut u, SketchTemplate::Custom { sketch: ring });
    let e = ok(&mut u, Command::extrude(None, sk, 10.0))
        .feature
        .unwrap();
    let rev = u.summary().revision;
    check(
        &mut u,
        shell(e, FaceRole::ExtrudeEndCap, 1.0),
        "inner loops",
    );
    assert_eq!(u.summary().revision, rev);
}

#[test]
fn shell_command_json() {
    let c: Command = serde_json::from_str(r#"{"cmd":"shell","extrude":2,"thickness":2}"#).unwrap();
    assert!(matches!(
        c,
        Command::Shell {
            open: FaceRole::ExtrudeEndCap,
            ..
        }
    ));
    let c: Command = serde_json::from_str(
        r#"{"cmd":"shell","extrude":2,"open":"ExtrudeStartCap","thickness":1.5}"#,
    )
    .unwrap();
    assert!(matches!(
        c,
        Command::Shell {
            open: FaceRole::ExtrudeStartCap,
            ..
        }
    ));
}
