//! Tapered tools. Expected volumes from closed forms: a square of side s at
//! the base offset inward by w·t has side s − 2wt, so V = ∫ (s − 2wt)² dw;
//! circles give the cone frustum πh/3 (R² + Rr + r²).
use super::*;
use crate::{VolumeResult, export_step, measure, point_membership};

fn square(s: f64, c: [f64; 2]) -> Profile {
    let h = s / 2.0;
    Profile::Polygon(vec![
        [c[0] - h, c[1] - h],
        [c[0] + h, c[1] - h],
        [c[0] + h, c[1] + h],
        [c[0] - h, c[1] + h],
    ])
}

/// ∫_0^d (s − 2wt)² dw.
fn square_frustum(s: f64, d: f64, t: f64) -> f64 {
    let f = |w: f64| -(s - 2.0 * w * t).powi(3) / (6.0 * t);
    if t == 0.0 { s * s * d } else { f(d) - f(0.0) }
}

fn volume(b: &Body) -> (f64, f64) {
    match measure(b).unwrap().volume {
        VolumeResult::Value {
            volume,
            error_estimate,
            ..
        } => (volume, error_estimate),
        VolumeResult::Unavailable { reason } => panic!("{reason}"),
    }
}

fn assert_volume(b: &Body, want: f64) {
    let (v, e) = volume(b);
    assert!(
        (v - want).abs() <= e.max(1e-6 * want),
        "{v} ± {e} vs {want}"
    );
}

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

/// Block [0,40]x[0,30]x[0,10].
fn block() -> Body {
    crate::extrude(
        &Profile::Polygon(vec![[0.0, 0.0], [40.0, 0.0], [40.0, 30.0], [0.0, 30.0]]),
        &Frame::XY,
        10.0,
    )
    .unwrap()
}

/// Frame on the top face (z = 10) pointing down into the block, right-handed.
fn top_down() -> Frame {
    Frame {
        origin: [0.0, 0.0, 10.0],
        x: [1.0, 0.0, 0.0],
        y: [0.0, -1.0, 0.0],
        z: [0.0, 0.0, -1.0],
    }
}

#[test]
fn tapered_tools_have_exact_volumes() {
    let t = 5f64.to_radians().tan();
    let b = tapered_tool(
        &square(20.0, [0.0, 0.0]),
        &Frame::XY,
        (0.0, 10.0),
        5f64.to_radians(),
    )
    .unwrap();
    assert_volume(&b, square_frustum(20.0, 10.0, t));
    // Negative draft grows; a span below the base plane continues the law.
    let b = tapered_tool(
        &square(20.0, [0.0, 0.0]),
        &Frame::XY,
        (-2.0, 3.0),
        -3f64.to_radians(),
    )
    .unwrap();
    let t3 = -(3f64.to_radians().tan());
    let want = square_frustum(20.0, 3.0, t3) + (square_frustum(20.0 - 2.0 * (-2.0) * t3, 2.0, t3));
    assert_volume(&b, want);
    // Circle: cone frustum.
    let b = tapered_tool(
        &Profile::Circle {
            center: [0.0, 0.0],
            radius: 5.0,
        },
        &Frame::XY,
        (0.0, 10.0),
        10f64.to_radians(),
    )
    .unwrap();
    let r2 = 5.0 - 10.0 * 10f64.to_radians().tan();
    assert_volume(
        &b,
        std::f64::consts::PI * 10.0 / 3.0 * (25.0 + 5.0 * r2 + r2 * r2),
    );
}

#[test]
fn tapered_cut_and_join_on_a_block() {
    let blk = block();
    let ang = 10f64.to_radians();
    let t = ang.tan();
    // Pocket 10x10 at (20,15) (frame y is flipped: centre (20,-15)), 5 deep,
    // walls drafted inward; the tool starts 1 mm above the face.
    let prof = square(10.0, [20.0, -15.0]);
    let (cut, check) = tapered_boolean(
        &blk,
        &prof,
        &top_down(),
        5.0,
        (-1.0, 5.0),
        ang,
        PrismOp::Cut,
    )
    .unwrap();
    assert!(check.faces_probed > 0, "{check:?}");
    assert_volume(&cut, 12000.0 - square_frustum(10.0, 5.0, t));
    // Probes: below the floor; beside the drafted wall at depth 4.5 (half
    // side 5 − 4.5·t ≈ 4.21); inside the pocket just under the face.
    let m = point_membership(
        &cut,
        &[[20.0, 15.0, 4.8], [24.5, 15.0, 5.5], [24.0, 15.0, 9.8]],
    )
    .unwrap();
    assert_eq!(m, vec![Some(true), Some(true), Some(false)]);
    evidence(&cut, "taper_cut_pocket");
    // Boss 10x10 at (20,15) on the top face, 5 tall, drafted 5°; the tool
    // overlaps 0.5 mm into the block (the law continues below the face).
    let up = Frame {
        origin: [0.0, 0.0, 10.0],
        ..Frame::XY
    };
    let a5 = 5f64.to_radians();
    let (boss, _) = tapered_boolean(
        &blk,
        &square(10.0, [20.0, 15.0]),
        &up,
        5.0,
        (-0.5, 5.0),
        a5,
        PrismOp::Join,
    )
    .unwrap();
    assert_volume(&boss, 12000.0 + square_frustum(10.0, 5.0, a5.tan()));
    evidence(&boss, "taper_join_boss");
}

#[test]
fn the_check_is_not_vacuous() {
    let blk = block();
    let ang = 10f64.to_radians();
    let prof = square(10.0, [20.0, -15.0]);
    let (cut, _) = tapered_boolean(
        &blk,
        &prof,
        &top_down(),
        5.0,
        (-1.0, 5.0),
        ang,
        PrismOp::Cut,
    )
    .unwrap();
    let prism = Prism {
        profile: &prof,
        holes: &[],
        start: &top_down(),
        length: 5.0,
    };
    // Checked as a straight pocket, the drafted walls are wrong.
    assert!(
        crate::verify_tapered_boolean(&blk, &cut, PrismOp::Cut, &prism, 0.0, (-1.0, 5.0)).is_err()
    );
    // And with the opposite draft.
    assert!(
        crate::verify_tapered_boolean(&blk, &cut, PrismOp::Cut, &prism, -ang, (-1.0, 5.0)).is_err()
    );
}

#[test]
fn round_tapered_cut_is_exact_or_refused() {
    // Truck 0.6 cannot boolean revolved cones (run-4 spike): either an exact,
    // checked result or a named failure, never a wrong body.
    let blk = block();
    let prof = Profile::Circle {
        center: [20.0, -15.0],
        radius: 5.0,
    };
    let ang = 10f64.to_radians();
    match tapered_boolean(
        &blk,
        &prof,
        &top_down(),
        5.0,
        (-1.0, 5.0),
        ang,
        PrismOp::Cut,
    ) {
        Ok((b, _)) => {
            let r2 = 5.0 - 5.0 * ang.tan();
            assert_volume(
                &b,
                12000.0 - std::f64::consts::PI * 5.0 / 3.0 * (25.0 + 5.0 * r2 + r2 * r2),
            );
            // Exported exactly when Truck's intersection edges have exact
            // replacements; otherwise the export refuses (never approximates).
            match export_step(&b, "test") {
                Ok(_) => evidence(&b, "taper_cut_round"),
                Err(e) => eprintln!("round tapered cut: STEP export refused: {e}"),
            }
        }
        Err(e) => {
            assert!(
                matches!(
                    e,
                    KernelError::BooleanFailed(_) | KernelError::InvalidResult(_)
                ),
                "{e:?}"
            );
            eprintln!("round tapered cut refused: {e}");
        }
    }
}

#[test]
fn taper_refusals() {
    let ang = 30f64.to_radians();
    // 10x10 square shrinking 2·10·tan30 = 11.5 > 10: collapses.
    assert!(tapered_tool(&square(10.0, [0.0, 0.0]), &Frame::XY, (0.0, 10.0), ang).is_err());
    let rounded = Profile::Loop(vec![
        crate::LoopSegment::Line { start: [0.0, 0.0] },
        crate::LoopSegment::Arc {
            start: [10.0, 0.0],
            center: [10.0, 5.0],
            ccw: true,
        },
        crate::LoopSegment::Line {
            start: [10.0, 10.0],
        },
    ]);
    let e = tapered_tool(&rounded, &Frame::XY, (0.0, 1.0), 0.1)
        .unwrap_err()
        .to_string();
    assert!(e.contains("rounded"), "{e}");
    assert!(tapered_tool(&square(10.0, [0.0, 0.0]), &Frame::XY, (0.0, 1.0), 1.56).is_err());
    assert!(tapered_tool(&square(10.0, [0.0, 0.0]), &Frame::XY, (1.0, 1.0), 0.1).is_err());
    // Circle shrinking to nothing.
    let c = Profile::Circle {
        center: [0.0, 0.0],
        radius: 1.0,
    };
    assert!(tapered_tool(&c, &Frame::XY, (0.0, 10.0), ang).is_err());
}
