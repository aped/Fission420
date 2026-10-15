//! Seeded corpus of round and rounded cuts (ADR 0007): through and
//! blind round holes that are concentric with (engulfing or engulfed by),
//! partially overlapping, or apart from earlier ones, rounded-rectangle and
//! rectangular pockets over them, on an 80 x 60 x 12 plate, all on 0.5 / 1 mm
//! grids so coincident radii, centres, floors and walls occur.
//!
//! Every accepted result is compared with an oracle computed from the nominal
//! CSG of the primitives only (never from kernel output):
//! * volume: plate minus the union of the cut prisms, integrated per z slab
//!   over exact scanline chord lengths (midpoint rule at N and 2N lines,
//!   Richardson-extrapolated; the difference is the oracle's error estimate);
//! * occupancy: random points (and points beside the new tool's boundary)
//!   classified by the primitives, against the result's winding number;
//!   points within 0.05 mm of a primitive boundary or level are skipped.
//!
//! A refusal must leave the document unchanged; refusals are counted by kind.
mod arc_corpus;

use arc_corpus::*;
use rcad_commands::{Command, ExtrudeOp, Session, SketchTemplate};
use rcad_document::{BodyId, PlaneBase, SketchPlane};

/// One test per case, so the test runner spreads the corpus over its workers. The
/// cases are independent and feature draws never depend on outcomes. A per-case
/// test does not record earlier cases' centres (see `arc_corpus::features`), so
/// after case 0 its features differ from the ignored whole-corpus run; both are
/// fixed by the seed.
macro_rules! arc_cases {
    ($($name:ident = $k:expr),* $(,)?) => {
        $(#[test] fn $name() { arc_corpus(Some($k)); })*
    };
}
arc_cases!(
    arc_case_00 = 0,
    arc_case_01 = 1,
    arc_case_02 = 2,
    arc_case_03 = 3,
    arc_case_04 = 4,
    arc_case_05 = 5,
    arc_case_06 = 6,
    arc_case_07 = 7,
    arc_case_08 = 8,
    arc_case_09 = 9,
    arc_case_10 = 10,
    arc_case_11 = 11,
    arc_case_12 = 12,
    arc_case_13 = 13,
    arc_case_14 = 14,
    arc_case_15 = 15,
    arc_case_16 = 16,
    arc_case_17 = 17,
    arc_case_18 = 18,
    arc_case_19 = 19,
    arc_case_20 = 20,
    arc_case_21 = 21,
    arc_case_22 = 22,
    arc_case_23 = 23,
);

/// The whole corpus in one process, for its single summary line (RCAD_ARC_CASE=k
/// limits it to one case):
/// `cargo test -p rcad-commands --release --test arc_fuzz -- --ignored --nocapture`
#[test]
#[ignore = "same cases as arc_case_00..23, run serially; for the corpus summary line"]
fn round_and_rounded_cuts_match_the_csg_oracle() {
    arc_corpus(
        std::env::var("RCAD_ARC_CASE")
            .ok()
            .and_then(|v| v.parse().ok()),
    );
}

/// Run the seeded corpus (all 24 cases, or only case `only`).
fn arc_corpus(only: Option<usize>) {
    let (mut accepted, mut probes, mut skipped) = (0usize, 0usize, 0usize);
    let mut refusals: Vec<String> = Vec::new();
    let t_start = std::time::Instant::now();
    let all = features(only);
    for case in 0..CASES {
        // Probe points come from their own stream, so the feature sequence
        // does not depend on outcomes (a single case can be replayed).
        let mut prng = Rng(0x9e37_79b9_7f4a_7c15 ^ (case as u64 + 1));
        if !all.iter().any(|f| f.case == case) {
            continue;
        }
        let (mut s, top) = plate();
        let mut prims: Vec<Prim> = Vec::new();
        for f in all.iter().filter(|f| f.case == case) {
            let (n, shape, through, depth, prim) = (f.n, f.shape, f.through, f.depth, f.prim);
            let rev = s.summary().revision;
            let before = vol(&s);
            let r = cut(&mut s, top, f);
            let what = format!(
                "case {case} #{n} {} {shape:?}",
                if through {
                    "through".to_string()
                } else {
                    format!("{depth} deep")
                }
            );
            let mut next = prims.clone();
            next.push(prim);
            if r.ok {
                accepted += 1;
                prims = next;
                let (want, est) = oracle_volume(&prims);
                let v = vol(&s);
                assert!(
                    (v - want).abs() <= 1e-6 * want + 10.0 * est,
                    "{what}: volume {v} vs oracle {want} (± {est}); cuts so far: {prims:#?}"
                );
                // Occupancy probes: random, and beside the new tool's boundary.
                let mut pts: Vec<[f64; 3]> = (0..160)
                    .map(|_| [prng.range(0.0, L), prng.range(0.0, W), prng.range(0.0, T)])
                    .collect();
                for k in 0..48 {
                    let t = k as f64 * std::f64::consts::TAU / 48.0;
                    let z = prng.range(prim.z0.max(0.0), prim.z1.min(T));
                    let (q_in, q_out) = match shape {
                        Shape::Circle { c, r } => (
                            [c[0] + (r - 0.2) * t.cos(), c[1] + (r - 0.2) * t.sin()],
                            [c[0] + (r + 0.2) * t.cos(), c[1] + (r + 0.2) * t.sin()],
                        ),
                        Shape::Rounded { lo, hi, .. } => {
                            let m = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
                            let e = [(hi[0] - lo[0]) / 2.0, (hi[1] - lo[1]) / 2.0];
                            let d = [t.cos(), t.sin()];
                            let k = (e[0] / d[0].abs().max(1e-9)).min(e[1] / d[1].abs().max(1e-9));
                            (
                                [m[0] + (k - 0.2) * d[0], m[1] + (k - 0.2) * d[1]],
                                [m[0] + (k + 0.2) * d[0], m[1] + (k + 0.2) * d[1]],
                            )
                        }
                    };
                    pts.push([q_in[0], q_in[1], z]);
                    pts.push([q_out[0], q_out[1], z]);
                }
                let model = s.model().unwrap();
                let body = model
                    .evaluation()
                    .body_by_id(BodyId(1))
                    .unwrap()
                    .geometry
                    .brep()
                    .unwrap();
                let got = rcad_kernel::point_membership(body, &pts).unwrap();
                for (p, g) in pts.iter().zip(got) {
                    match (oracle_point(&prims, *p, 0.05), g) {
                        (Some(o), Some(g)) => {
                            probes += 1;
                            assert_eq!(o, g, "{what}: probe {p:?} oracle {o}, result {g}");
                        }
                        _ => skipped += 1,
                    }
                }
            } else {
                let m = format!("{:?}", r.error);
                assert_eq!(
                    s.summary().revision,
                    rev + 1,
                    "{what}: only the sketch added"
                );
                assert!(
                    (vol(&s) - before).abs() < 1e-9,
                    "{what}: refusal changed it"
                );
                let (old, _) = oracle_volume(&prims);
                let (new, est) = oracle_volume(&next);
                let noop = (old - new).abs() <= 10.0 * est + 1e-9;
                // Head (which error) and tail (the slab construction's reason).
                let tail = m.len().saturating_sub(300).max(m.len().min(200));
                let tail = (tail..m.len())
                    .find(|&i| m.is_char_boundary(i))
                    .unwrap_or(m.len());
                refusals.push(format!(
                    "{what}{}: {} .. {}",
                    if noop { " [no-op]" } else { "" },
                    &m[..m.len().min(200)],
                    &m[tail..]
                ));
            }
        }
    }
    let secs = t_start.elapsed().as_secs_f64();
    let n = |k: &str| refusals.iter().filter(|l| l.contains(k)).count();
    eprintln!(
        "CORPUS arcs{}: features={} accepted_exact={accepted} accepted_incorrect=0 \
         refused_kernel={} refused_gate={} refused_product_limit={} refused_noop={} \
         (tangent {}, ambiguous {}) probes={probes} skipped={skipped} secs={secs:.1}",
        only.map(|k| format!(" case {k}")).unwrap_or_default(),
        accepted + refusals.len(),
        n("Kernel {"),
        n("GeometryCheck {"),
        n("ProductLimit("),
        n("[no-op]"),
        n("tangent"),
        n("ambiguous"),
    );
    for l in &refusals {
        eprintln!("  refused: {l}");
    }
}

/// Arc corpus finding (an earlier, outcome-dependent version of the
/// stream above, case 21 #4): a Ø11 blind cut externally tangent to the
/// straight wall of an earlier 1 mm deep rounded pocket. Truck returned a
/// closed solid in which that pocket was filled with material again (72.27
/// mm³ too much) and the result check accepted it: it probed result faces
/// only, and a refilled void leaves none. The check now also probes the
/// target's faces near the tool (ADR 0005 addendum): refused, unchanged. The
/// slab construction refuses too (a tangent contact elsewhere in the body).
/// With the truck-shapeops fork (order-stable topology walks) the outcome is
/// the same in every process: the refilled pocket, refused.
#[test]
fn tangent_round_cut_beside_a_shallow_rounded_pocket() {
    let mut s = Session::new();
    s.execute(&Command::NewDocument);
    let sk = s
        .execute(&Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: L,
                height: W,
                center: Some([L / 2.0, W / 2.0]),
            },
        })
        .feature
        .unwrap();
    s.execute(&Command::extrude(None, sk, T));
    let top = SketchPlane::Offset {
        base: PlaneBase::WorldXY,
        distance: T,
        angle: 0.0,
    };
    let cuts = [
        (
            Shape::Rounded {
                lo: [31.0, 38.5],
                hi: [41.0, 56.5],
                r: 1.0,
            },
            Some(9.0),
        ),
        (
            Shape::Rounded {
                lo: [54.0, 9.5],
                hi: [64.0, 17.5],
                r: 3.0,
            },
            Some(1.0),
        ),
        (
            Shape::Circle {
                c: [21.5, 17.5],
                r: 5.5,
            },
            Some(5.0),
        ),
        (
            Shape::Circle {
                c: [36.0, 47.5],
                r: 5.0,
            },
            None,
        ),
        (
            Shape::Circle {
                c: [48.5, 13.5],
                r: 5.5,
            },
            Some(6.0),
        ),
    ];
    let mut prims = Vec::new();
    for (i, (shape, depth)) in cuts.iter().enumerate() {
        let p = s
            .execute(&Command::CreateSketch {
                name: None,
                plane: top,
                template: template(shape),
            })
            .feature
            .unwrap();
        let rev = s.summary().revision;
        let r = s.execute(&Command::Extrude {
            name: None,
            sketch: p,
            distance: depth.unwrap_or(0.0),
            op: ExtrudeOp::Cut,
            target: Some(BodyId(1)),
            through_all: depth.is_none(),
            reverse: false,
            body_name: None,
            region: None,
            options: Default::default(),
        });
        // Cut 4 used to depend on Truck's run-to-run topology order (refilled pocket or the
        // right solid). With order-stable topology walks in the truck-shapeops fork Truck
        // refills the pocket in every process, and the check refuses it with the model
        // unchanged (`cross_process_determinism.rs` covers the whole corpus).
        if i == 4 {
            assert!(
                matches!(r.error, Some(rcad_document::DocError::GeometryCheck { ref detail, .. })
                    if detail.contains("beside a face of the target")),
                "{:?}",
                r.error
            );
            assert_eq!(s.summary().revision, rev);
            break;
        }
        assert!(r.ok, "cut {i}: {:?}", r.error);
        prims.push(Prim {
            shape: *shape,
            z0: depth.map_or(-1.0, |d| T - d),
            z1: if depth.is_some() { T } else { T + 1.0 },
        });
        let (want, est) = oracle_volume(&prims);
        let v = vol(&s);
        let model = s.model().unwrap();
        let body = model
            .evaluation()
            .body_by_id(BodyId(1))
            .unwrap()
            .geometry
            .brep()
            .unwrap();
        let mut pts = Vec::new();
        for a in 0..24 {
            for b in 0..24 {
                for z in [0.5, 6.5, 9.0, 11.5] {
                    pts.push([42.0 + a as f64 * 0.6, 7.0 + b as f64 * 0.6, z]);
                }
            }
        }
        let got = rcad_kernel::point_membership(body, &pts).unwrap();
        let bad: Vec<_> = pts
            .iter()
            .zip(&got)
            .filter_map(|(p, g)| match (oracle_point(&prims, *p, 0.05), g) {
                (Some(o), Some(g)) if o != *g => Some((*p, o)),
                _ => None,
            })
            .collect();
        assert!(bad.is_empty(), "cut {i}: probe mismatches {bad:?}");
        assert!(
            (v - want).abs() <= 1e-6 * want + 10.0 * est,
            "cut {i}: {v} vs {want}"
        );
    }
}
