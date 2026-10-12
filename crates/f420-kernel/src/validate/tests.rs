//! Validator tests. Expected values are analytic.
use super::*;
use crate::{VolumeResult, cut_prism, extrude, join_prism, measure};

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Profile {
    Profile::Polygon(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
}

fn volume(b: &Body) -> f64 {
    match measure(b).unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        VolumeResult::Unavailable { reason } => panic!("volume unavailable: {reason}"),
    }
}

/// Frame looking down -Z from z = `z` (the plate's top, cutting downward).
fn down(z: f64) -> Frame {
    Frame {
        origin: [0.0, 50.0, z],
        x: [1.0, 0.0, 0.0],
        y: [0.0, -1.0, 0.0],
        z: [0.0, 0.0, -1.0],
    }
}

fn plate() -> Body {
    extrude(&rect(0.0, 0.0, 80.0, 50.0), &Frame::XY, 10.0).unwrap()
}

#[test]
fn ordinary_cut_and_join_pass() {
    let p = plate();
    let hole = Profile::Circle {
        center: [20.0, 25.0],
        radius: 5.0,
    };
    let start = down(12.0);
    let out = cut_prism(&p, &hole, &start, 14.0).unwrap();
    let nominal = Prism {
        profile: &hole,
        start: &start,
        length: 14.0,
    };
    let s = verify_prism_boolean(&p, &out, PrismOp::Cut, &nominal, (0.0, 14.0)).unwrap();
    assert!(s.faces_probed >= 3, "{s:?}");
    let boss = rect(5.0, 5.0, 25.0, 15.0);
    let top = Frame::XY.offset(10.0);
    let j = join_prism(&p, &boss, &top.offset(-0.5), 5.5).unwrap();
    let nominal = Prism {
        profile: &boss,
        start: &top,
        length: 5.0,
    };
    let s = verify_prism_boolean(&p, &j, PrismOp::Join, &nominal, (-0.5, 5.0)).unwrap();
    assert!(s.faces_probed >= 5, "{s:?}");
}

/// Run-3 review sketch/kernel #3: Truck keeps the engulfed hole's walls.
#[test]
fn swallowed_hole_cut_is_detected() {
    let p = plate();
    let small = Profile::Circle {
        center: [20.0, 25.0],
        radius: 5.0,
    };
    let big = Profile::Circle {
        center: [20.0, 25.0],
        radius: 8.0,
    };
    let start = down(12.0);
    let holed = cut_prism(&p, &small, &start, 14.0).unwrap();
    let out = cut_prism(&holed, &big, &start, 14.0).unwrap();
    let removed = volume(&holed) - volume(&out);
    eprintln!(
        "swallowed: removed {removed}, faces {} -> {}",
        measure(&holed).unwrap().face_count,
        measure(&out).unwrap().face_count
    );
    let nominal = Prism {
        profile: &big,
        start: &start,
        length: 14.0,
    };
    let e = verify_prism_boolean(&holed, &out, PrismOp::Cut, &nominal, (0.0, 14.0));
    eprintln!("{e:?}");
    let want = std::f64::consts::PI * (64.0 - 25.0) * 10.0;
    if (removed - want).abs() > 1e-3 {
        assert!(matches!(e, Err(KernelError::InvalidResult(_))), "{e:?}");
    }
    // Workaround candidate: (T ∪ tool) \ tool = T \ tool.
    if let Ok(u) = join_prism(&holed, &big, &start, 14.0) {
        let u_ok = verify_prism_boolean(&holed, &u, PrismOp::Join, &nominal, (0.0, 14.0));
        eprintln!("union: vol {} check {u_ok:?}", volume(&u));
        if let Ok(c) = cut_prism(&u, &big, &start, 14.0) {
            let c_ok = verify_prism_boolean(&holed, &c, PrismOp::Cut, &nominal, (0.0, 14.0));
            eprintln!(
                "union-then-cut: removed {} (want {want}) check {c_ok:?}",
                volume(&holed) - volume(&c)
            );
        }
    }
}

/// Run-3 review regen #5 (skin over a cavity): a 0.5 mm overlap under a
/// 0.3 mm skin leaks into the cavity; the check sees it and clear_depth sizes
/// the overlap so it cannot.
#[test]
fn join_overlap_into_a_cavity_is_detected_and_clear_depth_avoids_it() {
    let block = extrude(&rect(-10.0, -10.0, 10.0, 10.0), &Frame::XY, 10.0).unwrap();
    // Pocket from below, 9.7 deep: skin 0.3 under the top face.
    let up = Frame::XY.offset(-1.0);
    let skinned = cut_prism(&block, &rect(-5.0, -5.0, 5.0, 5.0), &up, 10.7).unwrap();
    assert!((volume(&skinned) - (4000.0 - 970.0)).abs() < 1e-6);
    let top = Frame::XY.offset(10.0);
    let boss = rect(-2.0, -2.0, 2.0, 2.0);
    let nominal = Prism {
        profile: &boss,
        start: &top,
        length: 5.0,
    };
    let leaky = join_prism(&skinned, &boss, &top.offset(-0.5), 5.5).unwrap();
    let e = verify_prism_boolean(&skinned, &leaky, PrismOp::Join, &nominal, (-0.5, 5.0));
    assert!(matches!(e, Err(KernelError::InvalidResult(_))), "{e:?}");
    let d = clear_depth(&skinned, &top, &boss, 0.5).unwrap();
    assert!((d - 0.3).abs() < 1e-9, "clear depth {d}");
    let good = join_prism(&skinned, &boss, &top.offset(-d / 2.0), 5.0 + d / 2.0).unwrap();
    verify_prism_boolean(&skinned, &good, PrismOp::Join, &nominal, (-d / 2.0, 5.0)).unwrap();
    assert!((volume(&good) - 3110.0).abs() < 1e-6, "{}", volume(&good));
}

#[test]
fn clear_depth_cases() {
    let block = extrude(&rect(0.0, 0.0, 40.0, 30.0), &Frame::XY, 10.0).unwrap();
    let top = Frame::XY.offset(10.0);
    // Whole face, boundary on the walls: the walls do not count.
    let all = rect(0.0, 0.0, 40.0, 30.0);
    assert_eq!(clear_depth(&block, &top, &all, 0.5).unwrap(), 0.5);
    assert!((clear_depth(&block, &top, &all, 50.0).unwrap() - 10.0).abs() < 1e-9);
    // A Ø6 through hole under part of the profile.
    let holed = cut_prism(
        &block,
        &Profile::Circle {
            center: [20.0, 15.0],
            radius: 3.0,
        },
        &Frame::XY.offset(-1.0),
        12.0,
    )
    .unwrap();
    // The hole's wall reaches the face itself: depth ~0 (after the chord margin).
    let over = rect(15.0, 10.0, 25.0, 20.0);
    assert!(clear_depth(&holed, &top, &over, 0.5).unwrap() < 0.05);
    // Beside the hole: the full depth.
    let beside = rect(30.0, 5.0, 38.0, 25.0);
    assert!((clear_depth(&holed, &top, &beside, 50.0).unwrap() - 10.0).abs() < 1e-9);
    // Cylinder stacked on a cylinder of the same radius: its own wall is ignored.
    let disc = Profile::Circle {
        center: [0.0, 0.0],
        radius: 10.0,
    };
    let cyl = extrude(&disc, &Frame::XY, 5.0).unwrap();
    let d = clear_depth(&cyl, &Frame::XY.offset(5.0), &disc, 0.5).unwrap();
    assert_eq!(d, 0.5);
}

/// Feasibility spike (run 4): a 90° countersink as a revolved annular cone cut
/// from a plate with a Ø5.5 hole, and from a plain plate. Observed with Truck
/// 0.6: `and` returns None at every tolerance (holed plate) and panics (plain
/// plate), so the Hole feature offers no countersink. Prints, asserts nothing.
#[test]
#[ignore = "feasibility spike: countersink via a revolved cone tool"]
fn spike_countersink_cone() {
    use std::f64::consts::PI;
    let p = plate();
    let start = down(12.0);
    let r_hole = 2.75;
    let holed = cut_prism(
        &p,
        &Profile::Circle {
            center: [20.0, 25.0],
            radius: r_hole,
        },
        &start,
        14.0,
    )
    .unwrap();
    // Revolve frame: origin at the hole centre on the top face (z = 10), y = +Z (axis).
    let f = Frame {
        origin: [20.0, 25.0, 10.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [0.0, -1.0, 0.0],
    };
    let (big_r, m, r_in) = (5.5, 1.0, 1.0);
    // 90° countersink: radius shrinks 1 per unit depth.
    let tri = Profile::Polygon(vec![
        [r_in, m],
        [r_in, -(big_r - r_in)],
        [big_r + m, m],
    ]);
    let tri = match crate::revolve(&tri, &f, 2.0 * PI) {
        Ok(_) => tri,
        Err(e) => {
            eprintln!("revolve (ccw?) failed: {e}");
            Profile::Polygon(vec![[r_in, m], [big_r + m, m], [r_in, -(big_r - r_in)]])
        }
    };
    let cone = crate::revolve(&tri, &f, 2.0 * PI).unwrap();
    eprintln!("cone volume {:?}", measure(&cone).map(|m| m.volume));
    let plain = crate::build::subtract_solid_for_test(&p, &cone);
    eprintln!("plain plate cone cut: {:?}", plain.as_ref().map(|b| volume(&p) - volume(b)).map_err(|e| e.to_string()));
    let out = crate::build::subtract_solid_for_test(&holed, &cone);
    match out {
        Ok(b) => {
            let removed = volume(&holed) - volume(&b);
            let h = big_r - r_hole;
            // Frustum between radii r_hole and big_r, height h, minus the hole cylinder part.
            let frustum = PI * h / 3.0 * (big_r * big_r + big_r * r_hole + r_hole * r_hole);
            let want = frustum - PI * r_hole * r_hole * h;
            eprintln!("removed {removed} want {want} closed {:?}", measure(&b).map(|m| (m.shell_count, m.all_shells_closed)));
        }
        Err(e) => eprintln!("cut failed: {e}"),
    }
}
