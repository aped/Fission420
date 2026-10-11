use super::*;

const EPS: f64 = 1e-9;

fn corners(p: &Profile) -> Vec<[f64; 2]> {
    match p {
        Profile::Polygon(v) => v.clone(),
        other => panic!("expected polygon, got {other:?}"),
    }
}

fn assert_rect(s: &Sketch, w: f64, h: f64) {
    let got = corners(&s.profile().expect("profile"));
    let want = [
        [-w / 2.0, -h / 2.0],
        [w / 2.0, -h / 2.0],
        [w / 2.0, h / 2.0],
        [-w / 2.0, h / 2.0],
    ];
    assert_eq!(got.len(), 4);
    for (g, e) in got.iter().zip(want.iter()) {
        assert!(
            (g[0] - e[0]).abs() <= EPS && (g[1] - e[1]).abs() <= EPS,
            "{g:?} vs {e:?}"
        );
    }
}

/// Scramble the initial guess so the solver has real work to do.
fn perturb(s: &mut Sketch) {
    for (i, p) in s.points.iter_mut().enumerate() {
        p.x += 3.7 * (i as f64 + 1.0);
        p.y -= 2.3 * (i as f64 * 0.5 + 1.0);
    }
}

#[test]
fn rectangle_fully_constrained() {
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    perturb(&mut s);
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained, "{r:?}");
    assert_eq!(r.dof, 0);
    assert!(r.residual <= EPS);
    assert!(r.conflicting.is_empty());
    assert_rect(&s, 80.0, 50.0);
}

#[test]
fn rectangle_width_edit_resolves_centered() {
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    assert!(s.solve().is_success());
    s.set_dimension("width", 100.0).unwrap();
    // Profile must refuse to report geometry that no longer satisfies the constraints.
    assert!(matches!(s.profile(), Err(SketchError::NotSolved { .. })));
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained);
    assert!(r.iterations > 0);
    assert_rect(&s, 100.0, 50.0);
    assert_eq!(
        s.dimensions(),
        vec![("width".into(), 100.0), ("height".into(), 50.0)]
    );
}

#[test]
fn removing_constraint_underconstrains() {
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    s.constraints
        .retain(|c| !matches!(c, Constraint::HorizontalDistance { .. }));
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::UnderConstrained);
    assert_eq!(r.dof, 1);
    // Still a valid (rectangular, centred) profile from the solved state.
    assert!(s.profile().is_ok());
}

#[test]
fn conflicting_fixed_point_is_inconsistent_and_rolls_back() {
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    s.solve();
    s.constraints.push(Constraint::FixedPoint {
        p: PointId(1),
        x: -10.0,
        y: -10.0,
    });
    let before = s.clone();
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::Inconsistent, "{r:?}");
    assert!(r.residual > 1.0);
    let added = s.constraints.len() - 1;
    assert!(r.conflicting.contains(&added), "{:?}", r.conflicting);
    assert_eq!(s, before);
}

#[test]
fn redundant_consistent_constraint_is_overconstrained() {
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    // Same location the corner already must take: consistent but redundant.
    s.constraints.push(Constraint::FixedPoint {
        p: PointId(1),
        x: -40.0,
        y: -25.0,
    });
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::OverConstrained);
    assert!(r.residual <= EPS);
    assert_eq!(r.dof, 0);
    assert!(r.conflicting.contains(&(s.constraints.len() - 1)));
    // Fixing a corner duplicates information carried by the origin fix plus the
    // shape constraints, so the origin fix is in the redundant set as well.
    assert!(r.conflicting.contains(&0), "{:?}", r.conflicting);
}

#[test]
fn set_dimension_rejects_bad_input() {
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    let before = s.clone();
    for v in [-5.0, 0.0, f64::NAN, f64::INFINITY] {
        assert!(matches!(
            s.set_dimension("width", v),
            Err(SketchError::InvalidDimension { .. })
        ));
    }
    assert!(matches!(
        s.set_dimension("depth", 10.0),
        Err(SketchError::UnknownDimension(_))
    ));
    assert_eq!(s, before);
}

#[test]
fn constructors_reject_bad_dimensions() {
    assert!(Sketch::centered_rectangle(0.0, 5.0).is_err());
    assert!(Sketch::centered_rectangle(5.0, f64::NAN).is_err());
    assert!(Sketch::centered_circle(-1.0).is_err());
}

#[test]
fn circle_fully_constrained() {
    let mut s = Sketch::centered_circle(10.0).unwrap();
    s.points[1].x = 3.0;
    if let Entity::Circle { radius, .. } = &mut s.entities[0] {
        *radius = 1.0;
    }
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained);
    assert_eq!(r.dof, 0);
    match s.profile().unwrap() {
        Profile::Circle { center, radius } => {
            assert!((radius - 5.0).abs() <= EPS);
            assert!(center[0].abs() <= EPS && center[1].abs() <= EPS);
        }
        p => panic!("{p:?}"),
    }
}

#[test]
fn json_round_trip() {
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    s.solve();
    let json = serde_json::to_string(&s).unwrap();
    let back: Sketch = serde_json::from_str(&json).unwrap();
    assert_eq!(s, back);
    let c = Sketch::centered_circle(10.0).unwrap();
    let back: Sketch = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
    assert_eq!(c, back);
}

#[test]
fn invalid_references_fail_clearly() {
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    s.constraints
        .push(Constraint::Horizontal { line: EntityId(99) });
    let before = s.clone();
    assert_eq!(s.validate(), Err(SketchError::MissingEntity(EntityId(99))));
    assert_eq!(s.solve().status, SolveStatus::Invalid);
    assert_eq!(s, before);
    assert!(s.profile().is_err());

    let mut s = Sketch::centered_circle(10.0).unwrap();
    s.constraints
        .push(Constraint::Horizontal { line: EntityId(1) });
    assert!(matches!(
        s.validate(),
        Err(SketchError::WrongEntityKind { .. })
    ));
}

#[test]
fn duplicate_dimension_names_rejected() {
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    s.constraints.push(Constraint::HorizontalDistance {
        a: PointId(4),
        b: PointId(3),
        dim: Dimension::new("width", 80.0),
    });
    assert_eq!(
        s.validate(),
        Err(SketchError::DuplicateDimension("width".into()))
    );
}

#[test]
fn profile_rejects_open_and_mixed_geometry() {
    let mut s = Sketch::centered_rectangle(80.0, 50.0).unwrap();
    s.solve();
    let mut open = s.clone();
    open.entities.pop();
    open.constraints
        .retain(|c| !matches!(c, Constraint::Vertical { line } if *line == EntityId(4)));
    assert!(matches!(
        open.profile(),
        Err(SketchError::UnsupportedProfile(_))
    ));

    let mut mixed = s.clone();
    mixed.entities.push(Entity::Circle {
        id: EntityId(10),
        center: PointId(0),
        radius: 3.0,
    });
    assert!(matches!(
        mixed.profile(),
        Err(SketchError::UnsupportedProfile(_))
    ));

    assert!(matches!(
        Sketch::default().profile(),
        Err(SketchError::UnsupportedProfile(_))
    ));
}

#[test]
fn profile_ccw_even_when_lines_drawn_clockwise() {
    // Triangle drawn clockwise; no constraints, so the current state is "solved".
    let s = Sketch {
        construction: Default::default(),
        origin: None,
        points: vec![
            SketchPoint {
                id: PointId(1),
                x: 0.0,
                y: 0.0,
            },
            SketchPoint {
                id: PointId(2),
                x: 0.0,
                y: 10.0,
            },
            SketchPoint {
                id: PointId(3),
                x: 10.0,
                y: 0.0,
            },
        ],
        entities: vec![
            Entity::Line {
                id: EntityId(1),
                a: PointId(1),
                b: PointId(2),
            },
            Entity::Line {
                id: EntityId(2),
                a: PointId(2),
                b: PointId(3),
            },
            Entity::Line {
                id: EntityId(3),
                a: PointId(3),
                b: PointId(1),
            },
        ],
        constraints: vec![],
    };
    assert_eq!(
        corners(&s.profile().unwrap()),
        vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]]
    );
}

#[test]
fn profile_rejects_self_intersecting_bowtie() {
    let pts = [(0.0, 0.0), (10.0, 10.0), (10.0, 0.0), (0.0, 10.0)];
    let s = Sketch {
        construction: Default::default(),
        origin: None,
        points: pts
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| SketchPoint {
                id: PointId(i as u32),
                x,
                y,
            })
            .collect(),
        entities: (0..4)
            .map(|i| Entity::Line {
                id: EntityId(i),
                a: PointId(i),
                b: PointId((i + 1) % 4),
            })
            .collect(),
        constraints: vec![],
    };
    assert!(matches!(
        s.profile(),
        Err(SketchError::UnsupportedProfile(_))
    ));
}

#[test]
fn large_coordinates_use_scale_aware_tolerance() {
    let mut s = Sketch::centered_rectangle(5.0e6, 1.0e-3).unwrap();
    let r = s.solve();
    assert_eq!(r.status, SolveStatus::FullyConstrained, "{r:?}");
    assert!(s.profile().is_ok());
}
