//! Adapters from embedded source files to body geometry (ADR 0004). Pure
//! functions of the bytes and the recorded options, so regeneration is
//! deterministic and needs no external path.
use crate::regen::MeshBody;
use crate::{DocError, Result};
pub use f420_exchange::mesh::MeshDiagnostics;
use f420_exchange::stl;
pub use f420_kernel::{ImportedPlanarFace, ImportedSolid, LengthUnit, StepStats};

pub struct StepResult {
    pub solids: Vec<ImportedSolid>,
    pub unit: LengthUnit,
    pub stats: StepStats,
    /// Units, counts and warnings for the import summary.
    pub notes: Vec<String>,
}

/// Parse an embedded STEP file into exact solids (mm).
pub fn step(bytes: &[u8], unit_override_to_mm: Option<f64>) -> Result<StepResult> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| DocError::Validation(format!("STEP file is not valid text: {e}")))?;
    let opts = f420_kernel::StepImportOptions {
        max_bytes: crate::io::MAX_FILE_BYTES as usize,
        unit_override_to_mm,
        ..Default::default()
    };
    let imp =
        f420_kernel::import_step(text, &opts).map_err(|e| DocError::Validation(e.to_string()))?;
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
    Ok(StepResult {
        solids: imp.solids,
        unit: imp.length_unit,
        stats: imp.stats,
        notes,
    })
}

/// Parse an embedded STL file into a mesh body scaled by `to_mm`.
pub fn stl(bytes: &[u8], to_mm: f64) -> Result<(MeshBody, Vec<String>)> {
    let ex = |e: f420_exchange::ExchangeError| DocError::Validation(e.to_string());
    let mut m = stl::read(bytes, &stl::StlLimits::default()).map_err(ex)?;
    m.scale(to_mm).map_err(ex)?;
    let diagnostics = f420_exchange::mesh::diagnose(&m.positions, &m.triangles);
    if diagnostics.invalid_triangles > 0 {
        return Err(DocError::Validation(format!(
            "STL: {} triangles reference missing vertices",
            diagnostics.invalid_triangles
        )));
    }
    // Display mesh: unshared vertices with flat (geometric) normals.
    let n = m.triangles.len();
    let mut positions = Vec::with_capacity(3 * n);
    let mut normals = Vec::with_capacity(3 * n);
    let mut triangles = Vec::with_capacity(n);
    for i in 0..n {
        let Some([a, b, c]) = m.triangle(i) else {
            continue;
        };
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
        let base = positions.len() as u32;
        positions.extend([a, b, c]);
        normals.extend([nrm; 3]);
        triangles.push([base, base + 1, base + 2]);
    }
    let mut notes = vec![
        format!(
            "{} STL, {} triangles, {} vertices, scale {to_mm} mm per file unit",
            match m.encoding {
                stl::StlEncoding::Ascii => "ASCII",
                stl::StlEncoding::Binary => "binary",
            },
            diagnostics.triangles,
            diagnostics.vertices
        ),
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
    Ok((
        MeshBody {
            mesh: f420_kernel::TriMesh {
                positions,
                normals,
                triangles,
                tri_face: vec![0; n],
                chord_tolerance: 0.0,
            },
            diagnostics,
        },
        notes,
    ))
}
