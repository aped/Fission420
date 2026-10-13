//! STEP import against independent FreeCAD 1.1.4 / OpenCascade fixtures
//! (tests/fixtures/step/README.md). Expected values are analytic, not snapshots.
use rcad_kernel::*;
use std::f64::consts::PI;

fn fixture(name: &str) -> String {
    let p = format!("{}/tests/fixtures/step/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn import(name: &str) -> StepImport {
    import_step(&fixture(name), &StepImportOptions::default())
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Same acceptance tolerance as the existing kernel volume tests.
fn vol_tol(v: f64) -> f64 {
    0.001_f64.max(v.abs() * 1e-7)
}

fn volume(b: &Body) -> (f64, f64, VolumeMethod) {
    match measure(b).unwrap().volume {
        VolumeResult::Value {
            volume,
            error_estimate,
            method,
        } => (volume, error_estimate, method),
        VolumeResult::Unavailable { reason } => panic!("volume unavailable: {reason}"),
    }
}

fn assert_exact_volume(b: &Body, expected: f64) {
    let (v, _, method) = volume(b);
    assert_ne!(method, VolumeMethod::TessellationEstimate);
    assert!(
        (v - expected).abs() <= vol_tol(expected),
        "volume {v} vs analytic {expected} (err {:e})",
        v - expected
    );
}

fn assert_closed_single(b: &Body) {
    let m = measure(b).unwrap();
    assert_eq!(m.shell_count, 1);
    assert!(m.all_shells_closed);
}

fn close3(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= tol)
}

fn assert_bounds(b: &Body, min: [f64; 3], max: [f64; 3], tol: f64) {
    let m = measure(b).unwrap();
    assert!(
        close3(m.bounds.min, min, tol) && close3(m.bounds.max, max, tol),
        "bounds {:?} vs {min:?} {max:?}",
        m.bounds
    );
}

#[test]
fn box_mm_imports_exactly_with_planar_face_frames() {
    let imp = import("box_40x30x10_mm.step");
    assert_eq!(imp.length_unit.name, "millimetre");
    assert_eq!(imp.length_unit.to_mm, 1.0);
    assert!(imp.length_unit.declared);
    assert_eq!(imp.solids.len(), 1);
    assert_eq!(imp.stats.solids, 1);
    assert_eq!(imp.stats.surfaces.get("PLANE"), Some(&6));
    assert_eq!(imp.stats.curves.get("LINE"), Some(&12));
    let s = &imp.solids[0];
    assert_closed_single(&s.body);
    assert_eq!(face_count(&s.body), 6);
    assert_exact_volume(&s.body, 12000.0);
    assert_bounds(&s.body, [0.0; 3], [40.0, 30.0, 10.0], 1e-12);
    assert_eq!(s.planar_faces.len(), 6);
    // Each face: outward normal, centroid, area and the documented x-axis rule.
    let expect = [
        // (z, centroid, area, x)
        ([0.0, 0.0, 1.0], [20.0, 15.0, 10.0], 1200.0, [1.0, 0.0, 0.0]),
        ([0.0, 0.0, -1.0], [20.0, 15.0, 0.0], 1200.0, [1.0, 0.0, 0.0]),
        ([1.0, 0.0, 0.0], [40.0, 15.0, 5.0], 300.0, [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 15.0, 5.0], 300.0, [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [20.0, 30.0, 5.0], 400.0, [1.0, 0.0, 0.0]),
        ([0.0, -1.0, 0.0], [20.0, 0.0, 5.0], 400.0, [1.0, 0.0, 0.0]),
    ];
    for (z, c, area, x) in expect {
        let f = s
            .planar_faces
            .iter()
            .find(|f| close3(f.frame.z, z, 1e-12))
            .unwrap_or_else(|| panic!("no face with normal {z:?}"));
        assert!(close3(f.frame.origin, c, 1e-9), "{:?}", f.frame);
        assert!((f.area - area).abs() < 1e-9, "{}", f.area);
        assert!(close3(f.frame.x, x, 1e-12), "{:?}", f.frame);
        // right-handed: y = z x x
        let y = [
            z[1] * x[2] - z[2] * x[1],
            z[2] * x[0] - z[0] * x[2],
            z[0] * x[1] - z[1] * x[0],
        ];
        assert!(close3(f.frame.y, y, 1e-12));
        // The frame resolves to exactly one planar face of the body.
        assert_eq!(planar_faces_on(&s.body, &f.frame), 1);
    }
    // Indices are positions in face_iter order and deterministic for the same bytes.
    let again = import("box_40x30x10_mm.step");
    assert_eq!(again.solids[0].planar_faces, s.planar_faces);
    let mut idx: Vec<u32> = s.planar_faces.iter().map(|f| f.index).collect();
    idx.sort();
    assert_eq!(idx, vec![0, 1, 2, 3, 4, 5]);
}

#[test]
fn inch_and_metre_files_are_scaled_to_mm() {
    for (name, unit, k) in [
        ("box_40x30x10_inch.step", "inch", 25.4),
        ("box_40x30x10_m.step", "metre", 1000.0),
    ] {
        let imp = import(name);
        assert_eq!(imp.length_unit.name, unit);
        assert!((imp.length_unit.to_mm - k).abs() < 1e-12);
        assert!(imp.length_unit.declared);
        let b = &imp.solids[0].body;
        // The file stores 40/25.4 etc. rounded to 12+ digits; scaling is exact
        // arithmetic on those, so the result matches to ~1e-11 mm.
        assert_bounds(b, [0.0; 3], [40.0, 30.0, 10.0], 1e-9);
        assert_exact_volume(b, 12000.0);
    }
    // FreeCAD's own "unit relabel" export: coordinates stay 40/30/10 in inches.
    let imp = import("freecad_relabel_inch.step");
    assert_bounds(&imp.solids[0].body, [0.0; 3], [1016.0, 762.0, 254.0], 1e-9);
}

#[test]
fn undeclared_unit_requires_an_explicit_choice() {
    let text = fixture("missing_unit.step");
    let e = import_step(&text, &StepImportOptions::default()).unwrap_err();
    assert!(e.to_string().contains("units required"), "{e}");
    let opts = StepImportOptions {
        unit_override_to_mm: Some(25.4),
        ..Default::default()
    };
    let imp = import_step(&text, &opts).unwrap();
    assert!(!imp.length_unit.declared);
    assert_eq!(imp.length_unit.to_mm, 25.4);
    assert!(!imp.warnings.is_empty());
    assert_bounds(&imp.solids[0].body, [0.0; 3], [1016.0, 762.0, 254.0], 1e-9);
    // The override never replaces a declared unit.
    let imp = import_step(&fixture("box_40x30x10_mm.step"), &opts).unwrap();
    assert_eq!(imp.length_unit.to_mm, 1.0);
    assert!(imp.length_unit.declared);
}

#[test]
fn cylinder_imports_exactly() {
    let imp = import("cylinder_d20_h15_mm.step");
    assert_eq!(imp.stats.step_faces, 3);
    assert_eq!(imp.stats.surfaces.get("CYLINDRICAL_SURFACE"), Some(&2)); // split at the seam
    assert_eq!(imp.stats.curves.get("CIRCLE"), Some(&2));
    let b = &imp.solids[0].body;
    assert_closed_single(b);
    assert_exact_volume(b, PI * 100.0 * 15.0);
    assert_bounds(b, [-10.0, -10.0, 0.0], [10.0, 10.0, 15.0], 1e-9);
    let caps = &imp.solids[0].planar_faces;
    assert_eq!(caps.len(), 2);
    for f in caps {
        assert!((f.area - 100.0 * PI).abs() < 1e-8, "{}", f.area);
        assert!(close3(
            [f.frame.origin[0], f.frame.origin[1], 0.0],
            [0.0; 3],
            1e-9
        ));
    }
    let mesh = check_mesh(&tessellate(b, 0.05).unwrap());
    assert!(mesh.watertight);
}

#[test]
fn plate_with_hole_imports_exactly() {
    let imp = import("plate_hole_mm.step");
    let b = &imp.solids[0].body;
    assert_closed_single(b);
    assert_exact_volume(b, 60.0 * 40.0 * 8.0 - PI * 25.0 * 8.0);
    let top = imp.solids[0]
        .planar_faces
        .iter()
        .find(|f| f.frame.z == [0.0, 0.0, 1.0])
        .unwrap();
    assert!((top.area - (2400.0 - 25.0 * PI)).abs() < 1e-8);
    assert!(close3(top.frame.origin, [30.0, 20.0, 8.0], 1e-9));
}

/// Sphere and torus: exact geometry (verified against the analytic surfaces at
/// import), volume through the labelled tessellation estimate.
#[test]
fn sphere_and_torus_import_with_estimated_volume() {
    for (name, expected, min, max) in [
        (
            "sphere_r10_mm.step",
            4.0 / 3.0 * PI * 1000.0,
            [-5.0; 3],
            [15.0; 3],
        ),
        (
            "torus_R20_r5_mm.step",
            2.0 * PI * PI * 20.0 * 25.0,
            [-25.0, -25.0, -5.0],
            [25.0, 25.0, 5.0],
        ),
    ] {
        let imp = import(name);
        let b = &imp.solids[0].body;
        assert_closed_single(b);
        let (v, err, method) = volume(b);
        assert_eq!(method, VolumeMethod::TessellationEstimate, "{name}");
        assert!(
            (v - expected).abs() <= err && err < 0.01 * expected,
            "{name}: {v} +- {err} vs {expected}"
        );
        assert_bounds(b, min, max, 1e-9);
        assert!(check_mesh(&tessellate(b, 0.05).unwrap()).watertight);
    }
}

#[test]
fn multi_solid_compound_keeps_every_solid() {
    let imp = import("two_boxes_compound_mm.step");
    assert_eq!(imp.solids.len(), 2);
    for s in &imp.solids {
        assert_closed_single(&s.body);
        // OCC writes coordinates with 12 significant digits: the rotated box's
        // volume is exact only to that rounding (~1e-11 relative).
        assert_exact_volume(&s.body, 1000.0);
    }
    assert_bounds(&imp.solids[0].body, [0.0; 3], [10.0; 3], 1e-12);
    let c30 = 30f64.to_radians().cos();
    let s30 = 30f64.to_radians().sin();
    assert_bounds(
        &imp.solids[1].body,
        [45.0, 0.0, 0.0],
        [50.0 + 20.0 * c30, 20.0 * s30 + 10.0 * c30, 5.0],
        1e-9,
    );
}

#[test]
fn assembly_placements_are_flattened() {
    let imp = import("two_boxes_assembly_mm.step");
    assert_eq!(imp.solids.len(), 2);
    let names: Vec<_> = imp.solids.iter().map(|s| s.name.clone()).collect();
    assert_eq!(names, vec![Some("BoxA".into()), Some("BoxB".into())]);
    let c30 = 30f64.to_radians().cos();
    let s30 = 30f64.to_radians().sin();
    // BoxB is stored locally in [0,20]x[0,10]x[0,5] and placed by an
    // ITEM_DEFINED_TRANSFORMATION: rotation 30 deg about Z, then (50,0,0).
    assert_bounds(
        &imp.solids[1].body,
        [45.0, 0.0, 0.0],
        [50.0 + 20.0 * c30, 20.0 * s30 + 10.0 * c30, 5.0],
        1e-9,
    );
    assert_exact_volume(&imp.solids[1].body, 1000.0);
    // A rotated face frame: the +x side of the local box has normal (cos30, sin30, 0).
    let f = imp.solids[1]
        .planar_faces
        .iter()
        .find(|f| close3(f.frame.z, [c30, s30, 0.0], 1e-9))
        .unwrap();
    assert!((f.area - 50.0).abs() < 1e-9);
}

#[test]
fn malformed_files_are_errors_not_panics() {
    for name in ["truncated.step", "garbage.step"] {
        assert!(import_step(&fixture(name), &StepImportOptions::default()).is_err());
    }
    // Truncate a valid file at many byte positions.
    let text = fixture("plate_hole_mm.step");
    let mut errs = 0;
    for cut in (0..text.len()).step_by(61) {
        if !text.is_char_boundary(cut) {
            continue;
        }
        if import_step(&text[..cut], &StepImportOptions::default()).is_err() {
            errs += 1;
        }
    }
    assert!(errs > 0);
    // Deterministic byte mutations (digits, references, keywords).
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let bytes = text.as_bytes();
    let replacements = b"0123456789#.,()'E-$*;=X";
    for _ in 0..300 {
        let mut b = bytes.to_vec();
        for _ in 0..3 {
            let i = (next() as usize) % b.len();
            b[i] = replacements[(next() as usize) % replacements.len()];
        }
        if let Ok(s) = String::from_utf8(b) {
            let _ = import_step(&s, &StepImportOptions::default()); // must not panic
        }
    }
}

#[test]
fn limits_are_enforced() {
    let text = fixture("box_40x30x10_mm.step");
    let small = StepImportOptions {
        max_bytes: 100,
        ..Default::default()
    };
    assert!(
        import_step(&text, &small)
            .unwrap_err()
            .to_string()
            .contains("limit")
    );
    let few = StepImportOptions {
        max_entities: 10,
        ..Default::default()
    };
    assert!(
        import_step(&text, &few)
            .unwrap_err()
            .to_string()
            .contains("entities")
    );
    let deep = format!(
        "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n#1=A({}{});\nENDSEC;\nEND-ISO-10303-21;\n",
        "(".repeat(5000),
        ")".repeat(5000)
    );
    assert!(
        import_step(&deep, &StepImportOptions::default())
            .unwrap_err()
            .to_string()
            .contains("nesting")
    );
}

#[test]
fn unsupported_content_is_named() {
    // BREP_WITH_VOIDS is refused explicitly (voids would be lost).
    let text =
        fixture("box_40x30x10_mm.step").replacen("MANIFOLD_SOLID_BREP(", "BREP_WITH_VOIDS(", 1);
    let e = import_step(&text, &StepImportOptions::default()).unwrap_err();
    assert!(e.to_string().contains("BREP_WITH_VOIDS #"), "{e}");
    // An unsupported curve type names the entity type and id.
    let text = fixture("cylinder_d20_h15_mm.step").replacen("CIRCLE(", "HYPERBOLA(", 1);
    let e = import_step(&text, &StepImportOptions::default()).unwrap_err();
    assert!(e.to_string().contains("HYPERBOLA #"), "{e}");
}

/// Downstream editability: import a FreeCAD box, cut a centred Ø6 through hole from its
/// top-face frame, measure, tessellate, deep copy and export.
#[test]
fn imported_box_can_be_cut_measured_and_exported() {
    let imp = import("box_40x30x10_mm.step");
    let s = &imp.solids[0];
    let top = s
        .planar_faces
        .iter()
        .find(|f| f.frame.z == [0.0, 0.0, 1.0])
        .unwrap();
    let hole = Profile::Circle {
        center: [0.0, 0.0],
        radius: 3.0,
    };
    let cut = cut_through_all(&s.body, &hole, &top.frame).unwrap();
    assert_closed_single(&cut);
    // Exact Z boundary integral (ADR 0001): same tolerance as the plate tests.
    let expected = 12000.0 - 90.0 * PI;
    assert_exact_volume(&cut, expected);
    assert!(check_mesh(&tessellate(&cut, 0.01).unwrap()).watertight);
    let copy = cut.deep_copy().unwrap();
    assert_exact_volume(&copy, expected);
    // Side-face hole along X from an imported side-face frame.
    let side = s
        .planar_faces
        .iter()
        .find(|f| f.frame.z == [1.0, 0.0, 0.0])
        .unwrap();
    let side_cut = cut_through_all(
        &cut,
        &Profile::Circle {
            center: [-9.0, 0.0],
            radius: 2.0,
        },
        &side.frame,
    )
    .unwrap();
    assert_closed_single(&side_cut);
    // Ø4 along X at (y, z) = (6, 5) misses the Ø6 Z-hole at (x, y) = (20, 15).
    // Holes along Z and X: no single exact axis exists, so the volume is the
    // labelled tessellation estimate, which must contain the analytic value.
    let (v, err, method) = volume(&side_cut);
    let exp2 = expected - 4.0 * PI * 40.0;
    assert_eq!(method, VolumeMethod::TessellationEstimate);
    assert!((v - exp2).abs() <= err, "{v} +- {err} vs {exp2}");
    // STEP export of the edited body; re-import (not independent: covers the
    // B-spline surface / rational curve import path; FreeCAD validation is in
    // docs/formats/step.md).
    let text = export_step(&cut, "R-CAD test").unwrap();
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("imported_box_hole.step");
    std::fs::write(&out, &text).unwrap();
    let back = import_step(&text, &StepImportOptions::default()).unwrap();
    assert_eq!(back.solids.len(), 1);
    assert_exact_volume(&back.solids[0].body, expected);
}

#[test]
fn imported_plate_with_hole_takes_a_second_hole() {
    let imp = import("plate_hole_mm.step");
    let s = &imp.solids[0];
    let top = s
        .planar_faces
        .iter()
        .find(|f| f.frame.z == [0.0, 0.0, 1.0])
        .unwrap();
    // Frame origin is the face centroid (30, 20, 8); hole at (10, 10).
    let cut = cut_through_all(
        &s.body,
        &Profile::Circle {
            center: [-20.0, -10.0],
            radius: 3.0,
        },
        &top.frame,
    )
    .unwrap();
    assert_closed_single(&cut);
    assert_exact_volume(&cut, 60.0 * 40.0 * 8.0 - PI * 25.0 * 8.0 - PI * 9.0 * 8.0);
}
