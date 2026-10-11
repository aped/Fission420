//! Breakdown of one plate-with-hole regeneration (release build):
//! `cargo run --release -p f420-app --example regen_profile`
use std::time::Instant;

use f420_kernel::{self as k, Frame, Profile};

fn main() {
    let t = |label: &str, t0: Instant| {
        println!("{label:<34} {:>8.1} ms", t0.elapsed().as_secs_f64() * 1e3)
    };
    for width in [80.0, 100.0] {
        println!("-- plate {width} x 50 x 10, hole Ø10 --");
        let rect = Profile::Polygon(vec![
            [-width / 2.0, -25.0],
            [width / 2.0, -25.0],
            [width / 2.0, 25.0],
            [-width / 2.0, 25.0],
        ]);
        let t0 = Instant::now();
        let plate = k::extrude(&rect, &Frame::XY, 10.0).expect("extrude");
        t("extrude", t0);
        let hole = Profile::Circle {
            center: [0.0, 0.0],
            radius: 5.0,
        };
        let t0 = Instant::now();
        let body = k::cut_through_all(&plate, &hole, &Frame::XY.offset(10.0)).expect("cut");
        t("cut_through_all (boolean)", t0);
        let t0 = Instant::now();
        let m = k::measure(&body).expect("measure");
        t("measure (B-rep volume etc.)", t0);
        let t0 = Instant::now();
        let mesh = k::tessellate_display(&body, 0.05).expect("tess");
        t("tessellate_display 0.05 mm", t0);
        println!(
            "   volume {:?}, {} triangles",
            m.volume,
            mesh.triangles.len()
        );
        let t0 = Instant::now();
        let _ = k::measure(&plate).expect("measure");
        t("measure (plate, no hole)", t0);
    }
}
