//! Sketch text through the command API (run 6, SK04): add/edit/remove text,
//! raised and recessed glyph regions with counters, save/reload as format v6
//! without needing the font, undo. Volumes are analytic: plate box plus or
//! minus (glyph region area from the sketch polygons) × height. The polygon
//! areas are the exact areas of the profiles the kernel extrudes (the font
//! curves themselves are approximated within TEXT_OUTLINE_TOL, docs/TOLERANCES.md).
//!
//! Needs the installed font "Liberation Sans"; `RCAD_SKIP_FONT_TESTS=1` skips.

use rcad_commands::{Command, ExtrudeOp, Response, Session, SketchTemplate, end_cap_of};
use rcad_document::{FeatureId, FeatureKind, SketchPlane};
use rcad_kernel::VolumeResult;
use rcad_sketch::{FontSpec, HAlign, RegionRef, Sketch, SketchRegion, TextSpec, VAlign};

const FAMILY: &str = "Liberation Sans";

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
    }
}

fn volume(s: &Session) -> f64 {
    match s.summary().bodies[0].measurements.clone().unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn sketch_of(s: &Session, f: FeatureId) -> Sketch {
    match &s.model().unwrap().document().feature(f).unwrap().kind {
        FeatureKind::Sketch { sketch, .. } => sketch.clone(),
        _ => panic!(),
    }
}

/// Outermost text regions (the glyph bodies with their counters as holes).
fn glyph_regions(s: &Session, f: FeatureId) -> Vec<SketchRegion> {
    let mut sk = sketch_of(s, f);
    sk.solve();
    sk.regions()
        .unwrap()
        .into_iter()
        .filter(|r| r.depth == 0)
        .collect()
}

fn text_spec(text: &str, h: f64) -> TextSpec {
    let mut t = TextSpec::new(text, FAMILY, h, [0.0, 0.0]);
    t.halign = HAlign::Center;
    t.valign = VAlign::Middle;
    t
}

/// Plate 60 x 30 x 3 on XY; an empty sketch on its top face. Returns the text sketch.
fn plate(s: &mut Session) -> FeatureId {
    ok(s, Command::NewDocument);
    let base = ok(
        s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 60.0,
                height: 30.0,
                center: None,
            },
        },
    )
    .feature
    .unwrap();
    let ext = ok(s, extrude(base, 3.0, ExtrudeOp::NewBody, None))
        .feature
        .unwrap();
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

#[test]
fn raised_b8_joins_glyphs_with_counters() {
    if !have_font() {
        return;
    }
    let mut s = Session::new();
    let label = plate(&mut s);
    let r = ok(
        &mut s,
        Command::AddSketchText {
            feature: label,
            text: text_spec("B8", 12.0),
        },
    );
    assert_eq!(r.text, Some(1));
    let glyphs = glyph_regions(&s, label);
    assert_eq!(glyphs.len(), 2);
    assert!(glyphs.iter().all(|g| g.region.holes.len() == 2));
    let mut expect = 60.0 * 30.0 * 3.0;
    for g in &glyphs {
        ok(
            &mut s,
            extrude(label, 1.0, ExtrudeOp::Join, Some(g.id.clone())),
        );
        expect += g.area;
    }
    let v = volume(&s);
    assert!((v - expect).abs() < 1e-6 * expect, "{v} vs {expect}");
    assert_eq!(s.summary().bodies.len(), 1);
    // STEP export of the raised text (independent validation: copy it out with
    // RCAD_EVIDENCE_DIR=<dir> and check it with tools/freecad_check_step.py).
    let dir = tempfile::tempdir().unwrap();
    let step = dir.path().join("raised_b8.step");
    ok(
        &mut s,
        Command::ExportStep {
            path: step.clone(),
            bodies: None,
        },
    );
    assert!(std::fs::metadata(&step).unwrap().len() > 0);
    if let Ok(out) = std::env::var("RCAD_EVIDENCE_DIR") {
        std::fs::create_dir_all(&out).unwrap();
        std::fs::copy(&step, std::path::Path::new(&out).join("raised_b8.step")).unwrap();
        std::fs::write(
            std::path::Path::new(&out).join("raised_b8.expected.txt"),
            format!("volume {expect}\n"),
        )
        .unwrap();
    }

    // Placement edit (height): the same glyph regions regenerate, scaled.
    let mut t = text_spec("B8", 12.0);
    t.height = 10.0;
    ok(
        &mut s,
        Command::EditSketchText {
            feature: label,
            id: 1,
            text: t,
        },
    );
    let scaled: f64 = glyph_regions(&s, label).iter().map(|g| g.area).sum();
    let v2 = volume(&s);
    let e2 = 60.0 * 30.0 * 3.0 + scaled;
    assert!((v2 - e2).abs() < 1e-6 * e2, "{v2} vs {e2}");
    let k2 = (10.0f64 / 12.0).powi(2);
    let orig: f64 = glyphs.iter().map(|g| g.area).sum();
    assert!(
        (scaled - k2 * orig).abs() < 0.01 * orig,
        "{scaled} vs {}",
        k2 * orig
    );

    // Undo restores the 12 mm text and its volume.
    ok(&mut s, Command::Undo);
    assert!((volume(&s) - expect).abs() < 1e-6 * expect);
}

#[test]
fn recessed_text_cuts_and_reloads_without_the_font() {
    if !have_font() {
        return;
    }
    let mut s = Session::new();
    let label = plate(&mut s);
    ok(
        &mut s,
        Command::AddSketchText {
            feature: label,
            text: text_spec("B", 15.0),
        },
    );
    let g = glyph_regions(&s, label).remove(0);
    ok(
        &mut s,
        extrude(label, 1.0, ExtrudeOp::Cut, Some(g.id.clone())),
    );
    let expect = 60.0 * 30.0 * 3.0 - g.area;
    let v = volume(&s);
    assert!((v - expect).abs() < 1e-6 * expect, "{v} vs {expect}");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("label.rcad.json");
    ok(&mut s, Command::Save { path: path.clone() });
    let raw = std::fs::read_to_string(&path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(json["version"], 6, "sketch text needs format v6");
    // Another machine without the font: the requested family is unknown there.
    let moved = dir.path().join("moved.rcad.json");
    std::fs::write(&moved, raw.replace(FAMILY, "Font Not Installed Here")).unwrap();
    let mut s2 = Session::new();
    ok(&mut s2, Command::Open { path: moved });
    let v2 = volume(&s2);
    assert!((v2 - expect).abs() < 1e-6 * expect, "{v2}");
    // Placement edits still work; string edits are refused with the reason.
    let mut t = sketch_of(&s2, label).texts[0].spec.clone();
    t.position = [2.0, 1.0];
    ok(
        &mut s2,
        Command::EditSketchText {
            feature: label,
            id: 1,
            text: t.clone(),
        },
    );
    assert!((volume(&s2) - expect).abs() < 1e-6 * expect);
    t.text = "C".into();
    let r = s2.execute(&Command::EditSketchText {
        feature: label,
        id: 1,
        text: t,
    });
    assert!(!r.ok);
    let msg = format!("{:?}", r.error);
    assert!(msg.contains("font not found"), "{msg}");
    assert_eq!(sketch_of(&s2, label).texts[0].spec.text, "B");

    // A file claiming v5 cannot hold sketch text.
    let v5 = dir.path().join("v5.rcad.json");
    std::fs::write(&v5, raw.replace("\"version\": 6", "\"version\": 5")).unwrap();
    let r = Session::new().execute(&Command::Open { path: v5 });
    assert!(
        !r.ok && format!("{:?}", r.error).contains("v6 feature"),
        "{r:?}"
    );
}

#[test]
fn removing_text_breaks_the_glyph_reference_and_documents_without_text_stay_v4() {
    if !have_font() {
        return;
    }
    let mut s = Session::new();
    let label = plate(&mut s);
    let dir = tempfile::tempdir().unwrap();
    let p4 = dir.path().join("no_text.rcad.json");
    ok(&mut s, Command::Save { path: p4.clone() });
    let j: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p4).unwrap()).unwrap();
    assert_eq!(j["version"], 4);
    ok(
        &mut s,
        Command::AddSketchText {
            feature: label,
            text: text_spec("O", 10.0),
        },
    );
    let g = glyph_regions(&s, label).remove(0);
    ok(&mut s, extrude(label, 2.0, ExtrudeOp::Join, Some(g.id)));
    // Removing the text breaks the extrusion's region reference: refused, unchanged.
    let r = s.execute(&Command::RemoveSketchText {
        feature: label,
        id: 1,
    });
    assert!(!r.ok, "{r:?}");
    assert_eq!(sketch_of(&s, label).texts.len(), 1);
    let r = s.execute(&Command::RemoveSketchText {
        feature: label,
        id: 7,
    });
    assert!(!r.ok);
    // JSON shape of the command (CLI/scripts).
    let c: Command = serde_json::from_str(
        r#"{"cmd":"add_sketch_text","feature":3,"text":{"text":"B8","font":{"family":"Liberation Sans"},"height":8,"position":[0,0]}}"#,
    )
    .unwrap();
    assert!(matches!(c, Command::AddSketchText { .. }));
}
