//! Cooperative cancellation of the readers (`rcad_cancel`): with the flag set,
//! every reader stops at its first poll with `ExchangeError::Cancelled`; with the
//! flag cleared the same bytes read normally. One test in its own binary: the
//! flag is process-wide.
use rcad_exchange::{ExchangeError, dxf, stl, svg, threemf};

fn corpus(f: &str) -> Vec<u8> {
    std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tools/fixtures/corpus")
            .join(f),
    )
    .unwrap()
}

#[test]
fn readers_stop_when_cancellation_is_requested() {
    let stl_bin = corpus("m1_cylinder_binary.stl");
    let stl_asc = corpus("m2_sphere_ascii.stl");
    let tmf = corpus("h6_three_items_cm.3mf");
    let dxf_b = corpus("h7_counters_mm.dxf");
    let svg_t = corpus("h8_counters_mm.svg");
    let cancelled = |r: Result<(), ExchangeError>, what: &str| match r {
        Err(ExchangeError::Cancelled { .. }) => {}
        other => panic!("{what}: {other:?}"),
    };
    rcad_cancel::request();
    cancelled(
        stl::read(&stl_bin, &Default::default()).map(|_| ()),
        "binary STL",
    );
    cancelled(
        stl::read(&stl_asc, &Default::default()).map(|_| ()),
        "ASCII STL",
    );
    cancelled(threemf::read(&tmf, &Default::default()).map(|_| ()), "3MF");
    cancelled(
        dxf::import(&dxf_b, &dxf::DxfImportOptions::default()).map(|_| ()),
        "DXF",
    );
    cancelled(
        svg::import(&svg_t, &svg::SvgImportOptions::default()).map(|_| ()),
        "SVG",
    );
    rcad_cancel::clear();
    assert!(stl::read(&stl_bin, &Default::default()).is_ok());
    assert!(stl::read(&stl_asc, &Default::default()).is_ok());
    assert!(threemf::read(&tmf, &Default::default()).is_ok());
    assert!(dxf::import(&dxf_b, &dxf::DxfImportOptions::default()).is_ok());
    assert!(svg::import(&svg_t, &svg::SvgImportOptions::default()).is_ok());
}
