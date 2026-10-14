//! Interchange scenarios C (external STEP -> model -> export) and D (mesh and
//! sketch interchange), headless, through [`Session::execute`] only.
//!
//! Inputs are independently generated fixtures (FreeCAD 1.1.4 / OpenCascade
//! 7.8.1, or hand-written from the format references; provenance in the
//! fixture READMEs). Every expected value below is derived by hand in this file
//! from the fixture's construction, never read back from the implementation.
//! Sources are copied into `out_dir` and deleted before the native reload, so
//! the reload proves that no external path is needed. Exported files are
//! validated independently by `tools/external_validation.sh` (FreeCAD).
use super::{BOUNDS_TOL, Run, vol_tol};
use crate::{BodySummary, Command, ExtrudeOp, FaceSummary, Response, Session, SketchTemplate};
use rcad_document::{BodyId, BodySource, FaceRole, FeatureId, SketchPlane};
use rcad_kernel::{self as k, VolumeMethod, VolumeResult};
use rcad_sketch::{Entity, Sketch, SolveStatus};
use std::f64::consts::PI;
use std::path::{Path, PathBuf};

const STEP_FIXTURES: &str = "rcad-kernel/tests/fixtures/step";
const STL_FIXTURES: &str = "rcad-exchange/tests/fixtures/stl";
const DXF_FIXTURES: &str = "rcad-exchange/tests/fixtures/dxf";

/// Fixture path inside the workspace (`crates/<rel>/<file>`).
fn fixture(dir: &str, file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(dir)
        .join(file)
}

/// Inch in mm (exact by definition).
const INCH: f64 = 25.4;
/// Geometry read from a file and scaled: coordinates in the fixtures are short
/// decimals (FreeCAD writes 12-15 significant digits), so 1e-9 mm relative to
/// the part size is far above representation error and far below any modeling
/// mistake (a wrong unit or placement is off by millimetres).
const FILE_TOL: f64 = 1e-6;

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn max_abs_diff(a: &[f64], b: &[f64]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f64::max)
}

impl Run<'_> {
    fn bodies(&self) -> Vec<BodySummary> {
        self.s.summary().bodies
    }

    /// Copy a fixture into `<out_dir>/<sub>/` and return the copy's path.
    fn stage(&mut self, sub: &str, dir: &str, file: &str) -> Option<PathBuf> {
        let src = fixture(dir, file);
        let dst_dir = self.out_dir.join(sub);
        let dst = dst_dir.join(file);
        let res = std::fs::create_dir_all(&dst_dir).and_then(|_| std::fs::copy(&src, &dst));
        match res {
            Ok(_) => Some(dst),
            Err(e) => {
                self.check(
                    &format!("stage fixture {file}"),
                    "copied",
                    format!("{}: {e}", src.display()),
                    false,
                );
                None
            }
        }
    }

    /// Exact B-rep body: one closed shell, volume and bounds against analytic
    /// values. `bounds_tol` must be justified by the caller.
    fn check_brep(
        &mut self,
        label: &str,
        id: BodyId,
        volume: f64,
        min: [f64; 3],
        max: [f64; 3],
        bounds_tol: f64,
    ) {
        let Some(b) = self.body(id) else {
            return self.check(&format!("{label}: body {id}"), "present", "missing", false);
        };
        self.check(&format!("{label}: kind"), "brep", &b.kind, b.kind == "brep");
        let Some(m) = b.measurements else {
            return self.check(
                &format!("{label}: measurements"),
                "present",
                "missing",
                false,
            );
        };
        self.check(
            &format!("{label}: one closed shell"),
            "1 shell, closed",
            format!("{} shell(s), closed={}", m.shell_count, m.all_shells_closed),
            m.shell_count == 1 && m.all_shells_closed,
        );
        match m.volume {
            VolumeResult::Value {
                volume: v, method, ..
            } if method != VolumeMethod::TessellationEstimate => {
                let tol = vol_tol(volume);
                self.check(
                    &format!("{label}: exact B-rep volume (mm^3)"),
                    format!("{volume:.9} ± {tol:.3e}"),
                    format!("{v:.9} (err {:.3e}, {method:?})", v - volume),
                    (v - volume).abs() <= tol,
                );
            }
            other => self.check(
                &format!("{label}: exact B-rep volume (mm^3)"),
                format!("{volume:.9} (boundary integral)"),
                format!("{other:?}"),
                false,
            ),
        }
        self.check_bounds(label, m.bounds.min, m.bounds.max, min, max, bounds_tol);
    }

    fn check_bounds(
        &mut self,
        label: &str,
        got_min: [f64; 3],
        got_max: [f64; 3],
        min: [f64; 3],
        max: [f64; 3],
        tol: f64,
    ) {
        let err = max_abs_diff(&got_min, &min).max(max_abs_diff(&got_max, &max));
        self.check(
            &format!("{label}: bounds (mm)"),
            format!("{min:?}..{max:?} ± {tol:e}"),
            format!("{got_min:?}..{got_max:?} (max err {err:.2e})"),
            err <= tol,
        );
    }

    /// The only body created by import feature `f` (checked).
    fn created_bodies(&mut self, label: &str, x: &Response, n: usize) -> Vec<BodyId> {
        let created = x
            .feature
            .and_then(|f| self.feature_summary(f))
            .map(|f| f.creates)
            .unwrap_or_default();
        self.check(
            &format!("{label}: bodies created"),
            n,
            format!("{created:?}"),
            created.len() == n,
        );
        created
    }

    /// A failed command: structured, bounded, informative error (callers check
    /// the document/revision with `check_unchanged`).
    fn check_refused(&mut self, label: &str, x: &Response, must_mention: &[&str]) {
        let msg = x.error.as_ref().map(|e| e.to_string()).unwrap_or_default();
        let low = msg.to_lowercase();
        let mentions = must_mention.iter().all(|m| low.contains(&m.to_lowercase()));
        self.check(
            &format!("{label}: refused with a clear, bounded error"),
            format!("error mentioning {must_mention:?}, <= 600 chars"),
            format!("ok={} error={msg:?}", x.ok),
            !x.ok && !msg.is_empty() && msg.len() <= 600 && mentions,
        );
    }

    /// Hole check on an exact body: one circular inner loop of diameter `d`
    /// on the z=0 face and one on the z=`top` face, both centred on `xy`
    /// (=> the axis is parallel to Z through `xy`).
    fn check_z_hole(&mut self, label: &str, id: BodyId, xy: [f64; 2], d: f64, top: f64) {
        let loops = self
            .s
            .model()
            .and_then(|m| m.evaluation().body_by_id(id))
            .and_then(|b| b.geometry.brep())
            .map(k::z_face_inner_loops);
        let Some(Ok(loops)) = loops else {
            return self.check(
                &format!("{label}: hole loops"),
                "measurable",
                format!("{loops:?}"),
                false,
            );
        };
        let mut zs: Vec<f64> = loops.iter().map(|l| l.z).collect();
        zs.sort_by(f64::total_cmp);
        self.check(
            &format!("{label}: through-hole inner loops at z"),
            format!("[0, {top}]"),
            format!("{zs:?}"),
            zs.len() == 2 && zs[0].abs() <= BOUNDS_TOL && (zs[1] - top).abs() <= BOUNDS_TOL,
        );
        let axis_err = loops
            .iter()
            .map(|l| max_abs_diff(&l.centroid_xy, &xy))
            .fold(0.0, f64::max);
        self.check(
            &format!("{label}: hole axis at {xy:?}, parallel to Z"),
            format!("max err <= {BOUNDS_TOL:e}"),
            format!("{axis_err:.2e}"),
            !loops.is_empty() && axis_err <= BOUNDS_TOL,
        );
        let dia_err = loops
            .iter()
            .map(|l| (l.equivalent_diameter - d).abs())
            .fold(0.0, f64::max);
        self.check(
            &format!("{label}: hole diameter (area-equivalent, exact edges)"),
            format!("{d} ± {BOUNDS_TOL:e}"),
            format!("err {dia_err:.2e}"),
            !loops.is_empty() && dia_err <= BOUNDS_TOL,
        );
    }

    /// Imported planar face of `body` whose outward normal is `normal` and whose
    /// plane passes through `point`; exactly one must exist.
    fn imported_face(
        &mut self,
        label: &str,
        body: BodyId,
        normal: [f64; 3],
        point: [f64; 3],
    ) -> Option<FaceSummary> {
        let faces: Vec<FaceSummary> = self
            .s
            .summary()
            .features
            .into_iter()
            .flat_map(|f| f.faces)
            .filter(|fc| {
                matches!(fc.reference.role, FaceRole::Imported { body: b, .. } if b == body)
                    && max_abs_diff(&fc.frame.z, &normal) <= 1e-9
                    && dot(sub(point, fc.frame.origin), normal).abs() <= FILE_TOL
            })
            .collect();
        self.check(
            &format!("{label}: imported planar face (normal {normal:?} through {point:?})"),
            "exactly 1",
            faces.len(),
            faces.len() == 1,
        );
        (faces.len() == 1).then(|| faces[0].clone())
    }

    fn save_close_open(&mut self, path: &Path) -> bool {
        let x = self.exec(Command::Save {
            path: path.to_path_buf(),
        });
        self.ok("save", &x);
        let x = self.exec(Command::Close);
        self.ok("close", &x);
        let x = self.exec(Command::Open {
            path: path.to_path_buf(),
        });
        self.ok("open", &x).is_some() || x.ok
    }
}

/// Body identity that must survive save/reopen.
fn body_identity(b: &[BodySummary]) -> Vec<(BodyId, String, bool, String, BodySource)> {
    b.iter()
        .map(|b| {
            (
                b.id,
                b.name.clone(),
                b.visible,
                b.kind.clone(),
                b.source.clone(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------- scenario C

pub(super) fn scenario_c(r: &mut Run<'_>) {
    c_box_hole(r);
    c_curved(r);
    c_units(r);
    c_multi_and_failures(r);
}

/// C.1-C.4: imported FreeCAD box 40x30x10 -> top-face sketch -> centred Ø6
/// through-all cut -> save, delete source, reopen -> STEP export.
/// Expected: V = 40*30*10 - pi*3^2*10 = 12000 - 90 pi; bounds [0,40]x[0,30]x[0,10];
/// hole axis through (20, 15) parallel to Z.
fn c_box_hole(r: &mut Run<'_>) {
    r.s = Session::new();
    r.step = "C.1 import independent box STEP (FreeCAD, mm)".into();
    let Some(src) = r.stage("scenario_c_src", STEP_FIXTURES, "box_40x30x10_mm.step") else {
        return;
    };
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    let x = r.exec(Command::ImportStep {
        path: src.clone(),
        units_to_mm: None,
    });
    if r.ok("import_step", &x).is_none() {
        return;
    }
    let imp = x.feature;
    let created = r.created_bodies("box", &x, 1);
    let Some(&bid) = created.first() else { return };
    let b = r.body(bid);
    r.check(
        "body source is the imported file (no recovered history)",
        "ImportedStep { file_name: box_40x30x10_mm.step }",
        format!("{:?}", b.as_ref().map(|b| &b.source)),
        matches!(b.as_ref().map(|b| &b.source),
            Some(BodySource::ImportedStep { file_name, .. }) if file_name == "box_40x30x10_mm.step"),
    );
    r.check_brep(
        "imported box",
        bid,
        12000.0,
        [0.0; 3],
        [40.0, 30.0, 10.0],
        BOUNDS_TOL,
    );
    let n_faces = imp
        .and_then(|f| r.feature_summary(f))
        .map_or(0, |f| f.faces.len());
    r.check("sketchable imported planar faces", 6, n_faces, n_faces == 6);

    r.step = "C.2 sketch Ø6 on imported top face, cut through all".into();
    let Some(top) = r.imported_face("box", bid, [0.0, 0.0, 1.0], [20.0, 15.0, 10.0]) else {
        return;
    };
    // Frame contract (docs/formats/step.md): origin = face area centroid.
    let oerr = max_abs_diff(&top.frame.origin, &[20.0, 15.0, 10.0]);
    r.check(
        "top-face frame origin = face centroid (20,15,10)",
        format!("err <= {BOUNDS_TOL:e}"),
        format!("{:?} (err {oerr:.2e})", top.frame.origin),
        oerr <= BOUNDS_TOL,
    );
    // Box centre in the face's local 2D coordinates (independent of the frame
    // origin convention).
    let rel = sub([20.0, 15.0, 10.0], top.frame.origin);
    let local = [dot(rel, top.frame.x), dot(rel, top.frame.y)];
    let x = r.exec(Command::CreateSketch {
        name: Some("Hole sketch".into()),
        plane: SketchPlane::Face(top.reference),
        template: SketchTemplate::CenteredCircle {
            diameter: 6.0,
            center: Some(local),
        },
    });
    let Some(hs) = r.ok("create_sketch on imported face", &x) else {
        return;
    };
    let x = r.exec(Command::Extrude {
        name: Some("Hole".into()),
        sketch: hs,
        distance: 0.0,
        op: ExtrudeOp::Cut,
        target: Some(bid),
        through_all: true,
        reverse: false,
        body_name: None,
        region: None,
        options: Default::default(),
    });
    if r.ok("extrude cut through_all, target = imported body", &x)
        .is_none()
    {
        return;
    }
    let expected = 12000.0 - 90.0 * PI;
    r.check_brep(
        "box with hole",
        bid,
        expected,
        [0.0; 3],
        [40.0, 30.0, 10.0],
        BOUNDS_TOL,
    );
    r.check_z_hole("box with hole", bid, [20.0, 15.0], 6.0, 10.0);
    let n = r.bodies().len();
    r.check("body count after cut", 1, n, n == 1);

    r.step = "C.3 save, delete source STEP, reopen".into();
    let before_doc = r.s.model().map(|m| m.document().clone());
    let before_bodies = body_identity(&r.bodies());
    let native = r.out_dir.join("scenario_c.rcad.json");
    let x = r.exec(Command::Save {
        path: native.clone(),
    });
    r.ok("save", &x);
    let rm = std::fs::remove_file(&src);
    r.check(
        "source STEP deleted",
        "removed",
        format!("{rm:?}, exists={}", src.exists()),
        rm.is_ok() && !src.exists(),
    );
    let x = r.exec(Command::Close);
    r.ok("close", &x);
    let x = r.exec(Command::Open {
        path: native.clone(),
    });
    r.ok("open without the source file", &x);
    let after_doc = r.s.model().map(|m| m.document().clone());
    r.check(
        "document (features, parameters, references, embedded source) identical",
        "equal",
        if before_doc == after_doc {
            "equal"
        } else {
            "DIFFERENT"
        },
        before_doc.is_some() && before_doc == after_doc,
    );
    let after_bodies = body_identity(&r.bodies());
    r.check(
        "bodies (id, name, visibility, kind, source) identical",
        format!("{before_bodies:?}"),
        format!("{after_bodies:?}"),
        !before_bodies.is_empty() && before_bodies == after_bodies,
    );
    r.check_brep(
        "reopened box with hole",
        bid,
        expected,
        [0.0; 3],
        [40.0, 30.0, 10.0],
        BOUNDS_TOL,
    );
    r.check_z_hole("reopened", bid, [20.0, 15.0], 6.0, 10.0);

    r.step = "C.4 export STEP (validated by tools/external_validation.sh)".into();
    let step = r.out_dir.join("scenario_c.step");
    let x = r.exec(Command::ExportStep {
        path: step.clone(),
        bodies: Some(vec![bid]),
    });
    r.ok("export_step", &x);
    let text = std::fs::read_to_string(&step).unwrap_or_default();
    let solids = text.matches("MANIFOLD_SOLID_BREP").count();
    r.check(
        "STEP structure (1 MANIFOLD_SOLID_BREP, no INTERSECTION_CURVE)",
        "ISO-10303-21, 1 solid",
        format!("{} bytes, {solids} solid(s)", text.len()),
        text.starts_with("ISO-10303-21;") && solids == 1 && !text.contains("INTERSECTION_CURVE"),
    );
}

/// C.5: curved imported solids. Analytic values from the fixture construction
/// (README of the STEP fixtures).
fn c_curved(r: &mut Run<'_>) {
    r.s = Session::new();
    r.step = "C.5 curved solids (cylinder, plate with hole, sphere)".into();
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    // makeCylinder(10, 15): V = pi 10^2 15 = 1500 pi.
    if let Some(p) = r.stage(
        "scenario_c_curved",
        STEP_FIXTURES,
        "cylinder_d20_h15_mm.step",
    ) {
        let x = r.exec(Command::ImportStep {
            path: p,
            units_to_mm: None,
        });
        r.ok("import cylinder", &x);
        if let Some(&id) = r.created_bodies("cylinder", &x, 1).first() {
            // Bounds come from exact edge curves refined by golden-section
            // search (measure.rs): exact to ~1e-9 for circular arcs.
            r.check_brep(
                "cylinder",
                id,
                1500.0 * PI,
                [-10.0, -10.0, 0.0],
                [10.0, 10.0, 15.0],
                BOUNDS_TOL,
            );
            r.imported_face("cylinder", id, [0.0, 0.0, 1.0], [0.0, 0.0, 15.0]);
        }
    }
    // makeBox(60,40,8) - makeCylinder(5, 12) at (30,20): V = 19200 - 200 pi.
    if let Some(p) = r.stage("scenario_c_curved", STEP_FIXTURES, "plate_hole_mm.step") {
        let x = r.exec(Command::ImportStep {
            path: p,
            units_to_mm: None,
        });
        r.ok("import plate with hole", &x);
        if let Some(&id) = r.created_bodies("plate", &x, 1).first() {
            r.check_brep(
                "plate with hole",
                id,
                19200.0 - 200.0 * PI,
                [0.0; 3],
                [60.0, 40.0, 8.0],
                BOUNDS_TOL,
            );
            r.check_z_hole("plate with hole", id, [30.0, 20.0], 10.0, 8.0);
            let n = x
                .feature
                .and_then(|f| r.feature_summary(f))
                .map_or(0, |f| f.faces.len());
            r.check(
                "plate: planar faces offered for sketching (cylinder face excluded)",
                6,
                n,
                n == 6,
            );
        }
    }
    // makeSphere(10, (5,5,5)): V = 4000 pi / 3. No axis-ruled boundary
    // integral applies, so the kernel reports a labelled tessellation
    // estimate; the check is that it says so and that the analytic value lies
    // within its stated error.
    if let Some(p) = r.stage("scenario_c_curved", STEP_FIXTURES, "sphere_r10_mm.step") {
        let x = r.exec(Command::ImportStep {
            path: p,
            units_to_mm: None,
        });
        r.ok("import sphere (VERTEX_LOOP face)", &x);
        if let Some(&id) = r.created_bodies("sphere", &x, 1).first() {
            let m = r.body(id).and_then(|b| b.measurements);
            let exp = 4000.0 * PI / 3.0;
            let (pass, got) = match m.as_ref().map(|m| &m.volume) {
                Some(VolumeResult::Value {
                    volume,
                    error_estimate,
                    method,
                }) => (
                    *method == VolumeMethod::TessellationEstimate
                        && (volume - exp).abs() <= *error_estimate,
                    format!("{volume:.3} ± {error_estimate:.3} ({method:?})"),
                ),
                other => (false, format!("{other:?}")),
            };
            r.check(
                "sphere volume: labelled estimate containing 4000 pi / 3",
                format!("{exp:.3} within reported error, TessellationEstimate"),
                got,
                pass,
            );
            if let Some(m) = m {
                r.check(
                    "sphere: one closed shell",
                    "1, closed",
                    format!("{}, {}", m.shell_count, m.all_shells_closed),
                    m.shell_count == 1 && m.all_shells_closed,
                );
                // Extremes lie on the split meridian/equator edges (exact).
                r.check_bounds(
                    "sphere centred (5,5,5)",
                    m.bounds.min,
                    m.bounds.max,
                    [-5.0; 3],
                    [15.0; 3],
                    BOUNDS_TOL,
                );
            }
        }
    }
    let n = r.bodies().len();
    r.check("three separate bodies", 3, n, n == 3);
}

/// C.6: non-mm units. The inch/metre files describe the same 40x30x10 mm box
/// (coordinates divided by 25.4 / 1000, unit declared); the FreeCAD
/// "relabel" file is 40x30x10 inch = 1016 x 762 x 254 mm.
fn c_units(r: &mut Run<'_>) {
    r.s = Session::new();
    r.step = "C.6 units (inch, metre, unit missing)".into();
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    for (file, scale, mm_per) in [
        ("box_40x30x10_inch.step", 1.0, INCH),
        ("box_40x30x10_m.step", 1.0, 1000.0),
        ("freecad_relabel_inch.step", INCH, INCH),
    ] {
        let Some(p) = r.stage("scenario_c_units", STEP_FIXTURES, file) else {
            continue;
        };
        let x = r.exec(Command::ImportStep {
            path: p,
            units_to_mm: None,
        });
        r.ok(&format!("import {file}"), &x);
        let note = x.message.clone().unwrap_or_default();
        r.check(
            &format!("{file}: declared unit reported"),
            format!("'{mm_per} mm per unit, declared in file'"),
            &note,
            note.contains(&format!("{mm_per} mm per unit, declared in file")),
        );
        if let Some(&id) = r.created_bodies(file, &x, 1).first() {
            let (w, h, d) = (40.0 * scale, 30.0 * scale, 10.0 * scale);
            r.check_brep(file, id, w * h * d, [0.0; 3], [w, h, d], FILE_TOL * scale);
        }
    }
    let before = r.doc_and_rev();
    let Some(p) = r.stage("scenario_c_units", STEP_FIXTURES, "missing_unit.step") else {
        return;
    };
    let x = r.exec(Command::ImportStep {
        path: p.clone(),
        units_to_mm: None,
    });
    r.check_refused("missing_unit.step without a unit choice", &x, &["unit"]);
    r.check_unchanged("missing unit: document unchanged", &before);
    let x = r.exec(Command::ImportStep {
        path: p,
        units_to_mm: Some(INCH),
    });
    r.ok("missing_unit.step with units_to_mm = 25.4", &x);
    if let Some(&id) = r.created_bodies("missing_unit", &x, 1).first() {
        let (w, h, d) = (40.0 * INCH, 30.0 * INCH, 10.0 * INCH);
        r.check_brep(
            "missing_unit @ inch",
            id,
            w * h * d,
            [0.0; 3],
            [w, h, d],
            FILE_TOL * INCH,
        );
    }
}

/// C.7-C.8: multi-solid files (compound with baked placement; OCAF assembly with
/// an ITEM_DEFINED_TRANSFORMATION) and failed imports into a non-empty document.
/// A = box 10x10x10 at the origin. B = box 20x10x5 rotated 30° about Z, then
/// moved +50 X: its corners (0,0) (20,0) (0,10) (20,10) go to x = 50 + 20cos30
/// = 67.3205, 50 - 10 sin30 = 45; y = 0 .. 20 sin30 + 10 cos30 = 18.6603.
fn c_multi_and_failures(r: &mut Run<'_>) {
    r.s = Session::new();
    r.step = "C.7 multi-solid and placement".into();
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    let (c, s) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
    let b_min = [50.0 - 10.0 * s, 0.0, 0.0];
    let b_max = [50.0 + 20.0 * c, 20.0 * s + 10.0 * c, 5.0];
    for (file, names) in [
        ("two_boxes_assembly_mm.step", Some(["BoxA", "BoxB"])),
        ("two_boxes_compound_mm.step", None),
    ] {
        let Some(p) = r.stage("scenario_c_multi", STEP_FIXTURES, file) else {
            continue;
        };
        let x = r.exec(Command::ImportStep {
            path: p,
            units_to_mm: None,
        });
        r.ok(&format!("import {file}"), &x);
        let ids = r.created_bodies(file, &x, 2);
        if ids.len() != 2 {
            continue;
        }
        // Identify A and B by size, not by order.
        let vol = |r: &Run<'_>, id| {
            r.body(id)
                .and_then(|b| b.measurements)
                .map_or(0.0, |m| m.bounds.max[2] - m.bounds.min[2])
        };
        let (a, b) = if vol(r, ids[0]) > vol(r, ids[1]) {
            (ids[0], ids[1])
        } else {
            (ids[1], ids[0])
        };
        // FreeCAD rounds B's placed coordinates to 12 significant digits
        // (fixture README): 1e-9 mm covers that with margin.
        r.check_brep(
            &format!("{file} A"),
            a,
            1000.0,
            [0.0; 3],
            [10.0; 3],
            FILE_TOL,
        );
        r.check_brep(
            &format!("{file} B (placed)"),
            b,
            1000.0,
            b_min,
            b_max,
            FILE_TOL,
        );
        if let Some([na, nb]) = names {
            let got = [r.body(a).map(|b| b.name), r.body(b).map(|b| b.name)];
            r.check(
                &format!("{file}: product names kept"),
                format!("{na}, {nb}"),
                format!("{got:?}"),
                got[0].as_deref() == Some(na) && got[1].as_deref() == Some(nb),
            );
        }
    }
    let n = r.bodies().len();
    r.check("four separate bodies", 4, n, n == 4);

    r.step = "C.8 failed imports into a non-empty document".into();
    let before = r.doc_and_rev();
    for (file, words) in [
        ("truncated.step", &["step"][..]),
        ("garbage.step", &["step"][..]),
    ] {
        let Some(p) = r.stage("scenario_c_bad", STEP_FIXTURES, file) else {
            continue;
        };
        let x = r.exec(Command::ImportStep {
            path: p,
            units_to_mm: None,
        });
        r.check_refused(file, &x, words);
        r.check_unchanged(&format!("{file}: document and revision unchanged"), &before);
    }
    let x = r.exec(Command::ImportStep {
        path: r.out_dir.join("scenario_c_bad/does_not_exist.step"),
        units_to_mm: None,
    });
    r.check_refused("missing file", &x, &["does_not_exist.step"]);
    r.check_unchanged("missing file: document and revision unchanged", &before);
    let x = r.exec(Command::Undo);
    let n = r.bodies().len();
    r.check(
        "undo after failed imports removes the last successful import",
        "ok, 2 bodies",
        format!("ok={}, {n} bodies", x.ok),
        x.ok && n == 2,
    );
}

// ---------------------------------------------------------------- scenario D

pub(super) fn scenario_d(r: &mut Run<'_>) {
    d_stl(r);
    d_dxf(r);
}

/// D.1-D.5: STL. Fixture = FreeCAD `Part.makeBox(20,10,5)` tessellated
/// (8 points, 12 facets): V = 1000, A = 2(200+100+50) = 700, bounds 0..(20,10,5).
fn d_stl(r: &mut Run<'_>) {
    r.s = Session::new();
    r.step = "D.1 STL ASCII/binary with chosen units".into();
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    let before = r.doc_and_rev();
    let ascii = r.stage("scenario_d_src", STL_FIXTURES, "box_freecad_ascii.stl");
    let binary = r.stage("scenario_d_src", STL_FIXTURES, "box_freecad_binary.stl");
    let (Some(ascii), Some(binary)) = (ascii, binary) else {
        return;
    };
    let x = r.exec(Command::ImportStl {
        path: ascii.clone(),
        units_to_mm: None,
    });
    r.check_refused("STL without a unit choice", &x, &["unit"]);
    r.check_unchanged("no unit: document unchanged", &before);
    let mut mesh_ids = vec![];
    for (label, path, to_mm) in [
        ("ASCII @ mm", &ascii, 1.0),
        ("binary @ mm", &binary, 1.0),
        ("binary @ inch", &binary, INCH),
    ] {
        let x = r.exec(Command::ImportStl {
            path: path.clone(),
            units_to_mm: Some(to_mm),
        });
        r.ok(&format!("import {label}"), &x);
        let Some(&id) = r.created_bodies(label, &x, 1).first() else {
            continue;
        };
        mesh_ids.push(id);
        check_mesh_box(r, label, id, to_mm);
    }
    if let [a, b, _] = mesh_ids[..] {
        let (da, db) = (
            r.body(a).and_then(|b| b.mesh),
            r.body(b).and_then(|b| b.mesh),
        );
        r.check(
            "ASCII and binary fixtures give identical diagnostics",
            "equal",
            if da == db { "equal" } else { "DIFFERENT" },
            da.is_some() && da == db,
        );
    }

    r.step = "D.2 open (nonwatertight) mesh".into();
    if let Some(p) = r.stage("scenario_d_src", STL_FIXTURES, "box_open_freecad.stl") {
        let x = r.exec(Command::ImportStl {
            path: p,
            units_to_mm: Some(1.0),
        });
        r.ok("import open box (two +Z facets removed)", &x);
        let msg = x.message.clone().unwrap_or_default();
        r.check(
            "import warns that the mesh is open",
            "'open or inconsistent: 4 boundary'",
            &msg,
            msg.contains("open or inconsistent: 4 boundary"),
        );
        if let Some(&id) = r.created_bodies("open box", &x, 1).first() {
            let d = r.body(id).and_then(|b| b.mesh);
            // Removing the two +Z triangles leaves the 4 top rim edges with one use.
            r.check(
                "open box: not closed, 4 boundary edges, 10 triangles, NO volume, area 500",
                "closed=false, boundary=4, tris=10, volume=None, area=500",
                format!("{d:?}"),
                d.as_ref().is_some_and(|d| {
                    !d.closed
                        && d.boundary_edges == 4
                        && d.triangles == 10
                        && d.signed_volume.is_none()
                        && (d.surface_area - 500.0).abs() <= 1e-9
                }),
            );
            let m = r.body(id).and_then(|b| b.measurements);
            r.check(
                "open box: no B-rep measurements fabricated",
                "None",
                format!("{m:?}"),
                m.is_none(),
            );
        }
    }

    r.step = "D.3 malformed STL files".into();
    let before = r.doc_and_rev();
    for (file, words) in [
        ("ascii_malformed.stl", &["line 4"][..]),
        ("ascii_nan.stl", &["line 5"][..]),
        ("box_truncated.stl", &["664"][..]),
        ("ascii_unterminated.stl", &[][..]),
    ] {
        let Some(p) = r.stage("scenario_d_bad", STL_FIXTURES, file) else {
            continue;
        };
        let x = r.exec(Command::ImportStl {
            path: p,
            units_to_mm: Some(1.0),
        });
        r.check_refused(file, &x, words);
        r.check_unchanged(&format!("{file}: document and revision unchanged"), &before);
    }

    r.step = "D.4 exact operations refused on mesh bodies".into();
    let Some(&mesh_mm) = mesh_ids.first() else {
        return;
    };
    let refused = r.out_dir.join("scenario_d_refused.step");
    let _ = std::fs::remove_file(&refused);
    let x = r.exec(Command::ExportStep {
        path: refused.clone(),
        bodies: Some(vec![mesh_mm]),
    });
    r.check_refused("STEP export of a mesh body", &x, &["mesh"]);
    let x = r.exec(Command::ExportStep {
        path: refused.clone(),
        bodies: None,
    });
    r.check_refused("STEP export of all (mesh-only) bodies", &x, &["mesh"]);
    r.check(
        "no STEP file written",
        "absent",
        refused.exists(),
        !refused.exists(),
    );
    r.check_unchanged("document and revision unchanged", &before);
    // A sketch on a world plane, then Cut into the mesh body: refused.
    let x = r.exec(Command::CreateSketch {
        name: None,
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredCircle {
            diameter: 2.0,
            center: Some([10.0, 5.0]),
        },
    });
    if let Some(sk) = r.ok("create_sketch", &x) {
        let before = r.doc_and_rev();
        let x = r.exec(Command::Extrude {
            name: None,
            sketch: sk,
            distance: 0.0,
            op: ExtrudeOp::Cut,
            target: Some(mesh_mm),
            through_all: true,
            reverse: false,
            body_name: None,
            region: None,
            options: Default::default(),
        });
        r.check_refused("Cut into a mesh body", &x, &["mesh"]);
        r.check_unchanged("cut refused: document unchanged", &before);
        let x = r.exec(Command::Undo);
        r.ok("undo the sketch", &x);
    }

    r.step = "D.5 STL export of a mesh body".into();
    let out = r.out_dir.join("scenario_d_mesh.stl");
    let x = r.exec(Command::ExportStl {
        path: out.clone(),
        chord: None,
        bodies: Some(vec![mesh_mm]),
        print: Default::default(),
    });
    r.ok("export_stl (mesh body, as imported)", &x);
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    r.check("binary STL size = 84 + 50 * 12", 684, size, size == 684);

    r.step = "D.6 save, delete STL sources, reopen".into();
    let before_doc = r.s.model().map(|m| m.document().clone());
    let before_bodies = r.bodies();
    let native = r.out_dir.join("scenario_d_mesh.rcad.json");
    let x = r.exec(Command::Save {
        path: native.clone(),
    });
    r.ok("save", &x);
    let rm = std::fs::remove_dir_all(r.out_dir.join("scenario_d_src"));
    r.check("STL sources deleted", "ok", format!("{rm:?}"), rm.is_ok());
    let x = r.exec(Command::Close);
    r.ok("close", &x);
    let x = r.exec(Command::Open { path: native });
    r.ok("open without the source files", &x);
    let after_doc = r.s.model().map(|m| m.document().clone());
    r.check(
        "document identical",
        "equal",
        if before_doc == after_doc {
            "equal"
        } else {
            "DIFFERENT"
        },
        before_doc.is_some() && before_doc == after_doc,
    );
    let after_bodies = r.bodies();
    let same = before_bodies.len() == after_bodies.len()
        && before_bodies
            .iter()
            .zip(&after_bodies)
            .all(|(a, b)| a.id == b.id && a.name == b.name && a.kind == b.kind && a.mesh == b.mesh);
    r.check(
        "mesh bodies (ids, names, diagnostics) identical",
        format!("{} bodies", before_bodies.len()),
        format!("{} bodies, same={same}", after_bodies.len()),
        same && !after_bodies.is_empty(),
    );
}

/// Mesh diagnostics of the FreeCAD 20x10x5 box imported at `s` mm per unit.
fn check_mesh_box(r: &mut Run<'_>, label: &str, id: BodyId, s: f64) {
    let b = r.body(id);
    r.check(
        &format!("{label}: mesh body"),
        "kind mesh, ImportedMesh source",
        format!("{:?}", b.as_ref().map(|b| (&b.kind, &b.source))),
        b.as_ref().is_some_and(|b| {
            b.kind == "mesh" && matches!(b.source, BodySource::ImportedMesh { .. })
        }),
    );
    let Some(d) = b.and_then(|b| b.mesh) else {
        return r.check(
            &format!("{label}: diagnostics"),
            "present",
            "missing",
            false,
        );
    };
    r.check(
        &format!("{label}: 12 triangles, 8 welded vertices, closed, 1 component"),
        "12, 8, true, 1",
        format!(
            "{}, {}, {}, {}",
            d.triangles, d.vertices, d.closed, d.components
        ),
        d.triangles == 12 && d.vertices == 8 && d.closed && d.components == 1,
    );
    match d.bounds {
        Some((min, max)) => r.check_bounds(
            label,
            min,
            max,
            [0.0; 3],
            [20.0 * s, 10.0 * s, 5.0 * s],
            FILE_TOL * s,
        ),
        None => r.check(&format!("{label}: bounds"), "present", "None", false),
    }
    let exp_v = 1000.0 * s.powi(3);
    r.check(
        &format!("{label}: closed-mesh volume (diagnostics)"),
        format!("{exp_v:.6}"),
        format!("{:?}", d.signed_volume),
        d.signed_volume
            .is_some_and(|v| (v - exp_v).abs() <= vol_tol(exp_v)),
    );
    let exp_a = 700.0 * s * s;
    r.check(
        &format!("{label}: surface area"),
        format!("{exp_a:.6}"),
        format!("{:.6}", d.surface_area),
        (d.surface_area - exp_a).abs() <= 1e-9 * exp_a,
    );
}

/// Non-construction geometry of a sketch, for comparing local coordinates.
#[derive(Debug, Clone)]
enum Geo {
    Line([f64; 2], [f64; 2]),
    Arc {
        c: [f64; 2],
        s: [f64; 2],
        e: [f64; 2],
    },
    Circle([f64; 2], f64),
}

fn geometry(sk: &Sketch) -> Vec<Geo> {
    let p = |id| sk.point_xy(id).unwrap_or([f64::NAN; 2]);
    sk.entities
        .iter()
        .filter(|e| !sk.construction.contains(&e.id()))
        .map(|e| match e {
            Entity::Line { a, b, .. } => Geo::Line(p(*a), p(*b)),
            Entity::Arc {
                center, start, end, ..
            } => Geo::Arc {
                c: p(*center),
                s: p(*start),
                e: p(*end),
            },
            Entity::Circle { center, radius, .. } => Geo::Circle(p(*center), *radius),
        })
        .collect()
}

fn geo_dist(a: &Geo, b: &Geo) -> f64 {
    let d = |x: &[f64; 2], y: &[f64; 2]| max_abs_diff(x, y);
    match (a, b) {
        // Lines are undirected.
        (Geo::Line(a0, a1), Geo::Line(b0, b1)) => {
            d(a0, b0).max(d(a1, b1)).min(d(a0, b1).max(d(a1, b0)))
        }
        (
            Geo::Arc { c, s, e },
            Geo::Arc {
                c: c2,
                s: s2,
                e: e2,
            },
        ) => d(c, c2).max(d(s, s2)).max(d(e, e2)),
        (Geo::Circle(c, r), Geo::Circle(c2, r2)) => d(c, c2).max((r - r2).abs()),
        _ => f64::INFINITY,
    }
}

/// Max distance of a one-to-one matching of `a` onto `b` (greedy nearest;
/// infinite when the counts or kinds differ).
fn match_geometry(a: &[Geo], b: &[Geo]) -> f64 {
    if a.len() != b.len() {
        return f64::INFINITY;
    }
    let mut used = vec![false; b.len()];
    let mut worst: f64 = 0.0;
    for g in a {
        let best = (0..b.len())
            .filter(|&j| !used[j])
            .min_by(|&i, &j| geo_dist(g, &b[i]).total_cmp(&geo_dist(g, &b[j])));
        let Some(j) = best else {
            return f64::INFINITY;
        };
        used[j] = true;
        worst = worst.max(geo_dist(g, &b[j]));
    }
    worst
}

/// D.7-D.11: DXF contour -> sketch -> edit -> solid -> STEP; DXF round trip;
/// units; native reload.
fn d_dxf(r: &mut Run<'_>) {
    r.s = Session::new();
    r.step = "D.7 DXF line+arc contour (FreeCAD R14, mm) -> sketch".into();
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    let Some(src) = r.stage("scenario_d_dxf_src", DXF_FIXTURES, "slot_freecad_r14.dxf") else {
        return;
    };
    let x = r.exec(Command::ImportDxf {
        path: src,
        plane: SketchPlane::WorldXY,
        name: Some("Slot".into()),
        units_to_mm: None,
        allow_partial: false,
        layers: None,
    });
    let Some(sk) = r.ok("import_dxf", &x) else {
        return;
    };
    let fs = r.feature_summary(sk);
    let st = fs.as_ref().and_then(|f| f.solve_status);
    r.check(
        "imported sketch is not reported fully constrained",
        "UnderConstrained",
        format!("{st:?}"),
        st == Some(SolveStatus::UnderConstrained),
    );
    let Some(solved) = fs.and_then(|f| f.sketch) else {
        return r.check("solved sketch", "present", "missing", false);
    };
    // Fixture (tools/fixtures/dxf/README.md): (0,0)-(30,0), arc centre (30,10)
    // r 10 from -90° to 90° (CCW (30,0) -> (30,20)), (30,20)-(0,20), (0,20)-(0,0).
    let expected = vec![
        Geo::Line([0.0, 0.0], [30.0, 0.0]),
        Geo::Arc {
            c: [30.0, 10.0],
            s: [30.0, 0.0],
            e: [30.0, 20.0],
        },
        Geo::Line([30.0, 20.0], [0.0, 20.0]),
        Geo::Line([0.0, 20.0], [0.0, 0.0]),
    ];
    let err = match_geometry(&expected, &geometry(&solved));
    r.check(
        "3 lines + 1 exact arc at the analytic coordinates",
        format!("max err <= {FILE_TOL:e}"),
        format!("{err:.2e}"),
        err <= FILE_TOL,
    );

    r.step = "D.8 edit the imported sketch, extrude, export STEP".into();
    // Move the left edge from x=0 to x=-10: region = [-10,30]x[0,20] + half
    // disc r 10 => area 800 + 50 pi; extruded 5 => 4000 + 250 pi.
    let mut edited = solved.clone();
    for p in &mut edited.points {
        if p.x.abs() <= FILE_TOL && Some(p.id) != edited.origin {
            p.x = -10.0;
        }
    }
    let x = r.exec(Command::EditSketch {
        feature: sk,
        sketch: edited,
    });
    r.ok("edit_sketch (move left edge to x = -10)", &x);
    let x = r.exec(Command::Extrude {
        name: Some("Slot plate".into()),
        sketch: sk,
        distance: 5.0,
        op: ExtrudeOp::NewBody,
        target: None,
        through_all: false,
        reverse: false,
        body_name: Some("Slot plate".into()),
        region: None,
        options: Default::default(),
    });
    r.ok("extrude new body 5 mm", &x);
    let slot_body = r.created_bodies("slot plate", &x, 1).first().copied();
    if let Some(id) = slot_body {
        r.check_brep(
            "edited slot plate",
            id,
            5.0 * (800.0 + 50.0 * PI),
            [-10.0, 0.0, 0.0],
            [40.0, 20.0, 5.0],
            BOUNDS_TOL,
        );
        let step = r.out_dir.join("scenario_d_slot.step");
        let x = r.exec(Command::ExportStep {
            path: step.clone(),
            bodies: Some(vec![id]),
        });
        r.ok(
            "export_step (validated by tools/external_validation.sh)",
            &x,
        );
    }

    r.step = "D.9 DXF export -> import round trip (local coordinates)".into();
    let dxf = r.out_dir.join("scenario_d_slot.dxf");
    let x = r.exec(Command::ExportDxf {
        path: dxf.clone(),
        sketch: sk,
    });
    r.ok("export_dxf", &x);
    let x = r.exec(Command::ImportDxf {
        path: dxf,
        plane: SketchPlane::WorldXY,
        name: Some("Slot reimport".into()),
        units_to_mm: None,
        allow_partial: false,
        layers: None,
    });
    let back = r.ok("reimport exported DXF", &x);
    let edited_geo = r.solved_sketch(sk).map(|s| geometry(&s));
    let back_geo = back.and_then(|b| r.solved_sketch(b)).map(|s| geometry(&s));
    let err = match (&edited_geo, &back_geo) {
        (Some(a), Some(b)) => match_geometry(a, b),
        _ => f64::INFINITY,
    };
    r.check(
        "reimported lines/arcs match the edited sketch",
        format!("max err <= {FILE_TOL:e}"),
        format!("{err:.2e}"),
        err <= FILE_TOL,
    );

    r.step = "D.10 DXF units: declared inch, unitless".into();
    // rect_inch.dxf: 2 x 1 rectangle, $INSUNITS = 1 => 50.8 x 25.4 mm.
    if let Some(p) = r.stage("scenario_d_dxf_src", DXF_FIXTURES, "rect_inch.dxf") {
        let x = r.exec(Command::ImportDxf {
            path: p,
            plane: SketchPlane::WorldXY,
            name: Some("Inch rectangle".into()),
            units_to_mm: Some(1.0), // ignored: a declared unit wins
            allow_partial: false,
            layers: None,
        });
        if let Some(f) = r.ok("import rect_inch.dxf", &x) {
            let g = r.solved_sketch(f).map(|s| geometry(&s)).unwrap_or_default();
            let (w, h) = (2.0 * INCH, INCH);
            let exp = vec![
                Geo::Line([0.0, 0.0], [w, 0.0]),
                Geo::Line([w, 0.0], [w, h]),
                Geo::Line([w, h], [0.0, h]),
                Geo::Line([0.0, h], [0.0, 0.0]),
            ];
            let err = match_geometry(&exp, &g);
            r.check(
                "inch rectangle -> 50.8 x 25.4 mm (declared unit beats the override)",
                format!("max err <= {FILE_TOL:e}"),
                format!("{err:.2e}"),
                err <= FILE_TOL,
            );
        }
    }
    // notch_freecad_r12.dxf: no $INSUNITS. 40x20 rectangle with a semicircular
    // notch r 5 centred (20,20): area 800 - 12.5 pi.
    if let Some(p) = r.stage("scenario_d_dxf_src", DXF_FIXTURES, "notch_freecad_r12.dxf") {
        let before = r.doc_and_rev();
        let x = r.exec(Command::ImportDxf {
            path: p.clone(),
            plane: SketchPlane::WorldXY,
            name: None,
            units_to_mm: None,
            allow_partial: false,
            layers: None,
        });
        r.check_refused("unitless DXF without a unit choice", &x, &["unit"]);
        r.check_unchanged("unitless: document unchanged", &before);
        let x = r.exec(Command::ImportDxf {
            path: p,
            plane: SketchPlane::WorldXY,
            name: Some("Notch".into()),
            units_to_mm: Some(1.0),
            allow_partial: false,
            layers: None,
        });
        if let Some(f) = r.ok("unitless DXF with units_to_mm = 1", &x) {
            let x = r.exec(Command::Extrude {
                name: None,
                sketch: f,
                distance: 3.0,
                op: ExtrudeOp::NewBody,
                target: None,
                through_all: false,
                reverse: false,
                body_name: Some("Notch plate".into()),
                region: None,
                options: Default::default(),
            });
            r.ok("extrude notch 3 mm", &x);
            if let Some(&id) = r.created_bodies("notch", &x, 1).first() {
                r.check_brep(
                    "notch plate",
                    id,
                    3.0 * (800.0 - 12.5 * PI),
                    [0.0; 3],
                    [40.0, 20.0, 3.0],
                    BOUNDS_TOL,
                );
            }
        }
    }

    r.step = "D.11 native reload without the DXF sources".into();
    let before_doc = r.s.model().map(|m| m.document().clone());
    let before_bodies = body_identity(&r.bodies());
    let sketches_before: Vec<(FeatureId, Option<Sketch>)> =
        r.s.summary()
            .features
            .into_iter()
            .filter(|f| f.kind == "sketch")
            .map(|f| (f.id, f.sketch))
            .collect();
    let _ = std::fs::remove_dir_all(r.out_dir.join("scenario_d_dxf_src"));
    let native = r.out_dir.join("scenario_d_dxf.rcad.json");
    if !r.save_close_open(&native) {
        return;
    }
    let after_doc = r.s.model().map(|m| m.document().clone());
    r.check(
        "document identical",
        "equal",
        if before_doc == after_doc {
            "equal"
        } else {
            "DIFFERENT"
        },
        before_doc.is_some() && before_doc == after_doc,
    );
    let sketches_after: Vec<(FeatureId, Option<Sketch>)> =
        r.s.summary()
            .features
            .into_iter()
            .filter(|f| f.kind == "sketch")
            .map(|f| (f.id, f.sketch))
            .collect();
    r.check(
        "all imported sketches (solved geometry) identical",
        format!("{} sketches", sketches_before.len()),
        format!(
            "{} sketches, equal={}",
            sketches_after.len(),
            sketches_before == sketches_after
        ),
        sketches_before.len() == 4 && sketches_before == sketches_after,
    );
    let after_bodies = body_identity(&r.bodies());
    r.check(
        "bodies identical",
        format!("{before_bodies:?}"),
        format!("{after_bodies:?}"),
        before_bodies.len() == 2 && before_bodies == after_bodies,
    );
    if let Some(id) = slot_body {
        r.check_brep(
            "reopened slot plate",
            id,
            5.0 * (800.0 + 50.0 * PI),
            [-10.0, 0.0, 0.0],
            [40.0, 20.0, 5.0],
            BOUNDS_TOL,
        );
    }
}
