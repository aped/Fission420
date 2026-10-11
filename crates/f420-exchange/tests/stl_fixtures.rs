//! STL reader/diagnostics against independently written fixtures
//! (tools/fixtures/stl/README.md). Expected values are analytic.

use f420_exchange::ExchangeError;
use f420_exchange::mesh::diagnose;
use f420_exchange::stl::{self, StlEncoding, StlLimits, StlMesh};

fn fixture(name: &str) -> Vec<u8> {
    let p = format!("{}/tests/fixtures/stl/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn read(name: &str) -> Result<StlMesh, ExchangeError> {
    stl::read(&fixture(name), &StlLimits::default())
}

fn sorted_positions(m: &StlMesh) -> Vec<[f64; 3]> {
    let mut v = m.positions.clone();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v
}

/// 20 x 10 x 5 box, corner at the origin: V = 1000, A = 2(200 + 100 + 50) = 700.
#[test]
fn freecad_box_ascii_and_binary_agree_and_are_closed() {
    let a = read("box_freecad_ascii.stl").unwrap();
    let b = read("box_freecad_binary.stl").unwrap();
    assert_eq!(a.encoding, StlEncoding::Ascii);
    assert_eq!(b.encoding, StlEncoding::Binary);
    assert_eq!(a.name.as_deref(), Some("box20x10x5"));
    assert_eq!(a.triangles.len(), 12);
    assert_eq!(b.triangles.len(), 12);
    assert_eq!(a.positions.len(), 8);
    assert_eq!(sorted_positions(&a), sorted_positions(&b));
    for i in 0..12 {
        assert_eq!(a.triangle(i), b.triangle(i), "triangle {i}");
    }
    for m in [&a, &b] {
        let d = diagnose(&m.positions, &m.triangles);
        assert!(d.closed, "{d:?}");
        assert_eq!(d.boundary_edges, 0);
        assert_eq!(d.non_manifold_edges, 0);
        assert_eq!(d.inconsistent_orientation_edges, 0);
        assert_eq!(d.degenerate_triangles, 0);
        assert_eq!(d.components, 1);
        assert_eq!(d.bounds, Some(([0.0; 3], [20.0, 10.0, 5.0])));
        assert!((d.signed_volume.unwrap() - 1000.0).abs() < 1e-9);
        assert!((d.surface_area - 700.0).abs() < 1e-9);
    }
}

#[test]
fn scale_inches_to_mm() {
    let mut m = read("box_freecad_binary.stl").unwrap();
    m.scale(25.4).unwrap();
    let d = diagnose(&m.positions, &m.triangles);
    assert_eq!(d.bounds.unwrap().1, [20.0 * 25.4, 10.0 * 25.4, 5.0 * 25.4]);
    let v = 1000.0 * 25.4f64.powi(3);
    assert!((d.signed_volume.unwrap() - v).abs() < 1e-9 * v);
}

/// Box minus its two top facets: the 20 x 10 rim is 4 boundary edges, area 700 - 200.
#[test]
fn open_box_has_boundary_and_no_volume() {
    let m = read("box_open_freecad.stl").unwrap();
    let d = diagnose(&m.positions, &m.triangles);
    assert_eq!(d.triangles, 10);
    assert_eq!(d.boundary_edges, 4);
    assert!(!d.closed);
    assert_eq!(d.signed_volume, None);
    assert!((d.surface_area - 500.0).abs() < 1e-9);
}

/// Binary file whose header starts with "solid": detected by length, not by the token.
/// Tetrahedron with legs 10: V = 1000/6.
#[test]
fn binary_with_solid_header() {
    let m = read("tet_solid_header.stl").unwrap();
    assert_eq!(m.encoding, StlEncoding::Binary);
    assert!(
        m.name
            .as_deref()
            .unwrap()
            .starts_with("solid tetra_but_binary")
    );
    let d = diagnose(&m.positions, &m.triangles);
    assert!(d.closed);
    assert!((d.signed_volume.unwrap() - 1000.0 / 6.0).abs() < 1e-9);
}

#[test]
fn broken_files_are_errors() {
    match read("box_truncated.stl") {
        Err(ExchangeError::Truncated { message, .. }) => {
            assert!(message.contains("664"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        read("ascii_malformed.stl"),
        Err(ExchangeError::Malformed { line: Some(4), .. })
    ));
    assert!(matches!(
        read("ascii_nan.stl"),
        Err(ExchangeError::NonFinite { line: Some(5), .. })
    ));
    assert!(matches!(
        read("ascii_unterminated.stl"),
        Err(ExchangeError::Truncated { .. })
    ));
    // Errors serialize for the command layer.
    let e = read("ascii_nan.stl").unwrap_err();
    let json = serde_json::to_string(&e).unwrap();
    assert_eq!(serde_json::from_str::<ExchangeError>(&json).unwrap(), e);
}

#[test]
fn limits_are_enforced_for_both_encodings() {
    let tight = StlLimits {
        max_bytes: 1 << 20,
        max_triangles: 11,
    };
    for f in ["box_freecad_ascii.stl", "box_freecad_binary.stl"] {
        assert!(
            matches!(
                stl::read(&fixture(f), &tight),
                Err(ExchangeError::LimitExceeded { .. })
            ),
            "{f}"
        );
    }
    let small = StlLimits {
        max_bytes: 100,
        max_triangles: 100,
    };
    assert!(matches!(
        stl::read(&fixture("box_freecad_ascii.stl"), &small),
        Err(ExchangeError::LimitExceeded { .. })
    ));
}

/// Supplementary (same reader): our binary/ASCII writers reproduce the FreeCAD mesh.
#[test]
fn writer_round_trip_of_freecad_box() {
    let m = read("box_freecad_ascii.stl").unwrap();
    for bytes in [
        stl::write_binary(&m).unwrap(),
        stl::write_ascii(&m).unwrap().into_bytes(),
    ] {
        let r = stl::read(&bytes, &StlLimits::default()).unwrap();
        assert_eq!(r.positions, m.positions);
        assert_eq!(r.triangles, m.triangles);
    }
    assert!(!stl::write_binary(&m).unwrap().starts_with(b"solid"));
}
