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
fn mesh_notes(diagnostics: &MeshDiagnostics) -> Vec<String> {
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
fn mesh_body(positions: &[[f64; 3]], tris: &[[u32; 3]], diagnostics: MeshDiagnostics) -> MeshBody {
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
        mesh: f420_kernel::TriMesh {
            positions: out_positions,
            normals,
            triangles,
            tri_face: vec![0; n],
            chord_tolerance: 0.0,
        },
        diagnostics,
    }
}

/// One mesh object of an imported 3MF package.
pub struct ThreeMfObject {
    pub name: Option<String>,
    pub mesh: MeshBody,
}

/// Parse an embedded 3MF package: one mesh body per build item (units from the
/// file; item and component transforms applied by the adapter).
pub fn threemf(bytes: &[u8]) -> Result<(Vec<ThreeMfObject>, Vec<String>)> {
    use f420_exchange::threemf;
    let imp = threemf::read(bytes, &threemf::ThreeMfLimits::default())
        .map_err(|e| DocError::Validation(e.to_string()))?;
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
