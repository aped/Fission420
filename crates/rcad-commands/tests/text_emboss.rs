//! Text emboss / deboss on an enclosure's top face, end to end (run 6, SK04 +
//! the multi-region extrusion `options.regions`): a 100 x 70 x 30 box with a
//! 2.4 mm wall/top cavity cut from below; "B8" (12 mm cap height, centred) on
//! the top face; ALL glyph regions extruded in ONE feature (Join 1 mm raised,
//! or Cut 0.8 mm recessed). Oracles: analytic volume (box − cavity ± glyph
//! region areas × height; the region areas are the exact areas of the polygons
//! the kernel extrudes), point-membership probes (glyph strokes material/air,
//! counters stay open/solid), and text edits afterwards: "88" keeps the
//! glyph/counter structure (regions re-resolve, volume follows), "B9" removes a
//! counter of a referenced region (refused, naming the region; model kept).
//!
//! Needs the installed font "Liberation Sans"; `RCAD_SKIP_FONT_TESTS=1` skips.

use rcad_commands::{Command, ExtrudeOp, Response, Session, SketchTemplate, end_cap_of};
use rcad_document::{
    BodyId, ExtrudeOptions, FaceRef, FaceRole, FeatureId, FeatureKind, SketchPlane,
};
use rcad_kernel::VolumeResult;
use rcad_sketch::{FontSpec, HAlign, RegionRef, Sketch, SketchRegion, TextSpec, VAlign};

const FAMILY: &str = "Liberation Sans";
const BOX: f64 = 100.0 * 70.0 * 30.0;
const CAVITY: f64 = 95.2 * 65.2 * 27.6;

fn have_font() -> bool {
    match rcad_sketch::font::find_font(&FontSpec::family(FAMILY)) {
        Ok(_) => true,
        Err(e) if std::env::var("RCAD_SKIP_FONT_TESTS").as_deref() == Ok("1") => {
            eprintln!("SKIPPED (RCAD_SKIP_FONT_TESTS=1): {e}");
            false
        }
        Err(e) => panic!("{e}; install {FAMILY} or set RCAD_SKIP_FONT_TESTS=1"),
    }
}

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn volume(s: &Session) -> f64 {
    match s.summary().bodies[0].measurements.clone().unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn rect(w: f64, h: f64, plane: SketchPlane) -> Command {
    Command::CreateSketch {
        name: None,
        plane,
        template: SketchTemplate::CenteredRectangle {
            width: w,
            height: h,
            center: None,
        },
    }
}

fn extrude(sketch: FeatureId, d: f64, op: ExtrudeOp, regions: &[RegionRef]) -> Command {
    let options = ExtrudeOptions {
        regions: regions.iter().skip(1).cloned().collect(),
        ..Default::default()
    };
    Command::Extrude {
        name: None,
        sketch,
        distance: d,
        op,
        target: Some(BodyId(1)),
        through_all: false,
        reverse: false,
        body_name: None,
        region: regions.first().cloned(),
        options,
    }
}

/// Enclosure shell and an empty "Label" sketch on its top face; returns the
/// label sketch.
fn enclosure(s: &mut Session) -> FeatureId {
    ok(s, Command::NewDocument);
    let base = ok(s, rect(100.0, 70.0, SketchPlane::WorldXY))
        .feature
        .unwrap();
    let ext = ok(s, Command::extrude(None, base, 30.0)).feature.unwrap();
    let start = SketchPlane::Face(FaceRef {
        feature: ext,
        role: FaceRole::ExtrudeStartCap,
    });
    let cav = ok(s, rect(95.2, 65.2, start)).feature.unwrap();
    ok(s, extrude(cav, 27.6, ExtrudeOp::Cut, &[]));
    let v = volume(s);
    assert!((v - (BOX - CAVITY)).abs() < 1e-6 * BOX, "{v}");
    let mut empty = Sketch::default();
    empty.ensure_origin();
    ok(
        s,
        Command::CreateSketch {
            name: Some("Label".into()),
            plane: end_cap_of(ext),
            template: SketchTemplate::Custom { sketch: empty },
        },
    )
    .feature
    .unwrap()
}

fn solved_sketch(s: &Session, f: FeatureId) -> Sketch {
    match &s.model().unwrap().document().feature(f).unwrap().kind {
        FeatureKind::Sketch { sketch, .. } => {
            let mut k = sketch.clone();
            k.solve();
            k
        }
        _ => panic!(),
    }
}

fn glyphs(s: &Session, f: FeatureId) -> Vec<SketchRegion> {
    solved_sketch(s, f)
        .regions()
        .unwrap()
        .into_iter()
        .filter(|r| r.depth == 0)
        .collect()
}

/// A sketch point inside region `id` (grid search over the region's box).
fn point_in(sk: &Sketch, r: &SketchRegion) -> [f64; 2] {
    let rcad_sketch::Profile::Polygon(v) = &r.region.outer else {
        panic!("glyph loops are polygons")
    };
    let (x0, x1) = v
        .iter()
        .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p[0]), a.1.max(p[0])));
    let (y0, y1) = v
        .iter()
        .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p[1]), a.1.max(p[1])));
    for i in 1..40 {
        for j in 1..40 {
            let p = [
                x0 + (x1 - x0) * i as f64 / 40.0,
                y0 + (y1 - y0) * j as f64 / 40.0,
            ];
            if sk.region_at(p).unwrap().as_ref() == Some(&r.id) {
                // Keep clear of the boundary (probes use a tessellation).
                let near = (0..v.len()).any(|k| {
                    let (a, b) = (v[k], v[(k + 1) % v.len()]);
                    seg_dist(p, a, b) < 0.3
                });
                if !near {
                    return p;
                }
            }
        }
    }
    panic!("no interior point in {}", r.id.label())
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 0.0 {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - t * d[0]).hypot(p[1] - a[1] - t * d[1])
}

/// Stroke and counter probe points (sketch coordinates) of the glyph regions.
fn probe_points(s: &Session, label: FeatureId) -> (Vec<[f64; 2]>, Vec<[f64; 2]>) {
    let sk = solved_sketch(s, label);
    let rs = sk.regions().unwrap();
    let strokes = rs
        .iter()
        .filter(|r| r.depth == 0)
        .map(|r| point_in(&sk, r))
        .collect();
    let counters = rs
        .iter()
        .filter(|r| r.depth == 1)
        .map(|r| point_in(&sk, r))
        .collect();
    (strokes, counters)
}

/// Membership of sketch points lifted to height z (world; the label sketch
/// lies on z = 30 with the world axes).
fn membership(s: &Session, pts: &[[f64; 2]], z: f64) -> Vec<Option<bool>> {
    let body = s
        .model()
        .unwrap()
        .evaluation()
        .body_by_id(BodyId(1))
        .unwrap()
        .geometry
        .brep()
        .unwrap()
        .clone();
    let w: Vec<[f64; 3]> = pts.iter().map(|p| [p[0], p[1], z]).collect();
    rcad_kernel::point_membership(&body, &w).unwrap()
}

fn label_frame_is_world(s: &Session, label: FeatureId) {
    let f = s
        .summary()
        .features
        .into_iter()
        .find(|f| f.id == label)
        .unwrap();
    let fr = f.frame.expect("sketch frame");
    assert_eq!(fr.origin[2], 30.0);
    assert_eq!((fr.x, fr.y), ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]));
}

fn add_b8(s: &mut Session, label: FeatureId) {
    let mut t = TextSpec::new("B8", FAMILY, 12.0, [0.0, 0.0]);
    t.halign = HAlign::Center;
    t.valign = VAlign::Middle;
    ok(
        s,
        Command::AddSketchText {
            feature: label,
            text: t,
        },
    );
}

fn edit_string(s: &mut Session, label: FeatureId, text: &str) -> Response {
    let mut t = solved_sketch(s, label).texts[0].spec.clone();
    t.text = text.into();
    s.execute(&Command::EditSketchText {
        feature: label,
        id: 1,
        text: t,
    })
}

#[test]
fn raised_b8_in_one_feature_with_open_counters_and_string_edits() {
    if !have_font() {
        return;
    }
    let mut s = Session::new();
    let label = enclosure(&mut s);
    label_frame_is_world(&s, label);
    add_b8(&mut s, label);
    let g = glyphs(&s, label);
    assert_eq!(g.len(), 2);
    assert!(
        g.iter().all(|r| r.region.holes.len() == 2),
        "B and 8: two counters each"
    );
    let ids: Vec<RegionRef> = g.iter().map(|r| r.id.clone()).collect();
    let n_before = s.summary().features.len();
    ok(&mut s, extrude(label, 1.0, ExtrudeOp::Join, &ids));
    assert_eq!(s.summary().features.len(), n_before + 1, "one feature");
    let glyph_area: f64 = g.iter().map(|r| r.area).sum();
    let expect = BOX - CAVITY + glyph_area;
    let v = volume(&s);
    assert!((v - expect).abs() < 1e-6 * expect, "{v} vs {expect}");
    assert_eq!(s.summary().bodies.len(), 1);

    let (strokes, counters) = probe_points(&s, label);
    assert_eq!(counters.len(), 4);
    // 0.5 mm above the top: strokes are material, counters open (air).
    assert!(
        membership(&s, &strokes, 30.5)
            .iter()
            .all(|m| *m == Some(true))
    );
    assert!(
        membership(&s, &counters, 30.5)
            .iter()
            .all(|m| *m == Some(false))
    );
    // Below the top skin (z 29) the cavity: air; above the text (31.5): air.
    assert!(
        membership(&s, &strokes, 31.5)
            .iter()
            .all(|m| *m == Some(false))
    );
    assert!(
        membership(&s, &counters, 29.0)
            .iter()
            .all(|m| *m == Some(true))
    );

    // "88": same glyph/counter structure, regions re-resolve, volume follows.
    let r = edit_string(&mut s, label, "88");
    assert!(r.ok, "{:?}", r.error);
    let g88: f64 = glyphs(&s, label).iter().map(|r| r.area).sum();
    let e88 = BOX - CAVITY + g88;
    assert!((g88 - glyph_area).abs() > 1e-3, "different glyph");
    assert!(
        (volume(&s) - e88).abs() < 1e-6 * e88,
        "{} vs {e88}",
        volume(&s)
    );

    // "89": the referenced second glyph loses a counter: refused by name,
    // model unchanged.
    let r = edit_string(&mut s, label, "89");
    assert!(!r.ok);
    let msg = format!("{:?}", r.error);
    assert!(msg.contains("T1.2.0"), "names the region: {msg}");
    assert!((volume(&s) - e88).abs() < 1e-6 * e88);
    assert_eq!(solved_sketch(&s, label).texts[0].spec.text, "88");
}

#[test]
fn recessed_b8_in_one_feature_keeps_counter_islands() {
    if !have_font() {
        return;
    }
    let mut s = Session::new();
    let label = enclosure(&mut s);
    add_b8(&mut s, label);
    let g = glyphs(&s, label);
    let ids: Vec<RegionRef> = g.iter().map(|r| r.id.clone()).collect();
    ok(&mut s, extrude(label, 0.8, ExtrudeOp::Cut, &ids));
    let glyph_area: f64 = g.iter().map(|r| r.area).sum();
    let expect = BOX - CAVITY - 0.8 * glyph_area;
    let v = volume(&s);
    assert!((v - expect).abs() < 1e-6 * expect, "{v} vs {expect}");
    let (strokes, counters) = probe_points(&s, label);
    // 0.4 mm into the top skin: strokes cut away, counters remain solid islands.
    assert!(
        membership(&s, &strokes, 29.6)
            .iter()
            .all(|m| *m == Some(false))
    );
    assert!(
        membership(&s, &counters, 29.6)
            .iter()
            .all(|m| *m == Some(true))
    );
    // Below the recess floor (29.0) the skin is intact under the strokes.
    assert!(
        membership(&s, &strokes, 28.8)
            .iter()
            .all(|m| *m == Some(true))
    );
}
