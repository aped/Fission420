//! Drawing commands (docs/DRAWINGS.md): create/edit/delete drawings,
//! add/edit/remove views and hole callouts (each one undo step), and export a
//! drawing rendered from the current geometry as SVG or vector PDF.
use crate::{Command, Response, Session};
use rcad_document::drawing::{
    self as dw, Annotation, Drawing, DrawingId, DrawingView, Orientation, Projection, SheetSize,
    Side, ViewId, ViewKind,
};
use rcad_document::{BodyId, DocError, Document, FeatureId, FeatureKind, write_atomic};
use rcad_kernel as k;
use serde::{Deserialize, Serialize};

/// A view to add.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ViewSpec {
    /// `scale` default: the largest standard scale that fits; `position`
    /// default: the sheet's standard base-view position for the convention.
    Base {
        bodies: Vec<BodyId>,
        orientation: Orientation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<[f64; 2]>,
        /// Show the evaluated assembly (placed occurrences) too.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        assembly: bool,
    },
    Projected {
        parent: ViewId,
        from: Side,
    },
    /// Full section `label`-`label` of `parent` seen from `from`, cutting
    /// plane at `offset` model mm along the parent's up (above/below) or right
    /// (left/right) axis.
    Section {
        parent: ViewId,
        from: Side,
        offset: f64,
        label: String,
    },
    /// Detail `label` of `parent`: the region within `radius` (model mm) of
    /// `center` (parent view coordinates, model mm along its right/up axes),
    /// at `scale` (default twice the parent's) centred at `position` (sheet
    /// mm; default right of the middle of the sheet).
    Detail {
        parent: ViewId,
        center: [f64; 2],
        radius: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<[f64; 2]>,
        label: String,
    },
}

/// Export file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawingFormat {
    Svg,
    Pdf,
    /// R12 ASCII DXF in mm, one layer per line style.
    Dxf,
}

/// What an export reports (the sheet itself is in the file).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrawingReport {
    pub drawing: DrawingId,
    pub format: DrawingFormat,
    pub bytes: usize,
    pub views: Vec<dw::ViewReport>,
    /// Flagged views and dangling callouts; empty when every reference is valid.
    pub issues: Vec<String>,
    pub sample_spacing: f64,
    pub chord: f64,
    /// The BOM table rows as drawn (empty without a table).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bom: Vec<dw::BomRow>,
}

fn bad(m: String) -> DocError {
    DocError::Validation(m)
}

/// Combined exact bounds of `bodies` in the current evaluation.
fn bodies_extent(m: &rcad_document::Model, bodies: &[BodyId]) -> Result<f64, DocError> {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for b in bodies {
        let eb = m
            .evaluation()
            .body_by_id(*b)
            .ok_or_else(|| bad(format!("{b} is not an evaluated body")))?;
        let g = eb.geometry.brep().ok_or_else(|| {
            DocError::ProductLimit(format!(
                "{b} is a mesh body; drawings need exact (B-rep) bodies"
            ))
        })?;
        let bb = k::bounds(g).map_err(|e| bad(e.to_string()))?;
        for i in 0..3 {
            lo[i] = lo[i].min(bb.min[i]);
            hi[i] = hi[i].max(bb.max[i]);
        }
    }
    Ok((0..3).map(|i| hi[i] - lo[i]).fold(0.0, f64::max))
}

/// Largest extent of `bodies` plus, with `assembly`, every evaluated
/// occurrence (exact body bounds, placed: the 8 transformed box corners, a
/// conservative box).
fn view_extent(
    m: &rcad_document::Model,
    bodies: &[BodyId],
    assembly: bool,
) -> Result<f64, DocError> {
    if !assembly {
        return bodies_extent(m, bodies);
    }
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    let mut grow = |bb: &k::Bounds, t: Option<&k::Rigid>| {
        for c in 0..8 {
            let p = [0, 1, 2].map(|i| {
                if c >> i & 1 == 1 {
                    bb.max[i]
                } else {
                    bb.min[i]
                }
            });
            let p = t.map_or(p, |t| t.point(p));
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
    };
    let ev = m.evaluation();
    let brep_bounds = |b: &BodyId| -> Result<k::Bounds, DocError> {
        let eb = ev
            .body_by_id(*b)
            .ok_or_else(|| bad(format!("{b} is not an evaluated body")))?;
        let g = eb.geometry.brep().ok_or_else(|| {
            DocError::ProductLimit(format!(
                "{b} is a mesh body; drawings need exact (B-rep) bodies"
            ))
        })?;
        k::bounds(g).map_err(|e| bad(e.to_string()))
    };
    for b in bodies {
        grow(&brep_bounds(b)?, None);
    }
    for o in &ev.occurrences {
        for b in &o.bodies {
            grow(&brep_bounds(b)?, Some(&o.transform));
        }
    }
    if !lo[0].is_finite() {
        return Err(bad(
            "the assembly has no evaluated occurrences to draw".into()
        ));
    }
    Ok((0..3).map(|i| hi[i] - lo[i]).fold(0.0, f64::max))
}

/// Default base-view position: room for the view from above and from the
/// right on the side the convention puts them.
pub fn default_position(sheet: SheetSize, p: Projection) -> [f64; 2] {
    let (w, h) = sheet.mm();
    match p {
        Projection::ThirdAngle => [w * 0.28, h * 0.36],
        Projection::FirstAngle => [w * 0.56, h * 0.64],
    }
}

/// Default scale for bodies of largest extent `ext` on `sheet`.
pub fn default_scale(sheet: SheetSize, ext: f64) -> f64 {
    let (w, h) = sheet.mm();
    dw::fit_scale(ext, (w * 0.3).min(h * 0.28))
}

fn add_view(
    d: &mut Document,
    id: DrawingId,
    spec: &ViewSpec,
    auto_scale: Option<f64>,
) -> Result<ViewId, DocError> {
    let dr = d.drawing(id)?.clone();
    let kind = match spec {
        ViewSpec::Base {
            bodies,
            orientation,
            scale,
            position,
            assembly,
        } => {
            for b in bodies {
                d.body(*b)?;
            }
            if *assembly && d.assembly.occurrences.is_empty() {
                return Err(bad(
                    "the document has no component occurrences to draw".into()
                ));
            }
            ViewKind::Base {
                bodies: bodies.clone(),
                orientation: *orientation,
                scale: scale.or(auto_scale).unwrap_or(1.0),
                position: position.unwrap_or_else(|| default_position(dr.sheet, dr.projection)),
                assembly: *assembly,
            }
        }
        ViewSpec::Projected { parent, from } => {
            dr.view(*parent)?;
            ViewKind::Projected {
                parent: *parent,
                from: *from,
            }
        }
        ViewSpec::Section {
            parent,
            from,
            offset,
            label,
        } => {
            dr.view(*parent)?;
            ViewKind::Section {
                parent: *parent,
                from: *from,
                offset: *offset,
                label: label.clone(),
            }
        }
        ViewSpec::Detail {
            parent,
            center,
            radius,
            scale,
            position,
            label,
        } => {
            dr.view(*parent)?;
            let (_, ps, _) = dr.resolve(*parent)?;
            let (w, h) = dr.sheet.mm();
            ViewKind::Detail {
                parent: *parent,
                center: *center,
                radius: *radius,
                scale: scale.unwrap_or(ps * 2.0),
                position: position.unwrap_or([w * 0.8, h * 0.45]),
                label: label.clone(),
            }
        }
    };
    let dr = d.drawing_mut(id)?;
    let vid = ViewId(dr.next_view.max(1));
    dr.next_view = vid.0 + 1;
    dr.views.push(DrawingView {
        id: vid,
        kind,
        annotations: Vec::new(),
    });
    dr.validate()?;
    Ok(vid)
}

impl Session {
    /// Drawing commands; `Ok(false)` when `cmd` is not one.
    pub(crate) fn run_drawing(
        &mut self,
        cmd: &Command,
        r: &mut Response,
    ) -> Result<bool, DocError> {
        match cmd {
            Command::CreateDrawing {
                name,
                sheet,
                projection,
                title,
                part_number,
                material,
                bodies,
                standard_views,
                assembly,
            } => {
                let m = self.model_mut()?;
                let sheet = sheet.unwrap_or_default();
                let assembly = *assembly;
                let auto = if bodies.is_empty() && !assembly {
                    None
                } else {
                    Some(default_scale(sheet, view_extent(m, bodies, assembly)?))
                };
                let (name, projection) = (name.clone(), projection.unwrap_or_default());
                let (title, pn, mat) = (title.clone(), part_number.clone(), material.clone());
                let (bodies, standard) = (bodies.clone(), standard_views.unwrap_or(true));
                let id = m.transact("Create drawing", |d| {
                    let id = d.next_drawing_id();
                    let n = name.unwrap_or_else(|| format!("Drawing{}", id.0));
                    let mut dr = Drawing::new(id, &n);
                    dr.sheet = sheet;
                    dr.projection = projection;
                    if let Some(t) = title {
                        dr.title.title = t;
                    }
                    dr.title.part_number = pn.unwrap_or_default();
                    dr.title.material = mat.unwrap_or_default();
                    dr.validate()?;
                    d.drawings.push(dr);
                    if !bodies.is_empty() || assembly {
                        let base = add_view(
                            d,
                            id,
                            &ViewSpec::Base {
                                bodies,
                                orientation: Orientation::Front,
                                scale: None,
                                position: None,
                                assembly,
                            },
                            auto,
                        )?;
                        if standard {
                            for from in [Side::Above, Side::Right] {
                                add_view(d, id, &ViewSpec::Projected { parent: base, from }, None)?;
                            }
                        }
                        if assembly {
                            let dr = d.drawing_mut(id)?;
                            dr.view_mut(base)?.annotations.push(Annotation::Balloons);
                            dr.bom = Some(dw::BomTable::default());
                        }
                    }
                    Ok(id)
                })?;
                r.drawing = Some(id);
            }
            Command::EditDrawing {
                drawing,
                name,
                sheet,
                projection,
                title,
                part_number,
                material,
                auto_dimensions,
            } => {
                let id = *drawing;
                let (name, title, part_number, material) = (
                    name.clone(),
                    title.clone(),
                    part_number.clone(),
                    material.clone(),
                );
                let (sheet, projection, auto_dimensions) = (*sheet, *projection, *auto_dimensions);
                self.model_mut()?
                    .transact(&format!("Edit drawing {id}"), |d| {
                        let dr = d.drawing_mut(id)?;
                        if let Some(n) = name {
                            dr.name = n;
                        }
                        if let Some(s) = sheet {
                            dr.sheet = s;
                        }
                        if let Some(p) = projection {
                            dr.projection = p;
                        }
                        if let Some(t) = title {
                            dr.title.title = t;
                        }
                        if let Some(p) = part_number {
                            dr.title.part_number = p;
                        }
                        if let Some(x) = material {
                            dr.title.material = x;
                        }
                        if let Some(a) = auto_dimensions {
                            dr.auto_dimensions = a;
                        }
                        dr.validate()
                    })?;
                r.drawing = Some(id);
            }
            Command::DeleteDrawing { drawing } => {
                let id = *drawing;
                self.model_mut()?
                    .transact(&format!("Delete drawing {id}"), |d| {
                        d.drawing(id)?;
                        d.drawings.retain(|x| x.id != id);
                        Ok(())
                    })?;
            }
            Command::AddDrawingView { drawing, view } => {
                let id = *drawing;
                let m = self.model_mut()?;
                let auto = match view {
                    ViewSpec::Base {
                        bodies,
                        scale: None,
                        assembly,
                        ..
                    } => {
                        let sheet = m.document().drawing(id)?.sheet;
                        Some(default_scale(sheet, view_extent(m, bodies, *assembly)?))
                    }
                    ViewSpec::Base {
                        bodies, assembly, ..
                    } => {
                        view_extent(m, bodies, *assembly)?;
                        None
                    }
                    ViewSpec::Projected { .. }
                    | ViewSpec::Section { .. }
                    | ViewSpec::Detail { .. } => None,
                };
                let spec = view.clone();
                let vid = m.transact(&format!("Add view to {id}"), |d| {
                    add_view(d, id, &spec, auto)
                })?;
                r.drawing = Some(id);
                r.view = Some(vid);
            }
            Command::EditDrawingView {
                drawing,
                view,
                scale,
                position,
                orientation,
            } => {
                let (id, vid, scale, position, orientation) =
                    (*drawing, *view, *scale, *position, *orientation);
                self.model_mut()?.transact(&format!("Edit view {vid} of {id}"), |d| {
                    let dr = d.drawing_mut(id)?;
                    match &mut dr.view_mut(vid)?.kind {
                        ViewKind::Base {
                            scale: s,
                            position: p,
                            orientation: o,
                            ..
                        } => {
                            if let Some(x) = scale {
                                *s = x;
                            }
                            if let Some(x) = position {
                                *p = x;
                            }
                            if let Some(x) = orientation {
                                *o = x;
                            }
                        }
                        ViewKind::Detail {
                            scale: s,
                            position: p,
                            ..
                        } => {
                            if orientation.is_some() {
                                return Err(bad(format!(
                                    "{vid} is a detail view: its orientation follows its parent"
                                )));
                            }
                            if let Some(x) = scale {
                                *s = x;
                            }
                            if let Some(x) = position {
                                *p = x;
                            }
                        }
                        ViewKind::Projected { .. } | ViewKind::Section { .. } => {
                            if scale.is_some() || position.is_some() || orientation.is_some() {
                                return Err(bad(format!(
                                    "{vid} is a projected or section view: its scale, position and orientation follow its parent"
                                )));
                            }
                        }
                    }
                    dr.validate()
                })?;
                r.drawing = Some(id);
                r.view = Some(vid);
            }
            Command::RemoveDrawingView { drawing, view } => {
                let (id, vid) = (*drawing, *view);
                self.model_mut()?
                    .transact(&format!("Remove view {vid} from {id}"), |d| {
                        let dr = d.drawing_mut(id)?;
                        dr.view(vid)?;
                        let children: Vec<String> = dr
                            .views
                            .iter()
                            .filter(|v| v.kind.parent_id() == Some(vid))
                            .map(|v| v.id.to_string())
                            .collect();
                        if !children.is_empty() {
                            return Err(bad(format!(
                                "{vid} has projected views ({}); remove them first",
                                children.join(", ")
                            )));
                        }
                        dr.views.retain(|v| v.id != vid);
                        Ok(())
                    })?;
                r.drawing = Some(id);
            }
            Command::AddHoleCallout {
                drawing,
                view,
                feature,
            } => {
                let (id, vid, feature) = (*drawing, *view, *feature);
                self.model_mut()?
                    .transact(&format!("Add hole callout to {vid}"), |d| {
                        let f = d.feature(feature)?;
                        if !matches!(f.kind, FeatureKind::Hole { .. }) {
                            return Err(bad(format!("{feature} is not a hole")));
                        }
                        let v = d.drawing_mut(id)?.view_mut(vid)?;
                        let a = Annotation::HoleCallout { feature };
                        if v.annotations.contains(&a) {
                            return Err(bad(format!("{vid} already has a callout of {feature}")));
                        }
                        v.annotations.push(a);
                        Ok(())
                    })?;
                r.drawing = Some(id);
                r.view = Some(vid);
            }
            Command::RemoveHoleCallout {
                drawing,
                view,
                feature,
            } => {
                let (id, vid, feature) = (*drawing, *view, *feature);
                self.model_mut()?
                    .transact(&format!("Remove hole callout from {vid}"), |d| {
                        let v = d.drawing_mut(id)?.view_mut(vid)?;
                        let a = Annotation::HoleCallout { feature };
                        if !v.annotations.contains(&a) {
                            return Err(bad(format!("{vid} has no callout of {feature}")));
                        }
                        v.annotations.retain(|x| *x != a);
                        Ok(())
                    })?;
            }
            Command::AddDrawingDimension {
                drawing,
                view,
                a,
                b,
                tolerance,
            } => {
                let (id, vid, a, b, tolerance) = (*drawing, *view, *a, *b, *tolerance);
                let m = self.model_mut()?;
                // The references must resolve now (an existing dimension that
                // later stops resolving is reported as dangling instead).
                let probe = {
                    let mut d = m.document().clone();
                    let v = d.drawing_mut(id)?.view_mut(vid)?;
                    v.annotations = vec![Annotation::Dimension { a, b, tolerance }];
                    d.drawing(id)?.validate()?;
                    let out = dw::render(&d, m.evaluation(), id)?;
                    out.views
                        .into_iter()
                        .find(|x| x.id == vid)
                        .and_then(|x| x.dimensions.into_iter().find_map(|d| d.dangling))
                };
                if let Some(why) = probe {
                    return Err(bad(format!("dimension not added: {why}")));
                }
                m.transact(&format!("Add dimension to {vid}"), |d| {
                    let dr = d.drawing_mut(id)?;
                    dr.view_mut(vid)?
                        .annotations
                        .push(Annotation::Dimension { a, b, tolerance });
                    dr.validate()
                })?;
                r.drawing = Some(id);
                r.view = Some(vid);
            }
            Command::SetDimensionTolerance {
                drawing,
                view,
                index,
                tolerance,
            } => {
                let (id, vid, index, tol) = (*drawing, *view, *index, *tolerance);
                self.model_mut()?
                    .transact(&format!("Set tolerance in {vid}"), |d| {
                        let dr = d.drawing_mut(id)?;
                        match dr.view_mut(vid)?.annotations.get_mut(index) {
                            Some(Annotation::Dimension { tolerance, .. }) => *tolerance = tol,
                            _ => {
                                return Err(bad(format!(
                                    "{vid} has no dimension at index {index}"
                                )));
                            }
                        }
                        dr.validate()
                    })?;
                r.drawing = Some(id);
                r.view = Some(vid);
            }
            Command::RemoveDrawingAnnotation {
                drawing,
                view,
                index,
            } => {
                let (id, vid, index) = (*drawing, *view, *index);
                self.model_mut()?
                    .transact(&format!("Remove annotation from {vid}"), |d| {
                        let v = d.drawing_mut(id)?.view_mut(vid)?;
                        if index >= v.annotations.len() {
                            return Err(bad(format!("{vid} has no annotation {index}")));
                        }
                        v.annotations.remove(index);
                        Ok(())
                    })?;
                r.drawing = Some(id);
            }
            Command::SetBalloons {
                drawing,
                view,
                show,
            } => {
                let (id, vid, show) = (*drawing, *view, *show);
                self.model_mut()?
                    .transact(&format!("Balloons in {vid}"), |d| {
                        let v = d.drawing_mut(id)?.view_mut(vid)?;
                        let has = v.annotations.contains(&Annotation::Balloons);
                        match (show, has) {
                            (true, false) => v.annotations.push(Annotation::Balloons),
                            (false, true) => v.annotations.retain(|a| *a != Annotation::Balloons),
                            _ => {
                                return Err(bad(format!(
                                    "{vid} {} balloons",
                                    if has { "already has" } else { "has no" }
                                )));
                            }
                        }
                        Ok(())
                    })?;
                r.drawing = Some(id);
            }
            Command::SetBomTable {
                drawing,
                show,
                position,
            } => {
                let (id, show, position) = (*drawing, *show, *position);
                self.model_mut()?
                    .transact(&format!("BOM table of {id}"), |d| {
                        let dr = d.drawing_mut(id)?;
                        dr.bom = show.then_some(dw::BomTable { position });
                        dr.validate()
                    })?;
                r.drawing = Some(id);
            }
            Command::AddDrawingNote {
                drawing,
                text,
                position,
            } => {
                let (id, note) = (
                    *drawing,
                    dw::Note {
                        text: text.clone(),
                        position: *position,
                    },
                );
                self.model_mut()?
                    .transact(&format!("Add note to {id}"), |d| {
                        let dr = d.drawing_mut(id)?;
                        dr.notes.push(note);
                        dr.validate()
                    })?;
                r.drawing = Some(id);
            }
            Command::RemoveDrawingNote { drawing, index } => {
                let (id, index) = (*drawing, *index);
                self.model_mut()?
                    .transact(&format!("Remove note from {id}"), |d| {
                        let dr = d.drawing_mut(id)?;
                        if index >= dr.notes.len() {
                            return Err(bad(format!("{id} has no note {}", index + 1)));
                        }
                        dr.notes.remove(index);
                        Ok(())
                    })?;
                r.drawing = Some(id);
            }
            Command::ExportDrawing {
                drawing,
                path,
                format,
            } => {
                let format = match format {
                    Some(f) => *f,
                    None => match path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(str::to_ascii_lowercase)
                        .as_deref()
                    {
                        Some("svg") => DrawingFormat::Svg,
                        Some("pdf") => DrawingFormat::Pdf,
                        Some("dxf") => DrawingFormat::Dxf,
                        _ => {
                            return Err(bad(format!(
                                "give a format (svg, pdf or dxf) for {}",
                                path.display()
                            )));
                        }
                    },
                };
                let m = self.model_mut()?;
                let out = dw::render(m.document(), m.evaluation(), *drawing)?;
                let bytes = match format {
                    DrawingFormat::Svg => {
                        rcad_exchange::drawing::write_svg(&out.sheet).into_bytes()
                    }
                    DrawingFormat::Pdf => rcad_exchange::pdf::write_pdf(&out.sheet),
                    DrawingFormat::Dxf => {
                        rcad_exchange::drawing_dxf::write_dxf(&out.sheet).into_bytes()
                    }
                };
                write_atomic(path, &bytes)?;
                r.message = Some(format!(
                    "wrote {} ({} bytes, {:?}, {} views{})",
                    path.display(),
                    bytes.len(),
                    format,
                    out.views.len(),
                    if out.issues.is_empty() {
                        String::new()
                    } else {
                        format!(", {} issue(s): {}", out.issues.len(), out.issues.join("; "))
                    }
                ));
                r.drawing = Some(*drawing);
                r.drawing_report = Some(DrawingReport {
                    drawing: *drawing,
                    format,
                    bytes: bytes.len(),
                    views: out.views,
                    issues: out.issues,
                    sample_spacing: out.sample_spacing,
                    chord: out.chord,
                    bom: out.bom,
                });
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// Feature ids of Hole features (for UI pickers).
pub fn hole_features(d: &Document) -> Vec<(FeatureId, String)> {
    d.features
        .iter()
        .filter(|f| matches!(f.kind, FeatureKind::Hole { .. }))
        .map(|f| (f.id, f.name.clone()))
        .collect()
}
