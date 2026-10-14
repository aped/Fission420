//! Cuts the exact slab construction with arcs builds through the command path
//! (ADR 0007): the canonical engulfed-hole cut, an off-centre
//! partial overlap, round cuts sharing a floor, rounded pockets, holed tools,
//! and the transactional refusals that remain. Expected values are analytic;
//! occupancy probes use the result's own winding number (independent of the
//! construction); every result passed the ADR 0005 result check on commit.
//! With `RCAD_EVIDENCE_DIR` set, the results are also written as STEP there
//! for the external FreeCAD check (tools/check_slab_arcs.sh).
use rcad_commands::{Command, ExtrudeOp, Session, SketchTemplate};
use rcad_document::{BodyId, FaceRef, FaceRole, FeatureId, SketchPlane};
use rcad_kernel::VolumeResult;
use rcad_sketch::{Constraint, Dimension, EntityId, PointId, Sketch};
use std::f64::consts::PI;

fn ok(s: &mut Session, c: Command) -> FeatureId {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r.feature.unwrap_or(FeatureId(0))
}

fn sketch(s: &mut Session, plane: SketchPlane, template: SketchTemplate) -> FeatureId {
    ok(
        s,
        Command::CreateSketch {
            name: None,
            plane,
            template,
        },
    )
}

fn rect(w: f64, h: f64, c: [f64; 2]) -> SketchTemplate {
    SketchTemplate::CenteredRectangle {
        width: w,
        height: h,
        center: Some(c),
    }
}

fn circle(d: f64, c: [f64; 2]) -> SketchTemplate {
    SketchTemplate::CenteredCircle {
        diameter: d,
        center: Some(c),
    }
}

fn cut(sketch: FeatureId, d: f64, through: bool) -> Command {
    Command::Extrude {
        name: None,
        sketch,
        distance: d,
        op: ExtrudeOp::Cut,
        target: Some(BodyId(1)),
        through_all: through,
        reverse: false,
        body_name: None,
        region: None,
        options: Default::default(),
    }
}

fn top(e: FeatureId) -> SketchPlane {
    SketchPlane::Face(FaceRef {
        feature: e,
        role: FaceRole::ExtrudeEndCap,
    })
}

fn volume(s: &Session) -> f64 {
    let m = s.summary().bodies[0].measurements.clone().unwrap();
    assert_eq!((m.shell_count, m.all_shells_closed), (1, true));
    match m.volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn probe(s: &Session, pts: &[([f64; 3], bool)]) {
    let model = s.model().unwrap();
    let body = model
        .evaluation()
        .body_by_id(BodyId(1))
        .unwrap()
        .geometry
        .brep()
        .unwrap();
    let got =
        rcad_kernel::point_membership(body, &pts.iter().map(|p| p.0).collect::<Vec<_>>()).unwrap();
    for (p, g) in pts.iter().zip(got) {
        assert_eq!(g, Some(p.1), "probe {:?}", p.0);
    }
}

/// STEP export (always) and a copy for the external check (when requested).
fn export(s: &mut Session, name: &str) {
    let dir = std::env::var_os("RCAD_EVIDENCE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.step"));
    let r = s.execute(&Command::ExportStep {
        path: path.clone(),
        bodies: None,
    });
    assert!(r.ok, "{:?}", r.error);
    assert!(std::fs::metadata(&path).unwrap().len() > 1000);
}

/// 80 x 50 x 10 plate (corner at the origin) with a centred Ø10 through hole.
fn holed_plate(s: &mut Session, hole: bool) -> FeatureId {
    ok(s, Command::NewDocument);
    let sk = sketch(s, SketchPlane::WorldXY, rect(80.0, 50.0, [40.0, 25.0]));
    let e = ok(s, Command::extrude(None, sk, 10.0));
    if hole {
        let h = sketch(s, top(e), circle(10.0, [40.0, 25.0]));
        ok(s, cut(h, 0.0, true));
        assert!((volume(s) - (40000.0 - 250.0 * PI)).abs() < 1e-6 * 40000.0);
    }
    e
}

/// Area of the union of two discs (radii `a`, `b`, centres `d` apart).
fn disc_union(a: f64, b: f64, d: f64) -> f64 {
    if d >= a + b {
        return PI * (a * a + b * b);
    }
    if d <= (a - b).abs() {
        return PI * a.max(b).powi(2);
    }
    let lens = a * a * ((d * d + a * a - b * b) / (2.0 * d * a)).acos()
        + b * b * ((d * d + b * b - a * a) / (2.0 * d * b)).acos()
        - 0.5 * ((-d + a + b) * (d + a - b) * (d - a + b) * (d + a + b)).sqrt();
    PI * (a * a + b * b) - lens
}

/// Material probes around (40, 25): empty within `r_empty`, material just
/// beyond `r_mat`, at 8 angles and 3 heights.
fn ring_probes(r_empty: f64, r_mat: f64) -> Vec<([f64; 3], bool)> {
    let mut v = Vec::new();
    for k in 0..8 {
        let t = 0.3 + k as f64 * PI / 4.0;
        for z in [0.4, 5.0, 9.6] {
            for (r, m) in [(r_empty, false), (r_mat, true)] {
                v.push(([40.0 + r * t.cos(), 25.0 + r * t.sin(), z], m));
            }
        }
    }
    v
}

#[test]
fn canonical_engulfed_hole_cut_is_exact_and_regenerates() {
    let mut s = Session::new();
    let e = holed_plate(&mut s, true);
    let big = sketch(&mut s, top(e), circle(16.0, [40.0, 25.0]));
    let rev = s.summary().revision;
    let c = ok(&mut s, cut(big, 0.0, true));
    assert_eq!(s.summary().revision, rev + 1);
    let want = 40000.0 - 640.0 * PI;
    assert!((volume(&s) - want).abs() < 1e-6 * want, "{}", volume(&s));
    let b = s.summary().bodies[0].measurements.clone().unwrap();
    assert!((b.bounds.max[2] - 10.0).abs() < 1e-9 && b.bounds.min[2].abs() < 1e-9);
    // No vestigial Ø10 wall: material only beyond r = 8 (also between the
    // old r = 5 wall and the new one: empty). 6 plate faces + the Ø16 wall:
    // 3 patches when Truck builds it (truck-shapeops fork), 4 from the slab
    // fallback.
    probe(&s, &ring_probes(7.8, 8.2));
    probe(&s, &ring_probes(5.2, 8.2));
    assert!(b.face_count == 9 || b.face_count == 10, "{}", b.face_count);
    export(&mut s, "canonical_engulfed_d16");
    // Undo / redo restore the volumes.
    ok(&mut s, Command::Undo);
    assert!((volume(&s) - (40000.0 - 250.0 * PI)).abs() < 1e-6 * 40000.0);
    ok(&mut s, Command::Redo);
    assert!((volume(&s) - want).abs() < 1e-6 * want);
    // Upstream edits regenerate exactly: a Ø12 hole is still engulfed; a
    // Ø14 cut still engulfs the Ø10 hole.
    let hole_sketch = s.summary().features[2].id;
    ok(
        &mut s,
        Command::SetParameter {
            feature: hole_sketch,
            name: "diameter".into(),
            value: 12.0,
        },
    );
    assert!((volume(&s) - want).abs() < 1e-6 * want);
    ok(
        &mut s,
        Command::SetParameter {
            feature: hole_sketch,
            name: "diameter".into(),
            value: 10.0,
        },
    );
    ok(
        &mut s,
        Command::SetParameter {
            feature: big,
            name: "diameter".into(),
            value: 14.0,
        },
    );
    let want14 = 40000.0 - 490.0 * PI;
    assert!(
        (volume(&s) - want14).abs() < 1e-6 * want14,
        "{}",
        volume(&s)
    );
    probe(&s, &ring_probes(6.8, 7.2));
    let _ = c;
}

#[test]
fn off_centre_partial_overlap_is_exact() {
    // A distinct case: Ø16 offset 4 mm from the Ø10 hole.
    for (dx, dy, through) in [(4.0, 0.0, true), (2.4, -3.2, false)] {
        let mut s = Session::new();
        let e = holed_plate(&mut s, true);
        let c = sketch(&mut s, top(e), circle(16.0, [40.0 + dx, 25.0 + dy]));
        ok(&mut s, cut(c, if through { 0.0 } else { 6.0 }, through));
        let u = disc_union(5.0, 8.0, dx.hypot(dy));
        let want = if through {
            40000.0 - 10.0 * u
        } else {
            40000.0 - 250.0 * PI - 6.0 * (u - 25.0 * PI)
        };
        assert!((volume(&s) - want).abs() < 1e-6 * want, "{}", volume(&s));
        // The far side of the old hole (outside the Ø16) keeps its wall.
        let far = [40.0 - 5.0 * dx / 4.0, 25.0 - 5.0 * dy / 4.0];
        let n = dx.hypot(dy);
        let out = [far[0] - 0.2 * dx / n, far[1] - 0.2 * dy / n];
        probe(
            &s,
            &[
                ([out[0], out[1], 5.0], true),
                ([40.0, 25.0, 5.0], false),
                ([40.0 + dx, 25.0 + dy, 9.0], false),
            ],
        );
        if through {
            export(&mut s, "offset_overlap_d16");
        }
    }
}

#[test]
fn seedhole_repro_overlapping_blind_round_cuts_on_a_boss() {
    // Overlapping blind round cuts on a boss (seed-hole repro).
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let sk = sketch(&mut s, SketchPlane::WorldXY, rect(60.0, 30.0, [0.0, 0.0]));
    let e = ok(&mut s, Command::extrude(None, sk, 8.0));
    let b = sketch(&mut s, top(e), rect(10.0, 10.0, [15.0, 0.0]));
    let boss = ok(
        &mut s,
        Command::Extrude {
            name: None,
            sketch: b,
            distance: 5.0,
            op: ExtrudeOp::Join,
            target: Some(BodyId(1)),
            through_all: false,
            reverse: false,
            body_name: None,
            region: None,
            options: Default::default(),
        },
    );
    let h1 = sketch(&mut s, top(boss), circle(4.0, [15.0, 0.0]));
    ok(&mut s, cut(h1, 2.0, false));
    let h2 = sketch(&mut s, top(boss), circle(4.0, [16.0, 0.5]));
    ok(&mut s, cut(h2, 2.0, false));
    let want = 14400.0 + 500.0 - 2.0 * disc_union(2.0, 2.0, 0.5f64.hypot(1.0));
    assert!((volume(&s) - want).abs() < 1e-6 * want, "{}", volume(&s));
    probe(
        &s,
        &[
            ([15.5, 0.25, 12.0], false),
            ([15.5, 0.25, 10.5], true), // common floor at z = 11
            ([13.3, 0.0, 12.0], false),
            ([17.6, 1.4, 12.0], false),
            ([19.5, -4.5, 12.0], true),
        ],
    );
    export(&mut s, "seedhole_two_d4");
}

/// Rounded rectangle [x0, x0 + w] x [y0, y0 + h], corner radius r (sketch
/// fillets, fully constrained).
fn rounded(x0: f64, y0: f64, w: f64, h: f64, r: f64) -> Sketch {
    let mut s = Sketch::default();
    let p: Vec<PointId> = [[x0, y0], [x0 + w, y0], [x0 + w, y0 + h], [x0, y0 + h]]
        .iter()
        .map(|q| s.add_point(q[0], q[1]))
        .collect();
    let o = s.add_point(0.0, 0.0);
    s.origin = Some(o);
    let l: Vec<EntityId> = (0..4).map(|i| s.add_line(p[i], p[(i + 1) % 4])).collect();
    s.constraints.extend([
        Constraint::FixedPoint {
            p: o,
            x: 0.0,
            y: 0.0,
        },
        Constraint::FixedPoint {
            p: p[0],
            x: x0,
            y: y0,
        },
        Constraint::Horizontal { line: l[0] },
        Constraint::Horizontal { line: l[2] },
        Constraint::Vertical { line: l[1] },
        Constraint::Vertical { line: l[3] },
        Constraint::HorizontalDistance {
            a: p[0],
            b: p[1],
            dim: Dimension::new("w", w),
        },
        Constraint::VerticalDistance {
            a: p[1],
            b: p[2],
            dim: Dimension::new("h", h),
        },
    ]);
    assert!(s.solve().is_success());
    s.fillet_corner(p[0], r, Some("r")).unwrap();
    for c in &p[1..] {
        s.fillet_corner(*c, r, None).unwrap();
    }
    s
}

#[test]
fn rounded_pocket_around_the_hole_and_an_overlapping_one() {
    let rr = |w: f64, h: f64, r: f64| w * h - (4.0 - PI) * r * r;
    let mut s = Session::new();
    let e = holed_plate(&mut s, true);
    let a = sketch(
        &mut s,
        top(e),
        SketchTemplate::Custom {
            sketch: rounded(30.0, 15.0, 25.0, 20.0, 4.0),
        },
    );
    ok(&mut s, cut(a, 3.0, false));
    let want = 40000.0 - 250.0 * PI - 3.0 * (rr(25.0, 20.0, 4.0) - 25.0 * PI);
    assert!((volume(&s) - want).abs() < 1e-6 * want, "{}", volume(&s));
    // A second rounded pocket whose SW corner circle is the first one's NE
    // corner circle (51, 31); same depth (coplanar floors). Sketched on a
    // construction plane at the top (the top face is split by now? no: the
    // first pocket is inside it; a face reference still works).
    let b = sketch(
        &mut s,
        top(e),
        SketchTemplate::Custom {
            sketch: rounded(47.0, 27.0, 23.0, 18.0, 4.0),
        },
    );
    ok(&mut s, cut(b, 3.0, false));
    let overlap = 64.0 - 2.0 * (1.0 - PI / 4.0) * 16.0;
    let want2 = want - 3.0 * (rr(23.0, 18.0, 4.0) - overlap);
    assert!((volume(&s) - want2).abs() < 1e-6 * want2, "{}", volume(&s));
    probe(
        &s,
        &[
            ([54.5, 34.5, 8.5], false),
            ([47.5, 27.5, 8.5], false),
            ([56.0, 26.0, 8.5], true),
            ([46.0, 36.0, 8.5], true),
            ([51.0, 31.0, 6.8], true),
            ([40.0, 25.0, 3.0], false), // the hole below the pocket floor
            ([45.3, 25.0, 3.0], true),
        ],
    );
    export(&mut s, "rounded_pockets_over_hole");
}

/// Square ring (rect minus rect) and a disc-holed rectangle as Cut regions on
/// a plate (ADR 0007: tool region = outer minus holes).
fn ring_sketch(hole_circle: bool) -> Sketch {
    let mut s = Sketch::default();
    let o = s.add_point(0.0, 0.0);
    s.origin = Some(o);
    s.constraints.push(Constraint::FixedPoint {
        p: o,
        x: 0.0,
        y: 0.0,
    });
    let mut sq = |x0: f64, y0: f64, x1: f64, y1: f64| {
        let p: Vec<PointId> = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
            .iter()
            .map(|q| s.add_point(q[0], q[1]))
            .collect();
        for i in 0..4 {
            s.add_line(p[i], p[(i + 1) % 4]);
        }
        for (i, q) in [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].iter().enumerate() {
            s.constraints.push(Constraint::FixedPoint {
                p: p[i],
                x: q[0],
                y: q[1],
            });
        }
    };
    sq(20.0, 10.0, 60.0, 40.0);
    if hole_circle {
        let c = s.add_point(40.0, 25.0);
        s.add_circle(c, 6.0);
        s.constraints.push(Constraint::FixedPoint {
            p: c,
            x: 40.0,
            y: 25.0,
        });
    } else {
        sq(30.0, 18.0, 50.0, 32.0);
    }
    assert!(s.solve().is_success());
    s
}

fn region_cut(s: &mut Session, sk: FeatureId, d: f64) -> Command {
    let summary = s.summary();
    let f = summary.features.iter().find(|f| f.id == sk).unwrap();
    let regions = f.sketch.as_ref().unwrap().regions().unwrap();
    let holed = regions
        .into_iter()
        .find(|r| !r.id.holes.is_empty())
        .expect("holed region");
    Command::Extrude {
        name: None,
        sketch: sk,
        distance: d,
        op: ExtrudeOp::Cut,
        target: Some(BodyId(1)),
        through_all: false,
        reverse: false,
        body_name: None,
        region: Some(holed.id),
        options: Default::default(),
    }
}

#[test]
fn holed_region_cuts_leave_islands_exactly() {
    for (round, want_area) in [(false, 1200.0 - 280.0), (true, 1200.0 - 36.0 * PI)] {
        let mut s = Session::new();
        let e = holed_plate(&mut s, false);
        let sk = sketch(
            &mut s,
            top(e),
            SketchTemplate::Custom {
                sketch: ring_sketch(round),
            },
        );
        let c = region_cut(&mut s, sk, 4.0);
        ok(&mut s, c);
        let want = 40000.0 - 4.0 * want_area;
        assert!((volume(&s) - want).abs() < 1e-6 * want, "{}", volume(&s));
        probe(
            &s,
            &[
                ([40.0, 25.0, 8.0], true), // the island stands
                ([22.0, 12.0, 8.0], false),
                ([40.0, 25.0, 5.0], true),
            ],
        );
        if round {
            export(&mut s, "holed_region_round_island");
        }
    }
}

#[test]
fn remaining_refusals_leave_the_document_unchanged() {
    let state = |s: &Session| {
        let d = s.summary();
        (d.features.len(), d.revision, d.can_undo.clone(), volume(s))
    };
    // 1. A pocket wall tangent to the hole (x = 45): a non-manifold contact
    //    the polygons cannot resolve; refused by name.
    let mut s = Session::new();
    let e = holed_plate(&mut s, true);
    let p = sketch(&mut s, top(e), rect(15.0, 30.0, [52.5, 25.0]));
    let before = state(&s);
    let r = s.execute(&cut(p, 3.0, false));
    assert!(!r.ok);
    assert_eq!(state(&s), before);
    let m = format!("{:?}", r.error);
    assert!(m.contains("tangent"), "{m}");
    // 2. A holed region on a target with a hole across the tool axis (not
    //    prismatic): the slab construction does not apply; the general
    //    boolean with the holed tool builds it, checked, and exact:
    //    40000 − π·2²·50 (side hole) − (40·30 − π·6²)·4 (ring around a
    //    Ø12 island).
    let mut s = Session::new();
    let e = holed_plate(&mut s, false);
    let side = SketchPlane::Face(FaceRef {
        feature: e,
        role: FaceRole::ExtrudeSide { entity: 1 },
    });
    let x = sketch(&mut s, side, circle(4.0, [70.0, 5.0]));
    ok(&mut s, cut(x, 0.0, true));
    let sk = sketch(
        &mut s,
        top(e),
        SketchTemplate::Custom {
            sketch: ring_sketch(true),
        },
    );
    let c = region_cut(&mut s, sk, 4.0);
    let r = s.execute(&c);
    assert!(r.ok, "{:?}", r.error);
    let want = 40000.0 - PI * 4.0 * 50.0 - (1200.0 - 36.0 * PI) * 4.0;
    assert!((volume(&s) - want).abs() < 1e-6 * want, "{}", volume(&s));
    probe(
        &s,
        &[
            ([25.0, 25.0, 8.0], false),
            ([40.0, 25.0, 8.0], true),
            ([25.0, 25.0, 5.0], true),
            ([70.0, 25.0, 5.0], false),
        ],
    );
}

/// The legacy Cut Through All command (`legacy_cut`) has the slab fallback:
/// the canonical engulfing cut through it is exact.
#[test]
fn legacy_cut_through_all_engulfing_the_hole_is_exact() {
    let mut s = Session::new();
    let e = holed_plate(&mut s, true);
    let big = sketch(&mut s, top(e), circle(16.0, [40.0, 25.0]));
    let r = s.execute(&Command::CutThroughAll {
        name: None,
        sketch: big,
    });
    assert!(r.ok, "{:?}", r.error);
    let want = 40000.0 - 640.0 * PI;
    assert!((volume(&s) - want).abs() < 1e-6 * want, "{}", volume(&s));
    probe(&s, &ring_probes(5.2, 8.2));
}
