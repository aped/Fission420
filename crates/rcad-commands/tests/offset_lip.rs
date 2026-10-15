//! Offset lip acceptance: the enclosure lid's lip opening built by
//! offsetting the lip outline inward by `lip_wall` (an Offset dimension bound
//! to the parameter), then cutting the inner region from the lip's end face.
//! The lid volume equals `Enclosure::lid_volume()` for the baseline and after
//! the variant's parameter change (wall 2.0 -> 2.4 moves the offset).
use rcad_commands::printables::{self, ENCLOSURE_BASELINE, ENCLOSURE_VARIANT, enclosure_ids};
use rcad_commands::{Command, ExtrudeOp, Response, Session, SketchTemplate};
use rcad_document::{BodyId, FaceRef, FaceRole, FeatureId, SketchPlane};
use rcad_kernel::VolumeResult;
use rcad_sketch::{Constraint, Dimension, EntityId, PointId, RegionRef, Sketch};

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn vol(s: &Session, b: u32) -> f64 {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|x| x.id == BodyId(b))
        .unwrap();
    match b.measurements.unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

/// Lip outline (w x h centred at (cx, cy), dimensions `lip_w`, `lip_h`, `cx`,
/// `cy` from the sketch origin) and its inward offset by `wall` (dimension
/// `lip_wall_d`). Returns the sketch and the copy's entities.
fn lip_sketch(w: f64, h: f64, cx: f64, cy: f64, wall: f64) -> (Sketch, Vec<EntityId>) {
    let mut s = Sketch::default();
    let o = s.add_point(0.0, 0.0);
    s.origin = Some(o);
    let c = s.add_point(cx, cy);
    let p: Vec<PointId> = [
        [cx - w / 2.0, cy - h / 2.0],
        [cx + w / 2.0, cy - h / 2.0],
        [cx + w / 2.0, cy + h / 2.0],
        [cx - w / 2.0, cy + h / 2.0],
    ]
    .iter()
    .map(|q| s.add_point(q[0], q[1]))
    .collect();
    let l: Vec<EntityId> = (0..4).map(|i| s.add_line(p[i], p[(i + 1) % 4])).collect();
    s.constraints.extend([
        Constraint::FixedPoint {
            p: o,
            x: 0.0,
            y: 0.0,
        },
        Constraint::Horizontal { line: l[0] },
        Constraint::Horizontal { line: l[2] },
        Constraint::Vertical { line: l[1] },
        Constraint::Vertical { line: l[3] },
        Constraint::HorizontalDistance {
            a: p[0],
            b: p[1],
            dim: Dimension::new("lip_w", w),
        },
        Constraint::VerticalDistance {
            a: p[1],
            b: p[2],
            dim: Dimension::new("lip_h", h),
        },
        Constraint::SymmetricAboutPoint {
            a: p[0],
            b: p[2],
            center: c,
        },
        Constraint::HorizontalDistance {
            a: o,
            b: c,
            dim: Dimension::new("cx", cx),
        },
        Constraint::VerticalDistance {
            a: o,
            b: c,
            dim: Dimension::new("cy", cy),
        },
    ]);
    assert!(s.solve().is_success());
    let r = s.offset_loop(l[0], -wall, Some("lip_wall_d")).unwrap();
    assert_eq!(r.dimension, "lip_wall_d");
    (s, r.entities)
}

#[test]
fn lip_opening_by_offset_matches_the_analytic_lid_volume_and_follows_wall() {
    let dir = tempfile::tempdir().unwrap();
    let steps = printables::enclosure_workflow(&dir.path().join("e.rcad.json"));
    // Replay up to (excluding) the printables' own lip-opening sketch.
    let cut_at = steps
        .iter()
        .position(|st| {
            matches!(&st.cmd, Command::CreateSketch { name: Some(n), .. } if n == "Lip opening outline")
        })
        .unwrap();
    let mut s = Session::new();
    for st in &steps[..cut_at] {
        let r = s.execute(&st.cmd);
        assert_eq!(r.ok, st.expect_ok, "{:?}: {:?}", st.cmd, r.error);
    }
    let p = ENCLOSURE_BASELINE;
    let ((ol, ow), _) = p.lip();
    let (cx, cy) = (p.outer_length / 2.0, p.outer_width / 2.0 + p.lid_gap);
    let (sk, copy) = lip_sketch(ol, ow, cx, cy, p.wall);
    let f = ok(
        &mut s,
        Command::CreateSketch {
            name: Some("Lip opening by offset".into()),
            plane: SketchPlane::Face(FaceRef {
                feature: FeatureId(enclosure_ids::LIP),
                role: FaceRole::ExtrudeEndCap,
            }),
            template: SketchTemplate::Custom { sketch: sk },
        },
    )
    .feature
    .unwrap();
    for (name, expr) in [
        ("lip_w", "lip_length"),
        ("lip_h", "lip_width"),
        ("cx", "outer_length / 2"),
        ("cy", "outer_width / 2 + lid_gap"),
        ("lip_wall_d", "lip_wall"),
    ] {
        ok(
            &mut s,
            Command::SetExpression {
                feature: f,
                name: name.into(),
                expression: expr.into(),
            },
        );
    }
    let mut inner = copy.clone();
    inner.sort();
    let region = RegionRef {
        outer: inner,
        holes: vec![],
        outside: vec![],
    };
    let cut = ok(
        &mut s,
        Command::Extrude {
            name: Some("Lip opening".into()),
            sketch: f,
            distance: p.lip_depth,
            op: ExtrudeOp::Cut,
            target: Some(BodyId(enclosure_ids::LID_BODY)),
            through_all: false,
            reverse: false,
            body_name: None,
            region: Some(region),
            options: Default::default(),
        },
    )
    .feature
    .unwrap();
    ok(
        &mut s,
        Command::SetExpression {
            feature: cut,
            name: "distance".into(),
            expression: "lip_depth".into(),
        },
    );
    let check = |s: &Session, e: &printables::Enclosure| {
        let (vb, vl) = (vol(s, 1), vol(s, 2));
        assert!((vb - e.base_volume()).abs() < 1e-6, "base {vb}");
        assert!(
            (vl - e.lid_volume()).abs() < 1e-6,
            "lid {vl} vs {}",
            e.lid_volume()
        );
    };
    check(&s, &ENCLOSURE_BASELINE);
    // Variant: wall 2.4 (lip wall tracks it through the offset dimension),
    // larger box, clearance 0.35.
    for c in ENCLOSURE_VARIANT.set_parameters() {
        ok(&mut s, c);
    }
    check(&s, &ENCLOSURE_VARIANT);
    // The offset dimension shows the bound value.
    let sk = s
        .summary()
        .features
        .iter()
        .find(|x| x.id == f)
        .unwrap()
        .sketch
        .clone()
        .unwrap();
    assert!(
        sk.dimensions()
            .contains(&("lip_wall_d".to_string(), ENCLOSURE_VARIANT.wall))
    );
}
