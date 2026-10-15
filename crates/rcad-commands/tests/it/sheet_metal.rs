//! Sheet-metal contour flange (acceptance project "sheet-metal U-channel"):
//! width 40, thickness 1, inside radius 2, K 0.4, two 90° bends, straights
//! between tangent points 60 + 20 + 20. Flat length = 100 + 2·(π/2)·(2 + 0.4·1) = 107.539822 mm.
//! The sketch is the inner surface: segment lengths = straights + setbacks
//! (R tan 45° = 2 per bend). Folded volume = w·t·Σ straights + w·Σ θ/2·((R+t)² − R²).
//! The DXF flat pattern is read back independently (line entities parsed here).
use crate::support;
use rcad_commands::{Command, Session, SketchTemplate};
use rcad_document::SketchPlane;
use rcad_sketch::Sketch;
use serde_json::json;
use std::f64::consts::PI;
use support::tray;

/// U profile, inner surface: legs up, base along X; material outside
/// (right of the direction (0,22)->(0,0)->(64,0)->(64,22) is outside).
fn u_sketch(base: f64, leg: f64) -> Sketch {
    let mut s = Sketch::default();
    let p: Vec<_> = [[0.0, leg], [0.0, 0.0], [base, 0.0], [base, leg]]
        .iter()
        .map(|q| s.add_point(q[0], q[1]))
        .collect();
    for i in 0..3 {
        s.add_line(p[i], p[i + 1]);
    }
    s
}

fn folded_volume(w: f64, t: f64, r: f64, straights: f64, bends: usize) -> f64 {
    w * t * straights + w * bends as f64 * (PI / 4.0) * ((r + t).powi(2) - r * r)
}

fn dxf_lines(text: &str) -> Vec<(String, [f64; 4])> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let mut out = vec![];
    let mut i = 0;
    while i + 1 < lines.len() {
        if lines[i] == "0" && lines[i + 1] == "LINE" {
            let (mut layer, mut v) = (String::new(), [0.0; 4]);
            let mut j = i + 2;
            while j + 1 < lines.len() && lines[j] != "0" {
                match lines[j] {
                    "8" => layer = lines[j + 1].to_string(),
                    "10" => v[0] = lines[j + 1].parse().unwrap(),
                    "20" => v[1] = lines[j + 1].parse().unwrap(),
                    "11" => v[2] = lines[j + 1].parse().unwrap(),
                    "21" => v[3] = lines[j + 1].parse().unwrap(),
                    _ => {}
                }
                j += 2;
            }
            out.push((layer, v));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

#[test]
fn u_channel_folded_and_flat() {
    let mut s = Session::new();
    tray::ok(&mut s, &json!({"cmd": "new_document"}));
    let r = s.execute(&Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::Custom {
            sketch: u_sketch(64.0, 22.0),
        },
    });
    assert!(r.ok, "{:?}", r.error);
    tray::ok(
        &mut s,
        &json!({"cmd": "sheet_metal", "sketch": 1, "thickness": 1.0, "radius": 2.0,
                "k_factor": 0.4, "width": 40.0, "body_name": "Channel"}),
    );
    let want = folded_volume(40.0, 1.0, 2.0, 100.0, 2);
    assert!(
        (tray::volume(&s) - want).abs() < 1e-6,
        "{} vs {want}",
        tray::volume(&s)
    );
    // Section probes: the base plate lies below y = 0 (material outside),
    // the bend is a quarter tube about (2, 2).
    let c = std::f64::consts::FRAC_1_SQRT_2;
    let p = tray::probe(
        &s,
        &[
            [32.0, -0.5, 20.0],
            [32.0, 0.5, 20.0],
            [2.0 - 2.5 * c, 2.0 - 2.5 * c, 20.0],
            [2.0 - 1.5 * c, 2.0 - 1.5 * c, 20.0],
            [-0.5, 15.0, 20.0],
        ],
    );
    assert_eq!(
        p,
        vec![Some(true), Some(false), Some(true), Some(false), Some(true)]
    );
    // Flat pattern DXF, read back.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("u.dxf");
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "export_flat_pattern", "feature": 2, "path": path}),
    );
    let flat = 100.0 + 2.0 * (PI / 2.0) * (2.0 + 0.4 * 1.0);
    assert!((flat - 107.539822).abs() < 1e-6);
    assert!(r.message.unwrap().contains("107.539822"));
    let text = std::fs::read_to_string(&path).unwrap();
    let lines = dxf_lines(&text);
    let cut: Vec<_> = lines.iter().filter(|(l, _)| l == "CUT").collect();
    let bend: Vec<_> = lines.iter().filter(|(l, _)| l == "BEND").collect();
    assert_eq!(cut.len(), 4);
    assert_eq!(bend.len(), 2);
    let maxx = cut
        .iter()
        .flat_map(|(_, v)| [v[0], v[2]])
        .fold(f64::MIN, f64::max);
    assert!((maxx - flat).abs() < 1e-9, "{maxx}");
    // Bend centre lines: 20 + BA/2 and 20 + BA + 60 + BA/2.
    let ba = (PI / 2.0) * 2.4;
    let mut xs: Vec<f64> = bend.iter().map(|(_, v)| v[0]).collect();
    xs.sort_by(f64::total_cmp);
    assert!((xs[0] - (20.0 + ba / 2.0)).abs() < 1e-9 && (xs[1] - (80.0 + 1.5 * ba)).abs() < 1e-9);
    // Rule edits: thickness 2, radius 3 → recompute both.
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "thickness", "value": 2.0}),
    );
    tray::ok(
        &mut s,
        &json!({"cmd": "set_parameter", "feature": 2, "name": "radius", "value": 3.0}),
    );
    // Straights shrink by the larger setbacks: segments 22/64/22 minus 3 per bend.
    let want2 = folded_volume(40.0, 2.0, 3.0, 19.0 + 58.0 + 19.0, 2);
    assert!(
        (tray::volume(&s) - want2).abs() < 1e-6,
        "{}",
        tray::volume(&s)
    );
    let r = tray::ok(
        &mut s,
        &json!({"cmd": "export_flat_pattern", "feature": 2, "path": path}),
    );
    let flat2 = 96.0 + 2.0 * (PI / 2.0) * (3.0 + 0.4 * 2.0);
    assert!(r.message.unwrap().contains(&format!("{flat2:.6}")));
    // Save / reopen.
    let p2 = dir.path().join("u.rcad.json");
    tray::ok(&mut s, &json!({"cmd": "save", "path": p2}));
    let mut t = Session::new();
    tray::ok(&mut t, &json!({"cmd": "open", "path": p2}));
    assert!((tray::volume(&t) - want2).abs() < 1e-6);
}

#[test]
fn invalid_flanges_are_refused() {
    let mut s = Session::new();
    tray::ok(&mut s, &json!({"cmd": "new_document"}));
    // A third leg folding back over the first: self-overlapping.
    let mut sk = Sketch::default();
    let p: Vec<_> = [
        [0.0, 22.0],
        [0.0, 0.0],
        [30.0, 0.0],
        [30.0, 10.0],
        [-10.0, 10.0],
    ]
    .iter()
    .map(|q| sk.add_point(q[0], q[1]))
    .collect();
    for i in 0..4 {
        sk.add_line(p[i], p[i + 1]);
    }
    let r = s.execute(&Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::Custom { sketch: sk },
    });
    assert!(r.ok);
    let r = tray::exec(
        &mut s,
        &json!({"cmd": "sheet_metal", "sketch": 1, "thickness": 1.0, "radius": 2.0, "width": 40.0}),
    );
    assert!(!r.ok, "self-overlapping flange accepted");
    // Too-large radius for a short flange; bad K-factor.
    let r2 = s.execute(&Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::Custom {
            sketch: u_sketch(64.0, 22.0),
        },
    });
    assert!(r2.ok);
    for v in [
        json!({"cmd": "sheet_metal", "sketch": 2, "thickness": 1.0, "radius": 30.0, "width": 40.0}),
        json!({"cmd": "sheet_metal", "sketch": 2, "thickness": 1.0, "radius": 2.0, "k_factor": 1.5, "width": 40.0}),
        json!({"cmd": "sheet_metal", "sketch": 2, "thickness": 0.0, "radius": 2.0, "width": 40.0}),
    ] {
        let r = tray::exec(&mut s, &v);
        assert!(!r.ok, "{v} accepted");
    }
    assert!(s.summary().bodies.is_empty());
}
