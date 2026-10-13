//! Run 7 commands: sheet metal (base face, edge flanges, sheet rule, flat
//! pattern display and DXF) and surfaces (import, analysis, stitch, fill,
//! thicken). docs/SHEET_METAL.md, docs/SURFACES.md.
use crate::{Command, Response, Session};
use rcad_document::sheet::{ReliefShape, SheetRule};
use rcad_document::{BodyId, DocError, Document, FeatureId, FeatureKind};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn default_k() -> f64 {
    0.44
}
fn default_angle() -> f64 {
    90.0
}
fn default_tol() -> f64 {
    1e-6
}
fn yes() -> bool {
    true
}

/// `sheet_base`: a planar sheet plate from the closed profile of `sketch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SheetBaseArgs {
    #[serde(default)]
    pub name: Option<String>,
    pub sketch: FeatureId,
    pub thickness: f64,
    /// Inside bend radius for every bend of the body.
    pub radius: f64,
    #[serde(default = "default_k")]
    pub k_factor: f64,
    #[serde(default)]
    pub relief: ReliefShape,
    /// Default: the thickness.
    #[serde(default)]
    pub relief_width: Option<f64>,
    /// Default: the thickness.
    #[serde(default)]
    pub relief_depth: Option<f64>,
    /// Plate on the −normal side of the sketch.
    #[serde(default)]
    pub flip: bool,
    #[serde(default)]
    pub body_name: Option<String>,
}

/// `sheet_flange`: a straight edge flange on base-profile line `edge`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SheetFlangeArgs {
    #[serde(default)]
    pub name: Option<String>,
    /// The sheet base feature.
    pub base: FeatureId,
    /// Sketch line entity id of the base profile.
    pub edge: u32,
    #[serde(default = "default_angle")]
    pub angle_deg: f64,
    /// Straight length beyond the bend (tangent line to flange end).
    pub length: f64,
    #[serde(default)]
    pub down: bool,
    #[serde(default)]
    pub start_offset: f64,
    #[serde(default)]
    pub end_offset: f64,
}

/// `edit_sheet_rule`: change any part of the rule of `base` atomically.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SheetRuleArgs {
    pub base: FeatureId,
    #[serde(default)]
    pub thickness: Option<f64>,
    #[serde(default)]
    pub radius: Option<f64>,
    #[serde(default)]
    pub k_factor: Option<f64>,
    #[serde(default)]
    pub relief: Option<ReliefShape>,
    #[serde(default)]
    pub relief_width: Option<f64>,
    #[serde(default)]
    pub relief_depth: Option<f64>,
}

/// `sheet_flat_state`: show the flat pattern (creating its body on first use)
/// or the folded body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SheetFlatArgs {
    pub base: FeatureId,
    #[serde(default = "yes")]
    pub flat: bool,
}

/// `import_surfaces`: the surface models of a STEP file as one surface body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportSurfacesArgs {
    pub path: PathBuf,
    #[serde(default)]
    pub units_to_mm: Option<f64>,
}

/// `analyze_surface`: free edges, gaps (within `search` mm), boundary loops
/// and orientation conflicts of a body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalyzeSurfaceArgs {
    pub body: BodyId,
    #[serde(default = "default_search")]
    pub search: f64,
}
fn default_search() -> f64 {
    0.1
}

/// `stitch_surface` / `fill_surface`: a new body from surface body `body`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceRepairArgs {
    pub body: BodyId,
    #[serde(default = "default_tol")]
    pub tolerance: f64,
    #[serde(default)]
    pub body_name: Option<String>,
    /// Hide the source body (it is kept).
    #[serde(default = "yes")]
    pub hide_source: bool,
}

/// `thicken_surface`: a solid from the single face of surface body `body`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThickenArgs {
    pub body: BodyId,
    pub distance: f64,
    #[serde(default)]
    pub reverse: bool,
    #[serde(default)]
    pub body_name: Option<String>,
    #[serde(default = "yes")]
    pub hide_source: bool,
}

fn new_body(d: &mut Document, name: &Option<String>, default: &str) -> Result<BodyId, DocError> {
    let n = match name {
        Some(n) => crate::valid_name(n, "body")?,
        None => crate::unique_body_name(d, default),
    };
    if d.bodies.iter().any(|b| b.name == n) {
        return Err(DocError::Validation(format!(
            "another body is already named '{n}'"
        )));
    }
    Ok(d.add_body(n))
}

/// The sheet base feature `base` and its body.
fn sheet_base(d: &Document, base: FeatureId) -> Result<BodyId, DocError> {
    match &d.feature(base)?.kind {
        FeatureKind::SheetBase { body, .. } => Ok(*body),
        _ => Err(DocError::Validation(format!(
            "{base} is not a sheet-metal base feature"
        ))),
    }
}

/// The SheetFlat feature of `base`, if any (feature, body).
pub fn flat_of(d: &Document, base: FeatureId) -> Option<(FeatureId, BodyId)> {
    d.features.iter().find_map(|f| match f.kind {
        FeatureKind::SheetFlat { base: b, body } if b == base => Some((f.id, body)),
        _ => None,
    })
}

/// The sheet base a sheet feature belongs to.
pub fn base_of(d: &Document, feature: FeatureId) -> Option<FeatureId> {
    match d.feature(feature).ok()?.kind {
        FeatureKind::SheetBase { .. } => Some(feature),
        FeatureKind::SheetFlange { base, .. } | FeatureKind::SheetFlat { base, .. } => Some(base),
        _ => None,
    }
}

/// The current sheet model of `base` rebuilt from the document (for the
/// flat-pattern export): regenerates the history up to the end and reads
/// the base's sketch, applying every flange of its body in order.
fn current_build(
    s: &mut Session,
    base: FeatureId,
) -> Result<(rcad_document::sheet::SheetBuild, SheetRule), DocError> {
    let m = s.model_mut()?;
    let doc = m.document().clone();
    let ev = m.evaluation();
    let FeatureKind::SheetBase {
        sketch,
        rule,
        flip,
        body,
    } = doc.feature(base)?.kind.clone()
    else {
        return Err(DocError::Validation(format!("{base} is not a sheet base")));
    };
    let Some(eb) = ev.body_by_id(body) else {
        return Err(DocError::Validation(format!(
            "{base}: the sheet body is not evaluated (suppressed or failed)"
        )));
    };
    // Review S3: the flat pattern comes from the sheet definition; a later
    // non-sheet edit of the body (hole, cut, ...) would be silently missing.
    let last_sheet = doc
        .features
        .iter()
        .filter(|f| !f.suppressed && !doc.is_rolled_back(f.id))
        .filter(|f| {
            f.id == base || matches!(f.kind, FeatureKind::SheetFlange { base: b, .. } if b == base)
        })
        .map(|f| f.id)
        .next_back();
    if Some(eb.last_feature) != last_sheet {
        return Err(DocError::ProductLimit(format!(
            "{base}: the sheet body was changed by {} after its last sheet feature; the flat pattern would not show that edit (unfolding arbitrary edits is not supported)",
            eb.last_feature
        )));
    }
    let res = ev
        .features
        .iter()
        .find(|f| f.id == sketch)
        .ok_or_else(|| DocError::Validation(format!("{sketch} is not evaluated")))?;
    let solved = res
        .solved_sketch
        .clone()
        .ok_or_else(|| DocError::Validation(format!("{sketch} is not a sketch")))?;
    let frame = res
        .frame
        .ok_or_else(|| DocError::Validation(format!("{sketch} has no frame")))?;
    let region = solved
        .profile_region()
        .map_err(|e| DocError::Validation(format!("{sketch}: {e}")))?;
    let mut model =
        rcad_document::sheet::base_model(base, body, frame, flip, &rule, &region, &solved)?;
    for f in &doc.features {
        if doc.is_rolled_back(f.id) || f.suppressed {
            continue;
        }
        if let FeatureKind::SheetFlange { base: b, .. } = f.kind
            && b == base
        {
            model
                .flanges
                .extend(rcad_document::sheet::flange_def(f.id, &f.kind));
        }
    }
    let built = rcad_document::sheet::build(&model)?;
    Ok((built, rule))
}

impl Session {
    /// Run-7 sheet-metal and surface commands; false when `cmd` is not one.
    pub(crate) fn run_sheet_surface(
        &mut self,
        cmd: &Command,
        r: &mut Response,
    ) -> Result<bool, DocError> {
        match cmd {
            Command::SheetBase(a) => {
                let a = a.clone();
                let rule = SheetRule {
                    thickness: a.thickness,
                    radius: a.radius,
                    k_factor: a.k_factor,
                    relief: a.relief,
                    relief_width: a.relief_width.unwrap_or(a.thickness),
                    relief_depth: a.relief_depth.unwrap_or(a.thickness),
                };
                let id = self.model_mut()?.transact("Sheet base face", |d| {
                    let body = new_body(d, &a.body_name, "Sheet")?;
                    let n = a.name.clone().unwrap_or_else(|| d.default_name("Sheet"));
                    Ok(d.push(
                        n,
                        FeatureKind::SheetBase {
                            sketch: a.sketch,
                            rule,
                            flip: a.flip,
                            body,
                        },
                    ))
                })?;
                r.feature = Some(id);
            }
            Command::SheetFlange(a) => {
                let a = a.clone();
                if !(a.angle_deg > 0.0 && a.angle_deg <= 180.0) {
                    return Err(DocError::Validation(format!(
                        "flange angle must be in (0, 180] degrees (180 = open hem), got {}",
                        a.angle_deg
                    )));
                }
                let id = self.model_mut()?.transact("Edge flange", |d| {
                    let body = sheet_base(d, a.base)?;
                    let n = a.name.clone().unwrap_or_else(|| d.default_name("Flange"));
                    let flat = flat_of(d, a.base).map(|(f, _)| f);
                    let id = d.push(
                        n,
                        FeatureKind::SheetFlange {
                            base: a.base,
                            body,
                            edge: a.edge,
                            angle: a.angle_deg.to_radians(),
                            length: a.length,
                            down: a.down,
                            start_offset: a.start_offset,
                            end_offset: a.end_offset,
                        },
                    );
                    // Review #5: the flat pattern must include every flange of
                    // its body, so a new flange goes before an existing flat.
                    if let Some(f) = flat {
                        d.move_feature(id, Some(f))?;
                    }
                    Ok(id)
                })?;
                r.feature = Some(id);
            }
            Command::EditSheetRule(a) => {
                let a = a.clone();
                self.model_mut()?.transact("Edit sheet rule", |d| {
                    let f = d.feature_mut(a.base)?;
                    let FeatureKind::SheetBase { rule, .. } = &mut f.kind else {
                        return Err(DocError::Validation(format!(
                            "{} is not a sheet-metal base feature",
                            a.base
                        )));
                    };
                    let mut nr = rule.clone();
                    if let Some(v) = a.thickness {
                        nr.thickness = v;
                    }
                    if let Some(v) = a.radius {
                        nr.radius = v;
                    }
                    if let Some(v) = a.k_factor {
                        nr.k_factor = v;
                    }
                    if let Some(v) = a.relief {
                        nr.relief = v;
                    }
                    if let Some(v) = a.relief_width {
                        nr.relief_width = v;
                    }
                    if let Some(v) = a.relief_depth {
                        nr.relief_depth = v;
                    }
                    nr.validate(a.base)?;
                    *rule = nr;
                    Ok(())
                })?;
                r.feature = Some(a.base);
            }
            Command::SheetFlatState(a) => {
                let a = a.clone();
                let id = self.model_mut()?.transact(
                    if a.flat {
                        "Show flat pattern"
                    } else {
                        "Show folded"
                    },
                    |d| {
                        let folded = sheet_base(d, a.base)?;
                        let (fid, flat) = match flat_of(d, a.base) {
                            Some(x) => x,
                            None => {
                                let fname = format!(
                                    "{} (flat)",
                                    d.body(folded).map(|b| b.name.clone()).unwrap_or_default()
                                );
                                let body = new_body(d, &None, &fname)?;
                                let n = d.default_name("Flat pattern");
                                let fid = d.push(n, FeatureKind::SheetFlat { base: a.base, body });
                                (fid, body)
                            }
                        };
                        d.body_mut(flat)?.visible = a.flat;
                        d.body_mut(folded)?.visible = !a.flat;
                        Ok(fid)
                    },
                )?;
                r.feature = Some(id);
            }
            Command::ExportFlatPattern { feature, path } => {
                let Some(base) = self
                    .model
                    .as_ref()
                    .and_then(|m| base_of(m.document(), *feature))
                else {
                    return Ok(false); // the run-6 contour flange path
                };
                let (built, rule) = current_build(self, base)?;
                let text = rcad_document::sheet::flat_dxf(&built);
                rcad_document::write_atomic(path, text.as_bytes())?;
                let (lo, hi) = built.flat_extent;
                r.feature = Some(base);
                r.message = Some(format!(
                    "flat pattern {:.6} x {:.6} mm (t {} mm), {} bend line(s): {}",
                    hi[0] - lo[0],
                    hi[1] - lo[1],
                    rule.thickness,
                    built.bends.len(),
                    built
                        .bends
                        .iter()
                        .map(|b| format!(
                            "{} {} {:.1}°",
                            b.flange,
                            if b.up { "up" } else { "down" },
                            b.angle_deg
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            Command::ImportSurfaces(a) => {
                let bytes = crate::read_import(&a.path)?;
                let unit = a.units_to_mm;
                // Parse once up front for an early, precise error.
                let probe = rcad_document::import::step_opts(&bytes, unit, true)?;
                let Some(surf) = probe.surfaces else {
                    return Err(DocError::Validation(format!(
                        "{}: no surface model (SHELL_BASED_SURFACE_MODEL) in the file",
                        a.path.display()
                    )));
                };
                let file = crate::file_name(&a.path);
                let stem = crate::file_stem(&a.path);
                let label = surf.name.clone().unwrap_or(format!("{stem} surfaces"));
                let id = self
                    .model_mut()?
                    .transact(&format!("Import surfaces {file}"), |d| {
                        let blob = d.add_blob(&file, &bytes);
                        let body = new_body(d, &None, &label)?;
                        let n = d.default_name("Surfaces");
                        Ok(d.push(
                            n,
                            FeatureKind::ImportSurfaces {
                                blob,
                                unit_override_to_mm: unit,
                                body,
                            },
                        ))
                    })?;
                r.feature = Some(id);
                r.message = Some(format!(
                    "{} shell(s), {} open, {} face(s): a surface body (no volume)",
                    surf.shells, surf.open_shells, surf.faces
                ));
            }
            Command::AnalyzeSurface(a) => {
                let m = self.model_mut()?;
                let b = m
                    .evaluation()
                    .body_by_id(a.body)
                    .and_then(|b| b.geometry.brep().cloned())
                    .ok_or_else(|| {
                        DocError::Validation(format!("body {} is not an evaluated B-rep", a.body))
                    })?;
                let rep = rcad_kernel::analyze(&b, a.search)
                    .map_err(|e| DocError::Validation(format!("analyse body {}: {e}", a.body)))?;
                r.message = Some(format!(
                    "{}: {} shell(s), {} face(s), {} free edge(s), {} gap(s) within {} mm{}, {} boundary loop(s), {} orientation conflict(s), {} non-manifold edge(s)",
                    if rep.closed {
                        "closed solid"
                    } else {
                        "open shell"
                    },
                    rep.shells,
                    rep.faces,
                    rep.free_edges.len(),
                    rep.gaps_total,
                    rep.search,
                    rep.gaps
                        .iter()
                        .map(|g| g.size)
                        .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))))
                        .map(|v| format!(" (largest {v:.6} mm)"))
                        .unwrap_or_default(),
                    rep.boundary_loops.len(),
                    rep.orientation_conflicts.len(),
                    rep.non_manifold_edges
                ));
                r.surface_analysis = Some(rep);
            }
            Command::StitchSurface(a) | Command::FillSurface(a) => {
                let stitch = matches!(cmd, Command::StitchSurface(_));
                let a = a.clone();
                let id = self.model_mut()?.transact(
                    if stitch {
                        "Stitch surfaces"
                    } else {
                        "Fill surface"
                    },
                    |d| {
                        let src = d.body(a.body)?.name.clone();
                        let body = new_body(
                            d,
                            &a.body_name,
                            &format!("{src} {}", if stitch { "stitched" } else { "filled" }),
                        )?;
                        if a.hide_source {
                            d.body_mut(a.body)?.visible = false;
                        }
                        let n = d.default_name(if stitch { "Stitch" } else { "Fill" });
                        let kind = if stitch {
                            FeatureKind::SurfaceStitch {
                                source: a.body,
                                tolerance: a.tolerance,
                                body,
                            }
                        } else {
                            FeatureKind::SurfaceFill {
                                source: a.body,
                                tolerance: a.tolerance,
                                body,
                            }
                        };
                        Ok(d.push(n, kind))
                    },
                )?;
                r.feature = Some(id);
                r.message = self.feature_notes(id);
            }
            Command::ThickenSurface(a) => {
                let a = a.clone();
                let id = self.model_mut()?.transact("Thicken", |d| {
                    let src = d.body(a.body)?.name.clone();
                    let body = new_body(d, &a.body_name, &format!("{src} thickened"))?;
                    if a.hide_source {
                        d.body_mut(a.body)?.visible = false;
                    }
                    let n = d.default_name("Thicken");
                    Ok(d.push(
                        n,
                        FeatureKind::Thicken {
                            source: a.body,
                            distance: a.distance,
                            reverse: a.reverse,
                            body,
                        },
                    ))
                })?;
                r.feature = Some(id);
                r.message = self.feature_notes(id);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn feature_notes(&self, id: FeatureId) -> Option<String> {
        let m = self.model.as_ref()?;
        let f = m.evaluation().features.iter().find(|f| f.id == id)?;
        Some(f.notes.join("; "))
    }
}
