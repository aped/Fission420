//! Everyday printable parts built with the current modeling features, as
//! committed replayable workflows (`examples/<name>_workflow.json`, generated
//! here; regenerate with `RCAD_WRITE_EXAMPLES=1`) and saved documents
//! (`examples/<name>.rcad.json`):
//!
//! 1. `everyday_enclosure`: rounded-rectangle base (four vertical edge
//!    fillets) shelled from the top face, then its bottom outer loop filleted
//!    (the reverse order, fillet then shell, is refused); a lid with the same
//!    outline, its top outer loop filleted, and four through holes from a 2 × 2
//!    rectangular pattern; the width is a user parameter driving both outlines
//!    and the hole pattern.
//! 2. `everyday_tray`: 3 × 2 organizer, rounded outline, top outer loop
//!    chamfered, rounded compartments cut from one rounded sketch and repeated by
//!    a rectangular pattern.
//! 3. `everyday_knob`: revolved knob whose profile touches the axis, with a
//!    blind D-shaft hole (one arc + one line sketch).
//! 4. `threaded_pair`: modeled Ø20 × 2 rod and square nut.
//! 5. `split_bar`: a bar cut through into two pieces (separate bodies).
//!
//! Expected volumes and bounds are closed form from the dimensions (fillet and
//! chamfer loop removals by Pappus, rounded-rectangle areas, cylinder segment
//! areas, frustum volumes) or, for the thread, FreeCAD's read-back volumes
//! recorded in `tests/thread.rs`; never copied from R-CAD output.
mod support;
use rcad_commands::Session;
use rcad_document::BodyId;
use rcad_kernel::VolumeResult;
use rcad_sketch::{Constraint, Dimension, EntityId, PointId, Sketch};
use serde_json::{Value, json};
use std::f64::consts::PI;
use support::tray::{exec, ok};

// ------------------------------------------------------------------ helpers

fn repo() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn volume(s: &Session, body: u32) -> f64 {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id == BodyId(body))
        .unwrap_or_else(|| panic!("body {body}"));
    match b.measurements.expect("measured").volume {
        VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn bounds(s: &Session, body: u32) -> [f64; 6] {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id == BodyId(body))
        .unwrap();
    let m = b.measurements.expect("measured");
    let (lo, hi) = (m.bounds.min, m.bounds.max);
    [lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]]
}

/// Inside/outside of world points against body `body` (exact winding number).
fn probe(s: &Session, body: u32, pts: &[[f64; 3]]) -> Vec<Option<bool>> {
    let m = s.model().expect("model");
    let b = m
        .evaluation()
        .body_by_id(BodyId(body))
        .expect("body")
        .geometry
        .brep()
        .expect("brep");
    rcad_kernel::point_membership(b, pts).expect("membership")
}

fn rel_close(got: f64, want: f64, rel: f64, what: &str) {
    assert!(
        (got - want).abs() <= rel * want.abs().max(1.0),
        "{what}: {got} vs closed form {want}"
    );
}

fn bounds_close(got: [f64; 6], want: [f64; 6], tol: f64, what: &str) {
    for i in 0..6 {
        assert!(
            (got[i] - want[i]).abs() <= tol,
            "{what}: bounds {got:?} vs {want:?}"
        );
    }
}

/// Rounded rectangle area.
fn rr(l: f64, w: f64, r: f64) -> f64 {
    l * w - (4.0 - PI) * r * r
}

/// Material removed by a loop blend around a rounded-rectangle prism outline
/// (`l` × `w`, corner radius `big`): cross-section area `a` with first moment
/// `m` about the wall, along the straight sides and (Pappus) around the four
/// quarter-turn corners.
fn loop_removal(l: f64, w: f64, big: f64, a: f64, m: f64) -> f64 {
    let straight = 2.0 * (l - 2.0 * big) + 2.0 * (w - 2.0 * big);
    a * straight + 2.0 * PI * (a * big - m)
}

/// Fillet of radius r: spandrel area and first moment.
fn fillet_am(r: f64) -> (f64, f64) {
    (r * r * (1.0 - PI / 4.0), r.powi(3) * (5.0 / 6.0 - PI / 4.0))
}

/// 45° chamfer of leg c: triangle area and first moment (centroid c/3 from the wall).
fn chamfer_am(c: f64) -> (f64, f64) {
    (c * c / 2.0, c.powi(3) / 6.0)
}

fn side(feature: u32, e: u32) -> Value {
    json!({"feature": feature, "role": {"ExtrudeSide": {"entity": e}}})
}

fn cap(feature: u32, end: bool) -> Value {
    json!({"feature": feature, "role": if end { "ExtrudeEndCap" } else { "ExtrudeStartCap" }})
}

fn vertical_fillets(extrude: u32, r: f64) -> Vec<Value> {
    [(1, 2), (2, 3), (3, 4), (4, 1)]
        .iter()
        .map(|(a, b)| {
            json!({"cmd": "fillet", "a": side(extrude, *a), "b": side(extrude, *b), "radius": r})
        })
        .collect()
}

/// Rounded rectangle sketch: lower-left corner (x0, y0), size w × h, corner
/// radius r (sketch-plane mm, dimensions named for parameter edits).
fn rounded_rect(x0: f64, y0: f64, w: f64, h: f64, r: f64) -> Sketch {
    let mut s = Sketch::default();
    let p: Vec<PointId> = [[x0, y0], [x0 + w, y0], [x0 + w, y0 + h], [x0, y0 + h]]
        .iter()
        .map(|q| s.add_point(q[0], q[1]))
        .collect();
    let l: Vec<EntityId> = (0..4).map(|i| s.add_line(p[i], p[(i + 1) % 4])).collect();
    s.constraints.extend([
        Constraint::FixedPoint {
            p: p[0],
            x: x0,
            y: y0,
        },
        Constraint::Horizontal { line: l[0] },
        Constraint::Horizontal { line: l[2] },
        Constraint::Vertical { line: l[1] },
        Constraint::Vertical { line: l[3] },
        Constraint::HorizontalDistance {
            a: p[0],
            b: p[1],
            dim: Dimension::new("w", w),
        },
        Constraint::VerticalDistance {
            a: p[1],
            b: p[2],
            dim: Dimension::new("h", h),
        },
    ]);
    assert!(s.solve().is_success());
    s.fillet_corner(p[0], r, Some("r")).unwrap();
    for c in &p[1..] {
        s.fillet_corner(*c, r, Some("rx")).unwrap();
    }
    s
}

// ------------------------------------------------------------ the five parts

/// Enclosure dimensions (mm).
struct Encl {
    w: f64,
    d: f64,
    h: f64,
    r: f64,
    wall: f64,
    foot: f64,
    lid_t: f64,
    lid_z: f64,
    hole_d: f64,
    inset: f64,
}

const ENCL: Encl = Encl {
    w: 80.0,
    d: 50.0,
    h: 30.0,
    r: 6.0,
    wall: 2.0,
    foot: 1.5,
    lid_t: 3.0,
    lid_z: 35.0,
    hole_d: 3.4,
    inset: 8.0,
};

impl Encl {
    fn base(&self, w: f64) -> f64 {
        let (a, m) = fillet_am(self.foot);
        rr(w, self.d, self.r) * self.h
            - rr(
                w - 2.0 * self.wall,
                self.d - 2.0 * self.wall,
                self.r - self.wall,
            ) * (self.h - self.wall)
            - loop_removal(w, self.d, self.r, a, m)
    }
    fn lid(&self, w: f64) -> f64 {
        let (a, m) = fillet_am(self.foot);
        rr(w, self.d, self.r) * self.lid_t
            - loop_removal(w, self.d, self.r, a, m)
            - 4.0 * PI * (self.hole_d / 2.0).powi(2) * self.lid_t
    }
}

fn enclosure_steps(save: &str) -> Vec<Value> {
    let e = &ENCL;
    let mut v = vec![
        json!({"cmd": "new_document"}),
        json!({"cmd": "set_user_parameter", "name": "width", "expression": format!("{}", e.w),
               "unit": "mm", "comment": "outside X of base and lid"}),
        // F1 base outline, F2 base block.
        json!({"cmd": "create_sketch", "name": "Base outline", "plane": "WorldXY",
               "template": "centered_rectangle", "width": e.w, "height": e.d}),
        json!({"cmd": "set_expression", "feature": 1, "name": "width", "expression": "width"}),
        json!({"cmd": "extrude", "sketch": 1, "distance": e.h, "body_name": "Enclosure base"}),
    ];
    // F3..F6 vertical edges.
    v.extend(vertical_fillets(2, e.r));
    // F7 shell open at the top, F8 bottom outer loop rounded (radius <= wall).
    v.push(json!({"cmd": "shell_face", "face": cap(2, true), "thickness": e.wall}));
    v.push(json!({"cmd": "fillet_loop", "face": cap(2, false), "radius": e.foot}));
    // Lid: F9 outline above the base, F10 plate, F11..F14 vertical edges, F15
    // top outer loop rounded.
    v.push(json!({"cmd": "create_sketch", "name": "Lid outline",
                  "plane": {"Offset": {"base": "WorldXY", "distance": e.lid_z}},
                  "template": "centered_rectangle", "width": e.w, "height": e.d}));
    v.push(json!({"cmd": "set_expression", "feature": 9, "name": "width", "expression": "width"}));
    v.push(
        json!({"cmd": "extrude", "sketch": 9, "distance": e.lid_t, "body_name": "Enclosure lid"}),
    );
    v.extend(vertical_fillets(10, e.r));
    v.push(json!({"cmd": "fillet_loop", "face": cap(10, true), "radius": e.foot}));
    // F16 first screw hole, F17 its cut, F18 the 2 × 2 pattern.
    let (cx, cy) = (e.w / 2.0 - e.inset, e.d / 2.0 - e.inset);
    v.push(json!({"cmd": "create_sketch", "name": "Screw hole",
                  "plane": {"Offset": {"base": "WorldXY", "distance": e.lid_z + e.lid_t}},
                  "template": "centered_circle", "diameter": e.hole_d, "center": [cx, cy]}));
    v.push(
        json!({"cmd": "set_expression", "feature": 16, "name": "center_x",
                  "expression": format!("width / 2 - {}", e.inset)}),
    );
    v.push(json!({"cmd": "extrude", "sketch": 16, "op": "cut", "target": 2, "through_all": true}));
    v.push(
        json!({"cmd": "linear_pattern", "feature": 17, "count": 2, "spacing": 2.0 * cx,
                  "direction": [-1.0, 0.0],
                  "second": {"count": 2, "spacing": 2.0 * cy, "direction": [0.0, -1.0]}}),
    );
    v.push(
        json!({"cmd": "set_expression", "feature": 18, "name": "spacing",
                  "expression": format!("width - {}", 2.0 * e.inset)}),
    );
    v.push(json!({"cmd": "save", "path": save}));
    v
}

/// Tray dimensions (mm): 3 × 2 compartments.
struct TrayP {
    w: f64,
    d: f64,
    h: f64,
    r: f64,
    wall: f64,
    floor: f64,
    rc: f64,
    chamfer: f64,
}

const TRAY: TrayP = TrayP {
    w: 120.0,
    d: 80.0,
    h: 25.0,
    r: 5.0,
    wall: 2.0,
    floor: 2.0,
    rc: 4.0,
    chamfer: 1.0,
};

impl TrayP {
    fn cell(&self) -> (f64, f64) {
        (
            (self.w - 4.0 * self.wall) / 3.0,
            (self.d - 3.0 * self.wall) / 2.0,
        )
    }
    fn volume(&self) -> f64 {
        let (cw, ch) = self.cell();
        let (a, m) = chamfer_am(self.chamfer);
        rr(self.w, self.d, self.r) * self.h
            - loop_removal(self.w, self.d, self.r, a, m)
            - 6.0 * rr(cw, ch, self.rc) * (self.h - self.floor)
    }
}

fn tray_steps(save: &str) -> Vec<Value> {
    let t = &TRAY;
    let (cw, ch) = t.cell();
    let mut v = vec![
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "name": "Tray outline", "plane": "WorldXY",
               "template": "centered_rectangle", "width": t.w, "height": t.d,
               "center": [t.w / 2.0, t.d / 2.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": t.h, "body_name": "Tray"}),
    ];
    v.extend(vertical_fillets(2, t.r));
    // F7 top outer loop chamfer (before the pockets: the loop's face is then a
    // plain prism cap).
    v.push(json!({"cmd": "chamfer_loop", "face": cap(2, true), "distance": t.chamfer}));
    // F8 first compartment (rounded), F9 pocket, F10 3 × 2 pattern.
    let sk = rounded_rect(t.wall, t.wall, cw, ch, t.rc);
    v.push(json!({"cmd": "create_sketch", "name": "Compartment",
                  "plane": {"Offset": {"base": "WorldXY", "distance": t.floor}},
                  "template": "custom", "sketch": serde_json::to_value(&sk).unwrap()}));
    v.push(
        json!({"cmd": "extrude", "sketch": 8, "op": "cut", "target": 1,
                  "distance": t.h - t.floor + 1.0}),
    );
    v.push(
        json!({"cmd": "linear_pattern", "feature": 9, "count": 3, "spacing": cw + t.wall,
                  "direction": [1.0, 0.0],
                  "second": {"count": 2, "spacing": ch + t.wall, "direction": [0.0, 1.0]}}),
    );
    v.push(json!({"cmd": "save", "path": save}));
    v
}

/// Knob: base radius, base height, top radius, total height, D-shaft radius,
/// flat distance from the axis, shaft depth (mm).
const KNOB: [f64; 7] = [20.0, 6.0, 15.0, 14.0, 3.0, 1.5, 10.0];

fn knob_volume() -> f64 {
    let [rb, hb, rt, h, rs, flat, depth] = KNOB;
    let frustum = PI * (h - hb) / 3.0 * (rb * rb + rb * rt + rt * rt);
    let seg = rs * rs * (flat / rs).acos() - flat * (rs * rs - flat * flat).sqrt();
    PI * rb * rb * hb + frustum - (PI * rs * rs - seg) * depth
}

/// D profile: circle of radius `r` about the origin cut by the line x = `flat`.
fn d_shape(r: f64, flat: f64) -> Sketch {
    let mut s = Sketch::default();
    let y = (r * r - flat * flat).sqrt();
    let a = s.add_point(flat, -y);
    let b = s.add_point(flat, y);
    s.add_line(a, b);
    s.add_arc_3pt(b, a, [-r, 0.0]).expect("arc");
    s
}

fn knob_steps(save: &str) -> Vec<Value> {
    let [rb, hb, rt, h, rs, flat, depth] = KNOB;
    vec![
        json!({"cmd": "new_document"}),
        // F1 half profile on XZ touching the axis (x = 0) along two edges' ends.
        json!({"cmd": "create_sketch", "name": "Knob profile", "plane": "WorldXZ",
               "template": "polygon",
               "vertices": [[0.0, 0.0], [rb, 0.0], [rb, hb], [rt, h], [0.0, h]]}),
        json!({"cmd": "revolve", "sketch": 1, "angle_deg": 360.0}),
        json!({"cmd": "rename_body", "body": 1, "name": "Knob"}),
        // F3 shaft circle on the bottom, F4 blind cut up into the knob.
        json!({"cmd": "create_sketch", "name": "D shaft", "plane": "WorldXY",
               "template": "custom", "sketch": serde_json::to_value(d_shape(rs, flat)).unwrap()}),
        json!({"cmd": "extrude", "sketch": 3, "op": "cut", "target": 1, "distance": depth}),
        json!({"cmd": "save", "path": save}),
    ]
}

/// FreeCAD's read-back volumes of the Ø20 × 2 rod and the 30 mm square nut
/// (`tests/thread.rs`), mm³.
const ROD_V: f64 = 3215.09586;
const NUT_V: f64 = 7515.00173;

fn thread_steps(save: &str) -> Vec<Value> {
    vec![
        json!({"cmd": "new_document"}),
        json!({"cmd": "thread", "plane": "WorldXY", "diameter": 20.0, "pitch": 2.0,
               "length": 12.0, "body_name": "Rod M20x2"}),
        json!({"cmd": "thread", "plane": "WorldXY", "diameter": 20.0, "pitch": 2.0,
               "length": 12.0, "nut_width": 30.0, "body_name": "Nut M20x2"}),
        json!({"cmd": "save", "path": save}),
    ]
}

/// Bar length, width, height and the slot width cut through it (mm).
const BAR: [f64; 4] = [100.0, 20.0, 10.0, 4.0];

fn split_steps(save: &str) -> Vec<Value> {
    let [l, w, h, slot] = BAR;
    vec![
        json!({"cmd": "new_document"}),
        json!({"cmd": "create_sketch", "name": "Bar", "plane": "WorldXY",
               "template": "centered_rectangle", "width": l, "height": w,
               "center": [l / 2.0, w / 2.0]}),
        json!({"cmd": "extrude", "sketch": 1, "distance": h, "body_name": "Bar"}),
        json!({"cmd": "create_sketch", "name": "Slot", "plane": "WorldXY",
               "template": "centered_rectangle", "width": slot, "height": w + 10.0,
               "center": [l / 2.0, w / 2.0]}),
        json!({"cmd": "extrude", "sketch": 3, "op": "cut", "target": 1, "through_all": true}),
        json!({"cmd": "save", "path": save}),
    ]
}

/// (name, steps).
fn parts() -> Vec<(&'static str, Vec<Value>)> {
    let p = |n: &str| format!("examples/{n}.rcad.json");
    vec![
        (
            "everyday_enclosure",
            enclosure_steps(&p("everyday_enclosure")),
        ),
        ("everyday_tray", tray_steps(&p("everyday_tray"))),
        ("everyday_knob", knob_steps(&p("everyday_knob"))),
        ("threaded_pair", thread_steps(&p("threaded_pair"))),
        ("split_bar", split_steps(&p("split_bar"))),
    ]
}

/// Replay `steps`; a `save` goes to `save_dir/<file name>` instead.
fn replay(steps: &[Value], save_dir: &std::path::Path) -> Session {
    let mut s = Session::new();
    for v in steps {
        let mut v = v.clone();
        if v["cmd"] == "save" {
            let name = std::path::Path::new(v["path"].as_str().unwrap())
                .file_name()
                .unwrap()
                .to_owned();
            v["path"] = json!(save_dir.join(name));
        }
        ok(&mut s, &v);
    }
    s
}

/// The committed workflow is this file's output (regenerate with
/// `RCAD_WRITE_EXAMPLES=1 cargo test -p rcad-commands --test printable_corpus`,
/// which also rewrites the saved documents through the workflows' `save`).
#[test]
fn committed_everyday_workflows_are_current() {
    let write = std::env::var_os("RCAD_WRITE_EXAMPLES").is_some();
    for (name, steps) in parts() {
        let path = repo()
            .join("examples")
            .join(format!("{name}_workflow.json"));
        let json = Value::Array(steps.clone());
        if write {
            std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap() + "\n").unwrap();
            replay(&steps, &repo().join("examples"));
        }
        let committed: Value = serde_json::from_str(
            &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
        )
        .unwrap();
        assert_eq!(
            committed, json,
            "{name}: stale; regenerate with RCAD_WRITE_EXAMPLES=1"
        );
    }
}

/// Closed-form volume (exact B-rep, 1e-9 relative) and bounds of every body.
fn check_part(name: &str, s: &Session) {
    let tol = 1e-6;
    match name {
        "everyday_enclosure" => {
            let e = &ENCL;
            rel_close(volume(s, 1), e.base(e.w), 1e-9, "enclosure base");
            rel_close(volume(s, 2), e.lid(e.w), 1e-9, "enclosure lid");
            let (hw, hd) = (e.w / 2.0, e.d / 2.0);
            bounds_close(bounds(s, 1), [-hw, -hd, 0.0, hw, hd, e.h], tol, "base");
            bounds_close(
                bounds(s, 2),
                [-hw, -hd, e.lid_z, hw, hd, e.lid_z + e.lid_t],
                tol,
                "lid",
            );
        }
        "everyday_tray" => {
            rel_close(volume(s, 1), TRAY.volume(), 1e-9, "tray");
            bounds_close(
                bounds(s, 1),
                [0.0, 0.0, 0.0, TRAY.w, TRAY.d, TRAY.h],
                tol,
                "tray",
            );
        }
        "everyday_knob" => {
            rel_close(volume(s, 1), knob_volume(), 1e-9, "knob");
            let r = KNOB[0];
            bounds_close(bounds(s, 1), [-r, -r, 0.0, r, r, KNOB[3]], tol, "knob");
        }
        "threaded_pair" => {
            assert!((volume(s, 1) - ROD_V).abs() < 1e-3, "rod {}", volume(s, 1));
            assert!((volume(s, 2) - NUT_V).abs() < 1e-3, "nut {}", volume(s, 2));
            bounds_close(
                bounds(s, 2),
                [-15.0, -15.0, 0.0, 15.0, 15.0, 12.0],
                tol,
                "nut",
            );
            let b = bounds(s, 1);
            assert!(b[2].abs() < tol && (b[5] - 12.0).abs() < tol, "rod z {b:?}");
            assert!(
                b[0] >= -10.0 - tol && b[3] <= 10.0 + tol,
                "rod within Ø20 {b:?}"
            );
        }
        "split_bar" => {
            let [l, w, h, slot] = BAR;
            let piece = (l - slot) / 2.0;
            let names: Vec<String> = s.summary().bodies.iter().map(|b| b.name.clone()).collect();
            assert_eq!(names, ["Bar", "Bar (2)"], "pieces named after the source");
            rel_close(volume(s, 1), piece * w * h, 1e-9, "piece 1");
            rel_close(volume(s, 2), piece * w * h, 1e-9, "piece 2");
            bounds_close(bounds(s, 1), [0.0, 0.0, 0.0, piece, w, h], tol, "piece 1");
            bounds_close(
                bounds(s, 2),
                [piece + slot, 0.0, 0.0, l, w, h],
                tol,
                "piece 2",
            );
        }
        _ => unreachable!(),
    }
}

/// Point probes of the features (air / material), from the dimensions.
fn probe_part(name: &str, s: &Session) {
    match name {
        "everyday_enclosure" => {
            let e = &ENCL;
            let (hx, hy) = (e.w / 2.0 - e.inset, e.d / 2.0 - e.inset);
            let zl = e.lid_z + e.lid_t / 2.0;
            let mut pts = vec![];
            for sx in [-1.0, 1.0] {
                for sy in [-1.0, 1.0] {
                    pts.push([sx * hx, sy * hy, zl]); // screw hole: air
                }
            }
            pts.push([0.0, 0.0, zl]); // lid plate: material
            assert_eq!(
                probe(s, 2, &pts),
                vec![
                    Some(false),
                    Some(false),
                    Some(false),
                    Some(false),
                    Some(true)
                ],
                "lid holes"
            );
            // Base: cavity air, wall and floor material, rounded outer corner air.
            let c = e.r * (1.0 - std::f64::consts::FRAC_1_SQRT_2) / 2.0;
            assert_eq!(
                probe(
                    s,
                    1,
                    &[
                        [0.0, 0.0, e.h / 2.0],
                        [e.w / 2.0 - e.wall / 2.0, 0.0, e.h / 2.0],
                        [0.0, 0.0, e.wall / 2.0],
                        [e.w / 2.0 - c, e.d / 2.0 - c, e.h / 2.0],
                        // Bottom outer edge rounded: air at the old sharp edge.
                        [0.0, -e.d / 2.0 + 0.1, 0.1],
                    ]
                ),
                vec![
                    Some(false),
                    Some(true),
                    Some(true),
                    Some(false),
                    Some(false)
                ],
                "base"
            );
        }
        "everyday_tray" => {
            let t = &TRAY;
            let (cw, ch) = t.cell();
            let mut pts = vec![];
            let mut want = vec![];
            for i in 0..3 {
                for j in 0..2 {
                    let x = t.wall + cw / 2.0 + f64::from(i) * (cw + t.wall);
                    let y = t.wall + ch / 2.0 + f64::from(j) * (ch + t.wall);
                    pts.push([x, y, t.h / 2.0]); // compartment air
                    want.push(Some(false));
                    pts.push([x, y, t.floor / 2.0]); // floor material
                    want.push(Some(true));
                }
            }
            // Divider between columns 1 and 2, mid-height.
            pts.push([t.wall + cw + t.wall / 2.0, t.wall + ch / 2.0, t.h / 2.0]);
            want.push(Some(true));
            // Compartment corner rounded: material just inside the corner.
            pts.push([t.wall + 0.3, t.wall + 0.3, t.h / 2.0]);
            want.push(Some(true));
            // Top outer edge chamfered: air at the corner of the chamfer.
            pts.push([t.w / 2.0, 0.2, t.h - 0.2]);
            want.push(Some(false));
            assert_eq!(probe(s, 1, &pts), want, "tray");
        }
        "everyday_knob" => {
            let [_, _, _, _, rs, flat, depth] = KNOB;
            assert_eq!(
                probe(
                    s,
                    1,
                    &[
                        [0.0, 0.0, depth / 2.0],
                        [-rs + 0.3, 0.0, depth / 2.0],
                        [flat + 0.3, 0.0, depth / 2.0],
                        [0.0, 0.0, depth + 0.5],
                    ]
                ),
                vec![Some(false), Some(false), Some(true), Some(true)],
                "D shaft: round side air, flat side material, closed above"
            );
        }
        "split_bar" => {
            let [l, w, h, _] = BAR;
            let gap = [[l / 2.0, w / 2.0, h / 2.0]];
            assert_eq!(probe(s, 1, &gap), vec![Some(false)]);
            assert_eq!(probe(s, 2, &gap), vec![Some(false)]);
        }
        _ => {}
    }
}

#[test]
fn everyday_parts_match_closed_form_and_saved_documents() {
    let dir = tempfile::tempdir().unwrap();
    for (name, steps) in parts() {
        let s = replay(&steps, dir.path());
        check_part(name, &s);
        probe_part(name, &s);
        // The committed document regenerates to the same closed-form geometry.
        let mut c = Session::new();
        ok(
            &mut c,
            &json!({"cmd": "open", "path": repo().join(format!("examples/{name}.rcad.json"))}),
        );
        check_part(name, &c);
        assert_eq!(
            c.model().unwrap().document(),
            s.model().unwrap().document(),
            "{name}: committed document differs from the workflow's"
        );
    }
}

#[test]
fn enclosure_width_edit_moves_everything_upstream() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = replay(&enclosure_steps("x.rcad.json"), dir.path());
    let e = &ENCL;
    let r = exec(
        &mut s,
        &json!({"cmd": "set_user_parameter", "name": "width", "expression": "100", "unit": "mm"}),
    );
    assert!(r.ok, "{:?}", r.error);
    rel_close(volume(&s, 1), e.base(100.0), 1e-9, "base at width 100");
    rel_close(volume(&s, 2), e.lid(100.0), 1e-9, "lid at width 100");
    let hx = 50.0 - e.inset;
    let hy = e.d / 2.0 - e.inset;
    let zl = e.lid_z + e.lid_t / 2.0;
    assert_eq!(
        probe(
            &s,
            2,
            &[[hx, hy, zl], [-hx, -hy, zl], [40.0 - e.inset, hy, zl]]
        ),
        vec![Some(false), Some(false), Some(true)],
        "holes follow the width; the old position is material"
    );
    bounds_close(
        bounds(&s, 1),
        [-50.0, -25.0, 0.0, 50.0, 25.0, e.h],
        1e-6,
        "base",
    );
    // Undo restores the baseline.
    ok(&mut s, &json!({"cmd": "undo"}));
    rel_close(volume(&s, 1), e.base(e.w), 1e-9, "base after undo");
    rel_close(volume(&s, 2), e.lid(e.w), 1e-9, "lid after undo");
}
