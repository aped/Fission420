//! Slab booleans with arcs and cylindrical walls (ADR 0007). Expected volumes
//! are analytic (disc unions, rounded rectangles); every result also passes
//! the independent result check (ADR 0005), exports to STEP, and is probed for
//! material occupancy by the winding number of its own tessellation.
use super::*;
use crate::{Prism, PrismOp, VolumeResult, cut_prism, export_step, extrude, measure};
use std::f64::consts::PI;

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Profile {
    Profile::Polygon(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
}

fn disc(c: [f64; 2], r: f64) -> Profile {
    Profile::Circle {
        center: c,
        radius: r,
    }
}

fn volume(b: &Body) -> f64 {
    match measure(b).unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        VolumeResult::Unavailable { reason } => panic!("volume unavailable: {reason}"),
    }
}

/// One closed shell, the independent result check against the nominal tool
/// (with `holes`), and STEP export.
fn checked(
    t: &Body,
    r: &Body,
    p: &Profile,
    holes: &[Profile],
    start: &Frame,
    len: f64,
    op: PrismOp,
) {
    let m = measure(r).unwrap();
    assert_eq!((m.shell_count, m.all_shells_closed), (1, true));
    let prism = Prism {
        profile: p,
        holes,
        start,
        length: len,
    };
    crate::verify_prism_boolean(t, r, op, &prism, (0.0, len)).unwrap();
    export_step(r, "test").unwrap();
}

/// Area of the union of two discs (radii `a`, `b`, centres `d` apart).
pub(super) fn disc_union(a: f64, b: f64, d: f64) -> f64 {
    if d >= a + b {
        return PI * (a * a + b * b);
    }
    if d <= (a - b).abs() {
        return PI * a.max(b).powi(2);
    }
    let lens = a * a * ((d * d + a * a - b * b) / (2.0 * d * a)).acos()
        + b * b * ((d * d + b * b - a * a) / (2.0 * d * b)).acos()
        - 0.5 * ((-d + a + b) * (d + a - b) * (d - a + b) * (d + a + b)).sqrt();
    PI * (a * a + b * b) - lens
}

/// 80 x 50 x 10 plate with a through hole of radius `r` at (40, 25) (Truck).
fn holed_plate(r: f64) -> Body {
    let plate = extrude(&rect(0.0, 0.0, 80.0, 50.0), &Frame::XY, 10.0).unwrap();
    cut_prism(
        &plate,
        &disc([40.0, 25.0], r),
        &Frame::XY.offset(-1.0),
        12.0,
    )
    .unwrap()
}

/// Distances from the axis through `c` (along z) of the result's curved faces,
/// sampled at their parameter-domain centres.
fn curved_radii(b: &Body, c: [f64; 2]) -> Vec<f64> {
    use std::ops::Bound::Included;
    b.solid
        .face_iter()
        .filter_map(|f| match f.surface() {
            Surface::Plane(_) => None,
            s => match s.parameter_range() {
                ((Included(u0), Included(u1)), (Included(v0), Included(v1))) => {
                    let p = s.subs((u0 + u1) / 2.0, (v0 + v1) / 2.0);
                    Some((p.x - c[0]).hypot(p.y - c[1]))
                }
                _ => Some(f64::NAN),
            },
        })
        .collect()
}

fn probe(b: &Body, pts: &[([f64; 3], bool)]) {
    let got = crate::point_membership(b, &pts.iter().map(|p| p.0).collect::<Vec<_>>()).unwrap();
    for (p, g) in pts.iter().zip(got) {
        assert_eq!(g, Some(p.1), "probe {:?}", p.0);
    }
}

/// Pocket of `depth` from the plate top (z = 10) down.
fn pocket(t: &Body, p: &Profile, depth: f64) -> Result<Body> {
    prism_boolean(t, p, &Frame::XY.offset(10.0 - depth), depth, SlabOp::Cut)
}

#[test]
fn engulfing_cut_removes_the_old_hole_exactly() {
    // Canonical case: Ø10 through hole, then a concentric Ø16 through cut.
    let t = holed_plate(5.0);
    let tool = disc([40.0, 25.0], 8.0);
    let start = Frame::XY.offset(-1.0);
    let r = prism_boolean(&t, &tool, &start, 12.0, SlabOp::Cut).unwrap();
    assert!(
        (volume(&r) - (40000.0 - 640.0 * PI)).abs() < 1e-6,
        "{}",
        volume(&r)
    );
    checked(&t, &r, &tool, &[], &start, 12.0, PrismOp::Cut);
    // No vestigial Ø10 wall: every curved face is on the Ø16 cylinder.
    let radii = curved_radii(&r, [40.0, 25.0]);
    assert_eq!(radii.len(), 4, "{radii:?}");
    assert!(radii.iter().all(|r| (r - 8.0).abs() < 1e-9), "{radii:?}");
    // 6 plate faces + 4 quarter-turn hole patches.
    assert_eq!(measure(&r).unwrap().face_count, 10);
    probe(
        &r,
        &[
            ([40.0, 25.0, 5.0], false),
            ([46.0, 25.0, 5.0], false), // between the old and the new wall
            ([40.0, 32.9, 9.9], false),
            ([48.2, 25.0, 5.0], true),
            ([40.0, 16.8, 0.1], true),
        ],
    );
}

#[test]
fn partially_overlapping_round_cuts_are_exact() {
    for (dx, dy, r1, through) in [
        (4.0, 0.0, 8.0, true),
        (0.0, -4.0, 8.0, false),
        (3.0, 2.5, 6.5, true),
        (6.0, 0.0, 3.0, false),
    ] {
        let t = holed_plate(5.0);
        let tool = disc([40.0 + dx, 25.0 + dy], r1);
        let (start, len) = if through {
            (Frame::XY.offset(-1.0), 12.0)
        } else {
            (Frame::XY.offset(6.0), 4.0) // blind, 4 deep from the top
        };
        let r = prism_boolean(&t, &tool, &start, len, SlabOp::Cut).unwrap();
        let d = dx.hypot(dy);
        let want = if through {
            40000.0 - 10.0 * disc_union(5.0, r1, d)
        } else {
            40000.0 - 250.0 * PI - 4.0 * (disc_union(5.0, r1, d) - 25.0 * PI)
        };
        assert!(
            (volume(&r) - want).abs() < 1e-6,
            "offset ({dx}, {dy}) r {r1}: {} vs {want}",
            volume(&r)
        );
        checked(&t, &r, &tool, &[], &start, len, PrismOp::Cut);
    }
}

#[test]
fn rectangular_pocket_across_a_round_hole() {
    // Pocket x in [40, 60] over half the Ø10 hole, 4 deep.
    let t = holed_plate(5.0);
    let p = rect(40.0, 15.0, 60.0, 35.0);
    let r = pocket(&t, &p, 4.0).unwrap();
    let want = 40000.0 - 250.0 * PI - 4.0 * (400.0 - 12.5 * PI);
    assert!((volume(&r) - want).abs() < 1e-6, "{} vs {want}", volume(&r));
    checked(&t, &r, &p, &[], &Frame::XY.offset(6.0), 4.0, PrismOp::Cut);
    assert!(
        curved_radii(&r, [40.0, 25.0])
            .iter()
            .all(|r| (r - 5.0).abs() < 1e-9)
    );
}

/// Rounded rectangle [x0, x1] x [y0, y1] with corner radius `rr`.
pub(super) fn rounded(x0: f64, y0: f64, x1: f64, y1: f64, rr: f64) -> Profile {
    let l = |s: [f64; 2]| LoopSegment::Line { start: s };
    let a = |s: [f64; 2], c: [f64; 2]| LoopSegment::Arc {
        start: s,
        center: c,
        ccw: true,
    };
    Profile::Loop(vec![
        l([x0 + rr, y0]),
        a([x1 - rr, y0], [x1 - rr, y0 + rr]),
        l([x1, y0 + rr]),
        a([x1, y1 - rr], [x1 - rr, y1 - rr]),
        l([x1 - rr, y1]),
        a([x0 + rr, y1], [x0 + rr, y1 - rr]),
        l([x0, y1 - rr]),
        a([x0, y0 + rr], [x0 + rr, y0 + rr]),
    ])
}

fn rr_area(w: f64, h: f64, r: f64) -> f64 {
    w * h - (4.0 - PI) * r * r
}

#[test]
fn rounded_pockets_sharing_a_corner_circle() {
    // Rounded pocket containing the Ø10 hole, 3 deep.
    let t = holed_plate(5.0);
    let p = rounded(30.0, 15.0, 55.0, 35.0, 4.0);
    let r = pocket(&t, &p, 3.0).unwrap();
    let want = 40000.0 - 250.0 * PI - 3.0 * (rr_area(25.0, 20.0, 4.0) - 25.0 * PI);
    assert!((volume(&r) - want).abs() < 1e-6, "{} vs {want}", volume(&r));
    checked(&t, &r, &p, &[], &Frame::XY.offset(7.0), 3.0, PrismOp::Cut);
    // A second rounded pocket, same depth (coplanar floors). Its SW corner is
    // rounded about (51, 31), the centre of the first pocket's NE corner: the
    // two arcs are opposite quarters of one circle. Their overlap is the
    // square [47, 55] x [27, 35] minus those two rounded corners.
    let q = rounded(47.0, 27.0, 70.0, 45.0, 4.0);
    let start = Frame::XY.offset(7.0);
    let r2 = prism_boolean(&r, &q, &start, 3.0, SlabOp::Cut).unwrap();
    let overlap = 64.0 - 2.0 * (1.0 - PI / 4.0) * 16.0;
    let want2 = want - 3.0 * (rr_area(23.0, 18.0, 4.0) - overlap);
    assert!(
        (volume(&r2) - want2).abs() < 1e-6,
        "{} vs {want2}",
        volume(&r2)
    );
    checked(&r, &r2, &q, &[], &start, 3.0, PrismOp::Cut);
    probe(
        &r2,
        &[
            // Outside one pocket's rounded corner but inside the other.
            ([54.5, 34.5, 8.0], false),
            ([47.5, 27.5, 8.0], false),
            ([51.0, 31.0, 8.0], false),
            // Outside both, beside the overlap.
            ([56.0, 26.0, 8.0], true),
            ([46.0, 36.0, 8.0], true),
            ([51.0, 31.0, 6.9], true), // common floor
        ],
    );
}

/// 40 x 30 x 10 block on XY.
fn block() -> Body {
    extrude(&rect(0.0, 0.0, 40.0, 30.0), &Frame::XY, 10.0).unwrap()
}

#[test]
fn round_boss_on_the_top_and_a_pocket_with_an_island() {
    // Cylinder boss standing exactly on the top (coplanar contact).
    let b = disc([20.0, 15.0], 6.0);
    let start = Frame::XY.offset(10.0);
    let j = prism_boolean(&block(), &b, &start, 5.0, SlabOp::Join).unwrap();
    assert!((volume(&j) - (12000.0 + 180.0 * PI)).abs() < 1e-6);
    checked(&block(), &j, &b, &[], &start, 5.0, PrismOp::Join);
    // A rectangular pocket with a round island: tool = rectangle minus disc.
    let outer = rect(5.0, 5.0, 35.0, 25.0);
    let island = [disc([20.0, 15.0], 4.0)];
    let start = Frame::XY.offset(6.0);
    let r = prism_region_boolean(&block(), &outer, &island, &start, 4.0, SlabOp::Cut).unwrap();
    assert!((volume(&r) - (12000.0 - 4.0 * (600.0 - 16.0 * PI))).abs() < 1e-6);
    checked(&block(), &r, &outer, &island, &start, 4.0, PrismOp::Cut);
    probe(
        &r,
        &[
            ([20.0, 15.0, 8.0], true), // the island stands
            ([10.0, 15.0, 8.0], false),
            ([20.0, 15.0, 5.0], true),
        ],
    );
}

#[test]
fn overlapping_blind_round_cuts_on_a_boss_seedhole_repro() {
    // Repro: 60 x 30 x 8 block centred on the origin, a
    // 10 x 10 x 5 boss at (15, 0), a Ø4 blind cut 2 deep at (15, 0), then a
    // second Ø4 2 deep at (16, 0.5): Truck panics on the second.
    let base = extrude(&rect(-30.0, -15.0, 30.0, 15.0), &Frame::XY, 8.0).unwrap();
    let boss = prism_boolean(
        &base,
        &rect(10.0, -5.0, 20.0, 5.0),
        &Frame::XY.offset(8.0),
        5.0,
        SlabOp::Join,
    )
    .unwrap();
    let h1 = cut_prism(&boss, &disc([15.0, 0.0], 2.0), &Frame::XY.offset(11.0), 3.0).unwrap();
    assert!((volume(&h1) - (14400.0 + 500.0 - 8.0 * PI)).abs() < 1e-6);
    let tool = disc([16.0, 0.5], 2.0);
    let start = Frame::XY.offset(11.0);
    let r = prism_boolean(&h1, &tool, &start, 2.0, SlabOp::Cut).unwrap();
    let want = 14400.0 + 500.0 - 2.0 * disc_union(2.0, 2.0, 0.5f64.hypot(1.0));
    assert!((volume(&r) - want).abs() < 1e-6, "{} vs {want}", volume(&r));
    checked(&h1, &r, &tool, &[], &start, 2.0, PrismOp::Cut);
}

#[test]
fn tangent_contacts_are_refused_by_name() {
    let t = holed_plate(5.0);
    // A pocket wall tangent to the hole (x = 45).
    let Err(e) = pocket(&t, &rect(45.0, 10.0, 60.0, 40.0), 3.0) else {
        panic!("accepted a tangent contact")
    };
    assert!(e.to_string().contains("tangent"), "{e}");
    // A round cut internally tangent to the hole.
    let Err(e) = pocket(&t, &disc([43.0, 25.0], 8.0), 3.0) else {
        panic!("accepted a tangent contact")
    };
    assert!(e.to_string().contains("tangent"), "{e}");
    // ... and externally tangent.
    let Err(e) = pocket(&t, &disc([50.0, 25.0], 5.0), 3.0) else {
        panic!("accepted a tangent contact")
    };
    assert!(e.to_string().contains("tangent"), "{e}");
}

/// Area of the axis-aligned rectangle [x0, x1] x [y0, y1] inside the disc
/// (c, r): adaptive Simpson over y of the exact chord overlap.
fn rect_disc_area(x0: f64, y0: f64, x1: f64, y1: f64, c: [f64; 2], r: f64) -> f64 {
    let f = |y: f64| {
        let d = y - c[1];
        if d.abs() >= r {
            return 0.0;
        }
        let s = (r * r - d * d).sqrt();
        (x1.min(c[0] + s) - x0.max(c[0] - s)).max(0.0)
    };
    #[allow(clippy::too_many_arguments)]
    fn simpson(
        f: &dyn Fn(f64) -> f64,
        a: f64,
        b: f64,
        fa: f64,
        fm: f64,
        fb: f64,
        whole: f64,
        depth: u32,
    ) -> f64 {
        let m = 0.5 * (a + b);
        let (lm, rm) = (0.5 * (a + m), 0.5 * (m + b));
        let (flm, frm) = (f(lm), f(rm));
        let left = (m - a) / 6.0 * (fa + 4.0 * flm + fm);
        let right = (b - m) / 6.0 * (fm + 4.0 * frm + fb);
        if depth == 0 || (left + right - whole).abs() < 1e-13 {
            return left + right + (left + right - whole) / 15.0;
        }
        simpson(f, a, m, fa, flm, fm, left, depth - 1)
            + simpson(f, m, b, fm, frm, fb, right, depth - 1)
    }
    // Split at the disc's extent so each piece is smooth up to its ends.
    let mut cuts = vec![y0, y1, c[1] - r, c[1] + r];
    cuts.retain(|y| *y >= y0 && *y <= y1);
    cuts.sort_by(f64::total_cmp);
    cuts.windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let (fa, fm, fb) = (f(a), f(0.5 * (a + b)), f(b));
            simpson(
                &f,
                a,
                b,
                fa,
                fm,
                fb,
                (b - a) / 6.0 * (fa + 4.0 * fm + fb),
                40,
            )
        })
        .sum()
}

/// Two special points on a circle very close together (a
/// pocket corner 1e-3 mm outside a Ø20 hole: its two edges cross the circle
/// about 2e-4 rad apart). The ring vertex between them deviates from their
/// chord by ~1e-8 mm; the boolean output clean-up must not drop it as
/// collinear (the arc would become a straight edge, or the build refused as
/// non-manifold).
#[test]
fn close_special_points_keep_their_arc() {
    let plate = extrude(&rect(0.0, 0.0, 60.0, 60.0), &Frame::XY, 10.0).unwrap();
    let c = [30.0, 30.0];
    let t = cut_prism(&plate, &disc(c, 10.0), &Frame::XY.offset(-1.0), 12.0).unwrap();
    let k = (10.0 + 1e-3) / 2f64.sqrt();
    let (px, py) = (c[0] + k, c[1] + k);
    let p = rect(21.0, 23.0, px, py);
    let r = pocket(&t, &p, 3.0).unwrap();
    let want = 36000.0
        - 1000.0 * PI
        - 3.0 * ((px - 21.0) * (py - 23.0) - rect_disc_area(21.0, 23.0, px, py, c, 10.0));
    assert!((volume(&r) - want).abs() < 1e-6, "{} vs {want}", volume(&r));
    checked(&t, &r, &p, &[], &Frame::XY.offset(7.0), 3.0, PrismOp::Cut);
    let radii = curved_radii(&r, c);
    assert!(radii.iter().all(|x| (x - 10.0).abs() < 1e-9), "{radii:?}");
    // No arc piece was flattened: no planar face has all its vertices on the
    // hole's circle.
    let on_circle = |p: Point3| ((p.x - c[0]).hypot(p.y - c[1]) - 10.0).abs() < 1e-6;
    for f in r.solid.face_iter() {
        if matches!(f.surface(), Surface::Plane(_)) {
            let all = f
                .boundaries()
                .iter()
                .all(|w| w.vertex_iter().all(|v| on_circle(v.point())));
            assert!(!all, "a flat face replaced an arc of the hole");
        }
    }
}
