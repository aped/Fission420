//! STEP (ISO 10303-21, AP203/AP214/AP242 B-rep subset) import into exact Truck solids.
//!
//! Pipeline: bounded pre-scan (bytes, entities, nesting) -> ruststep parser (via
//! truck-stepio's re-export) -> entity index -> units -> per MANIFOLD_SOLID_BREP:
//! exact geometry conversion (`geom`), topology with periodic-face splitting
//! (`topo`), assembly placement (`assembly`) -> `Solid::extract` -> closed-shell and
//! outward-orientation checks -> planar face table.
//!
//! All-or-nothing: any unsupported or inconsistent entity aborts the import with an
//! error naming the entity type and `#id`. See docs/formats/step.md.
// `!(x > 0.0)` is used deliberately throughout: it also rejects NaN from file data.
#![allow(clippy::neg_cmp_op_on_partial_ord)]
mod assembly;
mod ast;
pub(crate) mod geom;
mod topo;
mod units;

/// Deepest STEP product structure read (usages per path, root included).
pub use assembly::MAX_STRUCTURE_DEPTH as STEP_MAX_ASSEMBLY_DEPTH;

use crate::{Body, Frame, KernelError, Result, guarded};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use truck_modeling::*;
use truck_topology::compress::CompressedSolid;

/// Internal error message (converted to `KernelError::InvalidInput` at the boundary).
#[derive(Debug)]
pub(crate) struct StepError(pub String);

pub(crate) fn fail(msg: String) -> StepError {
    StepError(msg)
}

/// Import limits and unit policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepImportOptions {
    /// Largest accepted file size in bytes (default 64 MiB).
    pub max_bytes: usize,
    /// Largest accepted number of entity instances (default 2,000,000).
    pub max_entities: usize,
    /// Millimetres per file length unit, used only when the file declares no length
    /// unit. Without it such files are refused ("units required").
    pub unit_override_to_mm: Option<f64>,
    /// Also read the SHELL_BASED_SURFACE_MODELs into
    /// [`StepImport::surfaces`] (failures then abort the import). Off: they
    /// are ignored with a warning, so a bad surface
    /// model never breaks a solid import.
    #[serde(default)]
    pub surfaces: bool,
    /// Read the product structure into
    /// [`StepImport::assembly`] (`import_step_assembly`). Structure errors are
    /// then fatal, and when the file has one the flattened `solids` are not
    /// built (nor limited by their instance count): every product's
    /// definition solids are read once. Off (plain import and regeneration
    /// of an embedded file): the structure is not read at all, so it can
    /// neither fail nor slow the import.
    #[serde(default)]
    pub assembly: bool,
}

impl Default for StepImportOptions {
    fn default() -> Self {
        StepImportOptions {
            max_bytes: 64 << 20,
            max_entities: 2_000_000,
            unit_override_to_mm: None,
            surfaces: false,
            assembly: false,
        }
    }
}

/// Length unit of the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LengthUnit {
    /// e.g. "millimetre", "metre", "inch".
    pub name: String,
    /// Millimetres per file unit; all geometry was multiplied by this.
    pub to_mm: f64,
    /// False when the file declared no length unit and `unit_override_to_mm` was used.
    pub declared: bool,
}

/// Counts of what was read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StepStats {
    /// Entity instances in the DATA section.
    pub entities: usize,
    pub solids: usize,
    /// ADVANCED_FACE / FACE_SURFACE entities used by the imported solids.
    pub step_faces: usize,
    /// Truck faces after splitting faces on periodic surfaces (see docs/formats/step.md).
    pub faces: usize,
    /// STEP surface entity type -> number of faces.
    pub surfaces: BTreeMap<String, usize>,
    /// STEP curve entity type -> number of edges.
    pub curves: BTreeMap<String, usize>,
}

/// One planar face of an imported solid with its stable import index and frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportedPlanarFace {
    /// Position of the face in the solid's `face_iter()` order right after import
    /// (deterministic for the same bytes).
    pub index: u32,
    /// origin = area centroid, z = outward normal, x = the world axis least aligned
    /// with z projected into the plane (ties: X, then Y, then Z), y = z x x.
    pub frame: Frame,
    /// Face area (mm^2).
    pub area: f64,
}

#[derive(Debug, Clone)]
pub struct ImportedSolid {
    /// MANIFOLD_SOLID_BREP label, else the owning PRODUCT's name.
    pub name: Option<String>,
    pub body: Body,
    pub planar_faces: Vec<ImportedPlanarFace>,
}

/// Product structure of a STEP assembly.
#[derive(Debug, Clone)]
pub struct ImportedAssembly {
    pub products: Vec<ImportedProduct>,
    /// Top products: every product used by no other (a
    /// free-standing part is a root too). Products without solids or
    /// children may be listed; they hold nothing to place.
    pub roots: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct ImportedProduct {
    pub name: String,
    /// Solids in the product's own coordinates.
    pub solids: Vec<ImportedSolid>,
    /// (child product, usage name, child placement in this product).
    pub children: Vec<(usize, String, crate::Rigid)>,
}

/// The surfaces of a STEP file: every shell of every
/// SHELL_BASED_SURFACE_MODEL, placed, gathered into one surface body (one
/// shell per STEP shell). Open shells stay open: never reported as a solid.
#[derive(Debug, Clone)]
pub struct ImportedSurfaces {
    /// First non-empty SHELL_BASED_SURFACE_MODEL / product name.
    pub name: Option<String>,
    pub body: Body,
    /// STEP shells read (OPEN_SHELL and CLOSED_SHELL).
    pub shells: usize,
    pub open_shells: usize,
    pub faces: usize,
}

#[derive(Debug, Clone)]
pub struct StepImport {
    /// The flattened, placed solids (empty when `assembly` holds the
    /// structure: see [`StepImportOptions::assembly`]).
    pub solids: Vec<ImportedSolid>,
    /// The product structure (assembly files, only when
    /// [`StepImportOptions::assembly`] is set): every product's solids in its
    /// own coordinates and its usages with placements. Every
    /// MANIFOLD_SOLID_BREP of the file is in it exactly once per placement
    /// path: one outside any product's shape is folded into the product its
    /// representation is placed in, else placed under an extra root product
    /// (with a warning either way).
    pub assembly: Option<ImportedAssembly>,
    /// Surface models (open shells), if the file has any.
    pub surfaces: Option<ImportedSurfaces>,
    pub length_unit: LengthUnit,
    /// Notes: what was approximated, skipped or restructured (always read
    /// them: e.g. solids outside the product structure).
    pub warnings: Vec<String>,
    pub stats: StepStats,
}

/// Paren nesting accepted by the pre-scan (bounds parser recursion).
const MAX_NESTING: usize = 64;
/// Largest accepted number of placed solid instances.
const MAX_INSTANCES: usize = 10_000;
/// Chords per curved edge for planar-face area/centroid (doubled for Richardson).
const FACE_CHORDS: usize = 512;

/// Import every solid (MANIFOLD_SOLID_BREP) of a STEP file as exact Truck B-rep.
pub fn import_step(text: &str, opts: &StepImportOptions) -> Result<StepImport> {
    let err = |e: StepError| KernelError::InvalidInput(format!("STEP import: {}", e.0));
    let entities = prescan(text, opts).map_err(err)?;
    let exchange = guarded("STEP parse", || {
        truck_stepio::r#in::ruststep::parser::parse(text)
    })?
    .map_err(|e| {
        let msg = e.to_string();
        let short: String = msg.chars().take(300).collect();
        KernelError::InvalidInput(format!("STEP import: syntax error: {short}"))
    })?;
    guarded("STEP import", || import_parsed(&exchange, entities, opts))?.map_err(err)
}

/// Size, entity-count and nesting limits checked on the raw text before parsing.
fn prescan(text: &str, opts: &StepImportOptions) -> std::result::Result<usize, StepError> {
    if text.len() > opts.max_bytes {
        return Err(fail(format!(
            "file is {} bytes, limit {}",
            text.len(),
            opts.max_bytes
        )));
    }
    if !text.trim_start().starts_with("ISO-10303-21") {
        return Err(fail("not an ISO 10303-21 (STEP) file".into()));
    }
    let (mut depth, mut max_depth, mut entities) = (0usize, 0usize, 0usize);
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\'' => {
                i += 1;
                while i < b.len() {
                    if b[i] == b'\'' {
                        if i + 1 < b.len() && b[i + 1] == b'\'' {
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            b'(' => {
                depth += 1;
                max_depth = max_depth.max(depth);
                if max_depth > MAX_NESTING {
                    return Err(fail(format!("parameter nesting deeper than {MAX_NESTING}")));
                }
            }
            b')' => depth = depth.saturating_sub(1),
            b'=' if depth == 0 => {
                entities += 1;
                if entities > opts.max_entities {
                    return Err(fail(format!("more than {} entities", opts.max_entities)));
                }
            }
            _ => {}
        }
        i += 1;
    }
    Ok(entities)
}

fn import_parsed(
    exchange: &truck_stepio::r#in::ruststep::ast::Exchange,
    _prescan_entities: usize,
    opts: &StepImportOptions,
) -> std::result::Result<StepImport, StepError> {
    let [data] = exchange.data.as_slice() else {
        return Err(fail(format!(
            "expected one DATA section, found {}",
            exchange.data.len()
        )));
    };
    if data.entities.len() > opts.max_entities {
        return Err(fail(format!("more than {} entities", opts.max_entities)));
    }
    let idx = ast::Index::build(&data.entities)?;
    let mut warnings = Vec::new();
    let (length_unit, angle) = units::units(&idx, opts, &mut warnings)?;
    let geo = geom::Geo {
        idx: &idx,
        k: length_unit.to_mm,
        angle,
        invert_revolution: false,
    };

    let mut stats = StepStats {
        entities: data.entities.len(),
        ..Default::default()
    };
    if let Some(&v) = idx.all_with("BREP_WITH_VOIDS").first() {
        return Err(fail(format!(
            "BREP_WITH_VOIDS #{v}: solids with internal voids are unsupported"
        )));
    }
    let breps = idx.all_simple("MANIFOLD_SOLID_BREP");
    let surface_models = idx.all_simple("SHELL_BASED_SURFACE_MODEL");
    if breps.is_empty() && (surface_models.is_empty() || !opts.surfaces) {
        if !surface_models.is_empty() {
            return Err(fail(format!(
                "no MANIFOLD_SOLID_BREP in file ({} SHELL_BASED_SURFACE_MODEL(s): import them as a surface body with import_surfaces)",
                surface_models.len()
            )));
        }
        let other: Vec<String> = ["FACETED_BREP", "GEOMETRIC_CURVE_SET"]
            .iter()
            .filter(|n| !idx.all_with(n).is_empty())
            .map(|n| n.to_string())
            .collect();
        return Err(fail(if other.is_empty() {
            "no MANIFOLD_SOLID_BREP in file".into()
        } else {
            format!(
                "no MANIFOLD_SOLID_BREP in file (unsupported content: {})",
                other.join(", ")
            )
        }));
    }
    let placements = assembly::Placements::new(&idx, &geo)?;
    let surfaces = if opts.surfaces {
        import_surfaces(&idx, &geo, &placements, &surface_models, &mut warnings)?
    } else {
        if !surface_models.is_empty() {
            warnings.push(format!(
                "{} SHELL_BASED_SURFACE_MODEL(s) not read: surfaces are imported separately (import_surfaces)",
                surface_models.len()
            ));
        }
        None
    };
    // Scale-aware consistency tolerance (docs/formats/step.md).
    let tol = consistency_tol(&geo)?;
    // The structure only on request (and then fatal).
    let structure = if opts.assembly {
        placements.structure()?
    } else {
        None
    };
    let mut solids = Vec::new();
    // With the structure, the flattened view (and its
    // instance limit) is not built.
    let flat: &[u64] = if structure.is_some() { &[] } else { &breps };
    for &brep in flat {
        // One solid is built into local copies of the
        // stats and warnings, committed only when it succeeds, so a solid
        // that fails while surfaces were asked for is skipped cleanly.
        let mut st = stats.clone();
        let mut w = Vec::new();
        let mut got = Vec::new();
        let mut limit = false;
        let r = (|| -> std::result::Result<(), StepError> {
            let (_, p) = idx.simple(brep, &["MANIFOLD_SOLID_BREP"])?;
            // OpenCascade labels every solid "Open CASCADE STEP translator <ver> <n>";
            // that is not a user name, so it is ignored.
            let label = p
                .opt_s(0)
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.starts_with("Open CASCADE STEP translator"));
            let shell_id = p.r(1)?;
            let built = build_checked(&geo, brep, shell_id, tol)?;
            let instances = placements.instances_of_item(brep)?;
            if solids.len() + instances.len() > MAX_INSTANCES {
                limit = true;
                return Err(fail(format!("more than {MAX_INSTANCES} solid instances")));
            }
            let name = placements
                .product_name_of_item(brep)
                .filter(|n| !n.starts_with("Open CASCADE STEP translator"))
                .or_else(|| label.map(String::from));
            let split = built.shell.faces.len() - count_step_faces(&idx, shell_id)?;
            if split > 0 {
                w.push(format!(
                    "MANIFOLD_SOLID_BREP #{brep}: {split} face(s) added by splitting faces on periodic surfaces (exact; see docs/formats/step.md)"
                ));
            }
            st.step_faces += count_step_faces(&idx, shell_id)? * instances.len();
            for t in &built.surface_types {
                *st.surfaces.entry(t.clone()).or_default() += instances.len();
            }
            for c in &built.curve_types {
                *st.curves.entry(c.clone()).or_default() += instances.len();
            }
            for m in instances {
                let mut shell = built.shell.clone();
                if let Some(m) = m {
                    transform_shell(&mut shell, m)?;
                }
                let solid = Solid::extract(CompressedSolid {
                    boundaries: vec![shell],
                })
                .map_err(|e| {
                    fail(format!(
                        "MANIFOLD_SOLID_BREP #{brep}: not a closed manifold shell ({e})"
                    ))
                })?;
                let body = orient_outward(Body { solid }, brep)?;
                let planar_faces = planar_faces(&body)
                    .map_err(|e| fail(format!("MANIFOLD_SOLID_BREP #{brep}: {}", e.0)))?;
                st.faces += body.solid.face_iter().count();
                got.push(ImportedSolid {
                    name: name.clone(),
                    body,
                    planar_faces,
                });
            }
            Ok(())
        })();
        match r {
            Ok(()) => {
                stats = st;
                warnings.extend(w);
                solids.extend(got);
            }
            // Surfaces asked for: a bad solid does not fail the surface
            // import (the converse: a bad surface model never fails a solid
            // import).
            Err(e) if opts.surfaces && !limit => {
                warnings.push(format!("{} (solid skipped: importing surfaces)", e.0))
            }
            Err(e) => return Err(e),
        }
    }
    stats.solids = solids.len();
    // The product structure as definitions + usages.
    let assembly = match structure {
        None => None,
        Some(st) => Some(structured(
            &idx,
            &geo,
            &placements,
            st,
            &breps,
            tol,
            opts.surfaces,
            &mut stats,
            &mut warnings,
        )?),
    };
    // The product structure is read only for placements and names; say
    // so rather than present the placed bodies as a preserved assembly.
    let usages = idx.all_with("NEXT_ASSEMBLY_USAGE_OCCURRENCE").len();
    if usages > 0 && assembly.is_none() {
        warnings.push(format!(
            "assembly structure flattened: {usages} component usage(s) \
             (NEXT_ASSEMBLY_USAGE_OCCURRENCE) became {} placed solid bodies named by product; \
             import_step does not recreate the hierarchy or the reuse of definitions (linked \
             occurrences); import_step_assembly does",
            solids.len()
        ));
    }
    Ok(StepImport {
        solids,
        assembly,
        surfaces,
        length_unit,
        warnings,
        stats,
    })
}

/// The shell of MANIFOLD_SOLID_BREP `brep`. With faces on
/// SURFACE_OF_REVOLUTION, the solid must tessellate watertight and
/// consistently oriented, with Truck's normal for those surfaces or else the
/// opposite one (writers differ: R-CAD's own writer needs either, observed per
/// solid); otherwise it is refused. Observed refusals: a face reaching the
/// surface's seam angle (the last sector of a full 360° revolve) and
/// R-CAD-written spheres with poles (exact volume, broken tessellation).
fn build_checked(
    geo: &geom::Geo,
    brep: u64,
    shell_id: u64,
    tol: f64,
) -> std::result::Result<topo::Built, StepError> {
    let build = |g: &geom::Geo| {
        topo::build_shell(g, shell_id, tol, false)
            .map_err(|e| fail(format!("MANIFOLD_SOLID_BREP #{brep}: {}", e.0)))
    };
    let built = build(geo)?;
    if !built
        .surface_types
        .iter()
        .any(|t| t == "SURFACE_OF_REVOLUTION")
    {
        return Ok(built);
    }
    let mut last = String::new();
    for attempt in 0..2 {
        let b = if attempt == 0 {
            built.clone()
        } else {
            let g = geom::Geo {
                invert_revolution: !geo.invert_revolution,
                ..*geo
            };
            match build(&g) {
                Ok(b) => b,
                Err(e) => {
                    last = e.0;
                    continue;
                }
            }
        };
        match revolution_mesh_ok(&b, brep) {
            Ok(()) => return Ok(b),
            Err(e) => last = e.0,
        }
    }
    Err(fail(last))
}

fn revolution_mesh_ok(built: &topo::Built, brep: u64) -> std::result::Result<(), StepError> {
    let solid = Solid::extract(CompressedSolid {
        boundaries: vec![built.shell.clone()],
    })
    .map_err(|e| {
        fail(format!(
            "MANIFOLD_SOLID_BREP #{brep}: not a closed manifold shell ({e})"
        ))
    })?;
    let body = orient_outward(Body { solid }, brep)?;
    let b = crate::bounds(&body).map_err(|e| fail(format!("MANIFOLD_SOLID_BREP #{brep}: {e}")))?;
    let diag = (0..3)
        .map(|i| (b.max[i] - b.min[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    let chord = (diag * 1e-3).max(1e-6);
    let m = crate::tessellate(&body, chord)
        .map_err(|e| fail(format!("MANIFOLD_SOLID_BREP #{brep}: {e}")))?;
    let c = crate::check_mesh(&m);
    if !(c.watertight && c.orientation_consistent) {
        return Err(fail(format!(
            "MANIFOLD_SOLID_BREP #{brep}: its SURFACE_OF_REVOLUTION faces do not tessellate \
             watertight with either normal ({} boundary, {} misoriented edges); a face reaching \
             the surface's seam angle is unsupported",
            c.boundary_edges, c.misoriented_edges
        )));
    }
    Ok(())
}

/// MANIFOLD_SOLID_BREP label (OpenCascade's generated labels ignored).
fn brep_label(p: &ast::P) -> Option<String> {
    p.opt_s(0)
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with("Open CASCADE STEP translator"))
        .map(String::from)
}

/// One MANIFOLD_SOLID_BREP as a closed, outward solid, optionally moved by
/// `m`; counts it into `stats`.
fn brep_solid(
    idx: &ast::Index,
    geo: &geom::Geo,
    brep: u64,
    m: Option<Matrix4>,
    tol: f64,
    stats: &mut StepStats,
) -> std::result::Result<(Body, Vec<ImportedPlanarFace>), StepError> {
    let (_, p) = idx.simple(brep, &["MANIFOLD_SOLID_BREP"])?;
    let shell_id = p.r(1)?;
    let built = build_checked(geo, brep, shell_id, tol)?;
    stats.step_faces += count_step_faces(idx, shell_id)?;
    for t in &built.surface_types {
        *stats.surfaces.entry(t.clone()).or_default() += 1;
    }
    for c in &built.curve_types {
        *stats.curves.entry(c.clone()).or_default() += 1;
    }
    let mut shell = built.shell;
    if let Some(m) = m {
        transform_shell(&mut shell, m)?;
    }
    let solid = Solid::extract(CompressedSolid {
        boundaries: vec![shell],
    })
    .map_err(|e| {
        fail(format!(
            "MANIFOLD_SOLID_BREP #{brep}: not a closed manifold shell ({e})"
        ))
    })?;
    let body = orient_outward(Body { solid }, brep)?;
    let planar_faces =
        planar_faces(&body).map_err(|e| fail(format!("MANIFOLD_SOLID_BREP #{brep}: {}", e.0)))?;
    stats.faces += body.solid.face_iter().count();
    stats.solids += 1;
    Ok((body, planar_faces))
}

/// Rigid placement from a (checked rigid) column-major matrix.
fn rigid_of(m: &Matrix4) -> crate::Rigid {
    crate::Rigid {
        r: std::array::from_fn(|i| std::array::from_fn(|j| m[j][i])),
        t: [m[3][0], m[3][1], m[3][2]],
    }
}

/// The product structure with every product's definition solids (each
/// MANIFOLD_SOLID_BREP built once per definition). Every solid of
/// the file lands somewhere: in the product whose shape holds
/// it; else folded into the one product its representation is placed in
/// (moved into that product's coordinates); else under an extra root
/// product "Unstructured solids" at each of its world placements. The last
/// two are reported in `warnings`.
#[allow(clippy::too_many_arguments)]
fn structured(
    idx: &ast::Index,
    geo: &geom::Geo,
    placements: &assembly::Placements,
    st: assembly::Structure,
    breps: &[u64],
    tol: f64,
    skip_bad: bool,
    stats: &mut StepStats,
    warnings: &mut Vec<String>,
) -> std::result::Result<ImportedAssembly, StepError> {
    // A solid that fails to build is skipped with a warning only when
    // surfaces were asked for (as in the flat path); otherwise it is fatal.
    let solid = |brep: u64,
                 m: Option<Matrix4>,
                 stats: &mut StepStats,
                 warnings: &mut Vec<String>|
     -> std::result::Result<Option<(Body, Vec<ImportedPlanarFace>)>, StepError> {
        let mut st = stats.clone();
        match brep_solid(idx, geo, brep, m, tol, &mut st) {
            Ok(x) => {
                *stats = st;
                Ok(Some(x))
            }
            Err(e) if skip_bad => {
                warnings.push(format!("{} (solid skipped: importing surfaces)", e.0));
                Ok(None)
            }
            Err(e) => Err(e),
        }
    };
    let mut products: Vec<ImportedProduct> = st
        .products
        .iter()
        .map(|p| ImportedProduct {
            name: p.name.clone(),
            solids: vec![],
            children: p
                .children
                .iter()
                .map(|(c, n, m)| (*c, n.clone(), rigid_of(m)))
                .collect(),
        })
        .collect();
    let mut roots = st.roots;
    // Identity group -> first product holding it, built once.
    let mut product_of: std::collections::HashMap<u64, usize> = Default::default();
    for (i, p) in st.products.iter().enumerate() {
        for &r in &p.reps {
            product_of.entry(placements.group_of(r)).or_insert(i);
        }
    }
    let mut loose_root: Option<usize> = None;
    for &brep in breps {
        let (_, p) = idx.simple(brep, &["MANIFOLD_SOLID_BREP"])?;
        let label = brep_label(&p);
        let direct = placements
            .item_groups(brep)
            .iter()
            .filter_map(|g| product_of.get(g).copied())
            .min();
        if let Some(pi) = direct {
            let Some((body, planar_faces)) = solid(brep, None, stats, warnings)? else {
                continue;
            };
            let fallback = products[pi].name.clone();
            products[pi].solids.push(ImportedSolid {
                name: label.or(Some(fallback)),
                body,
                planar_faces,
            });
            continue;
        }
        let above = placements.products_above(brep, &product_of)?;
        if let Some(&(pi, _)) = above.first()
            && above.iter().all(|(q, _)| *q == pi)
        {
            let pname = products[pi].name.clone();
            for (_, m) in &above {
                let Some((body, planar_faces)) = solid(brep, Some(*m), stats, warnings)? else {
                    continue;
                };
                products[pi].solids.push(ImportedSolid {
                    name: label.clone().or(Some(pname.clone())),
                    body,
                    planar_faces,
                });
            }
            warnings.push(format!(
                "MANIFOLD_SOLID_BREP #{brep}: not in a shape representation of a product; \
                 placed into product '{pname}' through its representation relationship \
                 ({} instance(s))",
                above.len()
            ));
            continue;
        }
        // Outside the structure (or placed into several products): its world
        // placements, under an extra root.
        let inst = placements.instances_of_item(brep)?;
        let name = placements
            .product_name_of_item(brep)
            .filter(|n| !n.starts_with("Open CASCADE STEP translator"))
            .or(label.clone())
            .unwrap_or_else(|| format!("Solid #{brep}"));
        let Some((body, planar_faces)) = solid(brep, None, stats, warnings)? else {
            continue;
        };
        let own = products.len();
        products.push(ImportedProduct {
            name: name.clone(),
            solids: vec![ImportedSolid {
                name: label.or(Some(name.clone())),
                body,
                planar_faces,
            }],
            children: vec![],
        });
        let root = *loose_root.get_or_insert_with(|| {
            products.push(ImportedProduct {
                name: "Unstructured solids".into(),
                solids: vec![],
                children: vec![],
            });
            roots.push(products.len() - 1);
            products.len() - 1
        });
        let n = inst.len();
        for m in inst {
            let t = m.as_ref().map_or(crate::Rigid::IDENTITY, rigid_of);
            products[root].children.push((own, String::new(), t));
        }
        warnings.push(format!(
            "MANIFOLD_SOLID_BREP #{brep} ('{name}'): outside the product structure (in no \
             product's shape{}); imported under 'Unstructured solids' at its {n} world \
             placement(s)",
            if above.len() > 1 {
                ", or placed into several products"
            } else {
                ""
            }
        ));
    }
    Ok(ImportedAssembly { products, roots })
}

/// All shells of the SHELL_BASED_SURFACE_MODELs `models` as one surface body.
fn import_surfaces(
    idx: &ast::Index,
    geo: &geom::Geo,
    placements: &assembly::Placements,
    models: &[u64],
    warnings: &mut Vec<String>,
) -> std::result::Result<Option<ImportedSurfaces>, StepError> {
    if models.is_empty() {
        return Ok(None);
    }
    let tol = consistency_tol(geo)?;
    let mut shells = Vec::new();
    let mut name = None;
    let (mut open, mut faces) = (0, 0);
    for &m in models {
        let (_, p) = idx.simple(m, &["SHELL_BASED_SURFACE_MODEL"])?;
        if name.is_none() {
            name = placements
                .product_name_of_item(m)
                .filter(|n| !n.starts_with("Open CASCADE STEP translator"))
                .or_else(|| {
                    p.opt_s(0)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                });
        }
        let instances = placements.instances_of_item(m)?;
        for sid in p.refs(1)? {
            let built = topo::build_shell(geo, sid, tol, true)
                .map_err(|e| fail(format!("SHELL_BASED_SURFACE_MODEL #{m}: {}", e.0)))?;
            for inst in &instances {
                let mut cs = built.shell.clone();
                if let Some(t) = inst {
                    transform_shell(&mut cs, *t)?;
                }
                let shell = Shell::extract(cs).map_err(|e| {
                    fail(format!("SHELL_BASED_SURFACE_MODEL #{m}: shell #{sid}: {e}"))
                })?;
                if shells.len() >= MAX_INSTANCES {
                    return Err(fail(format!("more than {MAX_INSTANCES} surface shells")));
                }
                faces += shell.len();
                if shell.shell_condition() != truck_topology::shell::ShellCondition::Closed {
                    open += 1;
                }
                shells.push(shell);
            }
        }
    }
    if shells.is_empty() {
        return Ok(None);
    }
    // Closed shells of a surface model are oriented outward like
    // solids; a surface model of several closed shells only is refused (one
    // body is one solid; import solids as MANIFOLD_SOLID_BREPs).
    if open == 0 && shells.len() > 1 {
        return Err(fail(format!(
            "the surface models are {} closed shells and no open one: several solids in one body are not supported",
            shells.len()
        )));
    }
    let mut oriented = Vec::with_capacity(shells.len());
    for sh in shells {
        if sh.shell_condition() == truck_topology::shell::ShellCondition::Closed {
            let solid =
                Solid::try_new(vec![sh]).map_err(|e| fail(format!("closed surface shell: {e}")))?;
            let b = orient_outward(Body { solid }, 0)?;
            oriented.extend(b.solid.into_boundaries());
        } else {
            oriented.push(sh);
        }
    }
    let shells = oriented;
    warnings.push(format!(
        "{} surface shell(s) ({open} open, {faces} face(s)) in SHELL_BASED_SURFACE_MODEL: not solids (no volume); import them as a surface body (import_surfaces)",
        shells.len()
    ));
    let n = shells.len();
    Ok(Some(ImportedSurfaces {
        name,
        body: Body {
            solid: Solid::new_unchecked(shells),
        },
        shells: n,
        open_shells: open,
        faces,
    }))
}

fn count_step_faces(idx: &ast::Index, shell_id: u64) -> std::result::Result<usize, StepError> {
    let (_, p) = idx.simple(shell_id, &["CLOSED_SHELL", "OPEN_SHELL"])?;
    Ok(p.refs(1)?.len())
}

/// Edge/vertex/surface consistency tolerance: max(1e-5 mm, 1e-9 x bounding diagonal
/// of all VERTEX_POINTs of the file).
fn consistency_tol(geo: &geom::Geo) -> std::result::Result<f64, StepError> {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for id in geo.idx.all_simple("VERTEX_POINT") {
        let p = geo.vertex(id)?;
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let diag = if lo[0].is_finite() {
        ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt()
    } else {
        0.0
    };
    Ok(1e-5_f64.max(1e-9 * diag))
}

/// Apply a rigid placement to every vertex, curve and surface (exact: affine maps of
/// control points / analytic frames).
fn transform_shell(shell: &mut topo::CShell, m: Matrix4) -> std::result::Result<(), StepError> {
    for v in &mut shell.vertices {
        *v = m.transform_point(*v);
    }
    for e in &mut shell.edges {
        e.curve.transform_by(m);
    }
    for f in &mut shell.faces {
        f.surface.transform_by(m);
    }
    Ok(())
}

/// Flip the solid if its enclosed volume is negative (inward-oriented shell).
fn orient_outward(mut body: Body, brep: u64) -> std::result::Result<Body, StepError> {
    let m = crate::measure(&body).map_err(|e| fail(format!("MANIFOLD_SOLID_BREP #{brep}: {e}")))?;
    if m.shell_count != 1 || !m.all_shells_closed {
        return Err(fail(format!(
            "MANIFOLD_SOLID_BREP #{brep}: result is not one closed shell"
        )));
    }
    match m.volume {
        crate::VolumeResult::Value {
            volume,
            error_estimate,
            ..
        } => {
            if volume.abs() <= error_estimate {
                return Err(fail(format!(
                    "MANIFOLD_SOLID_BREP #{brep}: volume {volume} indistinguishable from zero"
                )));
            }
            if volume < 0.0 {
                body.solid.not();
            }
            Ok(body)
        }
        crate::VolumeResult::Unavailable { reason } => Err(fail(format!(
            "MANIFOLD_SOLID_BREP #{brep}: cannot determine orientation ({reason})"
        ))),
    }
}

/// Planar faces with stable index, frame and area (exact edge curves, chord sums with
/// Richardson extrapolation, as for the B-rep volume).
fn planar_faces(body: &Body) -> std::result::Result<Vec<ImportedPlanarFace>, StepError> {
    let mut out = Vec::new();
    for (i, f) in body.solid.face_iter().enumerate() {
        let Surface::Plane(pl) = f.surface() else {
            continue;
        };
        let mut z = pl.normal();
        if !f.orientation() {
            z = -z;
        }
        let x = least_aligned_axis(z);
        let y = z.cross(x);
        let o = pl.origin();
        let moments = |n: usize| -> std::result::Result<[f64; 3], StepError> {
            let mut m = [0.0; 3];
            for w in f.boundaries() {
                for e in w.edge_iter() {
                    let c = e.oriented_curve();
                    let (t0, t1) = c.range_tuple();
                    let k = if matches!(c, Curve::Line(_)) { 1 } else { n };
                    let to2 = |p: Point3| {
                        let d = p - o;
                        (d.dot(x), d.dot(y))
                    };
                    let eval = |t: f64| match &c {
                        Curve::IntersectionCurve(ic) => ic.search_triple(t).map(|r| r.0),
                        other => Some(other.subs(t)),
                    };
                    let mut prev =
                        to2(eval(t0).ok_or_else(|| fail("edge evaluation failed".into()))?);
                    for j in 1..=k {
                        let p = to2(eval(t0 + (t1 - t0) * j as f64 / k as f64)
                            .ok_or_else(|| fail("edge evaluation failed".into()))?);
                        let cr = prev.0 * p.1 - p.0 * prev.1;
                        m[0] += cr;
                        m[1] += (prev.0 + p.0) * cr;
                        m[2] += (prev.1 + p.1) * cr;
                        prev = p;
                    }
                }
            }
            Ok([m[0] / 2.0, m[1] / 6.0, m[2] / 6.0])
        };
        let (a, b) = (moments(FACE_CHORDS)?, moments(2 * FACE_CHORDS)?);
        let r: Vec<f64> = (0..3).map(|k| (4.0 * b[k] - a[k]) / 3.0).collect();
        if !(r[0] > 0.0) {
            return Err(fail(format!(
                "planar face {i}: non-positive area {} about its outward normal",
                r[0]
            )));
        }
        let c = o + x * (r[1] / r[0]) + y * (r[2] / r[0]);
        out.push(ImportedPlanarFace {
            index: i as u32,
            frame: Frame {
                origin: [c.x, c.y, c.z],
                x: [x.x, x.y, x.z],
                y: [y.x, y.y, y.z],
                z: [z.x, z.y, z.z],
            },
            area: r[0],
        });
    }
    Ok(out)
}

/// The world axis least aligned with `z` (ties broken X, then Y, then Z), projected
/// into the plane normal to `z` and normalised.
pub(crate) fn least_aligned_axis(z: Vector3) -> Vector3 {
    let axes = [Vector3::unit_x(), Vector3::unit_y(), Vector3::unit_z()];
    let mut best = 0;
    for k in 1..3 {
        // Strictly smaller (beyond rounding) wins; ties keep the earlier axis.
        if z[k].abs() < z[best].abs() - 1e-12 {
            best = k;
        }
    }
    let a = axes[best];
    (a - z * a.dot(z)).normalize()
}
