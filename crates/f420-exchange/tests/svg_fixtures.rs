//! SVG import/export (f420_exchange::svg) against hand-written fixtures and analytic
//! geometry (derived here, not read from the implementation).
use f420_exchange::ExchangeError;
use f420_exchange::svg::{self, SvgImportOptions};
use f420_sketch::{Entity, Profile, Sketch};
use std::f64::consts::PI;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/svg/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn import(text: &str, opts: &SvgImportOptions) -> Result<svg::SvgImport, ExchangeError> {
    svg::import(text.as_bytes(), opts)
}

/// Area of the profile region (outer minus holes) from exact segment areas.
fn region_area(s: &Sketch) -> f64 {
    let r = s.profile_region().unwrap();
    let area = |p: &Profile| -> f64 {
        match p {
            Profile::Circle { radius, .. } => PI * radius * radius,
            Profile::Polygon(v) => {
                (0..v.len())
                    .map(|i| {
                        let (a, b) = (v[i], v[(i + 1) % v.len()]);
                        a[0] * b[1] - b[0] * a[1]
                    })
                    .sum::<f64>()
                    / 2.0
            }
            Profile::Loop(segs) => {
                let n = segs.len();
                let mut a2 = 0.0;
                let mut seg = 0.0;
                for i in 0..n {
                    let (p, q) = (segs[i].start(), segs[(i + 1) % n].start());
                    a2 += p[0] * q[1] - q[0] * p[1];
                    if let f420_sketch::LoopSegment::Arc { center, ccw, .. } = segs[i] {
                        let r = (p[0] - center[0]).hypot(p[1] - center[1]);
                        let a0 = (p[1] - center[1]).atan2(p[0] - center[0]);
                        let a1 = (q[1] - center[1]).atan2(q[0] - center[0]);
                        let mut th = (a1 - a0).rem_euclid(2.0 * PI);
                        if !ccw {
                            th -= 2.0 * PI;
                        }
                        seg += 0.5 * r * r * (th - th.sin());
                    }
                }
                a2 / 2.0 + seg
            }
        }
    };
    area(&r.outer) - r.holes.iter().map(|(p, _)| area(p)).sum::<f64>()
}

#[test]
fn plate_fixture_imports_with_units_flip_and_region() {
    let imp = svg::import(&fixture("plate_mm.svg"), &SvgImportOptions::default()).unwrap();
    assert!(
        imp.units.declared && (imp.units.to_mm - 1.0).abs() < 1e-12,
        "{:?}",
        imp.units
    );
    // Outline: 55 x 50 rectangle part (x 10..65) plus a half disc of radius 25
    // bulging to +x; minus the Ø10 hole and the 10 x 6 slot.
    let want = 55.0 * 50.0 + 0.5 * PI * 625.0 - 25.0 * PI - 60.0;
    let a = region_area(&imp.sketch);
    assert!((a - want).abs() < 1e-9 * want, "{a} vs {want}");
    // y is flipped: SVG y = 55 (bottom edge in the drawing) is sketch y = -55.
    let ys: Vec<f64> = imp.sketch.points.iter().map(|p| p.y).collect();
    assert!(ys.iter().any(|y| (y + 55.0).abs() < 1e-12));
    assert!(ys.iter().all(|y| *y <= 1e-12), "{ys:?}");
    assert_eq!(imp.solve.status, f420_sketch::SolveStatus::UnderConstrained);
    // The arc bulges toward +x: its sketch centre is (65, -30) and it is CCW
    // from (65, -55) to (65, -5).
    let arc = imp.sketch.entities.iter().find_map(|e| match e {
        Entity::Arc {
            center, start, end, ..
        } => Some((
            imp.sketch.point_xy(*center).unwrap(),
            imp.sketch.point_xy(*start).unwrap(),
            imp.sketch.point_xy(*end).unwrap(),
        )),
        _ => None,
    });
    let (c, s, e) = arc.unwrap();
    assert!(
        (c[0] - 65.0).abs() < 1e-9 && (c[1] + 30.0).abs() < 1e-9,
        "{c:?}"
    );
    assert!(
        (s[1] + 55.0).abs() < 1e-9 && (e[1] + 5.0).abs() < 1e-9,
        "{s:?} {e:?}"
    );
}

#[test]
fn unitless_needs_a_choice_and_units_scale() {
    let doc = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect x="0" y="0" width="4" height="2"/></svg>"#;
    assert!(matches!(
        import(doc, &SvgImportOptions::default()),
        Err(ExchangeError::UnitsRequired { .. })
    ));
    let k = import(
        doc,
        &SvgImportOptions {
            unit_override_to_mm: Some(25.4),
            ..Default::default()
        },
    )
    .unwrap();
    assert!((region_area(&k.sketch) - 8.0 * 25.4 * 25.4).abs() < 1e-9);
    // 2 in wide drawing with a 2-unit viewBox: 1 user unit = 25.4 mm.
    let inch = r#"<svg xmlns="http://www.w3.org/2000/svg" width="2in" height="1in" viewBox="0 0 2 1"><circle cx="1" cy="0.5" r="0.25"/></svg>"#;
    let i = import(inch, &SvgImportOptions::default()).unwrap();
    let Entity::Circle { radius, .. } = i.sketch.entities[0] else {
        panic!()
    };
    assert!((radius - 0.25 * 25.4).abs() < 1e-12 && (i.units.to_mm - 25.4).abs() < 1e-12);
}

#[test]
fn transforms_compose_and_skew_on_circles_is_unsupported() {
    // rotate(90) then translate: (10, 0) -> rotate -> (0, 10) -> +(5, 5) = (5, 15),
    // flipped to sketch (5, -15).
    let doc = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">
      <g transform="translate(5,5) rotate(90)"><line x1="0" y1="0" x2="10" y2="0"/></g></svg>"#;
    let s = import(doc, &SvgImportOptions::default()).unwrap().sketch;
    assert!(
        s.points
            .iter()
            .any(|p| (p.x - 5.0).abs() < 1e-12 && (p.y + 15.0).abs() < 1e-12),
        "{:?}",
        s.points
    );
    let skew = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm" viewBox="0 0 10 10">
      <g transform="scale(2,1)"><circle cx="2" cy="2" r="1"/></g><line x1="0" y1="0" x2="1" y2="0"/></svg>"#;
    match import(skew, &SvgImportOptions::default()) {
        Err(ExchangeError::Unsupported { entities, .. }) => {
            assert_eq!(entities[0].entity_type, "circle")
        }
        r => panic!("{r:?}"),
    }
    let part = import(
        skew,
        &SvgImportOptions {
            allow_partial: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(part.sketch.entities.len(), 1);
    assert!(part.skipped.iter().any(|u| u.entity_type == "circle"));
}

#[test]
fn beziers_text_and_doctype_are_reported() {
    let doc = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm" viewBox="0 0 10 10">
      <path d="M0 0 L5 0 C6 1 7 2 8 3 Z"/><text x="1" y="1">hi</text></svg>"#;
    match import(doc, &SvgImportOptions::default()) {
        Err(ExchangeError::Unsupported { entities, .. }) => {
            let kinds: Vec<_> = entities.iter().map(|e| e.entity_type.as_str()).collect();
            assert!(
                kinds.contains(&"path") && kinds.contains(&"text"),
                "{kinds:?}"
            );
        }
        r => panic!("{r:?}"),
    }
    let p = import(
        doc,
        &SvgImportOptions {
            allow_partial: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        p.warnings.iter().any(|w| w.contains("open endpoint")),
        "{:?}",
        p.warnings
    );
    let dt = r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY x "y">]><svg xmlns="http://www.w3.org/2000/svg"/>"#;
    assert!(matches!(
        import(dt, &SvgImportOptions::default()),
        Err(ExchangeError::Unsupported { .. })
    ));
    assert!(import("<svg", &SvgImportOptions::default()).is_err());
    assert!(import("<html/>", &SvgImportOptions::default()).is_err());
}

#[test]
fn export_then_import_round_trips_geometry() {
    let imp = svg::import(&fixture("plate_mm.svg"), &SvgImportOptions::default()).unwrap();
    let text = svg::export(&imp.sketch).unwrap();
    let back = svg::import(text.as_bytes(), &SvgImportOptions::default()).unwrap();
    let (a0, a1) = (region_area(&imp.sketch), region_area(&back.sketch));
    assert!((a0 - a1).abs() < 1e-9 * a0, "{a0} vs {a1}");
    assert_eq!(back.sketch.entities.len(), imp.sketch.entities.len());
    // Same point set (export keeps local coordinates).
    for p in &imp.sketch.points[1..] {
        assert!(
            back.sketch
                .points
                .iter()
                .any(|q| (q.x - p.x).abs() < 1e-9 && (q.y - p.y).abs() < 1e-9),
            "{p:?}"
        );
    }
}

#[test]
fn malformed_inputs_never_panic() {
    let base = std::fs::read_to_string(format!(
        "{}/tests/fixtures/svg/plate_mm.svg",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    for n in 0..base.len() {
        if base.is_char_boundary(n) {
            let _ = import(&base[..n], &SvgImportOptions::default());
        }
    }
    for bad in [
        "M",
        "M 1",
        "M 1 2 A 1 1 0 2 0 3 4",
        "M 1 2 Q",
        "L 1 2",
        "M 1e999 2 L 3 4",
    ] {
        let doc = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="1mm" height="1mm" viewBox="0 0 1 1"><path d="{bad}"/></svg>"#
        );
        assert!(import(&doc, &SvgImportOptions::default()).is_err(), "{bad}");
    }
}

#[test]
fn freecad_generated_svg_imports_with_freecad_area() {
    // tools/fixtures/svg/generate_freecad_svg.py (FreeCAD wires exported by Draft):
    // 60x40 outline, Ø12 hole at (20, 20), slot 10 long x 8 wide with r4 ends.
    let imp = svg::import(&fixture("freecad_plate.svg"), &SvgImportOptions::default()).unwrap();
    let a = region_area(&imp.sketch);
    let analytic = 60.0 * 40.0 - 36.0 * PI - (10.0 * 8.0 + 16.0 * PI);
    assert!((a - analytic).abs() < 1e-9 * a, "{a} vs {analytic}");
    // translate(0.6, 40.6) scale(1, -1) then the y flip: sketch = FreeCAD + (0.6, -40.6).
    assert!(
        imp.sketch
            .points
            .iter()
            .any(|p| (p.x - 60.6).abs() < 1e-12 && (p.y + 0.6).abs() < 1e-12)
    );
    assert_eq!(imp.sketch.profile_region().unwrap().holes.len(), 2);
}
