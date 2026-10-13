//! Write DXF exports of known sketches for external validation
//! (tools/fixtures/dxf/validate_export.sh runs FreeCAD's DXF importer on them).
//!
//! usage: cargo run --release -p rcad-exchange --example export_samples -- OUT_DIR

use rcad_exchange::dxf::{self, DxfExportOptions, DxfImportOptions};
use rcad_sketch::Sketch;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::args()
        .nth(1)
        .ok_or("usage: export_samples OUT_DIR")?;
    let out = Path::new(&out);
    std::fs::create_dir_all(out)?;
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dxf");
    for (src, dst, unit) in [
        ("slot_freecad_r14.dxf", "export_slot.dxf", None),
        ("lwpoly_bulge.dxf", "export_bulge.dxf", None),
        ("notch_freecad_r12.dxf", "export_notch.dxf", Some(1.0)),
    ] {
        let opts = DxfImportOptions {
            unit_override_to_mm: unit,
            ..Default::default()
        };
        let imp = dxf::import(&std::fs::read(fixtures.join(src))?, &opts)?;
        let text = dxf::export(&imp.sketch, &DxfExportOptions::default())?;
        std::fs::write(out.join(dst), text)?;
    }
    // Circle r 5 at the origin plus a construction line (-10,0)-(10,0) on CONSTRUCTION.
    let mut s = Sketch::default();
    s.ensure_origin();
    let c = s.add_point(0.0, 0.0);
    s.add_circle(c, 5.0);
    let a = s.add_point(-10.0, 0.0);
    let b = s.add_point(10.0, 0.0);
    let l = s.add_line(a, b);
    s.set_construction(l, true);
    let opts = DxfExportOptions {
        include_construction: true,
        ..Default::default()
    };
    std::fs::write(out.join("export_construction.dxf"), dxf::export(&s, &opts)?)?;
    println!("exports written to {}", out.display());
    Ok(())
}
