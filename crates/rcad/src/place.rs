//! Interactive placement of sketch templates in the 3D view.
//!
//! Pure geometry plus a small state machine driven by world rays, so the same
//! code serves mouse input, the `--demo-place` script and tests (no wgpu
//! needed). A finished drag yields a [`Proposal`]; the app turns it into a
//! pre-filled dialog and only the dialog's OK sends a
//! [`rcad_commands::Command`]. Nothing here touches the document.
//!
//! Planes are derived from semantic references only: world XY, or the end cap
//! of an extrusion (`end_cap_of`) whose frame is the profile sketch's frame
//! offset by the extrusion distance along its normal. No face indices or mesh
//! picks are used.
//!
//! Snapping: positions snap to [`SNAP_MM`] (1 mm) in the plane's own frame,
//! and the circle diameter snaps to whole millimetres too. Hold Shift for no
//! snapping. 1 mm was chosen because the product's sample parts are
//! dimensioned in whole millimetres and it keeps dialog values readable;
//! finer values can be typed into the dialog.

use rcad_commands::FeatureSummary;
use rcad_document::FeatureId;
use rcad_kernel::Frame;
use rcad_viewport::{Ray, ray_plane};

/// Default snap increment for placement, millimetres.
pub const SNAP_MM: f64 = 1.0;
/// Segments used for the preview circle.
const CIRCLE_SEGMENTS: usize = 72;

/// An orthonormal plane frame in f64 (local x, y in the plane; z normal).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaneFrame {
    pub origin: [f64; 3],
    pub x: [f64; 3],
    pub y: [f64; 3],
    pub z: [f64; 3],
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl PlaneFrame {
    pub const WORLD_XY: PlaneFrame = PlaneFrame {
        origin: [0.0; 3],
        x: [1.0, 0.0, 0.0],
        y: [0.0, 1.0, 0.0],
        z: [0.0, 0.0, 1.0],
    };

    pub fn from_kernel(f: &Frame) -> Self {
        Self {
            origin: f.origin,
            x: f.x,
            y: f.y,
            z: f.z,
        }
    }

    /// The same frame moved `d` along its normal.
    pub fn offset(self, d: f64) -> Self {
        let o = self.origin;
        Self {
            origin: [
                o[0] + d * self.z[0],
                o[1] + d * self.z[1],
                o[2] + d * self.z[2],
            ],
            ..self
        }
    }

    pub fn to_world(&self, p: [f64; 2]) -> [f64; 3] {
        std::array::from_fn(|k| self.origin[k] + p[0] * self.x[k] + p[1] * self.y[k])
    }

    pub fn to_local(&self, w: [f64; 3]) -> [f64; 2] {
        let r = [
            w[0] - self.origin[0],
            w[1] - self.origin[1],
            w[2] - self.origin[2],
        ];
        [dot(r, self.x), dot(r, self.y)]
    }

    /// Ray hit in local plane coordinates.
    pub fn intersect(&self, ray: &Ray) -> Option<[f64; 2]> {
        ray_plane(ray, self.origin, self.z).map(|w| self.to_local(w))
    }
}

/// Round to the nearest multiple of `step` (no-op for `None`/non-positive).
pub fn snap(v: f64, step: Option<f64>) -> f64 {
    match step {
        Some(s) if s > 0.0 && s.is_finite() => {
            let r = (v / s).round() * s;
            // Avoid "-0" in dialog text.
            if r == 0.0 { 0.0 } else { r }
        }
        _ => v,
    }
}

pub fn snap2(p: [f64; 2], step: Option<f64>) -> [f64; 2] {
    [snap(p[0], step), snap(p[1], step)]
}

/// Opposite corners → (width, height, centre). Order of corners is irrelevant.
pub fn rect_from_corners(a: [f64; 2], b: [f64; 2]) -> (f64, f64, [f64; 2]) {
    (
        (b[0] - a[0]).abs(),
        (b[1] - a[1]).abs(),
        [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])],
    )
}

/// Centre and a point on the rim → diameter, snapped to `step`.
pub fn diameter_from(center: [f64; 2], edge: [f64; 2], step: Option<f64>) -> f64 {
    let r = (edge[0] - center[0]).hypot(edge[1] - center[1]);
    snap(2.0 * r, step)
}

/// Advisory only (the command is authoritative): is the circle strictly inside
/// an axis-aligned rectangle given as (width, height, centre)?
pub fn circle_inside_rect(center: [f64; 2], diameter: f64, rect: (f64, f64, [f64; 2])) -> bool {
    let (w, h, c) = rect;
    let r = 0.5 * diameter;
    (center[0] - c[0]).abs() + r < 0.5 * w && (center[1] - c[1]).abs() + r < 0.5 * h
}

fn param(f: &FeatureSummary, name: &str) -> Option<f64> {
    f.parameters
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| *v)
}

/// The profile sketch an extrusion consumes.
fn profile_of(
    features: &[FeatureSummary],
    extrude: FeatureId,
) -> Result<(&FeatureSummary, &FeatureSummary), String> {
    let e = features
        .iter()
        .find(|f| f.id == extrude && f.kind == "extrude")
        .ok_or_else(|| format!("{extrude} is not an extrusion"))?;
    let s = features
        .iter()
        .find(|f| f.kind == "sketch" && e.depends_on.contains(&f.id))
        .ok_or_else(|| format!("{extrude} has no profile sketch"))?;
    Ok((e, s))
}

/// End-cap plane of `extrude`: profile-sketch frame offset by `distance`.
pub fn end_cap_frame(
    features: &[FeatureSummary],
    extrude: FeatureId,
) -> Result<PlaneFrame, String> {
    let (e, s) = profile_of(features, extrude)?;
    let frame = s
        .frame
        .as_ref()
        .ok_or_else(|| format!("{} has no evaluated frame", s.id))?;
    let d = param(e, "distance").ok_or_else(|| format!("{extrude} has no distance"))?;
    Ok(PlaneFrame::from_kernel(frame).offset(d))
}

/// The base rectangle of `extrude`, in its (and its end cap's) local frame.
pub fn host_rect(features: &[FeatureSummary], extrude: FeatureId) -> Option<(f64, f64, [f64; 2])> {
    let (_, s) = profile_of(features, extrude).ok()?;
    let c = [
        param(s, "center_x").unwrap_or(0.0),
        param(s, "center_y").unwrap_or(0.0),
    ];
    Some((param(s, "width")?, param(s, "height")?, c))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tool {
    /// Corner-to-corner rectangle on world XY.
    Rectangle,
    /// Centre-then-rim circle on the end cap of `extrude`.
    Hole { extrude: FeatureId },
}

/// Result of a completed drag: values to pre-fill the dialog with.
#[derive(Clone, Debug, PartialEq)]
pub enum Proposal {
    Rectangle {
        width: f64,
        height: f64,
        center: [f64; 2],
    },
    Circle {
        extrude: FeatureId,
        diameter: f64,
        center: [f64; 2],
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Press,
    Move,
    Release,
}

/// Preview geometry in world coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub outline: Vec<[f64; 3]>,
    /// Extra construction lines (radius, centre cross), each a 2-point segment.
    pub guides: Vec<[[f64; 3]; 2]>,
    pub label: String,
    pub label_at: [f64; 3],
    /// Advisory: false if the shape obviously violates the inside-the-base limit.
    pub plausible: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub tool: Tool,
    pub frame: PlaneFrame,
    /// Base rectangle for the advisory inside check (holes only).
    pub host: Option<(f64, f64, [f64; 2])>,
    start: Option<[f64; 2]>,
    current: Option<[f64; 2]>,
    snapping: bool,
    /// Last proposal, kept visible while its dialog is open.
    pub proposed: Option<Proposal>,
}

impl Placement {
    pub fn new(tool: Tool, frame: PlaneFrame) -> Self {
        Self {
            tool,
            frame,
            host: None,
            start: None,
            current: None,
            snapping: true,
            proposed: None,
        }
    }

    /// Start a placement for `tool` against the current document features.
    pub fn for_document(tool: Tool, features: &[FeatureSummary]) -> Result<Self, String> {
        Ok(match tool {
            Tool::Rectangle => Self::new(tool, PlaneFrame::WORLD_XY),
            Tool::Hole { extrude } => {
                let mut p = Self::new(tool, end_cap_frame(features, extrude)?);
                p.host = host_rect(features, extrude);
                p
            }
        })
    }

    pub fn dragging(&self) -> bool {
        self.start.is_some()
    }

    fn step(&self) -> Option<f64> {
        self.snapping.then_some(SNAP_MM)
    }

    /// Feed one pointer event. `snapping = false` when Shift is held.
    /// Returns a proposal (or a user-facing reason for none) on release.
    pub fn pointer(
        &mut self,
        phase: Phase,
        ray: &Ray,
        snapping: bool,
    ) -> Option<Result<Proposal, String>> {
        self.snapping = snapping;
        let hit = self.frame.intersect(ray);
        match phase {
            Phase::Press => {
                self.proposed = None;
                self.start = hit.map(|h| snap2(h, self.step()));
                self.current = self.start;
                if self.start.is_none() {
                    return Some(Err("pointer is not over the placement plane".into()));
                }
                None
            }
            Phase::Move => {
                if let Some(h) = hit {
                    self.current = Some(h);
                }
                None
            }
            Phase::Release => {
                if let Some(h) = hit {
                    self.current = Some(h);
                }
                let r = self.proposal();
                self.start = None;
                self.current = None;
                if let Some(Ok(p)) = &r {
                    self.proposed = Some(p.clone());
                }
                r
            }
        }
    }

    /// Cancel an in-progress drag (tool stays active).
    pub fn cancel_drag(&mut self) {
        self.start = None;
        self.current = None;
    }

    fn proposal(&self) -> Option<Result<Proposal, String>> {
        let (a, b) = (self.start?, self.current?);
        let step = self.step();
        Some(match self.tool {
            Tool::Rectangle => {
                let (width, height, center) = rect_from_corners(a, snap2(b, step));
                if width <= 0.0 || height <= 0.0 {
                    Err("drag a rectangle with non-zero width and height".into())
                } else {
                    Ok(Proposal::Rectangle {
                        width,
                        height,
                        center,
                    })
                }
            }
            Tool::Hole { extrude } => {
                let diameter = diameter_from(a, b, step);
                if diameter <= 0.0 {
                    Err("drag outward from the centre to set the hole size".into())
                } else {
                    Ok(Proposal::Circle {
                        extrude,
                        diameter,
                        center: a,
                    })
                }
            }
        })
    }

    /// Live preview while dragging, or the held proposal while its dialog is open.
    pub fn preview(&self) -> Option<Preview> {
        let shape = match (self.proposal(), &self.proposed) {
            (Some(Ok(p)), _) => p,
            (_, Some(p)) => p.clone(),
            _ => return None,
        };
        let f = &self.frame;
        Some(match shape {
            Proposal::Rectangle {
                width,
                height,
                center: c,
            } => {
                let (hw, hh) = (0.5 * width, 0.5 * height);
                let corners = [
                    [c[0] - hw, c[1] - hh],
                    [c[0] + hw, c[1] - hh],
                    [c[0] + hw, c[1] + hh],
                    [c[0] - hw, c[1] + hh],
                ];
                Preview {
                    outline: corners.iter().map(|p| f.to_world(*p)).collect(),
                    guides: vec![],
                    label: format!(
                        "{} × {} mm @ ({}, {})",
                        fmt(width),
                        fmt(height),
                        fmt(c[0]),
                        fmt(c[1])
                    ),
                    label_at: f.to_world(corners[2]),
                    plausible: true,
                }
            }
            Proposal::Circle {
                diameter,
                center: c,
                ..
            } => {
                let r = 0.5 * diameter;
                let outline = (0..CIRCLE_SEGMENTS)
                    .map(|i| {
                        let t = std::f64::consts::TAU * i as f64 / CIRCLE_SEGMENTS as f64;
                        f.to_world([c[0] + r * t.cos(), c[1] + r * t.sin()])
                    })
                    .collect();
                let tick = (0.15 * r).clamp(0.5, 3.0);
                let rim = [c[0] + r, c[1]];
                Preview {
                    outline,
                    guides: vec![
                        [f.to_world(c), f.to_world(rim)],
                        [
                            f.to_world([c[0] - tick, c[1]]),
                            f.to_world([c[0] + tick, c[1]]),
                        ],
                        [
                            f.to_world([c[0], c[1] - tick]),
                            f.to_world([c[0], c[1] + tick]),
                        ],
                    ],
                    label: format!("Ø{} mm @ ({}, {})", fmt(diameter), fmt(c[0]), fmt(c[1])),
                    label_at: f.to_world(rim),
                    plausible: self.host.is_none_or(|h| circle_inside_rect(c, diameter, h)),
                }
            }
        })
    }
}

/// Dialog-friendly number text (trims float noise from snapped values).
pub fn fmt(v: f64) -> String {
    let r = (v * 1.0e6).round() / 1.0e6;
    format!("{}", if r == 0.0 { 0.0 } else { r })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcad_viewport::Camera;

    fn ray_down(x: f32, y: f32) -> Ray {
        Ray {
            origin: [x, y, 100.0],
            dir: [0.0, 0.0, -1.0],
        }
    }

    fn feats() -> Vec<FeatureSummary> {
        let base = FeatureSummary {
            id: FeatureId(1),
            name: "Base".into(),
            kind: "sketch".into(),
            parameters: vec![
                ("width".into(), 80.0),
                ("height".into(), 50.0),
                ("center_x".into(), 5.0),
                ("center_y".into(), 0.0),
            ],
            depends_on: vec![],
            plane: None,
            solve_conflicts: vec![],
            sketch: None,
            profile_error: None,
            faces: vec![],
            solve_status: None,
            solve_dof: None,
            solve_residual: None,
            frame: Some(Frame::XY),
            operation: None,
            extent: None,
            reverse: false,
            body: None,
            creates: vec![],
            notes: vec![],
            expressions: Default::default(),
            region: None,
            suppressed: false,
        };
        let ext = FeatureSummary {
            id: FeatureId(2),
            name: "Plate".into(),
            kind: "extrude".into(),
            parameters: vec![("distance".into(), 12.0)],
            depends_on: vec![FeatureId(1)],
            frame: None,
            ..base.clone()
        };
        vec![base, ext]
    }

    #[test]
    fn snapping_rounds_to_step_and_shift_disables() {
        assert_eq!(snap(12.4, Some(1.0)), 12.0);
        assert_eq!(snap(-0.4, Some(1.0)), 0.0);
        assert!(snap(-0.4, Some(1.0)).is_sign_positive());
        assert_eq!(snap(2.26, Some(0.5)), 2.5);
        assert_eq!(snap(2.26, None), 2.26);
        assert_eq!(snap(2.26, Some(0.0)), 2.26);
    }

    #[test]
    fn corners_give_size_and_centre_in_any_order() {
        let (w, h, c) = rect_from_corners([30.0, 10.0], [-10.0, -20.0]);
        assert_eq!((w, h, c), (40.0, 30.0, [10.0, -5.0]));
        assert_eq!(rect_from_corners([-10.0, -20.0], [30.0, 10.0]), (w, h, c));
    }

    #[test]
    fn centre_and_rim_give_diameter() {
        assert_eq!(diameter_from([1.0, 1.0], [4.0, 5.0], None), 10.0);
        assert_eq!(diameter_from([0.0, 0.0], [3.3, 0.0], Some(1.0)), 7.0);
    }

    #[test]
    fn end_cap_frame_follows_profile_frame_and_distance() {
        let f = end_cap_frame(&feats(), FeatureId(2)).unwrap();
        assert_eq!(f.origin, [0.0, 0.0, 12.0]);
        assert_eq!(f.z, [0.0, 0.0, 1.0]);
        assert!(end_cap_frame(&feats(), FeatureId(1)).is_err());
        assert!(end_cap_frame(&feats(), FeatureId(9)).is_err());
        assert_eq!(
            host_rect(&feats(), FeatureId(2)),
            Some((80.0, 50.0, [5.0, 0.0]))
        );
        // Local/world round trip on a non-trivial frame.
        let tilted = PlaneFrame {
            origin: [1.0, 2.0, 3.0],
            x: [0.0, 1.0, 0.0],
            y: [0.0, 0.0, 1.0],
            z: [1.0, 0.0, 0.0],
        };
        let w = tilted.to_world([4.0, -2.0]);
        assert_eq!(w, [1.0, 6.0, 1.0]);
        assert_eq!(tilted.to_local(w), [4.0, -2.0]);
    }

    #[test]
    fn advisory_inside_check() {
        let host = (80.0, 50.0, [0.0, 0.0]);
        assert!(circle_inside_rect([20.0, 8.0], 12.0, host));
        assert!(!circle_inside_rect([36.0, 0.0], 10.0, host)); // tangent
        assert!(!circle_inside_rect([0.0, 0.0], 50.0, host));
    }

    #[test]
    fn rectangle_drag_produces_snapped_proposal() {
        let mut p = Placement::new(Tool::Rectangle, PlaneFrame::WORLD_XY);
        assert!(
            p.pointer(Phase::Press, &ray_down(-20.2, -9.8), true)
                .is_none()
        );
        assert!(p.dragging());
        p.pointer(Phase::Move, &ray_down(10.0, 5.0), true);
        let pv = p.preview().expect("live preview");
        assert_eq!(pv.outline.len(), 4);
        assert!(pv.label.starts_with("30 × 15 mm"), "{}", pv.label);
        let r = p.pointer(Phase::Release, &ray_down(40.4, 20.3), true);
        assert_eq!(
            r,
            Some(Ok(Proposal::Rectangle {
                width: 60.0,
                height: 30.0,
                center: [10.0, 5.0]
            }))
        );
        assert!(!p.dragging());
        // Proposal stays previewed until the app clears it.
        assert!(p.preview().is_some());
    }

    #[test]
    fn unsnapped_and_degenerate_drags() {
        let mut p = Placement::new(Tool::Rectangle, PlaneFrame::WORLD_XY);
        p.pointer(Phase::Press, &ray_down(0.25, 0.0), false);
        let r = p.pointer(Phase::Release, &ray_down(10.75, 4.5), false);
        let Some(Ok(Proposal::Rectangle { width, height, .. })) = r else {
            panic!("{r:?}");
        };
        assert!((width - 10.5).abs() < 1e-5 && (height - 4.5).abs() < 1e-5);
        // A click without movement is rejected with a reason.
        p.pointer(Phase::Press, &ray_down(3.0, 3.0), true);
        assert!(matches!(
            p.pointer(Phase::Release, &ray_down(3.2, 3.1), true),
            Some(Err(_))
        ));
        // Pressing off the plane (ray pointing away) reports an error.
        let away = Ray {
            origin: [0.0, 0.0, 100.0],
            dir: [0.0, 0.0, 1.0],
        };
        assert!(matches!(p.pointer(Phase::Press, &away, true), Some(Err(_))));
        assert!(!p.dragging());
    }

    #[test]
    fn hole_drag_on_end_cap_with_camera_rays() {
        let mut p = Placement::for_document(
            Tool::Hole {
                extrude: FeatureId(2),
            },
            &feats(),
        )
        .unwrap();
        let cam = Camera {
            target: [0.0, 0.0, 6.0],
            distance: 250.0,
            yaw: -0.7,
            pitch: 0.6,
            fov_y: 35f32.to_radians(),
        };
        let (w, h) = (1000.0, 700.0);
        let ray_at = |world: [f32; 3]| {
            let [x, y] = cam.project(world, w, h).unwrap();
            cam.ray(x, y, w, h).unwrap()
        };
        // Points on the cap plane z = 12.
        p.pointer(Phase::Press, &ray_at([20.2, 7.9, 12.0]), true);
        p.pointer(Phase::Move, &ray_at([26.1, 8.0, 12.0]), true);
        let pv = p.preview().unwrap();
        assert!(pv.plausible);
        assert!(pv.outline.iter().all(|q| (q[2] - 12.0).abs() < 1e-9));
        let r = p.pointer(Phase::Release, &ray_at([26.1, 8.0, 12.0]), true);
        assert_eq!(
            r,
            Some(Ok(Proposal::Circle {
                extrude: FeatureId(2),
                diameter: 12.0,
                center: [20.0, 8.0]
            }))
        );
        // A hole crossing the base edge is flagged in the preview.
        p.pointer(Phase::Press, &ray_at([40.0, 0.0, 12.0]), true);
        p.pointer(Phase::Move, &ray_at([46.0, 0.0, 12.0]), true);
        assert!(!p.preview().unwrap().plausible);
    }
}
