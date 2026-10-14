//! STEP export -> import round trips of curved bodies (rational B-spline arcs,
//! revolved surfaces, chamfer). The other STEP tests only string-check these
//! exports; here each file is read back by `import_step` and the imported body is
//! checked against ANALYTIC volume and bounds (derived below from elementary
//! geometry), not against the pre-export measurement.
//!
//! Caveat: writer and reader are both ours, so
//! this is a regression guard for the pair, not independent interoperability
//! evidence; the FreeCAD fixtures in tests/step_import.rs cover that direction.
//!
//! Tolerances (docs/TOLERANCES.md):
//! - volume: the exact-path acceptance threshold max(0.001 mm³, 1e-7 × V) used by
//!   tests/arcs.rs, tests/plate.rs and tests/step_import.rs; the measured
//!   `error_estimate` (difference of the two Richardson extrapolants) must also be
//!   inside it, so a pass is not luck of the estimator.
//! - bounds: 1e-6 mm (`tol::KERNEL_LINEAR`), the value arcs.rs / plate.rs use. STEP
//!   REALs are rounded to 1e-9 on export, well inside it.
//! - mesh: watertight at the default 0.01 mm chord, mesh volume within 1e-3
//!   relative (arcs.rs / plate.rs convention).
use rcad_kernel::*;
use std::f64::consts::PI;

fn line(x: f64, y: f64) -> LoopSegment {
    LoopSegment::Line { start: [x, y] }
}

fn arc(x: f64, y: f64, cx: f64, cy: f64, ccw: bool) -> LoopSegment {
    LoopSegment::Arc {
        start: [x, y],
        center: [cx, cy],
        ccw,
    }
}

/// Same construction as tests/arcs.rs `rounded_rect`.
fn rounded_rect(w: f64, h: f64, r: f64) -> Profile {
    let (a, b) = (w / 2.0, h / 2.0);
    Profile::Loop(vec![
        line(-a + r, -b),
        arc(a - r, -b, a - r, -b + r, true),
        line(a, -b + r),
        arc(a, b - r, a - r, b - r, true),
        line(a - r, b),
        arc(-a + r, b, -a + r, b - r, true),
        line(-a, b - r),
        arc(-a, -b + r, -a + r, -b + r, true),
    ])
}

/// Same construction as tests/arcs.rs `slot`.
fn slot(len: f64, r: f64) -> Profile {
    let c = len / 2.0;
    Profile::Loop(vec![
        line(-c, -r),
        arc(c, -r, c, 0.0, true),
        line(c, r),
        arc(-c, r, -c, 0.0, true),
    ])
}

/// Same construction as tests/arcs.rs `d_shape`.
fn d_shape(r: f64) -> Profile {
    Profile::Loop(vec![arc(r, 0.0, 0.0, 0.0, true), line(-r, 0.0)])
}

fn vol_tol(v: f64) -> f64 {
    0.001_f64.max(v.abs() * 1e-7)
}

/// Export, re-import, and return the single imported body.
fn roundtrip(body: &Body) -> Body {
    let text = export_step(body, "test").expect("export_step");
    let imp = import_step(&text, &StepImportOptions::default())
        .unwrap_or_else(|e| panic!("import_step of our own export failed: {e}"));
    assert_eq!(imp.solids.len(), 1, "solids: {:?}", imp.stats);
    assert_eq!(imp.stats.solids, 1);
    assert_eq!(imp.length_unit.to_mm, 1.0);
    imp.solids.into_iter().next().unwrap().body
}

/// Analytic checks on the re-imported body.
fn check_imported(b: &Body, expected: f64, min: [f64; 3], max: [f64; 3]) {
    let m = measure(b).unwrap();
    assert_eq!(m.shell_count, 1);
    assert!(m.all_shells_closed);
    match m.volume {
        VolumeResult::Value {
            volume,
            error_estimate,
            method,
        } => {
            let tol = vol_tol(expected);
            assert!(
                (volume - expected).abs() <= tol,
                "volume {volume} vs analytic {expected} (err {:e}, tol {tol:e}, est {error_estimate:e}, {method:?})",
                volume - expected
            );
            assert!(
                error_estimate.abs() <= tol,
                "error estimate {error_estimate:e} exceeds tol {tol:e} ({method:?})"
            );
        }
        VolumeResult::Unavailable { reason } => panic!("volume unavailable: {reason}"),
    }
    for i in 0..3 {
        assert!(
            (m.bounds.min[i] - min[i]).abs() <= 1e-6 && (m.bounds.max[i] - max[i]).abs() <= 1e-6,
            "bounds {:?} vs {min:?} {max:?}",
            m.bounds
        );
    }
    let mc = check_mesh(&tessellate(b, 0.01).unwrap());
    assert!(mc.watertight, "{mc:?}");
    assert!(
        (mc.mesh_volume - expected).abs() / expected <= 1e-3,
        "{mc:?} vs {expected}"
    );
}

#[test]
fn rounded_rectangle_roundtrip() {
    // Rectangle minus four corner squares r² plus four quarter discs πr²/4:
    // A = w·h − (4 − π)·r², V = A·t. Extrema lie on the straight edges.
    let (w, h, r, t) = (80.0, 50.0, 8.0, 10.0);
    let body = extrude(&rounded_rect(w, h, r), &Frame::XY, t).unwrap();
    let imp = roundtrip(&body);
    let v = (w * h - (4.0 - PI) * r * r) * t;
    check_imported(&imp, v, [-w / 2.0, -h / 2.0, 0.0], [w / 2.0, h / 2.0, t]);
}

#[test]
fn slot_roundtrip() {
    // Stadium: centre rectangle len × 2r plus two semicircles = one disc πr².
    // V = (2·r·len + π·r²)·t; x extent ±(len/2 + r), y extent ±r.
    let (len, r, t) = (60.0, 10.0, 5.0);
    let body = extrude(&slot(len, r), &Frame::XY, t).unwrap();
    let imp = roundtrip(&body);
    let v = (2.0 * r * len + PI * r * r) * t;
    check_imported(&imp, v, [-(len / 2.0 + r), -r, 0.0], [len / 2.0 + r, r, t]);
}

#[test]
fn d_shape_roundtrip() {
    // Half disc of radius r extruded h: half cylinder V = π·r²·h / 2.
    // Bounds: x ∈ [−r, r], y ∈ [0, r] (apex of the arc at angle π/2), z ∈ [0, h].
    let (r, h) = (10.0, 7.0);
    let body = extrude(&d_shape(r), &Frame::XY, h).unwrap();
    let imp = roundtrip(&body);
    check_imported(&imp, PI * r * r * h / 2.0, [-r, 0.0, 0.0], [r, r, h]);
}

#[test]
fn sector_270_roundtrip() {
    // Sector of sweep 3π/2: A = (θ/2)·r² = (3π/4)·r², V = A·t. The arc runs from
    // angle 0 CCW to −π/2 through π/2 and π, so it reaches x = −r, y = +r; the
    // centre corner (0,0) and the endpoint (0,−r) give y = −r: bounds ±r.
    let (r, t) = (10.0, 4.0);
    let p = Profile::Loop(vec![
        line(0.0, 0.0),
        arc(r, 0.0, 0.0, 0.0, true),
        line(0.0, -r),
    ]);
    let body = extrude(&p, &Frame::XY, t).unwrap();
    let imp = roundtrip(&body);
    check_imported(&imp, 0.75 * PI * r * r * t, [-r, -r, 0.0], [r, r, t]);
}

#[test]
fn revolve_washer_roundtrip() {
    // Rectangle x ∈ [20, 40], z ∈ [0, 10] in XZ revolved about Z (same as
    // tests/plate.rs). Pappus: V = A · θ · x̄. truck-stepio writes the revolved
    // faces as SURFACE_OF_REVOLUTION, which the importer reads.
    let (ri, ro, h) = (20.0, 40.0, 10.0);
    let prof = Profile::Polygon(vec![[ri, 0.0], [ro, 0.0], [ro, h], [ri, h]]);
    // Half turn (about -Y from the XZ frame: the half with y <= 0 or y >= 0):
    // read back exactly.
    let body = revolve(&prof, &Frame::XZ, PI).unwrap();
    let v = (ro - ri) * h * PI * (ri + ro) / 2.0;
    let b = bounds(&body).unwrap();
    let back = roundtrip(&body);
    check_imported(&back, v, b.min, b.max);
    // Full turn: the last sector reaches the surface's seam angle, which
    // does not read back yet (exact volume, broken tessellation observed);
    // it must be refused with a structured error, never imported wrongly.
    let body = revolve(&prof, &Frame::XZ, 2.0 * PI).unwrap();
    let text = export_step(&body, "test").unwrap();
    assert!(text.contains("SURFACE_OF_REVOLUTION"));
    match import_step(&text, &StepImportOptions::default()) {
        Err(KernelError::InvalidInput(msg)) => {
            assert!(msg.contains("SURFACE_OF_REVOLUTION"), "{msg}")
        }
        Err(e) => panic!("expected InvalidInput, got {e}"),
        Ok(imp) => check_imported(
            &imp.solids[0].body,
            PI * (ro * ro - ri * ri) * h,
            [-ro, -ro, 0.0],
            [ro, ro, h],
        ),
    }
}

#[test]
fn chamfered_block_roundtrip() {
    // 40 × 30 × 10 block, chamfer d = 2 on the top-front edge (z = 10, y = 0),
    // same as src/ops/tests.rs chamfer_box_edge_and_refusals. The removed wedge is
    // a right-triangle prism: (d²/2)·40 = 80, so V = 12000 − 80. The bbox is
    // unchanged (the chamfer only trims one edge; other edges keep the extremes).
    let block = extrude(
        &Profile::Polygon(vec![[0.0, 0.0], [40.0, 0.0], [40.0, 30.0], [0.0, 30.0]]),
        &Frame::XY,
        10.0,
    )
    .unwrap();
    let front = Frame {
        origin: [0.0, 0.0, 0.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [0.0, -1.0, 0.0],
    };
    let d = 2.0;
    let body = chamfer_edge(&block, &Frame::XY.offset(10.0), &front, d).unwrap();
    let imp = roundtrip(&body);
    assert_eq!(face_count(&imp), 7);
    check_imported(
        &imp,
        40.0 * 30.0 * 10.0 - 0.5 * d * d * 40.0,
        [0.0; 3],
        [40.0, 30.0, 10.0],
    );
}
