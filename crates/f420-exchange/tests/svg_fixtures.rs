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

/// A 100 x 100 mm page with a 100-unit viewBox (1 unit = 1 mm) around `body`.
fn page(body: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">{body}</svg>"#
    )
}

fn partial() -> SvgImportOptions {
    SvgImportOptions {
        allow_partial: true,
        ..Default::default()
    }
}

fn line_lengths(s: &Sketch) -> Vec<f64> {
    s.entities
        .iter()
        .filter_map(|e| match e {
            Entity::Line { a, b, .. } => {
                let (p, q) = (s.point_xy(*a).unwrap(), s.point_xy(*b).unwrap());
                Some((p[0] - q[0]).hypot(p[1] - q[1]))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn data_after_closepath_is_an_error_not_a_hang() {
    // Used to spin forever: implicit repetition of Z consumed nothing. These
    // inputs are deterministic and the import returns (no timeout needed).
    for d in [
        "M0 0 L10 0 L10 10 Z 5",
        "M0 0 L10 0 L10 10 Z;",
        "M0 0 L10 0 L10 10 Z.",
        "M0 0 L10 0 L10 10 z 1 2",
    ] {
        let doc = page(&format!(r#"<path d="{d}"/>"#));
        for opts in [SvgImportOptions::default(), partial()] {
            match import(&doc, &opts) {
                Err(ExchangeError::Malformed { message, .. }) => {
                    assert!(message.contains("closepath"), "{d}: {message}")
                }
                r => panic!("{d}: {r:?}"),
            }
        }
    }
    // Commands after Z (absolute and relative) still work.
    let ok = page(r#"<path d="M0 0 L10 0 L10 10 Z m 20 0 l 5 0 l 0 5 z"/>"#);
    let s = import(&ok, &SvgImportOptions::default()).unwrap().sketch;
    assert_eq!(s.entities.len(), 6);
}

#[test]
fn lengths_accept_scientific_notation() {
    let doc = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1e2mm" height="1.0E+2mm" viewBox="0 0 100 100">
      <line x1="0" y1="0" x2="1e1" y2="0"/><line x1="0" y1="5" x2="2.5E-1cm" y2="5"/></svg>"#;
    let imp = import(doc, &SvgImportOptions::default()).unwrap();
    assert!(imp.units.declared && (imp.units.to_mm - 1.0).abs() < 1e-15);
    let mut l = line_lengths(&imp.sketch);
    l.sort_by(f64::total_cmp);
    // 2.5 mm in CSS px, then 1 user unit = 1 mm: 2.5 * 96 / 25.4 units.
    let want = 2.5 * 96.0 / 25.4;
    assert!(
        (l[0] - want).abs() < 1e-12 && (l[1] - 10.0).abs() < 1e-12,
        "{l:?}"
    );
    // `em` is a unit, not an exponent.
    let em = page(r#"<line x1="0" y1="0" x2="2em" y2="0"/><line x1="0" y1="9" x2="5" y2="9"/>"#);
    let p = import(&em, &partial()).unwrap();
    assert!(
        p.skipped
            .iter()
            .any(|u| u.entity_type == "line" && u.reason.contains("'em'"))
    );
}

#[test]
fn inkscape_namespaced_elements_are_ignored_silently() {
    let doc = r##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg"
     xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape"
     xmlns:sodipodi="http://sodipodi.sourceforge.net/DTD/sodipodi-0.dtd"
     xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
     xmlns:svg="http://www.w3.org/2000/svg"
     width="50mm" height="50mm" viewBox="0 0 50 50" inkscape:version="1.3">
  <sodipodi:namedview id="nv" pagecolor="#ffffff" inkscape:zoom="1">
    <inkscape:page x="0" y="0" width="50" height="50" id="page1"/>
  </sodipodi:namedview>
  <inkscape:clipboard/>
  <undeclared:thing/>
  <g inkscape:label="Layer 1" inkscape:groupmode="layer" id="layer1">
    <svg:rect x="5" y="5" width="20" height="10"/>
  </g>
</svg>"##;
    let imp = import(doc, &SvgImportOptions::default()).unwrap();
    assert!(imp.skipped.is_empty(), "{:?}", imp.skipped);
    let mut l = line_lengths(&imp.sketch);
    l.sort_by(f64::total_cmp);
    assert_eq!(l.len(), 4);
    assert!((l[0] - 10.0).abs() < 1e-12 && (l[3] - 20.0).abs() < 1e-12);
}

#[test]
fn relative_lengths_skip_only_their_element() {
    // A full-page background rect (common in exported files) and a circle in
    // em: unsupported per element, the rest imports.
    let doc = page(
        r#"<rect width="100%" height="100%" fill="white"/><circle cx="50" cy="50" r="2em"/>
           <rect x="0" y="0" width="auto" height="3"/><line x1="0" y1="0" x2="10" y2="0"/>"#,
    );
    match import(&doc, &SvgImportOptions::default()) {
        Err(ExchangeError::Unsupported { entities, .. }) => {
            let mut kinds: Vec<_> = entities.iter().map(|e| e.entity_type.as_str()).collect();
            kinds.sort();
            assert_eq!(kinds, ["circle", "rect", "rect"], "{entities:?}");
        }
        r => panic!("{r:?}"),
    }
    let p = import(&doc, &partial()).unwrap();
    assert_eq!(p.sketch.entities.len(), 1);
    assert!(
        p.skipped
            .iter()
            .any(|u| u.entity_type == "rect" && u.reason.contains("'%'"))
    );
    // Garbage is still malformed (not silently skipped).
    let bad = page(r#"<line x1="0" y1="0" x2="10furlong" y2="0"/>"#);
    assert!(matches!(
        import(&bad, &partial()),
        Err(ExchangeError::Malformed { .. })
    ));
}

#[test]
fn root_size_must_be_positive() {
    for (w, h) in [("-100mm", "-100mm"), ("0mm", "10mm"), ("10mm", "-1in")] {
        let doc = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 10 10"><line x1="0" y1="0" x2="5" y2="0"/></svg>"#
        );
        match import(&doc, &SvgImportOptions::default()) {
            Err(ExchangeError::Malformed { message, .. }) => {
                assert!(message.contains("must be > 0"), "{message}")
            }
            r => panic!("{w} {h}: {r:?}"),
        }
    }
}

#[test]
fn root_transform_is_refused_not_ignored() {
    let doc = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100" transform="translate(50 0)"><line x1="0" y1="0" x2="5" y2="0"/></svg>"#;
    for opts in [SvgImportOptions::default(), partial()] {
        match import(doc, &opts) {
            Err(ExchangeError::Unsupported { entities, .. }) => {
                assert!(entities[0].reason.contains("transform"), "{entities:?}")
            }
            r => panic!("{r:?}"),
        }
    }
    let id = doc.replace("translate(50 0)", "translate(0)");
    assert_eq!(
        import(&id, &SvgImportOptions::default())
            .unwrap()
            .sketch
            .entities
            .len(),
        1
    );
}

#[test]
fn near_equal_radii_and_near_similarity_transforms_are_accepted() {
    // rx/ry equal to 1e-8 relative: a circular arc of the mean radius.
    let doc = page(r#"<path d="M0 0 A10 10.0000001 0 0 1 20 0 Z"/>"#);
    let s = import(&doc, &SvgImportOptions::default()).unwrap().sketch;
    let region = s.profile_region().unwrap();
    assert!(region.holes.is_empty());
    // Minor circular segment of radius r on the 20-unit chord (r is a hair over
    // 10, so the centre sits ~1e-3 off the chord: not quite a half disc).
    let r = (10.0 + 10.0000001) / 2.0;
    let th = 2.0 * (10.0f64 / r).asin();
    let want = r * r * (th - th.sin()) / 2.0;
    assert!(
        (region_area(&s) - want).abs() < 1e-9,
        "{} vs {want}",
        region_area(&s)
    );
    // Inkscape-style px -> mm factor rounded to 8 digits (non-uniform by 4e-8)
    // around a circle and a closed D shape (arc + line sharing endpoints).
    let (kx, ky) = (0.26458333f64, 0.26458334f64);
    let doc = page(
        r#"<g transform="scale(0.26458333,0.26458334)"><circle cx="100" cy="100" r="20"/>
           <path d="M200 100 A30 30 0 0 1 260 100 Z"/></g>"#,
    );
    let imp = import(&doc, &SvgImportOptions::default()).unwrap();
    assert!(imp.skipped.is_empty(), "{:?}", imp.skipped);
    let k = (kx * ky).sqrt();
    let s = &imp.sketch;
    let r = s
        .entities
        .iter()
        .find_map(|e| match e {
            Entity::Circle { radius, .. } => Some(*radius),
            _ => None,
        })
        .unwrap();
    assert!((r - 20.0 * k).abs() < 1e-12, "{r}");
    // The arc's centre is exactly equidistant from its (shared) end points.
    let Some(Entity::Arc {
        center, start, end, ..
    }) = s.entities.iter().find(|e| matches!(e, Entity::Arc { .. }))
    else {
        panic!("no arc")
    };
    let (c, a, b) = (
        s.point_xy(*center).unwrap(),
        s.point_xy(*start).unwrap(),
        s.point_xy(*end).unwrap(),
    );
    let (ra, rb) = (
        (a[0] - c[0]).hypot(a[1] - c[1]),
        (b[0] - c[0]).hypot(b[1] - c[1]),
    );
    assert!((ra - rb).abs() < 1e-12, "{ra} {rb}");
    assert!((ra - 30.0 * kx).abs() < 1e-9, "{ra}");
    // The end points stay where the transform puts them.
    assert!((a[1].abs() - 100.0 * ky).abs() < 1e-12 && (b[1].abs() - 100.0 * ky).abs() < 1e-12);
    // A really elliptical arc / non-uniform scale is still refused.
    let ell = page(r#"<path d="M0 0 A10 10.01 0 0 1 20 0"/>"#);
    assert!(matches!(
        import(&ell, &SvgImportOptions::default()),
        Err(ExchangeError::Unsupported { .. })
    ));
}

#[test]
fn aspect_ratio_alignments_width_only_and_odd_point_lists() {
    // 100 x 50 mm page, viewBox 200 x 200: meet = min(0.5, 0.25), slice = max.
    let pg = |par: &str| {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="50mm" viewBox="0 0 200 200" preserveAspectRatio="{par}"><line x1="0" y1="0" x2="8" y2="0"/></svg>"#
        )
    };
    for (par, k) in [
        ("xMinYMin meet", 0.25),
        ("xMaxYMid", 0.25),
        ("xMidYMid slice", 0.5),
    ] {
        let imp = import(&pg(par), &SvgImportOptions::default()).unwrap();
        assert!((imp.units.to_mm - k).abs() < 1e-15, "{par}");
    }
    for par in ["none", "xMidYMid bogus"] {
        assert!(
            matches!(
                import(&pg(par), &SvgImportOptions::default()),
                Err(ExchangeError::Unsupported { .. })
            ),
            "{par}"
        );
    }
    // Width only: the height follows the viewBox aspect ratio.
    let w = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" viewBox="0 0 100 50"><line x1="0" y1="0" x2="8" y2="0"/></svg>"#;
    let imp = import(w, &SvgImportOptions::default()).unwrap();
    assert!((imp.units.to_mm - 1.0).abs() < 1e-15);
    // Odd coordinate count: drawn up to the error, reported.
    let odd = page(r#"<polyline points="0 0 10 0 10 10 5"/>"#);
    let p = import(&odd, &SvgImportOptions::default()).unwrap();
    assert_eq!(p.sketch.entities.len(), 2);
    assert!(p.skipped.iter().any(|u| u.reason.contains("odd number")));
}
