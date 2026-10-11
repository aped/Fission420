//! The GOAL.md acceptance workflow, executed only through [`Session::execute`], with
//! every geometric claim checked against independently derived analytic values.
//! The SESSION_2 scenarios A and B (editor-built sketches) run after the GOAL.md
//! workflow in fresh sessions; their steps are prefixed "A." / "B.".

mod scenarios;
use crate::{Command, Response, Session, SketchTemplate, end_cap_of};
use f420_document::{FeatureId, SketchPlane};
use f420_kernel::{self as k, VolumeResult};
use f420_sketch::SolveStatus;
use serde::Serialize;
use std::f64::consts::PI;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub step: String,
    pub name: String,
    pub expected: String,
    pub actual: String,
    pub pass: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub checks: Vec<Check>,
    pub log: Vec<serde_json::Value>,
    pub passed: usize,
    pub failed: usize,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.failed == 0 && self.passed > 0
    }
}

struct Run<'a> {
    s: Session,
    step: String,
    checks: Vec<Check>,
    log: Vec<serde_json::Value>,
    out_dir: &'a Path,
}

/// GOAL.md volume tolerance: max(0.001 mm³, V * 1e-7).
fn vol_tol(v: f64) -> f64 {
    0.001f64.max(v * 1e-7)
}
const BOUNDS_TOL: f64 = 1e-6;

impl Run<'_> {
    fn exec(&mut self, cmd: Command) -> Response {
        let r = self.s.execute(&cmd);
        self.log.push(serde_json::json!({
            "step": self.step,
            "request": cmd,
            "ok": r.ok,
            "feature": r.feature,
            "message": r.message,
            "error": r.error,
            "volume": r.measurements.as_ref().map(|m| &m.volume),
            "bounds": r.measurements.as_ref().map(|m| m.bounds),
        }));
        r
    }

    fn check(&mut self, name: &str, expected: impl ToString, actual: impl ToString, pass: bool) {
        self.checks.push(Check {
            step: self.step.clone(),
            name: name.into(),
            expected: expected.to_string(),
            actual: actual.to_string(),
            pass,
        });
    }

    fn ok(&mut self, name: &str, r: &Response) -> Option<FeatureId> {
        let detail = match &r.error {
            Some(e) => format!("error: {e}"),
            None => "ok".into(),
        };
        self.check(name, "ok", detail, r.ok);
        r.feature
    }

    fn volume(&self) -> Option<f64> {
        let m = self.s.model()?.evaluation().measurements.as_ref()?;
        match m.volume {
            VolumeResult::Value { volume, .. } => Some(volume),
            VolumeResult::Unavailable { .. } => None,
        }
    }

    /// Check one connected closed solid, analytic volume, and bounds.
    fn check_body(&mut self, w: f64, h: f64, d: f64, hole_d: Option<f64>) {
        let expected = w * h * d - hole_d.map_or(0.0, |hd| PI * (hd / 2.0).powi(2) * d);
        let Some(m) = self
            .s
            .model()
            .and_then(|m| m.evaluation().measurements.clone())
        else {
            self.check("measurements", "present", "missing", false);
            return;
        };
        self.check("solid count", 1, m.shell_count, m.shell_count == 1);
        self.check(
            "shell closed",
            true,
            m.all_shells_closed,
            m.all_shells_closed,
        );
        match self.volume() {
            Some(v) => {
                let tol = vol_tol(expected);
                self.check(
                    "B-rep volume (mm^3)",
                    format!("{expected:.9} ± {tol:.3e}"),
                    format!("{v:.9} (err {:.3e})", v - expected),
                    (v - expected).abs() <= tol,
                );
            }
            None => self.check(
                "B-rep volume (mm^3)",
                format!("{expected:.9}"),
                "UNAVAILABLE",
                false,
            ),
        }
        let eb = ([-w / 2.0, -h / 2.0, 0.0], [w / 2.0, h / 2.0, d]);
        let err = (0..3)
            .map(|i| {
                (m.bounds.min[i] - eb.0[i])
                    .abs()
                    .max((m.bounds.max[i] - eb.1[i]).abs())
            })
            .fold(0.0, f64::max);
        self.check(
            "bounds (mm)",
            format!("{:?}..{:?} ± {BOUNDS_TOL:e}", eb.0, eb.1),
            format!("{:?}..{:?} (max err {err:.2e})", m.bounds.min, m.bounds.max),
            err <= BOUNDS_TOL,
        );
        if let Some(hd) = hole_d {
            self.check_hole(hd, d);
        }
    }

    /// Hole axis at X=0,Y=0 parallel to Z, through all material: the same circular
    /// inner loop must appear on the bottom (z=0) and top (z=d) faces.
    fn check_hole(&mut self, hd: f64, d: f64) {
        let loops = self
            .s
            .model()
            .and_then(|m| m.evaluation().body.as_ref())
            .map(k::z_face_inner_loops);
        let Some(Ok(loops)) = loops else {
            self.check("hole loops", "measurable", format!("{loops:?}"), false);
            return;
        };
        let zs: Vec<f64> = loops.iter().map(|l| l.z).collect();
        let through = loops.len() == 2
            && (zs[0] - 0.0).abs() <= BOUNDS_TOL
            && (zs[1] - d).abs() <= BOUNDS_TOL;
        self.check(
            "through-hole loops at z",
            format!("[0, {d}]"),
            format!("{zs:?}"),
            through,
        );
        let axis_err = loops
            .iter()
            .map(|l| l.centroid_xy[0].abs().max(l.centroid_xy[1].abs()))
            .fold(0.0, f64::max);
        self.check(
            "hole axis X=0,Y=0 (both faces => parallel to Z)",
            format!("max |xy| <= {BOUNDS_TOL:e}"),
            format!("{axis_err:.2e}"),
            !loops.is_empty() && axis_err <= BOUNDS_TOL,
        );
        let dia_err = loops
            .iter()
            .map(|l| (l.equivalent_diameter - hd).abs())
            .fold(0.0, f64::max);
        self.check(
            "hole diameter (area-equivalent, exact edges)",
            format!("{hd} ± 1e-6"),
            format!("err {dia_err:.2e}"),
            dia_err <= BOUNDS_TOL,
        );
    }

    fn param(&self, f: FeatureId, name: &str) -> Option<f64> {
        let m = self.s.model()?;
        m.document()
            .feature(f)
            .ok()?
            .parameters()
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v)
    }
}

/// Run the GOAL.md workflow and SESSION_2 scenarios A and B. Writes
/// `plate.*`, `scenario_a.*` and `scenario_b.*` (.f420.json, .stl, .step) into `out_dir`.
pub fn run(out_dir: &Path) -> Report {
    let mut r = Run {
        s: Session::new(),
        step: String::new(),
        checks: vec![],
        log: vec![],
        out_dir,
    };
    workflow(&mut r);
    scenarios::scenario_a(&mut r);
    scenarios::scenario_b(&mut r);
    scenarios::scenario_pocket(&mut r);
    let passed = r.checks.iter().filter(|c| c.pass).count();
    let failed = r.checks.len() - passed;
    Report {
        checks: r.checks,
        log: r.log,
        passed,
        failed,
    }
}

fn workflow(r: &mut Run<'_>) {
    r.step = "1 new document".into();
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);

    r.step = "2 XY rectangle 80x50".into();
    let x = r.exec(Command::CreateSketch {
        name: Some("Base sketch".into()),
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 80.0,
            height: 50.0,
            center: None,
        },
    });
    let Some(s1) = r.ok("create_sketch", &x) else {
        return;
    };
    let fs = x
        .summary
        .as_ref()
        .and_then(|s| s.features.iter().find(|f| f.id == s1).cloned());
    let st = fs.as_ref().and_then(|f| f.solve_status);
    let dof = fs.as_ref().and_then(|f| f.solve_dof);
    r.check(
        "constraint state",
        "FullyConstrained, dof 0",
        format!(
            "{st:?}, dof {dof:?}, residual {:?}",
            fs.as_ref().and_then(|f| f.solve_residual)
        ),
        st == Some(SolveStatus::FullyConstrained) && dof == Some(0),
    );
    let (w, h) = (r.param(s1, "width"), r.param(s1, "height"));
    r.check(
        "editable dimensions",
        "width=80, height=50",
        format!("width={w:?}, height={h:?}"),
        w == Some(80.0) && h == Some(50.0),
    );

    r.step = "3 extrude +10".into();
    let x = r.exec(Command::Extrude {
        name: Some("Plate".into()),
        sketch: s1,
        distance: 10.0,
    });
    let Some(e1) = r.ok("extrude", &x) else {
        return;
    };
    r.check_body(80.0, 50.0, 10.0, None);

    r.step = "4 Ø10 hole on top face, cut through all".into();
    let x = r.exec(Command::CreateSketch {
        name: Some("Hole sketch".into()),
        plane: end_cap_of(e1),
        template: SketchTemplate::CenteredCircle {
            diameter: 10.0,
            center: None,
        },
    });
    let Some(s2) = r.ok("create_sketch on top face", &x) else {
        return;
    };
    let plane =
        r.s.model()
            .and_then(|m| m.document().feature(s2).ok())
            .map(|f| format!("{:?}", f.kind));
    let semantic = plane.as_deref().is_some_and(|p| {
        p.contains("ExtrudeEndCap") && p.contains(&format!("FeatureId({})", e1.0))
    });
    r.check(
        "attachment is semantic",
        format!("Face(FaceRef {{ feature: {e1}, role: ExtrudeEndCap }})"),
        "see document",
        semantic,
    );
    let x = r.exec(Command::CutThroughAll {
        name: Some("Hole".into()),
        sketch: s2,
    });
    let Some(_c1) = r.ok("cut_through_all", &x) else {
        return;
    };
    r.check_body(80.0, 50.0, 10.0, Some(10.0));

    r.step = "5 width -> 100".into();
    let x = r.exec(Command::SetParameter {
        feature: s1,
        name: "width".into(),
        value: 100.0,
    });
    r.ok("set width", &x);
    r.check_body(100.0, 50.0, 10.0, Some(10.0));

    r.step = "6 depth -> 12 (sketch follows top face), restore 10".into();
    let x = r.exec(Command::SetParameter {
        feature: e1,
        name: "distance".into(),
        value: 12.0,
    });
    r.ok("set depth 12", &x);
    let fz = sketch_frame_z(r, s2);
    r.check(
        "hole sketch frame z follows top face",
        12.0,
        format!("{fz:?}"),
        fz.is_some_and(|z| (z - 12.0).abs() <= BOUNDS_TOL),
    );
    r.check_body(100.0, 50.0, 12.0, Some(10.0));
    let x = r.exec(Command::SetParameter {
        feature: e1,
        name: "distance".into(),
        value: 10.0,
    });
    r.ok("restore depth 10", &x);
    r.check_body(100.0, 50.0, 10.0, Some(10.0));

    r.step = "7 undo/redo width change; save, close, reload".into();
    for i in 0..3 {
        let x = r.exec(Command::Undo);
        r.ok(&format!("undo {}", i + 1), &x);
    }
    let w = r.param(s1, "width");
    r.check(
        "width after undo x3",
        80.0,
        format!("{w:?}"),
        w == Some(80.0),
    );
    r.check_body(80.0, 50.0, 10.0, Some(10.0));
    for i in 0..3 {
        let x = r.exec(Command::Redo);
        r.ok(&format!("redo {}", i + 1), &x);
    }
    let w = r.param(s1, "width");
    r.check(
        "width after redo x3",
        100.0,
        format!("{w:?}"),
        w == Some(100.0),
    );
    r.check_body(100.0, 50.0, 10.0, Some(10.0));
    let path = r.out_dir.join("plate.f420.json");
    let before = r.s.model().map(|m| m.document().clone());
    let x = r.exec(Command::Save { path: path.clone() });
    r.ok("save", &x);
    let x = r.exec(Command::Close);
    r.ok("close", &x);
    r.check(
        "closed",
        "no document",
        format!("open={}", r.s.model().is_some()),
        r.s.model().is_none(),
    );
    let x = r.exec(Command::Open { path: path.clone() });
    r.ok("open", &x);
    let after = r.s.model().map(|m| m.document().clone());
    r.check(
        "feature history, parameters, references identical after reload",
        "equal",
        if before == after {
            "equal"
        } else {
            "DIFFERENT"
        },
        before.is_some() && before == after,
    );
    r.check_body(100.0, 50.0, 10.0, Some(10.0));

    r.step = "8 STL export".into();
    let stl = r.out_dir.join("plate.stl");
    let x = r.exec(Command::ExportStl {
        path: stl.clone(),
        chord: Some(0.01),
    });
    r.ok("export_stl", &x);
    if let Some(mc) = &x.mesh_check {
        r.check(
            "STL watertight (edge-manifold, consistent orientation)",
            true,
            format!(
                "{} (boundary {}, nonmanifold {}, misoriented {})",
                mc.watertight, mc.boundary_edges, mc.nonmanifold_edges, mc.misoriented_edges
            ),
            mc.watertight,
        );
        let exp = 50000.0 - 250.0 * PI;
        let rel = (mc.mesh_volume - exp).abs() / exp;
        r.check(
            "STL mesh volume rel err (chord 0.01 mm)",
            "<= 1e-3",
            format!("{:.3e} (mesh {:.6})", rel, mc.mesh_volume),
            rel <= 1e-3,
        );
    }
    let size = std::fs::metadata(&stl).map(|m| m.len()).unwrap_or(0);
    let tris = x.mesh_check.as_ref().map_or(0, |m| m.triangles as u64);
    r.check(
        "STL file size = 84 + 50*triangles",
        84 + 50 * tris,
        size,
        size == 84 + 50 * tris && tris > 0,
    );

    let step = r.out_dir.join("plate.step");
    let x = r.exec(Command::ExportStep { path: step.clone() });
    r.ok("export_step", &x);
    let text = std::fs::read_to_string(&step).unwrap_or_default();
    let header = text.starts_with("ISO-10303-21;")
        && text.contains("MANIFOLD_SOLID_BREP")
        && !text.contains("INTERSECTION_CURVE");
    r.check(
        "STEP structure (B-rep solid, exact NURBS edges; external validation: tools/freecad_check_step.py)",
        "ISO-10303-21, MANIFOLD_SOLID_BREP, no INTERSECTION_CURVE",
        format!("{} bytes, ok={header}", text.len()),
        header,
    );

    r.step = "10 invalid edits rejected, document preserved".into();
    let before = r.s.model().map(|m| (m.document().clone(), m.revision));
    let x = r.exec(Command::SetParameter {
        feature: e1,
        name: "distance".into(),
        value: -10.0,
    });
    r.check(
        "negative thickness rejected",
        "error",
        format!("{:?}", x.error),
        !x.ok,
    );
    let x = r.exec(Command::SetParameter {
        feature: s2,
        name: "diameter".into(),
        value: 60.0,
    });
    r.check(
        "hole Ø60 > min(100,50) rejected (product limit)",
        "ProductLimit error",
        format!("{:?}", x.error),
        !x.ok && matches!(x.error, Some(f420_document::DocError::ProductLimit(_))),
    );
    let x = r.exec(Command::SetParameter {
        feature: s2,
        name: "diameter".into(),
        value: 50.0,
    });
    r.check(
        "hole Ø50 = plate height rejected (tangent, product limit)",
        "ProductLimit error",
        format!("{:?}", x.error),
        !x.ok,
    );
    let x = r.exec(Command::SetParameter {
        feature: s1,
        name: "height".into(),
        value: 8.0,
    });
    r.check(
        "height 8 < hole Ø10 rejected (product limit)",
        "ProductLimit error",
        format!("{:?}", x.error),
        !x.ok,
    );
    let after = r.s.model().map(|m| (m.document().clone(), m.revision));
    r.check(
        "document and revision unchanged after rejected edits",
        "equal",
        if before == after {
            "equal"
        } else {
            "DIFFERENT"
        },
        before.is_some() && before == after,
    );
    r.check_body(100.0, 50.0, 10.0, Some(10.0));
}

fn sketch_frame_z(r: &Run<'_>, id: FeatureId) -> Option<f64> {
    r.s.model()?
        .evaluation()
        .features
        .iter()
        .find(|f| f.id == id)?
        .frame
        .map(|f| f.origin[2])
}
