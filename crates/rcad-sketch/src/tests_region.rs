//! Profile regions (outer loop + holes).
use crate::*;

fn poly(s: &mut Sketch, v: &[[f64; 2]]) -> Vec<EntityId> {
    let p: Vec<_> = v.iter().map(|q| s.add_point(q[0], q[1])).collect();
    (0..p.len())
        .map(|i| s.add_line(p[i], p[(i + 1) % p.len()]))
        .collect()
}

fn circle(s: &mut Sketch, c: [f64; 2], r: f64) -> EntityId {
    let p = s.add_point(c[0], c[1]);
    s.add_circle(p, r)
}

fn base() -> Sketch {
    let mut s = Sketch::default();
    s.ensure_origin();
    s
}

#[test]
fn rectangle_with_circle_and_slot_holes() {
    let mut s = base();
    let outer = poly(
        &mut s,
        &[[0.0, 0.0], [80.0, 0.0], [80.0, 50.0], [0.0, 50.0]],
    );
    let c = circle(&mut s, [20.0, 25.0], 5.0);
    let slot = poly(
        &mut s,
        &[[50.0, 22.0], [60.0, 22.0], [60.0, 28.0], [50.0, 28.0]],
    );
    let r = s.profile_region().unwrap();
    assert!(matches!(r.outer, Profile::Polygon(ref v) if v.len() == 4));
    let mut o = r.outer_entities.clone();
    o.sort();
    assert_eq!(o, outer);
    assert_eq!(r.holes.len(), 2);
    assert!(
        r.holes
            .iter()
            .any(|(p, e)| matches!(p, Profile::Circle { .. }) && e == &vec![c])
    );
    assert!(r.holes.iter().any(|(_, e)| {
        let mut e = e.clone();
        e.sort();
        e == slot
    }));
    // The single-loop API still refuses several loops.
    assert!(s.profile().is_err());
}

#[test]
fn single_loop_region_matches_profile() {
    let mut s = base();
    poly(&mut s, &[[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]]);
    let r = s.profile_region().unwrap();
    assert_eq!(r.outer, s.profile().unwrap());
    assert!(r.holes.is_empty());
}

#[test]
fn islands_separate_outers_and_touching_loops_are_refused() {
    // Island inside a hole.
    let mut s = base();
    poly(
        &mut s,
        &[[0.0, 0.0], [80.0, 0.0], [80.0, 50.0], [0.0, 50.0]],
    );
    circle(&mut s, [40.0, 25.0], 20.0);
    circle(&mut s, [40.0, 25.0], 5.0);
    let e = s.profile_region().unwrap_err().to_string();
    assert!(e.contains("island"), "{e}");
    // Two separate outer loops.
    let mut s = base();
    poly(
        &mut s,
        &[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
    );
    circle(&mut s, [30.0, 5.0], 3.0);
    let e = s.profile_region().unwrap_err().to_string();
    assert!(e.contains("separate outer loops"), "{e}");
    // Hole crossing the outer boundary.
    let mut s = base();
    poly(
        &mut s,
        &[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
    );
    circle(&mut s, [10.0, 5.0], 2.0);
    let e = s.profile_region().unwrap_err().to_string();
    assert!(e.contains("touch or cross"), "{e}");
    // Hole tangent to the outer boundary (touching counts).
    let mut s = base();
    poly(
        &mut s,
        &[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
    );
    circle(&mut s, [5.0, 3.0], 3.0);
    let e = s.profile_region().unwrap_err().to_string();
    assert!(e.contains("touch or cross"), "{e}");
}

#[test]
fn hole_inside_an_arc_bulge_is_classified_by_the_arc() {
    // D-shaped outer loop: line (0,0)-(20,0), then a CCW half circle back over
    // the top (centre (10,0), r 10). A hole at (10, 7) lies only inside the arc's
    // circular segment (above the chord), so the arc term decides containment.
    let mut s = base();
    let a = s.add_point(0.0, 0.0);
    let b = s.add_point(20.0, 0.0);
    let c = s.add_point(10.0, 0.0);
    s.add_line(a, b);
    s.entities.push(Entity::Arc {
        id: s.next_entity_id(),
        center: c,
        start: b,
        end: a,
    });
    circle(&mut s, [10.0, 7.0], 1.0);
    let r = s.profile_region().unwrap();
    assert_eq!(r.holes.len(), 1);
    // Outside the D (below the line): two outer loops.
    let mut s2 = s.clone();
    if let Some(Entity::Circle { center, .. }) = s2.entities.last().cloned() {
        let p = s2.points.iter_mut().find(|p| p.id == center).unwrap();
        p.y = -5.0;
    }
    assert!(s2.profile_region().is_err());
}

#[test]
fn hole_touching_an_arc_chord_line_inside_is_accepted() {
    // Review finding: square whose top is an arc (10,10)->(0,10) via (5,12), and
    // a hole whose top point (5,10) lies on the arc's chord line. Classifying by
    // chord + segment toggle saw no crossing and refused "2 separate outer loops".
    let mut s = base();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(10.0, 0.0);
    let p2 = s.add_point(10.0, 10.0);
    let p3 = s.add_point(0.0, 10.0);
    s.add_line(p0, p1);
    s.add_line(p1, p2);
    s.add_line(p3, p0);
    s.add_arc_3pt(p2, p3, [5.0, 12.0]).unwrap();
    circle(&mut s, [5.0, 9.0], 1.0);
    let r = s.profile_region().unwrap();
    assert_eq!(r.holes.len(), 1);
    assert!(matches!(r.holes[0].0, Profile::Circle { .. }));
}

#[test]
fn loop_under_an_inward_arch_is_refused() {
    // Review finding: the bottom edge arches up into the part, (0,0)->(10,0) via
    // (5,3); the triangle under the arch is outside the part. It was accepted as
    // a hole (its first-edge midpoint (5.5,0) lies on the chord line), which gave
    // a solid with material "subtracted" outside the face.
    let mut s = base();
    let p0 = s.add_point(0.0, 0.0);
    let p1 = s.add_point(10.0, 0.0);
    let p2 = s.add_point(10.0, 10.0);
    let p3 = s.add_point(0.0, 10.0);
    s.add_arc_3pt(p0, p1, [5.0, 3.0]).unwrap();
    s.add_line(p1, p2);
    s.add_line(p2, p3);
    s.add_line(p3, p0);
    poly(&mut s, &[[5.0, -1.0], [6.0, 1.0], [4.0, 1.0]]);
    let e = s.profile_region().unwrap_err().to_string();
    assert!(e.contains("separate outer loops"), "{e}");
}

#[test]
fn invalid_sketch_region_is_an_error_not_a_panic() {
    let mut s = base();
    poly(&mut s, &[[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]]);
    s.entities.push(Entity::Line {
        id: EntityId(99),
        a: PointId(500),
        b: PointId(501),
    });
    assert_eq!(
        s.profile_region().unwrap_err(),
        SketchError::MissingPoint(PointId(500))
    );
}

/// Independent containment oracle for the sweeps below: loops are tessellated
/// finely from their construction parameters (not from the sketch's profile
/// code) and classified by winding number.
mod oracle {
    use std::f64::consts::{PI, TAU};

    /// Deterministic LCG (no dev-dependency needed).
    pub struct Rng(u64);
    impl Rng {
        pub fn new(seed: u64) -> Rng {
            Rng(seed)
        }
        /// Uniform in [lo, hi).
        pub fn f(&mut self, lo: f64, hi: f64) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            lo + (hi - lo) * ((self.0 >> 11) as f64 / (1u64 << 53) as f64)
        }
    }

    pub fn circle(c: [f64; 2], r: f64, n: usize) -> Vec<[f64; 2]> {
        (0..n)
            .map(|i| {
                let t = TAU * i as f64 / n as f64;
                [c[0] + r * t.cos(), c[1] + r * t.sin()]
            })
            .collect()
    }

    /// Points of the arc from `a` through `m` to `b`, excluding `b`.
    pub fn arc3(a: [f64; 2], m: [f64; 2], b: [f64; 2], n: usize) -> Vec<[f64; 2]> {
        let d = 2.0 * (a[0] * (m[1] - b[1]) + m[0] * (b[1] - a[1]) + b[0] * (a[1] - m[1]));
        let sq = |p: [f64; 2]| p[0] * p[0] + p[1] * p[1];
        let c = [
            (sq(a) * (m[1] - b[1]) + sq(m) * (b[1] - a[1]) + sq(b) * (a[1] - m[1])) / d,
            (sq(a) * (b[0] - m[0]) + sq(m) * (a[0] - b[0]) + sq(b) * (m[0] - a[0])) / d,
        ];
        let ang = |p: [f64; 2]| (p[1] - c[1]).atan2(p[0] - c[0]);
        let ccw = |from: f64, to: f64| (to - from).rem_euclid(TAU);
        let (ta, tm, tb) = (ang(a), ang(m), ang(b));
        // Signed sweep from a to b that passes m.
        let sweep = if ccw(ta, tm) < ccw(ta, tb) {
            ccw(ta, tb)
        } else {
            ccw(ta, tb) - TAU
        };
        let r = ((a[0] - c[0]).powi(2) + (a[1] - c[1]).powi(2)).sqrt();
        (0..n)
            .map(|i| {
                let t = ta + sweep * i as f64 / n as f64;
                [c[0] + r * t.cos(), c[1] + r * t.sin()]
            })
            .collect()
    }

    pub fn winding(x: [f64; 2], poly: &[[f64; 2]]) -> f64 {
        let mut w = 0.0;
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            let ta = (a[1] - x[1]).atan2(a[0] - x[0]);
            let tb = (b[1] - x[1]).atan2(b[0] - x[0]);
            let mut d = tb - ta;
            if d > PI {
                d -= TAU;
            } else if d < -PI {
                d += TAU;
            }
            w += d;
        }
        w / TAU
    }

    fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        ((p[0] - a[0] - t * dx).powi(2) + (p[1] - a[1] - t * dy).powi(2)).sqrt()
    }

    /// Smallest vertex-to-polyline distance between two closed polylines.
    fn gap(u: &[[f64; 2]], v: &[[f64; 2]]) -> f64 {
        let one = |u: &[[f64; 2]], v: &[[f64; 2]]| {
            u.iter()
                .map(|&p| {
                    (0..v.len())
                        .map(|i| seg_dist(p, v[i], v[(i + 1) % v.len()]))
                        .fold(f64::INFINITY, f64::min)
                })
                .fold(f64::INFINITY, f64::min)
        };
        one(u, v).min(one(v, u))
    }

    /// Some(true) when the hole is clearly inside the outer loop, Some(false)
    /// when clearly outside, None when too close to call (or crossing).
    pub fn classify(outer: &[[f64; 2]], hole: &[[f64; 2]], margin: f64) -> Option<bool> {
        if gap(outer, hole) < margin {
            return None;
        }
        let ins: Vec<bool> = hole
            .iter()
            .map(|&p| winding(p, outer).abs() > 0.5)
            .collect();
        if ins.iter().all(|&b| b) {
            Some(true)
        } else if ins.iter().all(|&b| !b) {
            Some(false)
        } else {
            None
        }
    }
}

/// Compare `profile_region` against the oracle for one outer + one hole case.
fn check_against_oracle(s: &Sketch, outer: &[[f64; 2]], hole: &[[f64; 2]]) -> Option<bool> {
    let expect = oracle::classify(outer, hole, 0.05)?;
    let got = s.profile_region();
    match (&got, expect) {
        (Ok(r), true) => assert_eq!(r.holes.len(), 1, "expected one hole"),
        (Err(e), false) => assert!(
            e.to_string().contains("2 separate outer loops"),
            "unexpected error {e}"
        ),
        _ => panic!(
            "region disagrees with the oracle (hole inside: {expect}): {:?}\nhole {:?}",
            got.map(|r| r.holes.len()),
            &hole[..3]
        ),
    }
    Some(expect)
}

#[test]
fn sweep_circle_outer_holes_on_the_centre_line_agree_with_oracle() {
    // The reviewer's sweep: a circle's half arcs have their chords on y = cy;
    // holes whose test point lands on that line used to be misclassified.
    let mut rng = oracle::Rng::new(0x5eed_0001);
    let (mut inside, mut outside) = (0, 0);
    for case in 0..1500 {
        let (cx, cy) = (rng.f(-20.0, 20.0).round(), rng.f(-20.0, 20.0).round());
        let r = [3.0, 5.0, 7.5, 10.0, 25.0][case % 5];
        let dx = rng.f(-r - 4.0, r + 4.0);
        let mut s = base();
        circle(&mut s, [cx, cy], r);
        let outer = oracle::circle([cx, cy], r, 2048);
        let hole = if case % 2 == 0 {
            // Circle hole whose top touches y = cy.
            let rh = [0.5, 1.0, 1.5][case % 3];
            circle(&mut s, [cx + dx, cy - rh], rh);
            oracle::circle([cx + dx, cy - rh], rh, 64)
        } else {
            // Triangle whose first edge has its midpoint on y = cy.
            let v = [
                [cx + dx, cy - 1.0],
                [cx + dx + 1.0, cy + 1.0],
                [cx + dx - 1.0, cy + 1.0],
            ];
            poly(&mut s, &v);
            v.to_vec()
        };
        match check_against_oracle(&s, &outer, &hole) {
            Some(true) => inside += 1,
            Some(false) => outside += 1,
            None => {}
        }
    }
    assert!(
        inside > 300 && outside > 100,
        "{inside} inside, {outside} outside"
    );
}

#[test]
fn sweep_arc_sided_square_holes_on_the_chord_line_agree_with_oracle() {
    // A 10 x 10 square whose bottom or top side is an arc bulging out of or into
    // the square; holes have their top, bottom or first-edge midpoint on the
    // chord line, near it, or at a random height.
    let mut rng = oracle::Rng::new(0x5eed_0002);
    let (mut inside, mut outside) = (0, 0);
    for case in 0..1500 {
        let sag = loop {
            let v = rng.f(-4.5, 4.5);
            if v.abs() > 0.5 {
                break v;
            }
        };
        let top = case % 2 == 0;
        let chord_y = if top { 10.0 } else { 0.0 };
        let mut s = base();
        let p0 = s.add_point(0.0, 0.0);
        let p1 = s.add_point(10.0, 0.0);
        let p2 = s.add_point(10.0, 10.0);
        let p3 = s.add_point(0.0, 10.0);
        let mut outer = Vec::new();
        if top {
            s.add_line(p0, p1);
            s.add_line(p1, p2);
            s.add_arc_3pt(p2, p3, [5.0, 10.0 + sag]).unwrap();
            s.add_line(p3, p0);
            outer.extend([[0.0, 0.0], [10.0, 0.0]]);
            outer.extend(oracle::arc3(
                [10.0, 10.0],
                [5.0, 10.0 + sag],
                [0.0, 10.0],
                2048,
            ));
        } else {
            s.add_arc_3pt(p0, p1, [5.0, sag]).unwrap();
            s.add_line(p1, p2);
            s.add_line(p2, p3);
            s.add_line(p3, p0);
            outer.extend(oracle::arc3([0.0, 0.0], [5.0, sag], [10.0, 0.0], 2048));
            outer.extend([[10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
        }
        let hx = rng.f(-3.0, 13.0);
        let y = match case % 3 {
            0 => chord_y,
            1 => chord_y + rng.f(-0.3, 0.3),
            _ => rng.f(-5.0, 15.0),
        };
        let hole = match (case / 2) % 3 {
            0 => {
                let rh = rng.f(0.2, 1.0);
                circle(&mut s, [hx, y - rh], rh);
                oracle::circle([hx, y - rh], rh, 64)
            }
            1 => {
                let rh = rng.f(0.2, 1.0);
                circle(&mut s, [hx, y + rh], rh);
                oracle::circle([hx, y + rh], rh, 64)
            }
            _ => {
                let v = [[hx, y - 0.5], [hx + 0.5, y + 0.5], [hx - 0.5, y + 0.5]];
                poly(&mut s, &v);
                v.to_vec()
            }
        };
        match check_against_oracle(&s, &outer, &hole) {
            Some(true) => inside += 1,
            Some(false) => outside += 1,
            None => {}
        }
    }
    assert!(
        inside > 200 && outside > 200,
        "{inside} inside, {outside} outside"
    );
}
