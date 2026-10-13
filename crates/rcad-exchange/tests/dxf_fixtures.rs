//! DXF import/export against independently written fixtures (FreeCAD's DXF writer and
//! hand-written files from the DXF reference; tools/fixtures/dxf/README.md). Expected values
//! are analytic.

use rcad_exchange::ExchangeError;
use rcad_exchange::dxf::{self, DxfExportOptions, DxfImport, DxfImportOptions};
use rcad_sketch::{Entity, LoopSegment, Profile, Sketch, SolveStatus};
use std::f64::consts::{FRAC_PI_2, PI, SQRT_2};

fn fixture(name: &str) -> Vec<u8> {
    let p = format!("{}/tests/fixtures/dxf/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn import(name: &str, opts: &DxfImportOptions) -> Result<DxfImport, ExchangeError> {
    dxf::import(&fixture(name), opts)
}

fn ok(name: &str) -> DxfImport {
    import(name, &DxfImportOptions::default()).unwrap()
}

fn mm_override(to_mm: f64) -> DxfImportOptions {
    DxfImportOptions {
        unit_override_to_mm: Some(to_mm),
        ..Default::default()
    }
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// Analytic area of a profile (shoelace over chords + circular segments of arcs).
fn profile_area(p: &Profile) -> f64 {
    match p {
        Profile::Circle { radius, .. } => PI * radius * radius,
        Profile::Polygon(v) => {
            let n = v.len();
            0.5 * (0..n)
                .map(|i| v[i][0] * v[(i + 1) % n][1] - v[(i + 1) % n][0] * v[i][1])
                .sum::<f64>()
        }
        Profile::Loop(segs) => {
            let n = segs.len();
            let mut a = 0.0;
            for i in 0..n {
                let s = segs[i].start();
                let e = segs[(i + 1) % n].start();
                a += 0.5 * (s[0] * e[1] - e[0] * s[1]);
                if let LoopSegment::Arc { center, ccw, .. } = segs[i] {
                    let r = (s[0] - center[0]).hypot(s[1] - center[1]);
                    let a0 = (s[1] - center[1]).atan2(s[0] - center[0]);
                    let a1 = (e[1] - center[1]).atan2(e[0] - center[0]);
                    let sweep = if ccw {
                        (a1 - a0).rem_euclid(2.0 * PI)
                    } else {
                        (a0 - a1).rem_euclid(2.0 * PI)
                    };
                    let seg = 0.5 * r * r * (sweep - sweep.sin());
                    a += if ccw { seg } else { -seg };
                }
            }
            a
        }
    }
}

/// (min, max) over sketch points used by entities, with circles/arcs approximated by
/// dense sampling (test-side only).
fn bounds(s: &Sketch) -> ([f64; 2], [f64; 2]) {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    let mut add = |p: [f64; 2]| {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    };
    for e in &s.entities {
        match e {
            Entity::Line { a, b, .. } => {
                add(s.point_xy(*a).unwrap());
                add(s.point_xy(*b).unwrap());
            }
            Entity::Arc { id, .. } => {
                let g = s.arc_geometry(*id).unwrap();
                for k in 0..=100_000 {
                    let t = g.start_angle + g.sweep * k as f64 / 100_000.0;
                    add([
                        g.center[0] + g.radius * t.cos(),
                        g.center[1] + g.radius * t.sin(),
                    ]);
                }
            }
            Entity::Circle { center, radius, .. } => {
                let c = s.point_xy(*center).unwrap();
                add([c[0] - radius, c[1] - radius]);
                add([c[0] + radius, c[1] + radius]);
            }
        }
    }
    (lo, hi)
}

fn arcs(s: &Sketch) -> Vec<rcad_sketch::ArcGeometry> {
    s.entities
        .iter()
        .filter_map(|e| match e {
            Entity::Arc { id, .. } => s.arc_geometry(*id),
            _ => None,
        })
        .collect()
}

fn count(imp: &DxfImport, t: &str) -> usize {
    imp.imported
        .iter()
        .find(|c| c.entity_type == t)
        .map_or(0, |c| c.count)
}

fn assert_unconstrained(imp: &DxfImport) {
    assert_eq!(imp.solve.status, SolveStatus::UnderConstrained);
    assert!(imp.solve.dof > 0);
    let s = &imp.sketch;
    let origin = s.origin.expect("origin");
    assert_eq!(s.point_xy(origin), Some([0.0, 0.0]));
    assert_eq!(s.constraints.len(), 1, "only the origin's FixedPoint");
    assert!(!s.point_used_by_entity(origin));
}

const TOL: f64 = 1e-9;

/// (a) FreeCAD slot: lines + CCW ARC centre (30,10) r 10 from -90 to 90 deg.
/// Area 600 + 50 pi, bounds (0,0)-(40,20).
#[test]
fn freecad_slot_line_arc_contour() {
    let imp = ok("slot_freecad_r14.dxf");
    assert_eq!(imp.version.as_deref(), Some("AC1014"));
    assert_eq!(imp.units.to_mm, 1.0);
    assert!(imp.units.declared);
    assert_eq!(count(&imp, "LINE"), 3);
    assert_eq!(count(&imp, "ARC"), 1);
    assert!(imp.skipped.is_empty());
    assert_unconstrained(&imp);
    let a = arcs(&imp.sketch);
    assert_eq!(a.len(), 1);
    assert!(close(a[0].center[0], 30.0, TOL) && close(a[0].center[1], 10.0, TOL));
    assert!(close(a[0].radius, 10.0, TOL));
    assert!(close(a[0].start_angle, -FRAC_PI_2, TOL));
    assert!(close(a[0].sweep, PI, TOL));
    let (lo, hi) = bounds(&imp.sketch);
    assert!(close(lo[0], 0.0, TOL) && close(lo[1], 0.0, TOL));
    assert!(close(hi[0], 40.0, 1e-6) && close(hi[1], 20.0, TOL));
    let p = imp.sketch.profile().unwrap();
    assert!(matches!(p, Profile::Loop(_)));
    assert!(close(profile_area(&p), 600.0 + 50.0 * PI, 1e-9));
    // 4 shared corners, no open ends.
    assert!(
        !imp.warnings.iter().any(|w| w.contains("open endpoint")),
        "{:?}",
        imp.warnings
    );
}

/// FreeCAD R12 notch (no $INSUNITS -> needs a unit): 40 x 20 minus a semicircle r 5
/// centred (20,20): area 800 - 12.5 pi. The contour runs clockwise around the notch centre;
/// the stored ARC is CCW from 180 to 360 deg.
#[test]
fn freecad_r12_notch_needs_units_then_imports() {
    let e = import("notch_freecad_r12.dxf", &DxfImportOptions::default()).unwrap_err();
    assert!(matches!(e, ExchangeError::UnitsRequired { .. }), "{e:?}");
    let imp = import("notch_freecad_r12.dxf", &mm_override(1.0)).unwrap();
    assert_eq!(imp.version.as_deref(), Some("AC1009"));
    assert!(!imp.units.declared);
    assert_unconstrained(&imp);
    let a = arcs(&imp.sketch);
    assert!(close(a[0].center[0], 20.0, TOL) && close(a[0].center[1], 20.0, TOL));
    assert!(close(a[0].radius, 5.0, TOL));
    assert!(close(a[0].start_angle.abs(), PI, TOL));
    assert!(close(a[0].sweep, PI, TOL));
    let p = imp.sketch.profile().unwrap();
    assert!(close(profile_area(&p), 800.0 - 12.5 * PI, 1e-9));
    let (lo, hi) = bounds(&imp.sketch);
    assert!(close(lo[1], 0.0, TOL) && close(hi[0], 40.0, TOL) && close(hi[1], 20.0, TOL));
}

fn check_bulge_square(imp: &DxfImport, k: f64) {
    assert_unconstrained(imp);
    let mut a = arcs(&imp.sketch);
    a.sort_by(|x, y| x.radius.total_cmp(&y.radius));
    assert_eq!(a.len(), 2);
    // Semicircle (bulge 1) from (20,0) to (20,20): centre (20,10), r 10, CCW from -90 deg.
    assert!(close(a[0].center[0], 20.0 * k, TOL * k) && close(a[0].center[1], 10.0 * k, TOL * k));
    assert!(close(a[0].radius, 10.0 * k, TOL * k));
    assert!(close(a[0].sweep, PI, TOL));
    assert!(close(a[0].start_angle, -FRAC_PI_2, TOL));
    // Quarter arc (bulge -tan(pi/8)) from (0,20) to (0,0), clockwise: centre (-10,10),
    // r 10 sqrt 2; stored CCW from (0,0) (-45 deg... angle of (10,-10)) sweeping 90 deg.
    assert!(close(a[1].center[0], -10.0 * k, TOL * k) && close(a[1].center[1], 10.0 * k, TOL * k));
    assert!(close(a[1].radius, 10.0 * SQRT_2 * k, TOL * k));
    assert!(close(a[1].sweep, FRAC_PI_2, TOL));
    assert!(close(a[1].start_angle, -PI / 4.0, TOL), "{a:?}");
    let p = imp.sketch.profile().unwrap();
    // 400 + 50 pi (semicircle) - (200/2)(pi/2 - 1) (inward quarter-arc segment) = 500.
    let area = 500.0 * k * k;
    assert!(
        close(profile_area(&p), area, 1e-9 * area),
        "{}",
        profile_area(&p)
    );
}

/// (b) LWPOLYLINE with bulges (closing segment carries the negative bulge).
#[test]
fn lwpolyline_bulges_are_exact_arcs() {
    let imp = ok("lwpoly_bulge.dxf");
    assert_eq!(count(&imp, "LWPOLYLINE"), 1);
    assert_eq!(imp.sketch.entities.len(), 4);
    check_bulge_square(&imp, 1.0);
}

/// (c) the same contour as R12 POLYLINE/VERTEX/SEQEND without units.
#[test]
fn r12_polyline_unitless_requires_choice() {
    match import(
        "polyline_bulge_r12_unitless.dxf",
        &DxfImportOptions::default(),
    ) {
        Err(ExchangeError::UnitsRequired { measurement, .. }) => assert_eq!(measurement, None),
        other => panic!("{other:?}"),
    }
    let imp = import("polyline_bulge_r12_unitless.dxf", &mm_override(1.0)).unwrap();
    assert_eq!(count(&imp, "POLYLINE"), 1);
    check_bulge_square(&imp, 1.0);
    let imp = import("polyline_bulge_r12_unitless.dxf", &mm_override(25.4)).unwrap();
    assert_eq!(imp.units.name, "user override");
    check_bulge_square(&imp, 25.4);
}

/// (c) inches: 2 x 1 in -> 50.8 x 25.4 mm. A unit override never beats a declared unit.
#[test]
fn inch_units_scale_to_mm() {
    let imp = import("rect_inch.dxf", &mm_override(1.0)).unwrap();
    assert_eq!(imp.units.name, "inches");
    assert_eq!(imp.units.to_mm, 25.4);
    assert!(imp.units.declared);
    assert_eq!(imp.units.insunits, Some(1));
    assert_eq!(imp.measurement, Some(0));
    assert_unconstrained(&imp);
    let (lo, hi) = bounds(&imp.sketch);
    assert_eq!(lo, [0.0, 0.0]);
    assert!(close(hi[0], 50.8, 1e-12) && close(hi[1], 25.4, 1e-12));
    let p = imp.sketch.profile().unwrap();
    assert!(matches!(p, Profile::Polygon(_)));
    assert!(close(profile_area(&p), 50.8 * 25.4, 1e-9));
}

/// (e) ARC in an OCS with extrusion (0,0,-1): OCS centre (-30,10), 90..270 deg is the WCS
/// arc centre (30,10) from -90 to 90 deg (through (40,10)). Area 600 + 50 pi.
#[test]
fn mirrored_ocs_arc_is_imported_in_wcs() {
    let imp = ok("slot_mirrored_arc.dxf");
    let a = arcs(&imp.sketch);
    assert!(close(a[0].center[0], 30.0, TOL) && close(a[0].center[1], 10.0, TOL));
    assert!(close(a[0].start_angle, -FRAC_PI_2, TOL));
    assert!(close(a[0].sweep, PI, TOL));
    let p = imp.sketch.profile().unwrap();
    assert!(close(profile_area(&p), 600.0 + 50.0 * PI, 1e-9));
}

/// (d) unsupported entities: all-or-nothing by default, partial on request, layer filter.
#[test]
fn unsupported_entities_are_reported_never_dropped() {
    let e = import("unsupported_entities.dxf", &DxfImportOptions::default()).unwrap_err();
    let ExchangeError::Unsupported { entities, .. } = &e else {
        panic!("{e:?}")
    };
    let mut kinds: Vec<(&str, usize)> = entities
        .iter()
        .map(|u| (u.entity_type.as_str(), u.count))
        .collect();
    kinds.sort();
    assert_eq!(
        kinds,
        [("INSERT", 1), ("LINE", 1), ("SPLINE", 1), ("TEXT", 1)],
        "{entities:?}"
    );
    let msg = e.to_string();
    assert!(
        msg.contains("not in the XY plane") && msg.contains("block references"),
        "{msg}"
    );

    let partial = DxfImportOptions {
        allow_partial: true,
        ..Default::default()
    };
    let imp = import("unsupported_entities.dxf", &partial).unwrap();
    assert_eq!(count(&imp, "LINE"), 4);
    assert_eq!(count(&imp, "CIRCLE"), 1);
    // The block definition's LINE is never imported.
    assert_eq!(imp.sketch.entities.len(), 5);
    assert_eq!(imp.skipped.iter().map(|s| s.count).sum::<usize>(), 4);
    assert!(imp.warnings.iter().any(|w| w.contains("SPLINE")));

    let only_profile = DxfImportOptions {
        layers: Some(vec!["profile".into()]),
        ..Default::default()
    };
    let imp = import("unsupported_entities.dxf", &only_profile).unwrap();
    assert!(imp.skipped.is_empty());
    let layer = |n: &str| imp.layers.iter().find(|l| l.name == n).unwrap().clone();
    assert_eq!(
        (layer("PROFILE").entities, layer("PROFILE").selected),
        (4, true)
    );
    assert_eq!(
        (layer("HOLES").entities, layer("HOLES").selected),
        (1, false)
    );
    assert_eq!(
        (layer("NOTES").entities, layer("NOTES").selected),
        (4, false)
    );
    let p = imp.sketch.profile().unwrap();
    assert!(close(profile_area(&p), 100.0, 1e-12));

    let holes = DxfImportOptions {
        layers: Some(vec!["HOLES".into()]),
        ..Default::default()
    };
    let imp = import("unsupported_entities.dxf", &holes).unwrap();
    assert_eq!(
        imp.sketch.profile().unwrap(),
        Profile::Circle {
            center: [5.0, 5.0],
            radius: 2.0
        }
    );
}

fn doc(header: &str, entities: &str) -> String {
    format!(
        "0\nSECTION\n2\nHEADER\n{header}0\nENDSEC\n0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n"
    )
}

const MM: &str = "9\n$INSUNITS\n70\n4\n";

fn line(a: [f64; 2], b: [f64; 2]) -> String {
    format!(
        "0\nLINE\n8\n0\n10\n{}\n20\n{}\n30\n0.0\n11\n{}\n21\n{}\n31\n0.0\n",
        a[0], a[1], b[0], b[1]
    )
}

/// Endpoints 1e-4 mm apart stay separate at the default tolerance (open loop, explicit
/// warning, profile refused) and join at a 1e-3 mm tolerance.
#[test]
fn join_tolerance_controls_closure() {
    let ents = line([0.0, 0.0], [10.0, 0.0])
        + &line([10.0, 0.0], [10.0, 10.0])
        + &line([10.0, 10.0], [0.0, 10.0])
        + &line([0.0, 10.0], [0.0, 0.0001]);
    let text = doc(MM, &ents);
    let imp = dxf::import(text.as_bytes(), &DxfImportOptions::default()).unwrap();
    assert!(
        imp.warnings.iter().any(|w| w.contains("2 open endpoint")),
        "{:?}",
        imp.warnings
    );
    assert!(imp.sketch.profile().is_err());
    let loose = DxfImportOptions {
        join_tolerance_mm: 1e-3,
        ..Default::default()
    };
    let imp = dxf::import(text.as_bytes(), &loose).unwrap();
    let p = imp.sketch.profile().unwrap();
    assert!(close(profile_area(&p), 100.0, 1e-12));
}

#[test]
fn malformed_and_bounded_input() {
    let bad = doc(MM, "0\nLINE\n10\nabc\n20\n0\n11\n1\n21\n1\n");
    match dxf::import(bad.as_bytes(), &DxfImportOptions::default()) {
        Err(ExchangeError::Malformed { line: Some(l), .. }) => assert_eq!(l, 17),
        other => panic!("{other:?}"),
    }
    let nan = doc(MM, &line([0.0, 0.0], [f64::NAN, 1.0]));
    assert!(matches!(
        dxf::import(nan.as_bytes(), &DxfImportOptions::default()),
        Err(ExchangeError::NonFinite { .. })
    ));
    let missing = doc(MM, "0\nARC\n10\n0\n20\n0\n40\n1\n50\n0\n");
    assert!(matches!(
        dxf::import(missing.as_bytes(), &DxfImportOptions::default()),
        Err(ExchangeError::Malformed { .. })
    ));
    let small = DxfImportOptions {
        max_bytes: 100,
        ..Default::default()
    };
    assert!(matches!(
        import("slot_freecad_r14.dxf", &small),
        Err(ExchangeError::LimitExceeded { .. })
    ));
    let few = DxfImportOptions {
        max_entities: 3,
        ..Default::default()
    };
    assert!(matches!(
        import("slot_freecad_r14.dxf", &few),
        Err(ExchangeError::LimitExceeded { .. })
    ));
    assert!(matches!(
        dxf::import(b"", &DxfImportOptions::default()),
        Err(ExchangeError::Truncated { .. })
    ));
    assert!(matches!(
        dxf::import(doc(MM, "").as_bytes(), &DxfImportOptions::default()),
        Err(ExchangeError::Empty { .. })
    ));
    let pl = doc(MM, "0\nPOLYLINE\n66\n1\n70\n1\n0\nVERTEX\n10\n0\n20\n0\n");
    assert!(matches!(
        dxf::import(pl.as_bytes(), &DxfImportOptions::default()),
        Err(ExchangeError::Malformed { .. })
    ));
    // Arbitrary garbage never panics.
    for seed in 0..200u32 {
        let junk: Vec<u8> = (0..300u32)
            .map(|i| (i.wrapping_mul(2654435761).wrapping_add(seed * 97) >> 7) as u8)
            .collect();
        let _ = dxf::import(&junk, &DxfImportOptions::default());
    }
}

#[test]
fn unsupported_units_extrusion_and_3d_polyline() {
    let au = doc("9\n$INSUNITS\n70\n18\n", &line([0.0, 0.0], [1.0, 0.0]));
    assert!(matches!(
        dxf::import(au.as_bytes(), &DxfImportOptions::default()),
        Err(ExchangeError::UnsupportedUnits { .. })
    ));
    let tilted = doc(
        MM,
        "0\nCIRCLE\n8\n0\n10\n0\n20\n0\n30\n0\n40\n1\n210\n0.6\n220\n0\n230\n0.8\n",
    );
    match dxf::import(tilted.as_bytes(), &DxfImportOptions::default()) {
        Err(ExchangeError::Unsupported { entities, .. }) => {
            assert!(entities[0].reason.contains("extrusion"))
        }
        other => panic!("{other:?}"),
    }
    let p3 = doc(
        MM,
        "0\nPOLYLINE\n66\n1\n70\n8\n0\nVERTEX\n10\n0\n20\n0\n30\n1\n70\n32\n0\nVERTEX\n10\n1\n20\n0\n30\n0\n70\n32\n0\nSEQEND\n",
    );
    match dxf::import(p3.as_bytes(), &DxfImportOptions::default()) {
        Err(ExchangeError::Unsupported { entities, .. }) => {
            assert_eq!(entities[0].entity_type, "POLYLINE")
        }
        other => panic!("{other:?}"),
    }
    let elevated = doc(MM, "0\nCIRCLE\n8\n0\n10\n0\n20\n0\n30\n2.5\n40\n1\n");
    assert!(matches!(
        dxf::import(elevated.as_bytes(), &DxfImportOptions::default()),
        Err(ExchangeError::Unsupported { .. })
    ));
}

/// Degenerate entities are dropped, listed and warned about; they do not block import.
#[test]
fn degenerate_entities_are_listed_not_fatal() {
    let ents = line([0.0, 0.0], [10.0, 0.0])
        + &line([5.0, 5.0], [5.0, 5.0])
        + "0\nARC\n8\n0\n10\n0\n20\n0\n30\n0\n40\n3\n50\n30\n51\n30\n";
    let imp = dxf::import(doc(MM, &ents).as_bytes(), &DxfImportOptions::default()).unwrap();
    assert_eq!(imp.sketch.entities.len(), 1);
    assert_eq!(imp.skipped.len(), 2, "{:?}", imp.skipped);
    assert!(
        imp.warnings
            .iter()
            .filter(|w| w.contains("degenerate"))
            .count()
            == 2
    );
}

/// Export -> re-import of the FreeCAD slot keeps the local geometry (supplementary: same
/// code on both sides; FreeCAD validates the writer in tools/fixtures/dxf/validate_export.sh).
#[test]
fn export_round_trip_keeps_geometry() {
    for (name, opts, area) in [
        (
            "slot_freecad_r14.dxf",
            DxfImportOptions::default(),
            600.0 + 50.0 * PI,
        ),
        ("lwpoly_bulge.dxf", DxfImportOptions::default(), 500.0),
        ("notch_freecad_r12.dxf", mm_override(1.0), 800.0 - 12.5 * PI),
    ] {
        let first = import(name, &opts).unwrap();
        let text = dxf::export(&first.sketch, &DxfExportOptions::default()).unwrap();
        assert!(text.contains("AC1009") && text.contains("$INSUNITS"));
        let again = dxf::import(text.as_bytes(), &DxfImportOptions::default()).unwrap();
        assert!(again.units.declared && again.units.to_mm == 1.0);
        let (a1, a2) = (arcs(&first.sketch), arcs(&again.sketch));
        assert_eq!(a1.len(), a2.len());
        for (x, y) in a1.iter().zip(&a2) {
            assert!(close(x.radius, y.radius, 1e-9));
            assert!(close(x.sweep, y.sweep, 1e-9));
            assert!(close(x.center[0], y.center[0], 1e-9) && close(x.center[1], y.center[1], 1e-9));
        }
        let p = again.sketch.profile().unwrap();
        assert!(close(profile_area(&p), area, 1e-8), "{name}");
    }
}

#[test]
fn export_construction_policy() {
    let mut s = Sketch::default();
    s.ensure_origin();
    let c = s.add_point(0.0, 0.0);
    s.add_circle(c, 5.0);
    let a = s.add_point(-10.0, 0.0);
    let b = s.add_point(10.0, 0.0);
    let l = s.add_line(a, b);
    s.set_construction(l, true);
    let plain = dxf::export(&s, &DxfExportOptions::default()).unwrap();
    assert!(!plain.contains("LINE\n") && plain.contains("CIRCLE\n"));
    assert!(!plain.contains("CONSTRUCTION"));
    let with = dxf::export(
        &s,
        &DxfExportOptions {
            include_construction: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(with.contains("LINE\n  8\nCONSTRUCTION\n"));
    let back = dxf::import(with.as_bytes(), &DxfImportOptions::default()).unwrap();
    let names: Vec<_> = back
        .layers
        .iter()
        .map(|l| (l.name.as_str(), l.entities))
        .collect();
    assert_eq!(names, [("0", 1), ("CONSTRUCTION", 1)]);
}

/// Every coordinate an imported sketch carries (points, circle radii, derived arcs) is
/// finite.
fn assert_finite(imp: &DxfImport, what: &str) {
    let s = &imp.sketch;
    for p in &s.points {
        assert!(p.x.is_finite() && p.y.is_finite(), "{what}: point {p:?}");
    }
    for e in &s.entities {
        match e {
            Entity::Circle { radius, .. } => assert!(radius.is_finite(), "{what}: {e:?}"),
            Entity::Arc { id, .. } => {
                if let Some(g) = s.arc_geometry(*id) {
                    assert!(
                        g.center.iter().all(|v| v.is_finite())
                            && g.radius.is_finite()
                            && g.start_angle.is_finite()
                            && g.sweep.is_finite(),
                        "{what}: {g:?}"
                    );
                }
            }
            Entity::Line { .. } => {}
        }
    }
}

const FUZZ_FIXTURES: [&str; 3] = [
    "slot_freecad_r14.dxf",
    "lwpoly_bulge.dxf",
    "polyline_bulge_r12_unitless.dxf",
];

/// Strict and partial import, both with a unit override (25.4) so the unitless R12 file and
/// prefixes that lost `$INSUNITS` still reach entity parsing and scaling.
fn fuzz_options() -> [DxfImportOptions; 2] {
    [false, true].map(|allow_partial| DxfImportOptions {
        allow_partial,
        unit_override_to_mm: Some(25.4),
        ..Default::default()
    })
}

/// Imports (must not panic) and checks any accepted sketch; returns whether it was accepted.
fn import_checked(bytes: &[u8], opts: &DxfImportOptions, what: &str) -> bool {
    match dxf::import(bytes, opts) {
        Ok(imp) => {
            assert_finite(&imp, what);
            true
        }
        Err(_) => false,
    }
}

#[test]
fn every_prefix_of_a_dxf_fixture_is_handled() {
    for name in FUZZ_FIXTURES {
        let z = fixture(name);
        for opts in &fuzz_options() {
            let full = dxf::import(&z, opts).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_finite(&full, name);
            for n in 0..z.len() {
                import_checked(&z[..n], opts, &format!("{name} prefix {n}"));
            }
        }
    }
}

/// One DXF-shaped byte (digit, sign, decimal point, newline) or, 1 time in 14, a random byte.
fn pick(next: &mut impl FnMut() -> u64) -> u8 {
    const REPLACEMENTS: &[u8] = b"0123456789-.\n";
    let k = (next() as usize) % (REPLACEMENTS.len() + 1);
    REPLACEMENTS.get(k).copied().unwrap_or(next() as u8)
}

/// Single-byte substitutions at every position (value chosen by xorshift from DXF-shaped
/// bytes, or a random byte), then multi-byte mutants at xorshift positions.
#[test]
fn byte_substitutions_in_a_dxf_fixture_are_handled() {
    // xorshift64, as in rcad-kernel's STEP mutation test.
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for name in FUZZ_FIXTURES {
        let z = fixture(name);
        let opts = fuzz_options();
        let mut accepted = 0usize;
        for i in 0..z.len() {
            let mut c = z.clone();
            c[i] = pick(&mut next);
            for o in &opts {
                accepted +=
                    import_checked(&c, o, &format!("{name} byte {i} = {:#04x}", c[i])) as usize;
            }
        }
        for m in 0..1000 {
            let mut c = z.clone();
            for _ in 0..3 {
                let i = (next() as usize) % c.len();
                c[i] = pick(&mut next);
            }
            for o in &opts {
                accepted += import_checked(&c, o, &format!("{name} mutant {m}")) as usize;
            }
        }
        // Non-vacuous: many single-byte edits (e.g. in a layer name or a digit) still import,
        // so the finiteness check above has run.
        assert!(accepted > 0, "{name}");
    }
}
