//! Sketch text: glyph outlines from an installed font become
//! profile loops with counters as holes. Oracles: the font's own glyph boxes
//! read directly with ttf-parser, the exact Bézier curves, rectangle areas.
//!
//! Needs the installed font "Liberation Sans" (Fedora: liberation-sans-fonts).
//! When it is missing these tests fail; set `RCAD_SKIP_FONT_TESTS=1` to skip
//! them explicitly (the skip is printed).

use rcad_sketch::text::{OutlineSeg, TEXT_OUTLINE_TOL, text_edge_parts};
use rcad_sketch::{FontSpec, HAlign, Sketch, SketchError, TextSpec, VAlign, font};

const FAMILY: &str = "Liberation Sans";

fn have_font() -> bool {
    match font::find_font(&FontSpec::family(FAMILY)) {
        Ok(_) => true,
        Err(e) => {
            if std::env::var("RCAD_SKIP_FONT_TESTS").as_deref() == Ok("1") {
                eprintln!("SKIPPED (RCAD_SKIP_FONT_TESTS=1): {e}");
                false
            } else {
                panic!("{e}; install {FAMILY} or set RCAD_SKIP_FONT_TESTS=1 to skip");
            }
        }
    }
}

/// A sketch with only the origin and one text item.
fn text_sketch(text: &str, height: f64) -> Sketch {
    let mut s = Sketch::default();
    s.ensure_origin();
    s.add_text(TextSpec::new(text, FAMILY, height, [0.0, 0.0]))
        .unwrap();
    s
}

/// Outermost regions with their hole counts, left to right.
fn glyph_holes(s: &Sketch) -> Vec<usize> {
    let mut v: Vec<(f64, usize)> = s
        .regions()
        .unwrap()
        .into_iter()
        .filter(|r| r.depth == 0)
        .map(|r| {
            let x = match &r.region.outer {
                rcad_sketch::Profile::Polygon(p) => p.iter().map(|q| q[0]).fold(f64::MAX, f64::min),
                _ => panic!("text loops are polygons"),
            };
            (x, r.region.holes.len())
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().map(|(_, h)| h).collect()
}

#[test]
fn counters_become_holes() {
    if !have_font() {
        return;
    }
    assert_eq!(glyph_holes(&text_sketch("B", 10.0)), vec![2]);
    assert_eq!(glyph_holes(&text_sketch("8", 10.0)), vec![2]);
    assert_eq!(glyph_holes(&text_sketch("O", 10.0)), vec![1]);
    assert_eq!(glyph_holes(&text_sketch("I", 10.0)), vec![0]);
    assert_eq!(glyph_holes(&text_sketch("B8", 10.0)), vec![2, 2]);
    // A space has no outline but advances the pen.
    assert_eq!(glyph_holes(&text_sketch("O I", 10.0)), vec![1, 0]);
    // Whole-sketch profile of one glyph: outer + its counters.
    assert_eq!(
        text_sketch("B", 5.0).profile_region().unwrap().holes.len(),
        2
    );
}

/// Font metrics read directly with ttf-parser (independent of the outline path).
fn font_box(ch: char) -> (f64, f64, [f64; 4]) {
    let f = font::find_font(&FontSpec::family(FAMILY)).unwrap();
    let data = std::fs::read(&f.path).unwrap();
    let face = ttf_parser::Face::parse(&data, f.index).unwrap();
    let g = face.glyph_index(ch).unwrap();
    let b = face.glyph_bounding_box(g).unwrap();
    let cap = f64::from(face.capital_height().unwrap());
    (
        cap,
        f64::from(face.units_per_em()),
        [b.x_min, b.y_min, b.x_max, b.y_max].map(f64::from),
    )
}

#[test]
fn letter_i_is_its_font_box_scaled_to_the_cap_height() {
    if !have_font() {
        return;
    }
    let h = 12.5;
    let s = text_sketch("I", h);
    let (cap, _, b) = font_box('I');
    let k = h / cap;
    let rs = s.regions().unwrap();
    assert_eq!(rs.len(), 1);
    // Liberation Sans 'I' is a plain rectangle: its area is the box area.
    let expect = (b[2] - b[0]) * (b[3] - b[1]) * k * k;
    assert!(
        (rs[0].area - expect).abs() < 1e-9 * expect,
        "area {} vs {expect}",
        rs[0].area
    );
    let rcad_sketch::Profile::Polygon(p) = &rs[0].region.outer else {
        panic!()
    };
    assert_eq!(p.len(), 4);
    let top = p.iter().map(|q| q[1]).fold(f64::MIN, f64::max);
    let bottom = p.iter().map(|q| q[1]).fold(f64::MAX, f64::min);
    // Height definition: the cap height is `height` mm (I sits on the baseline).
    assert!(
        (top - b[3] * k).abs() < 1e-9 && (b[3] - cap).abs() < 1.0,
        "top {top}"
    );
    assert!(bottom.abs() < 1e-9);
}

#[test]
fn alignment_rotation_and_anchor_place_the_text() {
    if !have_font() {
        return;
    }
    let h = 10.0;
    let mut spec = TextSpec::new("I", FAMILY, h, [30.0, 20.0]);
    spec.halign = HAlign::Center;
    spec.valign = VAlign::Middle;
    spec.rotation = std::f64::consts::FRAC_PI_2;
    let mut s = Sketch::default();
    s.ensure_origin();
    let id = s.add_text(spec.clone()).unwrap();
    let r = &s.regions().unwrap()[0];
    let rcad_sketch::Profile::Polygon(p) = &r.region.outer else {
        panic!()
    };
    // Rotated 90°: the cap height runs along -x, centred (cap/2) on x = 30.
    let (xmin, xmax) = (
        p.iter().map(|q| q[0]).fold(f64::MAX, f64::min),
        p.iter().map(|q| q[0]).fold(f64::MIN, f64::max),
    );
    assert!(
        (xmin - 25.0).abs() < 1e-9 && (xmax - 35.0).abs() < 1e-9,
        "{xmin} {xmax}"
    );
    // Bound to a sketch point: the text follows the solved point.
    let pt = s.add_point(-40.0, 5.0);
    spec.anchor = Some(pt);
    s.edit_text(id, spec).unwrap();
    let before = s.regions().unwrap()[0].id.clone();
    let c = s.regions().unwrap()[0].region.outer.clone();
    let rcad_sketch::Profile::Polygon(p) = c else {
        panic!()
    };
    let xmid = p.iter().map(|q| q[0]).sum::<f64>() / p.len() as f64;
    assert!((xmid + 40.0).abs() < 1.0, "{xmid}");
    let d = s.drag_point(pt, [10.0, 10.0]);
    assert!(d.outcome == rcad_sketch::DragOutcome::Reached, "{d:?}");
    assert_eq!(s.regions().unwrap()[0].id, before, "moving keeps identity");
}

/// Exact Bézier point (font units).
fn bez(p0: [f64; 2], seg: &OutlineSeg, t: f64) -> [f64; 2] {
    let l = |a: [f64; 2], b: [f64; 2], t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    match seg {
        OutlineSeg::Line { to } => l(p0, *to, t),
        OutlineSeg::Quad { c, to } => l(l(p0, *c, t), l(*c, *to, t), t),
        OutlineSeg::Cubic { c1, c2, to } => {
            let (a, b, c) = (l(p0, *c1, t), l(*c1, *c2, t), l(*c2, *to, t));
            l(l(a, b, t), l(b, c, t), t)
        }
    }
}

fn dist_to_loop(p: [f64; 2], v: &[[f64; 2]]) -> f64 {
    (0..v.len())
        .map(|i| {
            let (a, b) = (v[i], v[(i + 1) % v.len()]);
            let d = [b[0] - a[0], b[1] - a[1]];
            let l2 = d[0] * d[0] + d[1] * d[1];
            let t = if l2 > 0.0 {
                (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (p[0] - a[0] - t * d[0]).hypot(p[1] - a[1] - t * d[1])
        })
        .fold(f64::INFINITY, f64::min)
}

/// Every point of the exact font curves lies within TEXT_OUTLINE_TOL of the
/// polygon loop of its contour (one direction of the Hausdorff bound; the
/// polygon vertices lie on the curves by construction).
#[test]
fn polygons_stay_within_the_declared_outline_tolerance() {
    if !have_font() {
        return;
    }
    for (text, h) in [("O8", 10.0), ("B", 40.0), ("S", 3.0)] {
        let s = text_sketch(text, h);
        let t = &s.texts[0];
        let k = h / t.resolved.font.cap_height;
        let loops = s.text_loops().unwrap();
        let mut worst: f64 = 0.0;
        for lp in &loops {
            let g = &t.resolved.glyphs[lp.glyph];
            let c = &g.contours[lp.contour];
            let mut p0 = c.start;
            for seg in &c.segs {
                for i in 0..=50 {
                    let q = bez(p0, seg, i as f64 / 50.0);
                    let q = [(q[0] + g.x) * k, q[1] * k];
                    worst = worst.max(dist_to_loop(q, &lp.polygon));
                }
                p0 = match seg {
                    OutlineSeg::Line { to }
                    | OutlineSeg::Quad { to, .. }
                    | OutlineSeg::Cubic { to, .. } => *to,
                };
            }
        }
        assert!(worst <= TEXT_OUTLINE_TOL, "{text} at {h}: {worst}");
        assert!(worst > 0.0);
        // No edge shorter than the kernel minimum (1e-3 mm).
        for lp in &loops {
            let v = &lp.polygon;
            for i in 0..v.len() {
                let (a, b) = (v[i], v[(i + 1) % v.len()]);
                assert!((b[0] - a[0]).hypot(b[1] - a[1]) >= 1e-3);
            }
        }
    }
}

#[test]
fn text_inside_a_rectangle_makes_nested_regions() {
    if !have_font() {
        return;
    }
    let mut s = Sketch::centered_rectangle(60.0, 30.0).unwrap();
    s.solve();
    let mut spec = TextSpec::new("B8", FAMILY, 12.0, [0.0, 0.0]);
    spec.halign = HAlign::Center;
    spec.valign = VAlign::Middle;
    s.add_text(spec).unwrap();
    let rs = s.regions().unwrap();
    // Plate minus 2 glyphs; 2 glyphs minus their counters; 4 counters.
    let depth = |d| rs.iter().filter(|r| r.depth == d).count();
    assert_eq!((depth(0), depth(1), depth(2)), (1, 2, 4));
    let plate = &rs[0];
    assert_eq!(plate.region.holes.len(), 2);
    assert_eq!(plate.region.outer_entities.len(), 4);
    let glyph_outer: f64 = rs.iter().filter(|r| r.depth >= 1).map(|r| r.area).sum();
    // Areas add up: plate + glyphs (minus counters) + counters = rectangle.
    assert!(
        (plate.area + glyph_outer - 1800.0).abs() < 1e-6,
        "{}",
        plate.area + glyph_outer
    );
    // Every region's edges are named: entity ids for the rectangle, text ids for glyphs.
    for r in rs.iter().filter(|r| r.depth >= 1) {
        assert!(
            r.region
                .outer_entities
                .iter()
                .all(|e| text_edge_parts(*e).is_some())
        );
        assert!(r.id.label().starts_with('T'), "{}", r.id.label());
    }
    // The whole-sketch profile refuses islands; picking works.
    assert!(s.profile_region().is_err());
    for r in &rs {
        assert_eq!(s.region_by_ref(&r.id).unwrap().unwrap(), r.region);
    }
    // A point inside the plate but outside the glyphs.
    assert_eq!(s.region_at([-28.0, 13.0]).unwrap(), Some(plate.id.clone()));
}

#[test]
fn region_identity_survives_placement_edits_and_unrelated_string_edits() {
    if !have_font() {
        return;
    }
    let mut s = Sketch::default();
    s.ensure_origin();
    let spec = TextSpec::new("B8", FAMILY, 10.0, [0.0, 0.0]);
    let id = s.add_text(spec.clone()).unwrap();
    let ids = |s: &Sketch| -> Vec<rcad_sketch::RegionRef> {
        s.regions()
            .unwrap()
            .into_iter()
            .filter(|r| r.depth == 0)
            .map(|r| r.id)
            .collect()
    };
    let before = ids(&s);
    let edges_before = s.text_loops().unwrap()[0].edges.len();
    let mut e = spec.clone();
    e.height = 37.0; // more edges at a larger height
    e.position = [5.0, -3.0];
    e.rotation = 0.3;
    s.edit_text(id, e.clone()).unwrap();
    assert_ne!(s.text_loops().unwrap()[0].edges.len(), edges_before);
    let mut after = ids(&s);
    after.sort();
    let mut b = before.clone();
    b.sort();
    assert_eq!(after, b);
    // "B8" -> "B9": the B keeps its region, the 8 (2 counters) is gone.
    e.text = "B9".into();
    s.edit_text(id, e).unwrap();
    let now = ids(&s);
    let b_ref = s
        .regions()
        .unwrap()
        .into_iter()
        .find(|r| r.depth == 0 && r.region.holes.len() == 2)
        .unwrap()
        .id;
    assert!(before.contains(&b_ref));
    assert_eq!(now.iter().filter(|r| before.contains(r)).count(), 1);
}

#[test]
fn reload_and_placement_edits_do_not_need_the_font() {
    if !have_font() {
        return;
    }
    let s = text_sketch("B8", 10.0);
    let areas = |s: &Sketch| {
        s.regions()
            .unwrap()
            .iter()
            .map(|r| r.area)
            .collect::<Vec<_>>()
    };
    let json = serde_json::to_string(&s).unwrap();
    // Simulate a machine without the font: the requested family does not exist.
    let json = json.replace(FAMILY, "Font Not Installed Anywhere");
    let mut back: Sketch = serde_json::from_str(&json).unwrap();
    back.validate().unwrap();
    assert_eq!(areas(&back), areas(&s));
    let t = back.texts[0].clone();
    // Height edit: stored outlines, scaled areas (factor 2^2).
    let mut spec = t.spec.clone();
    spec.height = 20.0;
    back.edit_text(t.id, spec.clone()).unwrap();
    for (a, b) in areas(&back).iter().zip(areas(&s)) {
        assert!((a - 4.0 * b).abs() < 0.02 * b, "{a} vs 4 x {b}");
    }
    // String edit: needs the font, refused, sketch unchanged.
    let snapshot = back.clone();
    spec.text = "C".into();
    let err = back.edit_text(t.id, spec).unwrap_err();
    assert!(matches!(err, SketchError::FontNotFound(_)), "{err}");
    assert_eq!(back, snapshot);
}

#[test]
fn bad_text_is_refused_with_a_reason() {
    if !have_font() {
        return;
    }
    let mut s = Sketch::default();
    s.ensure_origin();
    let snapshot = s.clone();
    let err = |spec: TextSpec, s: &mut Sketch| s.add_text(spec).unwrap_err().to_string();
    let e = err(TextSpec::new("A\nB", FAMILY, 5.0, [0.0, 0.0]), &mut s);
    assert!(e.contains("control"), "{e}");
    let e = err(TextSpec::new("\u{E000}", FAMILY, 5.0, [0.0, 0.0]), &mut s);
    assert!(e.contains("no glyph"), "{e}");
    let e = err(TextSpec::new("A", FAMILY, 0.0, [0.0, 0.0]), &mut s);
    assert!(e.contains("height"), "{e}");
    let e = err(TextSpec::new("A", FAMILY, f64::NAN, [0.0, 0.0]), &mut s);
    assert!(e.contains("height"), "{e}");
    let e = err(TextSpec::new("  ", FAMILY, 5.0, [0.0, 0.0]), &mut s);
    assert!(e.contains("empty"), "{e}");
    assert_eq!(s, snapshot);
    // Two overlapping texts: regions refused, naming the glyphs.
    s.add_text(TextSpec::new("O", FAMILY, 10.0, [0.0, 0.0]))
        .unwrap();
    s.add_text(TextSpec::new("O", FAMILY, 10.0, [2.0, 0.0]))
        .unwrap();
    let e = s.regions().unwrap_err().to_string();
    assert!(
        e.contains("glyph 'O'") && e.contains("touch or cross"),
        "{e}"
    );
    // Tampered file: outlines that do not match the string.
    let mut t = text_sketch("AB", 5.0);
    t.texts[0].spec.text = "AC".into();
    assert!(
        t.validate()
            .unwrap_err()
            .to_string()
            .contains("do not match")
    );
}

#[test]
fn removing_the_anchor_point_unbinds_the_text_in_place() {
    if !have_font() {
        return;
    }
    let mut s = Sketch::default();
    s.ensure_origin();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(10.0, 0.0);
    let line = s.add_line(a, b);
    let mut spec = TextSpec::new("I", FAMILY, 5.0, [0.0, 0.0]);
    spec.anchor = Some(b);
    s.add_text(spec).unwrap();
    assert!(s.remove_entity(line));
    s.validate().unwrap();
    assert_eq!(s.texts[0].spec.anchor, None);
    assert_eq!(s.texts[0].spec.position, [10.0, 0.0]);
}
