//! Seeded corpus of round and rounded cuts (run 5, ADR 0007): through and
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
use rcad_commands::{Command, ExtrudeOp, Session, SketchTemplate};
use rcad_document::{BodyId, PlaneBase, SketchPlane};
use rcad_kernel::VolumeResult;
use rcad_sketch::{Constraint, Dimension, EntityId, PointId, Sketch};

const L: f64 = 80.0;
const W: f64 = 60.0;
const T: f64 = 12.0;

/// xorshift64*, deterministic.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.next()
    }
}

fn snap(v: f64, g: f64) -> f64 {
    (v / g).round() * g
}

#[derive(Clone, Copy, Debug)]
enum Shape {
    Circle {
        c: [f64; 2],
        r: f64,
    },
    /// Rectangle [x0, x1] x [y0, y1] with corner radius r (0: sharp).
    Rounded {
        lo: [f64; 2],
        hi: [f64; 2],
        r: f64,
    },
}

impl Shape {
    /// Signed distance (negative inside).
    fn sdf(&self, p: [f64; 2]) -> f64 {
        match *self {
            Shape::Circle { c, r } => (p[0] - c[0]).hypot(p[1] - c[1]) - r,
            Shape::Rounded { lo, hi, r } => {
                let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
                let q = [
                    (p[0] - c[0]).abs() - ((hi[0] - lo[0]) / 2.0 - r),
                    (p[1] - c[1]).abs() - ((hi[1] - lo[1]) / 2.0 - r),
                ];
                q[0].max(0.0).hypot(q[1].max(0.0)) + q[0].max(q[1]).min(0.0) - r
            }
        }
    }
    /// The x interval covered at height y, if any.
    fn chord(&self, y: f64) -> Option<(f64, f64)> {
        match *self {
            Shape::Circle { c, r } => {
                let d = y - c[1];
                (d.abs() < r).then(|| {
                    let h = (r * r - d * d).sqrt();
                    (c[0] - h, c[0] + h)
                })
            }
            Shape::Rounded { lo, hi, r } => {
                if y <= lo[1] || y >= hi[1] {
                    return None;
                }
                let d = if y < lo[1] + r {
                    lo[1] + r - y
                } else if y > hi[1] - r {
                    y - (hi[1] - r)
                } else {
                    0.0
                };
                let inset = if d > 0.0 {
                    r - (r * r - d * d).sqrt()
                } else {
                    0.0
                };
                Some((lo[0] + inset, hi[0] - inset))
            }
        }
    }
}

/// A cut prism: shape over z in [z0, z1].
#[derive(Clone, Copy, Debug)]
struct Prim {
    shape: Shape,
    z0: f64,
    z1: f64,
}

/// Oracle membership: `Some(material)`, None within `eps` of a boundary.
fn oracle_point(prims: &[Prim], p: [f64; 3], eps: f64) -> Option<bool> {
    let box_d = [p[0], L - p[0], p[1], W - p[1], p[2], T - p[2]]
        .into_iter()
        .fold(f64::INFINITY, f64::min);
    if box_d.abs() < eps {
        return None;
    }
    if box_d < 0.0 {
        return Some(false);
    }
    let mut removed = false;
    for q in prims {
        let s = q.shape.sdf([p[0], p[1]]);
        let dz = (p[2] - q.z0).abs().min((p[2] - q.z1).abs());
        let in_z = p[2] > q.z0 && p[2] < q.z1;
        if (s.abs() < eps && (in_z || dz < eps)) || (dz < eps && s < eps) {
            return None;
        }
        removed |= s < 0.0 && in_z;
    }
    Some(!removed)
}

/// Removed chord length at height y among `shapes` (union, clipped to the plate).
fn removed_len(shapes: &[Shape], y: f64, buf: &mut Vec<(f64, f64)>) -> f64 {
    buf.clear();
    buf.extend(shapes.iter().filter_map(|s| s.chord(y)));
    buf.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (mut total, mut cur): (f64, Option<(f64, f64)>) = (0.0, None);
    for &(a, b) in buf.iter() {
        let (a, b) = (a.max(0.0), b.min(L));
        if b <= a {
            continue;
        }
        cur = match cur {
            Some((ca, cb)) if a <= cb => Some((ca, cb.max(b))),
            Some((ca, cb)) => {
                total += cb - ca;
                Some((a, b))
            }
            None => Some((a, b)),
        };
    }
    if let Some((a, b)) = cur {
        total += b - a;
    }
    total
}

/// Oracle volume and its error estimate.
fn oracle_volume(prims: &[Prim]) -> (f64, f64) {
    let mut zs: Vec<f64> = prims.iter().flat_map(|p| [p.z0, p.z1]).collect();
    zs.extend([0.0, T]);
    zs.retain(|z| (0.0..=T).contains(z));
    zs.sort_by(f64::total_cmp);
    zs.dedup_by(|b, a| (*b - *a).abs() < 1e-12);
    let area = |shapes: &[Shape], n: usize| -> f64 {
        let h = W / n as f64;
        let mut buf = Vec::new();
        (0..n)
            .map(|i| removed_len(shapes, (i as f64 + 0.5) * h, &mut buf))
            .sum::<f64>()
            * h
    };
    const N: usize = 30_000;
    let (mut v, mut err) = (L * W * T, 0.0);
    for w in zs.windows(2) {
        let mid = 0.5 * (w[0] + w[1]);
        let shapes: Vec<Shape> = prims
            .iter()
            .filter(|p| p.z0 < mid && p.z1 > mid)
            .map(|p| p.shape)
            .collect();
        if shapes.is_empty() {
            continue;
        }
        let (a1, a2) = (area(&shapes, N), area(&shapes, 2 * N));
        // Midpoint error O(h^1.5) at circle tops: Richardson with p = 1.5.
        let k = 2f64.powf(1.5);
        let a = (k * a2 - a1) / (k - 1.0);
        v -= a * (w[1] - w[0]);
        err += (a2 - a1).abs() * (w[1] - w[0]);
    }
    (v, err)
}

fn vol(s: &Session) -> f64 {
    let m = s.summary().bodies[0].measurements.clone().unwrap();
    assert_eq!((m.shell_count, m.all_shells_closed), (1, true));
    match m.volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

/// Rounded rectangle sketch (fully constrained, sketch fillets).
fn rounded_sketch(lo: [f64; 2], hi: [f64; 2], r: f64) -> Sketch {
    let mut s = Sketch::default();
    let o = s.add_point(0.0, 0.0);
    s.origin = Some(o);
    let p: Vec<PointId> = [
        [lo[0], lo[1]],
        [hi[0], lo[1]],
        [hi[0], hi[1]],
        [lo[0], hi[1]],
    ]
    .iter()
    .map(|q| s.add_point(q[0], q[1]))
    .collect();
    let l: Vec<EntityId> = (0..4).map(|i| s.add_line(p[i], p[(i + 1) % 4])).collect();
    s.constraints.extend([
        Constraint::FixedPoint {
            p: o,
            x: 0.0,
            y: 0.0,
        },
        Constraint::FixedPoint {
            p: p[0],
            x: lo[0],
            y: lo[1],
        },
        Constraint::Horizontal { line: l[0] },
        Constraint::Horizontal { line: l[2] },
        Constraint::Vertical { line: l[1] },
        Constraint::Vertical { line: l[3] },
        Constraint::HorizontalDistance {
            a: p[0],
            b: p[1],
            dim: Dimension::new("w", hi[0] - lo[0]),
        },
        Constraint::VerticalDistance {
            a: p[1],
            b: p[2],
            dim: Dimension::new("h", hi[1] - lo[1]),
        },
    ]);
    assert!(s.solve().is_success());
    if r > 0.0 {
        s.fillet_corner(p[0], r, Some("r")).unwrap();
        for c in &p[1..] {
            s.fillet_corner(*c, r, None).unwrap();
        }
    }
    s
}

fn template(shape: &Shape) -> SketchTemplate {
    match *shape {
        Shape::Circle { c, r } => SketchTemplate::CenteredCircle {
            diameter: 2.0 * r,
            center: Some(c),
        },
        Shape::Rounded { lo, hi, r: 0.0 } => SketchTemplate::CenteredRectangle {
            width: hi[0] - lo[0],
            height: hi[1] - lo[1],
            center: Some([(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]),
        },
        Shape::Rounded { lo, hi, r } => SketchTemplate::Custom {
            sketch: rounded_sketch(lo, hi, r),
        },
    }
}

/// One test per case, so the test runner spreads the corpus over its workers. The
/// cases are independent: feature draws never depend on outcomes, so case k alone
/// replays exactly the features it has in the whole run.
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
    let mut rng = Rng(0x0a2c_5eed_0000_0005);
    let (mut accepted, mut probes, mut skipped) = (0usize, 0usize, 0usize);
    let mut refusals: Vec<String> = Vec::new();
    let t_start = std::time::Instant::now();
    for case in 0..24 {
        // Probe points come from their own stream, so the feature sequence
        // does not depend on outcomes (a single case can be replayed).
        let mut prng = Rng(0x9e37_79b9_7f4a_7c15 ^ (case as u64 + 1));
        let skip = only.is_some_and(|o| o != case);
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
        s.execute(&Command::extrude(None, sk, T)).feature.unwrap();
        let top = SketchPlane::Offset {
            base: PlaneBase::WorldXY,
            distance: T,
            angle: 0.0,
        };
        let mut prims: Vec<Prim> = Vec::new();
        let mut centres: Vec<[f64; 2]> = Vec::new();
        for n in 0..8 {
            let kind = (rng.next() * 4.0) as u32;
            let through = kind == 0;
            let depth = snap(rng.range(1.0, 10.0), 1.0);
            // Centre: concentric with, offset from, or apart from earlier ones.
            let pick = rng.next();
            let mut c = [
                snap(rng.range(12.0, 68.0), 0.5),
                snap(rng.range(12.0, 48.0), 0.5),
            ];
            if !centres.is_empty() && pick < 0.6 {
                let base = centres[(rng.next() * centres.len() as f64) as usize % centres.len()];
                c = base;
                if pick >= 0.35 {
                    c = [
                        base[0] + snap(rng.range(-6.0, 6.0), 0.5),
                        base[1] + snap(rng.range(-6.0, 6.0), 0.5),
                    ];
                }
            }
            let shape = if kind <= 1 {
                let r = snap(rng.range(1.5, 9.0), 0.5);
                Shape::Circle { c, r }
            } else {
                let (hw, hh) = (
                    snap(rng.range(3.0, 12.0), 1.0),
                    snap(rng.range(3.0, 12.0), 1.0),
                );
                let r = if kind == 2 {
                    snap(rng.range(1.0, 3.0), 1.0).min(hw.min(hh) - 0.5)
                } else {
                    0.0
                };
                Shape::Rounded {
                    lo: [c[0] - hw, c[1] - hh],
                    hi: [c[0] + hw, c[1] + hh],
                    r,
                }
            };
            // Keep every tool inside the plate (flush cuts: gate_fuzz.rs).
            let inside = match shape {
                Shape::Circle { c, r } => {
                    c[0] - r > 1.0 && c[0] + r < L - 1.0 && c[1] - r > 1.0 && c[1] + r < W - 1.0
                }
                Shape::Rounded { lo, hi, .. } => {
                    lo[0] > 1.0 && hi[0] < L - 1.0 && lo[1] > 1.0 && hi[1] < W - 1.0
                }
            };
            if !inside || skip {
                continue;
            }
            centres.push(c);
            let prim = Prim {
                shape,
                z0: if through { -1.0 } else { T - depth },
                z1: if through { T + 1.0 } else { T },
            };
            let rev = s.summary().revision;
            let before = vol(&s);
            let p = s
                .execute(&Command::CreateSketch {
                    name: None,
                    plane: top,
                    template: template(&shape),
                })
                .feature
                .expect("sketch");
            let r = s.execute(&Command::Extrude {
                name: None,
                sketch: p,
                distance: if through { 0.0 } else { depth },
                op: ExtrudeOp::Cut,
                target: Some(BodyId(1)),
                through_all: through,
                reverse: false,
                body_name: None,
                region: None,
            });
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

/// Run-5 arc corpus finding (an earlier, outcome-dependent version of the
/// stream above, case 21 #4): a Ø11 blind cut externally tangent to the
/// straight wall of an earlier 1 mm deep rounded pocket. Truck returned a
/// closed solid in which that pocket was filled with material again (72.27
/// mm³ too much) and the result check accepted it: it probed result faces
/// only, and a refilled void leaves none. The check now also probes the
/// target's faces near the tool (ADR 0005 addendum): refused, unchanged. The
/// slab construction refuses too (a tangent contact elsewhere in the body).
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
        });
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
