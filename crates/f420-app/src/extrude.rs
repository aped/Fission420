//! Extrude dialog (New Body / Join / Cut, target body, distance or Through
//! All, Reverse) and its client-side translucent preview.
//!
//! The preview is a prism computed in the UI from the sketch's solved profile,
//! its frame and the dialog values; it is never sent to the document. Direction
//! semantics match the regenerator (ADR 0003): along the sketch normal, except
//! a Cut sketched on a face of its target body goes into the material (against
//! the face's outward normal); Reverse flips either default. Through All covers
//! both sides of the sketch plane through the target's extent along the normal
//! (taken here from the target's display mesh, plus a small margin).

use eframe::egui::{self, Color32, Pos2, RichText, Stroke};
use f420_commands::{BodySummary, Command, ExtrudeOp, FeatureSummary};
use f420_document::{BodyId, FeatureId, SketchPlane};

use crate::app::{F420App, parse_num};
use crate::place::PlaneFrame;
use crate::probe::Probe as _;

/// Edit buffers of the Extrude dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtrudeForm {
    pub sketch: Option<FeatureId>,
    pub distance: String,
    pub op: ExtrudeOp,
    /// Join/Cut target (required for those operations).
    pub target: Option<BodyId>,
    /// Cut only.
    pub through_all: bool,
    pub reverse: bool,
}

/// Preview prism in world coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtrudePreview {
    /// Profile outline at the near and far end of the tool (same vertex order).
    pub start: Vec<[f64; 3]>,
    pub end: Vec<[f64; 3]>,
    /// Signed offsets (mm) of the two ends along the sketch normal.
    pub z_range: [f64; 2],
    pub op: ExtrudeOp,
}

/// Degrees per sampled arc chord of the preview outline.
const ARC_STEP_DEG: f64 = 6.0;
/// Through-all preview overshoot: 1 mm plus 1 % of the target's extent.
const THROUGH_MARGIN_MM: f64 = 1.0;
const THROUGH_MARGIN_REL: f64 = 0.01;

/// Closed outline of a solved sketch's profile in sketch coordinates.
pub fn profile_outline(sketch: &f420_sketch::Sketch) -> Option<Vec<[f64; 2]>> {
    use f420_sketch::{LoopSegment, Profile};
    let arc = |out: &mut Vec<[f64; 2]>, s: [f64; 2], e: [f64; 2], c: [f64; 2], ccw: bool| {
        let r = (s[0] - c[0]).hypot(s[1] - c[1]);
        let a0 = (s[1] - c[1]).atan2(s[0] - c[0]);
        let a1 = (e[1] - c[1]).atan2(e[0] - c[0]);
        let mut sweep = a1 - a0;
        let tau = std::f64::consts::TAU;
        if ccw {
            while sweep <= 0.0 {
                sweep += tau;
            }
        } else {
            while sweep >= 0.0 {
                sweep -= tau;
            }
        }
        let n = ((sweep.abs().to_degrees() / ARC_STEP_DEG).ceil() as usize).clamp(2, 120);
        for k in 0..n {
            let a = a0 + sweep * k as f64 / n as f64;
            out.push([c[0] + r * a.cos(), c[1] + r * a.sin()]);
        }
    };
    let pts = match sketch.profile().ok()? {
        Profile::Polygon(v) => v,
        Profile::Circle { center, radius } => (0..60)
            .map(|k| {
                let a = std::f64::consts::TAU * k as f64 / 60.0;
                [center[0] + radius * a.cos(), center[1] + radius * a.sin()]
            })
            .collect(),
        Profile::Loop(segs) => {
            let mut out = Vec::new();
            for (i, s) in segs.iter().enumerate() {
                let next = segs[(i + 1) % segs.len()].start();
                match s {
                    LoopSegment::Line { start } => out.push(*start),
                    LoopSegment::Arc { start, center, ccw } => {
                        arc(&mut out, *start, next, *center, *ccw)
                    }
                }
            }
            out
        }
    };
    (pts.len() >= 3).then_some(pts)
}

/// Body whose face hosts `sketch` (None: world plane or unresolved).
pub fn host_body(features: &[FeatureSummary], sketch: &FeatureSummary) -> Option<BodyId> {
    let SketchPlane::Face(r) = sketch.plane? else {
        return None;
    };
    features
        .iter()
        .flat_map(|f| f.faces.iter())
        .find(|fs| fs.reference == r)
        .map(|fs| fs.body)
}

/// The regenerator's `coplanar_face_side`, on the planes of every planar face
/// of `target` (from the document summary, so faces made by patterns, mirrors
/// and chamfers count too): +1/-1 when the sketch plane holds a face whose
/// outward normal is along / against the sketch normal; None when it holds no
/// face of the target.
pub fn face_side(
    bodies: &[BodySummary],
    sketch: &FeatureSummary,
    target: Option<BodyId>,
) -> Option<f64> {
    let (target, f) = (target?, sketch.frame?);
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let b = bodies.iter().find(|b| b.id == target)?;
    b.planes.iter().find_map(|p| {
        let c = dot(p.normal, f.z);
        let d = [
            p.point[0] - f.origin[0],
            p.point[1] - f.origin[1],
            p.point[2] - f.origin[2],
        ];
        ((c.abs() - 1.0).abs() < 1e-7 && dot(d, f.z).abs() < 1e-7).then_some(c.signum())
    })
}

/// Default target: the body the sketch is on, else the only B-rep body.
pub fn default_target(
    features: &[FeatureSummary],
    bodies: &[BodySummary],
    sketch: Option<&FeatureSummary>,
) -> Option<BodyId> {
    if let Some(b) = sketch.and_then(|s| host_body(features, s)) {
        return Some(b);
    }
    let breps: Vec<_> = bodies.iter().filter(|b| b.kind == "brep").collect();
    match breps.as_slice() {
        [one] => Some(one.id),
        _ => None,
    }
}

impl ExtrudeForm {
    /// A fresh form for `sketch`: Join onto the body the sketch is on, else New Body.
    pub fn new(app: &F420App, sketch: Option<FeatureId>) -> Self {
        let fs = &app.summary.features;
        let s = sketch.and_then(|id| fs.iter().find(|f| f.id == id));
        let host = s.and_then(|s| host_body(fs, s));
        Self {
            sketch,
            distance: "10".into(),
            op: if host.is_some() {
                ExtrudeOp::Join
            } else {
                ExtrudeOp::NewBody
            },
            target: default_target(fs, &app.summary.bodies, s),
            through_all: false,
            reverse: false,
        }
    }

    pub fn command(&self) -> Result<Command, String> {
        let sketch = self.sketch.ok_or("choose a profile sketch")?;
        let through_all = self.through_all && self.op == ExtrudeOp::Cut;
        let distance = if through_all {
            0.0
        } else {
            parse_num("distance", &self.distance)?
        };
        let target = match self.op {
            ExtrudeOp::NewBody => None,
            _ => Some(self.target.ok_or("choose a target body for Join / Cut")?),
        };
        Ok(Command::Extrude {
            name: None,
            sketch,
            distance,
            op: self.op,
            target,
            through_all,
            reverse: self.reverse,
            body_name: None,
        })
    }

    /// The sketch plane holds a face of the target (+1/-1 as in [`face_side`]).
    pub fn side(&self, features: &[FeatureSummary], bodies: &[BodySummary]) -> Option<f64> {
        let s = self
            .sketch
            .and_then(|id| features.iter().find(|f| f.id == id))?;
        (self.op != ExtrudeOp::NewBody)
            .then(|| face_side(bodies, s, self.target))
            .flatten()
    }

    /// +1 along the sketch normal, -1 against it: the regenerator's rule (a
    /// Cut from a face of the target goes into the material, a Join away from it).
    pub fn direction(&self, features: &[FeatureSummary], bodies: &[BodySummary]) -> f64 {
        let base = match (self.op, self.side(features, bodies)) {
            (ExtrudeOp::Cut, Some(side)) => -side,
            (ExtrudeOp::Join, Some(side)) => side,
            _ => 1.0,
        };
        if self.reverse { -base } else { base }
    }
}

impl F420App {
    /// Preview prism of the open Extrude dialog, if it can be computed.
    pub fn extrude_preview(&self) -> Option<ExtrudePreview> {
        let Some(crate::app::Dialog::Extrude(form)) = &self.dialog else {
            return None;
        };
        let fs = &self.summary.features;
        let s = fs.iter().find(|f| Some(f.id) == form.sketch)?;
        let frame = PlaneFrame::from_kernel(&s.frame?);
        let outline = profile_outline(s.sketch.as_ref()?)?;
        let z_range = if form.through_all && form.op == ExtrudeOp::Cut {
            // Exact bounds from the summary (also for a hidden target).
            let b = self
                .summary
                .bodies
                .iter()
                .find(|b| Some(b.id) == form.target)?
                .bounds?;
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for x in [b.min[0], b.max[0]] {
                for y in [b.min[1], b.max[1]] {
                    for z in [b.min[2], b.max[2]] {
                        let v = [
                            x - frame.origin[0],
                            y - frame.origin[1],
                            z - frame.origin[2],
                        ];
                        let h = v[0] * frame.z[0] + v[1] * frame.z[1] + v[2] * frame.z[2];
                        lo = lo.min(h);
                        hi = hi.max(h);
                    }
                }
            }
            let margin = THROUGH_MARGIN_MM + THROUGH_MARGIN_REL * (hi - lo);
            [lo.min(0.0) - margin, hi.max(0.0) + margin]
        } else {
            let d = form.distance.trim().parse::<f64>().ok()?;
            if !(d.is_finite() && d > 0.0) {
                return None;
            }
            [0.0, form.direction(fs, &self.summary.bodies) * d]
        };
        let at = |p: [f64; 2], h: f64| {
            let w = frame.to_world(p);
            [
                w[0] + frame.z[0] * h,
                w[1] + frame.z[1] * h,
                w[2] + frame.z[2] * h,
            ]
        };
        Some(ExtrudePreview {
            start: outline.iter().map(|p| at(*p, z_range[0])).collect(),
            end: outline.iter().map(|p| at(*p, z_range[1])).collect(),
            z_range,
            op: form.op,
        })
    }

    /// Display groups of the open Extrude dialog's target (highlighted).
    pub(crate) fn extrude_target_groups(&self) -> Option<Vec<u32>> {
        let Some(crate::app::Dialog::Extrude(form)) = &self.dialog else {
            return None;
        };
        if form.op == ExtrudeOp::NewBody {
            return None;
        }
        let d = self
            .scene
            .bodies
            .iter()
            .find(|b| Some(b.id) == form.target)?;
        Some(d.groups.clone().collect())
    }

    /// Draw the translucent preview prism over the 3D view.
    pub(crate) fn paint_extrude_preview(&self, painter: &egui::Painter, rect: egui::Rect) {
        let Some(pv) = self.extrude_preview() else {
            return;
        };
        let (cam, [w, h]) = self.view_camera();
        let proj = |p: &[f64; 3]| {
            cam.project(p.map(|v| v as f32), w, h)
                .map(|[x, y]| rect.min + egui::vec2(x, y))
        };
        let (Some(a), Some(b)) = (
            pv.start.iter().map(proj).collect::<Option<Vec<Pos2>>>(),
            pv.end.iter().map(proj).collect::<Option<Vec<Pos2>>>(),
        ) else {
            return;
        };
        let (fill, line) = match pv.op {
            ExtrudeOp::Cut => (
                Color32::from_rgba_unmultiplied(255, 90, 90, 55),
                Color32::from_rgb(255, 120, 120),
            ),
            _ => (
                Color32::from_rgba_unmultiplied(110, 220, 140, 55),
                Color32::from_rgb(130, 230, 160),
            ),
        };
        let n = a.len();
        let mut mesh = egui::Mesh::default();
        for i in 0..n {
            let j = (i + 1) % n;
            let base = mesh.vertices.len() as u32;
            for p in [a[i], a[j], b[j], b[i]] {
                mesh.colored_vertex(p, fill);
            }
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
        for cap in [&a, &b] {
            for [i, j, k] in ear_clip(cap) {
                let base = mesh.vertices.len() as u32;
                for p in [cap[i], cap[j], cap[k]] {
                    mesh.colored_vertex(p, fill);
                }
                mesh.add_triangle(base, base + 1, base + 2);
            }
        }
        painter.add(egui::Shape::mesh(mesh));
        let stroke = Stroke::new(1.5, line);
        painter.add(egui::Shape::closed_line(a.clone(), stroke));
        painter.add(egui::Shape::closed_line(b.clone(), stroke));
        let step = (n / 8).max(1);
        for i in (0..n).step_by(step) {
            painter.line_segment([a[i], b[i]], stroke);
        }
        let label = match pv.op {
            ExtrudeOp::NewBody => "New body",
            ExtrudeOp::Join => "Join",
            ExtrudeOp::Cut => "Cut",
        };
        // Fixed spot at the bottom of the view, right of the axis gizmo and
        // clear of the dialog (the run-3 label next to the prism collided with
        // the dialog at 1280x800).
        crate::view3d::chip(
            painter,
            egui::pos2(rect.min.x + 150.0, rect.max.y - 58.0),
            format!(
                "Preview (display only; the exact solid is built and checked on OK)\n{label}: {:.3} … {:.3} mm along the sketch normal",
                pv.z_range[0], pv.z_range[1]
            ),
            line,
        );
    }

    /// Body of the Extrude dialog.
    pub(crate) fn extrude_form_ui(&self, ui: &mut egui::Ui, form: &mut ExtrudeForm) {
        let features = &self.summary.features;
        let sketches: Vec<&FeatureSummary> =
            features.iter().filter(|f| f.kind == "sketch").collect();
        let before = form.sketch;
        crate::panels::feature_combo(ui, "Profile sketch", &mut form.sketch, &sketches);
        if form.sketch != before {
            // Re-derive the defaults for the new sketch.
            *form = ExtrudeForm {
                distance: form.distance.clone(),
                ..ExtrudeForm::new(self, form.sketch)
            };
        }
        let s = form
            .sketch
            .and_then(|id| features.iter().find(|f| f.id == id));
        if let Some(e) = s.and_then(|s| s.profile_error.clone()) {
            ui.colored_label(
                Color32::from_rgb(255, 110, 110),
                format!("This sketch's profile is not usable: {e}"),
            );
        }
        ui.horizontal(|ui| {
            ui.label("Operation");
            ui.radio_value(&mut form.op, ExtrudeOp::NewBody, "New Body")
                .probe("New Body");
            ui.radio_value(&mut form.op, ExtrudeOp::Join, "Join")
                .probe("Join");
            ui.radio_value(&mut form.op, ExtrudeOp::Cut, "Cut")
                .probe("Cut");
        });
        if form.op != ExtrudeOp::NewBody {
            let breps: Vec<&BodySummary> = self
                .summary
                .bodies
                .iter()
                .filter(|b| b.kind == "brep")
                .collect();
            let text = form
                .target
                .and_then(|id| breps.iter().find(|b| b.id == id))
                .map_or("(choose)".to_string(), |b| format!("{} ({})", b.name, b.id));
            egui::ComboBox::from_label("Target body")
                .selected_text(text)
                .show_ui(ui, |ui| {
                    for b in &breps {
                        let label = if b.visible {
                            format!("{} ({})", b.name, b.id)
                        } else {
                            format!("{} ({}, hidden)", b.name, b.id)
                        };
                        ui.selectable_value(&mut form.target, Some(b.id), label);
                    }
                });
            if form.target.is_none() {
                ui.colored_label(
                    Color32::from_rgb(255, 200, 120),
                    if breps.is_empty() {
                        "There is no solid body to join to or cut (mesh bodies cannot take part)."
                    } else {
                        "Choose the target body (several bodies could be meant)."
                    },
                );
            }
            if self.summary.bodies.iter().any(|b| b.kind == "mesh") {
                ui.small("Mesh (STL) bodies are not offered: exact operations are unavailable on meshes.");
            }
        }
        if form.op == ExtrudeOp::Cut {
            ui.checkbox(&mut form.through_all, "Through All")
                .probe("Through All");
        } else {
            form.through_all = false;
        }
        if form.through_all {
            ui.small("Through All: both sides of the sketch plane, through the target's extent along the sketch normal (derived from the target, plus a small margin).");
        } else {
            crate::panels::num_field(ui, "Extrude distance (mm)", &mut form.distance);
        }
        let on_face = !form.through_all && form.side(features, &self.summary.bodies).is_some();
        if on_face {
            // Reversing a blind cut/join from a face points into air / into the
            // material: the regenerator refuses it, so it is not offered.
            form.reverse = false;
        }
        ui.add_enabled(!on_face, egui::Checkbox::new(&mut form.reverse, "Reverse direction"))
            .probe("Reverse direction")
            .on_disabled_hover_text("The sketch is on a face of the target: the direction is set by the face (Cut into the material, Join away from it)");
        if !form.through_all {
            let dir = form.direction(features, &self.summary.bodies);
            let base = match (form.op, on_face) {
                (ExtrudeOp::Cut, true) => "Cut from a face of the target goes into the material",
                (ExtrudeOp::Join, true) => {
                    "Join on a face of the target goes away from the material"
                }
                _ => "Extrudes along the sketch normal",
            };
            ui.small(format!(
                "{base}{}; tool runs {} the sketch normal.",
                if form.reverse { ", reversed" } else { "" },
                if dir > 0.0 { "along" } else { "against" }
            ));
        }
        ui.label(
            RichText::new("Units: mm. Translucent preview only; nothing changes until OK.")
                .small()
                .weak(),
        );
    }
}

/// Triangulate a simple polygon (screen space) by ear clipping. Returns
/// vertex index triples; falls back to a fan if the polygon is degenerate.
fn ear_clip(p: &[Pos2]) -> Vec<[usize; 3]> {
    let n = p.len();
    if n < 3 {
        return vec![];
    }
    let area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let cross =
        |a: Pos2, b: Pos2, c: Pos2| ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)) * sign;
    let inside = |q: Pos2, a: Pos2, b: Pos2, c: Pos2| {
        cross(a, b, q) >= 0.0 && cross(b, c, q) >= 0.0 && cross(c, a, q) >= 0.0
    };
    let mut idx: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity(n - 2);
    let mut guard = 0;
    while idx.len() > 3 && guard < n * n {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for k in 0..m {
            let (i0, i1, i2) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            let (a, b, c) = (p[i0], p[i1], p[i2]);
            if cross(a, b, c) <= 0.0 {
                continue;
            }
            if idx
                .iter()
                .any(|&j| j != i0 && j != i1 && j != i2 && inside(p[j], a, b, c))
            {
                continue;
            }
            out.push([i0, i1, i2]);
            idx.remove(k);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    if idx.len() == 3 {
        out.push([idx[0], idx[1], idx[2]]);
    } else if idx.len() > 3 {
        for k in 1..idx.len() - 1 {
            out.push([idx[0], idx[k], idx[k + 1]]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ear_clip_concave_l_shape() {
        let p: Vec<Pos2> = [
            (0.0, 0.0),
            (2.0, 0.0),
            (2.0, 1.0),
            (1.0, 1.0),
            (1.0, 2.0),
            (0.0, 2.0),
        ]
        .iter()
        .map(|&(x, y)| Pos2::new(x, y))
        .collect();
        let t = ear_clip(&p);
        assert_eq!(t.len(), 4);
        let area: f32 = t
            .iter()
            .map(|[i, j, k]| {
                let (a, b, c) = (p[*i], p[*j], p[*k]);
                0.5 * ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs()
            })
            .sum();
        assert!((area - 3.0).abs() < 1e-5, "{area}");
    }
}
