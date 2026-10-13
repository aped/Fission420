//! Associative drawings (run 6, docs/DRAWINGS.md): a 40 x 30 x 10 block with
//! a Ø8 through hole. Every check reads the exported files back with the
//! small independent SVG and PDF readers below (not the writer's types) and
//! compares against values derived from the model definition by hand.
use rcad_commands::drawings::{DrawingFormat, ViewSpec};
use rcad_commands::{Command, Response, Session, SketchTemplate};
use rcad_document::drawing::{DrawingId, Orientation, Projection, Side, ViewId};
use rcad_document::{BodyId, FaceRef, FaceRole, FeatureId, SketchPlane};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn ok(s: &mut Session, c: Command) -> Response {
    let r = s.execute(&c);
    assert!(r.ok, "{c:?}: {:?}", r.error);
    r
}

fn out_dir() -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../artifacts/run6/drawings")
        .to_path_buf();
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Block on x [0, 40], y [0, 30], z [0, 10] with a Ø`d` through hole at
/// (`cx`, `cy`) from the top face. Returns (extrude, hole).
fn block(s: &mut Session, d: f64, cx: f64, cy: f64) -> (FeatureId, FeatureId) {
    ok(s, Command::NewDocument);
    let sk = ok(
        s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredRectangle {
                width: 40.0,
                height: 30.0,
                center: Some([20.0, 15.0]),
            },
        },
    )
    .feature
    .unwrap();
    let e = ok(s, Command::extrude(None, sk, 10.0)).feature.unwrap();
    let h = ok(
        s,
        Command::Hole {
            name: None,
            plane: SketchPlane::Face(FaceRef {
                feature: e,
                role: FaceRole::ExtrudeEndCap,
            }),
            center: [cx, cy],
            diameter: d,
            depth: None,
            counterbore: None,
            target: None,
        },
    )
    .feature
    .unwrap();
    (e, h)
}

// ------------------------------------------------------------ SVG reader

#[derive(Debug, Default, Clone)]
struct Seg {
    a: [f64; 2],
    b: [f64; 2],
}

#[derive(Debug, Default, Clone)]
struct GroupData {
    visible: Vec<Seg>,
    hidden: Vec<Seg>,
    center: Vec<Seg>,
    /// Thin lines (dimensions, leaders; hatching in a section view).
    thin: Vec<Seg>,
    /// (cx, cy, r, class)
    circles: Vec<(f64, f64, f64, String)>,
    texts: Vec<(f64, f64, String)>,
    flagged: bool,
}

/// Attributes of one XML start tag `<name a="b" ...>`.
fn attrs(tag: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let mut rest = tag;
    while let Some(eq) = rest.find("=\"") {
        let key = rest[..eq]
            .rsplit(char::is_whitespace)
            .next()
            .unwrap()
            .to_string();
        let after = &rest[eq + 2..];
        let end = after.find('"').unwrap();
        m.insert(key, after[..end].to_string());
        rest = &after[end + 1..];
    }
    m
}

/// SVG → (page height in mm, groups by id). y is flipped back to y-up.
fn read_svg(svg: &str) -> (f64, f64, BTreeMap<String, GroupData>) {
    let root = &svg[svg.find("<svg").unwrap()..];
    let ra = attrs(&root[..root.find('>').unwrap()]);
    let w: f64 = ra["width"].trim_end_matches("mm").parse().unwrap();
    let h: f64 = ra["height"].trim_end_matches("mm").parse().unwrap();
    let mut groups: BTreeMap<String, GroupData> = BTreeMap::new();
    let mut cur: Option<String> = None;
    let f = |m: &BTreeMap<String, String>, k: &str| -> f64 { m[k].parse().unwrap() };
    for piece in svg.split('<').skip(1) {
        let (tag, after) = piece.split_once('>').unwrap();
        let name = tag.split_whitespace().next().unwrap_or("");
        let a = attrs(tag);
        match name {
            "g" => cur = Some(a["id"].clone()),
            "/g" => cur = None,
            _ => {}
        }
        let Some(g) = cur.as_ref().map(|c| groups.entry(c.clone()).or_default()) else {
            continue;
        };
        if a.get("class").map(String::as_str) == Some("flag") {
            g.flagged = true;
        }
        match name {
            "line" => {
                let s = Seg {
                    a: [f(&a, "x1"), h - f(&a, "y1")],
                    b: [f(&a, "x2"), h - f(&a, "y2")],
                };
                match a["class"].as_str() {
                    "visible" => g.visible.push(s),
                    "hidden" => {
                        assert!(a.contains_key("stroke-dasharray"), "hidden line not dashed");
                        g.hidden.push(s)
                    }
                    "center" => g.center.push(s),
                    "thin" => g.thin.push(s),
                    _ => {}
                }
            }
            "circle" => {
                g.circles
                    .push((f(&a, "cx"), h - f(&a, "cy"), f(&a, "r"), a["class"].clone()))
            }
            "text" => g.texts.push((
                f(&a, "x"),
                h - f(&a, "y"),
                after
                    .replace("&amp;", "&")
                    .replace("&lt;", "<")
                    .replace("&gt;", ">"),
            )),
            _ => {}
        }
    }
    (w, h, groups)
}

// ------------------------------------------------------------ PDF reader

/// Minimal PDF content reader: checks the header, the xref offsets and the
/// page size, then interprets the (uncompressed) content stream: the `cm`
/// scale, marked-content groups, dash state, `m`/`l`/`S` segments and text.
fn read_pdf(pdf: &[u8]) -> (f64, f64, BTreeMap<String, GroupData>) {
    assert!(pdf.starts_with(b"%PDF-1."));
    let text: String = pdf.iter().map(|&b| b as char).collect(); // Latin-1: char i = byte i
    let chars: Vec<char> = text.chars().collect();
    let tail = &text[text.rfind("startxref").unwrap()..];
    let xref: usize = tail.lines().nth(1).unwrap().parse().unwrap();
    let xs: String = chars[xref..].iter().collect();
    assert!(xs.starts_with("xref"));
    for (i, line) in xs
        .lines()
        .skip(3)
        .take_while(|l| l.ends_with(" n "))
        .enumerate()
    {
        let off: usize = line[..10].parse().unwrap();
        let at: String = chars[off..off + 12].iter().collect();
        assert!(
            at.starts_with(&format!("{} 0 obj", i + 1)),
            "xref entry {}",
            i + 1
        );
    }
    let mb = &text[text.find("/MediaBox [").unwrap() + 11..];
    let nums: Vec<f64> = mb[..mb.find(']').unwrap()]
        .split_whitespace()
        .map(|x| x.parse().unwrap())
        .collect();
    let mm = 25.4 / 72.0;
    let (w, h) = (nums[2] * mm, nums[3] * mm);
    let s0 = text.find("stream\n").unwrap() + 7;
    let s1 = text.find("endstream").unwrap();
    let stream = &text[s0..s1];
    let mut groups: BTreeMap<String, GroupData> = BTreeMap::new();
    let mut cur = String::new();
    let mut scale = 1.0;
    let mut dashed = false;
    let mut chain = false;
    let mut red = false;
    let mut width = 0.0;
    let mut stack: Vec<String> = Vec::new();
    let mut path: Vec<[f64; 2]> = Vec::new();
    let mut segs: Vec<Seg> = Vec::new();
    let mut curve = false;
    for line in stream.lines() {
        // Text: "BT /F1 s Tf x y Td (..) Tj ET"
        if line.starts_with("BT ") {
            let toks: Vec<&str> = line.split_whitespace().collect();
            let td: [f64; 2] = [toks[4].parse().unwrap(), toks[5].parse().unwrap()];
            let open = line.find('(').unwrap();
            let close = line.rfind(") Tj").unwrap();
            let raw = &line[open + 1..close];
            let mut s = String::new();
            let mut it = raw.chars().peekable();
            while let Some(c) = it.next() {
                if c == '\\' {
                    let n = it.next().unwrap();
                    if n.is_ascii_digit() {
                        let o: String =
                            [n, it.next().unwrap(), it.next().unwrap()].iter().collect();
                        let b = u8::from_str_radix(&o, 8).unwrap();
                        s.push(match b {
                            0xD8 => 'Ø',
                            0xD7 => '×',
                            other => other as char,
                        });
                    } else {
                        s.push(n);
                    }
                } else {
                    s.push(c);
                }
            }
            let g = groups.entry(cur.clone()).or_default();
            g.texts.push((td[0], td[1], s));
            if red {
                g.flagged = true;
            }
            continue;
        }
        if line.starts_with("/RCADGroup") {
            let n = &line[line.find("(").unwrap() + 1..line.find(")").unwrap()];
            stack.push(n.to_string());
            cur = n.to_string();
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        let mut i = 0;
        let mut nums: Vec<f64> = Vec::new();
        while i < toks.len() {
            let t = toks[i];
            if let Ok(x) = t.parse::<f64>() {
                nums.push(x);
            } else {
                match t {
                    "cm" => scale = nums[0],
                    "RG" => red = nums[0] > 0.5,
                    "w" => width = nums[0],
                    "d" => {
                        // "[a b] 0 d": dash array tokens were collected as text.
                        let inside = &line[line.find('[').unwrap() + 1..line.find(']').unwrap()];
                        let n = inside.split_whitespace().count();
                        dashed = n == 2;
                        chain = n == 4;
                    }
                    "m" => {
                        path = vec![[nums[0], nums[1]]];
                        curve = false;
                    }
                    "l" => path.push([nums[0], nums[1]]),
                    "c" => {
                        path.push([nums[4], nums[5]]);
                        curve = true;
                    }
                    "S" | "f" => {
                        let g = groups.entry(cur.clone()).or_default();
                        if red {
                            g.flagged = true;
                        }
                        if curve && t == "S" {
                            // A circle: closed Bézier path; centre = mean of the
                            // 4 quadrant points, radius = distance to one.
                            let p = &path[..path.len().min(4)];
                            let c = [
                                p.iter().map(|q| q[0]).sum::<f64>() / p.len() as f64,
                                p.iter().map(|q| q[1]).sum::<f64>() / p.len() as f64,
                            ];
                            let r = (p[0][0] - c[0]).hypot(p[0][1] - c[1]);
                            let class = if dashed { "hidden" } else { "visible" };
                            g.circles.push((c[0], c[1], r, class.into()));
                        } else if t == "S" && path.len() == 2 {
                            let s = Seg {
                                a: path[0],
                                b: path[1],
                            };
                            if dashed {
                                g.hidden.push(s);
                            } else if chain {
                                g.center.push(s);
                            } else if width < 0.45 {
                                g.thin.push(s);
                            } else {
                                g.visible.push(s);
                            }
                        }
                        segs.clear();
                    }
                    "EMC" => {
                        stack.pop();
                        cur = stack.last().cloned().unwrap_or_default();
                    }
                    _ => {}
                }
                nums.clear();
            }
            i += 1;
        }
    }
    assert!((scale - 72.0 / 25.4).abs() < 1e-3, "content is not in mm");
    (w, h, groups)
}

// ------------------------------------------------------------ helpers

fn export(s: &mut Session, d: DrawingId, name: &str) -> (Response, Vec<u8>) {
    let path = out_dir().join(name);
    let r = ok(
        s,
        Command::ExportDrawing {
            drawing: d,
            path: path.clone(),
            format: None,
        },
    );
    (r, std::fs::read(path).unwrap())
}

fn len(s: &Seg) -> f64 {
    (s.b[0] - s.a[0]).hypot(s.b[1] - s.a[1])
}

fn bbox(segs: &[Seg]) -> ([f64; 2], [f64; 2]) {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for s in segs {
        for p in [s.a, s.b] {
            for i in 0..2 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
    }
    (lo, hi)
}

fn center(segs: &[Seg]) -> [f64; 2] {
    let (lo, hi) = bbox(segs);
    [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]
}

fn texts(g: &BTreeMap<String, GroupData>, id: &str) -> Vec<String> {
    let mut t: Vec<String> = g
        .get(id)
        .map(|x| x.texts.iter().map(|t| t.2.clone()).collect())
        .unwrap_or_default();
    t.sort();
    t
}

/// Rectangle `w` x `h` (sheet mm) of exactly four visible lines.
fn assert_rect(g: &GroupData, w: f64, h: f64, what: &str) {
    assert_eq!(g.visible.len(), 4, "{what}: visible lines {:?}", g.visible);
    let (lo, hi) = bbox(&g.visible);
    assert!(
        (hi[0] - lo[0] - w).abs() < 2e-3,
        "{what}: width {}",
        hi[0] - lo[0]
    );
    assert!(
        (hi[1] - lo[1] - h).abs() < 2e-3,
        "{what}: height {}",
        hi[1] - lo[1]
    );
}

/// The common checks of the three standard views of a Ø`d` hole at
/// (`cx`, `cy`) in the block, for either reader's output.
fn check_views(
    g: &BTreeMap<String, GroupData>,
    scale: f64,
    d: f64,
    cx: f64,
    cy: f64,
    third: bool,
    fmt: &str,
) {
    let (front, top, right) = (&g["view-1"], &g["view-2"], &g["view-3"]);
    assert_rect(front, 40.0 * scale, 10.0 * scale, &format!("{fmt} front"));
    assert_rect(top, 40.0 * scale, 30.0 * scale, &format!("{fmt} top"));
    assert_rect(right, 30.0 * scale, 10.0 * scale, &format!("{fmt} right"));
    // Front: the hole's two silhouettes are hidden (dashed), full height, Ø apart,
    // at x = cx ± d/2 relative to the block's left edge.
    let (flo, _) = bbox(&front.visible);
    assert_eq!(
        front.hidden.len(),
        2,
        "{fmt} front hidden {:?}",
        front.hidden
    );
    let mut xs: Vec<f64> = front
        .hidden
        .iter()
        .map(|s| (s.a[0] - flo[0]) / scale)
        .collect();
    xs.sort_by(f64::total_cmp);
    assert!(
        (xs[0] - (cx - d / 2.0)).abs() < 1e-3 && (xs[1] - (cx + d / 2.0)).abs() < 1e-3,
        "{fmt} {xs:?}"
    );
    for s in &front.hidden {
        assert!((s.a[0] - s.b[0]).abs() < 1e-3 && (len(s) - 10.0 * scale).abs() < 2e-3);
    }
    // Right view: hidden at y offsets cy ± d/2 (the right view's x is world +Y).
    let (rlo, _) = bbox(&right.visible);
    let mut ys: Vec<f64> = right
        .hidden
        .iter()
        .map(|s| (s.a[0] - rlo[0]) / scale)
        .collect();
    ys.sort_by(f64::total_cmp);
    assert_eq!(ys.len(), 2, "{fmt} right hidden");
    assert!((ys[0] - (cy - d / 2.0)).abs() < 1e-3 && (ys[1] - (cy + d / 2.0)).abs() < 1e-3);
    // Top: one visible circle of the hole, at the hole centre, nothing hidden.
    assert!(top.hidden.is_empty(), "{fmt} top hidden {:?}", top.hidden);
    let vis: Vec<_> = top.circles.iter().filter(|c| c.3 == "visible").collect();
    assert_eq!(vis.len(), 1, "{fmt} top circles {:?}", top.circles);
    let (tlo, _) = bbox(&top.visible);
    let c = vis[0];
    assert!((c.2 - d / 2.0 * scale).abs() < 2e-3, "{fmt} r {}", c.2);
    assert!(
        ((c.0 - tlo[0]) / scale - cx).abs() < 1e-3 && ((c.1 - tlo[1]) / scale - cy).abs() < 1e-3
    );
    assert_eq!(top.center.len(), 2, "{fmt} centre mark");
    // Placement by the projection convention: aligned with the front view.
    let (cf, ct, cr) = (
        center(&front.visible),
        center(&top.visible),
        center(&right.visible),
    );
    assert!((cf[0] - ct[0]).abs() < 1e-3 && (cf[1] - cr[1]).abs() < 1e-3);
    if third {
        assert!(
            ct[1] > cf[1] && cr[0] > cf[0],
            "{fmt}: third angle places top above, right to the right"
        );
    } else {
        assert!(
            ct[1] < cf[1] && cr[0] < cf[0],
            "{fmt}: first angle places top below, right to the left"
        );
    }
}

fn dims(g: &BTreeMap<String, GroupData>) -> Vec<String> {
    let mut all: Vec<String> = g
        .iter()
        .filter(|(k, _)| k.starts_with("dims-"))
        .flat_map(|(_, v)| v.texts.iter().map(|t| t.2.clone()))
        .collect();
    all.sort();
    all
}

fn drawing_of(s: &mut Session) -> DrawingId {
    ok(
        s,
        Command::CreateDrawing {
            name: Some("Block".into()),
            sheet: None,
            projection: None,
            title: Some("Holed block".into()),
            part_number: Some("RC-001".into()),
            material: None,
            bodies: vec![BodyId(1)],
            standard_views: None,
            assembly: false,
        },
    )
    .drawing
    .unwrap()
}

// ------------------------------------------------------------ tests

#[test]
fn block_with_hole_views_dimensions_and_both_formats() {
    let mut s = Session::new();
    block(&mut s, 8.0, 20.0, 15.0);
    let d = drawing_of(&mut s);
    let (r, svg) = export(&mut s, d, "block.svg");
    let rep = r.drawing_report.unwrap();
    assert!(rep.issues.is_empty(), "{:?}", rep.issues);
    let scale = rep.views[0].scale;
    assert_eq!(scale, 1.0);
    let (w, h, g) = read_svg(std::str::from_utf8(&svg).unwrap());
    assert_eq!((w, h), (297.0, 210.0));
    check_views(&g, scale, 8.0, 20.0, 15.0, true, "svg");
    assert_eq!(dims(&g), vec!["10", "30", "40", "Ø8"]);
    assert!(texts(&g, "title").contains(&"THIRD ANGLE PROJECTION".to_string()));
    assert!(texts(&g, "title").contains(&"RC-001".to_string()));
    assert!(texts(&g, "title").contains(&"1:1".to_string()));

    let (_, pdf) = export(&mut s, d, "block.pdf");
    let (pw, ph, pg) = read_pdf(&pdf);
    assert!((pw - 297.0).abs() < 1e-3 && (ph - 210.0).abs() < 1e-3);
    check_views(&pg, scale, 8.0, 20.0, 15.0, true, "pdf");
    assert_eq!(dims(&pg), vec!["10", "30", "40", "Ø8"]);

    // Deterministic bytes: again, and after save + reopen (fresh regeneration).
    assert_eq!(export(&mut s, d, "block.svg").1, svg);
    assert_eq!(export(&mut s, d, "block.pdf").1, pdf);
    let file = out_dir().join("block.rcad.json");
    ok(&mut s, Command::Save { path: file.clone() });
    let raw = std::fs::read_to_string(&file).unwrap();
    assert!(raw.contains("\"version\": 6") && raw.contains("\"drawings\""));
    let mut s2 = Session::new();
    ok(&mut s2, Command::Open { path: file });
    assert_eq!(export(&mut s2, d, "block_reopened.svg").1, svg);
    assert_eq!(export(&mut s2, d, "block_reopened.pdf").1, pdf);
}

#[test]
fn drawing_follows_hole_edits_and_flags_deleted_references() {
    let mut s = Session::new();
    let (e, hole) = block(&mut s, 8.0, 20.0, 15.0);
    let d = drawing_of(&mut s);
    // A semantic callout on the top view (replaces the automatic Ø there).
    ok(
        &mut s,
        Command::AddHoleCallout {
            drawing: d,
            view: ViewId(2),
            feature: hole,
        },
    );
    let (r, _) = export(&mut s, d, "edit_0.svg");
    let v2 = &r.drawing_report.as_ref().unwrap().views[1];
    assert_eq!(
        v2.dimensions
            .iter()
            .filter(|x| x.kind == "hole_callout")
            .count(),
        1
    );
    assert!(!v2.dimensions.iter().any(|x| x.kind == "diameter"));

    // Diameter 8 -> 12 and move the hole to (12, 10): the next export follows.
    for (n, v) in [("diameter", 12.0), ("center_x", 12.0), ("center_y", 10.0)] {
        ok(
            &mut s,
            Command::SetParameter {
                feature: hole,
                name: n.into(),
                value: v,
            },
        );
    }
    let (r, svg) = export(&mut s, d, "edit_1.svg");
    assert!(r.drawing_report.unwrap().issues.is_empty());
    let (_, _, g) = read_svg(std::str::from_utf8(&svg).unwrap());
    check_views(&g, 1.0, 12.0, 12.0, 10.0, true, "svg after edit");
    assert_eq!(dims(&g), vec!["10", "30", "40", "Ø12"]);
    let (_, pdf) = export(&mut s, d, "edit_1.pdf");
    let (_, _, pg) = read_pdf(&pdf);
    check_views(&pg, 1.0, 12.0, 12.0, 10.0, true, "pdf after edit");

    // First angle: the views swap sides; the sheet records it.
    ok(
        &mut s,
        Command::EditDrawing {
            drawing: d,
            name: None,
            sheet: None,
            projection: Some(Projection::FirstAngle),
            title: None,
            part_number: None,
            material: None,
            auto_dimensions: None,
        },
    );
    let (_, svg) = export(&mut s, d, "edit_first_angle.svg");
    let (_, _, g) = read_svg(std::str::from_utf8(&svg).unwrap());
    check_views(&g, 1.0, 12.0, 12.0, 10.0, false, "first angle");
    assert!(texts(&g, "title").contains(&"FIRST ANGLE PROJECTION".to_string()));

    // Delete the hole: its Ø disappears, the callout is reported dangling
    // (drawn in red without a value), no hidden lines remain.
    ok(&mut s, Command::DeleteFeature { feature: hole });
    let (r, svg) = export(&mut s, d, "edit_2_hole_deleted.svg");
    let rep = r.drawing_report.unwrap();
    assert_eq!(rep.issues.len(), 1, "{:?}", rep.issues);
    assert!(rep.issues[0].contains("dangling") && rep.issues[0].contains(&hole.to_string()));
    let call = rep.views[1]
        .dimensions
        .iter()
        .find(|x| x.kind == "hole_callout")
        .unwrap();
    assert!(call.value.is_none() && call.dangling.is_some());
    let (_, _, g) = read_svg(std::str::from_utf8(&svg).unwrap());
    assert!(g["view-1"].hidden.is_empty() && g["view-3"].hidden.is_empty());
    assert!(g["view-2"].circles.is_empty());
    assert!(
        !dims(&g)
            .iter()
            .any(|t| t.starts_with("Ø") && !t.starts_with("Ø?")),
        "{:?}",
        dims(&g)
    );
    assert!(g["dims-2"].flagged);
    assert_eq!(dims(&g).iter().filter(|t| t.starts_with("Ø?")).count(), 1);

    // Delete the body: every view is flagged, nothing is drawn from old data.
    ok(&mut s, Command::DeleteFeature { feature: e });
    let (r, svg) = export(&mut s, d, "edit_3_body_deleted.svg");
    let rep = r.drawing_report.unwrap();
    for v in &rep.views {
        assert!(
            matches!(&v.status, rcad_document::drawing::ViewStatus::MissingBodies { bodies } if bodies == &vec![BodyId(1)]),
            "{v:?}"
        );
        assert_eq!(v.visible_curves + v.hidden_curves, 0);
    }
    let (_, _, g) = read_svg(std::str::from_utf8(&svg).unwrap());
    for v in ["view-1", "view-2", "view-3"] {
        assert!(
            g[v].flagged && g[v].visible.is_empty() && g[v].hidden.is_empty(),
            "{v}"
        );
    }
    // Undo brings the body back and the drawing draws it again.
    ok(&mut s, Command::Undo);
    let (r, _) = export(&mut s, d, "edit_4_undo.svg");
    assert!(
        r.drawing_report
            .unwrap()
            .views
            .iter()
            .all(|v| v.visible_curves == 4)
    );
}

#[test]
fn drawing_commands_validate_and_undo() {
    let mut s = Session::new();
    block(&mut s, 8.0, 20.0, 15.0);
    let d = drawing_of(&mut s);
    // Bad inputs are refused unchanged.
    let rev = s.summary().revision;
    for c in [
        Command::AddDrawingView {
            drawing: d,
            view: ViewSpec::Base {
                bodies: vec![BodyId(9)],
                orientation: Orientation::Iso,
                scale: None,
                position: None,
                assembly: false,
            },
        },
        Command::AddDrawingView {
            drawing: d,
            view: ViewSpec::Projected {
                parent: ViewId(1),
                from: Side::Above,
            },
        },
        Command::EditDrawingView {
            drawing: d,
            view: ViewId(1),
            scale: Some(f64::NAN),
            position: None,
            orientation: None,
        },
        Command::EditDrawingView {
            drawing: d,
            view: ViewId(2),
            scale: Some(2.0),
            position: None,
            orientation: None,
        },
        Command::RemoveDrawingView {
            drawing: d,
            view: ViewId(1),
        },
        Command::ExportDrawing {
            drawing: d,
            path: out_dir().join("x.txt"),
            format: None,
        },
        Command::DeleteDrawing {
            drawing: DrawingId(7),
        },
    ] {
        let r = s.execute(&c);
        assert!(!r.ok, "{c:?} should fail");
    }
    assert_eq!(s.summary().revision, rev);
    // An iso view at 1:2 exports; no dimensions are placed on it.
    let v = ok(
        &mut s,
        Command::AddDrawingView {
            drawing: d,
            view: ViewSpec::Base {
                bodies: vec![BodyId(1)],
                orientation: Orientation::Iso,
                scale: Some(0.5),
                position: Some([230.0, 150.0]),
                assembly: false,
            },
        },
    )
    .view
    .unwrap();
    let r = ok(
        &mut s,
        Command::ExportDrawing {
            drawing: d,
            path: out_dir().join("iso.pdf"),
            format: Some(DrawingFormat::Pdf),
        },
    );
    let rep = r.drawing_report.unwrap();
    let iso = rep.views.iter().find(|x| x.id == v).unwrap();
    assert_eq!(iso.direction, "Iso");
    assert!(iso.visible_curves >= 9, "{iso:?}");
    assert!(iso.dimensions.iter().all(|x| x.kind == "diameter") || iso.dimensions.is_empty());
    // Delete + undo + redo.
    ok(&mut s, Command::DeleteDrawing { drawing: d });
    assert!(s.summary().drawings.is_empty());
    ok(&mut s, Command::Undo);
    assert_eq!(s.summary().drawings.len(), 1);
    assert_eq!(s.summary().drawings[0].views.len(), 4);
    ok(&mut s, Command::Redo);
    assert!(s.summary().drawings.is_empty());
}

#[test]
fn cylinder_silhouettes_are_exact() {
    // Ø20 x 30 cylinder on the XY plane about the Z axis: its side views are a
    // 20 x 30 rectangle whose vertical sides are the silhouettes of the
    // curved face (exact generators, not mesh edges); the top view a circle.
    let mut s = Session::new();
    ok(&mut s, Command::NewDocument);
    let sk = ok(
        &mut s,
        Command::CreateSketch {
            name: None,
            plane: SketchPlane::WorldXY,
            template: SketchTemplate::CenteredCircle {
                diameter: 20.0,
                center: None,
            },
        },
    )
    .feature
    .unwrap();
    ok(&mut s, Command::extrude(None, sk, 30.0));
    let d = drawing_of(&mut s);
    let (r, svg) = export(&mut s, d, "cylinder.svg");
    let scale = r.drawing_report.unwrap().views[0].scale;
    let (_, _, g) = read_svg(std::str::from_utf8(&svg).unwrap());
    for v in ["view-1", "view-3"] {
        assert_rect(&g[v], 20.0 * scale, 30.0 * scale, v);
        assert!(g[v].hidden.is_empty(), "{v}: {:?}", g[v].hidden);
    }
    let top = &g["view-2"];
    assert!(top.visible.is_empty() && top.hidden.is_empty());
    assert_eq!(top.circles.len(), 1);
    assert!((top.circles[0].2 - 10.0 * scale).abs() < 1e-3);
    assert_eq!(dims(&g), vec!["20", "20", "30", "Ø20"]);
}

/// DXF group-code pairs → (entity type, layer, codes) list.
fn read_dxf(text: &str) -> Vec<(String, String, BTreeMap<i32, Vec<String>>)> {
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.len().is_multiple_of(2), "odd number of DXF lines");
    let pairs: Vec<(i32, &str)> = lines
        .chunks(2)
        .map(|c| (c[0].trim().parse().unwrap(), c[1]))
        .collect();
    let start = pairs
        .iter()
        .position(|p| *p == (2, "ENTITIES"))
        .expect("ENTITIES section");
    let mut out: Vec<(String, String, BTreeMap<i32, Vec<String>>)> = Vec::new();
    for &(code, v) in &pairs[start + 1..] {
        if code == 0 {
            if v == "ENDSEC" {
                break;
            }
            out.push((v.to_string(), String::new(), BTreeMap::new()));
        } else if let Some(e) = out.last_mut() {
            if code == 8 {
                e.1 = v.to_string();
            }
            e.2.entry(code).or_default().push(v.to_string());
        }
    }
    out
}

#[test]
fn dxf_export_has_layers_units_and_the_same_geometry() {
    let mut s = Session::new();
    block(&mut s, 8.0, 20.0, 15.0);
    let d = drawing_of(&mut s);
    let path = out_dir().join("block.dxf");
    let r = ok(
        &mut s,
        Command::ExportDrawing {
            drawing: d,
            path: path.clone(),
            format: None,
        },
    );
    assert_eq!(r.drawing_report.unwrap().format, DrawingFormat::Dxf);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("$INSUNITS\n 70\n4\n"), "declared mm");
    let ents = read_dxf(&text);
    let count = |t: &str, l: &str| ents.iter().filter(|e| e.0 == t && e.1 == l).count();
    // 3 rectangles + nothing else visible (the circle is a CIRCLE).
    assert_eq!(count("LINE", "VISIBLE"), 12);
    assert_eq!(count("LINE", "HIDDEN"), 4);
    assert_eq!(count("CIRCLE", "VISIBLE"), 1);
    let c = ents.iter().find(|e| e.0 == "CIRCLE").unwrap();
    assert_eq!(c.2[&40][0].parse::<f64>().unwrap(), 4.0);
    let hidden_len: f64 = ents
        .iter()
        .filter(|e| e.0 == "LINE" && e.1 == "HIDDEN")
        .map(|e| {
            let g = |k: i32| e.2[&k][0].parse::<f64>().unwrap();
            (g(11) - g(10)).hypot(g(21) - g(20))
        })
        .sum();
    assert!((hidden_len - 40.0).abs() < 1e-5, "{hidden_len}");
    let texts: Vec<&str> = ents
        .iter()
        .filter(|e| e.0 == "TEXT")
        .map(|e| e.2[&1][0].as_str())
        .collect();
    for t in ["40", "30", "10", "%%c8", "THIRD ANGLE PROJECTION"] {
        assert!(texts.contains(&t), "{t} in {texts:?}");
    }
    // Deterministic.
    ok(
        &mut s,
        Command::ExportDrawing {
            drawing: d,
            path: out_dir().join("block2.dxf"),
            format: Some(DrawingFormat::Dxf),
        },
    );
    assert_eq!(
        std::fs::read_to_string(out_dir().join("block2.dxf")).unwrap(),
        text
    );
}

#[test]
fn title_block_fields_come_from_the_model_and_scale_applies() {
    let mut s = Session::new();
    block(&mut s, 8.0, 20.0, 15.0);
    ok(
        &mut s,
        Command::SetBodyMaterial {
            body: BodyId(1),
            material: Some("pla".into()),
            density: None,
            density_unit: Default::default(),
            name: None,
        },
    );
    ok(
        &mut s,
        Command::SetConfiguration {
            name: "Std".into(),
            values: Default::default(),
            suppressed: vec![],
            part_number: "PN-STD-40".into(),
            excluded_occurrences: vec![],
        },
    );
    ok(
        &mut s,
        Command::ActivateConfiguration { name: "Std".into() },
    );
    let d = ok(
        &mut s,
        Command::CreateDrawing {
            name: None,
            sheet: Some(rcad_document::drawing::SheetSize::A3Landscape),
            projection: None,
            title: None,
            part_number: None,
            material: None,
            bodies: vec![BodyId(1)],
            standard_views: None,
            assembly: false,
        },
    )
    .drawing
    .unwrap();
    ok(
        &mut s,
        Command::EditDrawingView {
            drawing: d,
            view: ViewId(1),
            scale: Some(2.0),
            position: None,
            orientation: None,
        },
    );
    let (r, svg) = export(&mut s, d, "a3_scale2.svg");
    let rep = r.drawing_report.unwrap();
    assert!(rep.issues.is_empty(), "{:?}", rep.issues);
    let (w, h, g) = read_svg(std::str::from_utf8(&svg).unwrap());
    assert_eq!((w, h), (420.0, 297.0));
    let t = texts(&g, "title");
    for want in ["PN-STD-40", "PLA", "2:1", "Drawing1", "A3 mm"] {
        assert!(t.contains(&want.to_string()), "{want} in {t:?}");
    }
    // Scale 2: every view doubles; dimension values stay model values.
    check_views(&g, 2.0, 8.0, 20.0, 15.0, true, "scale 2");
    assert_eq!(dims(&g), vec!["10", "30", "40", "Ø8"]);
}

/// Section A-A through the hole axis (plane x = cx) of the front view, seen
/// from the right: the cut face is the 30 x 10 rectangle minus the hole's
/// slot y in [cy - d/2, cy + d/2]; checks the outline, the hole walls, that
/// every hatch line lies inside the cut face (none across the slot), the
/// cutting line on the front view, and that the section follows edits.
fn check_section(g: &BTreeMap<String, GroupData>, d: f64, cy: f64, hole: bool, what: &str) {
    let sec = &g["view-4"];
    let n = if hole { 6 } else { 4 };
    assert_eq!(sec.visible.len(), n, "{what}: {:?}", sec.visible);
    assert!(sec.hidden.is_empty(), "{what}: {:?}", sec.hidden);
    let (lo, hi) = bbox(&sec.visible);
    assert!((hi[0] - lo[0] - 30.0).abs() < 2e-3 && (hi[1] - lo[1] - 10.0).abs() < 2e-3);
    let mut xs: Vec<f64> = sec
        .visible
        .iter()
        .filter(|s| (s.a[0] - s.b[0]).abs() < 1e-6)
        .map(|s| s.a[0] - lo[0])
        .collect();
    xs.sort_by(f64::total_cmp);
    let want: Vec<f64> = if hole {
        vec![0.0, cy - d / 2.0, cy + d / 2.0, 30.0]
    } else {
        vec![0.0, 30.0]
    };
    assert_eq!(xs.len(), want.len(), "{what}: {xs:?}");
    for (a, b) in xs.iter().zip(&want) {
        assert!((a - b).abs() < 1e-3, "{what}: {xs:?} vs {want:?}");
    }
    // Hatching: 45° thin lines, each inside one of the solid parts.
    let hatch: Vec<&Seg> = sec.thin.iter().collect();
    assert!(hatch.len() >= 6, "{what}: {} hatch lines", hatch.len());
    let gap = (cy - d / 2.0, cy + d / 2.0);
    let (mut left, mut right) = (0, 0);
    for s in &hatch {
        let (dx, dy) = (s.b[0] - s.a[0], s.b[1] - s.a[1]);
        assert!((dx.abs() - dy.abs()).abs() < 2e-3, "{what}: not 45°: {s:?}");
        let ys = [s.a[0] - lo[0], s.b[0] - lo[0]];
        for y in ys {
            assert!(
                (-1e-3..=30.0 + 1e-3).contains(&y),
                "{what}: hatch outside {s:?}"
            );
        }
        for z in [s.a[1] - lo[1], s.b[1] - lo[1]] {
            assert!(
                (-1e-3..=10.0 + 1e-3).contains(&z),
                "{what}: hatch outside {s:?}"
            );
        }
        if hole {
            let (a, b) = (ys[0].min(ys[1]), ys[0].max(ys[1]));
            assert!(
                b <= gap.0 + 1e-3 || a >= gap.1 - 1e-3,
                "{what}: hatch across the hole {s:?}"
            );
            if b <= gap.0 + 1e-3 {
                left += 1;
            } else {
                right += 1;
            }
        }
    }
    if hole {
        assert!(left > 0 && right > 0);
    }
    assert!(sec.texts.iter().any(|t| t.2 == "SECTION A-A"), "{what}");
    // Cutting line on the front view at x = 20, past its outline, labelled A twice.
    let (flo, fhi) = bbox(&g["view-1"].visible);
    let cut = &g["cut-A"];
    let line = cut.center.first().expect("cutting line");
    assert!((line.a[0] - (flo[0] + 20.0)).abs() < 1e-3 && (line.b[0] - line.a[0]).abs() < 1e-9);
    let (y0, y1) = (line.a[1].min(line.b[1]), line.a[1].max(line.b[1]));
    assert!(y0 < flo[1] && y1 > fhi[1]);
    assert_eq!(cut.texts.iter().filter(|t| t.2 == "A").count(), 2);
    // Placed to the right of the front view (third angle), past the right view.
    let (slo, _) = bbox(&sec.visible);
    let (_, rhi) = bbox(&g["view-3"].visible);
    assert!(
        slo[0] > rhi[0],
        "{what}: section placed past the right view"
    );
    assert!((center(&sec.visible)[1] - center(&g["view-1"].visible)[1]).abs() < 1e-3);
}

#[test]
fn section_view_through_the_hole_follows_the_model() {
    let mut s = Session::new();
    let (_, hole) = block(&mut s, 8.0, 20.0, 15.0);
    let d = drawing_of(&mut s);
    let sec = |label: &str, offset: f64| Command::AddDrawingView {
        drawing: d,
        view: ViewSpec::Section {
            parent: ViewId(1),
            from: Side::Right,
            offset,
            label: label.into(),
        },
    };
    let v = ok(&mut s, sec("A", 20.0)).view.unwrap();
    assert_eq!(v, ViewId(4));
    // Same label twice, a bad label, and a section of a section are refused.
    assert!(!s.execute(&sec("A", 10.0)).ok);
    assert!(!s.execute(&sec("A-1", 10.0)).ok);
    assert!(
        !s.execute(&Command::AddDrawingView {
            drawing: d,
            view: ViewSpec::Section {
                parent: v,
                from: Side::Right,
                offset: 1.0,
                label: "Z".into()
            },
        })
        .ok
    );
    let (r, svg) = export(&mut s, d, "section.svg");
    let rep = r.drawing_report.unwrap();
    assert!(rep.issues.is_empty(), "{:?}", rep.issues);
    let (_, _, g) = read_svg(std::str::from_utf8(&svg).unwrap());
    check_section(&g, 8.0, 15.0, true, "section");
    // The section has no automatic dimensions of its own.
    assert!(g["dims-4"].texts.is_empty());
    let (_, pdf) = export(&mut s, d, "section.pdf");
    let (_, _, pg) = read_pdf(&pdf);
    assert_eq!(pg["view-4"].visible.len(), 6);

    // Ø12 at y = 13: the walls move.
    for (n, x) in [("diameter", 12.0), ("center_y", 13.0)] {
        ok(
            &mut s,
            Command::SetParameter {
                feature: hole,
                name: n.into(),
                value: x,
            },
        );
    }
    let (_, svg) = export(&mut s, d, "section_edit.svg");
    let (_, _, g) = read_svg(std::str::from_utf8(&svg).unwrap());
    check_section(&g, 12.0, 13.0, true, "section after edit");
    // Hole deleted: one solid cut face.
    ok(&mut s, Command::DeleteFeature { feature: hole });
    let (_, svg) = export(&mut s, d, "section_no_hole.svg");
    let (_, _, g) = read_svg(std::str::from_utf8(&svg).unwrap());
    check_section(&g, 0.0, 15.0, false, "section without hole");
    // A cutting plane that misses the part is flagged, not drawn.
    ok(&mut s, sec("B", 100.0));
    let (r, svg) = export(&mut s, d, "section_miss.svg");
    let rep = r.drawing_report.unwrap();
    assert!(
        rep.issues.iter().any(|i| i.contains("section failed")),
        "{:?}",
        rep.issues
    );
    let (_, _, g) = read_svg(std::str::from_utf8(&svg).unwrap());
    assert!(g["view-5"].flagged && g["view-5"].visible.is_empty());
}

#[test]
fn detail_views_clip_and_enlarge() {
    let mut s = Session::new();
    let (_, hole) = block(&mut s, 8.0, 20.0, 15.0);
    let d = drawing_of(&mut s);
    let detail = |label: &str, c: [f64; 2], r: f64, pos: [f64; 2]| Command::AddDrawingView {
        drawing: d,
        view: ViewSpec::Detail {
            parent: ViewId(2),
            center: c,
            radius: r,
            scale: None,
            position: Some(pos),
            label: label.into(),
        },
    };
    // B: the whole hole (centre 4 mm from the detail centre, r 4 < 10 - 4).
    ok(&mut s, detail("B", [24.0, 15.0], 10.0, [200.0, 150.0]));
    // C: a chord of the block's right edge x = 40 (4 mm from the centre).
    ok(&mut s, detail("C", [36.0, 15.0], 8.0, [255.0, 150.0]));
    // D: the top part of the hole circle only (an arc).
    ok(&mut s, detail("D", [20.0, 21.0], 4.0, [200.0, 95.0]));
    // Refused: zero radius, duplicate label, detail of a detail.
    assert!(!s.execute(&detail("E", [0.0, 0.0], 0.0, [100.0, 100.0])).ok);
    assert!(!s.execute(&detail("B", [0.0, 0.0], 5.0, [100.0, 100.0])).ok);
    assert!(
        !s.execute(&Command::AddDrawingView {
            drawing: d,
            view: ViewSpec::Detail {
                parent: ViewId(4),
                center: [0.0, 0.0],
                radius: 3.0,
                scale: None,
                position: None,
                label: "F".into(),
            },
        })
        .ok
    );
    let (r, svg) = export(&mut s, d, "details.svg");
    let rep = r.drawing_report.unwrap();
    assert!(rep.issues.is_empty(), "{:?}", rep.issues);
    let (_, _, g) = read_svg(std::str::from_utf8(&svg).unwrap());
    let b = &g["view-4"];
    assert!(b.visible.is_empty() && b.hidden.is_empty(), "{b:?}");
    assert_eq!(b.circles.len(), 1);
    let (cx, cy, r) = (b.circles[0].0, b.circles[0].1, b.circles[0].2);
    assert!((r - 8.0).abs() < 1e-3, "Ø8 at 2:1: r {r}");
    // The hole centre is 4 mm left of the detail centre: 8 mm on the sheet.
    assert!((cx - (200.0 - 8.0)).abs() < 1e-3 && (cy - 150.0).abs() < 1e-3);
    assert_eq!(b.center.len(), 2);
    assert!(b.texts.iter().any(|t| t.2 == "DETAIL B (2:1)"));
    let bnd = &g["detail-B"];
    let mut radii: Vec<f64> = bnd.circles.iter().map(|c| c.2).collect();
    radii.sort_by(f64::total_cmp);
    assert_eq!(
        radii,
        vec![10.0, 20.0],
        "boundary on the top view (1:1) and the detail (2:1)"
    );
    let c = &g["view-5"];
    assert_eq!(c.visible.len(), 1);
    let want = 2.0 * (64.0f64 - 16.0).sqrt() * 2.0;
    assert!(
        (len(&c.visible[0]) - want).abs() < 1e-3,
        "{}",
        len(&c.visible[0])
    );
    assert!(
        (c.visible[0].a[0] - (255.0 + 8.0)).abs() < 1e-3,
        "x = 40 is 4 mm right of the centre"
    );
    assert!(c.circles.is_empty());
    let dv = rep.views.iter().find(|v| v.id == ViewId(6)).unwrap();
    assert_eq!((dv.visible_curves, dv.hidden_curves), (1, 0), "{dv:?}");
    // The detail follows the model: the hole grows to Ø10.
    ok(
        &mut s,
        Command::SetParameter {
            feature: hole,
            name: "diameter".into(),
            value: 10.0,
        },
    );
    let (_, svg) = export(&mut s, d, "details_edit.svg");
    let (_, _, g) = read_svg(std::str::from_utf8(&svg).unwrap());
    assert!((g["view-4"].circles[0].2 - 10.0).abs() < 1e-3);
}
