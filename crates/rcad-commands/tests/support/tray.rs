//! Organizer tray built from ordinary commands, and its independent
//! analytic oracle. The oracle is the tray volume formula, derived from
//! the parameters only; it never reads the model.
use rcad_commands::{Command, Response, Session};
use rcad_document::BodyId;
use rcad_kernel::VolumeResult;
use serde_json::{Value, json};

/// Tray parameters (mm; `n` = divider count).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tray {
    pub l: f64,
    pub w: f64,
    pub h: f64,
    pub wall: f64,
    pub floor: f64,
    pub dt: f64,
    pub n: u32,
    pub notch_w: f64,
    pub notch_d: f64,
}

impl Tray {
    pub const BASELINE: Tray = Tray {
        l: 120.0,
        w: 80.0,
        h: 30.0,
        wall: 2.0,
        floor: 2.0,
        dt: 2.0,
        n: 2,
        notch_w: 16.0,
        notch_d: 8.0,
    };

    /// Interior compartment width (L − 2·wall − n·d) / (n + 1).
    pub fn compartment(&self) -> f64 {
        (self.l - 2.0 * self.wall - f64::from(self.n) * self.dt) / f64::from(self.n + 1)
    }

    /// L·W·H − (L−2w)(W−2w)(H−f) + n·d·(W−2w)(H−f).
    pub fn plain_volume(&self) -> f64 {
        let (cl, cw, cd) = (
            self.l - 2.0 * self.wall,
            self.w - 2.0 * self.wall,
            self.h - self.floor,
        );
        self.l * self.w * self.h - cl * cw * cd + f64::from(self.n) * self.dt * cw * cd
    }

    /// Plain volume minus the rim notch (width × depth × wall).
    pub fn notched_volume(&self) -> f64 {
        self.plain_volume() - self.notch_w * self.notch_d * self.wall
    }

    /// x of the centre of compartment `i` (0-based).
    pub fn compartment_center(&self, i: u32) -> f64 {
        self.wall + self.compartment() / 2.0 + f64::from(i) * (self.compartment() + self.dt)
    }

    /// The parameter values as one `set_user_parameters` command (one step).
    pub fn set_all(&self) -> Command {
        serde_json::from_value(json!({
            "cmd": "set_user_parameters",
            "parameters": [
                {"name": "tray_length", "expression": num(self.l), "unit": "mm"},
                {"name": "tray_width", "expression": num(self.w), "unit": "mm"},
                {"name": "tray_height", "expression": num(self.h), "unit": "mm"},
                {"name": "wall", "expression": num(self.wall), "unit": "mm"},
                {"name": "floor", "expression": num(self.floor), "unit": "mm"},
                {"name": "divider_thickness", "expression": num(self.dt), "unit": "mm"},
                {"name": "divider_count", "expression": self.n.to_string(), "unit": "unitless"},
                {"name": "notch_width", "expression": num(self.notch_w), "unit": "mm"},
                {"name": "notch_depth", "expression": num(self.notch_d), "unit": "mm"},
            ]
        }))
        .expect("set_user_parameters")
    }
}

fn num(v: f64) -> String {
    format!("{v}")
}

fn p(name: &str, e: &str, unit: &str) -> Value {
    json!({"cmd": "set_user_parameter", "name": name, "expression": e, "unit": unit})
}

fn x(f: u32, name: &str, e: &str) -> Value {
    json!({"cmd": "set_expression", "feature": f, "name": name, "expression": e})
}

/// Feature ids of the tray workflow.
pub const OUTLINE: u32 = 1;
pub const BLANK: u32 = 2;
pub const COMPARTMENT: u32 = 4;
pub const COMPARTMENTS: u32 = 5;
pub const NOTCH: u32 = 7;

/// Commands from New: parameters, outline + blank, one compartment pocket,
/// linear pattern (count = divider_count + 1), and (optionally) the rim notch:
/// a rectangle on the front face flush with the rim, cut `wall` deep.
pub fn steps(t: &Tray, notch: bool) -> Vec<Value> {
    let mut s = vec![
        json!({"cmd": "new_document"}),
        p("tray_length", &num(t.l), "mm"),
        p("tray_width", &num(t.w), "mm"),
        p("tray_height", &num(t.h), "mm"),
        p("wall", &num(t.wall), "mm"),
        p("floor", &num(t.floor), "mm"),
        p("divider_thickness", &num(t.dt), "mm"),
        p("divider_count", &t.n.to_string(), "unitless"),
        p("notch_width", &num(t.notch_w), "mm"),
        p("notch_depth", &num(t.notch_d), "mm"),
        p("compartment_count", "divider_count + 1", "unitless"),
        p(
            "compartment_width",
            "(tray_length - 2 * wall - divider_count * divider_thickness) / compartment_count",
            "mm",
        ),
        p("cavity_width", "tray_width - 2 * wall", "mm"),
        p("cavity_depth", "tray_height - floor", "mm"),
        json!({"cmd": "create_sketch", "name": "Tray outline", "plane": "WorldXY",
               "template": "centered_rectangle", "width": t.l, "height": t.w,
               "center": [t.l / 2.0, t.w / 2.0]}),
        x(OUTLINE, "width", "tray_length"),
        x(OUTLINE, "height", "tray_width"),
        x(OUTLINE, "center_x", "tray_length / 2"),
        x(OUTLINE, "center_y", "tray_width / 2"),
        json!({"cmd": "extrude", "sketch": OUTLINE, "distance": t.h, "name": "Tray blank",
               "body_name": "Organizer tray"}),
        x(BLANK, "distance", "tray_height"),
        json!({"cmd": "create_sketch", "name": "Compartment outline",
               "plane": {"Face": {"feature": BLANK, "role": "ExtrudeEndCap"}},
               "template": "centered_rectangle", "width": t.compartment(),
               "height": t.w - 2.0 * t.wall, "center": [t.compartment_center(0), t.w / 2.0]}),
        x(3, "width", "compartment_width"),
        x(3, "height", "cavity_width"),
        x(3, "center_x", "wall + compartment_width / 2"),
        x(3, "center_y", "tray_width / 2"),
        json!({"cmd": "extrude", "sketch": 3, "distance": t.h - t.floor, "op": "cut",
               "target": 1, "name": "Compartment"}),
        x(COMPARTMENT, "distance", "cavity_depth"),
        json!({"cmd": "linear_pattern", "feature": COMPARTMENT, "count": t.n + 1,
               "spacing": t.compartment() + t.dt, "direction": [1.0, 0.0],
               "name": "Compartments"}),
        x(COMPARTMENTS, "count", "compartment_count"),
        x(
            COMPARTMENTS,
            "spacing",
            "compartment_width + divider_thickness",
        ),
    ];
    if notch {
        s.extend([
            json!({"cmd": "create_sketch", "name": "Notch outline",
                   "plane": {"Face": {"feature": BLANK, "role": {"ExtrudeSide": {"entity": 1}}}},
                   "template": "centered_rectangle", "width": t.notch_w, "height": t.notch_d,
                   "center": [t.compartment_center(0), t.h - t.notch_d / 2.0]}),
            x(6, "width", "notch_width"),
            x(6, "height", "notch_depth"),
            x(6, "center_x", "wall + compartment_width / 2"),
            x(6, "center_y", "tray_height - notch_depth / 2"),
            json!({"cmd": "extrude", "sketch": 6, "distance": t.wall, "op": "cut",
                   "target": 1, "name": "Rim notch"}),
            x(NOTCH, "distance", "wall"),
        ]);
    }
    s
}

pub fn exec(s: &mut Session, v: &Value) -> Response {
    let c: Command = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{v}: {e}"));
    s.execute(&c)
}

pub fn ok(s: &mut Session, v: &Value) -> Response {
    let r = exec(s, v);
    assert!(r.ok, "{v}: {:?}", r.error);
    r
}

pub fn build(t: &Tray, notch: bool) -> Session {
    let mut s = Session::new();
    for v in steps(t, notch) {
        ok(&mut s, &v);
    }
    s
}

pub fn volume(s: &Session) -> f64 {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id == BodyId(1))
        .expect("tray body");
    match b.measurements.expect("measured").volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

/// Inside / outside of world points against the tray body (exact winding
/// number on the kernel's validation tessellation; None = undecided).
pub fn probe(s: &Session, pts: &[[f64; 3]]) -> Vec<Option<bool>> {
    let m = s.model().expect("model");
    let body = m
        .evaluation()
        .body_by_id(BodyId(1))
        .expect("body")
        .geometry
        .brep()
        .expect("brep");
    rcad_kernel::point_membership(body, pts).expect("membership")
}

/// Occupancy probes derived from the parameters only: material in walls,
/// floor and dividers; air in every compartment, above the rim and in the
/// notch (when `notch`).
pub fn occupancy(t: &Tray, notch: bool) -> Vec<([f64; 3], bool, String)> {
    let mut v = Vec::new();
    let (mx, my, mz) = (t.l / 2.0, t.w / 2.0, t.floor + (t.h - t.floor) / 2.0);
    let hw = t.wall / 2.0;
    v.push(([hw, my, mz], true, "left wall".into()));
    v.push(([t.l - hw, my, mz], true, "right wall".into()));
    v.push(([mx + 1.3, t.w - hw, mz], true, "back wall".into()));
    v.push(([mx + 1.3, my, t.floor / 2.0], true, "floor".into()));
    v.push(([mx, my, t.h + 1.0], false, "above the rim".into()));
    v.push(([mx, my, -1.0], false, "below the floor".into()));
    for i in 0..=t.n {
        let c = t.compartment_center(i);
        v.push(([c, my, mz], false, format!("compartment {} centre", i + 1)));
        v.push((
            [c, t.wall + 0.25, t.floor + 0.25],
            false,
            format!("compartment {} front-bottom corner", i + 1),
        ));
        if i < t.n {
            let dx = c + t.compartment() / 2.0 + t.dt / 2.0;
            v.push(([dx, my, mz], true, format!("divider {}", i + 1)));
            v.push((
                [dx, t.wall + 0.1, t.h - 0.1],
                true,
                format!("divider {} top front", i + 1),
            ));
        }
    }
    let c0 = t.compartment_center(0);
    if notch {
        v.push(([c0, hw, t.h - t.notch_d / 2.0], false, "notch".into()));
        v.push((
            [c0 + t.notch_w / 2.0 + 0.5, hw, t.h - 0.5],
            true,
            "wall beside the notch".into(),
        ));
        v.push((
            [c0, hw, t.h - t.notch_d - 0.5],
            true,
            "wall below the notch".into(),
        ));
    } else {
        v.push((
            [c0, hw, t.h - t.notch_d / 2.0],
            true,
            "front wall (no notch)".into(),
        ));
    }
    v
}

pub fn assert_occupancy(s: &Session, t: &Tray, notch: bool) {
    let probes = occupancy(t, notch);
    let pts: Vec<[f64; 3]> = probes.iter().map(|p| p.0).collect();
    let got = probe(s, &pts);
    for ((pt, want, what), g) in probes.iter().zip(got) {
        assert_eq!(g, Some(*want), "{what} at {pt:?} for {t:?}");
    }
}
