//! Run 7 sheet metal (docs/SHEET_METAL.md): base face + edge flanges, sheet
//! rule, flat pattern state and DXF. Acceptance project 6 (U-channel) built
//! as a 60 × 40 base face with two 90° up flanges of length 20 on its 40 mm
//! edges, t 1, inside radius 2, K 0.4.
//!
//! Length convention (stated): a flange's `length` is the straight leg
//! beyond the bend, from the bend's tangent line to the flange end — the
//! "between tangent points" convention of SESSION_7 acceptance 6. So the
//! channel's straights are 60 + 20 + 20 and its neutral-axis flat length is
//! 100 + 2·(π/2)·(2 + 0.4·1) = 107.539822 mm (closed form here; the DXF is
//! parsed independently below and checked by FreeCAD in
//! tools/validate_sheet_flat.sh).
mod support;
use rcad_commands::{Command, Session, SketchTemplate};
use rcad_document::{BodyId, SketchPlane};
use rcad_kernel::VolumeResult;
use serde_json::json;
use std::f64::consts::PI;
use support::tray;

fn rect_sketch(s: &mut Session, w: f64, h: f64) {
    let r = s.execute(&Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: w,
            height: h,
            center: Some([w / 2.0, h / 2.0]),
        },
    });
    assert!(r.ok, "{:?}", r.error);
}

/// Id of the non-construction line of sketch feature `f` whose end points
/// both satisfy `pred`.
fn line_id(s: &Session, f: u32, pred: impl Fn([f64; 2]) -> bool) -> u32 {
    let sum = s.summary();
    let sk = sum
        .features
        .iter()
        .find(|x| x.id.0 == f)
        .and_then(|x| x.sketch.clone())
        .expect("sketch");
    let pt = |p| {
        let q = sk.points.iter().find(|q| q.id == p).unwrap();
        [q.x, q.y]
    };
    sk.entities
        .iter()
        .find_map(|e| match e {
            rcad_sketch::Entity::Line { id, a, b }
                if !sk.construction.contains(&e.id()) && pred(pt(*a)) && pred(pt(*b)) =>
            {
                Some(id.0)
            }
            _ => None,
        })
        .expect("line")
}

fn body_volume(s: &Session, id: u32) -> f64 {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id == BodyId(id))
        .unwrap_or_else(|| panic!("body {id}"));
    match b.measurements.expect("measured").volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn bounds(s: &Session, id: u32) -> ([f64; 3], [f64; 3]) {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id == BodyId(id))
        .unwrap();
    let m = b.measurements.unwrap();
    (m.bounds.min, m.bounds.max)
}

fn probe(s: &Session, id: u32, pts: &[[f64; 3]]) -> Vec<Option<bool>> {
    let m = s.model().unwrap();
    let b = m
        .evaluation()
        .body_by_id(BodyId(id))
        .unwrap()
        .geometry
        .brep()
        .unwrap();
    rcad_kernel::point_membership(b, pts).unwrap()
}

/// Closed form: plate + n quarter... bends of angle θ + legs, width w.
fn folded(w: f64, t: f64, r: f64, straights: f64, bend_angles: &[f64]) -> f64 {
    w * t * straights
        + bend_angles
            .iter()
            .map(|th| w * th / 2.0 * ((r + t).powi(2) - r * r))
            .sum::<f64>()
}

type DxfLines = Vec<(String, [f64; 4])>;
type DxfTexts = Vec<(String, String)>;

/// LINE / TEXT entities (layer, x0, y0, x1, y1 / text) of an R12 DXF.
fn dxf(text: &str) -> (DxfLines, DxfTexts, Option<String>) {
    let l: Vec<&str> = text.lines().map(str::trim).collect();
    let (mut lines, mut texts, mut units) = (vec![], vec![], None);
    let mut i = 0;
    while i + 1 < l.len() {
        if l[i] == "9" && l[i + 1] == "$INSUNITS" {
            units = Some(l[i + 3].to_string());
        }
        if l[i] == "0" && (l[i + 1] == "LINE" || l[i + 1] == "TEXT") {
            let kind = l[i + 1];
            let (mut layer, mut v, mut t) = (String::new(), [0.0; 4], String::new());
            let mut j = i + 2;
            while j + 1 < l.len() && l[j] != "0" {
                match l[j] {
                    "8" => layer = l[j + 1].into(),
                    "10" => v[0] = l[j + 1].parse().unwrap(),
                    "20" => v[1] = l[j + 1].parse().unwrap(),
                    "11" => v[2] = l[j + 1].parse().unwrap(),
                    "21" => v[3] = l[j + 1].parse().unwrap(),
                    "1" => t = l[j + 1].into(),
                    _ => {}
                }
                j += 2;
            }
            if kind == "LINE" {
                lines.push((layer, v));
            } else {
                texts.push((layer, t));
            }
            i = j;
        } else {
            i += 1;
        }
    }
    (lines, texts, units)
}

/// U-channel: base 60 × 40 (F1 sketch, F2 base, body 1), flanges on x = 0
/// and x = 60 (F3, F4).
fn channel() -> Session {
    let mut s = Session::new();
    tray::ok(&mut s, &json!({"cmd": "new_document"}));
    rect_sketch(&mut s, 60.0, 40.0);
    tray::ok(
        &mut s,
        &json!({"cmd": "sheet_base", "sketch": 1, "thickness": 1.0, "radius": 2.0,
                "k_factor": 0.4, "body_name": "Channel"}),
    );
    let left = line_id(&s, 1, |p| p[0].abs() < 1e-9);
    let right = line_id(&s, 1, |p| (p[0] - 60.0).abs() < 1e-9);
    for e in [left, right] {
        tray::ok(
            &mut s,
            &json!({"cmd": "sheet_flange", "base": 2, "edge": e, "angle_deg": 90.0, "length": 20.0}),
        );
    }
    s
}

#[test]
fn u_channel_base_and_flanges() {
    let mut s = channel();
    let half = PI / 2.0;
    let want = folded(40.0, 1.0, 2.0, 100.0, &[half, half]);
    assert!((want - (4000.0 + 100.0 * PI)).abs() < 1e-9);
    let v = body_volume(&s, 1);
    assert!((v - want).abs() < 1e-6, "{v} vs {want}");
    let (lo, hi) = bounds(&s, 1);
    let eq = |a: [f64; 3], b: [f64; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9);
    assert!(
        eq(lo, [-3.0, 0.0, 0.0]) && eq(hi, [63.0, 40.0, 23.0]),
        "{lo:?} {hi:?}"
    );
    let c = std::f64::consts::FRAC_1_SQRT_2;
    let p = probe(
        &s,
        1,
        &[
            [30.0, 20.0, 0.5],                     // plate
            [30.0, 20.0, 1.5],                     // above the plate
            [62.5, 20.0, 13.0],                    // right leg
            [61.5, 20.0, 13.0],                    // inside the channel
            [60.0 + 2.5 * c, 20.0, 3.0 - 2.5 * c], // right bend
            [-2.5, 20.0, 13.0],                    // left leg
            [-3.5, 20.0, 13.0],                    // outside
        ],
    );
    assert_eq!(
        p,
        vec![
            Some(true),
            Some(false),
            Some(true),
            Some(false),
            Some(true),
            Some(true),
            Some(false)
        ]
    );
    // Flat pattern DXF (independent parse).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("u.dxf");
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "export_flat_pattern", "feature": 4, "path": path}),
    );
    let flat = 100.0 + 2.0 * half * (2.0 + 0.4 * 1.0);
    assert!((flat - 107.539822).abs() < 1e-6);
    assert!(r.message.unwrap().contains("107.539822 x 40.000000"));
    let (lines, texts, units) = dxf(&std::fs::read_to_string(&path).unwrap());
    assert_eq!(units.as_deref(), Some("4"), "$INSUNITS mm");
    let cut: Vec<_> = lines.iter().filter(|(l, _)| l == "CUT").collect();
    let up: Vec<_> = lines.iter().filter(|(l, _)| l == "BEND_UP").collect();
    assert_eq!(cut.len(), 4, "{cut:?}");
    assert_eq!(up.len(), 2);
    assert!(lines.iter().all(|(l, _)| l != "BEND_DOWN"));
    let ba = half * 2.4;
    let xs: Vec<f64> = cut.iter().flat_map(|(_, v)| [v[0], v[2]]).collect();
    let (mn, mx) = xs
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    assert!(
        (mn + 20.0 + ba).abs() < 1e-9 && (mx - 80.0 - ba).abs() < 1e-9,
        "{mn} {mx}"
    );
    assert!(((mx - mn) - flat).abs() < 1e-9);
    let mut bx: Vec<f64> = up.iter().map(|(_, v)| v[0]).collect();
    bx.sort_by(f64::total_cmp);
    assert!((bx[0] + ba / 2.0).abs() < 1e-9 && (bx[1] - 60.0 - ba / 2.0).abs() < 1e-9);
    assert!(
        up.iter()
            .all(|(_, v)| v[1].min(v[3]).abs() < 1e-12 && (v[1].max(v[3]) - 40.0).abs() < 1e-12)
    );
    assert_eq!(texts.len(), 2);
    assert!(
        texts
            .iter()
            .all(|(l, t)| l == "BEND_NOTES" && t.starts_with("UP 90.0"))
    );
    // Flat state: the exact flat solid, folded body hidden.
    tray::ok(
        &mut s,
        &json!({"cmd": "sheet_flat_state", "base": 2, "flat": true}),
    );
    let sum = s.summary();
    let flat_body = sum.bodies.iter().find(|b| b.id != BodyId(1)).unwrap();
    assert!(flat_body.visible);
    assert!(
        !sum.bodies
            .iter()
            .find(|b| b.id == BodyId(1))
            .unwrap()
            .visible
    );
    let fv = body_volume(&s, flat_body.id.0);
    assert!((fv - flat * 40.0).abs() < 1e-6, "{fv}");
    let (flo, fhi) = bounds(&s, flat_body.id.0);
    assert!((flo[0] + 20.0 + ba).abs() < 1e-9 && (fhi[0] - 80.0 - ba).abs() < 1e-9);
    tray::ok(
        &mut s,
        &json!({"cmd": "sheet_flat_state", "base": 2, "flat": false}),
    );
    assert!(
        s.summary()
            .bodies
            .iter()
            .find(|b| b.id == BodyId(1))
            .unwrap()
            .visible
    );
}

#[test]
fn rule_edit_regenerates_flanges_and_flat() {
    let mut s = channel();
    tray::ok(
        &mut s,
        &json!({"cmd": "edit_sheet_rule", "base": 2, "thickness": 2.0, "radius": 3.0}),
    );
    let half = PI / 2.0;
    let want = folded(40.0, 2.0, 3.0, 100.0, &[half, half]);
    assert!((body_volume(&s, 1) - want).abs() < 1e-6);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("u2.dxf");
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "export_flat_pattern", "feature": 2, "path": path}),
    );
    let flat = 100.0 + 2.0 * half * (3.0 + 0.4 * 2.0);
    assert!(
        r.message
            .as_ref()
            .unwrap()
            .contains(&format!("{flat:.6} x 40")),
        "{r:?}"
    );
    // Rule parameters are ordinary feature parameters too (K-factor edit).
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "k_factor", "value": 0.5}),
    );
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "export_flat_pattern", "feature": 3, "path": path}),
    );
    let flat = 100.0 + 2.0 * half * (3.0 + 0.5 * 2.0);
    assert!(r.message.unwrap().contains(&format!("{flat:.6} x 40")));
    // Undo / redo restore the rule and the geometry.
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    tray::ok(&mut s, &json!({"cmd": "undo"}));
    let v0 = folded(40.0, 1.0, 2.0, 100.0, &[half, half]);
    assert!((body_volume(&s, 1) - v0).abs() < 1e-6);
    tray::ok(&mut s, &json!({"cmd": "redo"}));
    assert!((body_volume(&s, 1) - want).abs() < 1e-6);
    // Save / reopen (format v7) keeps the definition.
    let p = dir.path().join("u.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": p}));
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(text.contains("\"SheetFlange\"") && text.contains("\"version\": 7"));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": p}));
    assert!((body_volume(&t, 1) - want).abs() < 1e-6);
}

#[test]
fn upstream_sketch_edit_keeps_the_flanges_on_their_edges() {
    let mut s = channel();
    // Width 60 → 80: the flanged lines keep their ids (semantic reference).
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 1, "name": "width", "value": 80.0}),
    );
    let half = PI / 2.0;
    let want = folded(40.0, 1.0, 2.0, 120.0, &[half, half]);
    assert!((body_volume(&s, 1) - want).abs() < 1e-6);
    // The centred rectangle grows about its centre (30, 20): x in [-10, 70].
    let (lo, hi) = bounds(&s, 1);
    assert!(
        (lo[0] + 13.0).abs() < 1e-9 && (hi[0] - 73.0).abs() < 1e-9,
        "{lo:?} {hi:?}"
    );
}

#[test]
fn down_flange_partial_flange_with_reliefs_and_angles() {
    let mut s = Session::new();
    tray::ok(&mut s, &json!({"cmd": "new_document"}));
    rect_sketch(&mut s, 60.0, 40.0);
    tray::ok(
        &mut s,
        &json!({"cmd": "sheet_base", "sketch": 1, "thickness": 1.0, "radius": 1.0,
                "k_factor": 0.5, "relief_width": 2.0, "relief_depth": 3.0}),
    );
    let bottom = line_id(&s, 1, |p| p[1].abs() < 1e-9);
    // 45° down flange, 10 long, inset 10 at both ends: rectangular reliefs.
    tray::ok(
        &mut s,
        &json!({"cmd": "sheet_flange", "base": 2, "edge": bottom, "angle_deg": 45.0,
                "length": 10.0, "down": true, "start_offset": 10.0, "end_offset": 10.0}),
    );
    let th = PI / 4.0;
    // Plate minus two 2 × 3 notches, plus the 40-wide flange.
    let plate = 60.0 * 40.0 * 1.0 - 2.0 * (2.0 * 3.0 * 1.0);
    let want = plate + folded(40.0, 1.0, 1.0, 10.0, &[th]);
    let v = body_volume(&s, 1);
    assert!((v - want).abs() < 1e-6, "{v} vs {want}");
    // Bend down: the bend axis is at y = 0, z = -1 (inner surface is the bottom).
    let (lo, hi) = bounds(&s, 1);
    // Mirror of the up section about z = t/2: the lowest point is the inner
    // leg end, z = t − (t + R − R cos θ + L sin θ); the farthest out is the
    // outer leg end, y = −((R + t) sin θ + L cos θ).
    let zmin = 1.0 - (2.0 - th.cos() + 10.0 * th.sin());
    let ymin = -(2.0 * th.sin() + 10.0 * th.cos());
    assert!(
        (lo[2] - zmin).abs() < 1e-9 && (lo[1] - ymin).abs() < 1e-9 && (hi[2] - 1.0).abs() < 1e-9,
        "{lo:?} {hi:?}"
    );
    // Notches are empty, plate beside them is material.
    let p = probe(
        &s,
        1,
        &[
            [9.0, 1.5, 0.5],
            [9.0, 3.5, 0.5],
            [51.0, 1.5, 0.5],
            [30.0, 1.5, 0.5],
        ],
    );
    assert_eq!(p, vec![Some(false), Some(true), Some(false), Some(true)]);
    // DXF: down bend.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("d.dxf");
    tray::ok(
        &mut s,
        &json!({"cmd": "export_flat_pattern", "feature": 3, "path": path}),
    );
    let (lines, texts, _) = dxf(&std::fs::read_to_string(&path).unwrap());
    assert_eq!(lines.iter().filter(|(l, _)| l == "BEND_DOWN").count(), 1);
    assert!(texts[0].1.starts_with("DOWN 45.0"));
    // Obround reliefs (rule edit): notch = 2 × 2 rectangle + half disc r 1.
    tray::ok(
        &mut s,
        &json!({"cmd": "edit_sheet_rule", "base": 2, "relief": "obround"}),
    );
    let notch = 2.0 * 2.0 + PI / 2.0;
    let want2 = 60.0 * 40.0 - 2.0 * notch + folded(40.0, 1.0, 1.0, 10.0, &[th]);
    let v2 = body_volume(&s, 1);
    assert!((v2 - want2).abs() < 1e-6, "{v2} vs {want2}");
}

#[test]
fn invalid_flanges_and_reliefs_are_refused_atomically() {
    let mut s = channel();
    let before = body_volume(&s, 1);
    let feats = s.summary().features.len();
    let top = line_id(&s, 1, |p| (p[1] - 40.0).abs() < 1e-9);
    let left = line_id(&s, 1, |p| p[0].abs() < 1e-9);
    let bad = [
        // A second flange on an already flanged edge.
        json!({"cmd": "sheet_flange", "base": 2, "edge": left, "angle_deg": 90.0, "length": 5.0}),
        // Impossible relief: offset smaller than the relief width (t = 1).
        json!({"cmd": "sheet_flange", "base": 2, "edge": top, "length": 5.0, "start_offset": 0.5}),
        // Hem (180°) unsupported; zero length.
        json!({"cmd": "sheet_flange", "base": 2, "edge": top, "angle_deg": 180.0, "length": 5.0}),
        json!({"cmd": "sheet_flange", "base": 2, "edge": top, "length": 0.0}),
        // Not a line of the base.
        json!({"cmd": "sheet_flange", "base": 2, "edge": 999, "length": 5.0}),
        // Rule out of range.
        json!({"cmd": "edit_sheet_rule", "base": 2, "k_factor": 1.5}),
        json!({"cmd": "edit_sheet_rule", "base": 2, "relief_width": 0.0}),
    ];
    for v in &bad {
        let r = tray::exec(&mut s, v);
        assert!(!r.ok, "{v} accepted");
        assert!((body_volume(&s, 1) - before).abs() < 1e-9);
        assert_eq!(s.summary().features.len(), feats);
    }
    // Self-overlapping flanges: two 150° flanges on opposite edges lean over
    // the plate toward each other and cross at x = 30 (same heights).
    let mut o = Session::new();
    tray::ok(&mut o, &json!({"cmd": "new_document"}));
    rect_sketch(&mut o, 60.0, 40.0);
    tray::ok(
        &mut o,
        &json!({"cmd": "sheet_base", "sketch": 1, "thickness": 1.0, "radius": 2.0}),
    );
    let l = line_id(&o, 1, |p| p[0].abs() < 1e-9);
    let rr = line_id(&o, 1, |p| (p[0] - 60.0).abs() < 1e-9);
    tray::ok(
        &mut o,
        &json!({"cmd": "sheet_flange", "base": 2, "edge": l, "angle_deg": 150.0, "length": 50.0}),
    );
    let r = tray::exec(
        &mut o,
        &json!({"cmd": "sheet_flange", "base": 2, "edge": rr, "angle_deg": 150.0, "length": 50.0}),
    );
    assert!(!r.ok, "colliding flanges accepted");
    assert!(format!("{:?}", r.error).contains("overlaps flange"));
    // Shorter, they clear each other (tips at x = 1.5 + 20·cos 30° < 30).
    tray::ok(
        &mut o,
        &json!({"cmd": "sheet_flange", "base": 2, "edge": rr, "angle_deg": 150.0, "length": 20.0}),
    );
    // A deep relief through the plate is refused once a partial flange needs it.
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "new_document"}));
    rect_sketch(&mut t, 60.0, 10.0);
    tray::ok(
        &mut t,
        &json!({"cmd": "sheet_base", "sketch": 1, "thickness": 1.0, "radius": 1.0,
                "relief_depth": 12.0}),
    );
    let b = line_id(&t, 1, |p| p[1].abs() < 1e-9);
    let r = tray::exec(
        &mut t,
        &json!({"cmd": "sheet_flange", "base": 2, "edge": b, "length": 5.0,
                "start_offset": 10.0, "end_offset": 10.0}),
    );
    assert!(!r.ok);
    let msg = format!("{:?}", r.error);
    assert!(msg.contains("relief"), "{msg}");
}

#[test]
fn concave_corner_needs_an_offset() {
    // L-shaped base: the inner corner (30, 20) is concave.
    let mut s = Session::new();
    tray::ok(&mut s, &json!({"cmd": "new_document"}));
    let r = s.execute(&Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::Polygon {
            vertices: vec![
                [0.0, 0.0],
                [60.0, 0.0],
                [60.0, 20.0],
                [30.0, 20.0],
                [30.0, 40.0],
                [0.0, 40.0],
            ],
        },
    });
    assert!(r.ok);
    tray::ok(
        &mut s,
        &json!({"cmd": "sheet_base", "sketch": 1, "thickness": 1.0, "radius": 1.0}),
    );
    let inner = line_id(&s, 1, |p| (p[1] - 20.0).abs() < 1e-9 && p[0] >= 30.0);
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "sheet_flange", "base": 2, "edge": inner, "length": 5.0}),
    );
    assert!(!r.ok);
    assert!(format!("{:?}", r.error).contains("concave"));
    // With an end offset (and its relief) it is accepted.
    tray::ok(
        &mut s,
        &json!({"cmd": "sheet_flange", "base": 2, "edge": inner, "length": 5.0,
                "end_offset": 5.0}),
    );
    let base = 60.0 * 20.0 + 30.0 * 20.0 - 1.0 * 1.0; // minus one 1 × 1 relief
    let want = base + folded(25.0, 1.0, 1.0, 5.0, &[PI / 2.0]);
    let v = body_volume(&s, 1);
    assert!((v - want).abs() < 1e-6, "{v} vs {want}");
}
