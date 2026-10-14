//! Adapters from embedded source files to body geometry (ADR 0004). Pure
//! functions of the bytes and the recorded options, so regeneration is
//! deterministic and needs no external path.
use crate::regen::MeshBody;
use crate::{DocError, Result};
pub use rcad_exchange::mesh::MeshDiagnostics;
use rcad_exchange::stl;
pub use rcad_kernel::{ImportedPlanarFace, ImportedSolid, LengthUnit, StepStats};

pub struct StepResult {
    pub solids: Vec<ImportedSolid>,
    /// Surface models (open shells) of the file, as one surface body.
    pub surfaces: Option<rcad_kernel::ImportedSurfaces>,
    pub unit: LengthUnit,
    pub stats: StepStats,
    /// Units, counts and warnings for the import summary.
    pub notes: Vec<String>,
}

/// A reader error, or [`DocError::ImportCancelled`] when the read stopped
/// because cancellation was requested (`rcad_cancel`).
pub fn read_error(format: &str, e: impl std::fmt::Display) -> DocError {
    if crate::cancel_requested() {
        DocError::ImportCancelled(format!("a {format} file"))
    } else {
        DocError::Validation(e.to_string())
    }
}

/// Parse an embedded STEP file into exact solids (mm).
pub fn step(bytes: &[u8], unit_override_to_mm: Option<f64>) -> Result<StepResult> {
    step_opts(bytes, unit_override_to_mm, false)
}

/// [`step`] that also reads the file's surface models
/// (`import_surfaces`); without `surfaces` they are ignored with a warning.
pub fn step_opts(
    bytes: &[u8],
    unit_override_to_mm: Option<f64>,
    surfaces: bool,
) -> Result<StepResult> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| DocError::Validation(format!("STEP file is not valid text: {e}")))?;
    let opts = rcad_kernel::StepImportOptions {
        max_bytes: crate::io::MAX_FILE_BYTES as usize,
        unit_override_to_mm,
        surfaces,
        ..Default::default()
    };
    let imp = rcad_kernel::import_step(text, &opts).map_err(|e| read_error("STEP", e))?;
    let mut notes = vec![format!(
        "units: {} ({} mm per unit, {})",
        imp.length_unit.name,
        imp.length_unit.to_mm,
        if imp.length_unit.declared {
            "declared in file"
        } else {
            "chosen at import"
        }
    )];
    notes.push(format!(
        "{} solid(s), {} faces; surfaces: {}",
        imp.stats.solids,
        imp.stats.faces,
        imp.stats
            .surfaces
            .iter()
            .map(|(k, v)| format!("{k} x{v}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    notes.extend(imp.warnings.iter().map(|w| format!("warning: {w}")));
    if let Some(n) = presentation_styles(text) {
        notes.push(format!(
            "warning: colours/styles ignored: {n} STYLED_ITEM presentation entities (colours, \
             layers) are not imported"
        ));
    }
    Ok(StepResult {
        solids: imp.solids,
        surfaces: imp.surfaces,
        unit: imp.length_unit,
        stats: imp.stats,
        notes,
    })
}

/// Number of `STYLED_ITEM` / `OVER_RIDING_STYLED_ITEM` instances (colour and
/// style presentation, ISO 10303-214/242) in a STEP file's text: the kernel
/// reads geometry only, so their loss is reported. `None` when there are none.
fn presentation_styles(text: &str) -> Option<usize> {
    let n = text
        .split(';')
        .filter(|inst| {
            let t = inst.trim_start();
            t.starts_with('#')
                && t.split_once('=').is_some_and(|(_, rhs)| {
                    let r = rhs.trim_start();
                    r.starts_with("STYLED_ITEM") || r.starts_with("OVER_RIDING_STYLED_ITEM")
                })
        })
        .count();
    (n > 0).then_some(n)
}

/// Parse an embedded STL file into a mesh body scaled by `to_mm`.
pub fn stl(bytes: &[u8], to_mm: f64) -> Result<(MeshBody, Vec<String>)> {
    let ex = |e: rcad_exchange::ExchangeError| read_error("STL", e);
    let mut m = stl::read(bytes, &stl::StlLimits::default()).map_err(ex)?;
    m.scale(to_mm).map_err(ex)?;
    let diagnostics = rcad_exchange::mesh::diagnose(&m.positions, &m.triangles);
    if diagnostics.invalid_triangles > 0 {
        return Err(DocError::Validation(format!(
            "STL: {} triangles reference missing vertices",
            diagnostics.invalid_triangles
        )));
    }
    let mut notes = vec![format!(
        "{} STL, {} triangles, {} vertices, scale {to_mm} mm per file unit",
        match m.encoding {
            stl::StlEncoding::Ascii => "ASCII",
            stl::StlEncoding::Binary => "binary",
        },
        diagnostics.triangles,
        diagnostics.vertices
    )];
    notes.extend(mesh_notes(&diagnostics));
    Ok((mesh_body(&m.positions, &m.triangles, diagnostics), notes))
}

/// Topology notes for a mesh body (never an exact-solid claim).
pub(crate) fn mesh_notes(diagnostics: &MeshDiagnostics) -> Vec<String> {
    let mut notes = vec![
        if diagnostics.closed {
            "closed, consistently oriented".to_string()
        } else {
            format!(
                "open or inconsistent: {} boundary, {} non-manifold, {} misoriented edges",
                diagnostics.boundary_edges,
                diagnostics.non_manifold_edges,
                diagnostics.inconsistent_orientation_edges
            )
        },
        "mesh body: display/export only; exact operations are refused".to_string(),
    ];
    if diagnostics.degenerate_triangles > 0 {
        notes.push(format!(
            "{} degenerate triangles",
            diagnostics.degenerate_triangles
        ));
    }
    notes
}

/// Display mesh body: unshared vertices with flat (geometric) normals. The
/// caller checked that every index is in range.
pub(crate) fn mesh_body(
    positions: &[[f64; 3]],
    tris: &[[u32; 3]],
    diagnostics: MeshDiagnostics,
) -> MeshBody {
    let n = tris.len();
    let mut out_positions = Vec::with_capacity(3 * n);
    let mut normals = Vec::with_capacity(3 * n);
    let mut triangles = Vec::with_capacity(n);
    for t in tris {
        let [a, b, c] = t.map(|i| positions[i as usize]);
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cr = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let len = (cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2]).sqrt();
        let nrm = if len > 0.0 {
            [cr[0] / len, cr[1] / len, cr[2] / len]
        } else {
            [0.0, 0.0, 1.0]
        };
        let base = out_positions.len() as u32;
        out_positions.extend([a, b, c]);
        normals.extend([nrm; 3]);
        triangles.push([base, base + 1, base + 2]);
    }
    MeshBody {
        mesh: rcad_kernel::TriMesh {
            positions: out_positions,
            normals,
            triangles,
            tri_face: vec![0; n],
            chord_tolerance: 0.0,
        },
        diagnostics,
    }
}

/// Repaired copy of a mesh body (`rcad_exchange::mesh_repair`). The
/// display mesh's unshared corners are welded first (exact), then repaired.
pub fn repaired(
    m: &MeshBody,
    opts: &rcad_exchange::mesh_repair::RepairOptions,
) -> Result<(MeshBody, Vec<String>)> {
    let (pos, tris, rep) =
        rcad_exchange::mesh_repair::repair(&m.mesh.positions, &m.mesh.triangles, opts)
            .map_err(DocError::Validation)?;
    let mut notes = vec![format!(
        "mesh repair: weld tolerance {} mm; {} vertices merged ({} by tolerance), {} degenerate \
         and {} duplicate triangles removed, {} triangles flipped, {} holes filled ({} triangles), \
         {} left open",
        opts.weld_tolerance,
        rep.welded_vertices,
        rep.welded_by_tolerance,
        rep.removed_degenerate,
        rep.removed_duplicates,
        rep.flipped_triangles,
        rep.filled_holes,
        rep.fill_triangles,
        rep.unfilled_holes
    )];
    notes.extend(rep.notes.iter().cloned());
    notes.extend(mesh_notes(&rep.after));
    Ok((mesh_body(&pos, &tris, rep.after), notes))
}

/// One mesh object of an imported 3MF package.
pub struct ThreeMfObject {
    pub name: Option<String>,
    pub mesh: MeshBody,
}

/// Parse an embedded 3MF package: one mesh body per build item (units from the
/// file; item and component transforms applied by the adapter).
pub fn threemf(bytes: &[u8]) -> Result<(Vec<ThreeMfObject>, Vec<String>)> {
    use rcad_exchange::threemf;
    let imp = threemf::read(bytes, &threemf::ThreeMfLimits::default())
        .map_err(|e| read_error("3MF", e))?;
    let mut notes = vec![format!(
        "3MF, unit {} ({} mm), {} build item(s)",
        imp.unit.name(),
        imp.unit.to_mm(),
        imp.objects.len()
    )];
    notes.extend(imp.warnings.iter().map(|w| format!("warning: {w}")));
    let mut out = Vec::with_capacity(imp.objects.len());
    for o in imp.objects {
        if o.diagnostics.invalid_triangles > 0 {
            return Err(DocError::Validation(format!(
                "3MF object {}: {} triangles reference missing vertices",
                o.object_id, o.diagnostics.invalid_triangles
            )));
        }
        let label = o
            .name
            .clone()
            .unwrap_or_else(|| format!("object {}", o.object_id));
        notes.extend(
            mesh_notes(&o.diagnostics)
                .into_iter()
                .take(1)
                .map(|n| format!("{label}: {n}")),
        );
        out.push(ThreeMfObject {
            name: o.name.clone(),
            mesh: mesh_body(&o.positions, &o.triangles, o.diagnostics),
        });
    }
    notes.push("mesh bodies: display/export only; exact operations are refused".into());
    Ok((out, notes))
}
