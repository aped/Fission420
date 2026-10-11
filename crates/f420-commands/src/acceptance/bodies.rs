//! SESSION_3 scenarios A (multi-body, explicit booleans) and B (side-face
//! sketch, regeneration, broken reference), headless through
//! [`Session::execute`] only. Step prefixes "3A." / "3B.". Expected values are
//! derived by hand in the comments; none is read back from the implementation.
use super::{BOUNDS_TOL, Run, vol_tol};
use crate::{BodySummary, Command, ExtrudeOp, SketchTemplate};
use f420_document::{BodyId, DocError, FaceRef, FaceRole, FeatureId, SketchPlane};
use f420_kernel::{Frame, VolumeResult};

impl Run<'_> {
    fn body(&self, id: BodyId) -> Option<BodySummary> {
        self.s.summary().bodies.into_iter().find(|b| b.id == id)
    }

    fn body_volume(&self, id: BodyId) -> Option<f64> {
        match self.body(id)?.measurements?.volume {
            VolumeResult::Value { volume, .. } => Some(volume),
            VolumeResult::Unavailable { .. } => None,
        }
    }

    /// One closed shell with the analytic volume and bounds.
    fn check_body_solid(
        &mut self,
        label: &str,
        id: BodyId,
        volume: f64,
        min: [f64; 3],
        max: [f64; 3],
    ) {
        let Some(b) = self.body(id) else {
            self.check(&format!("{label}: exists"), id, "missing", false);
            return;
        };
        let Some(m) = b.measurements.clone() else {
            self.check(
                &format!("{label}: measurements"),
                "present",
                "missing",
                false,
            );
            return;
        };
        self.check(
            &format!("{label}: one closed shell"),
            "1, closed",
            format!("{}, {}", m.shell_count, m.all_shells_closed),
            m.shell_count == 1 && m.all_shells_closed,
        );
        let tol = vol_tol(volume);
        match self.body_volume(id) {
            Some(v) => self.check(
                &format!("{label}: volume (mm^3)"),
                format!("{volume:.9} ± {tol:.3e}"),
                format!("{v:.9} (err {:.3e})", v - volume),
                (v - volume).abs() <= tol,
            ),
            None => self.check(
                &format!("{label}: volume (mm^3)"),
                volume,
                "UNAVAILABLE",
                false,
            ),
        }
        let err = (0..3)
            .map(|i| {
                (m.bounds.min[i] - min[i])
                    .abs()
                    .max((m.bounds.max[i] - max[i]).abs())
            })
            .fold(0.0, f64::max);
        self.check(
            &format!("{label}: bounds (mm)"),
            format!("{min:?}..{max:?} ± {BOUNDS_TOL:e}"),
            format!("{:?}..{:?} (max err {err:.2e})", m.bounds.min, m.bounds.max),
            err <= BOUNDS_TOL,
        );
    }

    fn polygon(&mut self, name: &str, plane: SketchPlane, v: &[[f64; 2]]) -> Option<FeatureId> {
        let x = self.exec(Command::CreateSketch {
            name: Some(name.into()),
            plane,
            template: SketchTemplate::Polygon {
                vertices: v.to_vec(),
            },
        });
        self.ok(&format!("sketch {name}"), &x)
    }

    #[allow(clippy::too_many_arguments)]
    fn extrude_op(
        &mut self,
        name: &str,
        sketch: FeatureId,
        distance: f64,
        op: ExtrudeOp,
        target: Option<BodyId>,
        through_all: bool,
        body_name: Option<&str>,
    ) -> crate::Response {
        self.exec(Command::Extrude {
            name: Some(name.into()),
            sketch,
            distance,
            op,
            target,
            through_all,
            reverse: false,
            body_name: body_name.map(Into::into),
        })
    }

    /// The document and revision are unchanged by a refused command.
    fn refused(&mut self, name: &str, cmd: Command, must_contain: &str) {
        let before = self.doc_and_rev();
        let x = self.exec(cmd);
        let msg = x.error.as_ref().map(|e| e.to_string()).unwrap_or_default();
        self.check(
            &format!("{name}: refused"),
            format!("error containing '{must_contain}'"),
            if x.ok { "accepted".into() } else { msg.clone() },
            !x.ok && msg.contains(must_contain),
        );
        let after = self.doc_and_rev();
        self.check(
            &format!("{name}: document and revision unchanged"),
            true,
            before == after,
            before == after,
        );
    }

    fn body_count(&self) -> usize {
        self.s.summary().bodies.len()
    }
}

fn frame_close(a: &Frame, b: &Frame) -> bool {
    let c = |p: [f64; 3], q: [f64; 3]| (0..3).all(|i| (p[i] - q[i]).abs() <= 1e-9);
    c(a.origin, b.origin) && c(a.x, b.x) && c(a.y, b.y) && c(a.z, b.z)
}

/// Scenario A: two boxes as separate bodies, a join and a non-circular cut on
/// explicit targets, undo/redo, save/reload, multi-body STEP export.
pub(super) fn scenario_a(r: &mut Run<'_>) {
    r.s = crate::Session::new();
    r.step = "3A.1 two bodies".into();
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    let Some(sa) = r.polygon(
        "A profile",
        SketchPlane::WorldXY,
        &[[0.0, 0.0], [40.0, 0.0], [40.0, 30.0], [0.0, 30.0]],
    ) else {
        return;
    };
    let x = r.extrude_op(
        "A",
        sa,
        10.0,
        ExtrudeOp::NewBody,
        None,
        false,
        Some("Body A"),
    );
    let Some(ea) = r.ok("extrude A (new body)", &x) else {
        return;
    };
    let Some(sb) = r.polygon(
        "B profile",
        SketchPlane::WorldXY,
        &[[50.0, 0.0], [70.0, 0.0], [70.0, 20.0], [50.0, 20.0]],
    ) else {
        return;
    };
    let x = r.extrude_op(
        "B",
        sb,
        10.0,
        ExtrudeOp::NewBody,
        None,
        false,
        Some("Body B"),
    );
    r.ok("extrude B (new body)", &x);
    let bodies = r.s.summary().bodies;
    r.check(
        "two named bodies",
        "[Body A, Body B]",
        format!("{:?}", bodies.iter().map(|b| &b.name).collect::<Vec<_>>()),
        bodies.len() == 2 && bodies[0].name == "Body A" && bodies[1].name == "Body B",
    );
    if bodies.len() != 2 {
        return;
    }
    let (a, b) = (bodies[0].id, bodies[1].id);
    // A: 40*30*10 = 12000. B: 20*20*10 = 4000.
    r.check_body_solid("A", a, 12000.0, [0.0, 0.0, 0.0], [40.0, 30.0, 10.0]);
    r.check_body_solid("B", b, 4000.0, [50.0, 0.0, 0.0], [70.0, 20.0, 10.0]);

    r.step = "3A.2 hide B, save, reopen".into();
    let x = r.exec(Command::SetBodyVisible {
        body: b,
        visible: false,
    });
    r.ok("hide B", &x);
    r.check(
        "hidden body still evaluated",
        2,
        r.body_count(),
        r.body_count() == 2,
    );
    r.check_body_solid(
        "B (hidden)",
        b,
        4000.0,
        [50.0, 0.0, 0.0],
        [70.0, 20.0, 10.0],
    );
    let path = r.out_dir.join("bodies_a.f420.json");
    let x = r.exec(Command::Save { path: path.clone() });
    r.ok("save", &x);
    let x = r.exec(Command::Open { path: path.clone() });
    r.ok("open", &x);
    let bodies = r.s.summary().bodies;
    r.check(
        "reload keeps ids, names, visibility",
        format!("{a} Body A visible, {b} Body B hidden"),
        format!(
            "{:?}",
            bodies
                .iter()
                .map(|x| (x.id, &x.name, x.visible))
                .collect::<Vec<_>>()
        ),
        bodies.len() == 2
            && (bodies[0].id, bodies[0].name.as_str(), bodies[0].visible) == (a, "Body A", true)
            && (bodies[1].id, bodies[1].name.as_str(), bodies[1].visible) == (b, "Body B", false),
    );
    // Default export policy: visible bodies only (stated in the dialog).
    let one = r.out_dir.join("bodies_a_visible.step");
    let x = r.exec(Command::ExportStep {
        path: one.clone(),
        bodies: None,
    });
    r.ok("export STEP (visible bodies)", &x);
    let solids = std::fs::read_to_string(&one)
        .unwrap_or_default()
        .matches("MANIFOLD_SOLID_BREP(")
        .count();
    r.check(
        "visible-only STEP export has 1 solid",
        1,
        solids,
        solids == 1,
    );
    let x = r.exec(Command::SetBodyVisible {
        body: b,
        visible: true,
    });
    r.ok("show B", &x);

    r.step = "3A.3 join boss on A's top face".into();
    let top = SketchPlane::Face(FaceRef {
        feature: ea,
        role: FaceRole::ExtrudeEndCap,
    });
    // End-cap frame = sketch frame offset by 10: local (x, y) = world (x, y, 10).
    let Some(sboss) = r.polygon(
        "boss",
        top,
        &[[5.0, 5.0], [25.0, 5.0], [25.0, 15.0], [5.0, 15.0]],
    ) else {
        return;
    };
    r.refused(
        "join without target, two bodies, sketch on XY",
        Command::Extrude {
            name: None,
            sketch: sa,
            distance: 5.0,
            op: ExtrudeOp::Join,
            target: None,
            through_all: false,
            reverse: false,
            body_name: None,
        },
        "choose a target body",
    );
    let x = r.extrude_op("Boss", sboss, 5.0, ExtrudeOp::Join, Some(a), false, None);
    let Some(join) = r.ok("join boss into A", &x) else {
        return;
    };
    // 12000 + 20*10*5 = 13000; z max 15.
    r.check_body_solid("A + boss", a, 13000.0, [0.0, 0.0, 0.0], [40.0, 30.0, 15.0]);
    r.check_body_solid(
        "B unchanged",
        b,
        4000.0,
        [50.0, 0.0, 0.0],
        [70.0, 20.0, 10.0],
    );

    r.step = "3A.4 rectangular cut through A".into();
    // 10x5 rectangle away from the boss (boss spans x 5..25, y 5..15).
    let Some(scut) = r.polygon(
        "slot",
        top,
        &[[28.0, 20.0], [38.0, 20.0], [38.0, 25.0], [28.0, 25.0]],
    ) else {
        return;
    };
    let x = r.extrude_op("Slot cut", scut, 10.0, ExtrudeOp::Cut, Some(a), false, None);
    let Some(cut) = r.ok("cut 10x5 through A's 10 mm thickness", &x) else {
        return;
    };
    // 13000 - 10*5*10 = 12500.
    r.check_body_solid("A - slot", a, 12500.0, [0.0, 0.0, 0.0], [40.0, 30.0, 15.0]);
    r.check_body_solid(
        "B unchanged",
        b,
        4000.0,
        [50.0, 0.0, 0.0],
        [70.0, 20.0, 10.0],
    );
    let deps = r
        .feature_summary(cut)
        .map(|f| f.depends_on)
        .unwrap_or_default();
    r.check(
        "cut depends on its sketch and A's creator",
        format!("[{scut}, {ea}]"),
        format!("{deps:?}"),
        deps.contains(&scut) && deps.contains(&ea),
    );

    r.step = "3A.5 undo/redo".into();
    let x = r.exec(Command::Undo);
    r.ok("undo cut", &x);
    r.check_body_solid(
        "A after undo cut",
        a,
        13000.0,
        [0.0, 0.0, 0.0],
        [40.0, 30.0, 15.0],
    );
    let x = r.exec(Command::Undo);
    r.ok("undo cut sketch", &x);
    let x = r.exec(Command::Undo);
    r.ok("undo join", &x);
    r.check_body_solid(
        "A after undo join",
        a,
        12000.0,
        [0.0, 0.0, 0.0],
        [40.0, 30.0, 10.0],
    );
    for _ in 0..3 {
        let x = r.exec(Command::Redo);
        r.ok("redo", &x);
    }
    r.check_body_solid(
        "A after redo",
        a,
        12500.0,
        [0.0, 0.0, 0.0],
        [40.0, 30.0, 15.0],
    );
    let ids: Vec<BodyId> = r.s.summary().bodies.iter().map(|x| x.id).collect();
    r.check(
        "body ids stable through undo/redo",
        format!("[{a}, {b}]"),
        format!("{ids:?}"),
        ids == vec![a, b],
    );
    let fj = r.feature_summary(join).and_then(|f| f.body);
    r.check(
        "join feature acts on A",
        a,
        format!("{fj:?}"),
        fj == Some(a),
    );

    r.step = "3A.6 refusals keep the model".into();
    let Some(far) = r.polygon(
        "far",
        SketchPlane::WorldXY,
        &[
            [100.0, 100.0],
            [110.0, 100.0],
            [110.0, 110.0],
            [100.0, 110.0],
        ],
    ) else {
        return;
    };
    r.refused(
        "join not touching the target",
        Command::Extrude {
            name: None,
            sketch: far,
            distance: 5.0,
            op: ExtrudeOp::Join,
            target: Some(a),
            through_all: false,
            reverse: false,
            body_name: None,
        },
        "does not touch",
    );
    r.refused(
        "cut missing the target",
        Command::Extrude {
            name: None,
            sketch: far,
            distance: 5.0,
            op: ExtrudeOp::Cut,
            target: Some(b),
            through_all: false,
            reverse: false,
            body_name: None,
        },
        "does not intersect",
    );
    r.refused(
        "delete A's creating extrusion while used",
        Command::DeleteFeature { feature: ea },
        "is used by",
    );

    r.step = "3A.7 export both bodies".into();
    let step = r.out_dir.join("bodies_a.step");
    let x = r.exec(Command::ExportStep {
        path: step.clone(),
        bodies: Some(vec![a, b]),
    });
    r.ok("export STEP (A, B)", &x);
    let text = std::fs::read_to_string(&step).unwrap_or_default();
    let solids = text.matches("MANIFOLD_SOLID_BREP(").count();
    r.check(
        "STEP has two solids (external validation: tools/external_validation.sh)",
        2,
        solids,
        solids == 2,
    );
    let stl = r.out_dir.join("bodies_a.stl");
    let x = r.exec(Command::ExportStl {
        path: stl,
        chord: Some(0.01),
        bodies: Some(vec![a, b]),
    });
    r.ok("export STL (A, B; separate shells, not fused)", &x);
    // Re-read with the STEP importer in a fresh session (own reader; the
    // independent check is FreeCAD in tools/external_validation.sh).
    let mut s2 = crate::Session::new();
    s2.execute(&Command::NewDocument);
    let x = s2.execute(&Command::ImportStep {
        path: step,
        units_to_mm: None,
    });
    let vols: Vec<f64> = x
        .summary
        .map(|s| s.bodies)
        .unwrap_or_default()
        .iter()
        .filter_map(|b| match b.measurements.as_ref()?.volume {
            VolumeResult::Value { volume, .. } => Some(volume),
            _ => None,
        })
        .collect();
    let good = vols.len() == 2
        && (vols[0] - 12500.0).abs() <= vol_tol(12500.0)
        && (vols[1] - 4000.0).abs() <= vol_tol(4000.0);
    r.check(
        "re-import: two solids, volumes [12500, 4000]",
        "[12500, 4000]",
        format!("{vols:?}"),
        good,
    );
}

/// Scenario B: side-face sketch, blind cut, parametric edit moving the support,
/// then a topology change that removes the support (explicit reference error)
/// and its repair.
pub(super) fn scenario_b(r: &mut Run<'_>) {
    r.s = crate::Session::new();
    r.step = "3B.1 box 60x40x20".into();
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    // Centered rectangle with its centre at (30, 20): box spans [0,60]x[0,40].
    let x = r.exec(Command::CreateSketch {
        name: Some("Box profile".into()),
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::CenteredRectangle {
            width: 60.0,
            height: 40.0,
            center: Some([30.0, 20.0]),
        },
    });
    let Some(sbox) = r.ok("sketch", &x) else {
        return;
    };
    let x = r.extrude_op("Box", sbox, 20.0, ExtrudeOp::NewBody, None, false, None);
    let Some(ebox) = r.ok("extrude 20", &x) else {
        return;
    };
    let body =
        r.s.summary()
            .bodies
            .first()
            .map(|b| b.id)
            .unwrap_or_default();
    r.check_body_solid("box", body, 48000.0, [0.0, 0.0, 0.0], [60.0, 40.0, 20.0]);

    r.step = "3B.2 sketch on the x=60 side face".into();
    // The side face generated by the profile line on x = 60 (local x = +30).
    let side = r.feature_summary(ebox).and_then(|f| {
        f.faces.into_iter().find(|fs| {
            matches!(fs.reference.role, FaceRole::ExtrudeSide { .. })
                && (fs.frame.z[0] - 1.0).abs() < 1e-12
                && (fs.frame.origin[0] - 60.0).abs() < 1e-9
        })
    });
    let Some(side) = side else {
        r.check("side face x=60 offered", "present", "missing", false);
        return;
    };
    // Right-handed frame from definition: x along the CCW edge (+Y), z outward
    // (+X), y = z × x = +Z, origin at the edge start (60, 0, 0).
    let want = Frame {
        origin: [60.0, 0.0, 0.0],
        x: [0.0, 1.0, 0.0],
        y: [0.0, 0.0, 1.0],
        z: [1.0, 0.0, 0.0],
    };
    r.check(
        "side-face frame",
        format!("{want:?}"),
        format!("{:?}", side.frame),
        frame_close(&side.frame, &want),
    );
    // 10 (along y) x 6 (along z) centred at world y=20, z=10 -> local (20, 10).
    let x = r.exec(Command::CreateSketch {
        name: Some("Side slot".into()),
        plane: SketchPlane::Face(side.reference),
        template: SketchTemplate::CenteredRectangle {
            width: 10.0,
            height: 6.0,
            center: Some([20.0, 10.0]),
        },
    });
    let Some(sslot) = r.ok("sketch on side face", &x) else {
        return;
    };
    let x = r.extrude_op(
        "Side cut",
        sslot,
        15.0,
        ExtrudeOp::Cut,
        Some(body),
        false,
        None,
    );
    let Some(cut) = r.ok("blind cut 15 inward (default direction into material)", &x) else {
        return;
    };
    // 48000 - 10*6*15 = 47100. Bounds unchanged (pocket is interior to the face).
    r.check_body_solid(
        "box - side pocket",
        body,
        47100.0,
        [0.0, 0.0, 0.0],
        [60.0, 40.0, 20.0],
    );
    // Placement: the pocket floor is the cut's end cap at x = 60 - 15 = 45,
    // normal pointing back toward the opening (+X); its frame origin is the
    // sketch-frame origin (60, 0, 0) moved 15 along -X.
    let floor = r.feature_summary(cut).and_then(|f| {
        f.faces
            .into_iter()
            .find(|fs| fs.reference.role == FaceRole::ExtrudeEndCap)
    });
    let floor_ok = floor.as_ref().is_some_and(|f| {
        (f.frame.origin[0] - 45.0).abs() < 1e-9
            && f.frame.origin[1].abs() < 1e-9
            && f.frame.origin[2].abs() < 1e-9
            && (f.frame.z[0] - 1.0).abs() < 1e-12
    });
    r.check(
        "pocket floor at x=45, normal +X",
        "origin (45,0,0), z +X",
        format!("{:?}", floor.map(|f| f.frame)),
        floor_ok,
    );

    r.step = "3B.3 length 60 -> 70".into();
    for (p, v) in [("width", 70.0), ("center_x", 35.0)] {
        let x = r.exec(Command::SetParameter {
            feature: sbox,
            name: p.into(),
            value: v,
        });
        r.ok(&format!("set {p} = {v}"), &x);
    }
    // 70*40*20 - 900 = 55100.
    r.check_body_solid(
        "box 70 - side pocket",
        body,
        55100.0,
        [0.0, 0.0, 0.0],
        [70.0, 40.0, 20.0],
    );
    let moved = r.feature_summary(sslot).and_then(|f| f.frame);
    let want70 = Frame {
        origin: [70.0, 0.0, 0.0],
        ..want
    };
    r.check(
        "support moved to x=70, same orientation",
        format!("{want70:?}"),
        format!("{moved:?}"),
        moved.is_some_and(|m| frame_close(&m, &want70)),
    );
    let floor = r.feature_summary(cut).and_then(|f| {
        f.faces
            .into_iter()
            .find(|fs| fs.reference.role == FaceRole::ExtrudeEndCap)
    });
    r.check(
        "pocket floor follows to x=55",
        55.0,
        format!("{:?}", floor.as_ref().map(|f| f.frame.origin[0])),
        floor.is_some_and(|f| (f.frame.origin[0] - 55.0).abs() < 1e-9),
    );

    let x = r.exec(Command::ExportStep {
        path: r.out_dir.join("side_pocket.step"),
        bodies: None,
    });
    r.ok(
        "export STEP (external check: tools/check_run3_steps.sh)",
        &x,
    );

    r.step = "3B.4 removing the support is an explicit reference error".into();
    // Upstream topology edit in the editor's terms: delete the profile line that
    // generates the x=70 face and close the outline through a new apex point
    // (two new lines with fresh ids). The side sketch must not re-attach.
    let FaceRole::ExtrudeSide { entity } = side.reference.role else {
        return;
    };
    let Some(mut edited) = r.solved_sketch(sbox) else {
        return;
    };
    let before = r.doc_and_rev();
    let right = f420_sketch::EntityId(entity);
    let ends = edited.entity(right).map(|e| e.points()).unwrap_or_default();
    edited.remove_entity(right);
    if let [p0, p1] = ends[..] {
        let apex = edited.add_point(80.0, 20.0);
        edited.add_line(p0, apex);
        edited.add_line(apex, p1);
    }
    let x = r.exec(Command::EditSketch {
        feature: sbox,
        sketch: edited,
    });
    let msg = x.error.as_ref().map(|e| e.to_string()).unwrap_or_default();
    let broken = matches!(x.error, Some(DocError::BrokenReference { .. }));
    r.check(
        "edit removing the support face -> BrokenReference (no silent re-attach)",
        "BrokenReference naming the side face",
        if x.ok { "accepted".into() } else { msg.clone() },
        !x.ok && broken && msg.contains("no longer"),
    );
    r.check(
        "model kept",
        true,
        r.doc_and_rev() == before,
        r.doc_and_rev() == before,
    );

    r.step = "3B.5 repair by re-attaching".into();
    // Explicit repair: re-attach the slot sketch to the box's top face (end
    // cap: world XY frame at z = 20), keeping its local geometry. Local (20, 10)
    // -> world (20, 10, 20); the 10x6 pocket, 15 deep into the 20 mm plate,
    // removes the same 900 mm^3: 70*40*20 - 900 = 55100.
    let x = r.exec(Command::SetSketchPlane {
        feature: sslot,
        plane: SketchPlane::Face(FaceRef {
            feature: ebox,
            role: FaceRole::ExtrudeEndCap,
        }),
    });
    r.ok("re-attach side sketch to the top face", &x);
    r.check_body_solid(
        "re-attached pocket on top",
        body,
        55100.0,
        [0.0, 0.0, 0.0],
        [70.0, 40.0, 20.0],
    );
    // With the dependency repaired, the upstream edit is accepted. The apex
    // adds the triangle (70,0)-(80,20)-(70,40): 0.5*40*10*20 = 4000.
    if let Some(mut edited) = r.solved_sketch(sbox) {
        let ends = edited
            .entity(f420_sketch::EntityId(entity))
            .map(|e| e.points())
            .unwrap_or_default();
        edited.remove_entity(f420_sketch::EntityId(entity));
        if let [p0, p1] = ends[..] {
            let apex = edited.add_point(80.0, 20.0);
            edited.add_line(p0, apex);
            edited.add_line(apex, p1);
        }
        let x = r.exec(Command::EditSketch {
            feature: sbox,
            sketch: edited,
        });
        r.ok("upstream edit after repair", &x);
        r.check_body_solid(
            "pointed box - top pocket",
            body,
            59100.0,
            [0.0, 0.0, 0.0],
            [80.0, 40.0, 20.0],
        );
        let x = r.exec(Command::Undo);
        r.ok("undo upstream edit", &x);
    }
    let x = r.exec(Command::Undo);
    r.ok("undo re-attach (one step)", &x);
    let back = r.feature_summary(sslot).and_then(|f| f.plane);
    r.check(
        "plane restored to the side face",
        format!("{:?}", side.reference),
        format!("{back:?}"),
        back == Some(SketchPlane::Face(side.reference)),
    );
}

/// Backlog 1: a rectangle with an inner circle and an inner rectangle is one
/// profile region; one New Body extrusion makes a plate with two through holes.
pub(super) fn scenario_nested(r: &mut Run<'_>) {
    use std::f64::consts::PI;
    r.s = crate::Session::new();
    r.step = "3N.1 plate with holes from one sketch".into();
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    let mut sk = f420_sketch::Sketch::default();
    sk.ensure_origin();
    let quad = |sk: &mut f420_sketch::Sketch, v: [[f64; 2]; 4]| {
        let p: Vec<_> = v.iter().map(|q| sk.add_point(q[0], q[1])).collect();
        for i in 0..4 {
            sk.add_line(p[i], p[(i + 1) % 4]);
        }
    };
    quad(
        &mut sk,
        [[0.0, 0.0], [80.0, 0.0], [80.0, 50.0], [0.0, 50.0]],
    );
    let c = sk.add_point(20.0, 25.0);
    sk.add_circle(c, 5.0);
    quad(
        &mut sk,
        [[50.0, 22.0], [60.0, 22.0], [60.0, 28.0], [50.0, 28.0]],
    );
    let x = r.exec(Command::CreateSketch {
        name: Some("Plate with holes".into()),
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::Custom { sketch: sk },
    });
    let Some(sid) = r.ok("sketch: outer + circle + slot loops", &x) else {
        return;
    };
    let x = r.exec(Command::extrude(Some("Plate".into()), sid, 10.0));
    r.ok("extrude (new body)", &x);
    let Some(b) = r.s.summary().bodies.first().map(|b| b.id) else {
        return;
    };
    // (80*50 - pi*5^2 - 10*6) * 10.
    let want = (4000.0 - 25.0 * PI - 60.0) * 10.0;
    r.check_body_solid(
        "plate with holes",
        b,
        want,
        [0.0, 0.0, 0.0],
        [80.0, 50.0, 10.0],
    );
    let faces =
        r.s.model()
            .map(|m| m.evaluation().bodies[0].measurements.clone());
    let fc = faces.flatten().map(|m| m.face_count).unwrap_or(0);
    r.check(
        "faces: 6 plate + hole wall(s) + 4 slot walls",
        ">= 11",
        fc,
        fc >= 11,
    );
    let step = r.out_dir.join("nested_loops.step");
    let x = r.exec(Command::ExportStep {
        path: step.clone(),
        bodies: None,
    });
    r.ok("export STEP", &x);
    let mut s2 = crate::Session::new();
    s2.execute(&Command::NewDocument);
    let x = s2.execute(&Command::ImportStep {
        path: step,
        units_to_mm: None,
    });
    let v = x
        .summary
        .and_then(|s| s.bodies.first().and_then(|b| b.measurements.clone()))
        .and_then(|m| match m.volume {
            VolumeResult::Value { volume, .. } => Some(volume),
            _ => None,
        });
    r.check(
        "STEP re-import volume",
        format!("{want:.6}"),
        format!("{v:?}"),
        v.is_some_and(|v| (v - want).abs() <= vol_tol(want)),
    );
}

/// Backlog 3: a row of holes as a linear pattern of one Extrude Cut, with
/// editable count and spacing.
pub(super) fn scenario_pattern(r: &mut Run<'_>) {
    use std::f64::consts::PI;
    r.s = crate::Session::new();
    r.step = "3P.1 row of holes".into();
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    let Some(sp) = r.polygon(
        "Bar",
        SketchPlane::WorldXY,
        &[[0.0, 0.0], [80.0, 0.0], [80.0, 20.0], [0.0, 20.0]],
    ) else {
        return;
    };
    let x = r.extrude_op("Bar", sp, 5.0, ExtrudeOp::NewBody, None, false, None);
    let Some(bar) = r.ok("extrude bar 80x20x5", &x) else {
        return;
    };
    let x = r.exec(Command::CreateSketch {
        name: Some("Hole".into()),
        plane: SketchPlane::Face(FaceRef {
            feature: bar,
            role: FaceRole::ExtrudeEndCap,
        }),
        template: SketchTemplate::CenteredCircle {
            diameter: 5.0,
            center: Some([10.0, 10.0]),
        },
    });
    let Some(sh) = r.ok("hole sketch on top", &x) else {
        return;
    };
    let x = r.extrude_op("Hole", sh, 0.0, ExtrudeOp::Cut, None, true, None);
    let Some(hole) = r.ok("through-all cut", &x) else {
        return;
    };
    let x = r.exec(Command::LinearPattern {
        name: Some("Row".into()),
        feature: hole,
        count: 5,
        spacing: 15.0,
        direction: [1.0, 0.0],
    });
    let Some(row) = r.ok("pattern x5 at 15 mm along sketch x", &x) else {
        return;
    };
    let Some(b) = r.s.summary().bodies.first().map(|b| b.id) else {
        return;
    };
    let v = |n: f64| 80.0 * 20.0 * 5.0 - n * PI * 2.5 * 2.5 * 5.0;
    r.check_body_solid(
        "bar with 5 holes",
        b,
        v(5.0),
        [0.0, 0.0, 0.0],
        [80.0, 20.0, 5.0],
    );

    r.step = "3P.2 edit count and spacing".into();
    let x = r.exec(Command::SetParameter {
        feature: row,
        name: "count".into(),
        value: 3.0,
    });
    r.ok("count = 3", &x);
    r.check_body_solid(
        "bar with 3 holes",
        b,
        v(3.0),
        [0.0, 0.0, 0.0],
        [80.0, 20.0, 5.0],
    );
    let x = r.exec(Command::SetParameter {
        feature: row,
        name: "spacing".into(),
        value: 30.0,
    });
    r.ok("spacing = 30 (holes at x = 10, 40, 70)", &x);
    r.check_body_solid(
        "bar with 3 holes at 30",
        b,
        v(3.0),
        [0.0, 0.0, 0.0],
        [80.0, 20.0, 5.0],
    );

    // Placement: hole loops on the top face (z = 5) centred at x = 10, 40, 70, y = 10.
    let loops =
        r.s.model()
            .and_then(|m| {
                m.evaluation()
                    .body_by_id(b)
                    .and_then(|e| e.geometry.brep().cloned())
            })
            .map(|body| f420_kernel::z_face_inner_loops(&body));
    let mut centres: Vec<[f64; 2]> = match loops {
        Some(Ok(l)) => l
            .iter()
            .filter(|l| (l.z - 5.0).abs() < 1e-9)
            .map(|l| l.centroid_xy)
            .collect(),
        _ => vec![],
    };
    centres.sort_by(|p, q| p[0].total_cmp(&q[0]));
    let want = [[10.0, 10.0], [40.0, 10.0], [70.0, 10.0]];
    let placed = centres.len() == 3
        && centres
            .iter()
            .zip(want)
            .all(|(c, w)| (c[0] - w[0]).abs() < 1e-6 && (c[1] - w[1]).abs() < 1e-6);
    r.check(
        "hole centres on top face",
        format!("{want:?}"),
        format!("{centres:?}"),
        placed,
    );
    r.refused(
        "count 4 at 30 puts a hole at x = 100 (outside)",
        Command::SetParameter {
            feature: row,
            name: "count".into(),
            value: 4.0,
        },
        "instance 4 of 4",
    );
    r.refused(
        "non-integer count",
        Command::SetParameter {
            feature: row,
            name: "count".into(),
            value: 2.5,
        },
        "integer",
    );
    r.refused(
        "delete the seed while patterned",
        Command::DeleteFeature { feature: hole },
        "is used by",
    );
    let x = r.exec(Command::ExportStep {
        path: r.out_dir.join("pattern.step"),
        bodies: None,
    });
    r.ok(
        "export STEP (external check: tools/check_run3_steps.sh)",
        &x,
    );
    let path = r.out_dir.join("pattern.f420.json");
    let x = r.exec(Command::Save { path: path.clone() });
    r.ok("save", &x);
    let x = r.exec(Command::Open { path });
    r.ok("reopen", &x);
    r.check_body_solid("reopened", b, v(3.0), [0.0, 0.0, 0.0], [80.0, 20.0, 5.0]);
}

/// Backlog 5 (mirror) and Join patterns: a centred bar, a boss and a hole on its
/// left half mirrored across world YZ, and the boss patterned once along +x.
pub(super) fn scenario_mirror(r: &mut Run<'_>) {
    use std::f64::consts::PI;
    r.s = crate::Session::new();
    r.step = "3M.1 bar, boss, mirror".into();
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    let Some(sb) = r.polygon(
        "Bar",
        SketchPlane::WorldXY,
        &[[-40.0, 0.0], [40.0, 0.0], [40.0, 20.0], [-40.0, 20.0]],
    ) else {
        return;
    };
    let x = r.extrude_op("Bar", sb, 10.0, ExtrudeOp::NewBody, None, false, None);
    let Some(bar) = r.ok("extrude bar 80x20x10", &x) else {
        return;
    };
    let top = SketchPlane::Face(FaceRef {
        feature: bar,
        role: FaceRole::ExtrudeEndCap,
    });
    let Some(sboss) = r.polygon(
        "Boss",
        top,
        &[[-35.0, 5.0], [-25.0, 5.0], [-25.0, 15.0], [-35.0, 15.0]],
    ) else {
        return;
    };
    let x = r.extrude_op("Boss", sboss, 5.0, ExtrudeOp::Join, None, false, None);
    let Some(boss) = r.ok("join boss 10x10x5", &x) else {
        return;
    };
    let x = r.exec(Command::Mirror {
        name: None,
        feature: boss,
        plane: SketchPlane::WorldYZ,
    });
    r.ok("mirror boss across YZ", &x);
    let Some(b) = r.s.summary().bodies.first().map(|b| b.id) else {
        return;
    };
    // 16000 + 2 * 500.
    r.check_body_solid(
        "bar + 2 bosses",
        b,
        17000.0,
        [-40.0, 0.0, 0.0],
        [40.0, 20.0, 15.0],
    );

    r.step = "3M.2 hole, mirror, pattern the boss".into();
    let x = r.exec(Command::CreateSketch {
        name: Some("Hole".into()),
        plane: top,
        template: SketchTemplate::CenteredCircle {
            diameter: 6.0,
            center: Some([-5.0, 10.0]),
        },
    });
    let Some(sh) = r.ok("hole sketch", &x) else {
        return;
    };
    let x = r.extrude_op("Hole", sh, 0.0, ExtrudeOp::Cut, None, true, None);
    let Some(hole) = r.ok("through-all hole", &x) else {
        return;
    };
    let x = r.exec(Command::Mirror {
        name: None,
        feature: hole,
        plane: SketchPlane::WorldYZ,
    });
    r.ok("mirror hole across YZ", &x);
    let x = r.exec(Command::LinearPattern {
        name: None,
        feature: boss,
        count: 2,
        spacing: 14.0,
        direction: [1.0, 0.0],
    });
    r.ok("pattern boss x2 at 14 mm (join pattern)", &x);
    let want = 16000.0 + 3.0 * 500.0 - 2.0 * PI * 9.0 * 10.0;
    r.check_body_solid("final", b, want, [-40.0, 0.0, 0.0], [40.0, 20.0, 15.0]);
    let x = r.exec(Command::ExportStep {
        path: r.out_dir.join("mirror_pattern.step"),
        bodies: None,
    });
    r.ok(
        "export STEP (external check: tools/check_run3_steps.sh)",
        &x,
    );
    // A mirrored join that would land off the support face is refused: mirror the
    // boss across world XZ (y -> -y: outside the bar's top face).
    r.refused(
        "mirror boss across XZ (off the face)",
        Command::Mirror {
            name: None,
            feature: boss,
            plane: SketchPlane::WorldXZ,
        },
        "not inside a face",
    );
}

/// Backlog 6: equal-distance chamfer of a box edge named by its two faces.
pub(super) fn scenario_chamfer(r: &mut Run<'_>) {
    r.s = crate::Session::new();
    r.step = "3E.1 chamfer the top-front edge".into();
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    let Some(sb) = r.polygon(
        "Block",
        SketchPlane::WorldXY,
        &[[0.0, 0.0], [40.0, 0.0], [40.0, 30.0], [0.0, 30.0]],
    ) else {
        return;
    };
    let x = r.extrude_op("Block", sb, 10.0, ExtrudeOp::NewBody, None, false, None);
    let Some(blk) = r.ok("extrude 40x30x10", &x) else {
        return;
    };
    let faces = r.feature_summary(blk).map(|f| f.faces).unwrap_or_default();
    let top = faces
        .iter()
        .find(|f| f.reference.role == FaceRole::ExtrudeEndCap)
        .map(|f| f.reference);
    let front = faces
        .iter()
        .find(|f| {
            matches!(f.reference.role, FaceRole::ExtrudeSide { .. })
                && (f.frame.z[1] + 1.0).abs() < 1e-12
        })
        .map(|f| f.reference);
    let bottom = faces
        .iter()
        .find(|f| f.reference.role == FaceRole::ExtrudeStartCap)
        .map(|f| f.reference);
    let (Some(top), Some(front), Some(bottom)) = (top, front, bottom) else {
        r.check(
            "top/front/bottom faces offered",
            "present",
            "missing",
            false,
        );
        return;
    };
    let x = r.exec(Command::Chamfer {
        name: None,
        a: top,
        b: front,
        distance: 2.0,
    });
    let Some(ch) = r.ok("chamfer 2 mm", &x) else {
        return;
    };
    let Some(b) = r.s.summary().bodies.first().map(|b| b.id) else {
        return;
    };
    // 12000 - 0.5 * 2 * 2 * 40.
    r.check_body_solid(
        "chamfered block",
        b,
        11920.0,
        [0.0, 0.0, 0.0],
        [40.0, 30.0, 10.0],
    );
    let x = r.exec(Command::SetParameter {
        feature: ch,
        name: "distance".into(),
        value: 3.0,
    });
    r.ok("distance = 3", &x);
    // 12000 - 0.5 * 9 * 40.
    r.check_body_solid(
        "chamfer 3 mm",
        b,
        11820.0,
        [0.0, 0.0, 0.0],
        [40.0, 30.0, 10.0],
    );
    let x = r.exec(Command::ExportStep {
        path: r.out_dir.join("chamfer.step"),
        bodies: None,
    });
    r.ok(
        "export STEP (external check: tools/check_run3_steps.sh)",
        &x,
    );
    r.refused(
        "chamfer between parallel faces",
        Command::Chamfer {
            name: None,
            a: top,
            b: bottom,
            distance: 1.0,
        },
        "parallel",
    );
    r.refused(
        "chamfer larger than the face",
        Command::SetParameter {
            feature: ch,
            name: "distance".into(),
            value: 12.0,
        },
        "reaches past",
    );
}
