//! SESSION_2 core scenarios A and B, headless, through [`Session::execute`]
//! only, with editor-built sketches (see `crate::scenarios`). Every geometric
//! expectation below is derived by hand in this file (comments show the
//! derivation); none is read back from the implementation.
use super::{BOUNDS_TOL, Run, vol_tol};
use crate::scenarios::{self as sc, CHAMFER_LENGTH, CHAMFER_PLATE, MOVED_TO};
use crate::{Command, Session, SketchTemplate, end_cap_of};
use f420_document::{DocError, FeatureId, SketchPlane};
use f420_sketch::{Sketch, SolveStatus};
use std::f64::consts::PI;

impl Run<'_> {
    fn feature_summary(&self, id: FeatureId) -> Option<crate::FeatureSummary> {
        self.s.summary().features.into_iter().find(|f| f.id == id)
    }

    fn solved_sketch(&self, id: FeatureId) -> Option<Sketch> {
        self.feature_summary(id)?.sketch
    }

    fn doc_and_rev(&self) -> Option<(f420_document::Document, u64)> {
        self.s.model().map(|m| (m.document().clone(), m.revision))
    }

    /// One closed shell, B-rep volume, bounds and (optionally) B-rep topology
    /// counts against independent expectations.
    fn check_solid(&mut self, volume: f64, min: [f64; 3], max: [f64; 3], fev: Option<[usize; 3]>) {
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
                let tol = vol_tol(volume);
                self.check(
                    "B-rep volume (mm^3)",
                    format!("{volume:.9} ± {tol:.3e}"),
                    format!("{v:.9} (err {:.3e})", v - volume),
                    (v - volume).abs() <= tol,
                );
            }
            None => self.check("B-rep volume (mm^3)", volume, "UNAVAILABLE", false),
        }
        let err = (0..3)
            .map(|i| {
                (m.bounds.min[i] - min[i])
                    .abs()
                    .max((m.bounds.max[i] - max[i]).abs())
            })
            .fold(0.0, f64::max);
        self.check(
            "bounds (mm)",
            format!("{min:?}..{max:?} ± {BOUNDS_TOL:e}"),
            format!("{:?}..{:?} (max err {err:.2e})", m.bounds.min, m.bounds.max),
            err <= BOUNDS_TOL,
        );
        if let Some([f, e, v]) = fev {
            let got = [m.face_count, m.edge_count, m.vertex_count];
            self.check(
                "B-rep faces/edges/vertices (Euler V-E+F=2)",
                format!("{:?}", [f, e, v]),
                format!("{got:?}"),
                got == [f, e, v],
            );
        }
    }

    /// STL (watertight, mesh volume within chord error) and STEP (structure).
    fn check_exports(&mut self, stem: &str, volume: f64) {
        let stl = self.out_dir.join(format!("{stem}.stl"));
        let x = self.exec(Command::ExportStl {
            path: stl.clone(),
            chord: Some(0.01),
        });
        self.ok("export_stl", &x);
        match &x.mesh_check {
            Some(mc) => {
                self.check(
                    "STL watertight",
                    true,
                    format!(
                        "{} (boundary {}, nonmanifold {}, misoriented {})",
                        mc.watertight,
                        mc.boundary_edges,
                        mc.nonmanifold_edges,
                        mc.misoriented_edges
                    ),
                    mc.watertight,
                );
                let rel = (mc.mesh_volume - volume).abs() / volume;
                self.check(
                    "STL mesh volume rel err (chord 0.01 mm)",
                    "<= 1e-3",
                    format!("{rel:.3e} (mesh {:.6})", mc.mesh_volume),
                    rel <= 1e-3,
                );
            }
            None => self.check("STL mesh check", "present", "missing", false),
        }
        let step = self.out_dir.join(format!("{stem}.step"));
        let x = self.exec(Command::ExportStep { path: step.clone() });
        self.ok("export_step", &x);
        let text = std::fs::read_to_string(&step).unwrap_or_default();
        let good = text.starts_with("ISO-10303-21;")
            && text.contains("MANIFOLD_SOLID_BREP")
            && !text.contains("INTERSECTION_CURVE");
        self.check(
            "STEP structure (external validation: tools/external_validation.sh)",
            "ISO-10303-21, MANIFOLD_SOLID_BREP, no INTERSECTION_CURVE",
            format!("{} bytes, ok={good}", text.len()),
            good,
        );
    }

    fn check_unchanged(&mut self, name: &str, before: &Option<(f420_document::Document, u64)>) {
        let after = self.doc_and_rev();
        self.check(
            name,
            "document and revision unchanged",
            if &after == before {
                "equal"
            } else {
                "DIFFERENT"
            },
            before.is_some() && &after == before,
        );
    }
}

/// Plate with a centred through hole: w * h * t - pi * r^2 * t.
fn plate_volume(w: f64, h: f64, t: f64, hole_d: f64) -> f64 {
    w * h * t - PI * (hole_d / 2.0).powi(2) * t
}

pub(super) fn scenario_a(r: &mut Run<'_>) {
    r.s = Session::new();
    r.step = "A.1 editor rectangle 80x50 on XY, extrude 10".into();
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    let base = match sc::base_rectangle(80.0, 50.0) {
        Ok((s, _)) => s,
        Err(e) => return r.check("build editor rectangle", "ok", e, false),
    };
    let x = r.exec(Command::CreateSketch {
        name: Some("Base sketch".into()),
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::Custom { sketch: base },
    });
    let Some(s1) = r.ok("create_sketch (custom, editor-built)", &x) else {
        return;
    };
    let fs = r.feature_summary(s1);
    let st = fs.as_ref().and_then(|f| f.solve_status);
    let dof = fs.as_ref().and_then(|f| f.solve_dof);
    r.check(
        "constraint state",
        "FullyConstrained, dof 0",
        format!("{st:?}, dof {dof:?}"),
        st == Some(SolveStatus::FullyConstrained) && dof == Some(0),
    );
    let origin = fs
        .as_ref()
        .and_then(|f| f.sketch.as_ref())
        .and_then(|s| s.origin);
    let params = fs
        .as_ref()
        .map(|f| f.parameters.clone())
        .unwrap_or_default();
    r.check(
        "editor sketch (has origin point; no template center_x/center_y)",
        "origin Some, params [width=80, height=50]",
        format!("origin {origin:?}, params {params:?}"),
        origin.is_some() && params == vec![("width".into(), 80.0), ("height".into(), 50.0)],
    );
    let x = r.exec(Command::Extrude {
        name: Some("Plate".into()),
        sketch: s1,
        distance: 10.0,
    });
    let Some(e1) = r.ok("extrude 10", &x) else {
        return;
    };
    // Box: 6 faces, 12 edges, 8 vertices.
    r.check_solid(
        80.0 * 50.0 * 10.0,
        [-40.0, -25.0, 0.0],
        [40.0, 25.0, 10.0],
        Some([6, 12, 8]),
    );

    r.step = "A.2 Ø10 circle on end cap (centre = sketch origin), cut through all".into();
    let hole = match sc::hole_circle(10.0) {
        Ok(s) => s,
        Err(e) => return r.check("build editor circle", "ok", e, false),
    };
    let x = r.exec(Command::CreateSketch {
        name: Some("Hole sketch".into()),
        plane: end_cap_of(e1),
        template: SketchTemplate::Custom { sketch: hole },
    });
    let Some(s2) = r.ok("create_sketch on end cap", &x) else {
        return;
    };
    let fs = r.feature_summary(s2);
    let st = fs.as_ref().and_then(|f| f.solve_status);
    r.check(
        "hole sketch fully constrained",
        "FullyConstrained",
        format!("{st:?}"),
        st == Some(SolveStatus::FullyConstrained),
    );
    let fz = r
        .feature_summary(s2)
        .and_then(|f| f.frame)
        .map(|f| f.origin);
    r.check(
        "hole sketch frame origin = end-cap centre (0, 0, 10)",
        "[0, 0, 10]",
        format!("{fz:?}"),
        fz.is_some_and(|o| o[0].abs().max(o[1].abs()).max((o[2] - 10.0).abs()) <= BOUNDS_TOL),
    );
    let x = r.exec(Command::CutThroughAll {
        name: Some("Hole".into()),
        sketch: s2,
    });
    if r.ok("cut_through_all", &x).is_none() {
        return;
    }
    r.check_body(80.0, 50.0, 10.0, Some(10.0));

    r.step = "A.3 width -> 100 (hole stays centred), thickness -> 12 (sketch follows)".into();
    let x = r.exec(Command::SetParameter {
        feature: s1,
        name: "width".into(),
        value: 100.0,
    });
    r.ok("set width 100", &x);
    r.check_body(100.0, 50.0, 10.0, Some(10.0));
    let x = r.exec(Command::SetParameter {
        feature: e1,
        name: "distance".into(),
        value: 12.0,
    });
    r.ok("set thickness 12", &x);
    let fz = super::sketch_frame_z(r, s2);
    r.check(
        "hole sketch frame z follows end cap",
        12.0,
        format!("{fz:?}"),
        fz.is_some_and(|z| (z - 12.0).abs() <= BOUNDS_TOL),
    );
    r.check_body(100.0, 50.0, 12.0, Some(10.0));
    let v = r.volume().unwrap_or(f64::NAN);
    let exp = plate_volume(100.0, 50.0, 12.0, 10.0);
    r.check(
        "volume = w*h*t - pi*5^2*t (100, 50, 12)",
        format!("{exp:.9} ± {:.3e}", vol_tol(exp)),
        format!("{v:.9}"),
        (v - exp).abs() <= vol_tol(exp),
    );

    r.step = "A.4 undo/redo, save/close/reopen, exports".into();
    for (i, (w, t)) in [(100.0, 10.0), (80.0, 10.0)].into_iter().enumerate() {
        let x = r.exec(Command::Undo);
        r.ok(&format!("undo {}", i + 1), &x);
        r.check_body(w, 50.0, t, Some(10.0));
    }
    let wp = r.param(s1, "width");
    r.check(
        "width after 2 undos",
        80.0,
        format!("{wp:?}"),
        wp == Some(80.0),
    );
    for (i, (w, t)) in [(100.0, 10.0), (100.0, 12.0)].into_iter().enumerate() {
        let x = r.exec(Command::Redo);
        r.ok(&format!("redo {}", i + 1), &x);
        r.check_body(w, 50.0, t, Some(10.0));
    }
    let (wp, tp) = (r.param(s1, "width"), r.param(e1, "distance"));
    r.check(
        "parameters after 2 redos",
        "width=100, distance=12",
        format!("width={wp:?}, distance={tp:?}"),
        wp == Some(100.0) && tp == Some(12.0),
    );
    let path = r.out_dir.join("scenario_a.f420.json");
    let before = r.s.model().map(|m| m.document().clone());
    let x = r.exec(Command::Save { path: path.clone() });
    r.ok("save", &x);
    let x = r.exec(Command::Close);
    r.ok("close", &x);
    let x = r.exec(Command::Open { path });
    r.ok("open", &x);
    let after = r.s.model().map(|m| m.document().clone());
    r.check(
        "document identical after save/close/reopen",
        "equal",
        if before == after {
            "equal"
        } else {
            "DIFFERENT"
        },
        before.is_some() && before == after,
    );
    let fz = super::sketch_frame_z(r, s2);
    r.check(
        "reopened hole sketch frame z",
        12.0,
        format!("{fz:?}"),
        fz.is_some_and(|z| (z - 12.0).abs() <= BOUNDS_TOL),
    );
    r.check_body(100.0, 50.0, 12.0, Some(10.0));
    r.check_exports("scenario_a", exp);
}

/// Shoelace area (by hand-written formula, independent of the kernel).
fn shoelace(v: &[[f64; 2]]) -> f64 {
    let n = v.len();
    0.5 * (0..n)
        .map(|i| v[i][0] * v[(i + 1) % n][1] - v[(i + 1) % n][0] * v[i][1])
        .sum::<f64>()
}

pub(super) fn scenario_b(r: &mut Run<'_>) {
    r.s = Session::new();
    let t = 5.0;
    // Bounding box of every scenario B profile: x 0..60, y 0..40.
    let (bmin, bmax) = ([0.0, 0.0, 0.0], [60.0, 40.0, t]);
    // Pentagonal prism: 7 faces, 15 edges, 10 vertices.
    let prism = Some([7, 15, 10]);

    r.step = "B.1 free line loop (0,0)(60,0)(60,30)(50,40)(0,40), extrude 5".into();
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    let plate = match sc::chamfer_plate() {
        Ok(s) => s,
        Err(e) => return r.check("build loop", "ok", e, false),
    };
    let x = r.exec(Command::CreateSketch {
        name: Some("Chamfered profile".into()),
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::Custom { sketch: plate },
    });
    let Some(s1) = r.ok("create_sketch", &x) else {
        return;
    };
    let fs = r.feature_summary(s1);
    r.check(
        "loop is a usable profile, under-constrained (free vertices)",
        "profile_error None, UnderConstrained",
        format!(
            "{:?}, {:?}",
            fs.as_ref().and_then(|f| f.profile_error.clone()),
            fs.as_ref().and_then(|f| f.solve_status)
        ),
        fs.as_ref().is_some_and(|f| {
            f.profile_error.is_none() && f.solve_status == Some(SolveStatus::UnderConstrained)
        }),
    );
    // Area: 60 x 40 rectangle minus the 10 x 10 corner triangle = 2400 - 50.
    let area = 60.0 * 40.0 - 0.5 * 10.0 * 10.0;
    r.check(
        "expected area 2350 (hand) = shoelace of the specified vertices",
        2350.0,
        shoelace(&CHAMFER_PLATE),
        area == 2350.0 && (shoelace(&CHAMFER_PLATE) - area).abs() < 1e-9,
    );
    let x = r.exec(Command::Extrude {
        name: Some("Chamfered plate".into()),
        sketch: s1,
        distance: t,
    });
    let Some(e1) = r.ok("extrude 5", &x) else {
        return;
    };
    r.check_solid(area * t, bmin, bmax, prism);
    let v = r.volume().unwrap_or(f64::NAN);
    r.check(
        "B-rep volume / thickness = profile area 2350",
        format!("{area} ± {:.3e}", vol_tol(area * t) / t),
        format!("{:.9}", v / t),
        (v / t - area).abs() <= vol_tol(area * t) / t,
    );
    r.check_exports("scenario_b_initial", area * t);

    r.step = "B.2 edit vertex (50,40) -> (45,40)".into();
    let moved = r
        .solved_sketch(s1)
        .ok_or_else(|| "no solved sketch".to_string())
        .and_then(|s| sc::move_vertex(s, sc::MOVED_VERTEX, MOVED_TO).map_err(|e| e.to_string()));
    let moved = match moved {
        Ok(s) => s,
        Err(e) => return r.check("drag vertex", "Reached", e, false),
    };
    let x = r.exec(Command::EditSketch {
        feature: s1,
        sketch: moved,
    });
    r.ok("edit_sketch (moved vertex)", &x);
    // Corner triangle now has legs 10 (y 30..40) and 15 (x 45..60):
    // area = 2400 - 0.5 * 15 * 10 = 2325.
    let area_moved = 60.0 * 40.0 - 0.5 * 15.0 * 10.0;
    r.check_solid(area_moved * t, bmin, bmax, prism);
    let x = r.exec(Command::Undo);
    r.ok("undo vertex edit (one step)", &x);
    r.check_solid(area * t, bmin, bmax, prism);
    let x = r.exec(Command::Redo);
    r.ok("redo vertex edit", &x);
    r.check_solid(area_moved * t, bmin, bmax, prism);

    r.step = "B.3 driving chamfer-length dimension, edit to 12".into();
    let dimensioned = r
        .solved_sketch(s1)
        .ok_or_else(|| "no solved sketch".to_string())
        .and_then(|s| sc::dimension_chamfer_plate(s).map_err(|e| e.to_string()));
    let dimensioned = match dimensioned {
        Ok(s) => s,
        Err(e) => return r.check("add dimensions", "ok", e, false),
    };
    let x = r.exec(Command::EditSketch {
        feature: s1,
        sketch: dimensioned,
    });
    r.ok("edit_sketch (add constraints + chamfer dimension)", &x);
    let fs = r.feature_summary(s1);
    // Chamfer (60,30)-(45,40): sqrt(15^2 + 10^2) = sqrt(325).
    let ch0 = r.param(s1, "chamfer");
    r.check(
        "chamfer dimension = sqrt(325), sketch fully constrained",
        format!("{:.9}, FullyConstrained", 325f64.sqrt()),
        format!("{ch0:?}, {:?}", fs.as_ref().and_then(|f| f.solve_status)),
        ch0.is_some_and(|c| (c - 325f64.sqrt()).abs() < 1e-9)
            && fs.as_ref().and_then(|f| f.solve_status) == Some(SolveStatus::FullyConstrained),
    );
    r.check_solid(area_moved * t, bmin, bmax, prism);
    let x = r.exec(Command::SetParameter {
        feature: s1,
        name: "chamfer".into(),
        value: CHAMFER_LENGTH,
    });
    r.ok("set chamfer 12", &x);
    // v2 = (60, 30) fixed by width/side; v3 = (x, 40) on the top line.
    // (60 - x)^2 + 10^2 = 12^2  =>  60 - x = sqrt(44); triangle legs 10, sqrt(44).
    let run = (CHAMFER_LENGTH * CHAMFER_LENGTH - 100.0).sqrt();
    let area_ch = 60.0 * 40.0 - 0.5 * 10.0 * run;
    let v3 = r
        .solved_sketch(s1)
        .and_then(|s| sc::polygon_vertices(&s).get(3).and_then(|&p| s.point_xy(p)));
    r.check(
        "chamfer top vertex at (60 - sqrt(44), 40)",
        format!("[{:.9}, 40]", 60.0 - run),
        format!("{v3:?}"),
        v3.is_some_and(|p| (p[0] - (60.0 - run)).abs() <= 1e-8 && (p[1] - 40.0).abs() <= 1e-8),
    );
    r.check_solid(area_ch * t, bmin, bmax, prism);

    r.step = "B.4 invalid loops rejected, document preserved".into();
    let before = r.doc_and_rev();
    for (label, sk) in [
        ("self-intersecting", sc::bowtie()),
        ("open", sc::open_loop()),
    ] {
        let Ok(sk) = sk else {
            r.check(&format!("build {label} loop"), "ok", "error", false);
            continue;
        };
        let x = r.exec(Command::EditSketch {
            feature: s1,
            sketch: sk,
        });
        let detail = x.error.as_ref().map(|e| e.to_string()).unwrap_or_default();
        r.check(
            &format!("{label} loop: extrude regeneration fails with Validation"),
            "Validation error naming the profile problem",
            &detail,
            !x.ok && matches!(x.error, Some(DocError::Validation(_))) && detail.contains("profile"),
        );
    }
    r.check_unchanged("after rejected edits", &before);
    r.check_solid(area_ch * t, bmin, bmax, prism);
    let path = r.out_dir.join("scenario_b.f420.json");
    let before_doc = r.s.model().map(|m| m.document().clone());
    let x = r.exec(Command::Save { path: path.clone() });
    r.ok("save", &x);
    let x = r.exec(Command::Close);
    r.ok("close", &x);
    let x = r.exec(Command::Open { path });
    r.ok("open", &x);
    let after_doc = r.s.model().map(|m| m.document().clone());
    r.check(
        "document identical after save/close/reopen",
        "equal",
        if before_doc == after_doc {
            "equal"
        } else {
            "DIFFERENT"
        },
        before_doc.is_some() && before_doc == after_doc,
    );
    r.check_solid(area_ch * t, bmin, bmax, prism);
    r.check_exports("scenario_b", area_ch * t);
    let _ = e1;

    r.step = "B.5 new document: extrude of an invalid loop fails, revision unchanged".into();
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    for (label, sk) in [
        ("self-intersecting", sc::bowtie()),
        ("open", sc::open_loop()),
    ] {
        let Ok(sk) = sk else {
            r.check(&format!("build {label} loop"), "ok", "error", false);
            continue;
        };
        let x = r.exec(Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::Custom { sketch: sk },
        });
        let Some(sid) = r.ok(&format!("{label} sketch is kept (not extrudable)"), &x) else {
            continue;
        };
        let pe = r.feature_summary(sid).and_then(|f| f.profile_error);
        r.check(
            &format!("{label}: profile_error reported"),
            "Some(..)",
            format!("{pe:?}"),
            pe.is_some(),
        );
        let before = r.doc_and_rev();
        let x = r.exec(Command::Extrude {
            name: None,
            sketch: sid,
            distance: t,
        });
        r.check(
            &format!("{label}: extrude rejected"),
            "Validation error",
            format!("{:?}", x.error),
            !x.ok && matches!(x.error, Some(DocError::Validation(_))),
        );
        r.check_unchanged(&format!("{label}: after rejected extrude"), &before);
        let body = r.s.model().map(|m| m.evaluation().body.is_some());
        r.check(
            "no body created",
            false,
            format!("{body:?}"),
            body == Some(false),
        );
        // Remove the bad sketch so the next case starts from an empty history.
        let x = r.exec(Command::Undo);
        r.ok("undo sketch", &x);
    }
}
