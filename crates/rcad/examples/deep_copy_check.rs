//! Does measuring a `Body::deep_copy` agree with measuring the original?
//! `cargo run --release -p rcad --example deep_copy_check`
use rcad_kernel::{self as k, Frame, Profile};

fn main() {
    let rect = Profile::Polygon(vec![
        [-40.0, -25.0],
        [40.0, -25.0],
        [40.0, 25.0],
        [-40.0, 25.0],
    ]);
    let plate = k::extrude(&rect, &Frame::XY, 10.0).expect("extrude");
    let hole = Profile::Circle {
        center: [0.0, 0.0],
        radius: 5.0,
    };
    let cut = k::cut_through_all(&plate, &hole, &Frame::XY.offset(10.0)).expect("cut");
    for (name, body, n) in [("box", &plate, 200), ("plate+hole", &cut, 3)] {
        let want = k::measure(body).expect("measure").volume;
        let mut bad = 0;
        for _ in 0..n {
            let copy = body.deep_copy().expect("deep copy");
            let got = k::measure(&copy).map(|m| m.volume);
            if got.as_ref().ok() != Some(&want) {
                bad += 1;
                if bad < 3 {
                    println!("{name}: mismatch {got:?} vs {want:?}");
                }
            }
        }
        println!("{name}: {bad}/{n} mismatches (original {want:?})");
    }
}
