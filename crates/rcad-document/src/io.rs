//! Versioned native file format (`.rcad.json`) with atomic save.
//!
//! Layout: `{ "format": "r-cad", "version": 4, "document": {...}, "cache": {...} }`.
//! Files written before the rename (2026-10-13) say `"format": "fission420"` and are
//! usually named `*.f420.json`; they still load. The rename changed only the format name,
//! not the version or the document. The file extension is never checked.
//! `document` is authoritative. `cache` is an informational summary of the geometry at
//! save time; loaders regenerate and may compare against it, never trust it.
use crate::{DocError, Document, Evaluation, Result};
use rcad_kernel::{Bounds, VolumeResult};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

pub const FORMAT_NAME: &str = "r-cad";
/// Format names written by earlier builds (the project was called Fission420 before
/// 2026-10-13). Accepted on load; never written.
pub const LEGACY_FORMAT_NAMES: &[&str] = &["fission420"];
/// v2 (run 2): sketches may carry `origin` and `Distance` constraints, and the
/// history may contain `Pocket` features. v1 files are a strict subset of v2.
/// v3 (run 3, ADR 0003): bodies, blobs, extrude operations/extents, imports. v1/v2
/// files are migrated on load (see [`migrate_to_v3`]).
/// v4 (run 4, docs/PARAMETERS.md): named user parameters and feature expression
/// bindings (additive; a v3 document is a v4 document without them). Saves
/// write v4, so older builds refuse the file instead of dropping expressions.
pub const FORMAT_VERSION: u32 = 4;
/// Oldest version this build reads.
pub const MIN_FORMAT_VERSION: u32 = 1;
/// Refuse to parse native files larger than this (v3 embeds imported source files).
pub const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// The saved [`GeometryCache`] values are rounded to 1/this (1e-9 mm for
/// bounds, 1e-9 mm^3 for volume).
pub const CACHE_STEPS_PER_UNIT: f64 = 1.0e9;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeometryCache {
    pub volume: Option<f64>,
    pub bounds: Option<Bounds>,
    pub face_count: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeFile {
    pub format: String,
    pub version: u32,
    pub document: Document,
    #[serde(default)]
    pub cache: Option<GeometryCache>,
}

impl NativeFile {
    pub fn new(doc: &Document, eval: Option<&Evaluation>) -> Self {
        // Rounded to 1e-9 (mm, mm^3): Truck's B-rep vertices carry ~1e-15 mm of
        // noise that differs between regenerations, which would otherwise make
        // two saves of the same document differ in the last digits.
        // Dividing by the exact integer 1e9 gives the nearest double to the decimal.
        let r = |x: f64| (x * CACHE_STEPS_PER_UNIT).round() / CACHE_STEPS_PER_UNIT + 0.0;
        let cache = eval
            .and_then(|e| e.measurements.as_ref())
            .map(|m| GeometryCache {
                volume: match m.volume {
                    VolumeResult::Value { volume, .. } => Some(r(volume)),
                    VolumeResult::Unavailable { .. } => None,
                },
                bounds: Some(Bounds {
                    min: m.bounds.min.map(r),
                    max: m.bounds.max.map(r),
                }),
                face_count: Some(m.face_count),
            });
        Self {
            format: FORMAT_NAME.into(),
            version: FORMAT_VERSION,
            document: doc.clone(),
            cache,
        }
    }
}

/// Save atomically (see [`write_atomic`]). On any error the previous file is untouched.
pub fn save_file(path: &Path, doc: &Document, eval: Option<&Evaluation>) -> Result<()> {
    let json = serde_json::to_vec_pretty(&NativeFile::new(doc, eval))
        .map_err(|e| DocError::File(format!("serialize: {e}")))?;
    write_atomic(path, &json)
}

/// Write `bytes` to `path` atomically: unique sibling temp file (`create_new`), fsync,
/// copy the target's permissions if it exists, rename over the target, fsync the
/// directory (best effort). The rename replaces a symlink at `path` rather than its
/// target (documented limitation).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let fname = path
        .file_name()
        .ok_or_else(|| DocError::File(format!("not a file path: {}", path.display())))?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(
        ".{}.tmp-{}-{nonce}-{seq}",
        fname.to_string_lossy(),
        std::process::id()
    ));
    let res = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        if let Ok(meta) = std::fs::metadata(path) {
            f.set_permissions(meta.permissions())?;
        }
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)?;
        if let Ok(d) = std::fs::File::open(&dir) {
            let _ = d.sync_all(); // directory fsync is best-effort (not all filesystems support it)
        }
        Ok(())
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(DocError::File(format!("write {}: {e}", path.display())));
    }
    Ok(())
}

/// Load and validate a native file. Does not regenerate (see `Model::from_document`).
pub fn load_file(path: &Path) -> Result<NativeFile> {
    let ferr = |e: std::io::Error| DocError::File(format!("open {}: {e}", path.display()));
    let meta = std::fs::metadata(path).map_err(ferr)?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(DocError::File(format!(
            "file too large ({} bytes, max {MAX_FILE_BYTES})",
            meta.len()
        )));
    }
    let bytes = std::fs::read(path).map_err(ferr)?;
    parse(&bytes)
}

pub(crate) fn parse(bytes: &[u8]) -> Result<NativeFile> {
    // Check the header first so wrong-format files give a precise error.
    #[derive(Deserialize)]
    struct Header {
        format: Option<String>,
        version: Option<u32>,
    }
    let h: Header = serde_json::from_slice(bytes)
        .map_err(|e| DocError::File(format!("not valid JSON: {e}")))?;
    let known = h
        .format
        .as_deref()
        .is_some_and(|f| f == FORMAT_NAME || LEGACY_FORMAT_NAMES.contains(&f));
    if !known {
        return Err(DocError::File(format!(
            "not an {FORMAT_NAME} document (format = {:?})",
            h.format
        )));
    }
    match h.version {
        Some(v) if (MIN_FORMAT_VERSION..=FORMAT_VERSION).contains(&v) => {}
        Some(v) if v > FORMAT_VERSION => {
            return Err(DocError::File(format!(
                "file version {v} is newer than supported {FORMAT_VERSION}"
            )));
        }
        v => return Err(DocError::File(format!("unsupported file version {v:?}"))),
    }
    let mut nf: NativeFile = serde_json::from_slice(bytes)
        .map_err(|e| DocError::File(format!("invalid document structure: {e}")))?;
    if nf.version < 3 {
        // `nf.version` keeps the version read (so callers can report the
        // migration); `NativeFile::new` always writes FORMAT_VERSION.
        migrate_to_v3(&mut nf.document)?;
    }
    if nf.version < 4
        && (!nf.document.parameters.is_empty()
            || nf
                .document
                .features
                .iter()
                .any(|f| !f.expressions.is_empty()))
    {
        return Err(DocError::File(format!(
            "a version {} file cannot contain parameters or expressions (v4 feature)",
            nf.version
        )));
    }
    nf.document.evaluation_order()?;
    nf.document.validate_bodies()?;
    for f in &nf.document.features {
        if let crate::FeatureKind::Sketch { sketch, .. } = &f.kind {
            sketch
                .validate()
                .map_err(|e| DocError::File(format!("{}: invalid sketch: {e}", f.id)))?;
        }
    }
    Ok(nf)
}

/// v1/v2 -> v3 (ADR 0003). Older documents have one implicit base body: every
/// body-creating feature (Extrude/Revolve) gets a fresh body id in history order
/// ("Body1", ...), and every legacy Cut/Pocket targets the first one. Feature ids
/// and geometry are unchanged. Must only be called on documents without bodies.
pub fn migrate_to_v3(doc: &mut Document) -> Result<()> {
    use crate::{FeatureKind, Operation};
    if !doc.bodies.is_empty() || !doc.blobs.is_empty() {
        return Err(DocError::File(
            "pre-v3 file must not contain bodies or embedded files".into(),
        ));
    }
    doc.next_body_id = 1;
    doc.next_blob_id = 1;
    let mut base = None;
    for i in 0..doc.features.len() {
        let new_body = matches!(
            doc.features[i].kind,
            FeatureKind::Extrude { .. } | FeatureKind::Revolve { .. }
        );
        if new_body {
            let name = doc.default_body_name();
            let id = doc.add_body(name);
            base.get_or_insert(id);
            match &mut doc.features[i].kind {
                FeatureKind::Extrude { operation, .. } => {
                    *operation = Operation::NewBody { body: id }
                }
                FeatureKind::Revolve { body, .. } => *body = id,
                _ => unreachable!(),
            }
        }
    }
    for f in &mut doc.features {
        if let FeatureKind::Cut { target, .. } | FeatureKind::Pocket { target, .. } = &mut f.kind {
            *target = base.ok_or_else(|| DocError::BrokenReference {
                from: f.id,
                detail: "cut has no base body to cut".into(),
            })?;
        }
    }
    Ok(())
}
