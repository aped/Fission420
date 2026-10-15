//! Volume properties (volume, centroid, central second moments) against closed-form
//! values written independently here (box, cylinder, tube, wedge, frustum, torus,
//! sphere), on every exact method (Z, X, revolution) and the labelled tessellation
//! estimate.
use rcad_kernel::*;
use std::f64::consts::PI;

type M3 = [[f64; 3]; 3];

fn diag(a: f64, b: f64, c: f64) -> M3 {
    [[a, 0.0, 0.0], [0.0, b, 0.0], [0.0, 0.0, c]]
}

/// Exact-method acceptance: relative 1e-9 of the scale of each quantity.
fn check(
    label: &str,
    p: &VolumeProperties,
    method: VolumeMethod,
    v: f64,
    c: [f64; 3],
    s: M3,
    rel: f64,
) {
    let size = v.cbrt();
    let smax = s.iter().flatten().fold(0.0f64, |a, x| a.max(x.abs()));
    eprintln!(
        "{label}: V {:.12} (want {v:.12}) c {:?} (want {c:?}) {:?}; errors V {:.2e} c {:.2e} S {:.2e}",
        p.volume, p.centroid, p.method, p.volume_error, p.centroid_error, p.second_moments_error
    );
    assert_eq!(p.method, method, "{label}");
    assert!(
        (p.volume - v).abs() <= rel * v,
        "{label}: V {} vs {v}",
        p.volume
    );
    for i in 0..3 {
        assert!(
            (p.centroid[i] - c[i]).abs() <= rel * size.max(c[i].abs()),
            "{label}: c {:?} vs {c:?}",
            p.centroid
        );
        for (j, want) in s[i].iter().enumerate() {
            let got = p.second_moments[i][j];
            assert!(
                (got - want).abs() <= rel * smax,
                "{label}: S{i}{j} {got} vs {want}"
            );
        }
    }
}

fn props(b: &Body) -> VolumeProperties {
    let p = volume_properties(b).unwrap();
    // Same method and volume as the plain measurement.
    match measure(b).unwrap().volume {
        VolumeResult::Value { volume, method, .. } => {
            assert_eq!(method, p.method);
            assert!(
                (volume - p.volume).abs() <= 1e-9 * volume.abs() + 1e-9,
                "measure {volume} vs properties {}",
                p.volume
            );
        }
        VolumeResult::Unavailable { reason } => panic!("{reason}"),
    }
    p
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Profile {
    Profile::Polygon(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
}

fn circle(cx: f64, cy: f64, r: f64) -> Profile {
    Profile::Circle {
        center: [cx, cy],
        radius: r,
    }
}

/// Second moments of a solid box with edges (a, b, c) about its centre: V a²/12 ...
fn box_s(a: f64, b: f64, c: f64) -> M3 {
    let v = a * b * c;
    diag(v * a * a / 12.0, v * b * b / 12.0, v * c * c / 12.0)
}

#[test]
fn box_20x30x40_off_origin() {
    let b = extrude(&rect(5.0, -10.0, 25.0, 20.0), &Frame::XY.offset(3.0), 40.0).unwrap();
    let p = props(&b);
    check(
        "box",
        &p,
        VolumeMethod::BrepBoundaryIntegralZ,
        24000.0,
        [15.0, 5.0, 23.0],
        box_s(20.0, 30.0, 40.0),
        1e-9,
    );
    // Inertia at 1.2 g/cm³ = 1.2e-3 g/mm³: I_zz = m (a² + b²) / 12.
    let i = p.inertia(1.2e-3);
    let m = 28.8;
    assert!((i[2][2] - m * (400.0 + 900.0) / 12.0).abs() < 1e-9 * i[2][2]);
    assert!((i[0][0] - m * (900.0 + 1600.0) / 12.0).abs() < 1e-9 * i[0][0]);
    assert!(i[0][1].abs() < 1e-9 * i[0][0]);
}

#[test]
fn cylinder_and_tube_about_z() {
    let (r, h, cx, cy) = (10.0, 25.0, 5.0, -3.0);
    let cyl = extrude(&circle(cx, cy, r), &Frame::XY, h).unwrap();
    let v = PI * r * r * h;
    check(
        "cylinder",
        &props(&cyl),
        VolumeMethod::BrepBoundaryIntegralZ,
        v,
        [cx, cy, h / 2.0],
        diag(v * r * r / 4.0, v * r * r / 4.0, v * h * h / 12.0),
        1e-9,
    );
    // Hollow: coaxial Ø12 bore (cut through all).
    let ri = 6.0;
    let mut f = Frame::XY;
    f.origin = [cx, cy, 0.0];
    let tube = cut_through_all(&cyl, &circle(0.0, 0.0, ri), &f).unwrap();
    let v = PI * (r * r - ri * ri) * h;
    let sxx = v * (r * r + ri * ri) / 4.0;
    check(
        "tube",
        &props(&tube),
        VolumeMethod::BrepBoundaryIntegralZ,
        v,
        [cx, cy, h / 2.0],
        diag(sxx, sxx, v * h * h / 12.0),
        1e-9,
    );
}

#[test]
fn box_with_off_centre_hole_has_a_shifted_centroid() {
    // 60 x 40 x 20 box at the origin corner, Ø10 hole along Z at (15, 10).
    let b = extrude(&rect(0.0, 0.0, 60.0, 40.0), &Frame::XY, 20.0).unwrap();
    let mut f = Frame::XY.offset(20.0);
    f.origin = [15.0, 10.0, 20.0];
    let b = cut_through_all(&b, &circle(0.0, 0.0, 5.0), &f).unwrap();
    let (vb, vh) = (48000.0, PI * 25.0 * 20.0);
    let v = vb - vh;
    // First moments about the world origin, by subtraction.
    let c = [
        (vb * 30.0 - vh * 15.0) / v,
        (vb * 20.0 - vh * 10.0) / v,
        10.0,
    ];
    // Second moments about the world origin, then shifted to the centroid.
    let sb = box_s(60.0, 40.0, 20.0);
    let sh = diag(vh * 25.0 / 4.0, vh * 25.0 / 4.0, vh * 400.0 / 12.0);
    let (bc, hc) = ([30.0, 20.0, 10.0], [15.0, 10.0, 10.0]);
    let mut s = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let o = sb[i][j] + vb * bc[i] * bc[j] - (sh[i][j] + vh * hc[i] * hc[j]);
            s[i][j] = o - v * c[i] * c[j];
        }
    }
    check(
        "box with hole",
        &props(&b),
        VolumeMethod::BrepBoundaryIntegralZ,
        v,
        c,
        s,
        1e-9,
    );
}

#[test]
fn hole_along_x_uses_the_x_method() {
    // 60 x 40 x 20 box, Ø6 hole along X through (y, z) = (20, 10): symmetric, so the
    // centroid stays at the box centre; S subtracts the hole cylinder's moments.
    let b = extrude(&rect(0.0, 0.0, 60.0, 40.0), &Frame::XY, 20.0).unwrap();
    let f = Frame {
        origin: [60.0, 20.0, 10.0],
        x: [0.0, 1.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [1.0, 0.0, 0.0],
    };
    let b = cut_through_all(&b, &circle(0.0, 0.0, 3.0), &f).unwrap();
    let vh = PI * 9.0 * 60.0;
    let v = 48000.0 - vh;
    let sb = box_s(60.0, 40.0, 20.0);
    let sh = diag(vh * 3600.0 / 12.0, vh * 9.0 / 4.0, vh * 9.0 / 4.0);
    let mut s = sb;
    for i in 0..3 {
        s[i][i] -= sh[i][i];
    }
    check(
        "hole along X",
        &props(&b),
        VolumeMethod::BrepBoundaryIntegralX,
        v,
        [30.0, 20.0, 10.0],
        s,
        1e-9,
    );
}

/// Area, first and second moments of a triangle in (x, z): standard vertex formulas.
fn tri_moments(p: [[f64; 2]; 3]) -> [f64; 6] {
    let a = 0.5
        * ((p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]));
    let sx: f64 = p.iter().map(|q| q[0]).sum();
    let sz: f64 = p.iter().map(|q| q[1]).sum();
    let sq = |i: usize| {
        let s: f64 = p.iter().map(|q| q[i] * q[i]).sum();
        let c = p[0][i] * p[1][i] + p[1][i] * p[2][i] + p[0][i] * p[2][i];
        a / 6.0 * (s + c)
    };
    let xz = a / 12.0 * (p.iter().map(|q| q[0] * q[1]).sum::<f64>() + sx * sz);
    // [A, ∫x, ∫z, ∫x², ∫z², ∫xz]
    [a, a * sx / 3.0, a * sz / 3.0, sq(0), sq(1), xz]
}

#[test]
fn tilted_wedge_matches_polygon_moments() {
    // Trapezoid (0,0) (60,0) (60,20) (0,30) in XZ extruded 40 along -Y (tilted top).
    let trap = Profile::Polygon(vec![[0.0, 0.0], [60.0, 0.0], [60.0, 20.0], [0.0, 30.0]]);
    let b = extrude(&trap, &Frame::XZ, 40.0).unwrap();
    let t1 = tri_moments([[0.0, 0.0], [60.0, 0.0], [60.0, 20.0]]);
    let t2 = tri_moments([[0.0, 0.0], [60.0, 20.0], [0.0, 30.0]]);
    let m: Vec<f64> = (0..6).map(|i| t1[i] + t2[i]).collect();
    let (l, yc) = (40.0, -20.0);
    let v = m[0] * l;
    let c = [m[1] / m[0], yc, m[2] / m[0]];
    // About the centroid: x/z from the polygon, y from the extrusion length.
    let sxx = m[3] * l - v * c[0] * c[0];
    let szz = m[4] * l - v * c[2] * c[2];
    let sxz = m[5] * l - v * c[0] * c[2];
    let syy = m[0] * l * l * l / 12.0;
    let s = [[sxx, 0.0, sxz], [0.0, syy, 0.0], [sxz, 0.0, szz]];
    check(
        "wedge",
        &props(&b),
        VolumeMethod::BrepBoundaryIntegralZ,
        v,
        c,
        s,
        1e-9,
    );
}

/// ∫_0^h f(z) dz by Boole's rule (exact for polynomials of degree <= 5).
fn boole(h: f64, f: impl Fn(f64) -> f64) -> f64 {
    let d = h / 4.0;
    2.0 * d / 45.0
        * (7.0 * f(0.0) + 32.0 * f(d) + 12.0 * f(2.0 * d) + 32.0 * f(3.0 * d) + 7.0 * f(h))
}

#[test]
fn frustum_with_holes_by_revolution() {
    // Ø40 -> Ø30 over 30 along Z, coaxial Ø6 bore, off-axis Ø4 hole at x = 9.
    let (r0, r1, h) = (20.0, 15.0, 30.0);
    let frustum = loft(
        &circle(0.0, 0.0, r0),
        &Frame::XY,
        &circle(0.0, 0.0, r1),
        &Frame::XY.offset(h),
    )
    .unwrap();
    let rho = |z: f64| r0 + (r1 - r0) * z / h;
    // Solid of revolution: V = π∫ρ², ∫z = π∫ρ² z, ∫x² = ∫y² = π/4 ∫ρ⁴, ∫z² = π∫ρ² z².
    let v_f = PI * boole(h, |z| rho(z).powi(2));
    let mz_f = PI * boole(h, |z| rho(z).powi(2) * z);
    let xx_f = PI / 4.0 * boole(h, |z| rho(z).powi(4));
    let zz_f = PI * boole(h, |z| rho(z).powi(2) * z * z);
    let p = props(&frustum);
    let cz = mz_f / v_f;
    // Textbook frustum centroid as a cross-check of the reference itself.
    let cz_book =
        h * (r0 * r0 + 2.0 * r0 * r1 + 3.0 * r1 * r1) / (4.0 * (r0 * r0 + r0 * r1 + r1 * r1));
    assert!((cz - cz_book).abs() < 1e-12 * h);
    check(
        "frustum",
        &p,
        VolumeMethod::BrepBoundaryIntegralRevolution,
        v_f,
        [0.0, 0.0, cz],
        diag(xx_f, xx_f, zz_f - v_f * cz * cz),
        1e-9,
    );
    let b = cut_through_all(&frustum, &circle(0.0, 0.0, 3.0), &Frame::XY).unwrap();
    let b = cut_through_all(&b, &circle(9.0, 0.0, 2.0), &Frame::XY).unwrap();
    // Cylinders (radius a, centre x0, z 0..h) removed: V = π a² h, ∫x = V x0,
    // ∫x² = V (a²/4 + x0²), ∫y² = V a²/4, ∫z = V h/2, ∫z² = V h²/3.
    let cyl = |a: f64, x0: f64| {
        let v = PI * a * a * h;
        [
            v,
            v * x0,
            v * h / 2.0,
            v * (a * a / 4.0 + x0 * x0),
            v * a * a / 4.0,
            v * h * h / 3.0,
            v * x0 * h / 2.0,
        ]
    };
    let (c1, c2) = (cyl(3.0, 0.0), cyl(2.0, 9.0));
    let f = [v_f, 0.0, mz_f, xx_f, xx_f, zz_f, 0.0];
    let m: Vec<f64> = (0..7).map(|i| f[i] - c1[i] - c2[i]).collect();
    let v = m[0];
    let c = [m[1] / v, 0.0, m[2] / v];
    let s = [
        [m[3] - v * c[0] * c[0], 0.0, m[6] - v * c[0] * c[2]],
        [0.0, m[4], 0.0],
        [m[6] - v * c[0] * c[2], 0.0, m[5] - v * c[2] * c[2]],
    ];
    check(
        "frustum with holes",
        &props(&b),
        VolumeMethod::BrepBoundaryIntegralRevolution,
        v,
        c,
        s,
        1e-9,
    );
}

#[test]
fn torus_by_revolution() {
    let (big_r, r) = (40.0, 10.0);
    let b = revolve(&circle(big_r, 0.0, r), &Frame::XZ, 2.0 * PI).unwrap();
    let v = 2.0 * PI * PI * big_r * r * r;
    // ∫(x² + y²) = V (R² + 3r²/4), ∫z² = V r²/4 (the axis is world Z through 0).
    let sxx = v * (big_r * big_r + 0.75 * r * r) / 2.0;
    check(
        "torus",
        &props(&b),
        VolumeMethod::BrepBoundaryIntegralRevolution,
        v,
        [0.0, 0.0, 0.0],
        diag(sxx, sxx, v * r * r / 4.0),
        1e-9,
    );
}

#[test]
fn partial_revolution_has_the_pappus_centroid() {
    // Rectangle r in [5, 12], z in [0, 6] revolved by a half turn about Z (from the
    // XZ frame's +X towards -Y... whichever side): the centroid lies on the bisector at
    // distance (∫ρ² dA · 2 sin(θ/2)) / (∫ρ dA · θ) from the axis.
    let b = revolve(&rect(5.0, 0.0, 12.0, 6.0), &Frame::XZ, PI).unwrap();
    let p = props(&b);
    let th = PI;
    let (a0, a1, hz) = (5.0f64, 12.0f64, 6.0);
    let v = th / 2.0 * (a1 * a1 - a0 * a0) * hz;
    let d = (a1.powi(3) - a0.powi(3)) / 3.0 * hz * 2.0 * (th / 2.0).sin() / (v);
    let dist = (p.centroid[0].powi(2) + p.centroid[1].powi(2)).sqrt();
    eprintln!("half ring: {p:?}, expected centroid distance {d}");
    assert_eq!(p.method, VolumeMethod::BrepBoundaryIntegralZ);
    assert!((p.volume - v).abs() < 1e-9 * v);
    assert!((dist - d).abs() < 1e-9 * a1, "{dist} vs {d}");
    assert!((p.centroid[2] - 3.0).abs() < 1e-9);
}

#[test]
fn sphere_uses_the_labelled_tessellation_estimate() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/step/sphere_r10_mm.step");
    let text = std::fs::read_to_string(path).unwrap();
    let imp = import_step(&text, &StepImportOptions::default()).unwrap();
    let b = &imp.solids[0].body;
    let p = props(b);
    let r = 10.0;
    let v = 4.0 / 3.0 * PI * r * r * r;
    let s = v * r * r / 5.0;
    eprintln!("sphere: {p:?}");
    assert_eq!(p.method, VolumeMethod::TessellationEstimate);
    assert!(!p.is_exact());
    // The estimate contains the true value (it over-estimates the fine error ~3x).
    assert!(
        (p.volume - v).abs() <= p.volume_error,
        "V {} vs {v} ± {}",
        p.volume,
        p.volume_error
    );
    for i in 0..3 {
        assert!(
            (p.centroid[i] - 5.0).abs() <= p.centroid_error.max(1e-9 * r) + 1e-6,
            "{:?}",
            p.centroid
        );
        assert!(
            (p.second_moments[i][i] - s).abs() <= p.second_moments_error,
            "S{i}{i} {} vs {s} ± {}",
            p.second_moments[i][i],
            p.second_moments_error
        );
    }
    assert!(p.volume_error < 1e-3 * v);
}

#[test]
fn triangle_mesh_properties_are_exact_for_the_mesh() {
    // The box's own welded tessellation: a polyhedron identical to the box.
    let b = extrude(&rect(0.0, 0.0, 20.0, 30.0), &Frame::XY, 40.0).unwrap();
    let m = tessellate(&b, 0.01).unwrap();
    let p = mesh_volume_properties(&m).unwrap();
    assert_eq!(p.method, VolumeMethod::TriangleMesh);
    assert!((p.volume - 24000.0).abs() < 1e-9 * 24000.0);
    for (i, c) in [10.0, 15.0, 20.0].iter().enumerate() {
        assert!((p.centroid[i] - c).abs() < 1e-9);
    }
    let s = box_s(20.0, 30.0, 40.0);
    for i in 0..3 {
        assert!((p.second_moments[i][i] - s[i][i]).abs() < 1e-9 * s[2][2]);
    }
}
