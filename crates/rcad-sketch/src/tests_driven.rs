//! Driven dimensions: a redundant dimension measures instead of driving.
//! Expected values are the rectangle's diagonal sqrt(w² + h²).
use super::*;

/// Fully dimensioned w x h rectangle with its first corner fixed at the
/// origin. Returns the sketch and the corner points (BL, BR, TR, TL).
fn rect(w: f64, h: f64) -> (Sketch, [PointId; 4]) {
    let mut s = Sketch::default();
    let p = [
        s.add_point(0.0, 0.0),
        s.add_point(w + 1.0, 0.5),
        s.add_point(w + 2.0, h - 1.0),
        s.add_point(0.5, h + 1.0),
    ];
    let l: Vec<EntityId> = (0..4).map(|i| s.add_line(p[i], p[(i + 1) % 4])).collect();
    s.constraints.push(Constraint::FixedPoint {
        p: p[0],
        x: 0.0,
        y: 0.0,
    });
    s.constraints.push(Constraint::Horizontal { line: l[0] });
    s.constraints.push(Constraint::Horizontal { line: l[2] });
    s.constraints.push(Constraint::Vertical { line: l[1] });
    s.constraints.push(Constraint::Vertical { line: l[3] });
    s.constraints.push(Constraint::HorizontalDistance {
        a: p[0],
        b: p[1],
        dim: Dimension::new("width", w),
    });
    s.constraints.push(Constraint::VerticalDistance {
        a: p[1],
        b: p[2],
        dim: Dimension::new("height", h),
    });
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained, "{r:?}");
    (s, p)
}

fn diagonal(p: &[PointId; 4], value: f64, driven: bool) -> Constraint {
    Constraint::Distance {
        a: p[0],
        b: p[2],
        dim: if driven {
            Dimension::driven("diag", value)
        } else {
            Dimension::new("diag", value)
        },
    }
}

#[test]
fn a_redundant_dimension_is_driven_and_follows_the_geometry() {
    let (mut s, p) = rect(30.0, 40.0);
    let c = diagonal(&p, 50.0, false);
    assert!(s.dimension_is_redundant(&c), "the diagonal over-defines");
    s.constraints.push(diagonal(&p, 1.0, true)); // value is measured
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained, "no new equation");
    assert_eq!(r.dof, 0);
    let diag = |s: &Sketch| {
        s.dimensions()
            .into_iter()
            .find(|(n, _)| n == "diag")
            .unwrap()
            .1
    };
    assert!((diag(&s) - 50.0).abs() < 1e-9, "{}", diag(&s));
    s.set_dimension("width", 60.0).unwrap();
    assert!(s.solve().is_success());
    let want = (60f64 * 60.0 + 40.0 * 40.0).sqrt();
    assert!((diag(&s) - want).abs() < 1e-9, "{} vs {want}", diag(&s));
    // A driven dimension cannot be set.
    let err = s.set_dimension("diag", 10.0).unwrap_err();
    assert!(matches!(err, SketchError::Refused(_)), "{err}");
    // Making it driving would over-define the sketch.
    let i = s.constraints.len() - 1;
    s.constraints[i].dimension_mut().unwrap().driven = false;
    assert_eq!(s.solve().status, SolveStatus::OverConstrained);
}

#[test]
fn independent_and_conflicting_dimensions_are_not_driven() {
    let (mut s, p) = rect(30.0, 40.0);
    // Remove the height: the diagonal is now independent (driving).
    s.constraints
        .retain(|c| c.dimension().is_none_or(|d| d.name != "height"));
    assert!(!s.dimension_is_redundant(&diagonal(&p, 50.0, false)));
    // Back to fully dimensioned: a diagonal of the wrong value conflicts.
    let (s, p) = rect(30.0, 40.0);
    assert!(!s.dimension_is_redundant(&diagonal(&p, 55.0, false)));
    let mut t = s.clone();
    t.constraints.push(diagonal(&p, 55.0, false));
    assert_eq!(t.solve().status, SolveStatus::Inconsistent);
}

#[test]
fn driven_flag_round_trips_and_older_files_are_driving() {
    let d = Dimension::driven("diag", 50.0);
    let j = serde_json::to_string(&d).unwrap();
    assert!(j.contains("\"driven\":true"), "{j}");
    assert_eq!(serde_json::from_str::<Dimension>(&j).unwrap(), d);
    let old: Dimension = serde_json::from_str(r#"{"name":"w","value":3.0}"#).unwrap();
    assert!(!old.driven);
    assert!(!serde_json::to_string(&old).unwrap().contains("driven"));
    // Only measurable kinds may be driven.
    let (mut s, p) = rect(30.0, 40.0);
    s.constraints.push(Constraint::PatternRotate {
        from: p[1],
        to: p[3],
        center: p[0],
        k: 1,
        dim: Dimension::driven("pa", 90.0),
        axis: None,
    });
    assert!(s.validate().is_err());
}
