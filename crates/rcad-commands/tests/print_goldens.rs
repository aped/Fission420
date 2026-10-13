//! Read-back of the committed print exports in `examples/print/` (validated externally by
//! FreeCAD, the stdlib-Python mesh checker and Bambu Studio, see its README). Nothing here
//! re-exports: the committed bytes are parsed by the readers and compared with analytic
//! values, so a reader regression, or a regenerated file that drifted, fails here on every CI
//! platform without the external tools.
//!
//! Expected values follow the derivations in `print_examples.rs` (same parameters, same
//! chord bound), restated independently of the export report.
use rcad_commands::printables::{COUPON, COUPON_CLEARANCES, ENCLOSURE_BASELINE, ENCLOSURE_VARIANT};
use rcad_exchange::mesh::{MeshDiagnostics, diagnose};
use rcad_exchange::{stl, threemf};
use rcad_kernel::{StepImportOptions, VolumeResult, import_step, measure};
use std::f64::consts::PI;
use std::path::PathBuf;

/// Export chord (`tol::MESH_CHORD`), mm: sagitta bound of curved faces.
const CHORD: f64 = 0.01;
/// f32 coordinate rounding allowance on mesh volumes, mm^3.
const F32_VOL: f64 = 0.5;
/// f32 allowance on the bed contact height, mm.
const F32_LEN: f64 = 1e-4;

fn file(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/print")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Mesh volume interval of a tessellated cylinder of radius `r`, height `h`: the
/// inscribed polygon with sagitta <= CHORD has area in [pi (r - s)^2, pi r^2].
fn cyl(r: f64, h: f64) -> (f64, f64) {
    (PI * (r - CHORD).powi(2) * h, PI * r * r * h)
}

/// Closed, consistently outward, resting on z = 0, volume in `[lo, hi]` +- F32_VOL.
fn check_mesh(what: &str, d: &MeshDiagnostics, (lo, hi): (f64, f64)) {
    assert!(d.closed, "{what}: not closed: {d:?}");
    assert_eq!(d.non_manifold_edges, 0, "{what}");
    assert_eq!(d.degenerate_triangles, 0, "{what}");
    let v = d.signed_volume.unwrap();
    assert!(
        v >= lo - F32_VOL && v <= hi + F32_VOL,
        "{what}: mesh volume {v} outside [{lo}, {hi}]"
    );
    let (min, _) = d.bounds.unwrap();
    assert!(min[2].abs() <= F32_LEN, "{what}: min z {}", min[2]);
}

fn read_stl(name: &str) -> MeshDiagnostics {
    let m = stl::read(&file(name), &stl::StlLimits::default()).unwrap();
    diagnose(&m.positions, &m.triangles)
}

/// 3MF objects in file order: `(name, diagnostics)`; asserts millimetre units.
fn read_3mf(name: &str) -> Vec<(String, MeshDiagnostics)> {
    let imp = threemf::read(&file(name), &threemf::ThreeMfLimits::default()).unwrap();
    assert_eq!(imp.unit, threemf::ThreeMfUnit::Millimeter, "{name}");
    imp.objects
        .into_iter()
        .map(|o| (o.name.unwrap_or_default(), o.diagnostics))
        .collect()
}

/// One solid with the exact volume. Planar or axis-ruled bodies measure exactly
/// (relative 1e-7); a tessellation estimate must hold within 3x its error estimate.
fn check_step(name: &str, want: f64) {
    let text = String::from_utf8(file(name)).unwrap();
    let imp = import_step(&text, &StepImportOptions::default()).unwrap();
    assert_eq!(imp.solids.len(), 1, "{name}");
    let m = measure(&imp.solids[0].body).unwrap();
    assert!(m.all_shells_closed, "{name}");
    match m.volume {
        VolumeResult::Value {
            volume,
            error_estimate,
            ..
        } => assert!(
            (volume - want).abs() <= (3.0 * error_estimate).max(want * 1e-7),
            "{name}: {volume} vs {want} (error estimate {error_estimate})"
        ),
        v => panic!("{name}: {v:?}"),
    }
}

#[test]
fn enclosure_exports_read_back_to_analytic_volumes() {
    for (stem, p) in [
        ("enclosure_baseline", ENCLOSURE_BASELINE),
        ("enclosure_variant", ENCLOSURE_VARIANT),
    ] {
        // All faces planar: the mesh volume is exact up to f32 rounding.
        let base = (p.base_volume(), p.base_volume());
        let lid = (p.lid_volume(), p.lid_volume());
        let objs = read_3mf(&format!("{stem}.3mf"));
        assert_eq!(objs.len(), 2, "{stem}");
        for (n, d) in &objs {
            let want = if n.contains("base") { base } else { lid };
            check_mesh(&format!("{stem}.3mf {n}"), d, want);
        }
        let d = read_stl(&format!("{stem}-Enclosure_base.stl"));
        check_mesh(&format!("{stem} base stl"), &d, base);
        let d = read_stl(&format!("{stem}-Enclosure_lid.stl"));
        check_mesh(&format!("{stem} lid stl"), &d, lid);
        check_step(&format!("{stem}-body1.step"), p.base_volume());
        check_step(&format!("{stem}-body2.step"), p.lid_volume());
    }
}

#[test]
fn coupon_exports_read_back_to_analytic_volumes() {
    let (d, t, h, sp) = COUPON;
    let len = sp * COUPON_CLEARANCES.len() as f64 + 10.0;
    let slab = len * 20.0 * t;
    let holes = COUPON_CLEARANCES
        .iter()
        .map(|c| cyl((d + 2.0 * c) / 2.0, t))
        .fold((0.0, 0.0), |a, (lo, hi)| (a.0 + lo, a.1 + hi));
    // Smallest mesh plate = slab minus the largest (circumscribed-bound) holes.
    let plate = (slab - holes.1, slab - holes.0);
    let peg = cyl(d / 2.0, h);

    let objs = read_3mf("coupon.3mf");
    assert_eq!(objs.len(), 2);
    check_mesh("coupon.3mf plate", &objs[0].1, plate);
    check_mesh("coupon.3mf peg", &objs[1].1, peg);
    let d_plate = read_stl("coupon-Coupon_plate_holes_c_0_15_0_25_0_35_0_45_per_side.stl");
    check_mesh("coupon plate stl", &d_plate, plate);
    let d_peg = read_stl("coupon-Peg_6_nominal_test_in_each_hole.stl");
    check_mesh("coupon peg stl", &d_peg, peg);

    let holes_exact: f64 = COUPON_CLEARANCES
        .iter()
        .map(|c| PI * ((d + 2.0 * c) / 2.0).powi(2) * t)
        .sum();
    check_step("coupon-body1.step", slab - holes_exact);
    check_step("coupon-body2.step", PI * (d / 2.0).powi(2) * h);
}

#[test]
fn bracket_exports_read_back_to_analytic_volumes() {
    // 60 x 40 x 5 foot + 50 x 5 x 35 upright, 1 mm chamfer along a 60 mm edge
    // (0.5 * 1 * 1 * 60), four Ø5.5 through holes in 5 mm plate.
    let solid = 60.0 * 40.0 * 5.0 + 50.0 * 5.0 * 35.0 - 0.5 * 60.0;
    let r = 5.5 / 2.0;
    let (h_lo, h_hi) = cyl(r, 5.0);
    let mesh = (solid - 4.0 * h_hi, solid - 4.0 * h_lo);
    let objs = read_3mf("bracket.3mf");
    assert_eq!(objs.len(), 1);
    check_mesh("bracket.3mf", &objs[0].1, mesh);
    check_mesh("bracket.stl", &read_stl("bracket.stl"), mesh);
    check_step("bracket-body1.step", solid - 4.0 * PI * r * r * 5.0);
}
