//! Extruding a picked sketch region: two concentric
//! rectangles and a circle island give three regions; extrude only the ring,
//! edit dimensions and regenerate the same region, break a reference only when
//! the referenced region itself disappears, save/reopen, refusals. Volumes are
//! analytic (areas × distance).
use rcad_commands::{Command, ExtrudeOp, Response, Session, SketchTemplate};
use rcad_document::{DocError, FeatureId, SketchPlane};
use rcad_kernel::VolumeResult;
use rcad_sketch::{Constraint, Dimension, EntityId, PointId, RegionRef, Sketch};
use std::f64::consts::PI;

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

/// Centred rectangle with width/height dimensions `{tag}_w` / `{tag}_h`.
fn rect(s: &mut Sketch, w: f64, h: f64, tag: &str) -> Vec<EntityId> {
    let p: Vec<PointId> = [
        [-w / 2.0, -h / 2.0],
        [w / 2.0, -h / 2.0],
        [w / 2.0, h / 2.0],
        [-w / 2.0, h / 2.0],
    ]
    .iter()
    .map(|q| s.add_point(q[0], q[1]))
    .collect();
    let lines: Vec<EntityId> = (0..4).map(|i| s.add_line(p[i], p[(i + 1) % 4])).collect();
    s.constraints.extend([
        Constraint::Horizontal { line: lines[0] },
        Constraint::Horizontal { line: lines[2] },
        Constraint::Vertical { line: lines[1] },
        Constraint::Vertical { line: lines[3] },
        Constraint::HorizontalDistance {
            a: p[0],
            b: p[1],
            dim: Dimension::new(format!("{tag}_w"), w),
        },
        Constraint::VerticalDistance {
            a: p[1],
            b: p[2],
            dim: Dimension::new(format!("{tag}_h"), h),
        },
        Constraint::SymmetricAboutPoint {
            a: p[0],
            b: p[2],
            center: PointId(0),
        },
    ]);
    lines
}

/// Outer 60x40, inner 40x20, circle Ø10 island, all centred on the origin.
fn nested_sketch() -> Sketch {
    let mut s = Sketch::default();
    let o = s.add_point(0.0, 0.0);
    s.origin = Some(o);
    rect(&mut s, 60.0, 40.0, "outer");
    rect(&mut s, 40.0, 20.0, "inner");
    let c = s.add_point(0.0, 0.0);
    let circle = s.add_circle(c, 5.0);
    s.constraints.extend([
        Constraint::Coincident { a: c, b: o },
        Constraint::Diameter {
            circle,
            dim: Dimension::new("hole", 10.0),
        },
    ]);
    s
}

fn extrude(sketch: FeatureId, d: f64, op: ExtrudeOp, region: Option<RegionRef>) -> Command {
    Command::Extrude {
        name: None,
        sketch,
        distance: d,
        op,
        target: None,
        through_all: false,
        reverse: false,
        body_name: None,
        region,
        options: Default::default(),
    }
}

/// The same extrusion against the sketch normal.
fn rev(c: Command) -> Command {
    match c {
        Command::Extrude {
            name,
            sketch,
            distance,
            op,
            target,
            through_all,
            body_name,
            region,
            ..
        } => Command::Extrude {
            name,
            sketch,
            distance,
            op,
            target,
            through_all,
            reverse: true,
            body_name,
            region,
            options: Default::default(),
        },
        c => c,
    }
}

fn volume(s: &Session, body: usize) -> f64 {
    match s.summary().bodies[body]
        .measurements
        .clone()
        .unwrap()
        .volume
    {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

/// New document with the nested sketch; returns (sketch id, regions by depth).
fn setup(s: &mut Session) -> (FeatureId, Vec<RegionRef>) {
    ok(s, Command::NewDocument);
    let f = ok(
        s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::Custom {
                sketch: nested_sketch(),
            },
        },
    )
    .feature
    .unwrap();
    let sk = s.summary().features[0].sketch.clone().unwrap();
    let rs: Vec<RegionRef> = sk.regions().unwrap().into_iter().map(|r| r.id).collect();
    assert_eq!(rs.len(), 3);
    (f, rs)
}

const TOL: f64 = 1e-6;

#[test]
fn extrude_only_the_ring_then_edit_and_regenerate_the_same_region() {
    let mut s = Session::new();
    let (f, rs) = setup(&mut s);
    // Default (no region) keeps today's behaviour: the island is refused.
    let r = s.execute(&extrude(f, 10.0, ExtrudeOp::NewBody, None));
    assert!(!r.ok, "{r:?}");
    let r = ok(
        &mut s,
        extrude(f, 10.0, ExtrudeOp::NewBody, Some(rs[0].clone())),
    );
    let ring = r.feature.unwrap();
    assert!((volume(&s, 0) - (2400.0 - 800.0) * 10.0).abs() < TOL);
    let summary = s.summary();
    let fs = summary.features.iter().find(|x| x.id == ring).unwrap();
    assert_eq!(fs.region.as_ref(), Some(&rs[0]));
    // The middle region (inner minus island) and the island as further bodies.
    ok(
        &mut s,
        extrude(f, 5.0, ExtrudeOp::NewBody, Some(rs[1].clone())),
    );
    ok(
        &mut s,
        extrude(f, 2.0, ExtrudeOp::NewBody, Some(rs[2].clone())),
    );
    assert!((volume(&s, 1) - (800.0 - 25.0 * PI) * 5.0).abs() < 1e-3);
    assert!((volume(&s, 2) - 25.0 * PI * 2.0).abs() < 1e-3);
    // Dimension edits: the same regions regenerate with the new sizes.
    ok(
        &mut s,
        Command::SetParameter {
            feature: f,
            name: "outer_w".into(),
            value: 80.0,
        },
    );
    ok(
        &mut s,
        Command::SetParameter {
            feature: f,
            name: "hole".into(),
            value: 12.0,
        },
    );
    assert!((volume(&s, 0) - (3200.0 - 800.0) * 10.0).abs() < TOL);
    assert!((volume(&s, 1) - (800.0 - 36.0 * PI) * 5.0).abs() < 1e-3);
    assert!((volume(&s, 2) - 36.0 * PI * 2.0).abs() < 1e-3);
}

#[test]
fn deleting_the_island_breaks_only_the_region_that_had_it() {
    // Document A references only the ring: removing the island is fine.
    let mut s = Session::new();
    let (f, rs) = setup(&mut s);
    ok(
        &mut s,
        extrude(f, 10.0, ExtrudeOp::NewBody, Some(rs[0].clone())),
    );
    let mut no_island = s.summary().features[0].sketch.clone().unwrap();
    let circle = no_island
        .entities
        .iter()
        .find(|e| matches!(e, rcad_sketch::Entity::Circle { .. }))
        .unwrap()
        .id();
    assert!(no_island.remove_entity(circle));
    ok(
        &mut s,
        Command::EditSketch {
            feature: f,
            sketch: no_island.clone(),
        },
    );
    assert!((volume(&s, 0) - 16000.0).abs() < TOL);

    // Document B references the middle region (inner rectangle minus island):
    // the same edit is refused with a BrokenReference naming the region, and
    // nothing changes (never silently another region).
    let mut t = Session::new();
    let (g, rs) = setup(&mut t);
    let e = ok(
        &mut t,
        extrude(g, 5.0, ExtrudeOp::NewBody, Some(rs[1].clone())),
    )
    .feature
    .unwrap();
    let before = t.summary();
    let r = t.execute(&Command::EditSketch {
        feature: g,
        sketch: no_island,
    });
    match &r.error {
        Some(DocError::BrokenReference { from, detail }) => {
            assert_eq!(*from, e);
            assert!(detail.contains("no longer exists"), "{detail}");
        }
        other => panic!("expected BrokenReference, got {other:?}"),
    }
    let after = t.summary();
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.features, after.features);
}

#[test]
fn region_survives_save_and_reopen_and_json_is_additive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("regions.rcad.json");
    let mut s = Session::new();
    let (f, rs) = setup(&mut s);
    ok(
        &mut s,
        extrude(f, 10.0, ExtrudeOp::NewBody, Some(rs[0].clone())),
    );
    ok(&mut s, Command::Save { path: path.clone() });
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"region\""), "region stored");
    let mut t = Session::new();
    ok(
        &mut t,
        Command::Open {
            path,
            safe_mode: false,
        },
    );
    assert_eq!(t.summary().features[1].region.as_ref(), Some(&rs[0]));
    assert!((volume(&t, 0) - 16000.0).abs() < TOL);
    // Command JSON: absent region = previous shape; present = {"outer":[..],"holes":[[..]]}.
    let old: Command =
        serde_json::from_str(r#"{"cmd":"extrude","sketch":1,"distance":5}"#).unwrap();
    assert!(matches!(old, Command::Extrude { region: None, .. }));
    assert!(!serde_json::to_string(&old).unwrap().contains("region"));
    let new: Command = serde_json::from_str(
        r#"{"cmd":"extrude","sketch":1,"distance":5,"region":{"outer":[1,2,3,4],"holes":[[5,6,7,8]]}}"#,
    )
    .unwrap();
    assert!(matches!(new, Command::Extrude { region: Some(ref r), .. } if r.holes.len() == 1));
}

#[test]
fn cut_with_a_holed_region_is_exact_and_a_single_loop_region_cuts() {
    let mut s = Session::new();
    let (f, rs) = setup(&mut s);
    // A plate to cut, below the sketch plane (its top face is on the sketch
    // plane, so a Cut goes into the material by default).
    let plate = ok(
        &mut s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 100.0,
                height: 100.0,
                center: None,
            },
        },
    )
    .feature
    .unwrap();
    ok(&mut s, rev(extrude(plate, 10.0, ExtrudeOp::NewBody, None)));
    let v0 = volume(&s, 0);
    // A holed region on a prismatic target is cut by the exact slab
    // construction (ADR 0007): the ring (2400 - 800 mm²) becomes a groove.
    // Inner-loop refusals on non-prismatic targets: tests/it/slab_arcs.rs.
    let r = s.execute(&extrude(f, 3.0, ExtrudeOp::Cut, Some(rs[0].clone())));
    assert!(r.ok, "{r:?}");
    assert!((volume(&s, 0) - (v0 - 1600.0 * 3.0)).abs() < 1e-6 * v0);
    // The island disc is a single loop: an ordinary cut.
    ok(&mut s, extrude(f, 3.0, ExtrudeOp::Cut, Some(rs[2].clone())));
    assert!((volume(&s, 0) - (v0 - 1600.0 * 3.0 - 25.0 * PI * 3.0)).abs() < 1e-3);
}

/// Two overlapping circles (r = 10, centres 10 apart): the lens is picked
/// and extruded 5 mm (closed-form lens area × 5); a radius edit regenerates
/// the same picked face (same reference).
#[test]
fn extrude_the_lens_of_two_overlapping_circles() {
    let mut sk = Sketch::default();
    let o = sk.add_point(0.0, 0.0);
    sk.origin = Some(o);
    let c1 = sk.add_point(0.0, 0.0);
    let c2 = sk.add_point(10.0, 0.0);
    let k1 = sk.add_circle(c1, 10.0);
    let k2 = sk.add_circle(c2, 10.0);
    sk.constraints.extend([
        Constraint::FixedPoint {
            p: o,
            x: 0.0,
            y: 0.0,
        },
        Constraint::Coincident { a: c1, b: o },
        Constraint::FixedPoint {
            p: c2,
            x: 10.0,
            y: 0.0,
        },
        Constraint::Radius {
            entity: k1,
            dim: Dimension::new("r1", 10.0),
        },
        Constraint::Radius {
            entity: k2,
            dim: Dimension::new("r2", 10.0),
        },
    ]);
    let lens = |r: f64, d: f64| {
        2.0 * r * r * (d / (2.0 * r)).acos() - 0.5 * d * (4.0 * r * r - d * d).sqrt()
    };
    let lens_id = sk.region_at([5.0, 0.0]).unwrap().expect("lens");
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let f = ok(
        &mut s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::Custom { sketch: sk },
        },
    )
    .feature
    .unwrap();
    ok(&mut s, extrude(f, 5.0, ExtrudeOp::NewBody, Some(lens_id)));
    let v = volume(&s, 0);
    let want = lens(10.0, 10.0) * 5.0;
    assert!((v - want).abs() < 1e-6 * want, "{v} vs {want}");
    // Both radii 12: the lens of the same circles grows, same reference.
    for name in ["r1", "r2"] {
        ok(
            &mut s,
            Command::SetParameter {
                feature: f,
                name: name.into(),
                value: 12.0,
            },
        );
    }
    let v = volume(&s, 0);
    let want = lens(12.0, 10.0) * 5.0;
    assert!((v - want).abs() < 1e-6 * want, "{v} vs {want}");
}
