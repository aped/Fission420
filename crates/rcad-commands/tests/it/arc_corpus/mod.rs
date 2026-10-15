//! The seeded arc corpus shared by the oracle test (`arc_fuzz.rs`) and the
//! cross-process determinism test: round and rounded cuts on an 80 x 60 x 12
//! plate, the nominal CSG oracle, and the sketch templates.
#![allow(dead_code)]
use rcad_commands::{Session, SketchTemplate};
use rcad_kernel::VolumeResult;
use rcad_sketch::{Constraint, Dimension, EntityId, PointId, Sketch};

pub const L: f64 = 80.0;
pub const W: f64 = 60.0;
pub const T: f64 = 12.0;

/// xorshift64*, deterministic.
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.next()
    }
}

pub fn snap(v: f64, g: f64) -> f64 {
    (v / g).round() * g
}

#[derive(Clone, Copy, Debug)]
pub enum Shape {
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
    pub fn sdf(&self, p: [f64; 2]) -> f64 {
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
    pub fn chord(&self, y: f64) -> Option<(f64, f64)> {
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
pub struct Prim {
    pub shape: Shape,
    pub z0: f64,
    pub z1: f64,
}

/// Oracle membership: `Some(material)`, None within `eps` of a boundary.
pub fn oracle_point(prims: &[Prim], p: [f64; 3], eps: f64) -> Option<bool> {
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
pub fn removed_len(shapes: &[Shape], y: f64, buf: &mut Vec<(f64, f64)>) -> f64 {
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
pub fn oracle_volume(prims: &[Prim]) -> (f64, f64) {
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

pub fn vol(s: &Session) -> f64 {
    let m = s.summary().bodies[0].measurements.clone().unwrap();
    assert_eq!((m.shell_count, m.all_shells_closed), (1, true));
    match m.volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

/// Rounded rectangle sketch (fully constrained, sketch fillets).
pub fn rounded_sketch(lo: [f64; 2], hi: [f64; 2], r: f64) -> Sketch {
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

pub fn template(shape: &Shape) -> SketchTemplate {
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

/// One drawn cut of the corpus.
#[derive(Clone, Copy, Debug)]
pub struct Feature {
    pub case: usize,
    pub n: usize,
    pub shape: Shape,
    pub through: bool,
    pub depth: f64,
    pub prim: Prim,
}

/// Number of cases (independent documents) in the corpus.
pub const CASES: usize = 24;

/// The corpus features in order, for all cases or only case `only`. Feature
/// draws never depend on kernel outcomes. A case whose features are not
/// requested still consumes its draws, but its centres are not recorded, which
/// is how the per-case tests have always replayed a single case.
pub fn features(only: Option<usize>) -> Vec<Feature> {
    let mut rng = Rng(0x0a2c_5eed_0000_0005);
    let mut out = Vec::new();
    for case in 0..CASES {
        let skip = only.is_some_and(|o| o != case);
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
            out.push(Feature {
                case,
                n,
                shape,
                through,
                depth,
                prim: Prim {
                    shape,
                    z0: if through { -1.0 } else { T - depth },
                    z1: if through { T + 1.0 } else { T },
                },
            });
        }
    }
    out
}

/// A new document holding the plate, and the plane of its top face.
pub fn plate() -> (Session, rcad_document::SketchPlane) {
    use rcad_commands::Command;
    let mut s = Session::new();
    s.execute(&Command::NewDocument);
    let sk = s
        .execute(&Command::CreateSketch {
            name: None,
            plane: rcad_document::SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: L,
                height: W,
                center: Some([L / 2.0, W / 2.0]),
            },
        })
        .feature
        .unwrap();
    s.execute(&Command::extrude(None, sk, T)).feature.unwrap();
    let top = rcad_document::SketchPlane::Offset {
        base: rcad_document::PlaneBase::WorldXY,
        distance: T,
        angle: 0.0,
    };
    (s, top)
}

/// Sketch the feature's profile on `top` and cut it into body 1.
pub fn cut(
    s: &mut Session,
    top: rcad_document::SketchPlane,
    f: &Feature,
) -> rcad_commands::Response {
    use rcad_commands::{Command, ExtrudeOp};
    let p = s
        .execute(&Command::CreateSketch {
            name: None,
            plane: top,
            template: template(&f.shape),
        })
        .feature
        .expect("sketch");
    s.execute(&Command::Extrude {
        name: None,
        sketch: p,
        distance: if f.through { 0.0 } else { f.depth },
        op: ExtrudeOp::Cut,
        target: Some(rcad_document::BodyId(1)),
        through_all: f.through,
        reverse: false,
        body_name: None,
        region: None,
        options: Default::default(),
    })
}
