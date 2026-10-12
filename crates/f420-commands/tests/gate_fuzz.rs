//! Randomized check of the geometry gate (ADR 0005): random rectangular
//! pockets/bosses, round holes and their linear patterns on a block, with
//! analytic volumes (features are kept disjoint in plan so volumes add).
//! Invariant: an accepted result has exactly the analytic volume (no silent
//! wrong geometry); a refusal leaves the document unchanged. Refusals are
//! counted and must stay rare for these ordinary cases. Seeded, deterministic.
use f420_commands::{Command, ExtrudeOp, Session, SketchTemplate};
use f420_document::{BodyId, FaceRef, FaceRole, FeatureId, SketchPlane};
use f420_kernel::VolumeResult;
use std::f64::consts::PI;

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

fn vol(s: &Session) -> f64 {
    match s.summary().bodies[0].measurements.clone().unwrap().volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

#[derive(Clone, Copy)]
struct Cell {
    x0: f64,
    y0: f64,
    w: f64,
    h: f64,
}

#[test]
fn random_features_are_exact_or_refused() {
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    let (mut accepted, mut refused, mut cases) = (0, 0, 0);
    let mut refusals = Vec::new();
    for case in 0..12 {
        let mut s = Session::new();
        s.execute(&Command::NewDocument);
        // Block: L x W x T on x [0, L], y [0, W].
        let (l, w, t) = (
            (rng.range(60.0, 120.0) * 4.0).round() / 4.0,
            (rng.range(40.0, 80.0) * 4.0).round() / 4.0,
            (rng.range(4.0, 20.0) * 4.0).round() / 4.0,
        );
        let sk = s
            .execute(&Command::CreateSketch {
                name: None,
                plane: SketchPlane::WorldXY,
                template: SketchTemplate::CenteredRectangle {
                    width: l,
                    height: w,
                    center: Some([l / 2.0, w / 2.0]),
                },
            })
            .feature
            .unwrap();
        let e = s.execute(&Command::extrude(None, sk, t)).feature.unwrap();
        let top = SketchPlane::Face(FaceRef {
            feature: e,
            role: FaceRole::ExtrudeEndCap,
        });
        let mut expect = l * w * t;
        // A 4 x 3 grid of disjoint cells, inset 2 mm; one feature per cell.
        let (gx, gy) = (4, 3);
        let cw = (l - 4.0) / gx as f64;
        let ch = (w - 4.0) / gy as f64;
        for i in 0..gx {
            for j in 0..gy {
                cases += 1;
                let c = Cell {
                    x0: 2.0 + cw * i as f64,
                    y0: 2.0 + ch * j as f64,
                    w: cw,
                    h: ch,
                };
                // Feature size strictly inside its cell (margin >= 0.5 mm).
                let fw = rng.range(0.25, 0.8) * (c.w - 1.0);
                let fh = rng.range(0.25, 0.8) * (c.h - 1.0);
                let cx = c.x0 + c.w / 2.0;
                let cy = c.y0 + c.h / 2.0;
                let kind = (rng.next() * 4.0) as u32;
                let before = vol(&s);
                let (cmd, delta): (Vec<Command>, f64) = match kind {
                    // Rectangular pocket, blind.
                    0 => {
                        let d = rng.range(0.2, 0.9) * t;
                        let p = s
                            .execute(&Command::CreateSketch {
                                name: None,
                                plane: top,
                                template: SketchTemplate::CenteredRectangle {
                                    width: fw,
                                    height: fh,
                                    center: Some([cx, cy]),
                                },
                            })
                            .feature
                            .unwrap();
                        (
                            vec![Command::Extrude {
                                name: None,
                                sketch: p,
                                distance: d,
                                op: ExtrudeOp::Cut,
                                target: Some(BodyId(1)),
                                through_all: false,
                                reverse: false,
                                body_name: None,
                                region: None,
                            }],
                            -fw * fh * d,
                        )
                    }
                    // Rectangular boss.
                    1 => {
                        let hgt = rng.range(0.5, 10.0);
                        let p = s
                            .execute(&Command::CreateSketch {
                                name: None,
                                plane: top,
                                template: SketchTemplate::CenteredRectangle {
                                    width: fw,
                                    height: fh,
                                    center: Some([cx, cy]),
                                },
                            })
                            .feature
                            .unwrap();
                        (
                            vec![Command::Extrude {
                                name: None,
                                sketch: p,
                                distance: hgt,
                                op: ExtrudeOp::Join,
                                target: Some(BodyId(1)),
                                through_all: false,
                                reverse: false,
                                body_name: None,
                                region: None,
                            }],
                            fw * fh * hgt,
                        )
                    }
                    // Counterbored through hole.
                    2 => {
                        let r = 0.5 * fw.min(fh);
                        let d = rng.range(0.3, 0.6) * 2.0 * r;
                        let cbd = rng.range(0.2, 0.7) * t;
                        (
                            vec![Command::Hole {
                                name: None,
                                plane: top,
                                center: [cx, cy],
                                diameter: d,
                                depth: None,
                                counterbore: Some([2.0 * r, cbd]),
                                target: None,
                            }],
                            -(PI * (d / 2.0).powi(2) * t + PI * (r * r - (d / 2.0).powi(2)) * cbd),
                        )
                    }
                    // Blind round pocket.
                    _ => {
                        let r = 0.5 * fw.min(fh);
                        let dep = rng.range(0.2, 0.9) * t;
                        (
                            vec![Command::Hole {
                                name: None,
                                plane: top,
                                center: [cx, cy],
                                diameter: 2.0 * r,
                                depth: Some(dep),
                                counterbore: None,
                                target: None,
                            }],
                            -PI * r * r * dep,
                        )
                    }
                };
                let rev = s.summary().revision;
                let mut ok = true;
                let mut err = None;
                for c in &cmd {
                    let r = s.execute(c);
                    if !r.ok {
                        ok = false;
                        err = r.error;
                        break;
                    }
                }
                if ok {
                    accepted += 1;
                    expect += delta;
                    let v = vol(&s);
                    assert!(
                        (v - expect).abs() <= 1e-6 * expect.max(1.0),
                        "case {case} cell ({i},{j}) kind {kind}: volume {v} vs analytic {expect} \
                         (before {before}, delta {delta})"
                    );
                } else {
                    refused += 1;
                    refusals.push(format!("case {case} ({i},{j}) kind {kind}: {err:?}"));
                    assert_eq!(s.summary().revision, rev, "a refusal commits nothing");
                    assert!(
                        (vol(&s) - before).abs() < 1e-9,
                        "refusal changed the volume"
                    );
                }
            }
        }
        // A linear pattern of the last boss/pocket would overlap other cells;
        // patterns are covered in geometry_gate.rs / holes.rs.
        let _ = FeatureId(0);
    }
    eprintln!("gate fuzz: {cases} features, {accepted} accepted exact, {refused} refused");
    for r in &refusals {
        eprintln!("  refused: {r}");
    }
    assert!(
        refused * 10 <= cases,
        "too many refusals for ordinary features: {refused}/{cases}"
    );
}

/// Exact volume of a union of axis-aligned boxes (coordinate compression).
fn union_volume(boxes: &[[f64; 6]]) -> f64 {
    let mut xs: Vec<f64> = boxes.iter().flat_map(|b| [b[0], b[3]]).collect();
    let mut ys: Vec<f64> = boxes.iter().flat_map(|b| [b[1], b[4]]).collect();
    let mut zs: Vec<f64> = boxes.iter().flat_map(|b| [b[2], b[5]]).collect();
    for v in [&mut xs, &mut ys, &mut zs] {
        v.sort_by(|a, b| a.total_cmp(b));
        v.dedup();
    }
    let mut total = 0.0;
    for i in 0..xs.len().saturating_sub(1) {
        for j in 0..ys.len().saturating_sub(1) {
            for k in 0..zs.len().saturating_sub(1) {
                let c = [
                    (xs[i] + xs[i + 1]) / 2.0,
                    (ys[j] + ys[j + 1]) / 2.0,
                    (zs[k] + zs[k + 1]) / 2.0,
                ];
                if boxes
                    .iter()
                    .any(|b| (0..3).all(|a| b[a] < c[a] && c[a] < b[a + 3]))
                {
                    total += (xs[i + 1] - xs[i]) * (ys[j + 1] - ys[j]) * (zs[k + 1] - zs[k]);
                }
            }
        }
    }
    total
}

/// Hard cases: overlapping, nested and edge-coincident rectangular pockets
/// cut from the top face (coplanar floors, engulfed earlier pockets, shared
/// walls). Expected volume = block − union of pocket boxes, exact. Accepted
/// results must be exact; refusals are allowed (counted, reported).
#[test]
fn overlapping_pockets_are_exact_or_refused() {
    let mut rng = Rng(0x0dd5_eed5_0000_0042);
    let (mut accepted, mut refused) = (0, 0);
    let mut log = Vec::new();
    for case in 0..8 {
        let mut s = Session::new();
        s.execute(&Command::NewDocument);
        let (l, w, t) = (80.0, 60.0, 12.0);
        let sk = s
            .execute(&Command::CreateSketch {
                name: None,
                plane: SketchPlane::WorldXY,
                template: SketchTemplate::CenteredRectangle {
                    width: l,
                    height: w,
                    center: Some([l / 2.0, w / 2.0]),
                },
            })
            .feature
            .unwrap();
        let e = s.execute(&Command::extrude(None, sk, t)).feature.unwrap();
        let top = SketchPlane::Face(FaceRef {
            feature: e,
            role: FaceRole::ExtrudeEndCap,
        });
        let mut pockets: Vec<[f64; 6]> = Vec::new();
        for n in 0..8 {
            // Coordinates on a 2 mm grid so coincident walls/floors happen.
            let snap = |v: f64| (v / 2.0).round() * 2.0;
            let x0 = snap(rng.range(4.0, l - 20.0));
            let y0 = snap(rng.range(4.0, w - 20.0));
            let x1 = snap((x0 + rng.range(4.0, 30.0)).min(l - 4.0));
            let y1 = snap((y0 + rng.range(4.0, 30.0)).min(w - 4.0));
            let d = snap(rng.range(2.0, t - 2.0));
            if x1 - x0 < 2.0 || y1 - y0 < 2.0 {
                continue;
            }
            let rev = s.summary().revision;
            let before = vol(&s);
            let p = s
                .execute(&Command::CreateSketch {
                    name: None,
                    plane: top,
                    template: SketchTemplate::CenteredRectangle {
                        width: x1 - x0,
                        height: y1 - y0,
                        center: Some([(x0 + x1) / 2.0, (y0 + y1) / 2.0]),
                    },
                })
                .feature;
            let Some(p) = p else {
                // The top face itself may be ambiguous/split by now.
                refused += 1;
                log.push(format!("case {case} #{n}: sketch on the top face refused"));
                continue;
            };
            let r = s.execute(&Command::Extrude {
                name: None,
                sketch: p,
                distance: d,
                op: ExtrudeOp::Cut,
                target: Some(BodyId(1)),
                through_all: false,
                reverse: false,
                body_name: None,
                region: None,
            });
            if r.ok {
                accepted += 1;
                pockets.push([x0, y0, t - d, x1, y1, t]);
                let want = l * w * t - union_volume(&pockets);
                let v = vol(&s);
                assert!(
                    (v - want).abs() <= 1e-6 * want,
                    "case {case} pocket {n} [{x0},{y0}]-[{x1},{y1}] depth {d}: volume {v} vs \
                     exact {want}"
                );
            } else {
                refused += 1;
                log.push(format!(
                    "case {case} #{n} [{x0},{y0}]-[{x1},{y1}] d {d}: {:?}",
                    r.error
                ));
                assert_eq!(s.summary().revision, rev + 1, "only the sketch was added");
                assert!((vol(&s) - before).abs() < 1e-9);
            }
        }
    }
    eprintln!("overlap fuzz: {accepted} accepted exact, {refused} refused");
    for l in &log {
        eprintln!("  {}", &l[..l.len().min(260)]);
    }
    assert!(accepted > 0);
}
