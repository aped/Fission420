//! Write 3MF packages with `threemf::write` for external validation
//! (tools/fixtures/threemf/validate_writer.sh reads them with FreeCAD and with
//! an independent Python reader).
//!
//! usage: cargo run --release -p rcad-exchange --example threemf_samples -- OUT_DIR
//!
//! Content (mm, hand-indexed, not read from a file):
//! * `cube10` 10 mm cube at the origin, colour #FF8000 (V 1000);
//! * `tet` right tetrahedron with legs 12 moved to x = 30 (V 288), no colour.
//!
//! Written as `rcad_two_mm.3mf` (unit millimeter) and `rcad_two_inch.3mf` (unit inch:
//! coordinates divided by 25.4).
use rcad_exchange::threemf::{self, MeshObject, ThreeMfUnit};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::args()
        .nth(1)
        .ok_or("usage: threemf_samples OUT_DIR")?;
    let out = Path::new(&out);
    std::fs::create_dir_all(out)?;
    let p = |x: f64, y: f64, z: f64| [10.0 * x, 10.0 * y, 10.0 * z];
    let cube = MeshObject {
        name: Some("cube10".into()),
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
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [2, 3, 7],
            [2, 7, 6],
            [1, 2, 6],
            [1, 6, 5],
            [3, 0, 4],
            [3, 4, 7],
        ],
        color: Some([255, 128, 0, 255]),
    };
    let tet = MeshObject {
        name: Some("tet".into()),
        positions: vec![
            [30.0, 0.0, 0.0],
            [42.0, 0.0, 0.0],
            [30.0, 12.0, 0.0],
            [30.0, 0.0, 12.0],
        ],
        triangles: vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
        color: None,
    };
    let objects = [cube, tet];
    for (unit, file) in [
        (ThreeMfUnit::Millimeter, "rcad_two_mm.3mf"),
        (ThreeMfUnit::Inch, "rcad_two_inch.3mf"),
    ] {
        std::fs::write(out.join(file), threemf::write(&objects, unit)?)?;
        println!("wrote {}", out.join(file).display());
    }
    Ok(())
}
