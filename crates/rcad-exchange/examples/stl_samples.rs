//! Write STL files with the crate's writers for external validation
//! (tools/fixtures/stl/validate_writer.sh reads them with FreeCAD's Mesh module).
//!
//! usage: cargo run --release -p rcad-exchange --example stl_samples -- OUT_DIR

use rcad_exchange::stl::{self, StlEncoding, StlLimits, StlMesh};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::args()
        .nth(1)
        .ok_or("usage: stl_samples OUT_DIR")?;
    let out = Path::new(&out);
    std::fs::create_dir_all(out)?;
    // Unit cube scaled to 10 mm, 12 outward triangles, hand-indexed (not read from a file).
    let p = |x: f64, y: f64, z: f64| [10.0 * x, 10.0 * y, 10.0 * z];
    let cube = StlMesh {
        name: Some("cube10".into()),
        encoding: StlEncoding::Binary,
        positions: vec![
            p(0., 0., 0.),
            p(1., 0., 0.),
            p(1., 1., 0.),
            p(0., 1., 0.),
            p(0., 0., 1.),
            p(1., 0., 1.),
            p(1., 1., 1.),
            p(0., 1., 1.),
        ],
        triangles: vec![
            [0, 2, 1],
            [0, 3, 2], // bottom (-z)
            [4, 5, 6],
            [4, 6, 7], // top (+z)
            [0, 1, 5],
            [0, 5, 4], // front (-y)
            [2, 3, 7],
            [2, 7, 6], // back (+y)
            [1, 2, 6],
            [1, 6, 5], // right (+x)
            [0, 4, 7],
            [0, 7, 3], // left (-x)
        ],
        facet_normals: vec![],
    };
    std::fs::write(out.join("cube10_binary.stl"), stl::write_binary(&cube)?)?;
    std::fs::write(out.join("cube10_ascii.stl"), stl::write_ascii(&cube)?)?;
    // Sanity: our own reader agrees before FreeCAD looks at it.
    let back = stl::read(
        &std::fs::read(out.join("cube10_binary.stl"))?,
        &StlLimits::default(),
    )?;
    assert_eq!(back.triangles.len(), 12);
    println!("STL samples written to {}", out.display());
    Ok(())
}
