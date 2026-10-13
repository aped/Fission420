//! Faceted threads, Ø20 x 2 x 12 test profile. Independent oracle: the
//! volume of the *ideal* thread solid integrated directly from its definition
//! (radius as a function of angle and height), V = ∫∫ R(θ, z)² / 2 dθ dz,
//! compared with the exact volume of the faceted B-rep within the facet
//! deviation times the surface area.
use super::*;
use crate::{VolumeResult, export_step, measure, point_membership};

fn spec(left: bool) -> ThreadSpec {
    ThreadSpec {
        major_diameter: 20.0,
        pitch: 2.0,
        length: 12.0,
        left_hand: left,
        chord_tol: 0.01,
    }
}

fn volume(b: &Body) -> f64 {
    match measure(b).unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        VolumeResult::Unavailable { reason } => panic!("{reason}"),
    }
}

/// Ideal radius of the thread solid at angle `th` (rad, sense of the thread)
/// and height `z` (definition in the module docs, independent of the grid).
fn ideal_r(th: f64, z: f64, p: f64, turns: f64, r_core: f64, depth: f64) -> f64 {
    let a = th / TAU; // turns of rotation in [0, 1)
    let u = z / p - a; // helical coordinate: sheet k + profile fraction s
    let k = u.floor();
    let s = u - k;
    let t = k + a; // continuous turn count of the sheet (row / n)
    let lam = t.min(turns - 1.0 - t).clamp(0.0, 1.0);
    let ridge = if s < 0.25 {
        0.0
    } else if s < 0.5625 {
        (s - 0.25) / 0.3125
    } else if s < 0.6875 {
        1.0
    } else {
        (1.0 - s) / 0.3125
    };
    r_core + lam * depth * ridge
}

fn ideal_volume(p: f64, l: f64, d: f64) -> f64 {
    let depth = 5.0 * p * 3f64.sqrt() / 16.0;
    let r_core = d / 2.0 - depth;
    let (nt, nz) = (720, 6000);
    let mut v = 0.0;
    for i in 0..nt {
        let th = TAU * (i as f64 + 0.5) / nt as f64;
        for j in 0..nz {
            let z = l * (j as f64 + 0.5) / nz as f64;
            let r = ideal_r(th, z, p, l / p, r_core, depth);
            v += r * r / 2.0;
        }
    }
    v * (TAU / nt as f64) * (l / nz as f64)
}

#[test]
fn threaded_rod_matches_the_ideal_thread() {
    let (b, rep) = threaded_rod(&Frame::XY, &spec(false)).unwrap();
    assert!(rep.faceted);
    assert!(rep.max_facet_deviation <= 0.01, "{rep:?}");
    assert!((rep.depth - 5.0 * 2.0 * 3f64.sqrt() / 16.0).abs() < 1e-12);
    let m = measure(&b).unwrap();
    assert_eq!((m.shell_count, m.all_shells_closed), (1, true));
    assert!(m.bounds.max[0] <= 10.0 + 1e-9 && m.bounds.min[0] >= -10.0 - 1e-9);
    assert!((m.bounds.min[2]).abs() < 1e-9 && (m.bounds.max[2] - 12.0).abs() < 1e-9);
    let v = volume(&b);
    let ideal = ideal_volume(2.0, 12.0, 20.0);
    // Lateral area < 2π r_major L + flank surplus (< 2x): bound the facet error.
    let bound = rep.max_facet_deviation * 2.0 * TAU * 10.0 * 12.0 + 1e-3 * ideal * 1e-3;
    eprintln!("thread rod: {rep:?}; faceted volume {v}, ideal {ideal}, bound {bound}");
    assert!(
        (v - ideal).abs() <= bound,
        "faceted {v} vs ideal {ideal} (bound {bound})"
    );
    // Volume between the core cylinder and the major cylinder.
    let rc = rep.minor_diameter / 2.0;
    assert!(v > PI * rc * rc * 12.0 && v < PI * 100.0 * 12.0);
    // Probes at angle 0 in the full-profile turns: crest at z = p (k + 0.625),
    // root at z = p (k + 0.125); radius 9.9 is inside the crest, outside the root.
    let m = point_membership(
        &b,
        &[
            [9.9, 0.0, 5.25],
            [9.9, 0.0, 4.25],
            [8.5, 0.0, 4.25],
            [0.0, 9.9, 5.75],
        ],
    )
    .unwrap();
    // At 90° the right-hand crest is a quarter pitch higher (z = 5.75).
    assert_eq!(m, vec![Some(true), Some(false), Some(true), Some(true)]);
    if let Some(dir) = std::env::var_os("RCAD_EVIDENCE_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        let p = std::path::Path::new(&dir);
        std::fs::write(
            p.join("thread_rod_m20x2.step"),
            export_step(&b, "R-CAD").unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn left_hand_rod_is_the_mirror_image() {
    let (r, _) = threaded_rod(&Frame::XY, &spec(false)).unwrap();
    let (l, _) = threaded_rod(&Frame::XY, &spec(true)).unwrap();
    assert!((volume(&r) - volume(&l)).abs() < 1e-6 * volume(&r));
    // At 90° the left-hand crest is a quarter pitch lower (z = 4.75).
    let m = point_membership(&l, &[[0.0, 9.9, 4.75], [0.0, 9.9, 5.75]]).unwrap();
    assert_eq!(m, vec![Some(true), Some(false)]);
}

#[test]
fn nut_mates_with_the_rod() {
    let s = spec(false);
    let (rod, _) = threaded_rod(&Frame::XY, &s).unwrap();
    let (nut, rep) = threaded_nut(&Frame::XY, &s, 30.0, 0.1).unwrap();
    let m = measure(&nut).unwrap();
    assert_eq!((m.shell_count, m.all_shells_closed), (1, true));
    // Nut volume = box − (rod shape offset 0.1 radially), the latter from the
    // ideal integral of (R + 0.1)²/2.
    let depth = rep.depth;
    let r_core = 10.0 - depth;
    let (nt, nz) = (720, 6000);
    let mut void = 0.0;
    for i in 0..nt {
        let th = TAU * (i as f64 + 0.5) / nt as f64;
        for j in 0..nz {
            let z = 12.0 * (j as f64 + 0.5) / nz as f64;
            let r = ideal_r(th, z, 2.0, 6.0, r_core, depth) + 0.1;
            void += r * r / 2.0;
        }
    }
    void *= (TAU / nt as f64) * (12.0 / nz as f64);
    let bound = rep.max_facet_deviation * 2.0 * TAU * 10.1 * 12.0;
    let v = volume(&nut);
    assert!(
        (v - (30.0 * 30.0 * 12.0 - void)).abs() <= bound,
        "{v} vs {}",
        10800.0 - void
    );
    // No point is inside both (sampled through the thread zone).
    let mut pts = Vec::new();
    for i in 0..48 {
        let th = TAU * i as f64 / 48.0;
        for k in 0..60 {
            let z = 0.1 + 11.8 * k as f64 / 59.0;
            for r in [8.95, 9.3, 9.6, 9.9] {
                pts.push([r * th.cos(), r * th.sin(), z]);
            }
        }
    }
    let (a, b) = (
        point_membership(&rod, &pts).unwrap(),
        point_membership(&nut, &pts).unwrap(),
    );
    let both = a
        .iter()
        .zip(&b)
        .filter(|(x, y)| **x == Some(true) && **y == Some(true))
        .count();
    let in_rod = a.iter().filter(|x| **x == Some(true)).count();
    let in_nut = b.iter().filter(|x| **x == Some(true)).count();
    assert_eq!(both, 0, "rod and nut overlap");
    assert!(in_rod > 1000 && in_nut > 1000, "{in_rod} {in_nut}");
    if let Some(dir) = std::env::var_os("RCAD_EVIDENCE_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        let p = std::path::Path::new(&dir);
        std::fs::write(
            p.join("thread_nut_m20x2.step"),
            export_step(&nut, "R-CAD").unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn thread_refusals() {
    let mut s = spec(false);
    s.length = 11.0;
    assert!(
        threaded_rod(&Frame::XY, &s)
            .unwrap_err()
            .to_string()
            .contains("whole number")
    );
    s.length = 4.0;
    assert!(threaded_rod(&Frame::XY, &s).is_err(), "fewer than 3 turns");
    let mut s = spec(false);
    s.chord_tol = 1e-7;
    assert!(threaded_rod(&Frame::XY, &s).is_err(), "too fine");
    let mut s = spec(false);
    s.major_diameter = 3.0;
    assert!(threaded_rod(&Frame::XY, &s).is_err());
    assert!(
        threaded_nut(&Frame::XY, &spec(false), 20.0, 0.1).is_err(),
        "nut too small"
    );
    assert!(threaded_nut(&Frame::XY, &spec(false), 30.0, -0.1).is_err());
}
