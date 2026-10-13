//! Exact sweeps. Expected volumes by Pappus (independently written here),
//! bounds and probes from the analytic tube; STEP (FreeCAD:
//! tools/check_sweep.sh).
use super::*;
use crate::{VolumeResult, export_step, point_membership};
use std::f64::consts::PI;

fn evidence(b: &Body, name: &str) {
    let text = export_step(b, "test").unwrap();
    if let Some(dir) = std::env::var_os("RCAD_EVIDENCE_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join(format!("{name}.step")),
            text,
        )
        .unwrap();
    }
}

fn rect(w: f64, h: f64) -> Profile {
    Profile::Polygon(vec![
        [-w / 2.0, -h / 2.0],
        [w / 2.0, -h / 2.0],
        [w / 2.0, h / 2.0],
        [-w / 2.0, h / 2.0],
    ])
}

/// Line 10 along +Z, 90° arc R 15 about the axis through (15, 0, 10) along
/// +Y, line 10 along +X: ends at (25, 0, 25).
fn handle_path() -> Vec<PathSegment> {
    vec![
        PathSegment::Line {
            end: [0.0, 0.0, 10.0],
        },
        PathSegment::Arc {
            center: [15.0, 0.0, 10.0],
            end: [15.0, 0.0, 25.0],
        },
        PathSegment::Line {
            end: [25.0, 0.0, 25.0],
        },
    ]
}

fn volume_and_estimate(b: &Body) -> (f64, f64) {
    match measure(b).unwrap().volume {
        VolumeResult::Value {
            volume,
            error_estimate,
            ..
        } => (volume, error_estimate),
        VolumeResult::Unavailable { reason } => panic!("{reason}"),
    }
}

fn probe(b: &Body, pts: &[([f64; 3], bool)]) {
    let got = point_membership(b, &pts.iter().map(|p| p.0).collect::<Vec<_>>()).unwrap();
    for (p, g) in pts.iter().zip(got) {
        assert_eq!(g, Some(p.1), "probe {:?}", p.0);
    }
}

#[test]
fn handle_rectangle_along_line_arc_line() {
    // 8 (along X) x 4 (along Y) rectangle in the XY plane.
    let b = sweep(&rect(8.0, 4.0), &Frame::XY, &handle_path()).unwrap();
    let want = 32.0 * (10.0 + 15.0 * PI / 2.0 + 10.0);
    let (v, est) = volume_and_estimate(&b);
    assert!(
        (v - want).abs() < 1e-6 + 3.0 * est,
        "{v} vs {want} (± {est})"
    );
    assert!(
        est < 1e-6,
        "exact path expected (curved faces all about Y): {est}"
    );
    let m = measure(&b).unwrap();
    assert_eq!((m.shell_count, m.all_shells_closed), (1, true));
    let (lo, hi) = ([-4.0, -2.0, 0.0], [25.0, 2.0, 29.0]);
    for i in 0..3 {
        assert!(
            (m.bounds.min[i] - lo[i]).abs() < 1e-6 && (m.bounds.max[i] - hi[i]).abs() < 1e-6,
            "{:?}",
            m.bounds
        );
    }
    // Probes: on the centreline (inside) and just past each side.
    let s = 0.5f64.sqrt();
    let mid = [15.0 - 15.0 * s, 0.0, 10.0 + 15.0 * s]; // arc midpoint
    let out = [-s, 0.0, s]; // from the arc centre outwards there
    probe(
        &b,
        &[
            ([0.0, 0.0, 5.0], true),
            ([3.8, 1.8, 5.0], true),
            ([4.2, 0.0, 5.0], false),
            ([0.0, 2.2, 5.0], false),
            (mid, true),
            ([mid[0] + 3.8 * out[0], 0.0, mid[2] + 3.8 * out[2]], true),
            ([mid[0] + 4.2 * out[0], 0.0, mid[2] + 4.2 * out[2]], false),
            ([mid[0] - 4.2 * out[0], 0.0, mid[2] - 4.2 * out[2]], false),
            ([mid[0], 2.2, mid[2]], false),
            ([20.0, 0.0, 25.0], true),
            ([20.0, 0.0, 29.2], false),
            ([20.0, 0.0, 20.8], false),
        ],
    );
    evidence(&b, "sweep_handle");
}

#[test]
fn circular_channel_and_single_segments() {
    let disc = Profile::Circle {
        center: [0.0, 0.0],
        radius: 3.0,
    };
    let c = sweep(&disc, &Frame::XY, &handle_path()).unwrap();
    let want = 9.0 * PI * (20.0 + 7.5 * PI);
    // Cylinders along Z and X and a torus: no single axis-ruled integral, but
    // `measure`'s surface-of-revolution integral is exact (FreeCAD also checks
    // the exact volume: tools/check_sweep.sh).
    let (v, est) = volume_and_estimate(&c);
    assert!(
        est < 1e-9 * want && (v - want).abs() <= 1e-9 * want,
        "{v} vs {want} (± {est})"
    );
    probe(
        &c,
        &[
            ([0.0, 2.8, 5.0], true),
            ([0.0, 3.2, 5.0], false),
            ([20.0, 0.0, 27.8], true),
            ([20.0, 0.0, 28.2], false),
        ],
    );
    evidence(&c, "sweep_channel_d6");
    // A single line equals the extrusion.
    let l = sweep(
        &rect(8.0, 4.0),
        &Frame::XY,
        &[PathSegment::Line {
            end: [0.0, 0.0, 7.0],
        }],
    )
    .unwrap();
    assert!((volume_and_estimate(&l).0 - 224.0).abs() < 1e-9);
    // A single 180° arc (R 10), profile offset from the path point.
    let off = Profile::Polygon(vec![[1.0, -2.0], [5.0, -2.0], [5.0, 2.0], [1.0, 2.0]]);
    let a = sweep(
        &off,
        &Frame::XY,
        &[PathSegment::Arc {
            center: [-10.0, 0.0, 0.0],
            end: [-20.0, 0.0, 0.0],
        }],
    )
    .unwrap();
    // Centroid (3, 0): 13 mm from the axis through (-10, 0, 0) along Y.
    let (v, est) = volume_and_estimate(&a);
    assert!((v - 16.0 * PI * 13.0).abs() < 1e-6 + 3.0 * est, "{v}");
}

#[test]
fn sweep_refusals() {
    let msg = |r: Result<Body>| match r {
        Ok(_) => panic!("accepted"),
        Err(e) => e.to_string(),
    };
    let r = rect(8.0, 4.0);
    // First segment not along the frame normal.
    let e = msg(sweep(
        &r,
        &Frame::XY,
        &[PathSegment::Line {
            end: [1.0, 0.0, 10.0],
        }],
    ));
    assert!(e.contains("start along"), "{e}");
    // Non-tangent joint (a corner).
    let e = msg(sweep(
        &r,
        &Frame::XY,
        &[
            PathSegment::Line {
                end: [0.0, 0.0, 10.0],
            },
            PathSegment::Line {
                end: [10.0, 0.0, 10.0],
            },
        ],
    ));
    assert!(e.contains("tangentially"), "{e}");
    // Arc radius 3 does not clear the 8 mm wide profile (4 mm inwards).
    let e = msg(sweep(
        &r,
        &Frame::XY,
        &[PathSegment::Arc {
            center: [3.0, 0.0, 0.0],
            end: [6.0, 0.0, 0.0],
        }],
    ));
    assert!(e.contains("does not clear"), "{e}");
    // A full-turn arc (end = start).
    let e = msg(sweep(
        &r,
        &Frame::XY,
        &[PathSegment::Arc {
            center: [20.0, 0.0, 0.0],
            end: [0.0, 0.0, 0.0],
        }],
    ));
    assert!(e.contains("full turn"), "{e}");
    // Non-planar: a second arc about a different axis.
    let e = msg(sweep(
        &r,
        &Frame::XY,
        &[
            PathSegment::Arc {
                center: [20.0, 0.0, 0.0],
                end: [20.0, 0.0, 20.0],
            },
            PathSegment::Arc {
                center: [20.0, 20.0, 20.0],
                end: [20.0, 20.0, 40.0],
            },
        ],
    ));
    assert!(
        e.contains("non-planar") || e.contains("tangentially"),
        "{e}"
    );
    // Two 180° arcs on one circle: a full turn back to the start.
    let e = msg(sweep(
        &rect(2.0, 2.0),
        &Frame::XY,
        &[
            PathSegment::Arc {
                center: [10.0, 0.0, 0.0],
                end: [20.0, 0.0, 0.0],
            },
            PathSegment::Arc {
                center: [10.0, 0.0, 0.0],
                end: [0.0, 0.0, 0.0],
            },
        ],
    ));
    assert!(e.contains("full turn"), "{e}");
    // A 270° arc (R 1.6) then a line passing back 1.6 mm from the start,
    // within twice the profile's reach (conservative self-intersection guard).
    let e = msg(sweep(
        &rect(2.0, 2.0),
        &Frame::XY,
        &[
            PathSegment::Arc {
                center: [1.6, 0.0, 0.0],
                end: [1.6, 0.0, -1.6],
            },
            PathSegment::Line {
                end: [-20.0, 0.0, -1.6],
            },
        ],
    ));
    assert!(e.contains("comes back"), "{e}");
}
