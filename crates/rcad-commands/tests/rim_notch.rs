//! Rim notch through a tray's front wall (SESSION_5 §4, §7): a rectangle
//! sketched on the outer side face, flush with the rim, cut exactly the wall
//! thickness, so the notch floor lies in the cavity's wall plane. Truck 0.6
//! panics on it; the exact slab construction (ADR 0006) builds it. Coordinates
//! from derived dimensions (1/3 mm compartments) carry float noise that once
//! left a 1e-9 mm spike in a face loop ("NotSimpleWire", coordinator repro
//! `artifacts/run5/kernel_repro_rim_notch.json`). Expected values are the
//! tray formula of SESSION_5 §7, independent of the model builder.
use rcad_commands::{Command, ExtrudeOp, Session, SketchTemplate};
use rcad_document::{BodyId, FaceRef, FaceRole, FeatureId, SketchPlane};
use rcad_kernel::VolumeResult;

fn ok(s: &mut Session, c: Command) -> FeatureId {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r.feature.unwrap_or(FeatureId(0))
}

fn rect_sketch(s: &mut Session, plane: SketchPlane, w: f64, h: f64, c: [f64; 2]) -> FeatureId {
    ok(
        s,
        Command::CreateSketch {
            name: None,
            plane,
            template: SketchTemplate::CenteredRectangle {
                width: w,
                height: h,
                center: Some(c),
            },
        },
    )
}

fn cut(sketch: FeatureId, d: f64) -> Command {
    Command::Extrude {
        name: None,
        sketch,
        distance: d,
        op: ExtrudeOp::Cut,
        target: Some(BodyId(1)),
        through_all: false,
        reverse: false,
        body_name: None,
        region: None,
    }
}

struct Tray {
    l: f64,
    w: f64,
    h: f64,
    wall: f64,
    floor: f64,
    div: f64,
    n: u32,
    notch_w: f64,
    notch_d: f64,
    /// Plane of the blank's sketch and the tray's corner in its coordinates
    /// (translated / rotated placements, SESSION_5 §4).
    plane: SketchPlane,
    corner: [f64; 2],
    /// Coordinator repro variant: the notch and compartment sketches are
    /// created at rounded centres and then bound by expressions, so their
    /// solved points carry solver noise (~1e-13 mm) instead of exact values.
    rebind: bool,
    /// Added to the notch's centre x, y, width and depth (sub-grid noise).
    jitter: [f64; 4],
}

impl Tray {
    fn cw(&self) -> f64 {
        (self.l - 2.0 * self.wall - self.n as f64 * self.div) / (self.n as f64 + 1.0)
    }
    /// SESSION_5 §7 formula, minus the notch.
    fn volume(&self) -> f64 {
        let (cx, cy, cz) = (
            self.l - 2.0 * self.wall,
            self.w - 2.0 * self.wall,
            self.h - self.floor,
        );
        self.l * self.w * self.h - cx * cy * cz + self.n as f64 * self.div * cy * cz
            - self.notch_w * self.notch_d * self.wall
    }
    /// Blank, first compartment pocket, linear pattern, then the notch cut
    /// (returned so a caller can check it in isolation).
    fn build(&self, s: &mut Session) -> Command {
        ok(s, Command::NewDocument);
        let [ox, oy] = self.corner;
        let sk = rect_sketch(
            s,
            self.plane,
            self.l,
            self.w,
            [ox + self.l / 2.0, oy + self.w / 2.0],
        );
        let blank = ok(s, Command::extrude(None, sk, self.h));
        let cw = self.cw();
        let top = SketchPlane::Face(FaceRef {
            feature: blank,
            role: FaceRole::ExtrudeEndCap,
        });
        let cx = ox + self.wall + cw / 2.0;
        let c = rect_sketch(
            s,
            top,
            if self.rebind { cw.round() } else { cw },
            self.w - 2.0 * self.wall,
            [if self.rebind { cx.round() } else { cx }, oy + self.w / 2.0],
        );
        // The expression of the derived compartment width, as the tray
        // example writes it.
        let cw_expr = format!(
            "(({}) - 2 * ({}) - ({}) * ({})) / ({})",
            self.l,
            self.wall,
            self.n,
            self.div,
            self.n + 1
        );
        if self.rebind {
            bind(s, c, "width", &cw_expr);
            bind(
                s,
                c,
                "center_x",
                &format!("{ox} + {} + {cw_expr} / 2", self.wall),
            );
        }
        let pocket = ok(s, cut(c, self.h - self.floor));
        ok(
            s,
            Command::LinearPattern {
                name: None,
                feature: pocket,
                count: self.n + 1,
                spacing: cw + self.div,
                direction: [1.0, 0.0],
            },
        );
        // Front side face (the rectangle's first edge, y = corner y): its
        // sketch x runs along the edge from the corner, its y up the wall.
        let front = SketchPlane::Face(FaceRef {
            feature: blank,
            role: FaceRole::ExtrudeSide { entity: 1 },
        });
        let j = self.jitter;
        let (nx, ny) = (
            self.wall + cw / 2.0 + j[0],
            self.h - self.notch_d / 2.0 + j[1],
        );
        let n = rect_sketch(
            s,
            front,
            self.notch_w + j[2],
            self.notch_d + j[3],
            if self.rebind {
                [nx.round(), ny.round()]
            } else {
                [nx, ny]
            },
        );
        if self.rebind {
            bind(s, n, "center_x", &format!("{} + {cw_expr} / 2", self.wall));
            bind(
                s,
                n,
                "center_y",
                &format!("{} - {} / 2", self.h, self.notch_d),
            );
        }
        cut(n, self.wall)
    }
}

fn bind(s: &mut Session, feature: FeatureId, name: &str, expression: &str) {
    ok(
        s,
        Command::SetExpression {
            feature,
            name: name.into(),
            expression: expression.into(),
        },
    );
}

fn check(t: &Tray) {
    let mut s = Session::new();
    let notch = t.build(&mut s);
    let before = s.summary().revision;
    let r = s.execute(&notch);
    assert!(r.ok, "notch refused: {:?}", r.error);
    assert_eq!(s.summary().revision, before + 1);
    let b = s.summary().bodies.into_iter().next().unwrap();
    let m = b.measurements.unwrap();
    assert_eq!((m.shell_count, m.all_shells_closed), (1, true));
    let v = match m.volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    };
    let want = t.volume();
    assert!(
        (v - want).abs() <= 1e-6 * want,
        "volume {v} vs formula {want}"
    );
    // Tray-local (x along the length, y across, z up) to world, through the
    // blank sketch's frame.
    let f = s.summary().features[0].frame.unwrap();
    let world = |p: [f64; 3]| -> [f64; 3] {
        let (u, v) = (t.corner[0] + p[0], t.corner[1] + p[1]);
        [0, 1, 2].map(|i| f.origin[i] + u * f.x[i] + v * f.y[i] + p[2] * f.z[i])
    };
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for c in 0..8 {
        let q = world([
            if c & 1 == 0 { 0.0 } else { t.l },
            if c & 2 == 0 { 0.0 } else { t.w },
            if c & 4 == 0 { 0.0 } else { t.h },
        ]);
        for i in 0..3 {
            lo[i] = lo[i].min(q[i]);
            hi[i] = hi[i].max(q[i]);
        }
    }
    for i in 0..3 {
        assert!(
            (m.bounds.min[i] - lo[i]).abs() < 1e-6 && (m.bounds.max[i] - hi[i]).abs() < 1e-6,
            "bounds {:?} vs {lo:?} {hi:?}",
            m.bounds
        );
    }
    // Occupancy probes (winding number of the result, independent of the
    // slab construction): the notch is open through the wall, from the rim
    // to its floor, and nothing else around it changed.
    let cw = t.cw();
    let nx = t.wall + cw / 2.0;
    let (y_wall, z_notch) = (t.wall / 2.0, t.h - t.notch_d / 2.0);
    let pts = [
        // notch: open through the wall, near the rim, near its floor
        ([nx, y_wall, z_notch], false),
        ([nx, y_wall, t.h - 0.2], false),
        (
            [nx - t.notch_w / 2.0 + 0.2, y_wall, t.h - t.notch_d + 0.2],
            false,
        ),
        // wall beside and below the notch
        ([nx - t.notch_w / 2.0 - 0.2, y_wall, z_notch], true),
        ([nx + t.notch_w / 2.0 + 0.2, y_wall, z_notch], true),
        ([nx, y_wall, t.h - t.notch_d - 0.2], true),
        // first compartment behind the notch, floor below it
        ([nx, t.wall + 1.0, z_notch], false),
        ([nx, t.w / 2.0, t.floor - 0.2], true),
        // front wall of the second compartment is untouched
        ([t.wall + cw + t.div + cw / 2.0, y_wall, z_notch], true),
        // first divider
        ([t.wall + cw + t.div / 2.0, t.w / 2.0, z_notch], true),
    ];
    let model = s.model().unwrap();
    let body = model
        .evaluation()
        .body_by_id(BodyId(1))
        .unwrap()
        .geometry
        .brep()
        .unwrap();
    let got = rcad_kernel::point_membership(body, &pts.map(|p| world(p.0))).unwrap();
    for ((p, want), got) in pts.iter().zip(got) {
        assert_eq!(got, Some(*want), "probe {p:?}");
    }
    // Exact STEP export of the result.
    // RCAD_EVIDENCE_DIR: also kept for the external FreeCAD check
    // (tools/check_slab_arcs.sh).
    let dir = std::env::var_os("RCAD_EVIDENCE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!(
        "rim_notch_{}x{}x{}_{:?}_{}_{}.step",
        t.l, t.w, t.h, t.plane, t.corner[0], t.corner[1]
    ));
    let r = s.execute(&Command::ExportStep {
        path: path.clone(),
        bodies: None,
    });
    assert!(r.ok, "{:?}", r.error);
    assert!(std::fs::metadata(&path).unwrap().len() > 1000);
}

/// The coordinator's repro: 120 x 80 x 30, wall/floor/divider 2, 2 dividers,
/// notch 16 x 8 on compartment 1 (expected 49408 mm³).
#[test]
fn rim_notch_on_the_baseline_tray_is_exact() {
    let t = Tray {
        l: 120.0,
        w: 80.0,
        h: 30.0,
        wall: 2.0,
        floor: 2.0,
        div: 2.0,
        n: 2,
        notch_w: 16.0,
        notch_d: 8.0,
        plane: SketchPlane::WorldXY,
        corner: [0.0, 0.0],
        rebind: false,
        jitter: [0.0; 4],
    };
    assert!((t.volume() - 49408.0).abs() < 1e-9);
    check(&t);
}

/// SESSION_5 §7 variant 1 (160 x 90 x 35, 2.4 mm walls, 3 dividers) and
/// slightly varied dimensions around both.
#[test]
fn rim_notch_at_varied_dimensions_is_exact() {
    for (l, w, h, wall, n, nw, nd) in [
        (160.0, 90.0, 35.0, 2.4, 3, 16.0, 8.0),
        (121.3, 79.7, 30.2, 2.1, 2, 15.5, 7.9),
        (97.0, 61.0, 24.0, 1.6, 1, 12.0, 6.5),
        (140.0, 85.0, 33.0, 2.0, 4, 10.0, 9.0),
    ] {
        check(&Tray {
            l,
            w,
            h,
            wall,
            floor: wall,
            div: wall,
            n,
            notch_w: nw,
            notch_d: nd,
            plane: SketchPlane::WorldXY,
            corner: [0.0, 0.0],
            rebind: false,
            jitter: [0.0; 4],
        });
    }
}

/// Translated and rotated placements of the baseline tray: blank sketched on
/// the world XZ and YZ planes (tray axis along -Y / +X) and offset off-grid.
#[test]
fn rim_notch_translated_and_rotated_is_exact() {
    for (plane, corner) in [
        (SketchPlane::WorldXY, [-37.3, 12.65]),
        (SketchPlane::WorldXZ, [5.5, -20.25]),
        (SketchPlane::WorldYZ, [-61.7, 3.1]),
    ] {
        check(&Tray {
            plane,
            corner,
            ..baseline()
        });
    }
}

fn baseline() -> Tray {
    Tray {
        l: 120.0,
        w: 80.0,
        h: 30.0,
        wall: 2.0,
        floor: 2.0,
        div: 2.0,
        n: 2,
        notch_w: 16.0,
        notch_d: 8.0,
        plane: SketchPlane::WorldXY,
        corner: [0.0, 0.0],
        rebind: false,
        jitter: [0.0; 4],
    }
}

/// Coordinator follow-up: the failure was sensitivity to sub-grid coordinate
/// noise, not the configuration. Both construction variants (sketch at its
/// final centre; sketch at a rounded centre re-bound by expressions, as in
/// `artifacts/run5/kernel_repro_rim_notch.json`) and seeded random ±1e-12 mm
/// perturbations of the notch's centre and size must all be exact.
#[test]
fn rim_notch_is_robust_to_sub_grid_noise() {
    check(&baseline());
    check(&Tray {
        rebind: true,
        ..baseline()
    });
    check(&Tray {
        rebind: true,
        l: 160.0,
        w: 90.0,
        h: 35.0,
        wall: 2.4,
        floor: 2.4,
        div: 2.4,
        n: 3,
        ..baseline()
    });
    let mut x: u64 = 0x5eed_0000_0000_0b5e;
    let mut unit = || {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    for k in 0..12 {
        let jitter = [0; 4].map(|_| 1e-12 * unit());
        check(&Tray {
            jitter,
            rebind: k % 2 == 1,
            ..baseline()
        });
    }
}
