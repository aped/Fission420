//! Versioned native file format (`.f420.json`) with atomic save.
//!
//! Layout: `{ "format": "fission420", "version": 1, "document": {...}, "cache": {...} }`.
//! `document` is authoritative. `cache` is an informational summary of the geometry at
//! save time; loaders regenerate and may compare against it, never trust it.
use crate::{DocError, Document, Evaluation, Result};
use f420_kernel::{Bounds, VolumeResult};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

pub const FORMAT_NAME: &str = "fission420";
pub const FORMAT_VERSION: u32 = 1;
/// Refuse to parse native files larger than this.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

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
        let cache = eval
            .and_then(|e| e.measurements.as_ref())
            .map(|m| GeometryCache {
                volume: match m.volume {
                    VolumeResult::Value { volume, .. } => Some(volume),
                    VolumeResult::Unavailable { .. } => None,
                },
                bounds: Some(m.bounds),
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

/// Save atomically: write a sibling temp file, fsync, rename over the target, fsync dir.
/// On any error the previous file at `path` is untouched.
pub fn save_file(path: &Path, doc: &Document, eval: Option<&Evaluation>) -> Result<()> {
    let ferr =
        |what: &str, e: std::io::Error| DocError::File(format!("{what} {}: {e}", path.display()));
    let json = serde_json::to_vec_pretty(&NativeFile::new(doc, eval))
        .map_err(|e| DocError::File(format!("serialize: {e}")))?;
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let fname = path
        .file_name()
        .ok_or_else(|| DocError::File(format!("not a file path: {}", path.display())))?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        fname.to_string_lossy(),
        std::process::id()
    ));
    let res = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&json)?;
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
        return Err(ferr("save", e));
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
    if h.format.as_deref() != Some(FORMAT_NAME) {
        return Err(DocError::File(format!(
            "not a {FORMAT_NAME} document (format = {:?})",
            h.format
        )));
    }
    match h.version {
        Some(FORMAT_VERSION) => {}
        Some(v) if v > FORMAT_VERSION => {
            return Err(DocError::File(format!(
                "file version {v} is newer than supported {FORMAT_VERSION}"
            )));
        }
        v => return Err(DocError::File(format!("unsupported file version {v:?}"))),
    }
    let nf: NativeFile = serde_json::from_slice(bytes)
        .map_err(|e| DocError::File(format!("invalid document structure: {e}")))?;
    nf.document.evaluation_order()?;
    for f in &nf.document.features {
        if let crate::FeatureKind::Sketch { sketch, .. } = &f.kind {
            sketch
                .validate()
                .map_err(|e| DocError::File(format!("{}: invalid sketch: {e}", f.id)))?;
        }
    }
    Ok(nf)
}
