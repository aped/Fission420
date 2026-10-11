//! SESSION_4 §4 printable examples as plain command data: a parametric
//! enclosure base with a mating lid, a clearance-test coupon and a mounting
//! bracket. Every size is a named user parameter and every dimension that
//! depends on one is bound to an expression, so the documents are revised by
//! editing parameters (in the Parameters panel or with `set_user_parameter`),
//! never by re-entering dimensions. The replayable workflows
//! `examples/{enclosure,clearance_coupon,mounting_bracket}_workflow.json` are
//! generated from these builders (see `tests/printable_examples.rs`).
//!
//! Construction (honest labels): the enclosure cavity is a blind rectangular
//! Cut from the base's top face (an editable pocket, not a generic Shell); the
//! lid's locating lip is a rectangular Join from the plate's underside whose
//! opening is a blind Cut from the lip's bottom face back to the plate.
use crate::scenarios::Step;
use crate::{Command, ExtrudeOp, SketchTemplate};
use f420_document::{BodyId, FaceRef, FaceRole, FeatureId, ParamUnit, SketchPlane};
use std::path::Path;

fn ok(cmd: Command) -> Step {
    Step {
        cmd,
        expect_ok: true,
    }
}

fn param(name: &str, expression: &str, unit: ParamUnit, comment: &str) -> Step {
    ok(Command::SetUserParameter {
        name: name.into(),
        expression: expression.into(),
        unit: Some(unit),
        comment: (!comment.is_empty()).then(|| comment.to_string()),
    })
}

fn bind(feature: u32, slot: &str, expression: &str) -> Step {
    ok(Command::SetExpression {
        feature: FeatureId(feature),
        name: slot.into(),
        expression: expression.into(),
    })
}

fn rect(name: &str, plane: SketchPlane, w: f64, h: f64, c: [f64; 2]) -> Step {
    ok(Command::CreateSketch {
        name: Some(name.into()),
        plane,
        template: SketchTemplate::CenteredRectangle {
            width: w,
            height: h,
            center: Some(c),
        },
    })
}

fn circle(name: &str, plane: SketchPlane, d: f64, c: [f64; 2]) -> Step {
    ok(Command::CreateSketch {
        name: Some(name.into()),
        plane,
        template: SketchTemplate::CenteredCircle {
            diameter: d,
            center: Some(c),
        },
    })
}

fn face(feature: u32, role: FaceRole) -> SketchPlane {
    SketchPlane::Face(FaceRef {
        feature: FeatureId(feature),
        role,
    })
}

fn new_body(name: &str, sketch: u32, d: f64, body: &str) -> Step {
    ok(Command::Extrude {
        name: Some(name.into()),
        sketch: FeatureId(sketch),
        distance: d,
        op: ExtrudeOp::NewBody,
        target: None,
        through_all: false,
        reverse: false,
        body_name: Some(body.into()),
    })
}

fn modify(name: &str, sketch: u32, d: f64, op: ExtrudeOp, target: u32, through: bool) -> Step {
    ok(Command::Extrude {
        name: Some(name.into()),
        sketch: FeatureId(sketch),
        distance: d,
        op,
        target: Some(BodyId(target)),
        through_all: through,
        reverse: false,
        body_name: None,
    })
}

// ------------------------------------------------------------------ enclosure

/// Baseline enclosure parameters (SESSION_4 §6.A), mm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Enclosure {
    pub outer_length: f64,
    pub outer_width: f64,
    pub outer_height: f64,
    pub wall: f64,
    pub floor: f64,
    pub lid_thickness: f64,
    pub lip_depth: f64,
    /// Per side (gap between the lip's outer wall and the cavity wall).
    pub fit_clearance: f64,
    /// Lid placement gap (modeling layout only).
    pub lid_gap: f64,
}

pub const ENCLOSURE_BASELINE: Enclosure = Enclosure {
    outer_length: 80.0,
    outer_width: 50.0,
    outer_height: 25.0,
    wall: 2.0,
    floor: 2.0,
    lid_thickness: 2.0,
    lip_depth: 3.0,
    fit_clearance: 0.25,
    lid_gap: 10.0,
};

/// SESSION_4 §6.B regeneration variant.
pub const ENCLOSURE_VARIANT: Enclosure = Enclosure {
    outer_length: 100.0,
    outer_width: 65.0,
    outer_height: 30.0,
    wall: 2.4,
    floor: 2.4,
    lid_thickness: 2.0,
    lip_depth: 3.0,
    fit_clearance: 0.35,
    lid_gap: 10.0,
};

impl Enclosure {
    /// Base: L*W*H - (L-2w)*(W-2w)*(H-f).
    pub fn base_volume(&self) -> f64 {
        let (cl, cw) = self.cavity();
        self.outer_length * self.outer_width * self.outer_height
            - cl * cw * (self.outer_height - self.floor)
    }
    /// Cavity length/width.
    pub fn cavity(&self) -> (f64, f64) {
        (
            self.outer_length - 2.0 * self.wall,
            self.outer_width - 2.0 * self.wall,
        )
    }
    /// Lip outer and inner sizes (lip wall tracks `wall`).
    pub fn lip(&self) -> ((f64, f64), (f64, f64)) {
        let (cl, cw) = self.cavity();
        let o = (cl - 2.0 * self.fit_clearance, cw - 2.0 * self.fit_clearance);
        (o, (o.0 - 2.0 * self.wall, o.1 - 2.0 * self.wall))
    }
    /// Lid: L*W*t + (outer area - inner area) * lip depth.
    pub fn lid_volume(&self) -> f64 {
        let ((ol, ow), (il, iw)) = self.lip();
        self.outer_length * self.outer_width * self.lid_thickness
            + (ol * ow - il * iw) * self.lip_depth
    }
    /// Commands that set every user parameter to these values.
    pub fn set_parameters(&self) -> Vec<Command> {
        [
            ("outer_length", self.outer_length),
            ("outer_width", self.outer_width),
            ("outer_height", self.outer_height),
            ("wall", self.wall),
            ("floor", self.floor),
            ("lid_thickness", self.lid_thickness),
            ("lip_depth", self.lip_depth),
            ("fit_clearance", self.fit_clearance),
            ("lid_gap", self.lid_gap),
        ]
        .into_iter()
        .map(|(n, v)| Command::SetUserParameter {
            name: n.into(),
            expression: format!("{v}"),
            unit: None,
            comment: None,
        })
        .collect()
    }
}

/// Feature ids of the enclosure workflow.
pub mod enclosure_ids {
    pub const BASE_SKETCH: u32 = 1;
    pub const BASE: u32 = 2;
    pub const CAVITY_SKETCH: u32 = 3;
    pub const CAVITY: u32 = 4;
    pub const LID_SKETCH: u32 = 5;
    pub const LID: u32 = 6;
    pub const LIP_SKETCH: u32 = 7;
    pub const LIP: u32 = 8;
    pub const OPENING_SKETCH: u32 = 9;
    pub const OPENING: u32 = 10;
    /// Bodies.
    pub const BASE_BODY: u32 = 1;
    pub const LID_BODY: u32 = 2;
}

/// The parametric enclosure: base with an open cavity (floor and four walls)
/// and a separate lid with a locating lip, then saved to `doc`.
///
/// Fixture coordinates: base x [0, L], y [0, W], z [0, H]; cavity from the top
/// down to z = floor; lid plate z [0, t] centred at y = -(W/2 + lid_gap); lip
/// z [-lip_depth, 0] (the lid is modelled upside down relative to its fitted
/// position; export placement turns it over onto the bed).
pub fn enclosure_workflow(doc: &Path) -> Vec<Step> {
    use enclosure_ids::*;
    let p = ENCLOSURE_BASELINE;
    let (cl, cw) = p.cavity();
    let ((ol, ow), (il, iw)) = p.lip();
    let (cx, cy) = (p.outer_length / 2.0, p.outer_width / 2.0);
    let lid_y = p.outer_width / 2.0 + p.lid_gap;
    let mm = ParamUnit::Mm;
    vec![
        ok(Command::NewDocument),
        param("outer_length", "80", mm, "base outside, X"),
        param("outer_width", "50", mm, "base outside, Y"),
        param(
            "outer_height",
            "25",
            mm,
            "base outside, Z (floor + cavity depth)",
        ),
        param(
            "wall",
            "2",
            mm,
            "side wall thickness (base walls and lid lip wall)",
        ),
        param("floor", "2", mm, "base floor thickness"),
        param("lid_thickness", "2", mm, "lid plate thickness"),
        param(
            "lip_depth",
            "3",
            mm,
            "locating lip height below the lid plate",
        ),
        param(
            "fit_clearance",
            "0.25",
            mm,
            "PER SIDE gap between lip and cavity wall (diametral = 2x); calibrate with the clearance coupon",
        ),
        param(
            "lid_gap",
            "10",
            mm,
            "modeling layout only: lid distance from the base",
        ),
        param("cavity_length", "outer_length - 2 * wall", mm, ""),
        param("cavity_width", "outer_width - 2 * wall", mm, ""),
        param("cavity_depth", "outer_height - floor", mm, ""),
        param("lip_wall", "wall", mm, "lip wall tracks the base wall"),
        param("lip_length", "cavity_length - 2 * fit_clearance", mm, ""),
        param("lip_width", "cavity_width - 2 * fit_clearance", mm, ""),
        // Base.
        rect(
            "Base outline",
            SketchPlane::WorldXY,
            p.outer_length,
            p.outer_width,
            [cx, cy],
        ),
        bind(BASE_SKETCH, "width", "outer_length"),
        bind(BASE_SKETCH, "height", "outer_width"),
        bind(BASE_SKETCH, "center_x", "outer_length / 2"),
        bind(BASE_SKETCH, "center_y", "outer_width / 2"),
        new_body("Base", BASE_SKETCH, p.outer_height, "Enclosure base"),
        bind(BASE, "distance", "outer_height"),
        // Cavity: a blind pocket from the top face down to the floor.
        rect(
            "Cavity outline",
            face(BASE, FaceRole::ExtrudeEndCap),
            cl,
            cw,
            [cx, cy],
        ),
        bind(CAVITY_SKETCH, "width", "cavity_length"),
        bind(CAVITY_SKETCH, "height", "cavity_width"),
        bind(CAVITY_SKETCH, "center_x", "outer_length / 2"),
        bind(CAVITY_SKETCH, "center_y", "outer_width / 2"),
        modify(
            "Cavity",
            CAVITY_SKETCH,
            p.outer_height - p.floor,
            ExtrudeOp::Cut,
            BASE_BODY,
            false,
        ),
        bind(CAVITY, "distance", "cavity_depth"),
        // Lid plate (beside the base, in the -Y direction).
        rect(
            "Lid outline",
            SketchPlane::WorldXY,
            p.outer_length,
            p.outer_width,
            [cx, -lid_y],
        ),
        bind(LID_SKETCH, "width", "outer_length"),
        bind(LID_SKETCH, "height", "outer_width"),
        bind(LID_SKETCH, "center_x", "outer_length / 2"),
        bind(LID_SKETCH, "center_y", "-(outer_width / 2 + lid_gap)"),
        new_body("Lid plate", LID_SKETCH, p.lid_thickness, "Enclosure lid"),
        bind(LID, "distance", "lid_thickness"),
        // Lip: joined to the plate's underside (start cap: local y = -world y).
        rect(
            "Lip outline",
            face(LID, FaceRole::ExtrudeStartCap),
            ol,
            ow,
            [cx, lid_y],
        ),
        bind(LIP_SKETCH, "width", "lip_length"),
        bind(LIP_SKETCH, "height", "lip_width"),
        bind(LIP_SKETCH, "center_x", "outer_length / 2"),
        bind(LIP_SKETCH, "center_y", "outer_width / 2 + lid_gap"),
        modify(
            "Lip",
            LIP_SKETCH,
            p.lip_depth,
            ExtrudeOp::Join,
            LID_BODY,
            false,
        ),
        bind(LIP, "distance", "lip_depth"),
        // Lip opening: cut from the lip's bottom face back up to the plate.
        rect(
            "Lip opening outline",
            face(LIP, FaceRole::ExtrudeEndCap),
            il,
            iw,
            [cx, lid_y],
        ),
        bind(OPENING_SKETCH, "width", "lip_length - 2 * lip_wall"),
        bind(OPENING_SKETCH, "height", "lip_width - 2 * lip_wall"),
        bind(OPENING_SKETCH, "center_x", "outer_length / 2"),
        bind(OPENING_SKETCH, "center_y", "outer_width / 2 + lid_gap"),
        modify(
            "Lip opening",
            OPENING_SKETCH,
            p.lip_depth,
            ExtrudeOp::Cut,
            LID_BODY,
            false,
        ),
        bind(OPENING, "distance", "lip_depth"),
        ok(Command::Save {
            path: doc.to_path_buf(),
        }),
        ok(Command::Inspect),
    ]
}

// ------------------------------------------------------- clearance coupon

/// Nominal per-side clearances of the coupon's holes (mm). Test values for
/// choosing `fit_clearance` from a real print, not recommended settings.
pub const COUPON_CLEARANCES: [f64; 4] = [0.15, 0.25, 0.35, 0.45];
/// Peg diameter, plate thickness, peg height, hole spacing (mm).
pub const COUPON: (f64, f64, f64, f64) = (6.0, 4.0, 10.0, 15.0);

/// Coupon feature ids: plate sketch 1, plate 2, hole i sketch 3+2i, hole i
/// cut 4+2i (i = 0..4), peg sketch 11, peg 12. Bodies: plate 1, peg 2.
pub fn coupon_workflow(doc: &Path) -> Vec<Step> {
    let (d, t, h, sp) = COUPON;
    let mm = ParamUnit::Mm;
    let n = COUPON_CLEARANCES.len() as f64;
    let length = sp * n + 10.0;
    let mut v = vec![
        ok(Command::NewDocument),
        param("peg_d", "6", mm, "peg diameter (nominal)"),
        param("plate_t", "4", mm, "coupon plate thickness"),
        param("peg_h", "10", mm, "peg height"),
        param("hole_spacing", "15", mm, "hole centre spacing"),
        param(
            "c1",
            "0.15",
            mm,
            "hole 1 clearance PER SIDE (hole Ø = peg_d + 2*c1)",
        ),
        param("c2", "0.25", mm, "hole 2 clearance per side"),
        param("c3", "0.35", mm, "hole 3 clearance per side"),
        param("c4", "0.45", mm, "hole 4 clearance per side"),
        rect(
            "Coupon plate outline",
            SketchPlane::WorldXY,
            length,
            20.0,
            [length / 2.0, 10.0],
        ),
        bind(1, "width", "hole_spacing * 4 + 10"),
        bind(1, "center_x", "(hole_spacing * 4 + 10) / 2"),
        new_body(
            "Coupon plate",
            1,
            t,
            "Coupon plate (holes c=0.15/0.25/0.35/0.45 per side)",
        ),
        bind(2, "distance", "plate_t"),
    ];
    for (i, c) in COUPON_CLEARANCES.iter().enumerate() {
        let (sk, cut) = (3 + 2 * i as u32, 4 + 2 * i as u32);
        let x = 5.0 + sp / 2.0 + sp * i as f64;
        v.push(circle(
            &format!("Hole {} outline (c={c} per side)", i + 1),
            face(2, FaceRole::ExtrudeEndCap),
            d + 2.0 * c,
            [x, 10.0],
        ));
        v.push(bind(sk, "diameter", &format!("peg_d + 2 * c{}", i + 1)));
        v.push(bind(
            sk,
            "center_x",
            &format!("5 + hole_spacing / 2 + hole_spacing * {i}"),
        ));
        v.push(modify(
            &format!("Hole {} c={c}", i + 1),
            sk,
            0.0,
            ExtrudeOp::Cut,
            1,
            true,
        ));
        let _ = cut;
    }
    v.extend([
        circle(
            "Peg outline",
            SketchPlane::WorldXY,
            d,
            [length / 2.0, -15.0],
        ),
        bind(11, "diameter", "peg_d"),
        new_body("Peg", 11, h, "Peg Ø6 nominal (test in each hole)"),
        bind(12, "distance", "peg_h"),
        ok(Command::Save {
            path: doc.to_path_buf(),
        }),
        ok(Command::Inspect),
    ]);
    v
}

// --------------------------------------------------------- mounting bracket

/// Bracket feature ids.
pub mod bracket_ids {
    pub const PLATE_SKETCH: u32 = 1;
    pub const PLATE: u32 = 2;
    pub const UPRIGHT_SKETCH: u32 = 3;
    pub const UPRIGHT: u32 = 4;
    pub const BASE_HOLE_SKETCH: u32 = 5;
    pub const BASE_HOLE: u32 = 6;
    pub const BASE_HOLES: u32 = 7;
    pub const UPRIGHT_HOLE_SKETCH: u32 = 8;
    pub const UPRIGHT_HOLE: u32 = 9;
    pub const UPRIGHT_HOLES: u32 = 10;
    pub const CHAMFER: u32 = 11;
}

/// Bracket parameters: plate 60x40x5, upright 50x5x35 inset 5 from the sides
/// and back, Ø5.5 holes (2 in the plate, 2 in the upright, linear patterns),
/// 1 mm chamfer on the plate's front top edge.
pub fn bracket_workflow(doc: &Path) -> Vec<Step> {
    use bracket_ids::*;
    let mm = ParamUnit::Mm;
    vec![
        ok(Command::NewDocument),
        param("plate_l", "60", mm, "base plate X"),
        param("plate_w", "40", mm, "base plate Y"),
        param("plate_t", "5", mm, "base plate thickness"),
        param("upright_h", "35", mm, "upright height above the plate"),
        param("upright_t", "5", mm, "upright thickness"),
        param(
            "inset",
            "5",
            mm,
            "upright inset from the plate's sides and back",
        ),
        param("hole_d", "5.5", mm, "screw clearance hole (M5)"),
        param("hole_spacing", "30", mm, "hole pitch"),
        param("hole_count", "2", ParamUnit::Unitless, "holes per row"),
        param("edge_chamfer", "1", mm, "front edge chamfer"),
        rect(
            "Plate outline",
            SketchPlane::WorldXY,
            60.0,
            40.0,
            [30.0, 20.0],
        ),
        bind(PLATE_SKETCH, "width", "plate_l"),
        bind(PLATE_SKETCH, "height", "plate_w"),
        bind(PLATE_SKETCH, "center_x", "plate_l / 2"),
        bind(PLATE_SKETCH, "center_y", "plate_w / 2"),
        new_body("Plate", PLATE_SKETCH, 5.0, "Mounting bracket"),
        bind(PLATE, "distance", "plate_t"),
        rect(
            "Upright outline",
            face(PLATE, FaceRole::ExtrudeEndCap),
            50.0,
            5.0,
            [30.0, 32.5],
        ),
        bind(UPRIGHT_SKETCH, "width", "plate_l - 2 * inset"),
        bind(UPRIGHT_SKETCH, "height", "upright_t"),
        bind(UPRIGHT_SKETCH, "center_x", "plate_l / 2"),
        bind(
            UPRIGHT_SKETCH,
            "center_y",
            "plate_w - inset - upright_t / 2",
        ),
        modify("Upright", UPRIGHT_SKETCH, 35.0, ExtrudeOp::Join, 1, false),
        bind(UPRIGHT, "distance", "upright_h"),
        circle(
            "Plate hole outline",
            face(PLATE, FaceRole::ExtrudeEndCap),
            5.5,
            [15.0, 15.0],
        ),
        bind(BASE_HOLE_SKETCH, "diameter", "hole_d"),
        bind(BASE_HOLE_SKETCH, "center_x", "(plate_l - hole_spacing) / 2"),
        modify("Plate hole", BASE_HOLE_SKETCH, 0.0, ExtrudeOp::Cut, 1, true),
        ok(Command::LinearPattern {
            name: Some("Plate holes".into()),
            feature: FeatureId(BASE_HOLE),
            count: 2,
            spacing: 30.0,
            direction: [1.0, 0.0],
        }),
        bind(BASE_HOLES, "spacing", "hole_spacing"),
        bind(BASE_HOLES, "count", "hole_count"),
        // Upright front face: side face of upright line 1 (local x = world X
        // from the upright's left end, local y = world Z from the plate top).
        circle(
            "Upright hole outline",
            face(UPRIGHT, FaceRole::ExtrudeSide { entity: 1 }),
            5.5,
            [10.0, 20.0],
        ),
        bind(UPRIGHT_HOLE_SKETCH, "diameter", "hole_d"),
        bind(
            UPRIGHT_HOLE_SKETCH,
            "center_x",
            "(plate_l - hole_spacing) / 2 - inset",
        ),
        bind(UPRIGHT_HOLE_SKETCH, "center_y", "upright_h * 0.6"),
        modify(
            "Upright hole",
            UPRIGHT_HOLE_SKETCH,
            0.0,
            ExtrudeOp::Cut,
            1,
            true,
        ),
        ok(Command::LinearPattern {
            name: Some("Upright holes".into()),
            feature: FeatureId(UPRIGHT_HOLE),
            count: 2,
            spacing: 30.0,
            direction: [1.0, 0.0],
        }),
        bind(UPRIGHT_HOLES, "spacing", "hole_spacing"),
        bind(UPRIGHT_HOLES, "count", "hole_count"),
        ok(Command::Chamfer {
            name: Some("Front edge chamfer".into()),
            a: FaceRef {
                feature: FeatureId(PLATE),
                role: FaceRole::ExtrudeEndCap,
            },
            b: FaceRef {
                feature: FeatureId(PLATE),
                role: FaceRole::ExtrudeSide { entity: 1 },
            },
            distance: 1.0,
        }),
        bind(CHAMFER, "distance", "edge_chamfer"),
        ok(Command::Save {
            path: doc.to_path_buf(),
        }),
        ok(Command::Inspect),
    ]
}
