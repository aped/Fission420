//! 3MF reader/writer against independent fixtures (tools/fixtures/threemf/README.md):
//! FreeCAD 1.1.4's 3MF writer and hand-written packages (Python stdlib). Expected values
//! are analytic, from the fixture construction.
use f420_exchange::ExchangeError;
use f420_exchange::threemf::{self, MeshObject, ThreeMfLimits, ThreeMfUnit};

fn fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/threemf")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn read(name: &str) -> threemf::ThreeMfImport {
    threemf::read(&fixture(name), &ThreeMfLimits::default())
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn err(name: &str) -> ExchangeError {
    threemf::read(&fixture(name), &ThreeMfLimits::default())
        .err()
        .unwrap_or_else(|| panic!("{name} was accepted"))
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn bounds_close(d: &f420_exchange::mesh::MeshDiagnostics, min: [f64; 3], max: [f64; 3]) {
    let (lo, hi) = d.bounds.unwrap();
    for i in 0..3 {
        assert!(
            close(lo[i], min[i], 1e-9) && close(hi[i], max[i], 1e-9),
            "{lo:?} {hi:?}"
        );
    }
}

#[test]
fn freecad_two_objects_mm() {
    // Part.makeBox(10,10,10) and makeBox(20,10,5) at (50,0,0), FreeCAD Mesh 3MF writer.
    let r = read("freecad_two_boxes.3mf");
    assert_eq!(r.unit, ThreeMfUnit::Millimeter);
    assert_eq!(r.objects.len(), 2);
    let a = &r.objects[0].diagnostics;
    let b = &r.objects[1].diagnostics;
    assert!(a.closed && b.closed && a.triangles == 12 && b.triangles == 12);
    assert!(close(a.signed_volume.unwrap(), 1000.0, 1e-9));
    assert!(close(b.signed_volume.unwrap(), 1000.0, 1e-9));
    bounds_close(a, [0.0; 3], [10.0; 3]);
    bounds_close(b, [50.0, 0.0, 0.0], [70.0, 10.0, 5.0]);
    assert!(
        r.metadata
            .contains(&("Application".into(), "FreeCAD".into()))
    );
}

#[test]
fn inch_unit_and_item_transform() {
    // Box 1 x 0.5 x 0.25 in, item rotated +90 deg about Z and moved +2 in X:
    // x [1.5, 2] in, y [0, 1] in, z [0, 0.25] in; V = 0.125 in^3.
    let r = read("box_inch_rotated.3mf");
    assert_eq!(r.unit, ThreeMfUnit::Inch);
    let o = &r.objects[0];
    assert_eq!(o.name.as_deref(), Some("Box"));
    let d = &o.diagnostics;
    assert!(d.closed);
    bounds_close(d, [38.1, 0.0, 0.0], [50.8, 25.4, 6.35]);
    let v = 0.125 * 25.4f64.powi(3);
    assert!(close(d.signed_volume.unwrap(), v, 1e-9 * v));
}

#[test]
fn components_are_flattened_and_mirrors_keep_outward_winding() {
    // Two right tetrahedra (legs 10), one mirrored in X and moved to x in [20, 30]:
    // V = 2 * 1000/6, two components, closed and outward.
    let r = read("components_mirror_stored.3mf");
    assert_eq!(r.objects.len(), 1);
    let o = &r.objects[0];
    assert_eq!((o.name.as_deref(), o.object_id), (Some("Pair"), 2));
    let d = &o.diagnostics;
    assert!(d.closed && d.components == 2 && d.triangles == 8);
    assert!(close(d.signed_volume.unwrap(), 2000.0 / 6.0, 1e-9));
    bounds_close(d, [0.0; 3], [30.0, 10.0, 10.0]);
    assert!(r.warnings.iter().any(|w| w.contains("mirroring")));
}

#[test]
fn object_colours_are_imported_and_the_rest_is_reported() {
    let r = read("colors.3mf");
    let c: Vec<_> = r.objects.iter().map(|o| o.color).collect();
    assert_eq!(
        c,
        vec![Some([255, 0, 0, 128]), Some([0, 255, 0, 255]), None]
    );
    let w = r.warnings.join(" | ");
    assert!(w.contains("materials extension"), "{w}");
    assert!(w.contains("1 triangle(s) carry their own"), "{w}");
    assert!(w.contains("property group 6"), "{w}");
    for (i, o) in r.objects.iter().enumerate() {
        assert!(close(o.diagnostics.signed_volume.unwrap(), 1000.0, 1e-9));
        let x0 = 20.0 * i as f64;
        bounds_close(&o.diagnostics, [x0, 0.0, 0.0], [x0 + 10.0, 10.0, 10.0]);
    }
}

#[test]
fn unsupported_and_malformed_packages_are_refused() {
    assert!(matches!(
        err("requires_production.3mf"),
        ExchangeError::Unsupported { .. }
    ));
    let e = err("bad_index.3mf").to_string();
    assert!(e.contains("references a vertex beyond 8"), "{e}");
    assert!(matches!(err("no_build.3mf"), ExchangeError::Empty { .. }));
    assert!(err("doctype.3mf").to_string().contains("DOCTYPE"));
    let e = err("lying_size.3mf").to_string();
    assert!(
        e.contains("larger than declared") || e.contains("declared"),
        "{e}"
    );
}

#[test]
fn decompressed_size_is_bounded() {
    // 2 MiB of padding: fine by default, refused with a 1 MiB model limit before
    // inflating (the declared size is checked first).
    let r = read("padded_2mib.3mf");
    assert!(r.objects[0].diagnostics.closed);
    let small = ThreeMfLimits {
        max_model_bytes: 1 << 20,
        ..Default::default()
    };
    assert!(matches!(
        threemf::read(&fixture("padded_2mib.3mf"), &small),
        Err(ExchangeError::LimitExceeded { .. })
    ));
    let few = ThreeMfLimits {
        max_triangles: 11,
        ..Default::default()
    };
    assert!(matches!(
        threemf::read(&fixture("no_rels.3mf"), &few),
        Err(ExchangeError::LimitExceeded { .. })
    ));
}

#[test]
fn missing_relationships_fall_back_with_a_warning() {
    let r = read("no_rels.3mf");
    assert!(r.warnings.iter().any(|w| w.contains("_rels/.rels")));
    assert!(close(
        r.objects[0].diagnostics.signed_volume.unwrap(),
        1.0,
        1e-12
    ));
}

#[test]
fn every_prefix_and_byte_flip_of_a_fixture_is_handled() {
    for name in ["freecad_two_boxes.3mf", "components_mirror_stored.3mf"] {
        let z = fixture(name);
        for n in 0..z.len() {
            assert!(
                threemf::read(&z[..n], &ThreeMfLimits::default()).is_err(),
                "{name} prefix {n}"
            );
        }
        for i in 0..z.len() {
            let mut c = z.clone();
            c[i] = c[i].wrapping_add(0x41);
            let _ = threemf::read(&c, &ThreeMfLimits::default()); // must not panic
        }
    }
}

#[test]
fn writer_output_reads_back_with_units_names_and_colours() {
    // Supplementary own-writer round trip (independent checks of the writer:
    // tools/fixtures/threemf/validate_writer.sh with FreeCAD and Python).
    let fc = read("freecad_two_boxes.3mf");
    let objects: Vec<MeshObject> = fc
        .objects
        .iter()
        .zip([Some([10, 20, 30, 255]), None])
        .map(|(o, color)| MeshObject {
            name: Some(format!("obj {}", o.object_id)),
            positions: o.positions.clone(),
            triangles: o.triangles.clone(),
            color,
        })
        .collect();
    for unit in [
        ThreeMfUnit::Millimeter,
        ThreeMfUnit::Inch,
        ThreeMfUnit::Micron,
    ] {
        let bytes = threemf::write(&objects, unit).unwrap();
        let back = threemf::read(&bytes, &ThreeMfLimits::default()).unwrap();
        assert_eq!(back.unit, unit);
        assert_eq!(back.objects.len(), 2);
        assert_eq!(back.objects[0].color, Some([10, 20, 30, 255]));
        for (a, b) in fc.objects.iter().zip(&back.objects) {
            let (va, vb) = (
                a.diagnostics.signed_volume.unwrap(),
                b.diagnostics.signed_volume.unwrap(),
            );
            assert!(close(va, vb, 1e-9 * va), "{unit:?}: {va} vs {vb}");
        }
    }
    assert!(threemf::write(&[], ThreeMfUnit::Millimeter).is_err());
    let bad = MeshObject {
        name: None,
        positions: vec![[0.0; 3]],
        triangles: vec![[0, 0, 1]],
        color: None,
    };
    assert!(threemf::write(&[bad], ThreeMfUnit::Millimeter).is_err());
}
