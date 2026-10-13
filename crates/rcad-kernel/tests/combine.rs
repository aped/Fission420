//! `boolean` + `verify_boolean_by_membership`: correct results pass, deliberately
//! wrong "results" are caught, nothing-decidable is an error; `combine` with
//! several tools against closed-form volumes.
use rcad_kernel::*;

fn boxed(x0: f64, y0: f64, z0: f64, a: f64, b: f64, c: f64) -> Body {
    let p = Profile::Polygon(vec![[x0, y0], [x0 + a, y0], [x0 + a, y0 + b], [x0, y0 + b]]);
    extrude(&p, &Frame::XY.offset(z0), c).unwrap()
}

fn volume(b: &Body) -> f64 {
    match measure(b).unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

#[test]
fn correct_results_pass_and_wrong_ones_are_caught() {
    let a = boxed(0.0, 0.0, 0.0, 40.0, 30.0, 20.0);
    let b = boxed(20.0, 10.0, 5.0, 40.0, 30.0, 20.0);
    for (op, want) in [
        (BoolOp::Join, 42_000.0),
        (BoolOp::Cut, 18_000.0),
        (BoolOp::Intersect, 6_000.0),
    ] {
        let r = boolean(&a, &b, op).unwrap();
        assert!((volume(&r) - want).abs() < 1e-6, "{op:?}");
        let rep = verify_boolean_by_membership(&a, &b, &r, op, 12).unwrap();
        eprintln!("{op:?}: {rep:?}");
        assert!(rep.passed(), "{op:?}: {rep:?}");
        assert_eq!(rep.samples, 12 * 12 * 12);
        assert!(rep.compared > rep.samples / 2);
        assert_eq!(rep.volume_ok, Some(true));
    }
    // The untouched target presented as the result of a cut / intersect, and the
    // intersection presented as a join: all disagree with the inputs.
    let inter = boolean(&a, &b, BoolOp::Intersect).unwrap();
    for (op, wrong) in [
        (BoolOp::Cut, &a),
        (BoolOp::Intersect, &a),
        (BoolOp::Join, &inter),
    ] {
        let rep = verify_boolean_by_membership(&a, &b, wrong, op, 12).unwrap();
        assert!(!rep.passed(), "{op:?}");
        assert!(rep.mismatches > 0);
        assert!(rep.failure().unwrap().contains("independent check failed"));
    }
}

#[test]
fn combine_with_two_tools() {
    let a = boxed(0.0, 0.0, 0.0, 60.0, 40.0, 20.0);
    // Two 10 x 10 posts through the plate at different places.
    let t1 = boxed(5.0, 5.0, -5.0, 10.0, 10.0, 30.0);
    let t2 = boxed(40.0, 20.0, -5.0, 10.0, 10.0, 30.0);
    let (r, rep) = combine(&a, &[t1.clone(), t2.clone()], CombineOp::Cut).unwrap();
    assert!(rep.passed());
    assert!((volume(&r) - (48_000.0 - 2.0 * 2_000.0)).abs() < 1e-6);
    let (r, _) = combine(&a, &[t1, t2], CombineOp::Join).unwrap();
    assert!((volume(&r) - (48_000.0 + 2.0 * 1_000.0)).abs() < 1e-6);
}

#[test]
fn disjoint_bodies_are_refused_before_any_boolean() {
    let a = boxed(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
    let b = boxed(50.0, 0.0, 0.0, 10.0, 10.0, 10.0);
    for op in [BoolOp::Join, BoolOp::Cut, BoolOp::Intersect] {
        assert!(matches!(
            boolean(&a, &b, op),
            Err(KernelError::InvalidInput(_))
        ));
    }
}
