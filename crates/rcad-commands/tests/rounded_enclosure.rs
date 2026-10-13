//! Run-6 acceptance project 1: a rounded, labelled enclosure built from New
//! with general tools only (no template loader): parameters; a block with
//! solid-filleted vertical edges; a cavity whose outline is the projected top
//! face offset inward by the wall (associative: it follows the block and the
//! corner radius); four free-standing screw bosses up to the rim plane with
//! pilot holes; a separate lid whose outline projects the base's bottom face,
//! with four counterbored holes and raised "B8" text (glyph counters kept).
//!
//! Oracles are closed-form from the parameters (rounded rectangle areas,
//! cylinders, counterbores) plus the glyph areas of the text regions (shoelace
//! areas of the stored outlines, independent of the B-rep), and point probes
//! for wall/floor thickness, the rounded corners, boss/hole positions and the
//! glyph counters. Upstream edits (length, corner radius), an excessive radius
//! refused unchanged, undo/redo, save/reopen and STEP/STL/3MF exports.
//!
//! Needs the installed font "Liberation Sans" (`RCAD_SKIP_FONT_TESTS=1` skips).
//! `RCAD_EVIDENCE_DIR=<dir>` also writes the native document and exports there
//! for the external validators (FreeCAD, mesh checker, slicer).
use rcad_commands::{Command, Response, Session, SketchTemplate};
use rcad_document::{FeatureId, FeatureKind, SketchPlane};
use rcad_sketch::{FontSpec, HAlign, Sketch, TextSpec, VAlign};
use serde_json::{Value, json};
use std::f64::consts::PI;

const FAMILY: &str = "Liberation Sans";

fn have_font() -> bool {
    match rcad_sketch::font::find_font(&FontSpec::family(FAMILY)) {
        Ok(_) => true,
        Err(e) if std::env::var("RCAD_SKIP_FONT_TESTS").as_deref() == Ok("1") => {
            eprintln!("SKIPPED (RCAD_SKIP_FONT_TESTS=1): {e}");
            false
        }
        Err(e) => panic!("{e}; install {FAMILY} or set RCAD_SKIP_FONT_TESTS=1"),
    }
}

/// Parameters (mm).
#[derive(Clone, Copy, Debug)]
struct P {
    length: f64,
    width: f64,
    height: f64,
    wall: f64,
    floor: f64,
    r: f64,
    boss_d: f64,
    pilot_d: f64,
    pilot_depth: f64,
    lid_t: f64,
    lid_gap: f64,
    clear_d: f64,
    cbore_d: f64,
    cbore_depth: f64,
    text_h: f64,
    text_raise: f64,
}

const P0: P = P {
    length: 100.0,
    width: 70.0,
    height: 30.0,
    wall: 2.4,
    floor: 2.4,
    r: 6.0,
    boss_d: 7.0,
    pilot_d: 2.5,
    pilot_depth: 8.0,
    lid_t: 2.4,
    lid_gap: 10.0,
    clear_d: 3.2,
    cbore_d: 6.0,
    cbore_depth: 1.2,
    text_h: 12.0,
    text_raise: 0.8,
};

impl P {
    fn rr_area(l: f64, w: f64, r: f64) -> f64 {
        l * w - (4.0 - PI) * r * r
    }
    /// Boss centre offset from the middle (x, y): 1 mm clear of the walls.
    fn boss(&self) -> (f64, f64) {
        let g = self.wall + self.boss_d / 2.0 + 1.0;
        (self.length / 2.0 - g, self.width / 2.0 - g)
    }
    fn base_volume(&self) -> f64 {
        let (l, w, h, t, f, r) = (self.length, self.width, self.height, self.wall, self.floor, self.r);
        let inner_r = r - t;
        assert!(inner_r > 0.0, "the cavity corner needs r > wall");
        Self::rr_area(l, w, r) * h - Self::rr_area(l - 2.0 * t, w - 2.0 * t, inner_r) * (h - f)
            + 4.0 * PI * (self.boss_d / 2.0).powi(2) * (h - f)
            - 4.0 * PI * (self.pilot_d / 2.0).powi(2) * self.pilot_depth
    }
    fn lid_volume(&self, text_area: f64) -> f64 {
        Self::rr_area(self.length, self.width, self.r) * self.lid_t
            - 4.0
                * (PI * (self.cbore_d / 2.0).powi(2) * self.cbore_depth
                    + PI * (self.clear_d / 2.0).powi(2) * (self.lid_t - self.cbore_depth))
            + text_area * self.text_raise
    }
}

struct Built {
    s: Session,
    /// Commands as sent (for the replayable example workflow).
    log: Vec<Value>,
    /// Feature ids.
    block: FeatureId,
    fillets: Vec<FeatureId>,
    text_sketch: FeatureId,
}

impl Built {
    fn cmd(&mut self, c: Command) -> Response {
        self.log.push(serde_json::to_value(&c).unwrap());
        let r = self.s.execute(&c);
        assert!(r.ok, "{c:?}: {:?}", r.error);
        r
    }
    fn json(&mut self, v: Value) -> Response {
        let c: Command = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{v}: {e}"));
        self.cmd(c)
    }
    fn bind(&mut self, f: FeatureId, name: &str, expr: &str) {
        self.json(json!({"cmd": "set_expression", "feature": f, "name": name,
                         "expression": expr}));
    }
    fn sketch(&self, f: FeatureId) -> Sketch {
        match &self.s.model().unwrap().document().feature(f).unwrap().kind {
            FeatureKind::Sketch { sketch, .. } => sketch.clone(),
            _ => panic!("{f} is not a sketch"),
        }
    }
    fn solved(&self, f: FeatureId) -> Sketch {
        self.s
            .summary()
            .features
            .iter()
            .find(|x| x.id == f)
            .and_then(|x| x.sketch.clone())
            .expect("solved sketch")
    }
}

fn plane_z(z: f64) -> Value {
    json!({"Offset": {"base": "WorldXY", "distance": z}})
}

fn empty_sketch(b: &mut Built, plane: Value) -> FeatureId {
    let plane: SketchPlane = serde_json::from_value(plane).unwrap();
    b.cmd(Command::CreateSketch {
        name: None,
        plane,
        template: SketchTemplate::Custom {
            sketch: Sketch::default(),
        },
    })
    .feature
    .unwrap()
}

fn build(p: &P) -> Built {
    let mut b = Built {
        s: Session::new(),
        log: vec![],
        block: FeatureId(0),
        fillets: vec![],
        text_sketch: FeatureId(0),
    };
    b.json(json!({"cmd": "new_document"}));
    for (n, v, c) in [
        ("length", p.length, "outside X"),
        ("width", p.width, "outside Y"),
        ("height", p.height, "base outside height"),
        ("wall", p.wall, "side wall"),
        ("floor", p.floor, "floor thickness"),
        ("corner_r", p.r, "outer vertical corner radius"),
        ("boss_d", p.boss_d, "screw boss diameter"),
        ("lid_t", p.lid_t, "lid thickness"),
    ] {
        b.json(json!({"cmd": "set_user_parameter", "name": n, "expression": v.to_string(),
                      "unit": "mm", "comment": c}));
    }
    // Block: rectangle on XY bound to length x width, extruded height.
    let sk = b
        .json(json!({"cmd": "create_sketch", "plane": "WorldXY",
                     "template": "centered_rectangle", "width": p.length,
                     "height": p.width}))
        .feature
        .unwrap();
    b.bind(sk, "width", "length");
    b.bind(sk, "height", "width");
    b.block = b
        .json(json!({"cmd": "extrude", "sketch": sk, "distance": p.height,
                     "body_name": "Base"}))
        .feature
        .unwrap();
    let blk = b.block;
    b.bind(blk, "distance", "height");
    // Solid fillets on the four vertical edges (sides 1-2, 2-3, 3-4, 4-1).
    for (e1, e2) in [(1, 2), (2, 3), (3, 4), (4, 1)] {
        let f = b
            .json(json!({"cmd": "fillet",
                         "a": {"feature": blk, "role": {"ExtrudeSide": {"entity": e1}}},
                         "b": {"feature": blk, "role": {"ExtrudeSide": {"entity": e2}}},
                         "radius": p.r}))
            .feature
            .unwrap();
        b.bind(f, "radius", "corner_r");
        b.fillets.push(f);
    }
    // Cavity: project the (rounded) top face into a sketch on it, offset the
    // projected loop inward by the wall, cut that region floor-deep.
    let top = json!({"Face": {"feature": blk, "role": "ExtrudeEndCap"}});
    let cav = empty_sketch(&mut b, top.clone());
    b.json(json!({"cmd": "project", "sketch": cav, "source": top}));
    let mut s = b.sketch(cav);
    let first = s.entities[0].id();
    s.offset_loop(first, -p.wall, Some("wall")).expect("inward offset");
    b.cmd(Command::EditSketch {
        feature: cav,
        sketch: s,
    });
    b.bind(cav, "wall", "wall");
    let inner = b
        .solved(cav)
        .regions()
        .unwrap()
        .into_iter()
        .find(|r| r.depth == 1)
        .expect("inner region")
        .id;
    let cut = b
        .json(json!({"cmd": "extrude", "sketch": cav, "distance": p.height - p.floor,
                     "op": "cut", "target": 1, "region": inner}))
        .feature
        .unwrap();
    b.bind(cut, "distance", "height - floor");
    // Bosses on the cavity floor, up to the rim plane (z = height).
    let floor_face = json!({"Face": {"feature": cut, "role": "ExtrudeEndCap"}});
    let gx = "length / 2 - wall - boss_d / 2 - 1";
    let gy = "width / 2 - wall - boss_d / 2 - 1";
    let (bx, by) = p.boss();
    let corners = [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)];
    for (sx, sy) in corners {
        let f = b
            .json(json!({"cmd": "create_sketch", "plane": floor_face,
                         "template": "centered_circle", "diameter": p.boss_d,
                         "center": [sx * bx, sy * by]}))
            .feature
            .unwrap();
        b.bind(f, "diameter", "boss_d");
        b.bind(f, "center_x", &format!("{}({gx})", if sx < 0.0 { "-" } else { "" }));
        b.bind(f, "center_y", &format!("{}({gy})", if sy < 0.0 { "-" } else { "" }));
        let e = b
            .json(json!({"cmd": "extrude", "sketch": f, "op": "join", "target": 1,
                         "options": {"to": plane_z(p.height)}}))
            .feature
            .unwrap();
        b.bind(e, "to_offset", "height");
    }
    // Pilot holes from the rim plane into the bosses.
    for (sx, sy) in corners {
        let h = b
            .json(json!({"cmd": "hole", "plane": plane_z(p.height), "target": 1,
                         "center": [sx * bx, sy * by], "diameter": p.pilot_d,
                         "depth": p.pilot_depth}))
            .feature
            .unwrap();
        b.bind(h, "plane_offset", "height");
        b.bind(h, "center_x", &format!("{}({gx})", if sx < 0.0 { "-" } else { "" }));
        b.bind(h, "center_y", &format!("{}({gy})", if sy < 0.0 { "-" } else { "" }));
    }
    // Lid: the base's bottom outline projected to a plane lid_gap above it.
    let lz = p.height + p.lid_gap;
    let lid_sk = empty_sketch(&mut b, plane_z(lz));
    b.bind(lid_sk, "plane_offset", &format!("height + {}", p.lid_gap));
    b.json(json!({"cmd": "project", "sketch": lid_sk,
                  "source": {"Face": {"feature": blk, "role": "ExtrudeStartCap"}}}));
    let lid = b
        .json(json!({"cmd": "extrude", "sketch": lid_sk, "distance": p.lid_t,
                     "body_name": "Lid"}))
        .feature
        .unwrap();
    b.bind(lid, "distance", "lid_t");
    // Counterbored clearance holes over the bosses, from the lid top.
    for (sx, sy) in corners {
        let h = b
            .json(json!({"cmd": "hole", "plane": plane_z(lz + p.lid_t), "target": 2,
                         "center": [sx * bx, sy * by], "diameter": p.clear_d,
                         "counterbore": [p.cbore_d, p.cbore_depth]}))
            .feature
            .unwrap();
        b.bind(h, "plane_offset", &format!("height + {} + lid_t", p.lid_gap));
        b.bind(h, "center_x", &format!("{}({gx})", if sx < 0.0 { "-" } else { "" }));
        b.bind(h, "center_y", &format!("{}({gy})", if sy < 0.0 { "-" } else { "" }));
    }
    // Raised "B8" on the lid top, centred; all glyph regions in one Join.
    let ts = empty_sketch(&mut b, plane_z(lz + p.lid_t));
    b.bind(ts, "plane_offset", &format!("height + {} + lid_t", p.lid_gap));
    let mut t = TextSpec::new("B8", FAMILY, p.text_h, [0.0, 0.0]);
    t.halign = HAlign::Center;
    t.valign = VAlign::Middle;
    b.cmd(Command::AddSketchText {
        feature: ts,
        text: t,
    });
    let glyphs: Vec<_> = b
        .solved(ts)
        .regions()
        .unwrap()
        .into_iter()
        .filter(|r| r.depth == 0)
        .map(|r| r.id)
        .collect();
    assert_eq!(glyphs.len(), 2, "B and 8");
    let (first, rest) = glyphs.split_first().unwrap();
    b.json(json!({"cmd": "extrude", "sketch": ts, "distance": p.text_raise, "op": "join",
                  "target": 2, "region": first, "options": {"regions": rest}}));
    b.text_sketch = ts;
    b
}

/// Total area of the glyph regions (outer minus counters), from the sketch's
/// own outline polygons.
fn text_area(b: &Built) -> f64 {
    b.solved(b.text_sketch)
        .regions()
        .unwrap()
        .iter()
        .filter(|r| r.depth == 0)
        .map(|r| r.area)
        .sum()
}

/// Polygon of a (text) profile; glyph outlines are polygons.
fn poly(p: &rcad_sketch::Profile) -> Vec<[f64; 2]> {
    match p {
        rcad_sketch::Profile::Polygon(v) => v.clone(),
        other => panic!("expected a polygon, got {other:?}"),
    }
}

fn inside(p: [f64; 2], v: &[[f64; 2]]) -> bool {
    let mut c = false;
    for i in 0..v.len() {
        let (a, b) = (v[i], v[(i + 1) % v.len()]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0])
        {
            c = !c;
        }
    }
    c
}

/// A point well inside a region (inside the outer loop, outside its holes,
/// farthest from every vertex on a 60 x 60 grid).
fn interior(r: &rcad_sketch::Region) -> [f64; 2] {
    let outer = poly(&r.outer);
    let holes: Vec<_> = r.holes.iter().map(|(h, _)| poly(h)).collect();
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for q in &outer {
        for k in 0..2 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    let mut best = (f64::MIN, [0.0, 0.0]);
    for i in 1..60 {
        for j in 1..60 {
            let p = [
                lo[0] + (hi[0] - lo[0]) * i as f64 / 60.0,
                lo[1] + (hi[1] - lo[1]) * j as f64 / 60.0,
            ];
            if !inside(p, &outer) || holes.iter().any(|h| inside(p, h)) {
                continue;
            }
            let d = std::iter::once(&outer)
                .chain(holes.iter())
                .flat_map(|l| l.iter())
                .map(|q| (q[0] - p[0]).hypot(q[1] - p[1]))
                .fold(f64::MAX, f64::min);
            if d > best.0 {
                best = (d, p);
            }
        }
    }
    assert!(best.0 > 0.2, "no interior point");
    best.1
}

fn vol(s: &Session, body: u32) -> f64 {
    let b = s
        .summary()
        .bodies
        .into_iter()
        .find(|b| b.id.0 == body)
        .expect("body");
    let m = b.measurements.expect("measured");
    assert!(m.all_shells_closed, "body {body} is not closed");
    assert_eq!(m.shell_count, 1, "body {body}: one connected solid");
    match m.volume {
        rcad_kernel::VolumeResult::Value { volume, .. } => volume,
        v => panic!("{v:?}"),
    }
}

fn probe(s: &Session, body: u32, pts: &[[f64; 3]]) -> Vec<Option<bool>> {
    let m = s.model().unwrap();
    let g = m
        .evaluation()
        .body_by_id(rcad_document::BodyId(body))
        .unwrap()
        .geometry
        .brep()
        .unwrap();
    rcad_kernel::point_membership(g, pts).unwrap()
}

/// Wall, corner, floor, boss and hole probes of the base (true = material).
fn base_probes(p: &P) -> Vec<([f64; 3], bool, &'static str)> {
    let (l2, w2, h, t, f, r) = (p.length / 2.0, p.width / 2.0, p.height, p.wall, p.floor, p.r);
    let mid = h / 2.0;
    let (bx, by) = p.boss();
    // Corner arc centre (outer and inner arcs share it).
    let (cx, cy) = (l2 - r, w2 - r);
    let d = std::f64::consts::FRAC_1_SQRT_2;
    vec![
        ([l2 - t / 2.0, 0.0, mid], true, "right wall"),
        ([l2 - t - 0.05, 0.0, mid], false, "inside the right wall"),
        ([l2 + 0.05, 0.0, mid], false, "outside the right wall"),
        ([0.0, -w2 + t / 2.0, mid], true, "front wall"),
        ([0.0, -w2 + t + 0.05, mid], false, "inside the front wall"),
        ([0.0, 0.0, f / 2.0], true, "floor"),
        ([0.0, 0.0, f + 0.05], false, "above the floor"),
        ([cx + (r - t / 2.0) * d, cy + (r - t / 2.0) * d, mid], true, "rounded corner wall"),
        ([cx + (r + 0.05) * d, cy + (r + 0.05) * d, mid], false, "outside the round"),
        ([cx + (r - t - 0.05) * d, cy + (r - t - 0.05) * d, mid], false, "inside the round"),
        ([bx + p.boss_d / 2.0 - 0.2, by, mid], true, "boss wall"),
        ([bx + p.boss_d / 2.0 + 0.2, by, mid], false, "beside the boss"),
        ([bx, by, h - 1.0], false, "pilot hole"),
        ([bx, by, h - p.pilot_depth - 0.5], true, "below the pilot hole"),
        ([-bx, -by, h - 1.0], false, "opposite pilot hole"),
    ]
}

fn check_base(s: &Session, p: &P) {
    let pr = base_probes(p);
    let got = probe(s, 1, &pr.iter().map(|x| x.0).collect::<Vec<_>>());
    for ((pt, want, what), g) in pr.iter().zip(got) {
        assert_eq!(g, Some(*want), "{what} at {pt:?}");
    }
}

#[test]
fn rounded_labelled_enclosure_from_new() {
    if !have_font() {
        return;
    }
    let p = P0;
    let mut b = build(&p);
    assert_eq!(b.s.summary().bodies.len(), 2);
    let ta = text_area(&b);
    assert!(ta > 20.0 && ta < 200.0, "glyph area {ta}");
    let base = vol(&b.s, 1);
    assert!((base - p.base_volume()).abs() < 1e-5, "base {base} vs {}", p.base_volume());
    let lid = vol(&b.s, 2);
    assert!((lid - p.lid_volume(ta)).abs() < 1e-5, "lid {lid} vs {}", p.lid_volume(ta));
    check_base(&b.s, &p);
    // Lid: counterbore, clearance hole, plate, and the "B" counters open.
    let (bx, by) = p.boss();
    let lz = p.height + p.lid_gap;
    let lp = probe(
        &b.s,
        2,
        &[
            [bx, by, lz + p.lid_t - 0.5],
            [bx + p.cbore_d / 2.0 - 0.3, by, lz + p.lid_t - 0.5],
            [bx + p.clear_d / 2.0 + 0.3, by, lz + 0.5],
            [bx, by, lz + 0.5],
            [0.0, 0.0, lz + p.lid_t / 2.0],
        ],
    );
    assert_eq!(lp, vec![Some(false), Some(false), Some(true), Some(false), Some(true)]);
    // Text: a point inside a counter of "B" (a hole of a glyph region) is air
    // at the raised height; a point on the glyph body is material.
    let sk = b.solved(b.text_sketch);
    let regions = sk.regions().unwrap();
    let counter = regions
        .iter()
        .find(|r| r.depth == 1)
        .expect("a counter region");
    let c = interior(&counter.region);
    let glyph = interior(&regions.iter().find(|r| r.depth == 0).unwrap().region);
    let z = lz + p.lid_t + p.text_raise / 2.0;
    let tp = probe(&b.s, 2, &[[c[0], c[1], z], [glyph[0], glyph[1], z]]);
    assert_eq!(tp, vec![Some(false), Some(true)], "counter open, glyph raised");

    // Upstream edits: longer box, larger radius. Cavity, bosses, holes, lid
    // and text follow.
    for (name, v) in [("length", 120.0), ("corner_r", 8.0)] {
        b.json(json!({"cmd": "set_user_parameter", "name": name, "expression": v.to_string(),
                      "unit": "mm"}));
    }
    let p2 = P {
        length: 120.0,
        r: 8.0,
        ..p
    };
    assert!((vol(&b.s, 1) - p2.base_volume()).abs() < 1e-5);
    assert!((vol(&b.s, 2) - p2.lid_volume(ta)).abs() < 1e-5);
    check_base(&b.s, &p2);
    // Excessive radius (> width / 2) is refused, nothing changes.
    let before = b.s.model().unwrap().document().clone();
    let r = b.s.execute(
        &serde_json::from_value(json!({"cmd": "set_user_parameter", "name": "corner_r",
                                       "expression": "36", "unit": "mm"}))
        .unwrap(),
    );
    assert!(!r.ok, "corner radius 36 on a 70 mm wide box was accepted");
    assert_eq!(b.s.model().unwrap().document(), &before);
    // Undo / redo the coordinated edit.
    b.json(json!({"cmd": "undo"}));
    b.json(json!({"cmd": "undo"}));
    assert!((vol(&b.s, 1) - p.base_volume()).abs() < 1e-5);
    b.json(json!({"cmd": "redo"}));
    b.json(json!({"cmd": "redo"}));
    assert!((vol(&b.s, 1) - p2.base_volume()).abs() < 1e-5);
    // Save / reopen without any external file.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("enclosure.rcad.json");
    b.json(json!({"cmd": "save", "path": path}));
    let mut t = Session::new();
    let r = t.execute(&Command::Open { path: path.clone() });
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(t.model().unwrap().document(), b.s.model().unwrap().document());
    assert!((vol(&t, 1) - p2.base_volume()).abs() < 1e-5);
    assert!((vol(&t, 2) - p2.lid_volume(ta)).abs() < 1e-5);
    // Exports.
    let out = std::env::var_os("RCAD_EVIDENCE_DIR").map(std::path::PathBuf::from);
    let ex = out.clone().unwrap_or_else(|| dir.path().to_path_buf());
    std::fs::create_dir_all(&ex).unwrap();
    for c in [
        json!({"cmd": "export_step", "path": ex.join("rounded_enclosure.step")}),
        json!({"cmd": "export_stl", "path": ex.join("rounded_enclosure.stl")}),
        json!({"cmd": "export_3mf", "path": ex.join("rounded_enclosure.3mf")}),
        // Print layout: both bodies as modeled (base on its floor, lid on its
        // plate, text up), side by side on the P1S bed.
        json!({"cmd": "export_3mf", "path": ex.join("rounded_enclosure_placed.3mf"),
               "placement": {"layout": {"gap": 10}}, "bed": {"preset": "bambu_p1s"}}),
    ] {
        let c: Command = serde_json::from_value(c).unwrap();
        let r = t.execute(&c);
        assert!(r.ok, "{c:?}: {:?}", r.error);
    }
    if let Some(dir) = out {
        std::fs::copy(&path, dir.join("rounded_enclosure.rcad.json")).unwrap();
        std::fs::write(
            dir.join("rounded_enclosure_workflow.json"),
            serde_json::to_string_pretty(&b.log).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("rounded_enclosure_expected.json"),
            serde_json::to_string_pretty(&json!({
                "base_volume": p2.base_volume(), "lid_volume": p2.lid_volume(ta),
                "text_area": ta, "length": p2.length, "width": p2.width,
                "height": p2.height, "corner_r": p2.r, "wall": p2.wall,
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let _ = &b.fillets;
}
