//! Print export (STL / 3MF): body selection policy, tessellation with chord and
//! optional angular limits, strict solid-mesh checks, export-only placement on the
//! bed, bed fit, and the per-body report. See docs/PRINT_EXPORT.md.
//!
//! Placement never touches the document: it transforms copies of the exported
//! meshes with proper rotations ([`k::RigidTransform`], det +1) and translations.

use crate::Response;
use rcad_document::{BodyGeometry, BodyId, DocError, FaceRef, Model, write_atomic};
use rcad_kernel::{self as k, MeshCheck, RigidTransform, TriMesh};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Optional print-export settings shared by `export_stl` and `export_3mf`
/// (flattened into the command JSON; every field is optional and omitted when
/// unset, so older command JSON parses and behaves as before).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PrintOptions {
    /// Maximum angle (degrees) between the normals of adjacent triangles of one
    /// curved face. Enforced by refining the chord until the measured angle of
    /// the tessellation meets it; refused when that needs a chord below the
    /// kernel's minimum. `None`: chord only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle_deg: Option<f64>,
    /// Export-only placement on the bed (z = 0). `None`: coordinates as modeled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
    /// Printable volume to report fit against (never changes geometry).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bed: Option<BedSpec>,
    /// STL only: one file per body, `<stem>-<sanitized body name>.stl` next to `path`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub separate_files: bool,
    /// Allow imported mesh bodies that are not closed, manifold, outward solids
    /// (exported as labelled reference meshes). B-rep tessellations must always
    /// be valid solids.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_open_meshes: bool,
    /// 3MF only: write each object in its modeled coordinates and the placement
    /// (rotation + translation) as its build item's `transform`, instead of baking
    /// the placement into the vertices. Readers place the objects identically; the
    /// per-object transform stays editable in the slicer. Without `placement` no
    /// transform is written (identity).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub item_transforms: bool,
}

/// Export-only placement: every exported body is oriented (default as modeled),
/// then translated so its lowest point is at z = 0.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// Per-body orientation; bodies not listed keep their modeled orientation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bodies: Vec<BodyPlacement>,
    /// Arrange the bodies side by side along X (export order) with `gap` mm
    /// between bounding boxes, centred on the bed (or the origin without a bed).
    /// `None`: each body keeps the XY centre of its bounding box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Layout>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodyPlacement {
    pub body: BodyId,
    #[serde(default)]
    pub orient: Orient,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orient {
    #[default]
    AsModeled,
    /// Rotate (proper rotation) so this planar face's outward normal points to
    /// -Z, i.e. the face lies on the bed. The face must belong to the body and be
    /// a lowest face after rotation.
    FaceDown(FaceRef),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub gap: f64,
}

/// A printable volume: a preset from an installed slicer profile, or manual
/// dimensions (mm; origin at bed corner (0, 0)).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BedSpec {
    Preset { preset: BedPreset },
    Manual { width: f64, depth: f64, height: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BedPreset {
    /// Bambu Lab P1S, values from the Bambu Studio machine profile (see
    /// [`P1S_SOURCE`]).
    BambuP1s,
}

/// Provenance of the P1S preset (docs/PRINT_EXPORT.md has the full chain).
pub const P1S_SOURCE: &str = "Bambu Studio 2.8.2.61 flatpak com.bambulab.BambuStudio, \
     share/BambuStudio/profiles/BBL/machine: 'Bambu Lab P1S 0.4 nozzle.json' (bed_exclude_area \
     0x0,18x0,18x28,0x28) inherits fdm_bbl_3dp_001_common.json (printable_area \
     0x0,256x0,256x256,0x256) inherits fdm_machine_common.json (printable_height 250)";
const P1S_AREA: [[f64; 2]; 2] = [[0.0, 0.0], [256.0, 256.0]];
const P1S_HEIGHT: f64 = 250.0;
const P1S_EXCLUDE: [[f64; 2]; 2] = [[0.0, 0.0], [18.0, 28.0]];

/// Resolved bed: rectangle `[min, max]` in XY, height, excluded rectangles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BedReport {
    pub name: String,
    pub min: [f64; 2],
    pub max: [f64; 2],
    pub height: f64,
    /// Areas of the bed that must stay empty (`[min, max]` rectangles).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<[[f64; 2]; 2]>,
    pub source: String,
}

impl BedSpec {
    fn resolve(&self) -> Result<BedReport, DocError> {
        match *self {
            BedSpec::Preset {
                preset: BedPreset::BambuP1s,
            } => Ok(BedReport {
                name: "bambu_p1s".into(),
                min: P1S_AREA[0],
                max: P1S_AREA[1],
                height: P1S_HEIGHT,
                exclude: vec![P1S_EXCLUDE],
                source: P1S_SOURCE.into(),
            }),
            BedSpec::Manual {
                width,
                depth,
                height,
            } => {
                for (n, v) in [("width", width), ("depth", depth), ("height", height)] {
                    if !v.is_finite() || v <= 0.0 || v > 10_000.0 {
                        return Err(DocError::Validation(format!(
                            "bed {n} must be in (0, 10000] mm, got {v}"
                        )));
                    }
                }
                Ok(BedReport {
                    name: "manual".into(),
                    min: [0.0, 0.0],
                    max: [width, depth],
                    height,
                    exclude: vec![],
                    source: "manual dimensions".into(),
                })
            }
        }
    }
}

/// Fit of one placed body (or the whole layout) against the bed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fit {
    /// Dimensions (in the exported orientation) fit within the printable volume.
    pub fits_size: bool,
    /// Placed bounds lie inside the printable area and height (only when the
    /// export is placed; as-modeled coordinates are not bed positions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within_bed: Option<bool>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodyExport {
    pub id: BodyId,
    pub name: String,
    /// "solid" (exact B-rep, tessellated) or "mesh" (imported triangle mesh).
    pub kind: String,
    pub visible: bool,
    /// "valid solid mesh" or "open reference mesh (not a printable solid)".
    pub label: String,
    pub triangles: usize,
    /// Chord actually used (B-rep only; may be finer than requested to meet `angle_deg`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chord: Option<f64>,
    /// Measured max facet-to-facet angle within curved faces (B-rep only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_facet_angle_deg: Option<f64>,
    /// Export-only placement applied (None: as modeled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<RigidTransform>,
    /// Exported (placed) bounds and dimensions, mm.
    pub bounds_min: [f64; 3],
    pub bounds_max: [f64; 3],
    pub dims: [f64; 3],
    /// Checks of the exported mesh (topology by index after exact welding).
    pub mesh_check: MeshCheck,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<Fit>,
    /// STL `separate_files`: the file this body went to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileExport {
    pub path: PathBuf,
    pub bytes: u64,
    pub bodies: Vec<BodyId>,
}

/// Structured result of a print export (`Response::export`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportReport {
    /// "stl" or "3mf".
    pub format: String,
    /// Coordinate units of the written file(s), stated for the user.
    pub units: String,
    /// Committed document revision the geometry came from.
    pub revision: u64,
    pub chord: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle_deg: Option<f64>,
    /// How bodies were chosen, naming any hidden body excluded or included.
    pub selection: String,
    /// "as modeled" or a description of the export-only placement.
    pub placement: String,
    pub bodies: Vec<BodyExport>,
    pub files: Vec<FileExport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bed: Option<BedReport>,
    /// Whole layout inside the bed (placed exports with a bed only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_fit: Option<Fit>,
    pub warnings: Vec<String>,
}

pub(crate) enum Format {
    Stl,
    ThreeMf(rcad_exchange::threemf::ThreeMfUnit),
}

/// A file to write: path, bytes, bodies it holds.
type OutFile = (PathBuf, Vec<u8>, Vec<BodyId>);

struct Prepared {
    mesh: TriMesh,
    report: BodyExport,
    /// The mesh before placement (set by `place`).
    unplaced: Option<TriMesh>,
}

const STL_UNITS: &str = "mm (STL has no unit field: coordinates are written in millimetres and the slicer must interpret them as mm)";
/// Upper bound on chord refinement steps for `angle_deg`.
const MAX_ANGLE_REFINEMENTS: usize = 8;
/// Refuse tessellations above this size (memory/performance guard).
const MAX_EXPORT_TRIANGLES: usize = 20_000_000;

fn invalid(msg: String) -> DocError {
    DocError::Validation(msg)
}

/// Tessellate a B-rep at `chord`, refining while the facet angle exceeds `angle`.
fn tessellate_body(
    body: &k::Body,
    name: &str,
    chord: f64,
    angle: Option<f64>,
) -> Result<(TriMesh, f64, f64), DocError> {
    let mut c = chord;
    for _ in 0..=MAX_ANGLE_REFINEMENTS {
        let mesh = k::tessellate(body, c).map_err(|e| match angle {
            Some(a) if c < chord => invalid(format!(
                "{name}: angle_deg {a} would need a chord below the kernel minimum ({e}); \
                 raise angle_deg or the chord"
            )),
            _ => invalid(format!("{name}: {e}")),
        })?;
        if mesh.triangles.len() > MAX_EXPORT_TRIANGLES {
            return Err(invalid(format!(
                "{name}: {} triangles at chord {c} mm exceeds the export limit of \
                 {MAX_EXPORT_TRIANGLES}; use a coarser chord/angle",
                mesh.triangles.len()
            )));
        }
        let got = k::max_facet_angle_deg(&mesh);
        match angle {
            Some(a) if got > a => {
                // Facet angle ~ sqrt(chord): shrink by the squared ratio (with margin).
                let ratio = (a / got).powi(2) * 0.9;
                c *= ratio.clamp(0.05, 0.9);
            }
            _ => return Ok((mesh, c, got)),
        }
    }
    Err(invalid(format!(
        "{name}: angle_deg {} not reached after {MAX_ANGLE_REFINEMENTS} chord refinements",
        angle.unwrap_or_default()
    )))
}

fn bounds(m: &TriMesh) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in &m.positions {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    (lo, hi)
}

fn selection_text(m: &Model, ids: &[BodyId], explicit: bool) -> String {
    let ev = m.evaluation();
    let hidden: Vec<String> = ev
        .bodies
        .iter()
        .filter(|b| !b.visible)
        .map(|b| (b.id, format!("'{}'", b.name)))
        .filter(|(id, _)| ids.contains(id) == explicit)
        .map(|(_, n)| n)
        .collect();
    match (explicit, hidden.is_empty()) {
        (false, true) => "default: all visible bodies (none hidden)".into(),
        (false, false) => format!(
            "default: all visible bodies; hidden not exported: {}",
            hidden.join(", ")
        ),
        (true, true) => "explicit selection (no hidden bodies)".into(),
        (true, false) => format!("explicit selection, includes hidden: {}", hidden.join(", ")),
    }
}

/// Mesh, checks and placement of every selected body; nothing is written.
fn prepare(
    m: &Model,
    ids: &[BodyId],
    chord: f64,
    opts: &PrintOptions,
    bed: Option<&BedReport>,
    r: &mut Response,
    warnings: &mut Vec<String>,
) -> Result<Vec<Prepared>, DocError> {
    let ev = m.evaluation();
    if let Some(p) = &opts.placement {
        for bp in &p.bodies {
            if !ids.contains(&bp.body) {
                return Err(invalid(format!(
                    "placement names body {} which is not exported",
                    bp.body
                )));
            }
        }
        if let Some(l) = p.layout
            && (!l.gap.is_finite() || l.gap < 0.0 || l.gap > 10_000.0)
        {
            return Err(invalid(format!(
                "layout gap must be in [0, 10000] mm, got {}",
                l.gap
            )));
        }
    }
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let b = ev
            .body_by_id(*id)
            .ok_or_else(|| invalid(format!("unknown body {id}")))?;
        let (mesh, check, kind, used_chord, angle) = match &b.geometry {
            BodyGeometry::Brep(body) => {
                let (mesh, c, a) = tessellate_body(body, &b.name, chord, opts.angle_deg)?;
                // Same document, same bytes: canonical vertex/triangle order.
                let mesh = k::canonical_mesh(&mesh);
                let check = k::check_mesh(&mesh);
                if !check.is_valid_solid() {
                    let detail = check.problems().join("; ");
                    r.mesh_check = Some(check);
                    return Err(invalid(format!(
                        "tessellation of {} is not a valid closed solid ({detail}); nothing \
                         written. This is a kernel/tessellation defect for this body: try another \
                         chord, or report the model",
                        b.name
                    )));
                }
                (mesh, check, "solid", Some(c), Some(a))
            }
            BodyGeometry::Mesh(mb) => {
                let check = k::check_mesh(&k::weld_exact(&mb.mesh));
                if !check.is_valid_solid() && !opts.allow_open_meshes {
                    let detail = check.problems().join("; ");
                    r.mesh_check = Some(check);
                    return Err(invalid(format!(
                        "mesh body {} is not a closed, manifold, outward solid ({detail}); nothing \
                         written. Repair it in a mesh tool, deselect it, or set allow_open_meshes \
                         to export it as a labelled reference mesh",
                        b.name
                    )));
                }
                (mb.mesh.clone(), check, "mesh", None, None)
            }
        };
        let label = if check.is_valid_solid() {
            "valid solid mesh".to_string()
        } else {
            warnings.push(format!(
                "{} exported as an open reference mesh (allow_open_meshes): {}",
                b.name,
                check.problems().join("; ")
            ));
            "open reference mesh (not a printable solid)".to_string()
        };
        if check.degenerate_triangles > 0 {
            warnings.push(format!(
                "{}: {} zero-area triangles",
                b.name, check.degenerate_triangles
            ));
        }
        let (lo, hi) = bounds(&mesh);
        out.push(Prepared {
            report: BodyExport {
                id: *id,
                name: b.name.clone(),
                kind: kind.into(),
                visible: b.visible,
                label,
                triangles: mesh.triangles.len(),
                chord: used_chord,
                max_facet_angle_deg: angle,
                transform: None,
                bounds_min: lo,
                bounds_max: hi,
                dims: std::array::from_fn(|i| hi[i] - lo[i]),
                mesh_check: check,
                fit: None,
                file: None,
            },
            mesh,
            unplaced: None,
        });
    }
    if let Some(p) = &opts.placement {
        place(m, &mut out, p, bed)?;
    }
    for p in &mut out {
        if let Some(bed) = bed {
            p.report.fit = Some(fit(
                p.report.bounds_min,
                p.report.bounds_max,
                bed,
                opts.placement.is_some(),
            ));
        }
    }
    Ok(out)
}

/// Orient, drop to z = 0 and (optionally) lay out the prepared meshes.
fn place(
    m: &Model,
    out: &mut [Prepared],
    p: &Placement,
    bed: Option<&BedReport>,
) -> Result<(), DocError> {
    let ev = m.evaluation();
    let mut placed = Vec::with_capacity(out.len());
    for prep in out.iter() {
        let id = prep.report.id;
        let orient = p
            .bodies
            .iter()
            .find(|bp| bp.body == id)
            .map(|bp| bp.orient)
            .unwrap_or_default();
        let rot = match orient {
            Orient::AsModeled => RigidTransform::IDENTITY,
            Orient::FaceDown(fr) => {
                let face = ev.faces.iter().find(|f| f.reference == fr).ok_or_else(|| {
                    invalid(format!(
                        "face_down: face {:?} of feature {} is not a referenceable planar face of \
                         the current model",
                        fr.role, fr.feature
                    ))
                })?;
                if face.body != id {
                    return Err(invalid(format!(
                        "face_down for body {} names a face of body {}",
                        prep.report.name, face.body
                    )));
                }
                let rot = RigidTransform::rotation_to_minus_z(face.frame.z)
                    .map_err(|e| invalid(e.to_string()))?;
                // The face must be a lowest face, i.e. actually rest on the bed.
                let face_z = rot.apply(face.frame.origin)[2];
                let min_z = prep
                    .mesh
                    .positions
                    .iter()
                    .map(|q| rot.apply(*q)[2])
                    .fold(f64::INFINITY, f64::min);
                let tol = 1e-6 + 10.0 * k::tol::KERNEL_LINEAR;
                if min_z < face_z - tol {
                    return Err(invalid(format!(
                        "face_down: the chosen face of {} would not rest on the bed (the part \
                         extends {:.4} mm below it); choose an outer face",
                        prep.report.name,
                        face_z - min_z
                    )));
                }
                rot
            }
        };
        let rotated = rot.apply_mesh(&prep.mesh);
        let (lo, hi) = bounds(&rotated);
        // Keep the XY centre of the modeled bounding box; lowest point at z = 0.
        let (mlo, mhi) = (prep.report.bounds_min, prep.report.bounds_max);
        let t = [
            0.5 * (mlo[0] + mhi[0]) - 0.5 * (lo[0] + hi[0]),
            0.5 * (mlo[1] + mhi[1]) - 0.5 * (lo[1] + hi[1]),
            -lo[2],
        ];
        placed.push((rot, t, lo, hi));
    }
    if let Some(l) = p.layout {
        let total: f64 = placed
            .iter()
            .map(|(_, _, lo, hi)| hi[0] - lo[0])
            .sum::<f64>()
            + l.gap * (placed.len().saturating_sub(1)) as f64;
        let centre = bed.map_or([0.0, 0.0], |b| {
            [0.5 * (b.min[0] + b.max[0]), 0.5 * (b.min[1] + b.max[1])]
        });
        let mut x = centre[0] - 0.5 * total;
        for (_, t, lo, hi) in placed.iter_mut() {
            t[0] = x - lo[0];
            t[1] = centre[1] - 0.5 * (lo[1] + hi[1]);
            x += hi[0] - lo[0] + l.gap;
        }
    }
    for (prep, (rot, t, _, _)) in out.iter_mut().zip(placed) {
        let tf = rot.then_translate(t);
        let placed = tf.apply_mesh(&prep.mesh);
        prep.unplaced = Some(std::mem::replace(&mut prep.mesh, placed));
        let (lo, hi) = bounds(&prep.mesh);
        prep.report.transform = Some(tf);
        prep.report.bounds_min = lo;
        prep.report.bounds_max = hi;
        prep.report.dims = std::array::from_fn(|i| hi[i] - lo[i]);
        prep.report.mesh_check.bounds_min = lo;
        prep.report.mesh_check.bounds_max = hi;
    }
    Ok(())
}

/// Bed tolerance for fit decisions (mm): bounds within 1 µm of the edge fit.
const FIT_TOL: f64 = 1e-3;

fn fit(lo: [f64; 3], hi: [f64; 3], bed: &BedReport, placed: bool) -> Fit {
    let size = [bed.max[0] - bed.min[0], bed.max[1] - bed.min[1], bed.height];
    let dims: [f64; 3] = std::array::from_fn(|i| hi[i] - lo[i]);
    let over: Vec<String> = ["x", "y", "z"]
        .iter()
        .enumerate()
        .filter(|(i, _)| dims[*i] > size[*i] + FIT_TOL)
        .map(|(i, a)| format!("{a} {:.3} > {:.3}", dims[i], size[i]))
        .collect();
    // Bounding-box overlap with an excluded area (conservative: the footprint
    // may not reach the excluded corner even when the box does).
    let excluded = bed.exclude.iter().any(|[a, b]| {
        lo[0] < b[0] - FIT_TOL
            && hi[0] > a[0] + FIT_TOL
            && lo[1] < b[1] - FIT_TOL
            && hi[1] > a[1] + FIT_TOL
    });
    let inside = lo[0] >= bed.min[0] - FIT_TOL
        && lo[1] >= bed.min[1] - FIT_TOL
        && hi[0] <= bed.max[0] + FIT_TOL
        && hi[1] <= bed.max[1] + FIT_TOL
        && lo[2] >= -FIT_TOL
        && hi[2] <= bed.height + FIT_TOL;
    let within = placed.then_some(inside && !excluded);
    let detail = match (over.is_empty(), within) {
        (false, _) => format!("too large for {}: {}", bed.name, over.join(", ")),
        (true, Some(false)) if inside => format!(
            "fits {} by size but the placed bounding box overlaps an excluded bed area",
            bed.name
        ),
        (true, Some(false)) => format!(
            "fits {} by size but the placed position is outside the printable area",
            bed.name
        ),
        (true, Some(true)) => format!("fits {} at the placed position", bed.name),
        (true, None) => format!(
            "fits {} by size (as-modeled coordinates: position not checked)",
            bed.name
        ),
    };
    Fit {
        fits_size: over.is_empty(),
        within_bed: within,
        detail,
    }
}

/// `<stem>-<name>.stl` component: ASCII letters, digits, `-` and `_` kept; runs
/// of anything else (including `.` and path separators) become one `_`; leading
/// and trailing `_` removed; at most 64 characters; empty becomes `body`.
pub fn sanitize_file_component(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
            s.push(c);
        } else if !s.ends_with('_') {
            s.push('_');
        }
    }
    let s: String = s.trim_matches('_').chars().take(64).collect();
    let s = s.trim_end_matches('_').to_string();
    if s.is_empty() { "body".into() } else { s }
}

fn concat(meshes: &[&TriMesh], chord: f64) -> TriMesh {
    let mut all = TriMesh {
        chord_tolerance: chord,
        ..Default::default()
    };
    for mesh in meshes {
        let base = all.positions.len() as u32;
        all.positions.extend(&mesh.positions);
        all.normals.extend(&mesh.normals);
        all.triangles.extend(
            mesh.triangles
                .iter()
                .map(|t| [t[0] + base, t[1] + base, t[2] + base]),
        );
        all.tri_face.extend(&mesh.tri_face);
    }
    all
}

/// Write `files` (all bytes already produced). Each file is atomic; when one
/// fails, files this call newly created are removed again.
fn write_all(files: &[(PathBuf, Vec<u8>)]) -> Result<(), DocError> {
    let mut created = Vec::new();
    for (path, bytes) in files {
        let existed = path.exists();
        if let Err(e) = write_atomic(path, bytes) {
            for p in &created {
                let _ = std::fs::remove_file(p);
            }
            return Err(e);
        }
        if !existed {
            created.push(path.clone());
        }
    }
    Ok(())
}

fn separate_paths(path: &Path, names: &[(BodyId, String)]) -> Vec<PathBuf> {
    let dir = path.parent().unwrap_or(Path::new(""));
    let stem = path
        .file_stem()
        .map(|s| sanitize_file_component(&s.to_string_lossy()))
        .unwrap_or_else(|| "export".into());
    let mut used: Vec<String> = Vec::new();
    names
        .iter()
        .map(|(id, n)| {
            let mut f = format!("{stem}-{}", sanitize_file_component(n));
            if used.iter().any(|u| u.eq_ignore_ascii_case(&f)) {
                f = format!("{f}-{id}");
            }
            used.push(f.clone());
            dir.join(format!("{f}.stl"))
        })
        .collect()
}

/// Run a print export. `bodies`: explicit selection (may include hidden bodies)
/// or `None` for every visible body.
pub(crate) fn export(
    m: &Model,
    path: &Path,
    chord: f64,
    bodies: Option<&[BodyId]>,
    opts: &PrintOptions,
    format: Format,
    r: &mut Response,
) -> Result<(), DocError> {
    if let Some(a) = opts.angle_deg
        && !(a.is_finite() && a > 0.0 && a < 90.0)
    {
        return Err(invalid(format!("angle_deg must be in (0, 90), got {a}")));
    }
    if opts.item_transforms && !matches!(format, Format::ThreeMf(_)) {
        return Err(invalid(
            "item_transforms is for 3MF (STL has no objects or transforms)".into(),
        ));
    }
    if opts.separate_files && !matches!(format, Format::Stl) {
        return Err(invalid(
            "separate_files is for STL; a 3MF file already keeps one object per body".into(),
        ));
    }
    let ids = crate::export_selection(m, bodies)?;
    let bed = opts.bed.map(|b| b.resolve()).transpose()?;
    let mut warnings = Vec::new();
    let mut prepared = prepare(m, &ids, chord, opts, bed.as_ref(), r, &mut warnings)?;
    let layout_fit = match (&bed, &opts.placement) {
        (Some(bed), Some(_)) => {
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            for p in &prepared {
                for i in 0..3 {
                    lo[i] = lo[i].min(p.report.bounds_min[i]);
                    hi[i] = hi[i].max(p.report.bounds_max[i]);
                }
            }
            Some(fit(lo, hi, bed, true))
        }
        _ => None,
    };
    for p in &prepared {
        if let Some(f) = &p.report.fit
            && (!f.fits_size || f.within_bed == Some(false))
        {
            warnings.push(format!("{}: {}", p.report.name, f.detail));
        }
    }
    let (files, units): (Vec<OutFile>, String) = match format {
        Format::Stl if opts.separate_files => {
            let names: Vec<(BodyId, String)> = prepared
                .iter()
                .map(|p| (p.report.id, p.report.name.clone()))
                .collect();
            let paths = separate_paths(path, &names);
            let mut files = Vec::new();
            for (p, fp) in prepared.iter_mut().zip(paths) {
                let bytes = k::write_stl_binary(&p.mesh, &format!("{} units=mm", p.report.name));
                p.report.file = Some(fp.clone());
                files.push((fp, bytes, vec![p.report.id]));
            }
            (files, STL_UNITS.into())
        }
        Format::Stl => {
            let all = concat(&prepared.iter().map(|p| &p.mesh).collect::<Vec<_>>(), chord);
            let bytes = k::write_stl_binary(&all, "units=mm");
            (
                vec![(path.to_path_buf(), bytes, ids.clone())],
                STL_UNITS.into(),
            )
        }
        Format::ThreeMf(unit) => {
            use rcad_exchange::threemf;
            if opts.item_transforms
                && opts.placement.is_some()
                && unit != threemf::ThreeMfUnit::Millimeter
            {
                // Observed: Bambu Studio 2.8.2.61 refuses to slice such a file
                // ("no object fully inside the print volume") although the
                // file follows the core spec (transforms in model units).
                warnings.push(format!(
                    "item transforms in a {} file: valid 3MF, but Bambu Studio 2.8.2.61 did not \
                     place such objects on the plate in testing; use millimeter for slicers",
                    unit.name()
                ));
            }
            let mut items: Vec<Option<threemf::ItemTransform>> = Vec::new();
            let objects: Vec<threemf::MeshObject> = prepared
                .iter()
                .map(|p| {
                    let (mesh, item) = match (&p.unplaced, &p.report.transform) {
                        (Some(u), Some(tf)) if opts.item_transforms => {
                            // Row-vector convention: M[r][c] = R[c][r].
                            let r = tf.rotation;
                            (
                                u,
                                Some([
                                    [r[0][0], r[1][0], r[2][0]],
                                    [r[0][1], r[1][1], r[2][1]],
                                    [r[0][2], r[1][2], r[2][2]],
                                    tf.translation,
                                ]),
                            )
                        }
                        _ => (&p.mesh, None),
                    };
                    items.push(item);
                    threemf::MeshObject {
                        name: Some(p.report.name.clone()),
                        positions: mesh.positions.clone(),
                        triangles: mesh.triangles.clone(),
                        color: None,
                    }
                })
                .collect();
            let bytes =
                threemf::write_items(&objects, &items, unit).map_err(|e| invalid(e.to_string()))?;
            (
                vec![(path.to_path_buf(), bytes, ids.clone())],
                format!(
                    "{} (declared in the 3MF model's unit attribute); geometry only, no \
                     printer/process/filament settings",
                    unit.name()
                ),
            )
        }
    };
    let to_write: Vec<(PathBuf, Vec<u8>)> = files
        .iter()
        .map(|(p, b, _)| (p.clone(), b.clone()))
        .collect();
    write_all(&to_write)?;
    let placement = match &opts.placement {
        None => "as modeled (document coordinates)".to_string(),
        Some(p) => {
            format!(
                "export-only: lowest point at z = 0{}; document unchanged",
                match p.layout {
                    Some(l) => format!(
                        ", side by side along X with {} mm gaps, centred on {}",
                        l.gap,
                        bed.as_ref()
                            .map_or("the origin".to_string(), |b| format!("the {} bed", b.name))
                    ),
                    None => ", XY centre of each body kept".into(),
                }
            ) + if opts.item_transforms {
                "; objects written as modeled with the placement as each 3MF build item's transform"
            } else {
                ""
            }
        }
    };
    let triangles: usize = prepared.iter().map(|p| p.report.triangles).sum();
    let report = ExportReport {
        format: match format {
            Format::Stl => "stl".into(),
            Format::ThreeMf(_) => "3mf".into(),
        },
        units,
        revision: m.revision,
        chord,
        angle_deg: opts.angle_deg,
        selection: selection_text(m, &ids, bodies.is_some()),
        placement,
        files: files
            .iter()
            .map(|(p, b, ids)| FileExport {
                path: p.clone(),
                bytes: b.len() as u64,
                bodies: ids.clone(),
            })
            .collect(),
        bodies: prepared.into_iter().map(|p| p.report).collect(),
        bed,
        layout_fit,
        warnings,
    };
    if report.bodies.len() == 1 {
        r.mesh_check = Some(report.bodies[0].mesh_check.clone());
    }
    let n = report.bodies.len();
    let what = match &report.format[..] {
        "stl" if report.files.len() > 1 => format!(
            "wrote {} STL files ({triangles} triangles, {n} bodies, chord {chord} mm)",
            report.files.len()
        ),
        "stl" => format!(
            "wrote {} ({triangles} triangles, {n} bod{}, chord {chord} mm)",
            path.display(),
            if n == 1 { "y" } else { "ies" }
        ),
        _ => format!(
            "wrote {} ({n} object{}, unit {}, chord {chord} mm)",
            path.display(),
            if n == 1 { "" } else { "s" },
            match format {
                Format::ThreeMf(u) => u.name(),
                Format::Stl => "",
            }
        ),
    };
    r.message = Some(format!(
        "{what} from committed revision {}; {}; placement {}; units {}{}",
        m.revision,
        report.selection,
        report.placement,
        report.units,
        if report.warnings.is_empty() {
            String::new()
        } else {
            format!("; warnings: {}", report.warnings.join("; "))
        }
    ));
    r.export = Some(report);
    Ok(())
}
