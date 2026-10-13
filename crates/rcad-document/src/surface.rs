//! Surface bodies and open-shell repair (run 7, docs/SURFACES.md, ADR 0012):
//! import of STEP surface models as open-shell bodies, stitching within a
//! stated healing tolerance, planar filling of boundary loops, and thickening
//! of single planar/cylindrical faces. Each operation makes a new body and
//! keeps its input (preserved for inspection and re-editing). Pure functions
//! of their inputs; regeneration calls them.
use crate::{DocError, FeatureId, Result};
use rcad_kernel::{self as k, Body};

/// Kernel refusals of unsupported input are validation errors (a stated
/// limit, not a kernel failure); other kernel errors stay kernel errors.
fn kerr(id: FeatureId) -> impl Fn(k::KernelError) -> DocError {
    move |e| match e {
        k::KernelError::InvalidInput(m) => DocError::Validation(format!("{id}: {m}")),
        other => DocError::Kernel {
            feature: id,
            detail: other.to_string(),
        },
    }
}

/// The surface body of an embedded STEP file and import notes.
pub fn import(
    id: FeatureId,
    bytes: &[u8],
    unit_override_to_mm: Option<f64>,
) -> Result<(Body, Option<String>, Vec<String>)> {
    let imp = crate::import::step(bytes, unit_override_to_mm).map_err(|e| match e {
        DocError::Validation(m) => DocError::Validation(format!("{id}: {m}")),
        other => other,
    })?;
    let s = imp.surfaces.ok_or_else(|| {
        DocError::Validation(format!(
            "{id}: the STEP file has no surface model (SHELL_BASED_SURFACE_MODEL); import its solids with import_step"
        ))
    })?;
    let mut notes = vec![imp.notes[0].clone()];
    notes.push(format!(
        "surface body: {} shell(s), {} open, {} face(s); open shells have no volume",
        s.shells, s.open_shells, s.faces
    ));
    if !imp.solids.is_empty() {
        notes.push(format!(
            "the file also has {} solid(s): import them with import_step",
            imp.solids.len()
        ));
    }
    Ok((s.body, s.name, notes))
}

fn source_must_be_surface(id: FeatureId, src: &Body, what: &str) -> Result<()> {
    if !src.is_open_shell() {
        return Err(DocError::Validation(format!(
            "{id}: {what} needs a surface (open-shell) body; the source is already a closed solid"
        )));
    }
    Ok(())
}

fn tol_ok(id: FeatureId, tol: f64) -> Result<()> {
    if !(tol.is_finite() && (0.0..=1.0).contains(&tol)) {
        return Err(DocError::Validation(format!(
            "{id}.tolerance: the healing tolerance must be in [0, 1] mm, got {tol}"
        )));
    }
    Ok(())
}

/// Stitch `src` within `tol` mm (new body; `src` unchanged).
pub fn stitch(id: FeatureId, src: &Body, tol: f64) -> Result<(Body, Vec<String>)> {
    tol_ok(id, tol)?;
    source_must_be_surface(id, src, "stitch")?;
    let (b, r) = k::stitch(src, tol).map_err(kerr(id))?;
    let s = &r.stats;
    let notes = vec![
        format!("healing tolerance {} mm", r.tolerance),
        format!(
            "merged {} vertices (largest gap {:.6} mm), {} edges (largest gap {:.6} mm); {} face(s) reoriented",
            s.vertices_merged, s.max_vertex_gap, s.edges_merged, s.max_edge_gap, s.faces_flipped
        ),
        if r.closed {
            "result: closed solid".into()
        } else {
            format!(
                "result: open shell, {} free edge(s), {} piece(s) (no volume)",
                r.free_edges, r.pieces
            )
        },
    ];
    Ok((b, notes))
}

/// Fill the planar boundary loops of `src` (new body).
pub fn fill(id: FeatureId, src: &Body, tol: f64) -> Result<(Body, Vec<String>)> {
    tol_ok(id, tol)?;
    source_must_be_surface(id, src, "fill")?;
    let (b, r) = k::fill_planar(src, tol).map_err(kerr(id))?;
    Ok((
        b,
        vec![
            format!(
                "filled {} planar loop(s) (largest planarity deviation {:.3e} mm, tolerance {tol} mm)",
                r.filled, r.max_planarity
            ),
            if r.closed {
                "result: closed solid".into()
            } else {
                format!("result: open shell, {} free edge(s)", r.free_edges)
            },
        ],
    ))
}

/// Thicken the single face of `src` (new solid body).
pub fn thicken(
    id: FeatureId,
    src: &Body,
    distance: f64,
    reverse: bool,
) -> Result<(Body, Vec<String>)> {
    let b = k::thicken(src, distance, reverse).map_err(kerr(id))?;
    Ok((
        b,
        vec![format!(
            "thickened {distance} mm {} the face normal",
            if reverse { "against" } else { "along" }
        )],
    ))
}
