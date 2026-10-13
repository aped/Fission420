//! Associative drawings (run 6, SESSION_7 §4 foundation; docs/DRAWINGS.md).
//!
//! A [`Drawing`] is stored in the document (format v6): sheet size, projection
//! convention, title block fields and views. Views reference bodies by
//! [`BodyId`] and carry only placement (orientation, scale, position) and
//! semantic annotations; nothing geometric is stored. Every line, dimension
//! and centre mark is computed by [`render`] from the *current* regenerated
//! geometry at export time, so a stale dimension cannot exist: nothing is
//! cached. A view whose body no longer exists is reported and drawn as a red
//! flag, never redrawn from old data; a hole callout whose feature was
//! deleted, suppressed or no longer appears end-on is reported as dangling and
//! drawn in red without a value.
//!
//! Hidden-line removal is *sampled*: every sharp B-rep edge (exact lines and
//! arcs from `rcad_kernel::sharp_edges`) and every silhouette is sampled at
//! [`sample_spacing`] and each sample is tested against a tessellation of the
//! view's bodies along the view direction (chord [`hlr_chord`]); transitions
//! between visible and hidden samples are refined by bisection to
//! [`TRANSITION_TOL`]. The drawn curves are the exact projected edges cut at
//! those transitions. See docs/DRAWINGS.md for the stated tolerances.
//!
//! Section views cut the bodies with the exact Split Body cut
//! (`regen::split_by_plane`), draw the part behind the plane the same way and
//! hatch the faces lying in the plane ([`HATCH_SPACING`]).
use crate::{BodyId, DocError, Document, Evaluation, FeatureId, FeatureKind, Result};
use rcad_exchange::drawing::{Anchor, Group, LineStyle, Prim, SheetPlot};
use rcad_kernel::{self as k, EdgeCurve};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::f64::consts::TAU;
use std::fmt;

/// Identifier of a drawing in a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DrawingId(pub u32);

impl fmt::Display for DrawingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "D{}", self.0)
    }
}

/// Identifier of a view within a drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ViewId(pub u32);

impl fmt::Display for ViewId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "V{}", self.0)
    }
}

/// Sheet size (landscape).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SheetSize {
    #[default]
    A4Landscape,
    A3Landscape,
}

impl SheetSize {
    /// Width and height in mm.
    pub fn mm(self) -> (f64, f64) {
        match self {
            SheetSize::A4Landscape => (297.0, 210.0),
            SheetSize::A3Landscape => (420.0, 297.0),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            SheetSize::A4Landscape => "A4",
            SheetSize::A3Landscape => "A3",
        }
    }
}

/// Projection convention, recorded on the sheet (title block).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Projection {
    /// ISO E / ASME: the view from above is placed above, from the right to
    /// the right.
    #[default]
    ThirdAngle,
    /// ISO A: the view from above is placed below, from the right to the left.
    FirstAngle,
}

impl Projection {
    pub fn label(self) -> &'static str {
        match self {
            Projection::ThirdAngle => "THIRD ANGLE PROJECTION",
            Projection::FirstAngle => "FIRST ANGLE PROJECTION",
        }
    }
}

/// Standard orientation of a base view (Z up, the viewport's presets).
///
/// | view   | looks along | right | up  |
/// |--------|-------------|-------|-----|
/// | Front  | +Y          | +X    | +Z  |
/// | Back   | -Y          | -X    | +Z  |
/// | Top    | -Z          | +X    | +Y  |
/// | Bottom | +Z          | +X    | -Y  |
/// | Right  | -X          | +Y    | +Z  |
/// | Left   | +X          | -Y    | +Z  |
/// | Iso    | (-1, 1, -1) | (1, 1, 0) | Z-ish |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Front,
    Back,
    Top,
    Bottom,
    Right,
    Left,
    Iso,
}

impl Orientation {
    pub const ALL: [Orientation; 7] = [
        Orientation::Front,
        Orientation::Top,
        Orientation::Right,
        Orientation::Left,
        Orientation::Back,
        Orientation::Bottom,
        Orientation::Iso,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Orientation::Front => "Front",
            Orientation::Back => "Back",
            Orientation::Top => "Top",
            Orientation::Bottom => "Bottom",
            Orientation::Right => "Right",
            Orientation::Left => "Left",
            Orientation::Iso => "Iso",
        }
    }
    /// The view frame.
    pub fn frame(self) -> ViewFrame {
        let (r, u) = match self {
            Orientation::Front => ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            Orientation::Back => ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            Orientation::Top => ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            Orientation::Bottom => ([1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
            Orientation::Right => ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            Orientation::Left => ([0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
            Orientation::Iso => {
                let s = std::f64::consts::FRAC_1_SQRT_2;
                let toward = norm([1.0, -1.0, 1.0]);
                let r = [s, s, 0.0];
                (r, cross(toward, r))
            }
        };
        ViewFrame::from_right_up(r, u)
    }
}

/// Which side the parent is seen from in a projected view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Above,
    Below,
    Left,
    Right,
}

impl Side {
    pub fn label(self) -> &'static str {
        match self {
            Side::Above => "from above",
            Side::Below => "from below",
            Side::Left => "from the left",
            Side::Right => "from the right",
        }
    }
    /// Unit direction on the sheet where the projected view is placed
    /// relative to its parent under `p`.
    pub fn placement(self, p: Projection) -> [f64; 2] {
        let d = match self {
            Side::Above => [0.0, 1.0],
            Side::Below => [0.0, -1.0],
            Side::Left => [-1.0, 0.0],
            Side::Right => [1.0, 0.0],
        };
        match p {
            Projection::ThirdAngle => d,
            Projection::FirstAngle => [-d[0], -d[1]],
        }
    }
}

/// Orthonormal view frame: `right`, `up` on the sheet, `toward` the viewer
/// (`right × up = toward`; the view looks along `-toward`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewFrame {
    pub right: [f64; 3],
    pub up: [f64; 3],
    pub toward: [f64; 3],
}

impl ViewFrame {
    fn from_right_up(r: [f64; 3], u: [f64; 3]) -> Self {
        let r = norm(r);
        let u = norm(u);
        ViewFrame {
            right: r,
            up: u,
            toward: cross(r, u),
        }
    }
    /// The parent seen from `side` (orthographic projection by 90°).
    pub fn seen_from(&self, side: Side) -> ViewFrame {
        let neg = |v: [f64; 3]| v.map(|x| -x);
        let (toward, up) = match side {
            Side::Above => (self.up, neg(self.toward)),
            Side::Below => (neg(self.up), self.toward),
            Side::Right => (self.right, self.up),
            Side::Left => (neg(self.right), self.up),
        };
        ViewFrame {
            right: cross(up, toward),
            up,
            toward,
        }
    }
    /// View coordinates (x right, y up, z toward the viewer), model mm.
    pub fn project(&self, p: [f64; 3]) -> [f64; 3] {
        [dot(p, self.right), dot(p, self.up), dot(p, self.toward)]
    }
    /// Named standard direction of `toward`, if axis-aligned.
    pub fn name(&self) -> &'static str {
        let t = self.toward;
        let names = [
            ([0.0, -1.0, 0.0], "Front"),
            ([0.0, 1.0, 0.0], "Back"),
            ([0.0, 0.0, 1.0], "Top"),
            ([0.0, 0.0, -1.0], "Bottom"),
            ([1.0, 0.0, 0.0], "Right"),
            ([-1.0, 0.0, 0.0], "Left"),
        ];
        names
            .iter()
            .find(|(d, _)| dot(*d, t) > 1.0 - 1e-9)
            .map(|(_, n)| *n)
            .unwrap_or("Iso")
    }
    /// World axis index of `v` when it is axis-aligned.
    fn axis_of(v: [f64; 3]) -> Option<usize> {
        (0..3).find(|&i| v[i].abs() > 1.0 - 1e-12)
    }
}

/// What a view shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    /// Bodies seen in a standard orientation at `scale` (sheet mm per model
    /// mm), centred at `position` (sheet mm from the bottom-left corner).
    ///
    /// Run 7 (format v7): with `assembly`, the view also shows every placed
    /// component occurrence of the evaluated assembly (suppressed ones
    /// excluded; `bodies` are then loose bodies shown in addition, and may
    /// be empty).
    Base {
        bodies: Vec<BodyId>,
        orientation: Orientation,
        scale: f64,
        position: [f64; 2],
        #[serde(default, skip_serializing_if = "is_false")]
        assembly: bool,
    },
    /// The parent seen from `from`, at the parent's scale, aligned with it and
    /// placed by the drawing's projection convention.
    Projected { parent: ViewId, from: Side },
    /// Full section `label`-`label`: the parent cut by the plane through its
    /// cutting line and seen from `from` (placed like a projected view). The
    /// cutting line lies in the parent view at `offset` model mm along the
    /// parent's up axis (`from` above/below: a horizontal line) or right axis
    /// (`from` left/right: a vertical line), in world coordinates projected on
    /// that axis. The material between the plane and the viewer is removed;
    /// faces in the plane are hatched.
    Section {
        parent: ViewId,
        from: Side,
        offset: f64,
        label: String,
    },
    /// Detail `label`: the region of the parent within `radius` model mm of
    /// `center` (parent view coordinates: model mm along its right/up axes),
    /// enlarged to `scale` and centred at `position` on the sheet. Same
    /// orientation as the parent; curves are clipped to the circle.
    Detail {
        parent: ViewId,
        center: [f64; 2],
        radius: f64,
        scale: f64,
        position: [f64; 2],
        label: String,
    },
}

impl ViewKind {
    /// The parent and side of a projected or section view.
    pub fn parent(&self) -> Option<(ViewId, Side)> {
        match self {
            ViewKind::Base { .. } | ViewKind::Detail { .. } => None,
            ViewKind::Projected { parent, from } | ViewKind::Section { parent, from, .. } => {
                Some((*parent, *from))
            }
        }
    }
    /// Label of a section or detail view.
    pub fn label(&self) -> Option<&str> {
        match self {
            ViewKind::Section { label, .. } | ViewKind::Detail { label, .. } => Some(label),
            _ => None,
        }
    }
    /// The view this one is derived from (projected, section or detail).
    pub fn parent_id(&self) -> Option<ViewId> {
        match self {
            ViewKind::Base { .. } => None,
            ViewKind::Projected { parent, .. }
            | ViewKind::Section { parent, .. }
            | ViewKind::Detail { parent, .. } => Some(*parent),
        }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A semantic annotation of a view, resolved at every render.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Annotation {
    /// Diameter callout of Hole feature `feature`, attached to its circle
    /// where the hole is seen end-on.
    HoleCallout { feature: FeatureId },
    /// Run 7 (v7): a driven dimension between semantic references, measured
    /// from the current model at every render (never stored). With `b`: the
    /// distance between `a` and `b` (parallel planar faces, a face and a
    /// parallel hole axis, or two parallel hole axes), measured
    /// perpendicular to them. Without `b`: the diameter of hole `a`. Drawn
    /// only where the measured direction lies in the view plane along the
    /// view's horizontal or vertical axis; otherwise, or when a reference no
    /// longer resolves, it is drawn in red without a value and reported as
    /// dangling.
    Dimension {
        a: DimRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        b: Option<DimRef>,
        #[serde(default, skip_serializing_if = "Tolerance::is_none")]
        tolerance: Tolerance,
    },
    /// Run 7 (v7): a balloon (item number of the drawing's BOM) for every
    /// component shown in this view, with a leader to the centre of its
    /// first occurrence. Only meaningful in assembly views.
    Balloons,
}

/// A stable geometric reference for a drawing dimension.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DimRef {
    /// A planar face by its semantic reference (originating feature + role).
    Face(crate::FaceRef),
    /// The axis (and diameter) of a Hole feature.
    Hole(FeatureId),
}

/// Tolerance printed with a dimension value (model mm).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Tolerance {
    #[default]
    None,
    /// `value ±t`.
    Symmetric { value: f64 },
    /// `value +upper/lower` (deviations; `lower` is usually negative).
    Deviation { upper: f64, lower: f64 },
    /// Limit dimensions: `value+upper` over `value+lower`.
    Limits { upper: f64, lower: f64 },
}

impl Tolerance {
    pub fn is_none(&self) -> bool {
        matches!(self, Tolerance::None)
    }

    pub fn validate(&self) -> std::result::Result<(), String> {
        match *self {
            Tolerance::None => Ok(()),
            Tolerance::Symmetric { value } => {
                if value.is_finite() && value > 0.0 {
                    Ok(())
                } else {
                    Err(format!("a ± tolerance must be positive, got {value}"))
                }
            }
            Tolerance::Deviation { upper, lower } | Tolerance::Limits { upper, lower } => {
                if upper.is_finite() && lower.is_finite() && upper > lower {
                    Ok(())
                } else {
                    Err(format!(
                        "the upper deviation {upper} must exceed the lower {lower}"
                    ))
                }
            }
        }
    }

    /// Dimension text for nominal `v`: "40", "40 ±0.1", "40 +0.1/-0.05",
    /// "40.1/39.95".
    pub fn text(&self, v: f64) -> String {
        let signed = |x: f64| {
            if x >= 0.0 {
                format!("+{}", fmt_tol(x))
            } else {
                format!("-{}", fmt_tol(-x))
            }
        };
        match *self {
            Tolerance::None => fmt_len(v),
            Tolerance::Symmetric { value } => format!("{} ±{}", fmt_len(v), fmt_tol(value)),
            Tolerance::Deviation { upper, lower } => {
                format!("{} {}/{}", fmt_len(v), signed(upper), signed(lower))
            }
            Tolerance::Limits { upper, lower } => {
                format!("{}/{}", fmt_tol(v + upper), fmt_tol(v + lower))
            }
        }
    }
}

/// Tolerance values keep three decimals (0.005 is a common deviation).
fn fmt_tol(x: f64) -> String {
    let s = format!("{:.3}", x + 0.0);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

/// A free text note on the sheet (run 7, v7). `position`: sheet mm of the
/// first line's start; lines are separated by `\n`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub text: String,
    pub position: [f64; 2],
}

/// The bill-of-materials table on the sheet (run 7, v7): flattened
/// quantities of the evaluated assembly (see [`bom_rows`]). `position`: its
/// bottom-left corner in sheet mm (default: the sheet's top-right corner
/// inside the border).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct BomTable {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f64; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrawingView {
    pub id: ViewId,
    pub kind: ViewKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<Annotation>,
}

/// Title block fields. Empty `part_number` / `material`: taken from the
/// model (the active configuration's part number; the bodies' materials).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TitleBlock {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub part_number: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub material: String,
}

/// A drawing sheet (format v6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Drawing {
    pub id: DrawingId,
    pub name: String,
    #[serde(default)]
    pub sheet: SheetSize,
    #[serde(default)]
    pub projection: Projection,
    #[serde(default)]
    pub title: TitleBlock,
    #[serde(default)]
    pub views: Vec<DrawingView>,
    #[serde(default = "one")]
    pub next_view: u32,
    /// Overall bounding dimensions (each world axis once) and hole diameters.
    #[serde(default = "yes")]
    pub auto_dimensions: bool,
    /// Run 7 (v7): free text notes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
    /// Run 7 (v7): BOM table on the sheet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bom: Option<BomTable>,
}

fn one() -> u32 {
    1
}
fn yes() -> bool {
    true
}

/// Accepted scale range (sheet mm per model mm).
pub const MIN_SCALE: f64 = 1.0e-3;
pub const MAX_SCALE: f64 = 1.0e3;
/// Default sheet gap (mm) between a view's outline and a projected view's.
pub const VIEW_GAP: f64 = 22.0;

impl Drawing {
    pub fn new(id: DrawingId, name: &str) -> Self {
        Drawing {
            id,
            name: name.to_string(),
            sheet: SheetSize::default(),
            projection: Projection::default(),
            title: TitleBlock {
                title: name.to_string(),
                ..Default::default()
            },
            views: Vec::new(),
            next_view: 1,
            auto_dimensions: true,
            notes: Vec::new(),
            bom: None,
        }
    }

    pub fn view(&self, id: ViewId) -> Result<&DrawingView> {
        self.views
            .iter()
            .find(|v| v.id == id)
            .ok_or_else(|| DocError::Validation(format!("{} has no view {id}", self.id)))
    }

    pub fn view_mut(&mut self, id: ViewId) -> Result<&mut DrawingView> {
        let did = self.id;
        self.views
            .iter_mut()
            .find(|v| v.id == id)
            .ok_or_else(|| DocError::Validation(format!("{did} has no view {id}")))
    }

    /// Frame, scale and the base view of `id` (following projected parents).
    pub fn resolve(&self, id: ViewId) -> Result<(ViewFrame, f64, ViewId)> {
        let mut chain = Vec::new();
        let mut cur = id;
        loop {
            if chain.len() > self.views.len() {
                return Err(DocError::Validation(format!(
                    "{}: projected views form a cycle at {id}",
                    self.id
                )));
            }
            let v = self.view(cur)?;
            match &v.kind {
                ViewKind::Base {
                    orientation, scale, ..
                } => {
                    let mut f = orientation.frame();
                    for side in chain.iter().rev() {
                        f = f.seen_from(*side);
                    }
                    return Ok((f, *scale, cur));
                }
                ViewKind::Projected { parent, from } | ViewKind::Section { parent, from, .. } => {
                    chain.push(*from);
                    cur = *parent;
                }
                ViewKind::Detail { parent, scale, .. } => {
                    if cur != id || *parent == id {
                        return Err(DocError::Validation(format!(
                            "{}: views cannot be derived from the detail view {cur}",
                            self.id
                        )));
                    }
                    let (f, _, base) = self.resolve(*parent)?;
                    return Ok((f, *scale, base));
                }
            }
        }
    }

    /// Bodies shown by view `id` (a projected view shows its base's bodies).
    pub fn bodies(&self, id: ViewId) -> Result<Vec<BodyId>> {
        let (_, _, base) = self.resolve(id)?;
        match &self.view(base)?.kind {
            ViewKind::Base { bodies, .. } => Ok(bodies.clone()),
            _ => unreachable!("resolve returns a base view"),
        }
    }

    /// Structural checks (every edit): finite scale/position in range, at
    /// least one body per base view, parents earlier in the list, no two
    /// projected views of one parent from the same side.
    pub fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(DocError::Validation(format!("{}: {m}", self.id)));
        if self.name.trim().is_empty() {
            return bad("the drawing needs a name".into());
        }
        let (w, h) = self.sheet.mm();
        for (i, v) in self.views.iter().enumerate() {
            if self.views[..i].iter().any(|o| o.id == v.id) {
                return bad(format!("duplicate view id {}", v.id));
            }
            match &v.kind {
                ViewKind::Base {
                    bodies,
                    scale,
                    position,
                    assembly,
                    ..
                } => {
                    if bodies.is_empty() && !assembly {
                        return bad(format!("{} shows no body", v.id));
                    }
                    if !(scale.is_finite() && (MIN_SCALE..=MAX_SCALE).contains(scale)) {
                        return bad(format!(
                            "{} scale must be in {MIN_SCALE}..={MAX_SCALE}, got {scale}",
                            v.id
                        ));
                    }
                    if !(position[0].is_finite()
                        && position[1].is_finite()
                        && (0.0..=w).contains(&position[0])
                        && (0.0..=h).contains(&position[1]))
                    {
                        return bad(format!(
                            "{} position {position:?} is outside the {} sheet",
                            v.id,
                            self.sheet.label()
                        ));
                    }
                }
                ViewKind::Projected { parent, from } | ViewKind::Section { parent, from, .. } => {
                    let Some(pv) = self.views[..i].iter().find(|o| o.id == *parent) else {
                        return bad(format!(
                            "{} projects from {parent}, which is not an earlier view",
                            v.id
                        ));
                    };
                    if matches!(pv.kind, ViewKind::Section { .. } | ViewKind::Detail { .. }) {
                        return bad(format!(
                            "{} cannot be derived from the section/detail view {parent}",
                            v.id
                        ));
                    }
                    let section = matches!(v.kind, ViewKind::Section { .. });
                    if !section
                        && self.views[..i].iter().any(|o| {
                            matches!(o.kind, ViewKind::Projected { .. })
                                && o.kind.parent() == Some((*parent, *from))
                        })
                    {
                        return bad(format!("{parent} already has a view {}", from.label()));
                    }
                    if let ViewKind::Section { offset, label, .. } = &v.kind {
                        if !offset.is_finite() {
                            return bad(format!("{} section offset must be finite", v.id));
                        }
                        if label.is_empty()
                            || label.chars().count() > 3
                            || !label.chars().all(|c| c.is_ascii_alphanumeric())
                        {
                            return bad(format!(
                                "{} section label must be 1 to 3 letters/numbers, got '{label}'",
                                v.id
                            ));
                        }
                        if self.views[..i]
                            .iter()
                            .any(|o| o.kind.label() == Some(label.as_str()))
                        {
                            return bad(format!("section/detail {label} already exists"));
                        }
                    }
                }
                ViewKind::Detail {
                    parent,
                    center,
                    radius,
                    scale,
                    position,
                    label,
                } => {
                    let Some(pv) = self.views[..i].iter().find(|o| o.id == *parent) else {
                        return bad(format!(
                            "{} details {parent}, which is not an earlier view",
                            v.id
                        ));
                    };
                    if matches!(pv.kind, ViewKind::Section { .. } | ViewKind::Detail { .. }) {
                        return bad(format!(
                            "{} cannot be derived from the section/detail view {parent}",
                            v.id
                        ));
                    }
                    if !(center[0].is_finite() && center[1].is_finite()) {
                        return bad(format!("{} detail centre must be finite", v.id));
                    }
                    if !(radius.is_finite() && *radius > k::tol::MIN_LENGTH) {
                        return bad(format!(
                            "{} detail radius must be positive, got {radius}",
                            v.id
                        ));
                    }
                    if !(scale.is_finite() && (MIN_SCALE..=MAX_SCALE).contains(scale)) {
                        return bad(format!(
                            "{} scale must be in {MIN_SCALE}..={MAX_SCALE}, got {scale}",
                            v.id
                        ));
                    }
                    if !(position[0].is_finite()
                        && position[1].is_finite()
                        && (0.0..=w).contains(&position[0])
                        && (0.0..=h).contains(&position[1]))
                    {
                        return bad(format!(
                            "{} position {position:?} is outside the {} sheet",
                            v.id,
                            self.sheet.label()
                        ));
                    }
                    if label.is_empty()
                        || label.chars().count() > 3
                        || !label.chars().all(|c| c.is_ascii_alphanumeric())
                    {
                        return bad(format!(
                            "{} detail label must be 1 to 3 letters/numbers, got '{label}'",
                            v.id
                        ));
                    }
                    if self.views[..i]
                        .iter()
                        .any(|o| o.kind.label() == Some(label.as_str()))
                    {
                        return bad(format!("section/detail {label} already exists"));
                    }
                }
            }
            for a in &v.annotations {
                if let Annotation::Dimension { a, b, tolerance } = a {
                    tolerance
                        .validate()
                        .or_else(|e| bad(format!("{}: {e}", v.id)))?;
                    if b.is_none() && !matches!(a, DimRef::Hole(_)) {
                        return bad(format!(
                            "{}: a dimension with one reference must name a hole (diameter)",
                            v.id
                        ));
                    }
                    if b.as_ref() == Some(a) {
                        return bad(format!(
                            "{}: a dimension needs two different references",
                            v.id
                        ));
                    }
                }
            }
        }
        for (i, n) in self.notes.iter().enumerate() {
            if n.text.trim().is_empty() || n.text.chars().count() > 500 {
                return bad(format!("note {} must have 1 to 500 characters", i + 1));
            }
            if n.text.chars().any(|c| c.is_control() && c != '\n') {
                return bad(format!("note {} contains control characters", i + 1));
            }
            if !(n.position.iter().all(|x| x.is_finite())
                && (0.0..=w).contains(&n.position[0])
                && (0.0..=h).contains(&n.position[1]))
            {
                return bad(format!("note {} is outside the sheet", i + 1));
            }
        }
        if let Some(BomTable { position: Some(p) }) = &self.bom
            && !(p.iter().all(|x| x.is_finite())
                && (0.0..=w).contains(&p[0])
                && (0.0..=h).contains(&p[1]))
        {
            return bad("the BOM table position is outside the sheet".into());
        }
        Ok(())
    }

    /// Whether this drawing uses a run-7 (format v7) addition.
    pub fn v7_feature(&self) -> Option<&'static str> {
        if self
            .views
            .iter()
            .any(|v| matches!(v.kind, ViewKind::Base { assembly: true, .. }))
        {
            return Some("assembly drawing views");
        }
        if self.views.iter().any(|v| {
            v.annotations
                .iter()
                .any(|a| !matches!(a, Annotation::HoleCallout { .. }))
        }) {
            return Some("drawing dimensions and balloons");
        }
        if !self.notes.is_empty() {
            return Some("drawing notes");
        }
        if self.bom.is_some() {
            return Some("drawing BOM tables");
        }
        None
    }
}

impl Document {
    pub fn drawing(&self, id: DrawingId) -> Result<&Drawing> {
        self.drawings
            .iter()
            .find(|d| d.id == id)
            .ok_or_else(|| DocError::Validation(format!("unknown drawing {id}")))
    }
    pub fn drawing_mut(&mut self, id: DrawingId) -> Result<&mut Drawing> {
        self.drawings
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| DocError::Validation(format!("unknown drawing {id}")))
    }
    /// Next free drawing id.
    pub fn next_drawing_id(&self) -> DrawingId {
        DrawingId(self.drawings.iter().map(|d| d.id.0).max().unwrap_or(0) + 1)
    }
}

// ---------------------------------------------------------------- rendering

/// Bisection tolerance (model mm) on the position of a visible/hidden
/// transition along an edge.
pub const TRANSITION_TOL: f64 = 1.0e-4;

/// Chord tolerance (mm) of the occlusion tessellation for a part of bounding
/// diagonal `diag`: 2e-4 × diag, at least the kernel minimum.
pub fn hlr_chord(diag: f64) -> f64 {
    (diag * 2.0e-4)
        .max(k::tol::MIN_MESH_CHORD_ABS)
        .max(diag * k::tol::MIN_MESH_CHORD_REL * 2.0)
}

/// Sample spacing (model mm) along edges for the visibility test: diag/400,
/// at least 0.02 mm.
pub fn sample_spacing(diag: f64) -> f64 {
    (diag / 400.0).max(0.02)
}

/// How a view rendered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ViewStatus {
    Ok,
    /// Some or all of the view's bodies no longer exist (or are not
    /// evaluated): they are not drawn; the view shows a red flag.
    MissingBodies {
        bodies: Vec<BodyId>,
    },
    /// Bodies without exact edges (mesh bodies) are not drawn.
    Unsupported {
        bodies: Vec<BodyId>,
        reason: String,
    },
}

/// One dimension or callout as drawn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimensionReport {
    /// "horizontal", "vertical", "diameter", "hole_callout"; run 7: "distance",
    /// "hole_diameter" (user dimensions), "balloon" (value = item number).
    pub kind: String,
    /// Model value (mm); None for a dangling callout.
    pub value: Option<f64>,
    pub text: String,
    /// Why a callout could not be resolved (drawn in red, no value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dangling: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewReport {
    pub id: ViewId,
    pub label: String,
    /// Named direction of the view ("Front", …, "Iso").
    pub direction: String,
    pub scale: f64,
    /// Sheet position of the view's centre (mm).
    pub center: [f64; 2],
    #[serde(flatten)]
    pub status: ViewStatus,
    pub visible_curves: usize,
    pub hidden_curves: usize,
    pub dimensions: Vec<DimensionReport>,
}

/// A rendered drawing: the sheet to write, per-view reports and every problem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrawingRender {
    pub sheet: SheetPlot,
    pub views: Vec<ViewReport>,
    /// Flagged views and dangling annotations (empty: all references valid).
    pub issues: Vec<String>,
    /// Sampled-HLR parameters used (mm).
    pub sample_spacing: f64,
    pub chord: f64,
    /// Rows of the BOM table as drawn (empty without a table).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bom: Vec<BomRow>,
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: [f64; 3]) -> [f64; 3] {
    let l = dot(a, a).sqrt();
    a.map(|x| x / l)
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Per-body data reused by every view of one render.
struct BodyData {
    edges: Vec<EdgeCurve>,
    /// Smooth edges (seams, tangent edges) with their surface normals.
    smooth: Vec<(EdgeCurve, [[f64; 3]; 3])>,
    mesh: k::TriMesh,
    /// (origin, unit axis, radius) of cylindrical faces, for exact silhouettes.
    cylinders: Vec<([f64; 3], [f64; 3], f64)>,
    bounds: k::Bounds,
}

fn body_data(b: &k::Body, chord: f64) -> Result<BodyData> {
    let kerr = |e: k::KernelError| DocError::Validation(format!("drawing: {e}"));
    let all = k::drawing_edges(b).map_err(kerr)?;
    let smooth = all
        .iter()
        .filter(|e| e.smooth)
        .filter_map(|e| e.normals.map(|n| (e.curve.clone(), n)))
        .collect();
    let edges = all
        .into_iter()
        .filter(|e| !e.smooth)
        .map(|e| e.curve)
        .collect();
    let mesh = k::tessellate_display(b, chord).map_err(kerr)?;
    let cylinders = k::classify_faces(b)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| match f {
            k::DirectFace::Cylinder {
                origin,
                axis,
                radius,
                ..
            } => Some((origin, norm(axis), radius)),
            _ => None,
        })
        .collect();
    let bounds = k::bounds(b).map_err(kerr)?;
    Ok(BodyData {
        edges,
        smooth,
        mesh,
        cylinders,
        bounds,
    })
}

/// One occluding triangle: projected corners, depths, shared edges.
type OccTri = ([[f64; 2]; 3], [f64; 3], [bool; 3]);

/// Front-facing triangles in view coordinates with a uniform grid.
struct Occluder {
    /// (projected corners, depths, edge `i` (opposite corner `i`) shared
    /// with another front-facing triangle).
    tris: Vec<OccTri>,
    lo: [f64; 2],
    cell: f64,
    nx: usize,
    ny: usize,
    grid: Vec<Vec<u32>>,
    margin: f64,
    depth_eps: f64,
}

impl Occluder {
    fn new(meshes: &[&k::TriMesh], f: &ViewFrame, margin: f64, depth_eps: f64) -> Self {
        let mut tris = Vec::new();
        // Edges between two front-facing triangles are interior to the
        // occluding surface (a triangulation diagonal, or a convex edge
        // between two faces both seen from the front): a point near them is
        // still covered. Keyed by the exact 3D end points.
        type EdgeKey = ([u64; 3], [u64; 3]);
        let key = |a: [f64; 3], b: [f64; 3]| -> EdgeKey {
            let (a, b) = (a.map(f64::to_bits), b.map(f64::to_bits));
            if a <= b { (a, b) } else { (b, a) }
        };
        let mut corners: Vec<[[f64; 3]; 3]> = Vec::new();
        let mut count: HashMap<EdgeKey, u32> = HashMap::new();
        for m in meshes {
            for t in &m.triangles {
                let w = t.map(|i| m.positions[i as usize]);
                let p = w.map(|x| f.project(x));
                let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1])
                    - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
                if area > 1e-12 {
                    tris.push((p.map(|q| [q[0], q[1]]), p.map(|q| q[2]), [false; 3]));
                    corners.push(w);
                    for i in 0..3 {
                        *count
                            .entry(key(w[(i + 1) % 3], w[(i + 2) % 3]))
                            .or_default() += 1;
                    }
                }
            }
        }
        for (t, w) in tris.iter_mut().zip(&corners) {
            for i in 0..3 {
                t.2[i] = count
                    .get(&key(w[(i + 1) % 3], w[(i + 2) % 3]))
                    .is_some_and(|c| *c >= 2);
            }
        }
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for (p, _, _) in &tris {
            for q in p {
                for i in 0..2 {
                    lo[i] = lo[i].min(q[i]);
                    hi[i] = hi[i].max(q[i]);
                }
            }
        }
        if tris.is_empty() {
            lo = [0.0; 2];
            hi = [1.0; 2];
        }
        let n = ((tris.len() as f64).sqrt().ceil() as usize).clamp(1, 256);
        let cell = ((hi[0] - lo[0]).max(hi[1] - lo[1]) / n as f64).max(1e-9);
        let nx = (((hi[0] - lo[0]) / cell).ceil() as usize).max(1);
        let ny = (((hi[1] - lo[1]) / cell).ceil() as usize).max(1);
        let mut grid = vec![Vec::new(); nx * ny];
        for (ti, (p, _, _)) in tris.iter().enumerate() {
            let (mut a, mut b) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
            for q in p {
                for i in 0..2 {
                    a[i] = a[i].min(q[i]);
                    b[i] = b[i].max(q[i]);
                }
            }
            let ix = |x: f64, n: usize| (((x - lo[0]) / cell).floor().max(0.0) as usize).min(n - 1);
            let iy = |y: f64, n: usize| (((y - lo[1]) / cell).floor().max(0.0) as usize).min(n - 1);
            for gy in iy(a[1], ny)..=iy(b[1], ny) {
                for gx in ix(a[0], nx)..=ix(b[0], nx) {
                    grid[gy * nx + gx].push(ti as u32);
                }
            }
        }
        Occluder {
            tris,
            lo,
            cell,
            nx,
            ny,
            grid,
            margin,
            depth_eps,
        }
    }

    /// Is the view-space point `q` (x, y, depth) behind some triangle?
    fn hidden(&self, q: [f64; 3]) -> bool {
        let gx = ((q[0] - self.lo[0]) / self.cell).floor();
        let gy = ((q[1] - self.lo[1]) / self.cell).floor();
        if gx < 0.0 || gy < 0.0 || gx >= self.nx as f64 || gy >= self.ny as f64 {
            return false;
        }
        let cell = &self.grid[gy as usize * self.nx + gx as usize];
        cell.iter().any(|&ti| {
            let (p, z, shared) = &self.tris[ti as usize];
            // Signed distances to the three edges (counter-clockwise
            // triangle): strictly inside by the margin across outline edges;
            // across an edge shared with another front-facing triangle (a
            // triangulation diagonal) no margin: a point on such an edge is
            // covered by the surface, not near its outline.
            let mut bary = [0.0; 3];
            let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1])
                - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
            for i in 0..3 {
                let (a, b) = (p[(i + 1) % 3], p[(i + 2) % 3]);
                let e = [b[0] - a[0], b[1] - a[1]];
                let len = e[0].hypot(e[1]);
                let c = e[0] * (q[1] - a[1]) - e[1] * (q[0] - a[0]);
                // Across a shared edge the neighbour continues the surface:
                // inside (or on) this triangle is enough there.
                let inside = if shared[i] {
                    c / len >= 0.0
                } else {
                    c / len > self.margin
                };
                if !inside {
                    return false;
                }
                bary[i] = c / area;
            }
            let depth = bary[0] * z[0] + bary[1] * z[1] + bary[2] * z[2];
            depth > q[2] + self.depth_eps
        })
    }
}

/// A projected curve kind.
enum Proj {
    /// Collinear on the sheet: carrier through `o` with unit `d`.
    Line {
        o: [f64; 2],
        d: [f64; 2],
    },
    /// End-on circle: centre, radius, angle at s = 0, signed sweep.
    Circle {
        c: [f64; 2],
        r: f64,
        a0: f64,
        sweep: f64,
    },
    Curve,
}

/// A parameter interval.
type Span = (f64, f64);
/// A parameter interval and whether it is visible.
type Run = (f64, f64, bool);

/// Visible / hidden pieces collected per carrier.
#[derive(Default)]
struct Pieces {
    /// (origin, dir, intervals[(t0, t1, visible)])
    lines: Vec<([f64; 2], [f64; 2], Vec<Run>)>,
    /// (centre, r, angular intervals in [0, 2π): (a0, a1, visible))
    circles: Vec<([f64; 2], f64, Vec<Run>)>,
    curves: Vec<(Vec<[f64; 2]>, bool)>,
    tol: f64,
}

impl Pieces {
    fn line(&mut self, o: [f64; 2], d: [f64; 2], t0: f64, t1: f64, vis: bool) {
        let tol = self.tol;
        let (a, b) = (
            [o[0] + d[0] * t0, o[1] + d[1] * t0],
            [o[0] + d[0] * t1, o[1] + d[1] * t1],
        );
        // Canonical direction: angle in [0, π).
        let d = if d[1] < -1e-15 || (d[1].abs() <= 1e-15 && d[0] < 0.0) {
            [-d[0], -d[1]]
        } else {
            d
        };
        for (lo, ld, iv) in &mut self.lines {
            let par = (ld[0] * d[1] - ld[1] * d[0]).abs() < 1e-9;
            let off = |p: [f64; 2]| ((p[0] - lo[0]) * ld[1] - (p[1] - lo[1]) * ld[0]).abs();
            if par && off(a) <= tol && off(b) <= tol {
                let t = |p: [f64; 2]| (p[0] - lo[0]) * ld[0] + (p[1] - lo[1]) * ld[1];
                let (u0, u1) = (t(a).min(t(b)), t(a).max(t(b)));
                iv.push((u0, u1, vis));
                return;
            }
        }
        // New carrier, origin at the foot of the perpendicular from (0, 0).
        let s = a[0] * d[0] + a[1] * d[1];
        let lo = [a[0] - d[0] * s, a[1] - d[1] * s];
        let t = |p: [f64; 2]| (p[0] - lo[0]) * d[0] + (p[1] - lo[1]) * d[1];
        self.lines
            .push((lo, d, vec![(t(a).min(t(b)), t(a).max(t(b)), vis)]));
    }

    fn arc(&mut self, c: [f64; 2], r: f64, a0: f64, a1: f64, vis: bool) {
        let tol = self.tol;
        // Normalised counter-clockwise interval(s) in [0, 2π).
        let (lo, hi) = (a0.min(a1), a0.max(a1));
        let mut ivs = Vec::new();
        if hi - lo >= TAU - 1e-12 {
            ivs.push((0.0, TAU));
        } else {
            let s = lo.rem_euclid(TAU);
            let e = s + (hi - lo);
            if e > TAU {
                ivs.push((s, TAU));
                ivs.push((0.0, e - TAU));
            } else {
                ivs.push((s, e));
            }
        }
        for (cc, cr, iv) in &mut self.circles {
            if (cc[0] - c[0]).hypot(cc[1] - c[1]) <= tol && (*cr - r).abs() <= tol {
                iv.extend(ivs.iter().map(|&(a, b)| (a, b, vis)));
                return;
            }
        }
        self.circles
            .push((c, r, ivs.into_iter().map(|(a, b)| (a, b, vis)).collect()));
    }
}

/// Union of intervals (gaps below `tol` closed).
fn union(mut iv: Vec<(f64, f64)>, tol: f64) -> Vec<(f64, f64)> {
    iv.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (a, b) in iv {
        match out.last_mut() {
            Some(l) if a <= l.1 + tol => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// `iv` minus the union `cut`.
fn subtract(iv: &[(f64, f64)], cut: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for &(a, b) in iv {
        let mut pieces = vec![(a, b)];
        for &(c, d) in cut {
            pieces = pieces
                .into_iter()
                .flat_map(|(x, y)| {
                    let mut v = Vec::new();
                    if c > x {
                        v.push((x, y.min(c)));
                    }
                    if d < y {
                        v.push((x.max(d), y));
                    }
                    v.into_iter().filter(|(p, q)| q > p)
                })
                .collect();
        }
        out.extend(pieces);
    }
    out
}

/// Split intervals into (visible union, hidden minus visible).
fn resolve_intervals(iv: &[Run], tol: f64) -> (Vec<Span>, Vec<Span>) {
    let vis = union(iv.iter().filter(|x| x.2).map(|x| (x.0, x.1)).collect(), tol);
    let hid = union(
        iv.iter().filter(|x| !x.2).map(|x| (x.0, x.1)).collect(),
        tol,
    );
    let hid = subtract(&hid, &vis)
        .into_iter()
        .filter(|(a, b)| b - a > tol)
        .collect();
    (vis, hid)
}

/// Geometry of one view in view coordinates (model mm), before placement.
#[derive(Default)]
struct ViewGeom {
    /// (prim in view mm, visible)
    prims: Vec<(Prim, bool)>,
    /// Visible full circles: (centre, r).
    circles: Vec<([f64; 2], f64)>,
    lo: [f64; 2],
    hi: [f64; 2],
}

/// Project `curve`, classify its samples, and add the pieces.
#[allow(clippy::too_many_arguments)]
fn add_curve(
    out: &mut Pieces,
    curve: &EdgeCurve,
    f: &ViewFrame,
    occ: &Occluder,
    spacing: f64,
    lo: &mut [f64; 2],
    hi: &mut [f64; 2],
) {
    let len = curve.length();
    if len <= 0.0 {
        return;
    }
    let n = ((len / spacing).ceil() as usize).clamp(2, 4000);
    let pt = |s: f64| f.project(curve.point(s));
    let vis = |s: f64| !occ.hidden(pt(s));
    let samples: Vec<[f64; 3]> = (0..=n).map(|i| pt(i as f64 / n as f64)).collect();
    for q in &samples {
        for i in 0..2 {
            lo[i] = lo[i].min(q[i]);
            hi[i] = hi[i].max(q[i]);
        }
    }
    // Projected kind.
    let a = samples[0];
    let (mut far, mut fd) = (a, 0.0);
    for q in &samples {
        let d = (q[0] - a[0]).hypot(q[1] - a[1]);
        if d > fd {
            (far, fd) = (*q, d);
        }
    }
    if fd <= out.tol {
        return; // seen end-on: a point
    }
    let dir = [(far[0] - a[0]) / fd, (far[1] - a[1]) / fd];
    let collinear = samples
        .iter()
        .all(|q| ((q[0] - a[0]) * dir[1] - (q[1] - a[1]) * dir[0]).abs() <= out.tol);
    let kind = if collinear {
        Proj::Line {
            o: [a[0], a[1]],
            d: dir,
        }
    } else if let EdgeCurve::Arc {
        center,
        normal,
        radius,
        start,
        sweep,
    } = curve
    {
        let nz = dot(*normal, f.toward);
        if nz.abs() > 1.0 - 1e-9 {
            let c = f.project(*center);
            let s = f.project(*start);
            Proj::Circle {
                c: [c[0], c[1]],
                r: *radius,
                a0: (s[1] - c[1]).atan2(s[0] - c[0]),
                sweep: sweep * nz.signum(),
            }
        } else {
            Proj::Curve
        }
    } else {
        Proj::Curve
    };
    // Runs of equal visibility with refined transitions.
    let flags: Vec<bool> = (0..=n).map(|i| vis(i as f64 / n as f64)).collect();
    let mut runs: Vec<(f64, f64, bool)> = Vec::new();
    let mut start = 0.0;
    let ds = TRANSITION_TOL / len;
    for i in 0..n {
        if flags[i] != flags[i + 1] {
            let (mut x0, mut x1) = (i as f64 / n as f64, (i + 1) as f64 / n as f64);
            while x1 - x0 > ds {
                let m = 0.5 * (x0 + x1);
                if vis(m) == flags[i] {
                    x0 = m;
                } else {
                    x1 = m;
                }
            }
            let t = 0.5 * (x0 + x1);
            runs.push((start, t, flags[i]));
            start = t;
        }
    }
    runs.push((start, 1.0, flags[n]));
    // Runs shorter than the occlusion margin zone are artefacts of the
    // margin around triangle borders (an edge ending on an occluding face's
    // outline): they take the visibility of their longer neighbour.
    let short = 2.0 * occ.margin / len;
    while runs.len() > 1 {
        let Some(i) = (0..runs.len()).find(|&i| runs[i].1 - runs[i].0 < short) else {
            break;
        };
        let (s0, s1, _) = runs.remove(i);
        let j = if i == 0 {
            0
        } else if i >= runs.len() {
            runs.len() - 1
        } else if runs[i - 1].1 - runs[i - 1].0 >= runs[i].1 - runs[i].0 {
            i - 1
        } else {
            i
        };
        runs[j].0 = runs[j].0.min(s0);
        runs[j].1 = runs[j].1.max(s1);
        let mut k = 0;
        while k + 1 < runs.len() {
            if runs[k].2 == runs[k + 1].2 {
                runs[k].1 = runs[k + 1].1;
                runs.remove(k + 1);
            } else {
                k += 1;
            }
        }
    }
    for (s0, s1, v) in runs {
        match kind {
            Proj::Line { o, d } => {
                // Extent of the run along the carrier (folded curves seen
                // edge-on cover [min, max]).
                let m = ((s1 - s0) * n as f64).ceil().max(1.0) as usize;
                let (mut t0, mut t1) = (f64::INFINITY, f64::NEG_INFINITY);
                for j in 0..=m {
                    let q = pt(s0 + (s1 - s0) * j as f64 / m as f64);
                    let t = (q[0] - o[0]) * d[0] + (q[1] - o[1]) * d[1];
                    t0 = t0.min(t);
                    t1 = t1.max(t);
                }
                if t1 - t0 > out.tol {
                    out.line(o, d, t0, t1, v);
                }
            }
            Proj::Circle { c, r, a0, sweep } => {
                out.arc(c, r, a0 + sweep * s0, a0 + sweep * s1, v);
            }
            Proj::Curve => {
                let m = ((s1 - s0) * n as f64).ceil().max(1.0) as usize;
                let pts: Vec<[f64; 2]> = (0..=m)
                    .map(|j| {
                        let q = pt(s0 + (s1 - s0) * j as f64 / m as f64);
                        [q[0], q[1]]
                    })
                    .collect();
                out.curves.push((pts, v));
            }
        }
    }
}

/// Silhouette segments of curved faces: mesh edges inside one B-rep face
/// between a front- and a back-facing triangle; on a cylinder whose axis is
/// perpendicular to the view, snapped to the exact generator.
fn silhouettes(d: &BodyData, f: &ViewFrame, chord: f64) -> Vec<EdgeCurve> {
    let m = &d.mesh;
    let facing: Vec<f64> = m
        .triangles
        .iter()
        .map(|t| {
            let p = t.map(|i| m.positions[i as usize]);
            let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
            let l = dot(n, n).sqrt();
            if l <= 0.0 { 0.0 } else { dot(n, f.toward) / l }
        })
        .collect();
    let mut edges: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
    for (ti, t) in m.triangles.iter().enumerate() {
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            edges.entry((a.min(b), a.max(b))).or_default().push(ti);
        }
    }
    const EDGE_ON: f64 = 1e-6;
    let mut out = Vec::new();
    for ((a, b), ts) in edges {
        let [t0, t1] = ts[..] else { continue };
        if m.tri_face[t0] != m.tri_face[t1] {
            continue;
        }
        let (f0, f1) = (facing[t0], facing[t1]);
        if !((f0 > EDGE_ON && f1 < -EDGE_ON) || (f0 < -EDGE_ON && f1 > EDGE_ON)) {
            continue;
        }
        let mut p = m.positions[a as usize];
        let mut q = m.positions[b as usize];
        // Exact cylinder generator.
        for (o, ax, r) in &d.cylinders {
            if dot(*ax, f.toward).abs() > 1e-9 {
                continue;
            }
            let foot = |x: [f64; 3]| {
                let t = dot(sub(x, *o), *ax);
                [o[0] + ax[0] * t, o[1] + ax[1] * t, o[2] + ax[2] * t]
            };
            let dist = |x: [f64; 3]| {
                let v = sub(x, foot(x));
                dot(v, v).sqrt()
            };
            let near = 3.0 * chord + 1e-6;
            if (dist(p) - r).abs() <= near && (dist(q) - r).abs() <= near {
                let s = norm(cross(*ax, f.toward));
                let snap = |x: [f64; 3]| {
                    let fo = foot(x);
                    let sg = dot(sub(x, fo), s).signum();
                    std::array::from_fn(|i| fo[i] + s[i] * r * sg)
                };
                (p, q) = (snap(p), snap(q));
                break;
            }
        }
        out.push(EdgeCurve::Line { a: p, b: q });
    }
    out
}

fn view_geometry(
    datas: &[&BodyData],
    f: &ViewFrame,
    chord: f64,
    spacing: f64,
    diag: f64,
) -> ViewGeom {
    let meshes: Vec<&k::TriMesh> = datas.iter().map(|d| &d.mesh).collect();
    let tol = 1e-6 * diag.max(1.0);
    let occ = Occluder::new(&meshes, f, 1.5 * chord + tol, 2.0 * chord + tol);
    let mut pieces = Pieces {
        tol: tol.max(1e-6),
        ..Default::default()
    };
    let mut g = ViewGeom {
        lo: [f64::INFINITY; 2],
        hi: [f64::NEG_INFINITY; 2],
        ..Default::default()
    };
    for d in datas {
        // Smooth edges are drawn only where they are silhouettes (the surface
        // normal along them is perpendicular to the view: seams and tangent
        // edges lying exactly on the outline).
        let smooth_sil = d.smooth.iter().filter_map(|(c, n)| {
            n.iter()
                .all(|n| dot(*n, f.toward).abs() <= 1e-9)
                .then_some(c)
        });
        let mesh_sil = silhouettes(d, f, chord);
        for e in d.edges.iter().chain(smooth_sil).chain(mesh_sil.iter()) {
            add_curve(&mut pieces, e, f, &occ, spacing, &mut g.lo, &mut g.hi);
        }
    }
    let ptol = pieces.tol;
    for (o, d, iv) in &pieces.lines {
        let (vis, hid) = resolve_intervals(iv, ptol);
        for (list, v) in [(vis, true), (hid, false)] {
            for (a, b) in list {
                if b - a <= 10.0 * ptol {
                    continue;
                }
                g.prims.push((
                    Prim::Line {
                        a: [o[0] + d[0] * a, o[1] + d[1] * a],
                        b: [o[0] + d[0] * b, o[1] + d[1] * b],
                        style: if v {
                            LineStyle::Visible
                        } else {
                            LineStyle::Hidden
                        },
                    },
                    v,
                ));
            }
        }
    }
    for (c, r, iv) in &pieces.circles {
        let atol = ptol / r.max(1e-9);
        let (vis, hid) = resolve_intervals(iv, atol);
        for (list, v) in [(vis, true), (hid, false)] {
            let mut list = list;
            // Join across angle 0.
            if list.len() >= 2
                && list[0].0 <= atol
                && list.last().is_some_and(|l| l.1 >= TAU - atol)
            {
                let first = list.remove(0);
                if let Some(l) = list.last_mut() {
                    l.1 = TAU + first.1;
                }
            }
            for (a, b) in list {
                if (b - a) * r <= 10.0 * ptol {
                    continue;
                }
                let full = b - a >= TAU - atol;
                if full && v {
                    g.circles.push((*c, *r));
                }
                g.prims.push((
                    Prim::Arc {
                        c: *c,
                        r: *r,
                        start: if full { 0.0 } else { a },
                        sweep: if full { TAU } else { b - a },
                        style: if v {
                            LineStyle::Visible
                        } else {
                            LineStyle::Hidden
                        },
                    },
                    v,
                ));
            }
        }
    }
    for (pts, v) in &pieces.curves {
        g.prims.push((
            Prim::Polyline {
                pts: pts.clone(),
                style: if *v {
                    LineStyle::Visible
                } else {
                    LineStyle::Hidden
                },
            },
            *v,
        ));
    }
    if !g.lo[0].is_finite() {
        g.lo = [0.0; 2];
        g.hi = [0.0; 2];
    }
    g
}

/// Format a model length for a dimension: up to 2 decimals, trailing zeros
/// removed.
pub fn fmt_len(x: f64) -> String {
    let s = format!("{:.2}", x + 0.0);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

/// "1:2", "2:1", "1:1", "1:2.5".
pub fn fmt_scale(s: f64) -> String {
    if s >= 1.0 {
        format!("{}:1", fmt_len(s))
    } else {
        format!("1:{}", fmt_len(1.0 / s))
    }
}

/// Largest standard scale at which an extent `ext` (mm) spans at most
/// `room` sheet mm.
pub fn fit_scale(ext: f64, room: f64) -> f64 {
    const STD: [f64; 13] = [
        10.0, 5.0, 2.0, 1.0, 0.5, 0.2, 0.1, 0.05, 0.02, 0.01, 0.005, 0.002, 0.001,
    ];
    STD.into_iter()
        .find(|s| ext * s <= room)
        .unwrap_or(MIN_SCALE)
}

const TEXT: f64 = 3.5;
const ARROW: f64 = 2.5;

fn arrow(tip: [f64; 2], dir: [f64; 2]) -> Prim {
    // `dir`: unit direction the arrow points to.
    let (w, l) = (ARROW / 6.0, ARROW);
    let base = [tip[0] - dir[0] * l, tip[1] - dir[1] * l];
    let n = [-dir[1], dir[0]];
    Prim::Polygon {
        pts: vec![
            tip,
            [base[0] + n[0] * w, base[1] + n[1] * w],
            [base[0] - n[0] * w, base[1] - n[1] * w],
        ],
        fill: true,
        style: LineStyle::Thin,
    }
}

/// Linear dimension between sheet points `p0`, `p1` measured along unit
/// `along`, with the dimension line offset by `off` along unit `normal`.
fn linear_dim(
    out: &mut Vec<Prim>,
    p0: [f64; 2],
    p1: [f64; 2],
    normal: [f64; 2],
    off: f64,
    text: &str,
) {
    let q0 = [p0[0] + normal[0] * off, p0[1] + normal[1] * off];
    let q1 = [p1[0] + normal[0] * off, p1[1] + normal[1] * off];
    let ext = 2.0;
    for (p, q) in [(p0, q0), (p1, q1)] {
        out.push(Prim::Line {
            a: [p[0] + normal[0] * 1.5, p[1] + normal[1] * 1.5],
            b: [q[0] + normal[0] * ext, q[1] + normal[1] * ext],
            style: LineStyle::Thin,
        });
    }
    out.push(Prim::Line {
        a: q0,
        b: q1,
        style: LineStyle::Thin,
    });
    let l = (q1[0] - q0[0]).hypot(q1[1] - q0[1]).max(1e-9);
    let d = [(q1[0] - q0[0]) / l, (q1[1] - q0[1]) / l];
    out.push(arrow(q0, [-d[0], -d[1]]));
    out.push(arrow(q1, d));
    let mid = [(q0[0] + q1[0]) / 2.0, (q0[1] + q1[1]) / 2.0];
    // Text beside the line: below a horizontal dimension placed under the
    // view would collide with the arrows, so it sits on the outer side.
    let at = if normal[1] < 0.0 {
        [mid[0], mid[1] - 1.0 - TEXT]
    } else if normal[0] > 0.0 {
        [mid[0] + 1.0, mid[1] - TEXT / 2.0]
    } else if normal[0] < 0.0 {
        [mid[0] - 1.0, mid[1] - TEXT / 2.0]
    } else {
        [mid[0], mid[1] + 1.0]
    };
    out.push(Prim::Text {
        at,
        size: TEXT,
        text: text.to_string(),
        anchor: if normal[0] > 0.0 {
            Anchor::Start
        } else if normal[0] < 0.0 {
            Anchor::End
        } else {
            Anchor::Middle
        },
        style: LineStyle::Thin,
    });
}

fn diameter_callout(out: &mut Vec<Prim>, c: [f64; 2], r: f64, text: &str, style: LineStyle) {
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let tip = [c[0] + r * s, c[1] + r * s];
    let knee = [tip[0] + 5.0, tip[1] + 5.0];
    let end = [knee[0] + 3.0, knee[1]];
    out.push(arrow(tip, [-s, -s]));
    out.push(Prim::Polyline {
        pts: vec![tip, knee, end],
        style,
    });
    out.push(Prim::Text {
        at: [end[0] + 0.8, end[1] - TEXT / 3.0],
        size: TEXT,
        text: text.to_string(),
        anchor: Anchor::Start,
        style,
    });
}

/// Canonical primitive order for deterministic output.
fn sort_key(p: &Prim) -> (u8, LineStyle, Vec<i64>) {
    let q = |x: f64| (x * 1e3).round() as i64;
    match p {
        Prim::Line { a, b, style } => {
            let (a, b) = if (q(a[0]), q(a[1])) <= (q(b[0]), q(b[1])) {
                (a, b)
            } else {
                (b, a)
            };
            (0, *style, vec![q(a[0]), q(a[1]), q(b[0]), q(b[1])])
        }
        Prim::Arc {
            c,
            r,
            start,
            sweep,
            style,
        } => (
            1,
            *style,
            vec![q(c[0]), q(c[1]), q(*r), q(*start), q(*sweep)],
        ),
        Prim::Polyline { pts, style } => (
            2,
            *style,
            pts.iter().flat_map(|p| [q(p[0]), q(p[1])]).collect(),
        ),
        Prim::Polygon { pts, style, .. } => (
            3,
            *style,
            pts.iter().flat_map(|p| [q(p[0]), q(p[1])]).collect(),
        ),
        Prim::Text { at, style, .. } => (4, *style, vec![q(at[0]), q(at[1])]),
    }
}

/// Map a view-space primitive to the sheet.
fn place(p: &Prim, s: f64, off: [f64; 2]) -> Prim {
    let m = |q: [f64; 2]| [off[0] + q[0] * s, off[1] + q[1] * s];
    match p {
        Prim::Line { a, b, style } => Prim::Line {
            a: m(*a),
            b: m(*b),
            style: *style,
        },
        Prim::Arc {
            c,
            r,
            start,
            sweep,
            style,
        } => Prim::Arc {
            c: m(*c),
            r: r * s,
            start: *start,
            sweep: *sweep,
            style: *style,
        },
        Prim::Polyline { pts, style } => Prim::Polyline {
            pts: pts.iter().map(|q| m(*q)).collect(),
            style: *style,
        },
        other => other.clone(),
    }
}

struct Placed {
    /// Sheet bounding box of the view.
    lo: [f64; 2],
    hi: [f64; 2],
    /// Sheet = off + scale * view coordinates.
    off: [f64; 2],
    scale: f64,
    frame: ViewFrame,
}

/// Clip a view's curves to the circle about `c` of radius `r` (detail views):
/// lines exactly, arcs by the exact circle-circle intersection angles,
/// polylines segment by segment. Only full circles lying inside stay
/// "end-on circles" (centre marks).
fn clip_to_circle(g: &mut ViewGeom, c: [f64; 2], r: f64) {
    let seg = |a: [f64; 2], b: [f64; 2]| -> Option<([f64; 2], [f64; 2])> {
        let d = [b[0] - a[0], b[1] - a[1]];
        let f = [a[0] - c[0], a[1] - c[1]];
        let qa = d[0] * d[0] + d[1] * d[1];
        if qa <= 0.0 {
            return None;
        }
        let qb = 2.0 * (f[0] * d[0] + f[1] * d[1]);
        let qc = f[0] * f[0] + f[1] * f[1] - r * r;
        let disc = qb * qb - 4.0 * qa * qc;
        if disc <= 0.0 {
            return None;
        }
        let s = disc.sqrt();
        let (t0, t1) = (
            ((-qb - s) / (2.0 * qa)).max(0.0),
            ((-qb + s) / (2.0 * qa)).min(1.0),
        );
        (t1 > t0 + 1e-12).then(|| {
            let p = |t: f64| [a[0] + d[0] * t, a[1] + d[1] * t];
            (p(t0), p(t1))
        })
    };
    let mut out = Vec::new();
    for (p, vis) in std::mem::take(&mut g.prims) {
        match p {
            Prim::Line { a, b, style } => {
                if let Some((a, b)) = seg(a, b) {
                    out.push((Prim::Line { a, b, style }, vis));
                }
            }
            Prim::Polyline { pts, style } => {
                for w in pts.windows(2) {
                    if let Some((a, b)) = seg(w[0], w[1]) {
                        out.push((Prim::Line { a, b, style }, vis));
                    }
                }
            }
            Prim::Arc {
                c: p0,
                r: q,
                start,
                sweep,
                style,
            } => {
                let d = (p0[0] - c[0]).hypot(p0[1] - c[1]);
                if d + q <= r {
                    out.push((
                        Prim::Arc {
                            c: p0,
                            r: q,
                            start,
                            sweep,
                            style,
                        },
                        vis,
                    ));
                    continue;
                }
                if d >= r + q || d + r <= q {
                    continue;
                }
                let phi = (c[1] - p0[1]).atan2(c[0] - p0[0]);
                let alpha = ((q * q + d * d - r * r) / (2.0 * q * d))
                    .clamp(-1.0, 1.0)
                    .acos();
                let (a0, a1) = (start, start + sweep);
                for kk in -1..=2 {
                    let w0 = phi - alpha + TAU * kk as f64;
                    let w1 = phi + alpha + TAU * kk as f64;
                    let (s0, s1) = (a0.max(w0), a1.min(w1));
                    if s1 > s0 + 1e-12 {
                        out.push((
                            Prim::Arc {
                                c: p0,
                                r: q,
                                start: s0,
                                sweep: s1 - s0,
                                style,
                            },
                            vis,
                        ));
                    }
                }
            }
            other => out.push((other, vis)),
        }
    }
    g.prims = out;
    g.circles
        .retain(|(p, q)| (p[0] - c[0]).hypot(p[1] - c[1]) + q <= r);
    g.lo = [c[0] - r, c[1] - r];
    g.hi = [c[0] + r, c[1] + r];
}

/// Sheet spacing (mm) of section hatch lines (45°).
pub const HATCH_SPACING: f64 = 2.5;

/// 45° hatch segments (view coordinates) over the faces of `datas` lying in
/// `plane` and facing the viewer (outward normal = plane normal): their
/// tessellation triangles, each hatch line clipped to them and the pieces
/// united, so the hatch follows the face boundaries to the chord tolerance.
fn hatch(
    datas: &[&BodyData],
    plane: &k::Frame,
    f: &ViewFrame,
    spacing: f64,
    tol: f64,
) -> Vec<([f64; 2], [f64; 2])> {
    let s2 = std::f64::consts::FRAC_1_SQRT_2;
    let (u, n) = ([s2, s2], [-s2, s2]);
    let mut lines: BTreeMap<i64, Vec<Span>> = BTreeMap::new();
    for d in datas {
        let m = &d.mesh;
        for t in &m.triangles {
            let p = t.map(|i| m.positions[i as usize]);
            if p.iter()
                .any(|q| dot(sub(*q, plane.origin), plane.z).abs() > tol)
            {
                continue;
            }
            let nn = cross(sub(p[1], p[0]), sub(p[2], p[0]));
            let l = dot(nn, nn).sqrt();
            if l <= 0.0 || dot(nn, plane.z) / l < 0.999 {
                continue;
            }
            let q = p.map(|x| {
                let v = f.project(x);
                [v[0], v[1]]
            });
            let sv = q.map(|x| x[0] * n[0] + x[1] * n[1]);
            let tv = q.map(|x| x[0] * u[0] + x[1] * u[1]);
            let (lo, hi) = (
                sv.iter().copied().fold(f64::INFINITY, f64::min),
                sv.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            );
            let k0 = (lo / spacing).ceil() as i64;
            let k1 = (hi / spacing).floor() as i64;
            for kk in k0..=k1 {
                let s = kk as f64 * spacing;
                let mut ts = Vec::new();
                for i in 0..3 {
                    let (a, b) = (i, (i + 1) % 3);
                    let (da, db) = (sv[a] - s, sv[b] - s);
                    if (da <= 0.0 && db >= 0.0) || (da >= 0.0 && db <= 0.0) {
                        if (db - da).abs() < 1e-15 {
                            ts.push(tv[a]);
                            ts.push(tv[b]);
                        } else {
                            let w = da / (da - db);
                            ts.push(tv[a] + (tv[b] - tv[a]) * w);
                        }
                    }
                }
                if ts.len() >= 2 {
                    let t0 = ts.iter().copied().fold(f64::INFINITY, f64::min);
                    let t1 = ts.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    if t1 > t0 {
                        lines.entry(kk).or_default().push((t0, t1));
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for (kk, iv) in lines {
        let s = kk as f64 * spacing;
        for (t0, t1) in union(iv, 1e-9) {
            if t1 - t0 > 1e-6 {
                let at = |t: f64| [n[0] * s + u[0] * t, n[1] * s + u[1] * t];
                out.push((at(t0), at(t1)));
            }
        }
    }
    out
}

/// The cutting line of section `label` on its parent view `p`, with arrows
/// in the direction of sight and the label at both ends.
fn cutting_line(
    parent: ViewId,
    from: Side,
    offset: f64,
    label: &str,
    p: &Placed,
    section: &ViewFrame,
) -> Group {
    let look = section.toward.map(|x| -x);
    let dir = [dot(look, p.frame.right), dot(look, p.frame.up)];
    let ext = 6.0;
    let (a, b) = match from {
        Side::Above | Side::Below => {
            let y = p.off[1] + offset * p.scale;
            ([p.lo[0] - ext, y], [p.hi[0] + ext, y])
        }
        Side::Left | Side::Right => {
            let x = p.off[0] + offset * p.scale;
            ([x, p.lo[1] - ext], [x, p.hi[1] + ext])
        }
    };
    let mut prims = vec![Prim::Line {
        a,
        b,
        style: LineStyle::Center,
    }];
    for e in [a, b] {
        // Thick end strokes, arrow, letter.
        let along = [b[0] - a[0], b[1] - a[1]];
        let l = along[0].hypot(along[1]).max(1e-9);
        let along = [along[0] / l, along[1] / l];
        let inward = if e == a { 1.0 } else { -1.0 };
        prims.push(Prim::Line {
            a: e,
            b: [
                e[0] + along[0] * 4.0 * inward,
                e[1] + along[1] * 4.0 * inward,
            ],
            style: LineStyle::Visible,
        });
        let tip = [e[0] + dir[0] * 7.0, e[1] + dir[1] * 7.0];
        prims.push(Prim::Line {
            a: e,
            b: tip,
            style: LineStyle::Thin,
        });
        prims.push(arrow(tip, dir));
        prims.push(Prim::Text {
            at: [
                tip[0] + dir[0] * 2.0,
                tip[1] + dir[1] * 2.0 - if dir[1] < 0.0 { TEXT } else { 0.0 },
            ],
            size: TEXT,
            text: label.to_string(),
            anchor: Anchor::Middle,
            style: LineStyle::Thin,
        });
    }
    Group {
        id: format!("cut-{label}"),
        label: format!("Cutting line {label}-{label} on {parent}"),
        prims,
    }
}

fn flag_box(prims: &mut Vec<Prim>, c: [f64; 2], msg: &str) -> ([f64; 2], [f64; 2]) {
    let (w, h) = (60.0, 16.0);
    let lo = [c[0] - w / 2.0, c[1] - h / 2.0];
    let hi = [c[0] + w / 2.0, c[1] + h / 2.0];
    prims.push(Prim::Polygon {
        pts: vec![lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]],
        fill: false,
        style: LineStyle::Flag,
    });
    prims.push(Prim::Line {
        a: lo,
        b: hi,
        style: LineStyle::Flag,
    });
    prims.push(Prim::Text {
        at: [c[0], hi[1] + 1.5],
        size: 2.5,
        text: msg.to_string(),
        anchor: Anchor::Middle,
        style: LineStyle::Flag,
    });
    (lo, hi)
}

/// Render drawing `id` from the current evaluation (see the module docs).
pub fn render(doc: &Document, eval: &Evaluation, id: DrawingId) -> Result<DrawingRender> {
    let dr = doc.drawing(id)?;
    dr.validate()?;
    let (w, h) = dr.sheet.mm();
    // Everything drawn anywhere in the drawing (definition bodies and placed
    // occurrence bodies), with one shared HLR resolution.
    let mut used_keys: Vec<Key> = Vec::new();
    for v in &dr.views {
        if matches!(v.kind, ViewKind::Base { .. }) {
            for key in view_keys(dr, v.id, eval)? {
                if !used_keys.contains(&key) {
                    used_keys.push(key);
                }
            }
        }
    }
    let mut used: Vec<BodyId> = Vec::new();
    for (_, b) in &used_keys {
        if !used.contains(b) {
            used.push(*b);
        }
    }
    let mut diag: f64 = 0.0;
    let mut breps: BTreeMap<Key, k::Body> = BTreeMap::new();
    let mut unsupported: Vec<BodyId> = Vec::new();
    for key in &used_keys {
        let Some(eb) = eval.body_by_id(key.1) else {
            continue;
        };
        match eb.geometry.brep() {
            Some(body) => {
                let body = match key.0 {
                    0 => body.clone(),
                    i => {
                        let t = &eval.occurrences[i - 1].transform;
                        k::transform_body(body, t)
                            .map_err(|e| DocError::Validation(format!("drawing: {e}")))?
                    }
                };
                let bb =
                    k::bounds(&body).map_err(|e| DocError::Validation(format!("drawing: {e}")))?;
                let d = (0..3)
                    .map(|i| (bb.max[i] - bb.min[i]).powi(2))
                    .sum::<f64>()
                    .sqrt();
                diag = diag.max(d);
                breps.insert(*key, body);
            }
            None => {
                if !unsupported.contains(&key.1) {
                    unsupported.push(key.1)
                }
            }
        }
    }
    let diag = diag.max(1.0);
    let chord = hlr_chord(diag);
    let spacing = sample_spacing(diag);
    let mut data: HashMap<Key, BodyData> = HashMap::new();
    for (key, body) in &breps {
        data.insert(*key, body_data(body, chord)?);
    }
    let bom = bom_rows(doc, eval);

    let mut groups: Vec<Group> = Vec::new();
    let mut issues: Vec<String> = Vec::new();
    let mut reports: Vec<ViewReport> = Vec::new();
    let mut placed: BTreeMap<ViewId, Placed> = BTreeMap::new();
    let mut dimensioned = [false; 3];
    // Balloons are laid out after every view is placed (they must avoid the
    // other views): (view, anchors, view box, clearance left/right/below).
    #[allow(clippy::type_complexity)]
    let mut pending_balloons: Vec<(
        ViewId,
        Vec<(usize, [f64; 2])>,
        [f64; 2],
        [f64; 2],
        [f64; 3],
    )> = Vec::new();

    for v in &dr.views {
        let (frame, scale, base) = dr.resolve(v.id)?;
        let bodies = dr.bodies(v.id)?;
        let keys = view_keys(dr, v.id, eval)?;
        let assembly_view = keys.iter().any(|k| k.0 != 0);
        // Review run 7 #15: occurrence bodies that are not evaluated
        // (consumed, suppressed, rolled back) are reported, not skipped.
        let mut missing_occ: Vec<BodyId> = keys
            .iter()
            .filter(|k| k.0 != 0 && eval.body_by_id(k.1).is_none())
            .map(|k| k.1)
            .collect();
        missing_occ.dedup();
        let mut missing: Vec<BodyId> = bodies
            .iter()
            .copied()
            .filter(|b| eval.body_by_id(*b).is_none())
            .collect();
        for b in missing_occ {
            if !missing.contains(&b) {
                missing.push(b);
            }
        }
        let unsup: Vec<BodyId> = bodies
            .iter()
            .copied()
            .filter(|b| unsupported.contains(b))
            .collect();
        let mut shown: Vec<&BodyData> = keys.iter().filter_map(|k| data.get(k)).collect();
        let label = match &v.kind {
            ViewKind::Base { orientation, .. } => {
                format!("{} {} (base)", v.id, orientation.label())
            }
            ViewKind::Projected { parent, from } => {
                format!("{} {} ({} {})", v.id, frame.name(), parent, from.label())
            }
            ViewKind::Section { label, .. } => format!("SECTION {label}-{label}"),
            ViewKind::Detail { label, scale, .. } => {
                format!("DETAIL {label} ({})", fmt_scale(*scale))
            }
        };
        // Section views: the bodies cut by the section plane (part behind it).
        let mut section_data: Vec<BodyData> = Vec::new();
        let mut section_error: Option<String> = None;
        let mut cut_plane: Option<k::Frame> = None;
        if let ViewKind::Section { parent, offset, .. } = &v.kind
            && !shown.is_empty()
        {
            let (pf, _, _) = dr.resolve(*parent)?;
            let (_, from) = v.kind.parent().unwrap_or((*parent, Side::Below));
            let axis = match from {
                Side::Above | Side::Below => pf.up,
                Side::Left | Side::Right => pf.right,
            };
            let plane = k::Frame {
                origin: axis.map(|a| a * offset),
                x: frame.right,
                y: frame.up,
                z: frame.toward,
            };
            cut_plane = Some(plane);
            for key in &keys {
                let Some(body) = breps.get(key) else { continue };
                match crate::regen::split_by_plane(body, &plane, FeatureId(0))
                    .and_then(|(behind, _)| body_data(&behind, chord))
                {
                    Ok(d) => section_data.push(d),
                    Err(e) => {
                        section_error = Some(format!("{}: {e}", key.1));
                        break;
                    }
                }
            }
            shown = if section_error.is_some() {
                Vec::new()
            } else {
                section_data.iter().collect()
            };
        }
        let mut geom =
            (!shown.is_empty()).then(|| view_geometry(&shown, &frame, chord, spacing, diag));
        if let (ViewKind::Detail { center, radius, .. }, Some(g)) = (&v.kind, geom.as_mut()) {
            clip_to_circle(g, *center, *radius);
        }
        // Placement.
        let (glo, ghi) = match &geom {
            Some(g) => (g.lo, g.hi),
            None => ([0.0; 2], [0.0; 2]),
        };
        let half = [
            (ghi[0] - glo[0]) * scale / 2.0,
            (ghi[1] - glo[1]) * scale / 2.0,
        ];
        let half = if geom.is_some() { half } else { [30.0, 8.0] };
        let center = match &v.kind {
            ViewKind::Base { position, .. } | ViewKind::Detail { position, .. } => *position,
            ViewKind::Projected { parent, from } | ViewKind::Section { parent, from, .. } => {
                let p = placed.get(parent).ok_or_else(|| {
                    DocError::Validation(format!("{}: {} is placed before its parent", dr.id, v.id))
                })?;
                let pc = [(p.lo[0] + p.hi[0]) / 2.0, (p.lo[1] + p.hi[1]) / 2.0];
                let ph = [(p.hi[0] - p.lo[0]) / 2.0, (p.hi[1] - p.lo[1]) / 2.0];
                let d = from.placement(dr.projection);
                let mut c = [
                    pc[0] + d[0] * (ph[0] + VIEW_GAP + half[0]),
                    pc[1] + d[1] * (ph[1] + VIEW_GAP + half[1]),
                ];
                // Aligned with the parent; moved further out past any view
                // already in that slot (deterministic: views in order).
                for _ in 0..dr.views.len() {
                    let hit = placed.values().find(|o| {
                        (0..2).all(|i| {
                            c[i] - half[i] < o.hi[i] + VIEW_GAP / 2.0
                                && c[i] + half[i] > o.lo[i] - VIEW_GAP / 2.0
                        })
                    });
                    let Some(o) = hit else { break };
                    let i = if d[0] != 0.0 { 0 } else { 1 };
                    c[i] = if d[i] > 0.0 {
                        o.hi[i] + VIEW_GAP + half[i]
                    } else {
                        o.lo[i] - VIEW_GAP - half[i]
                    };
                }
                c
            }
        };
        let gc = [(glo[0] + ghi[0]) / 2.0, (glo[1] + ghi[1]) / 2.0];
        let off = [center[0] - gc[0] * scale, center[1] - gc[1] * scale];
        let mut prims = Vec::new();
        let mut dims_prims = Vec::new();
        let mut dims = Vec::new();
        let mut status = ViewStatus::Ok;
        let (mut nvis, mut nhid) = (0, 0);
        let (lo, hi);
        if !missing.is_empty() {
            let names: Vec<String> = missing.iter().map(|b| b.to_string()).collect();
            let msg = format!("{}: body {} missing", v.id, names.join(", "));
            issues.push(format!(
                "{} view {}: body {} no longer exists; it is not drawn",
                dr.id,
                v.id,
                names.join(", ")
            ));
            status = ViewStatus::MissingBodies {
                bodies: missing.clone(),
            };
            if geom.is_none() {
                let (a, b) = flag_box(&mut prims, center, &msg);
                (lo, hi) = (a, b);
            } else {
                lo = [off[0] + glo[0] * scale, off[1] + glo[1] * scale];
                hi = [off[0] + ghi[0] * scale, off[1] + ghi[1] * scale];
                prims.push(Prim::Text {
                    at: [lo[0], hi[1] + 2.0],
                    size: 2.5,
                    text: msg,
                    anchor: Anchor::Start,
                    style: LineStyle::Flag,
                });
            }
        } else if !unsup.is_empty() && geom.is_none() {
            let names: Vec<String> = unsup.iter().map(|b| b.to_string()).collect();
            let reason = "mesh bodies have no exact edges to draw".to_string();
            issues.push(format!(
                "{} view {}: body {} not drawn ({reason})",
                dr.id,
                v.id,
                names.join(", ")
            ));
            status = ViewStatus::Unsupported {
                bodies: unsup.clone(),
                reason,
            };
            let (a, b) = flag_box(
                &mut prims,
                center,
                &format!("{}: mesh body not drawn", v.id),
            );
            (lo, hi) = (a, b);
        } else if let Some(e) = &section_error {
            issues.push(format!("{} view {}: section failed: {e}", dr.id, v.id));
            status = ViewStatus::Unsupported {
                bodies: bodies.clone(),
                reason: format!("section failed: {e}"),
            };
            let (a, b) = flag_box(&mut prims, center, &format!("{}: section failed", v.id));
            (lo, hi) = (a, b);
        } else {
            lo = [off[0] + glo[0] * scale, off[1] + glo[1] * scale];
            hi = [off[0] + ghi[0] * scale, off[1] + ghi[1] * scale];
        }
        if let Some(g) = &geom {
            for (p, vis) in &g.prims {
                if !*vis && frame.name() == "Iso" {
                    // Pictorial views show visible edges only.
                    continue;
                }
                if *vis {
                    nvis += 1;
                } else {
                    nhid += 1;
                }
                prims.push(place(p, scale, off));
            }
            // Centre marks.
            for (c, r) in &g.circles {
                let c = [off[0] + c[0] * scale, off[1] + c[1] * scale];
                let e = r * scale + 2.0;
                prims.push(Prim::Line {
                    a: [c[0] - e, c[1]],
                    b: [c[0] + e, c[1]],
                    style: LineStyle::Center,
                });
                prims.push(Prim::Line {
                    a: [c[0], c[1] - e],
                    b: [c[0], c[1] + e],
                    style: LineStyle::Center,
                });
            }
            // Section hatching of the faces lying in the cutting plane.
            if let Some(plane) = &cut_plane {
                let tol = 1e-6 * diag;
                for (a, b) in hatch(&shown, plane, &frame, HATCH_SPACING / scale, tol) {
                    prims.push(Prim::Line {
                        a: [off[0] + a[0] * scale, off[1] + a[1] * scale],
                        b: [off[0] + b[0] * scale, off[1] + b[1] * scale],
                        style: LineStyle::Thin,
                    });
                }
            }
            // Hole callouts (semantic; resolved now).
            let mut called: Vec<usize> = Vec::new();
            for a in &v.annotations {
                let Annotation::HoleCallout { feature } = a else {
                    continue;
                };
                match resolve_hole(doc, eval, *feature, &frame, g, &bodies) {
                    Ok((ci, d)) => {
                        let (c, r) = g.circles[ci];
                        let text = format!("Ø{}", fmt_len(d));
                        diameter_callout(
                            &mut dims_prims,
                            [off[0] + c[0] * scale, off[1] + c[1] * scale],
                            r * scale,
                            &text,
                            LineStyle::Thin,
                        );
                        called.push(ci);
                        dims.push(DimensionReport {
                            kind: "hole_callout".into(),
                            value: Some(d),
                            text,
                            dangling: None,
                        });
                    }
                    Err(why) => {
                        let text = format!("Ø? {feature}: {why}");
                        dims_prims.push(Prim::Text {
                            at: [lo[0], lo[1] - 4.0 - TEXT * dims.len() as f64],
                            size: 2.5,
                            text: text.clone(),
                            anchor: Anchor::Start,
                            style: LineStyle::Flag,
                        });
                        issues.push(format!(
                            "{} view {}: hole callout of {feature} is dangling: {why}",
                            dr.id, v.id
                        ));
                        dims.push(DimensionReport {
                            kind: "hole_callout".into(),
                            value: None,
                            text,
                            dangling: Some(why),
                        });
                    }
                }
            }
            if dr.auto_dimensions
                && missing.is_empty()
                && cut_plane.is_none()
                && !matches!(v.kind, ViewKind::Detail { .. })
            {
                // Overall dimensions from the exact bounds, each world axis once.
                let mut bb = k::Bounds {
                    min: [f64::INFINITY; 3],
                    max: [f64::NEG_INFINITY; 3],
                };
                for d in &shown {
                    for i in 0..3 {
                        bb.min[i] = bb.min[i].min(d.bounds.min[i]);
                        bb.max[i] = bb.max[i].max(d.bounds.max[i]);
                    }
                }
                for (dir, horizontal) in [(frame.right, true), (frame.up, false)] {
                    let Some(ax) = ViewFrame::axis_of(dir) else {
                        continue;
                    };
                    if dimensioned[ax] {
                        continue;
                    }
                    dimensioned[ax] = true;
                    let value = bb.max[ax] - bb.min[ax];
                    let text = fmt_len(value);
                    // Sheet coordinate of the two extremes along this axis.
                    let ends = [bb.min[ax], bb.max[ax]].map(|x| {
                        let mut p = [0.0; 3];
                        p[ax] = x;
                        let t = dot(p, dir);
                        if horizontal {
                            off[0] + t * scale
                        } else {
                            off[1] + t * scale
                        }
                    });
                    let (e0, e1) = (ends[0].min(ends[1]), ends[0].max(ends[1]));
                    if horizontal {
                        linear_dim(
                            &mut dims_prims,
                            [e0, lo[1]],
                            [e1, lo[1]],
                            [0.0, -1.0],
                            8.0,
                            &text,
                        );
                    } else {
                        linear_dim(
                            &mut dims_prims,
                            [hi[0], e0],
                            [hi[0], e1],
                            [1.0, 0.0],
                            8.0,
                            &text,
                        );
                    }
                    dims.push(DimensionReport {
                        kind: if horizontal { "horizontal" } else { "vertical" }.into(),
                        value: Some(value),
                        text,
                        dangling: None,
                    });
                }
                // Diameters of visible end-on circles, grouped by size.
                let mut sizes: Vec<(f64, Vec<usize>)> = Vec::new();
                for (ci, (_, r)) in g.circles.iter().enumerate() {
                    if called.contains(&ci) {
                        continue;
                    }
                    match sizes.iter_mut().find(|(s, _)| (s - r).abs() <= 1e-6 * diag) {
                        Some((_, l)) => l.push(ci),
                        None => sizes.push((*r, vec![ci])),
                    }
                }
                for (r, list) in sizes {
                    let d = 2.0 * r;
                    let text = if list.len() > 1 {
                        format!("{}× Ø{}", list.len(), fmt_len(d))
                    } else {
                        format!("Ø{}", fmt_len(d))
                    };
                    // Attach to the right-most, then top-most circle.
                    let ci = *list
                        .iter()
                        .max_by(|a, b| {
                            let (pa, pb) = (g.circles[**a].0, g.circles[**b].0);
                            (pa[0], pa[1])
                                .partial_cmp(&(pb[0], pb[1]))
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .unwrap_or(&list[0]);
                    let c = g.circles[ci].0;
                    diameter_callout(
                        &mut dims_prims,
                        [off[0] + c[0] * scale, off[1] + c[1] * scale],
                        r * scale,
                        &text,
                        LineStyle::Thin,
                    );
                    dims.push(DimensionReport {
                        kind: "diameter".into(),
                        value: Some(d),
                        text,
                        dangling: None,
                    });
                }
            }
        }
        // Run 7: driven dimensions between semantic references, and balloons.
        // Rows of user dimensions below the view (the view label goes under them).
        let mut user_rows_below = 0usize;
        if let Some(g) = &geom {
            let auto_h = dims.iter().any(|d| d.kind == "horizontal");
            let auto_v = dims.iter().any(|d| d.kind == "vertical");
            let (mut nh, mut nv, mut nflag) = (0usize, 0usize, 0usize);
            for a in &v.annotations {
                let Annotation::Dimension { a, b, tolerance } = a else {
                    continue;
                };
                let resolved = if assembly_view {
                    Err(
                        "dimensions refer to definition bodies; they are not supported in \
                         assembly views"
                            .to_string(),
                    )
                } else {
                    measure_dimension(doc, eval, a, b.as_ref(), &frame, &bodies)
                };
                let kind = if b.is_some() {
                    "distance"
                } else {
                    "hole_diameter"
                };
                match resolved {
                    Ok(Measured::Diameter { d }) => {
                        let text = tolerance.text(d);
                        let found = match a {
                            DimRef::Hole(f) => resolve_hole(doc, eval, *f, &frame, g, &bodies),
                            DimRef::Face(_) => Err("not a hole".into()),
                        };
                        match found {
                            Ok((ci, _)) => {
                                let (c, r) = g.circles[ci];
                                diameter_callout(
                                    &mut dims_prims,
                                    [off[0] + c[0] * scale, off[1] + c[1] * scale],
                                    r * scale,
                                    &format!("Ø{text}"),
                                    LineStyle::Thin,
                                );
                                dims.push(DimensionReport {
                                    kind: kind.into(),
                                    value: Some(d),
                                    text: format!("Ø{text}"),
                                    dangling: None,
                                });
                            }
                            Err(why) => {
                                nflag += 1;
                                dangling_dim(
                                    &mut dims_prims,
                                    &mut dims,
                                    &mut issues,
                                    (dr.id, v.id),
                                    [lo[0], hi[1] + 3.0 + (TEXT + 1.0) * (nflag - 1) as f64],
                                    kind,
                                    why,
                                );
                            }
                        }
                    }
                    Ok(Measured::Distance {
                        value,
                        ta,
                        tb,
                        horizontal,
                        sign,
                    }) => {
                        let text = tolerance.text(value);
                        if horizontal {
                            let off_y = if auto_h { 16.0 } else { 8.0 } + 8.0 * nh as f64;
                            nh += 1;
                            user_rows_below = nh;
                            let xa = off[0] + sign * ta * scale;
                            let xb = off[0] + sign * tb * scale;
                            linear_dim(
                                &mut dims_prims,
                                [xa.min(xb), lo[1]],
                                [xa.max(xb), lo[1]],
                                [0.0, -1.0],
                                off_y,
                                &text,
                            );
                        } else {
                            let off_x = if auto_v { 8.0 } else { 0.0 } + 8.0 + 8.0 * nv as f64;
                            nv += 1;
                            let ya = off[1] + sign * ta * scale;
                            let yb = off[1] + sign * tb * scale;
                            linear_dim(
                                &mut dims_prims,
                                [lo[0], ya.min(yb)],
                                [lo[0], ya.max(yb)],
                                [-1.0, 0.0],
                                off_x,
                                &text,
                            );
                        }
                        dims.push(DimensionReport {
                            kind: kind.into(),
                            value: Some(value),
                            text,
                            dangling: None,
                        });
                    }
                    Err(why) => {
                        nflag += 1;
                        dangling_dim(
                            &mut dims_prims,
                            &mut dims,
                            &mut issues,
                            (dr.id, v.id),
                            [lo[0], hi[1] + 3.0 + (TEXT + 1.0) * (nflag - 1) as f64],
                            kind,
                            why,
                        );
                    }
                }
            }
            if v.annotations.contains(&Annotation::Balloons) {
                // Anchor: the centre of each BOM item's first occurrence in
                // this view (view coordinates), then balloons outside the view.
                let mut anchors: Vec<(usize, [f64; 2])> = Vec::new();
                for row in &bom {
                    let mut bb: Option<k::Bounds> = None;
                    let first = keys
                        .iter()
                        .find(|key| {
                            key.0 != 0 && eval.occurrences[key.0 - 1].component == row.component
                        })
                        .map(|key| key.0);
                    let Some(occ) = first else { continue };
                    for key in keys.iter().filter(|key| key.0 == occ) {
                        if let Some(d) = data.get(key) {
                            let b = bb.get_or_insert(d.bounds);
                            for i in 0..3 {
                                b.min[i] = b.min[i].min(d.bounds.min[i]);
                                b.max[i] = b.max[i].max(d.bounds.max[i]);
                            }
                        }
                    }
                    let Some(b) = bb else { continue };
                    let c3 = [0, 1, 2].map(|i| (b.min[i] + b.max[i]) / 2.0);
                    let c = frame.project(c3);
                    anchors.push((row.item, [off[0] + c[0] * scale, off[1] + c[1] * scale]));
                }
                // Clear the dimensions on each side: automatic vertical
                // dimensions stand right of the view, user ones left of it.
                let right = if auto_v { 18.0 } else { 0.0 };
                let left = if nv > 0 { 8.0 * nv as f64 + 14.0 } else { 0.0 };
                let below = if auto_h { 22.0 } else { 10.0 } + 8.0 * nh as f64;
                pending_balloons.push((v.id, anchors.clone(), lo, hi, [left, right, below]));
                for (item, _) in &anchors {
                    dims.push(DimensionReport {
                        kind: "balloon".into(),
                        value: Some(*item as f64),
                        text: item.to_string(),
                        dangling: None,
                    });
                }
                if anchors.is_empty() {
                    issues.push(format!(
                        "{} view {}: balloons requested but the view shows no component occurrence",
                        dr.id, v.id
                    ));
                }
            }
        }
        if !unsup.is_empty() && geom.is_some() {
            issues.push(format!(
                "{} view {}: mesh bodies not drawn: {:?}",
                dr.id, v.id, unsup
            ));
        }
        // View label.
        prims.push(Prim::Text {
            at: [
                (lo[0] + hi[0]) / 2.0,
                lo[1]
                    - if dims.iter().any(|d| d.kind == "horizontal") {
                        18.0
                    } else {
                        6.0
                    }
                    - if user_rows_below > 0 {
                        8.0 * user_rows_below as f64
                            + if dims.iter().any(|d| d.kind == "horizontal") {
                                0.0
                            } else {
                                4.0
                            }
                    } else {
                        0.0
                    },
            ],
            size: 2.5,
            text: label.clone(),
            anchor: Anchor::Middle,
            style: LineStyle::Thin,
        });
        prims.sort_by_cached_key(sort_key);
        groups.push(Group {
            id: format!("view-{}", v.id.0),
            label: label.clone(),
            prims,
        });
        groups.push(Group {
            id: format!("dims-{}", v.id.0),
            label: format!("{} dimensions", v.id),
            prims: dims_prims,
        });
        let _ = base;
        reports.push(ViewReport {
            id: v.id,
            label,
            direction: frame.name().to_string(),
            scale,
            center,
            status,
            visible_curves: nvis,
            hidden_curves: nhid,
            dimensions: dims,
        });
        placed.insert(
            v.id,
            Placed {
                lo,
                hi,
                off,
                scale,
                frame,
            },
        );
        if let (
            ViewKind::Section {
                parent,
                from,
                offset,
                label,
            },
            Some(p),
        ) = (
            &v.kind,
            placed.get(&v.kind.parent().map(|x| x.0).unwrap_or(v.id)),
        ) {
            groups.push(cutting_line(*parent, *from, *offset, label, p, &frame));
        }
        if let ViewKind::Detail {
            parent,
            center: dc,
            radius,
            label,
            ..
        } = &v.kind
            && let Some(p) = placed.get(parent)
        {
            // The detail's boundary in the detail view and on its parent.
            let mut prims = vec![Prim::Arc {
                c: [off[0] + dc[0] * scale, off[1] + dc[1] * scale],
                r: radius * scale,
                start: 0.0,
                sweep: TAU,
                style: LineStyle::Thin,
            }];
            let pc = [p.off[0] + dc[0] * p.scale, p.off[1] + dc[1] * p.scale];
            let pr = radius * p.scale;
            prims.push(Prim::Arc {
                c: pc,
                r: pr,
                start: 0.0,
                sweep: TAU,
                style: LineStyle::Thin,
            });
            let s = std::f64::consts::FRAC_1_SQRT_2;
            prims.push(Prim::Text {
                at: [pc[0] + pr * s + 1.0, pc[1] + pr * s + 1.0],
                size: TEXT,
                text: label.clone(),
                anchor: Anchor::Start,
                style: LineStyle::Thin,
            });
            groups.push(Group {
                id: format!("detail-{label}"),
                label: format!("Detail {label} boundary on {parent} and {}", v.id),
                prims,
            });
        }
    }
    // Clipping check: everything drawn must be on the sheet inside the border.
    for (vid, p) in &placed {
        if p.lo[0] < 10.0 || p.lo[1] < 10.0 || p.hi[0] > w - 10.0 || p.hi[1] > h - 10.0 {
            issues.push(format!(
                "{} view {vid} extends past the sheet border (reduce the scale or move it)",
                dr.id
            ));
        }
    }
    let table_box = dr.bom.as_ref().map(|t| {
        let (_, lo, hi) = bom_table(&bom, t, w, h);
        (lo, hi)
    });
    for (vid, anchors, lo, hi, clear) in &pending_balloons {
        let mut others: Vec<([f64; 2], [f64; 2])> = placed
            .iter()
            .filter(|(id, _)| *id != vid)
            .map(|(_, p)| (p.lo, p.hi))
            .collect();
        others.extend(table_box);
        let free = |b: &([f64; 2], [f64; 2])| {
            b.0[0] >= 10.0
                && b.0[1] >= 10.0
                && b.1[0] <= w - 10.0
                && b.1[1] <= h - 10.0
                && others.iter().all(|o| {
                    b.1[0] <= o.0[0] || b.0[0] >= o.1[0] || b.1[1] <= o.0[1] || b.0[1] >= o.1[1]
                })
        };
        let layouts: Vec<BalloonLayout> = [
            BalloonSide::Right,
            BalloonSide::Left,
            BalloonSide::Above,
            BalloonSide::Below,
        ]
        .into_iter()
        .map(|side| balloons(anchors, *lo, *hi, *clear, side))
        .collect();
        let pick = layouts.iter().position(|(_, b)| free(b));
        if pick.is_none() {
            issues.push(format!(
                "{} view {vid}: the balloons overlap another view or the BOM table on every side",
                dr.id
            ));
        }
        groups.push(Group {
            id: format!("balloons-{}", vid.0),
            label: format!("{vid} balloons"),
            prims: layouts[pick.unwrap_or(0)].0.clone(),
        });
    }
    if let Some(t) = &dr.bom {
        let (g, lo, hi) = bom_table(&bom, t, w, h);
        for (vid, p) in &placed {
            if lo[0] < p.hi[0] && hi[0] > p.lo[0] && lo[1] < p.hi[1] && hi[1] > p.lo[1] {
                issues.push(format!(
                    "{} the BOM table overlaps view {vid} (move the table or the view)",
                    dr.id
                ));
            }
        }
        if bom.is_empty() {
            issues.push(format!(
                "{} the BOM table is empty (no evaluated component occurrences)",
                dr.id
            ));
        }
        groups.push(g);
    }
    if !dr.notes.is_empty() {
        let mut prims = Vec::new();
        for n in &dr.notes {
            for (i, line) in n.text.lines().enumerate() {
                prims.push(Prim::Text {
                    at: [n.position[0], n.position[1] - 1.6 * TEXT * i as f64],
                    size: TEXT,
                    text: line.to_string(),
                    anchor: Anchor::Start,
                    style: LineStyle::Thin,
                });
            }
        }
        groups.push(Group {
            id: "notes".into(),
            label: "Notes".into(),
            prims,
        });
    }
    issues.sort();
    let mut all = vec![border(dr, w, h), title_block(doc, dr, w, &used)];
    all.extend(groups);
    Ok(DrawingRender {
        sheet: SheetPlot {
            width: w,
            height: h,
            title: dr.title.title.clone(),
            groups: all,
        },
        views: reports,
        issues,
        sample_spacing: spacing,
        chord,
        bom: if dr.bom.is_some() { bom } else { Vec::new() },
    })
}

/// What is drawn: (0, body) for a definition/loose body at its own
/// placement, (i + 1, body) for a body of `eval.occurrences[i]` placed by its
/// transform.
type Key = (usize, BodyId);

/// The drawn items of view `id` (its base view's bodies and, for an assembly
/// view, every evaluated occurrence's bodies).
fn view_keys(dr: &Drawing, id: ViewId, eval: &Evaluation) -> Result<Vec<Key>> {
    let (_, _, base) = dr.resolve(id)?;
    let ViewKind::Base {
        bodies, assembly, ..
    } = &dr.view(base)?.kind
    else {
        return Ok(Vec::new());
    };
    let mut out: Vec<Key> = bodies.iter().map(|b| (0, *b)).collect();
    if *assembly {
        for (i, o) in eval.occurrences.iter().enumerate() {
            out.extend(o.bodies.iter().map(|b| (i + 1, *b)));
        }
    }
    Ok(out)
}

/// One row of the drawing's bill of materials.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BomRow {
    /// Item number (balloons show it).
    pub item: usize,
    pub component: crate::assembly::ComponentId,
    pub name: String,
    pub part_number: String,
    /// Evaluated (not suppressed) occurrences, nested ones included.
    pub quantity: usize,
    /// Distinct materials of the definition bodies ("-" when none).
    pub material: String,
}

/// Flattened BOM of the evaluated assembly: one row per component with at
/// least one evaluated occurrence, in component order; quantity = number of
/// evaluated occurrences (suppressed ones are not evaluated). Item numbers
/// are 1.. in that order.
pub fn bom_rows(doc: &Document, eval: &Evaluation) -> Vec<BomRow> {
    let mut rows = Vec::new();
    for c in &doc.assembly.components {
        let quantity = eval
            .occurrences
            .iter()
            .filter(|o| o.component == c.id)
            .count();
        if quantity == 0 {
            continue;
        }
        let mut mats: Vec<String> = c
            .bodies
            .iter()
            .filter_map(|b| doc.bodies.iter().find(|x| x.id == *b))
            .filter_map(|b| b.material.as_ref().map(|m| m.name.clone()))
            .collect();
        mats.sort();
        mats.dedup();
        rows.push(BomRow {
            item: rows.len() + 1,
            component: c.id,
            name: c.name.clone(),
            part_number: if c.part_number.is_empty() {
                "-".into()
            } else {
                c.part_number.clone()
            },
            quantity,
            material: if mats.is_empty() {
                "-".into()
            } else {
                mats.join(", ")
            },
        });
    }
    rows
}

/// Geometry of a dimension reference.
enum RefGeom {
    Plane { o: [f64; 3], n: [f64; 3] },
    Axis { p: [f64; 3], d: [f64; 3], dia: f64 },
}

enum Measured {
    Diameter {
        d: f64,
    },
    /// `value` = |tb − ta|; `ta`/`tb` are the references' coordinates along
    /// the view's right (horizontal) or up axis, times `sign`.
    Distance {
        value: f64,
        ta: f64,
        tb: f64,
        horizontal: bool,
        sign: f64,
    },
}

fn resolve_ref(
    doc: &Document,
    eval: &Evaluation,
    r: &DimRef,
    bodies: &[BodyId],
) -> std::result::Result<RefGeom, String> {
    match r {
        DimRef::Face(fr) => {
            let c = eval
                .faces
                .iter()
                .find(|c| c.reference == *fr)
                .ok_or_else(|| {
                    format!(
                        "face {:?} of {} no longer exists (deleted, suppressed or changed)",
                        fr.role, fr.feature
                    )
                })?;
            if !bodies.contains(&c.body) {
                return Err(format!(
                    "the face is on {}, which this view does not show",
                    c.body
                ));
            }
            Ok(RefGeom::Plane {
                o: c.frame.origin,
                n: c.frame.z,
            })
        }
        DimRef::Hole(feature) => {
            let f = doc
                .features
                .iter()
                .find(|f| f.id == *feature)
                .ok_or_else(|| format!("hole {feature} no longer exists"))?;
            let FeatureKind::Hole {
                center,
                diameter,
                target,
                ..
            } = &f.kind
            else {
                return Err(format!("{feature} is not a hole"));
            };
            if f.suppressed {
                return Err(format!("hole {feature} is suppressed"));
            }
            if !bodies.contains(target) {
                return Err(format!(
                    "hole {feature} is in {target}, which this view does not show"
                ));
            }
            let fr = eval
                .features
                .iter()
                .find(|r| r.id == *feature)
                .and_then(|r| r.frame)
                .ok_or_else(|| format!("hole {feature} was not evaluated"))?;
            let (p, d) = (fr.to_world(*center), fr.z);
            // Review run 7 #3: the hole's frame is where it was made. Require
            // its cylinder in the current body on that axis (a later move,
            // scale or fill of the body makes the reference dangle instead of
            // printing a stale value).
            if !hole_cylinder_present(eval, *target, p, d, diameter / 2.0) {
                return Err(format!(
                    "hole {feature}'s cylinder is no longer in {target} at its modelled place \
                     (moved, scaled or filled)"
                ));
            }
            Ok(RefGeom::Axis {
                p,
                d,
                dia: *diameter,
            })
        }
    }
}

/// Whether `body` has a cylindrical face of radius `r` whose axis is the line
/// through `p` along `d` (kernel linear tolerance, relative 1e-9 on angles).
fn hole_cylinder_present(
    eval: &Evaluation,
    body: BodyId,
    p: [f64; 3],
    d: [f64; 3],
    r: f64,
) -> bool {
    let Some(b) = eval.body_by_id(body).and_then(|b| b.geometry.brep()) else {
        return false;
    };
    let tol = k::tol::KERNEL_LINEAR.max(1e-9 * r);
    (0..k::face_count(b)).any(|i| match k::face_geometry(b, i) {
        Ok(k::DirectFace::Cylinder {
            origin,
            axis,
            radius,
            ..
        }) => {
            let a = norm(axis);
            let w = sub(p, origin);
            let along = dot(w, a);
            let off = [0, 1, 2].map(|k| w[k] - along * a[k]);
            (radius - r).abs() <= tol
                && dot(a, d).abs() >= 1.0 - 1e-9
                && dot(off, off).sqrt() <= tol
        }
        _ => false,
    })
}

/// Measure a dimension in the current model and check it can be drawn in
/// true length along the view's horizontal or vertical axis.
fn measure_dimension(
    doc: &Document,
    eval: &Evaluation,
    a: &DimRef,
    b: Option<&DimRef>,
    frame: &ViewFrame,
    bodies: &[BodyId],
) -> std::result::Result<Measured, String> {
    const PAR: f64 = 1e-9;
    let ga = resolve_ref(doc, eval, a, bodies)?;
    let Some(b) = b else {
        return match ga {
            RefGeom::Axis { d, dia, .. } => {
                if (dot(d, frame.toward).abs() - 1.0).abs() > PAR {
                    Err("the hole is not seen end-on in this view".into())
                } else {
                    Ok(Measured::Diameter { d: dia })
                }
            }
            RefGeom::Plane { .. } => Err("a diameter needs a hole reference".into()),
        };
    };
    let gb = resolve_ref(doc, eval, b, bodies)?;
    let (dir, pa, pb) = match (&ga, &gb) {
        (RefGeom::Plane { o: oa, n: na }, RefGeom::Plane { o: ob, n: nb }) => {
            if dot(*na, *nb).abs() < 1.0 - PAR {
                return Err("the faces are not parallel".into());
            }
            (*na, *oa, *ob)
        }
        (RefGeom::Plane { o, n }, RefGeom::Axis { p, d, .. })
        | (RefGeom::Axis { p, d, .. }, RefGeom::Plane { o, n }) => {
            if dot(*n, *d).abs() > PAR {
                return Err("the hole axis is not parallel to the face".into());
            }
            if matches!(ga, RefGeom::Plane { .. }) {
                (*n, *o, *p)
            } else {
                (*n, *p, *o)
            }
        }
        (RefGeom::Axis { p: pa, d: da, .. }, RefGeom::Axis { p: pb, d: db, .. }) => {
            if dot(*da, *db).abs() < 1.0 - PAR {
                return Err("the hole axes are not parallel".into());
            }
            let w = sub(*pb, *pa);
            let along = dot(w, *da);
            let perp = [0, 1, 2].map(|i| w[i] - along * da[i]);
            if dot(perp, perp).sqrt() <= k::tol::MIN_LENGTH {
                return Err("the hole axes coincide".into());
            }
            (norm(perp), *pa, *pb)
        }
    };
    if dot(dir, frame.toward).abs() > PAR {
        return Err("the measured direction is not in this view's plane (not true length)".into());
    }
    let (x, y) = (dot(dir, frame.right), dot(dir, frame.up));
    let horizontal = if x.abs() > 1.0 - PAR {
        true
    } else if y.abs() > 1.0 - PAR {
        false
    } else {
        return Err("oblique dimensions are not supported (measured direction is not along the view's horizontal or vertical axis)".into());
    };
    let sign = if horizontal { x.signum() } else { y.signum() };
    let (ta, tb) = (dot(pa, dir), dot(pb, dir));
    Ok(Measured::Distance {
        value: (tb - ta).abs(),
        ta,
        tb,
        horizontal,
        sign,
    })
}

/// A dangling dimension: red text without a value, reported as an issue.
fn dangling_dim(
    prims: &mut Vec<Prim>,
    dims: &mut Vec<DimensionReport>,
    issues: &mut Vec<String>,
    (did, vid): (DrawingId, ViewId),
    at: [f64; 2],
    kind: &str,
    why: String,
) {
    let text = format!("? {kind}: {why}");
    prims.push(Prim::Text {
        at,
        size: 2.5,
        text: text.clone(),
        anchor: Anchor::Start,
        style: LineStyle::Flag,
    });
    issues.push(format!(
        "{did} view {vid}: {kind} dimension is dangling: {why}"
    ));
    dims.push(DimensionReport {
        kind: kind.into(),
        value: None,
        text,
        dangling: Some(why),
    });
}

/// Balloon radius (sheet mm).
const BALLOON_R: f64 = 4.0;

/// Balloon primitives and the box of their circles.
type BalloonLayout = (Vec<Prim>, ([f64; 2], [f64; 2]));

#[derive(Clone, Copy, PartialEq)]
enum BalloonSide {
    Right,
    Left,
    Above,
    Below,
}

/// Balloons in a column right or left of the view, or a row above or below
/// it, past `clear` (left, right, below) sheet mm of dimensions, at least
/// 2.25 balloon radii apart, each with a leader ending in a dot at its
/// anchor. Returns the primitives and the box of the balloon circles.
fn balloons(
    anchors: &[(usize, [f64; 2])],
    lo: [f64; 2],
    hi: [f64; 2],
    clear: [f64; 3],
    side: BalloonSide,
) -> BalloonLayout {
    let mut out = Vec::new();
    let mut sorted: Vec<&(usize, [f64; 2])> = anchors.iter().collect();
    let column = matches!(side, BalloonSide::Right | BalloonSide::Left);
    if column {
        sorted.sort_by(|a, b| b.1[1].total_cmp(&a.1[1]).then(a.0.cmp(&b.0)));
    } else {
        sorted.sort_by(|a, b| a.1[0].total_cmp(&b.1[0]).then(a.0.cmp(&b.0)));
    }
    let fixed = match side {
        BalloonSide::Right => hi[0] + clear[1] + 3.0 * BALLOON_R,
        BalloonSide::Left => lo[0] - clear[0] - 3.0 * BALLOON_R,
        BalloonSide::Above => hi[1] + 2.5 * BALLOON_R,
        BalloonSide::Below => lo[1] - clear[2] - 2.0 * BALLOON_R,
    };
    let (mut blo, mut bhi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    let mut last = if column {
        f64::INFINITY
    } else {
        f64::NEG_INFINITY
    };
    for (item, a) in sorted {
        let c = if column {
            let y = a[1].min(last - 2.25 * BALLOON_R);
            last = y;
            [fixed, y]
        } else {
            let x = a[0].max(last + 2.25 * BALLOON_R);
            last = x;
            [x, fixed]
        };
        for i in 0..2 {
            blo[i] = blo[i].min(c[i] - BALLOON_R);
            bhi[i] = bhi[i].max(c[i] + BALLOON_R);
        }
        out.push(Prim::Arc {
            c,
            r: BALLOON_R,
            start: 0.0,
            sweep: TAU,
            style: LineStyle::Thin,
        });
        out.push(Prim::Text {
            at: [c[0], c[1] - TEXT / 3.0],
            size: TEXT,
            text: item.to_string(),
            anchor: Anchor::Middle,
            style: LineStyle::Thin,
        });
        let d = [a[0] - c[0], a[1] - c[1]];
        let l = d[0].hypot(d[1]).max(1e-9);
        out.push(Prim::Line {
            a: [c[0] + d[0] / l * BALLOON_R, c[1] + d[1] / l * BALLOON_R],
            b: *a,
            style: LineStyle::Thin,
        });
        let r = 0.6;
        out.push(Prim::Polygon {
            pts: vec![
                [a[0] - r, a[1]],
                [a[0], a[1] - r],
                [a[0] + r, a[1]],
                [a[0], a[1] + r],
            ],
            fill: true,
            style: LineStyle::Thin,
        });
    }
    (out, (blo, bhi))
}

/// Column titles and widths (mm) of the BOM table.
const BOM_COLS: [(&str, f64); 5] = [
    ("ITEM", 12.0),
    ("PART NUMBER", 36.0),
    ("NAME", 40.0),
    ("QTY", 12.0),
    ("MATERIAL", 30.0),
];
const BOM_ROW: f64 = 6.0;

/// Text cut to the characters that fit `width` at `size` (Helvetica average
/// width ≈ 0.55 em), with "..." when cut.
fn fit_text(t: &str, width: f64, size: f64) -> String {
    let n = ((width - 2.0) / (0.55 * size)).floor().max(1.0) as usize;
    if t.chars().count() <= n {
        t.to_string()
    } else {
        let mut s: String = t.chars().take(n.saturating_sub(3)).collect();
        s.push_str("...");
        s
    }
}

/// The BOM table group and its sheet box (header row on top, items below
/// in order, bottom row = last item).
fn bom_table(rows: &[BomRow], t: &BomTable, w: f64, h: f64) -> (Group, [f64; 2], [f64; 2]) {
    let width: f64 = BOM_COLS.iter().map(|c| c.1).sum();
    let height = BOM_ROW * (rows.len() + 1) as f64;
    // Default: top-right corner inside the border (views are placed from the
    // lower left, the title block is bottom right).
    let p0 = t.position.unwrap_or([w - 10.0 - width, h - 10.0 - height]);
    let (lo, hi) = (p0, [p0[0] + width, p0[1] + height]);
    let mut prims = vec![Prim::Polygon {
        pts: vec![lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]],
        fill: false,
        style: LineStyle::Thin,
    }];
    for r in 1..=rows.len() {
        let y = lo[1] + BOM_ROW * r as f64;
        prims.push(Prim::Line {
            a: [lo[0], y],
            b: [hi[0], y],
            style: LineStyle::Thin,
        });
    }
    let mut x = lo[0];
    for (i, (_, cw)) in BOM_COLS.iter().enumerate() {
        if i > 0 {
            prims.push(Prim::Line {
                a: [x, lo[1]],
                b: [x, hi[1]],
                style: LineStyle::Thin,
            });
        }
        x += cw;
    }
    let size = 2.5;
    let mut line = |y: f64, cells: [String; 5]| {
        let mut x = lo[0];
        for (c, (_, cw)) in cells.iter().zip(BOM_COLS.iter()) {
            prims.push(Prim::Text {
                at: [x + 1.0, y + (BOM_ROW - size) / 2.0],
                size,
                text: fit_text(c, *cw, size),
                anchor: Anchor::Start,
                style: LineStyle::Thin,
            });
            x += cw;
        }
    };
    line(hi[1] - BOM_ROW, BOM_COLS.map(|c| c.0.to_string()));
    for (i, r) in rows.iter().enumerate() {
        line(
            hi[1] - BOM_ROW * (i + 2) as f64,
            [
                r.item.to_string(),
                r.part_number.clone(),
                r.name.clone(),
                r.quantity.to_string(),
                r.material.clone(),
            ],
        );
    }
    (
        Group {
            id: "bom".into(),
            label: "Bill of materials".into(),
            prims,
        },
        lo,
        hi,
    )
}

/// Find the end-on circle of Hole `feature` in a view: (circle index, diameter).
fn resolve_hole(
    doc: &Document,
    eval: &Evaluation,
    feature: FeatureId,
    frame: &ViewFrame,
    g: &ViewGeom,
    bodies: &[BodyId],
) -> std::result::Result<(usize, f64), String> {
    let f = doc
        .features
        .iter()
        .find(|f| f.id == feature)
        .ok_or_else(|| "the feature no longer exists".to_string())?;
    let FeatureKind::Hole {
        center,
        diameter,
        target,
        ..
    } = &f.kind
    else {
        return Err("the feature is not a hole".into());
    };
    if f.suppressed {
        return Err("the hole is suppressed".into());
    }
    if !bodies.contains(target) {
        return Err(format!(
            "the hole is in {target}, which this view does not show"
        ));
    }
    let fr = eval
        .features
        .iter()
        .find(|r| r.id == feature)
        .and_then(|r| r.frame)
        .ok_or_else(|| "the hole was not evaluated".to_string())?;
    if dot(fr.z, frame.toward).abs() < 1.0 - 1e-9 {
        return Err("the hole is not seen end-on in this view".into());
    }
    let c = frame.project(fr.to_world(*center));
    let r = diameter / 2.0;
    g.circles
        .iter()
        .position(|(cc, cr)| {
            (cc[0] - c[0]).hypot(cc[1] - c[1]) <= 1e-6 * r.max(1.0)
                && (cr - r).abs() <= 1e-6 * r.max(1.0)
        })
        .map(|i| (i, *diameter))
        .ok_or_else(|| "no visible circle of the hole's diameter at its centre in this view".into())
}

fn border(_dr: &Drawing, w: f64, h: f64) -> Group {
    let m = 10.0;
    Group {
        id: "border".into(),
        label: "Border".into(),
        prims: vec![Prim::Polygon {
            pts: vec![[m, m], [w - m, m], [w - m, h - m], [m, h - m]],
            fill: false,
            style: LineStyle::Frame,
        }],
    }
}

/// Part number and material from the model when the title block leaves them
/// empty.
pub fn title_fields(doc: &Document, dr: &Drawing, bodies: &[BodyId]) -> (String, String) {
    let pn = if !dr.title.part_number.is_empty() {
        dr.title.part_number.clone()
    } else {
        doc.active_configuration
            .as_ref()
            .and_then(|a| doc.configurations.iter().find(|c| &c.name == a))
            .map(|c| c.part_number.clone())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "-".into())
    };
    let mat = if !dr.title.material.is_empty() {
        dr.title.material.clone()
    } else {
        let mut names: Vec<String> = bodies
            .iter()
            .filter_map(|b| doc.bodies.iter().find(|x| x.id == *b))
            .filter_map(|b| b.material.as_ref().map(|m| m.name.clone()))
            .collect();
        names.sort();
        names.dedup();
        if names.is_empty() {
            "-".into()
        } else {
            names.join(", ")
        }
    };
    (pn, mat)
}

fn title_block(doc: &Document, dr: &Drawing, w: f64, bodies: &[BodyId]) -> Group {
    let (bw, bh) = (130.0, 32.0);
    let x0 = w - 10.0 - bw;
    let y0 = 10.0;
    let mut p = vec![Prim::Polygon {
        pts: vec![[x0, y0], [x0 + bw, y0], [x0 + bw, y0 + bh], [x0, y0 + bh]],
        fill: false,
        style: LineStyle::Thin,
    }];
    let row = |y: f64| Prim::Line {
        a: [x0, y],
        b: [x0 + bw, y],
        style: LineStyle::Thin,
    };
    p.push(row(y0 + 12.0));
    p.push(row(y0 + 22.0));
    p.push(Prim::Line {
        a: [x0 + 65.0, y0],
        b: [x0 + 65.0, y0 + 22.0],
        style: LineStyle::Thin,
    });
    let (pn, mat) = title_fields(doc, dr, bodies);
    let scales: Vec<f64> = dr
        .views
        .iter()
        .filter_map(|v| match v.kind {
            ViewKind::Base { scale, .. } => Some(scale),
            _ => None,
        })
        .collect();
    let scale = match scales.first() {
        Some(s) if scales.iter().all(|x| x == s) => fmt_scale(*s),
        Some(_) => "AS SHOWN".into(),
        None => "-".into(),
    };
    let label = |x: f64, y: f64, t: &str| Prim::Text {
        at: [x, y],
        size: 1.8,
        text: t.to_string(),
        anchor: Anchor::Start,
        style: LineStyle::Thin,
    };
    let value = |x: f64, y: f64, t: &str, s: f64| Prim::Text {
        at: [x, y],
        size: s,
        text: t.to_string(),
        anchor: Anchor::Start,
        style: LineStyle::Thin,
    };
    p.push(label(x0 + 1.5, y0 + bh - 3.0, "TITLE"));
    p.push(value(x0 + 1.5, y0 + 24.5, &dr.title.title, 5.0));
    p.push(label(x0 + 1.5, y0 + 19.5, "PART NUMBER"));
    p.push(value(x0 + 1.5, y0 + 14.0, &pn, 3.5));
    p.push(label(x0 + 66.5, y0 + 19.5, "MATERIAL"));
    p.push(value(x0 + 66.5, y0 + 14.0, &mat, 3.5));
    p.push(label(x0 + 1.5, y0 + 9.5, "SCALE"));
    p.push(value(x0 + 1.5, y0 + 3.0, &scale, 3.5));
    p.push(label(x0 + 32.0, y0 + 9.5, "SHEET"));
    p.push(value(
        x0 + 32.0,
        y0 + 3.0,
        &format!("{} mm", dr.sheet.label()),
        3.5,
    ));
    p.push(label(x0 + 66.5, y0 + 9.5, "PROJECTION"));
    p.push(value(x0 + 66.5, y0 + 3.0, dr.projection.label(), 2.8));
    Group {
        id: "title".into(),
        label: "Title block".into(),
        prims: p,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_right_handed_and_projected_views_match_presets() {
        for o in Orientation::ALL {
            let f = o.frame();
            let c = cross(f.right, f.up);
            assert!(dot(c, f.toward) > 1.0 - 1e-12, "{o:?}");
        }
        let front = Orientation::Front.frame();
        assert_eq!(front.seen_from(Side::Above).name(), "Top");
        assert_eq!(front.seen_from(Side::Below).name(), "Bottom");
        assert_eq!(front.seen_from(Side::Right).name(), "Right");
        assert_eq!(front.seen_from(Side::Left).name(), "Left");
        let top = front.seen_from(Side::Above);
        let t = Orientation::Top.frame();
        assert!(dot(top.right, t.right) > 1.0 - 1e-12 && dot(top.up, t.up) > 1.0 - 1e-12);
        let right = front.seen_from(Side::Right);
        let r = Orientation::Right.frame();
        assert!(dot(right.right, r.right) > 1.0 - 1e-12 && dot(right.up, r.up) > 1.0 - 1e-12);
        assert_eq!(Side::Above.placement(Projection::ThirdAngle), [0.0, 1.0]);
        assert_eq!(Side::Above.placement(Projection::FirstAngle), [-0.0, -1.0]);
    }

    #[test]
    fn interval_logic() {
        let (v, h) = resolve_intervals(&[(0.0, 10.0, true), (5.0, 20.0, false)], 1e-9);
        assert_eq!(v, vec![(0.0, 10.0)]);
        assert_eq!(h, vec![(10.0, 20.0)]);
        assert_eq!(fmt_len(40.0), "40");
        assert_eq!(fmt_len(7.5), "7.5");
        assert_eq!(fmt_len(12.345), "12.35");
        assert_eq!(fmt_scale(0.5), "1:2");
        assert_eq!(fmt_scale(2.0), "2:1");
        assert_eq!(fit_scale(40.0, 90.0), 2.0);
    }
}
