//! Randomized check of the geometry gate (ADR 0005): random rectangular
//! pockets/bosses, round holes and their linear patterns on a block, with
//! analytic volumes (features are kept disjoint in plan so volumes add).
//! Invariant: an accepted result has exactly the analytic volume (no silent
//! wrong geometry); a refusal leaves the document unchanged. Refusals are
//! counted and must stay rare for these ordinary cases. Seeded, deterministic.
use rcad_commands::{Command, ExtrudeOp, Session, SketchTemplate};
use rcad_document::{BodyId, FaceRef, FaceRole, FeatureId, PlaneBase, SketchPlane};
use rcad_kernel::VolumeResult;
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
                                options: Default::default(),
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
                                options: Default::default(),
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
    corpus_line("disjoint", accepted, &refusals);
    for r in &refusals {
        eprintln!("  refused: {r}");
    }
    assert!(
        refused * 10 <= cases,
        "too many refusals for ordinary features: {refused}/{cases}"
    );
}

/// One machine-readable summary line per corpus (run-5 evidence): accepted
/// results are all exact (an incorrect one fails the test before this line);
/// refusals by kind, from their error text.
fn corpus_line(name: &str, accepted: usize, refusals: &[String]) {
    let n = |k: &str| refusals.iter().filter(|l| l.contains(k)).count();
    let sketch = n("sketch on the top face refused");
    let (kernel, gate, limit) = (n("Kernel {"), n("GeometryCheck {"), n("ProductLimit("));
    let other = refusals.len() - sketch - kernel - gate - limit;
    eprintln!(
        "CORPUS {name}: features={} accepted_exact={accepted} accepted_incorrect=0 \
         refused_kernel={kernel} refused_gate={gate} refused_product_limit={limit} \
         refused_sketch_reference={sketch} refused_other={other}",
        accepted + refusals.len()
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
                options: Default::default(),
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
    corpus_line("overlap", accepted, &log);
    for l in &log {
        eprintln!("  {}", &l[..l.len().min(260)]);
    }
    // Before the exact slab construction (ADR 0006): 43 accepted, 19 refused by
    // Truck, 2 by the result check. Now no cut of this seed is refused; only a
    // sketch whose face reference became ambiguous (top face split in two).
    assert!(accepted >= 63, "{accepted} accepted");
    assert!(
        log.iter()
            .all(|l| l.contains("sketch on the top face refused")),
        "{log:#?}"
    );
}

/// Pockets and bosses on a 2 mm grid that may be flush with the block's outer
/// walls, reach through the whole thickness, coincide with earlier floors and
/// walls, and bosses sketched exactly on the top face (coplanar contact, possibly
/// leaving the face). The expected solid is tracked exactly on a 2 mm occupancy
/// grid. Accepted results must match it; refusals must leave the document
/// unchanged and may only be the legitimate kinds (disconnected or non-manifold
/// result, nothing removed, ambiguous face reference or material side).
#[test]
fn flush_coplanar_pockets_and_bosses_are_exact_or_refused() {
    const G: f64 = 2.0;
    let (nx, ny, nz) = (30usize, 20usize, 12usize); // 60 x 40 x 24 mm grid
    let t = 6usize; // block thickness in cells (12 mm)
    let mut rng = Rng(0x0dd5_eed5_0000_0077);
    let (mut accepted, mut refused) = (0, 0);
    let mut log = Vec::new();
    for case in 0..24 {
        let mut s = Session::new();
        s.execute(&Command::NewDocument);
        let sk = s
            .execute(&Command::CreateSketch {
                name: None,
                plane: SketchPlane::WorldXY,
                template: SketchTemplate::CenteredRectangle {
                    width: nx as f64 * G,
                    height: ny as f64 * G,
                    center: Some([nx as f64 * G / 2.0, ny as f64 * G / 2.0]),
                },
            })
            .feature
            .unwrap();
        let e = s
            .execute(&Command::extrude(None, sk, t as f64 * G))
            .feature
            .unwrap();
        let _ = e;
        // A construction plane at the top height: flush pockets split the top
        // face, which would make a face reference ambiguous (a separate rule).
        let top = SketchPlane::Offset {
            base: PlaneBase::WorldXY,
            distance: t as f64 * G,
            angle: 0.0,
        };
        let mut occ = vec![false; nx * ny * nz];
        let at = |i: usize, j: usize, k: usize| (k * ny + j) * nx + i;
        for i in 0..nx {
            for j in 0..ny {
                for k in 0..t {
                    occ[at(i, j, k)] = true;
                }
            }
        }
        let grid_vol = |o: &[bool]| o.iter().filter(|&&b| b).count() as f64 * G * G * G;
        for n in 0..8 {
            let pick = |rng: &mut Rng, hi: usize| (rng.next() * (hi as f64 + 1.0)) as usize;
            // Corners on the grid, flush with the outer walls (0 or max) often.
            let mut i0 = pick(&mut rng, nx - 2);
            let mut j0 = pick(&mut rng, ny - 2);
            if rng.next() < 0.3 {
                i0 = 0;
            }
            if rng.next() < 0.3 {
                j0 = 0;
            }
            let mut i1 = (i0 + 1 + pick(&mut rng, 12)).min(nx);
            let mut j1 = (j0 + 1 + pick(&mut rng, 10)).min(ny);
            if rng.next() < 0.3 {
                i1 = nx;
            }
            if rng.next() < 0.3 {
                j1 = ny;
            }
            let mut boss = rng.next() < 0.3;
            // Mostly supported bosses: one over air leaves down-facing faces in the
            // top plane, after which sketches there are refused (ambiguous side).
            let supported = (i0..i1).all(|i| (j0..j1).all(|j| occ[at(i, j, t - 1)]));
            if boss && !supported && rng.next() < 0.8 {
                boss = false;
            }
            let d = 1 + pick(&mut rng, if boss { 4 } else { t - 1 });
            let rev = s.summary().revision;
            let before = vol(&s);
            let p = s
                .execute(&Command::CreateSketch {
                    name: None,
                    plane: top,
                    template: SketchTemplate::CenteredRectangle {
                        width: (i1 - i0) as f64 * G,
                        height: (j1 - j0) as f64 * G,
                        center: Some([(i0 + i1) as f64 * G / 2.0, (j0 + j1) as f64 * G / 2.0]),
                    },
                })
                .feature;
            let Some(p) = p else {
                refused += 1;
                log.push(format!("case {case} #{n}: sketch on the top face refused"));
                continue;
            };
            let r = s.execute(&Command::Extrude {
                name: None,
                sketch: p,
                distance: d as f64 * G,
                op: if boss {
                    ExtrudeOp::Join
                } else {
                    ExtrudeOp::Cut
                },
                target: Some(BodyId(1)),
                through_all: false,
                reverse: false,
                body_name: None,
                region: None,
                options: Default::default(),
            });
            let what = format!(
                "case {case} #{n} {} [{i0},{j0}]-[{i1},{j1}] cells, {d} deep",
                if boss { "boss" } else { "pocket" }
            );
            // Direction rule of the document (ADR 0003): when the sketch plane
            // holds faces of the target, a cut goes into the material and a join
            // away from it; with no face there, both go along +Z.
            let (mut up, mut down) = (false, false);
            for i in 0..nx {
                for j in 0..ny {
                    let (b, a) = (occ[at(i, j, t - 1)], occ[at(i, j, t)]);
                    up |= b && !a;
                    down |= a && !b;
                }
            }
            let side = match (up, down) {
                (true, false) => Some(1),
                (false, true) => Some(-1),
                _ => None,
            };
            let goes_up = match side {
                Some(sd) if boss => sd > 0,
                Some(sd) => sd < 0,
                None => true,
            };
            let mut next = occ.clone();
            let ks = if goes_up { t..t + d } else { t - d..t };
            for i in i0..i1 {
                for j in j0..j1 {
                    for k in ks.clone() {
                        next[at(i, j, k)] = boss;
                    }
                }
            }
            if r.ok {
                accepted += 1;
                occ = next;
                let (v, want) = (vol(&s), grid_vol(&occ));
                assert!(
                    (v - want).abs() <= 1e-6 * want,
                    "{what}: volume {v} vs exact {want}"
                );
            } else {
                refused += 1;
                let m = format!("{:?}", r.error);
                // Every refusal must be justified by the exact expected solid.
                let justified = if m.contains("disconnected") || m.contains("manifold") {
                    !grid_is_single_manifold_solid(&next, [nx, ny, nz])
                } else if m.contains("removes no material") || m.contains("adds no material") {
                    next == occ
                } else if m.contains("facing both ways") {
                    // A boss bridged over a through pocket leaves down-facing faces
                    // in the top plane: the material side of a sketch there is
                    // ambiguous (document rule).
                    up && down
                } else {
                    false
                };
                if !justified && let Some(dir) = std::env::var_os("RCAD_FUZZ_KEEP") {
                    // Keep the document (with the refused feature's sketch) for a repro.
                    let path =
                        std::path::PathBuf::from(dir).join(format!("case{case}_{n}.rcad.json"));
                    s.execute(&Command::Save { path });
                }
                assert!(justified, "{what}: unjustified refusal {m}");
                // Head (what failed) and tail (the slab construction's reason).
                let tail = m.len().saturating_sub(160).max(m.len().min(160));
                log.push(format!(
                    "{what}: {} .. {}",
                    &m[..m.len().min(160)],
                    &m[tail..]
                ));
                assert_eq!(s.summary().revision, rev + 1, "only the sketch was added");
                assert!((vol(&s) - before).abs() < 1e-9);
            }
        }
    }
    let kinds = |k: &str| log.iter().filter(|l| l.contains(k)).count();
    eprintln!(
        "flush/coplanar fuzz: {accepted} accepted exact, {refused} refused (all justified \
         by the exact grid): {} ambiguous side, {} disconnected, {} non-manifold, {} no-op",
        kinds("facing both ways"),
        kinds("disconnected"),
        kinds("not be a manifold"),
        kinds("no material")
    );
    for l in &log {
        eprintln!("  {l}");
    }
    corpus_line("flush_coplanar", accepted, &log);
    assert!(accepted >= 120, "{accepted} accepted");
}

/// Whether the occupied cells of a `dims` grid (x fastest) form one connected
/// (through faces) solid with a manifold boundary: no two cells meeting only
/// along an edge or at a corner without a face path around it.
fn grid_is_single_manifold_solid(occ: &[bool], dims: [usize; 3]) -> bool {
    let [nx, ny, nz] = dims;
    let get = |i: isize, j: isize, k: isize| {
        i >= 0
            && j >= 0
            && k >= 0
            && (i as usize) < nx
            && (j as usize) < ny
            && (k as usize) < nz
            && occ[((k as usize) * ny + j as usize) * nx + i as usize]
    };
    // Face-connected components.
    let mut seen = vec![false; occ.len()];
    let mut comps = 0;
    for start in 0..occ.len() {
        if !occ[start] || seen[start] {
            continue;
        }
        comps += 1;
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(c) = stack.pop() {
            let (i, j, k) = (c % nx, c / nx % ny, c / (nx * ny));
            for (di, dj, dk) in [
                (1, 0, 0),
                (-1, 0, 0),
                (0, 1, 0),
                (0, -1, 0),
                (0, 0, 1),
                (0, 0, -1),
            ] {
                let (a, b, e) = (i as isize + di, j as isize + dj, k as isize + dk);
                if get(a, b, e) {
                    let n = ((e as usize) * ny + b as usize) * nx + a as usize;
                    if !seen[n] {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
    }
    if comps != 1 {
        return false;
    }
    // Around every grid vertex, the occupied and the empty cells of its 2x2x2
    // neighbourhood must each be face-connected within it (covers both edge
    // and corner contacts).
    for k in 0..=nz as isize {
        for j in 0..=ny as isize {
            for i in 0..=nx as isize {
                let cell = |m: usize| {
                    get(
                        i - 1 + (m & 1) as isize,
                        j - 1 + (m >> 1 & 1) as isize,
                        k - 1 + (m >> 2 & 1) as isize,
                    )
                };
                for want in [true, false] {
                    let set: Vec<usize> = (0..8).filter(|&m| cell(m) == want).collect();
                    if set.len() <= 1 || set.len() == 8 {
                        continue;
                    }
                    let mut reach = vec![set[0]];
                    let mut idx = 0;
                    while idx < reach.len() {
                        let m = reach[idx];
                        idx += 1;
                        for bit in [1, 2, 4] {
                            let n = m ^ bit;
                            if cell(n) == want && !reach.contains(&n) {
                                reach.push(n);
                            }
                        }
                    }
                    if reach.len() != set.len() {
                        return false;
                    }
                }
            }
        }
    }
    true
}
