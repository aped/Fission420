//! Stretch scenario R: three-point arcs (rounded rectangle, slot, refused
//! line/arc loops), headless through [`Session::execute`] only. Every expected
//! value is derived by hand in the comments below; none is read back from the
//! implementation.
//!
//! Areas used:
//! * rounded rectangle w x h with corner radius r: the w x h rectangle minus
//!   four r x r corner squares plus four quarter discs = w·h − (4 − π)·r²;
//! * slot (stadium) with cap-centre distance L and radius r: an L x 2r
//!   rectangle plus two half discs = 2·r·L + π·r².
use super::Run;
use crate::scenarios::{
    self as sc, CORNER_HOLE_CENTER, ROUNDED_PLATE, ROUNDED_RADIUS_EDIT, SLOT_PLATE,
};
use crate::sketches;
use crate::{Command, Session, SketchTemplate, end_cap_of};
use f420_document::{DocError, FeatureId, SketchPlane};
use f420_sketch::{Profile, SolveStatus};
use std::f64::consts::PI;

fn rounded_area(w: f64, h: f64, r: f64) -> f64 {
    w * h - (4.0 - PI) * r * r
}

impl Run<'_> {
    fn check_fully_constrained(&mut self, id: FeatureId, what: &str) {
        let fs = self.feature_summary(id);
        let st = fs.as_ref().and_then(|f| f.solve_status);
        let dof = fs.as_ref().and_then(|f| f.solve_dof);
        self.check(
            &format!("{what}: constraint state"),
            "FullyConstrained, dof 0",
            format!("{st:?}, dof {dof:?}"),
            st == Some(SolveStatus::FullyConstrained) && dof == Some(0),
        );
    }

    /// A circular cut sketched on the end cap of `e1` at `center` must be refused
    /// by the containment product limit, leaving the document unchanged.
    fn check_corner_cut_refused(&mut self, e1: FeatureId, d: f64, center: [f64; 2], why: &str) {
        let Ok((sk, _)) = sketches::offset_circle(d, center[0], center[1]) else {
            return self.check("build offset circle", "ok", "error", false);
        };
        let x = self.exec(Command::CreateSketch {
            name: Some("Corner hole sketch".into()),
            plane: end_cap_of(e1),
            template: SketchTemplate::Custom { sketch: sk },
        });
        let Some(sid) = self.ok("corner circle sketch on end cap", &x) else {
            return;
        };
        let before = self.doc_and_rev();
        let x = self.exec(Command::CutThroughAll {
            name: Some("Corner hole".into()),
            sketch: sid,
        });
        let detail = x.error.as_ref().map(|e| e.to_string()).unwrap_or_default();
        self.check(
            &format!("Ø{d} at {center:?} refused: {why}"),
            "ProductLimit (containment)",
            &detail,
            !x.ok && matches!(x.error, Some(DocError::ProductLimit(_))),
        );
        self.check_unchanged("after refused corner cut", &before);
        let x = self.exec(Command::Undo);
        self.ok("undo corner sketch", &x);
    }
}

pub(super) fn scenario_r(r: &mut Run<'_>) {
    let [w, h, rad, t] = ROUNDED_PLATE;
    let (bmin, bmax) = ([-w / 2.0, -h / 2.0, 0.0], [w / 2.0, h / 2.0, t]);
    r.s = Session::new();
    r.step = format!("R.1 (stretch) rounded rectangle {w}x{h} r={rad}, extrude {t}");
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    let base = match sketches::rounded_rectangle(w, h, rad) {
        Ok((s, _)) => s,
        Err(e) => return r.check("build rounded rectangle", "ok", e, false),
    };
    let x = r.exec(Command::CreateSketch {
        name: Some("Rounded base".into()),
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::Custom { sketch: base },
    });
    let Some(s1) = r.ok("create_sketch (4 lines + 4 three-point arcs)", &x) else {
        return;
    };
    r.check_fully_constrained(s1, "rounded rectangle");
    let params = r.feature_summary(s1).map(|f| f.parameters);
    r.check(
        "parameters",
        "width=80, height=50, radius=8",
        format!("{params:?}"),
        params
            == Some(vec![
                ("width".into(), w),
                ("height".into(), h),
                ("radius".into(), rad),
            ]),
    );
    let arcs_in_profile = r
        .solved_sketch(s1)
        .and_then(|s| s.profile().ok())
        .map(|p| match p {
            Profile::Loop(v) => v.len(),
            _ => 0,
        });
    r.check(
        "profile is a line/arc loop of 8 segments",
        "Some(8)",
        format!("{arcs_in_profile:?}"),
        arcs_in_profile == Some(8),
    );
    let x = r.exec(Command::Extrude {
        name: Some("Rounded plate".into()),
        sketch: s1,
        distance: t,
    });
    let Some(e1) = r.ok("extrude", &x) else {
        return;
    };
    // 80*50 - (4 - pi)*64 = 3945.0619298...; x 10.
    let v8 = rounded_area(w, h, rad) * t;
    // Topology: 4 planar + 4 cylindrical (one 90° arc edge each) side faces and
    // 2 caps = 10 faces; 8 edges per cap + 8 vertical = 24; 16 vertices.
    // Bounds are attained on the straight sides.
    r.check_solid(v8, bmin, bmax, Some([10, 24, 16]));
    r.check_exports("scenario_r_initial", v8);

    r.step = format!("R.2 radius {rad} -> {ROUNDED_RADIUS_EDIT} (all corners), undo/redo");
    let x = r.exec(Command::SetParameter {
        feature: s1,
        name: "radius".into(),
        value: ROUNDED_RADIUS_EDIT,
    });
    r.ok("set radius 12", &x);
    // 80*50 - (4 - pi)*144 = 3876.3893421...; x 10.
    let v12 = rounded_area(w, h, ROUNDED_RADIUS_EDIT) * t;
    r.check_solid(v12, bmin, bmax, Some([10, 24, 16]));
    let x = r.exec(Command::Undo);
    r.ok("undo", &x);
    r.check_solid(v8, bmin, bmax, None);
    let x = r.exec(Command::Redo);
    r.ok("redo", &x);
    r.check_solid(v12, bmin, bmax, None);
    // Out-of-range radius: 2r = 50 = height leaves no straight side; the edit
    // must fail and roll back.
    let before = r.doc_and_rev();
    let x = r.exec(Command::SetParameter {
        feature: s1,
        name: "radius".into(),
        value: 26.0,
    });
    r.check(
        "radius 26 (> height/2) refused",
        "error",
        format!("{:?}", x.error),
        !x.ok,
    );
    r.check_unchanged("after refused radius", &before);

    r.step = "R.3 Ø10 through-all cut centred on the end cap".into();
    let hole = match sc::hole_circle(10.0) {
        Ok(s) => s,
        Err(e) => return r.check("build hole circle", "ok", e, false),
    };
    let x = r.exec(Command::CreateSketch {
        name: Some("Hole sketch".into()),
        plane: end_cap_of(e1),
        template: SketchTemplate::Custom { sketch: hole },
    });
    let Some(s2) = r.ok("hole sketch on end cap", &x) else {
        return;
    };
    let x = r.exec(Command::CutThroughAll {
        name: Some("Hole".into()),
        sketch: s2,
    });
    if r.ok("cut_through_all", &x).is_none() {
        return;
    }
    let vh = v12 - PI * 25.0 * t;
    r.check_solid(vh, bmin, bmax, None);
    r.check_hole(10.0, t);

    r.step = "R.4 corner holes inside the bounding rectangle but breaking the rounded loop".into();
    // Ø10 at (34.5, 19.5): 40 - 34.5 - 5 = 0.5 and 25 - 19.5 - 5 = 0.5 mm from the
    // straight sides' lines (a polygon host would accept it), but the TR arc
    // (centre (28, 13), r 12) is 12 - |(6.5, 6.5)| = 12 - 9.19 = 2.81 mm < 5 away.
    let rect_clear =
        (w / 2.0 - CORNER_HOLE_CENTER[0] - 5.0).min(h / 2.0 - CORNER_HOLE_CENTER[1] - 5.0);
    let arc_clear = ROUNDED_RADIUS_EDIT
        - ((CORNER_HOLE_CENTER[0] - 28.0).powi(2) + (CORNER_HOLE_CENTER[1] - 13.0).powi(2)).sqrt()
        - 5.0;
    r.check(
        "hand check: clear of the bounding rectangle, not of the corner arc",
        "rect clearance > 0, arc clearance < 0",
        format!("{rect_clear:.3}, {arc_clear:.3}"),
        rect_clear > 0.0 && arc_clear < 0.0,
    );
    r.check_corner_cut_refused(e1, 10.0, CORNER_HOLE_CENTER, "crosses the corner arc");
    // Ø1 at (39, 24): inside the rectangle, but |(39,24) - (28,13)| = 15.56 > 12:
    // the centre itself lies outside the rounded loop.
    r.check_corner_cut_refused(e1, 1.0, [39.0, 24.0], "centre outside the loop");
    r.check_solid(vh, bmin, bmax, None);

    r.step = "R.5 save/close/reopen, exports".into();
    let path = r.out_dir.join("scenario_r.f420.json");
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
    r.check_solid(vh, bmin, bmax, None);
    r.check_exports("scenario_r", vh);

    let [len, sr, st] = SLOT_PLATE;
    r.step = format!("R.6 slot (cap centres {len} apart, r={sr}), extrude {st}");
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    let slot = match sketches::slot(len, sr) {
        Ok((s, _)) => s,
        Err(e) => return r.check("build slot", "ok", e, false),
    };
    let x = r.exec(Command::CreateSketch {
        name: Some("Slot".into()),
        plane: SketchPlane::WorldXY,
        template: SketchTemplate::Custom { sketch: slot },
    });
    let Some(ss) = r.ok("slot sketch (2 lines + 2 semicircular arcs)", &x) else {
        return;
    };
    r.check_fully_constrained(ss, "slot");
    let x = r.exec(Command::Extrude {
        name: Some("Slot plate".into()),
        sketch: ss,
        distance: st,
    });
    let Some(es) = r.ok("extrude slot", &x) else {
        return;
    };
    // 60*20 + pi*100 = 1514.159265...; x 5. Bounds: x in [-30-10, 30+10]
    // (the caps' extreme points), y in [-10, 10].
    let vs = (2.0 * sr * len + PI * sr * sr) * st;
    let (smin, smax) = ([-len / 2.0 - sr, -sr, 0.0], [len / 2.0 + sr, sr, st]);
    r.check_solid(vs, smin, smax, None);
    r.check_exports("scenario_r_slot", vs);
    let x = r.exec(Command::SetParameter {
        feature: ss,
        name: "length".into(),
        value: 40.0,
    });
    r.ok("set slot length 40", &x);
    let vs40 = (2.0 * sr * 40.0 + PI * sr * sr) * st;
    r.check_solid(vs40, [-30.0, -sr, 0.0], [30.0, sr, st], None);
    let thickness = r.param(es, "distance");
    r.check(
        "slot thickness parameter",
        st,
        format!("{thickness:?}"),
        thickness == Some(st),
    );

    r.step = "R.7 new document: self-intersecting line+arc loop and open arc chain refused".into();
    let x = r.exec(Command::NewDocument);
    r.ok("new_document", &x);
    for (label, sk) in [
        (
            "self-intersecting line+arc",
            sc::self_intersecting_arc_loop(),
        ),
        ("open arc chain", sc::open_arc_chain()),
    ] {
        let Ok(sk) = sk else {
            r.check(&format!("build {label} sketch"), "ok", "error", false);
            continue;
        };
        let x = r.exec(Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::Custom { sketch: sk },
        });
        let Some(sid) = r.ok(&format!("{label}: sketch is kept (not extrudable)"), &x) else {
            continue;
        };
        let pe = r.feature_summary(sid).and_then(|f| f.profile_error);
        let want = if label.starts_with("open") {
            "closed loop"
        } else {
            "self-intersecting"
        };
        r.check(
            &format!("{label}: profile_error names the problem"),
            format!("contains '{want}'"),
            format!("{pe:?}"),
            pe.as_deref().is_some_and(|e| e.contains(want)),
        );
        let before = r.doc_and_rev();
        let x = r.exec(Command::Extrude {
            name: None,
            sketch: sid,
            distance: 5.0,
        });
        let detail = x.error.as_ref().map(|e| e.to_string()).unwrap_or_default();
        r.check(
            &format!("{label}: extrude rejected with a structured error"),
            "Validation error naming the profile",
            &detail,
            !x.ok && matches!(x.error, Some(DocError::Validation(_))) && detail.contains("profile"),
        );
        r.check_unchanged(&format!("{label}: after rejected extrude"), &before);
        let body = r.s.model().map(|m| m.evaluation().body.is_some());
        r.check(
            "no body created",
            false,
            format!("{body:?}"),
            body == Some(false),
        );
        let x = r.exec(Command::Undo);
        r.ok("undo sketch", &x);
    }
}
