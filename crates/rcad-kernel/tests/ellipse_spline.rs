//! Ellipse and B-spline profiles (`LoopSegment::Cubic` spans): exact NURBS
//! edges through extrude, measurement, cut / join with the independent result
//! check, revolve and STEP. Expected values are closed-form (ellipse area πab,
//! Pappus for revolutions); the spans are built here from the ellipse's
//! parametric form, independently of the sketch crate.
use rcad_kernel::*;
use std::f64::consts::{FRAC_1_SQRT_2, PI};

/// Exact ellipse spans (centre `c`, semi-axes `a` along x, `b` along y):
/// each quarter is the rational quadratic (P0, P0 + P2 − C, P2; 1, √2/2, 1),
/// elevated to degree 3.
fn ellipse(c: [f64; 2], a: f64, b: f64) -> Profile {
    let q = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)];
    let at = |u: f64, v: f64| [c[0] + a * u, c[1] + b * v];
    let w = FRAC_1_SQRT_2;
    let segs = (0..4)
        .map(|j| {
            let (u0, v0) = q[j];
            let (u1, v1) = q[(j + 1) % 4];
            let (p0, m, p2) = (at(u0, v0), at(u0 + u1, v0 + v1), at(u1, v1));
            let k = 1.0 + 2.0 * w;
            LoopSegment::Cubic {
                start: p0,
                c1: [(p0[0] + 2.0 * w * m[0]) / k, (p0[1] + 2.0 * w * m[1]) / k],
                c2: [(2.0 * w * m[0] + p2[0]) / k, (2.0 * w * m[1] + p2[1]) / k],
                w: [k / 3.0, k / 3.0],
            }
        })
        .collect();
    Profile::Loop(segs)
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Profile {
    Profile::Polygon(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
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

fn assert_volume(b: &Body, expected: f64) {
    let (v, est, method) = volume(b);
    let tol = 1e-3f64.max(expected * 1e-7);
    assert!(
        (v - expected).abs() <= tol,
        "volume {v} vs {expected} (err {:e}, est {est:e}, {method:?})",
        v - expected
    );
}

#[test]
fn extruded_ellipse_volume_bounds_and_faces() {
    let (a, b, h) = (20.0, 10.0, 15.0);
    let body = extrude(&ellipse([0.0, 0.0], a, b), &Frame::XY, h).unwrap();
    let m = measure(&body).unwrap();
    assert_eq!((m.shell_count, m.all_shells_closed), (1, true));
    // Four quarter side faces plus two caps.
    assert_eq!(m.face_count, 6);
    assert_volume(&body, PI * a * b * h);
    for i in 0..3 {
        let (lo, hi) = ([-a, -b, 0.0][i], [a, b, h][i]);
        assert!(
            (m.bounds.min[i] - lo).abs() < 1e-6 && (m.bounds.max[i] - hi).abs() < 1e-6,
            "{:?}",
            m.bounds
        );
    }
    let mc = check_mesh(&tessellate(&body, 0.01).unwrap());
    assert!(mc.watertight, "{mc:?}");
}

#[test]
fn rectangle_with_elliptical_hole() {
    let hole = ellipse([5.0, 0.0], 15.0, 8.0);
    let body = extrude_region(&rect(-30.0, -20.0, 30.0, 20.0), &[hole], &Frame::XY, 10.0).unwrap();
    assert_volume(&body, (60.0 * 40.0 - PI * 15.0 * 8.0) * 10.0);
}

#[test]
fn elliptical_cut_through_a_block_passes_the_result_check() {
    let block = extrude(&rect(-30.0, -20.0, 30.0, 20.0), &Frame::XY, 10.0).unwrap();
    let e = ellipse([0.0, 0.0], 12.0, 6.0);
    let start = Frame::XY.offset(-2.0);
    let cut = match cut_prism(&block, &e, &start, 14.0) {
        Ok(c) => c,
        Err(err) => panic!("elliptical cut failed: {err}"),
    };
    assert_volume(&cut, (60.0 * 40.0 - PI * 12.0 * 6.0) * 10.0);
    let prism = Prism {
        profile: &e,
        holes: &[],
        start: &start,
        length: 14.0,
    };
    let chk = verify_prism_boolean(&block, &cut, PrismOp::Cut, &prism, (0.0, 14.0)).unwrap();
    assert!(chk.probes > chk.skipped, "{chk:?}");
}

#[test]
fn elliptical_boss_joined_on_a_block() {
    let block = extrude(&rect(-30.0, -20.0, 30.0, 20.0), &Frame::XY, 10.0).unwrap();
    let e = ellipse([0.0, 0.0], 12.0, 6.0);
    // Overlap 1 mm into the block (no coplanar faces).
    let start = Frame::XY.offset(9.0);
    let joined = join_prism(&block, &e, &start, 6.0).unwrap();
    assert_volume(&joined, 60.0 * 40.0 * 10.0 + PI * 12.0 * 6.0 * 5.0);
    let prism = Prism {
        profile: &e,
        holes: &[],
        start: &start,
        length: 6.0,
    };
    verify_prism_boolean(&block, &joined, PrismOp::Join, &prism, (0.0, 6.0)).unwrap();
}

#[test]
fn revolved_ellipse_matches_pappus() {
    // Ellipse centred at x = 30 in the XZ plane, revolved a full turn about Z
    // (the frame's local y axis): V = 2π · 30 · πab.
    let (a, b) = (8.0, 5.0);
    let body = revolve(&ellipse([30.0, 0.0], a, b), &Frame::XZ, 2.0 * PI).unwrap();
    let (v, est, method) = volume(&body);
    let want = 2.0 * PI * 30.0 * PI * a * b;
    // The exact parametric boundary integral covers the revolved rational
    // face; a tessellation estimate would have to cover its error.
    match method {
        VolumeMethod::BrepBoundaryIntegralParametric => {
            assert!((v - want).abs() <= 1e-6 * want, "{v} vs {want}")
        }
        VolumeMethod::TessellationEstimate => {
            assert!((v - want).abs() <= est, "{v} vs {want} (estimate {est:e})");
            assert!(est <= 2e-3 * want, "{est:e}");
        }
        m => panic!("unexpected volume method {m:?}"),
    }
    // Profiles touching the axis are refused (convex-hull bound).
    assert!(revolve(&ellipse([7.9, 0.0], a, b), &Frame::XZ, PI).is_err());
}

#[test]
fn step_round_trip_keeps_the_rational_curves() {
    let (a, b, h) = (20.0, 10.0, 15.0);
    let body = extrude(&ellipse([0.0, 0.0], a, b), &Frame::XY, h).unwrap();
    let text = export_step(&body, "test").unwrap();
    assert!(
        text.contains("RATIONAL_B_SPLINE_CURVE"),
        "rational curves are written"
    );
    let imp = import_step(&text, &StepImportOptions::default()).unwrap();
    assert_eq!(imp.solids.len(), 1);
    assert_volume(&imp.solids[0].body, PI * a * b * h);
}

#[test]
fn spline_span_profile_extrudes_and_sweep_refuses() {
    // D shape: the line (0,0) → (w,0), then one polynomial cubic span with
    // control points (w,0), (w,h), (0,h), (0,0) back over the top.
    // With s = 1 − t: x(t) = w(s³ + 3s²t), x'(t) = −6wst, y(t) = 3hst, so the
    // enclosed area is ∫ y |x'| dt = 18hw ∫ s²t² dt = 18hw / 30 = (3/5) h w.
    let (w, h) = (30.0, 20.0);
    let segs = vec![
        LoopSegment::Line { start: [0.0, 0.0] },
        LoopSegment::Cubic {
            start: [w, 0.0],
            c1: [w, h],
            c2: [0.0, h],
            w: [1.0, 1.0],
        },
    ];
    let body = extrude(&Profile::Loop(segs.clone()), &Frame::XY, 5.0).unwrap();
    assert_volume(&body, 0.6 * h * w * 5.0);
    let r = sweep(
        &Profile::Loop(segs),
        &Frame::XY,
        &[PathSegment::Line {
            end: [0.0, 0.0, 10.0],
        }],
    );
    assert!(matches!(r, Err(KernelError::InvalidInput(m)) if m.contains("spline")));
}

/// Writes artifacts/step_curves/*.step for tools/check_step_curves.sh
/// (FreeCAD / OpenCascade reads them independently).
#[test]
#[ignore = "writes artifacts/ for the external FreeCAD check; run via tools/check_step_curves.sh"]
fn write_curve_step_artifacts() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/step_curves");
    std::fs::create_dir_all(&dir).unwrap();
    let prism = extrude(&ellipse([0.0, 0.0], 20.0, 10.0), &Frame::XY, 15.0).unwrap();
    let block = extrude(&rect(-30.0, -20.0, 30.0, 20.0), &Frame::XY, 10.0).unwrap();
    let cut = cut_prism(
        &block,
        &ellipse([0.0, 0.0], 12.0, 6.0),
        &Frame::XY.offset(-2.0),
        14.0,
    )
    .unwrap();
    let (w, h) = (30.0, 20.0);
    let d = Profile::Loop(vec![
        LoopSegment::Line { start: [0.0, 0.0] },
        LoopSegment::Cubic {
            start: [w, 0.0],
            c1: [w, h],
            c2: [0.0, h],
            w: [1.0, 1.0],
        },
    ]);
    let spline = extrude(&d, &Frame::XY, 5.0).unwrap();
    for (name, b) in [
        ("ellipse_prism", &prism),
        ("ellipse_cut_block", &cut),
        ("spline_d_prism", &spline),
    ] {
        std::fs::write(
            dir.join(format!("{name}.step")),
            export_step(b, "test").unwrap(),
        )
        .unwrap();
    }
}

/// Cuts by elliptical prisms get explicit exact intersection edges (the
/// cap plane meets the extruded rational cubic surface in the translated
/// span, rebuilt and verified by `exact.rs`), so later measurement does not
/// evaluate implicit curves: four sequential cuts, each volume closed-form.
#[test]
fn elliptical_cuts_get_exact_intersection_edges() {
    let mut b = extrude(&rect(-40.0, -20.0, 40.0, 20.0), &Frame::XY, 10.0).unwrap();
    let hole = PI * 6.0 * 4.0 * 10.0;
    for (i, x) in [-25.0, -10.0, 5.0, 25.0].into_iter().enumerate() {
        let e = ellipse([x, 0.0], 6.0, 4.0);
        let start = Frame::XY.offset(-2.0);
        let nb = cut_prism(&b, &e, &start, 14.0).unwrap();
        assert!(!has_implicit_edges(&nb), "cut {i} kept implicit edges");
        let prism = Prism {
            profile: &e,
            holes: &[],
            start: &start,
            length: 14.0,
        };
        verify_prism_boolean(&b, &nb, PrismOp::Cut, &prism, (0.0, 14.0)).unwrap();
        assert_volume(&nb, 32_000.0 - (i + 1) as f64 * hole);
        b = nb;
    }
}
