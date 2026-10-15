//! Line/arc loop profiles (`Profile::Loop`): exact arc edges through extrude,
//! measurement, tessellation, cuts and STEP. Expected values are derived by hand
//! from elementary geometry (sector / segment / rectangle areas), not from the
//! implementation.
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

/// Half disc of radius `r` above the x axis: arc (r,0) -> (-r,0) CCW, then the diameter.
fn d_shape(r: f64) -> Profile {
    Profile::Loop(vec![arc(r, 0.0, 0.0, 0.0, true), line(-r, 0.0)])
}

/// Rounded rectangle centred at the origin, CCW from the bottom edge.
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

/// Stadium: centres (+-len/2, 0), radius r; two exact semicircles.
fn slot(len: f64, r: f64) -> Profile {
    let c = len / 2.0;
    Profile::Loop(vec![
        line(-c, -r),
        arc(c, -r, c, 0.0, true),
        line(c, r),
        arc(-c, r, -c, 0.0, true),
    ])
}

fn vol(m: &Measurements) -> f64 {
    match m.volume {
        VolumeResult::Value { volume, .. } => volume,
        VolumeResult::Unavailable { ref reason } => panic!("volume unavailable: {reason}"),
    }
}

/// Same volume tolerance convention as tests/it/plate.rs.
fn assert_volume(v: f64, expected: f64) {
    let tol = 0.001f64.max(expected * 1e-7);
    assert!((v - expected).abs() <= tol, "volume {v} vs {expected}");
}

fn assert_bounds(m: &Measurements, min: [f64; 3], max: [f64; 3]) {
    for i in 0..3 {
        assert!((m.bounds.min[i] - min[i]).abs() <= 1e-6, "{:?}", m.bounds);
        assert!((m.bounds.max[i] - max[i]).abs() <= 1e-6, "{:?}", m.bounds);
    }
}

fn check_solid(body: &Body, expected: f64) -> Measurements {
    let m = measure(body).unwrap();
    assert_eq!(m.shell_count, 1);
    assert!(m.all_shells_closed);
    assert_volume(vol(&m), expected);
    let mesh = tessellate(body, 0.01).unwrap();
    let mc = check_mesh(&mesh);
    assert!(mc.watertight, "{mc:?}");
    assert!(
        (mc.mesh_volume - expected).abs() / expected <= 1e-3,
        "{mc:?} vs {expected}"
    );
    m
}

fn write_artifact(name: &str, text: &str) {
    std::fs::create_dir_all("../../artifacts").ok();
    std::fs::write(format!("../../artifacts/{name}"), text).unwrap();
}

#[test]
fn d_shape_extrusion_is_half_cylinder() {
    let (r, h) = (10.0, 7.0);
    let body = extrude(&d_shape(r), &Frame::XY, h).unwrap();
    let m = check_solid(&body, PI * r * r * h / 2.0);
    assert_bounds(&m, [-r, 0.0, 0.0], [r, r, h]);
    // bottom, top, flat side, curved side as two quarter-turn patches (arcs are
    // split into <= 90 degree single-span edges)
    assert_eq!(m.face_count, 5);
    assert_eq!(planar_faces_on(&body, &Frame::XY.offset(h)), 1);
}

#[test]
fn clockwise_loop_is_rejected() {
    // The same half disc traversed clockwise: arc (-r,0) -> (r,0) over the top, CW.
    let cw = Profile::Loop(vec![arc(-10.0, 0.0, 0.0, 0.0, false), line(10.0, 0.0)]);
    let e = extrude(&cw, &Frame::XY, 5.0).unwrap_err();
    assert!(format!("{e}").contains("counter-clockwise"), "{e}");
}

#[test]
fn rounded_rectangle_80x50_r8() {
    let (w, h, r, t) = (80.0, 50.0, 8.0, 10.0);
    let body = extrude(&rounded_rect(w, h, r), &Frame::XY, t).unwrap();
    let area = w * h - (4.0 - PI) * r * r;
    let m = check_solid(&body, area * t);
    // Extrema lie on the straight edges: exact.
    assert_bounds(&m, [-40.0, -25.0, 0.0], [40.0, 25.0, t]);
    assert_eq!(m.face_count, 10);
    let text = export_step(&body, "test").unwrap();
    assert!(text.contains("B_SPLINE_CURVE_WITH_KNOTS"));
    assert!(text.contains("RATIONAL_B_SPLINE_CURVE"));
    write_artifact("kernel_rounded_rect_80x50x10_r8.step", &text);
}

#[test]
fn slot_with_exact_semicircles() {
    // Each cap sweeps exactly pi: a single-span quadratic arc would have weight 0.
    let (len, r, t) = (60.0, 10.0, 5.0);
    let body = extrude(&slot(len, r), &Frame::XY, t).unwrap();
    let m = check_solid(&body, (len * 2.0 * r + PI * r * r) * t);
    assert_bounds(&m, [-40.0, -10.0, 0.0], [40.0, 10.0, t]);
    // caps, two flat sides, two quarter patches per semicircle
    assert_eq!(m.face_count, 8);
    write_artifact(
        "kernel_slot_60_r10x5.step",
        &export_step(&body, "test").unwrap(),
    );
}

#[test]
fn three_quarter_sector_sweep_over_pi() {
    // Lines (0,0)->(10,0), arc CCW from angle 0 to -pi/2 (sweep 3pi/2), line back.
    let (r, t) = (10.0, 4.0);
    let p = Profile::Loop(vec![
        line(0.0, 0.0),
        arc(r, 0.0, 0.0, 0.0, true),
        line(0.0, -r),
    ]);
    let body = extrude(&p, &Frame::XY, t).unwrap();
    let m = check_solid(&body, 0.75 * PI * r * r * t);
    assert_bounds(&m, [-r, -r, 0.0], [r, r, t]);
    write_artifact(
        "kernel_sector270_r10x4.step",
        &export_step(&body, "test").unwrap(),
    );
}

#[test]
fn d_shape_step_export() {
    let body = extrude(&d_shape(10.0), &Frame::XY, 7.0).unwrap();
    let text = export_step(&body, "test").unwrap();
    assert!(text.starts_with("ISO-10303-21;"));
    assert!(text.contains("RATIONAL_B_SPLINE_CURVE"));
    assert!(!text.contains("INTERSECTION_CURVE"));
    write_artifact("kernel_d_shape_r10x7.step", &text);
}

#[test]
fn loop_on_tilted_frame_keeps_volume() {
    let body = extrude(&slot(60.0, 10.0), &Frame::XZ, 5.0).unwrap();
    // Volume needs Z-ruled/horizontal faces; on XZ the sides are not. Check the mesh.
    let mesh = tessellate(&body, 0.01).unwrap();
    let mc = check_mesh(&mesh);
    let expected = (60.0 * 20.0 + PI * 100.0) * 5.0;
    assert!(mc.watertight);
    assert!(
        (mc.mesh_volume - expected).abs() / expected <= 1e-3,
        "{mc:?}"
    );
}

#[test]
fn through_all_hole_in_rounded_rectangle() {
    let (w, h, r, t) = (80.0, 50.0, 8.0, 10.0);
    let plate = extrude(&rounded_rect(w, h, r), &Frame::XY, t).unwrap();
    let hole = Profile::Circle {
        center: [30.0, 0.0],
        radius: 5.0,
    };
    let body = cut_through_all(&plate, &hole, &Frame::XY.offset(t)).unwrap();
    let area = w * h - (4.0 - PI) * r * r - PI * 25.0;
    let m = check_solid(&body, area * t);
    assert_bounds(&m, [-40.0, -25.0, 0.0], [40.0, 25.0, t]);
    let text = export_step(&body, "test").unwrap();
    assert!(!text.contains("INTERSECTION_CURVE"));
    write_artifact("kernel_rounded_rect_hole.step", &text);
}

#[test]
fn loop_validation_rejects_bad_input() {
    let f = &Frame::XY;
    let bad = [
        // one segment
        Profile::Loop(vec![arc(10.0, 0.0, 0.0, 0.0, true)]),
        // zero chord
        Profile::Loop(vec![
            line(0.0, 0.0),
            line(10.0, 0.0),
            line(10.0, 0.0),
            line(0.0, 10.0),
        ]),
        // end (-9, 0) is not on the radius-10 circle
        Profile::Loop(vec![arc(10.0, 0.0, 0.0, 0.0, true), line(-9.0, 0.0)]),
        // radius below MIN_LENGTH
        Profile::Loop(vec![arc(1e-4, 0.0, 0.0, 0.0, true), line(-1e-4, 0.0)]),
        // non-finite
        Profile::Loop(vec![arc(10.0, 0.0, f64::NAN, 0.0, true), line(-10.0, 0.0)]),
        Profile::Loop(vec![]),
    ];
    for p in bad {
        assert!(
            matches!(extrude(&p, f, 5.0), Err(KernelError::InvalidInput(_))),
            "{p:?}"
        );
    }
}

#[test]
fn revolve_of_loop_respects_axis_side() {
    // Half disc centred at x = 20 bulging toward -x: the arc spans angle pi, so its
    // min x is 20 - 5 = 15 (> 0): accepted.
    let ok = Profile::Loop(vec![arc(20.0, 5.0, 20.0, 0.0, true), line(20.0, -5.0)]);
    let body = revolve(&ok, &Frame::XZ, PI).unwrap();
    let mesh = tessellate(&body, 0.01).unwrap();
    let mc = check_mesh(&mesh);
    // Pappus: area pi*25/2, centroid x = 20 - 4r/(3pi); swept angle pi.
    let r: f64 = 5.0;
    let expected = (PI * r * r / 2.0) * (20.0 - 4.0 * r / (3.0 * PI)) * PI;
    assert!(mc.watertight);
    assert!(
        (mc.mesh_volume.abs() - expected).abs() / expected <= 2e-3,
        "{mc:?} vs {expected}"
    );
    // Same shape moved so the arc crosses the axis (endpoints stay at x = 3 > 0).
    let bad = Profile::Loop(vec![arc(3.0, 5.0, 3.0, 0.0, true), line(3.0, -5.0)]);
    assert!(matches!(
        revolve(&bad, &Frame::XZ, PI),
        Err(KernelError::InvalidInput(_))
    ));
}

/// Quarter-arc corners whose sweep is pi/2 only up to rounding (as from a
/// solved sketch) still make one edge each: 10 faces, 24 edges, 16 vertices.
#[test]
fn nearly_quarter_arcs_stay_single_edges() {
    let Profile::Loop(mut segs) = rounded_rect(80.0, 50.0, 8.0) else {
        unreachable!()
    };
    // Rotate every arc start back by 1e-12 rad about its centre: each CCW sweep
    // becomes pi/2 + 1e-12.
    let t = -1e-12_f64;
    for s in &mut segs {
        if let LoopSegment::Arc { start, center, .. } = s {
            let (x, y) = (start[0] - center[0], start[1] - center[1]);
            *start = [
                center[0] + x * t.cos() - y * t.sin(),
                center[1] + x * t.sin() + y * t.cos(),
            ];
        }
    }
    let p = Profile::Loop(segs);
    let body = extrude(&p, &Frame::XY, 10.0).unwrap();
    let m = measure(&body).unwrap();
    assert_eq!([m.face_count, m.edge_count, m.vertex_count], [10, 24, 16]);
}
