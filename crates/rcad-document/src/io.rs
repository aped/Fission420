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
/// v5 (run 5): feature suppression, face references to pattern/mirror
/// occurrences and chamfer faces (`FaceRole::Copy`, `FaceRole::Chamfer`), and
/// the `TangentArcs` sketch constraint (all additive). A document is written
/// as v5 only when it uses one of them ([`required_version`]); otherwise it
/// stays v4, byte for byte as before, so older builds keep reading it and
/// refuse (rather than misread) a v5 file.
/// v6 (run 6): extrusion options (`ExtrudeOptions`: symmetric, two sides,
/// start offset, up to a plane, thin, taper) and the Intersect operation, plus
/// the other run-6 additions listed in [`v6_feature`]. Written only when used
/// (otherwise v5 or v4 as before), so a run-5 build refuses the file instead
/// of silently ignoring a field it does not know.
/// v7 (run 7): the run-7 additions listed in [`v7_feature`] (nested
/// assemblies, more joint types, drawing annotations, sheet-metal rules,
/// surfaces, ...). Written only when used (otherwise v6 or older as before).
pub const FORMAT_VERSION: u32 = 7;
/// Oldest version this build reads.
pub const MIN_FORMAT_VERSION: u32 = 1;
/// Refuse to parse native files larger than this (v3 embeds imported source files).
pub const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// The saved [`GeometryCache`] bounds are rounded to 1/this (1e-9 mm).
pub const CACHE_STEPS_PER_UNIT: f64 = 1.0e9;
/// The saved cache volume is rounded to 1/this (1e-6 mm³; run 5): a volume
/// integrated over Truck boolean results varies by ~1e-12 relative between
/// regenerations (observed 49303.902664472 vs …473 on one binary), which
/// straddled the 1e-9 rounding and made saves of the same document differ.
pub const CACHE_VOLUME_STEPS: f64 = 1.0e6;

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
                    VolumeResult::Value { volume, .. } => {
                        Some((volume * CACHE_VOLUME_STEPS).round() / CACHE_VOLUME_STEPS + 0.0)
                    }
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
            version: required_version(doc),
            document: doc.clone(),
            cache,
        }
    }
}

/// What in `doc` needs format v5, if anything (see [`FORMAT_VERSION`]).
pub fn v5_feature(doc: &Document) -> Option<&'static str> {
    if doc.features.iter().any(|f| f.suppressed) {
        return Some("suppressed features");
    }
    if doc
        .features
        .iter()
        .any(|f| matches!(f.kind, crate::FeatureKind::Fillet { .. }))
    {
        return Some("solid edge fillets");
    }
    if doc
        .features
        .iter()
        .any(|f| matches!(f.kind, crate::FeatureKind::Loft { .. }))
    {
        return Some("lofts");
    }
    if doc
        .features
        .iter()
        .any(|f| matches!(f.kind, crate::FeatureKind::Sweep { .. }))
    {
        return Some("sweeps");
    }
    // Run-5 parameter ranges / slots a run-4 build refuses with a misleading
    // message (review run 5): a pattern of count 1, and `plane_offset`
    // bindings on a Mirror or Hole.
    if doc.features.iter().any(|f| {
        matches!(
            f.kind,
            crate::FeatureKind::LinearPattern { count: 1, .. }
                | crate::FeatureKind::CircularPattern { count: 1, .. }
        ) || (matches!(
            f.kind,
            crate::FeatureKind::Mirror { .. } | crate::FeatureKind::Hole { .. }
        ) && f.expressions.contains_key("plane_offset"))
    }) {
        return Some("a pattern of count 1 or a bound mirror/hole plane offset");
    }
    fn scan(v: &serde_json::Value) -> Option<&'static str> {
        match v {
            serde_json::Value::Object(m) => {
                for (k, x) in m {
                    if k == "role"
                        && (x.as_str() == Some("Chamfer")
                            || x.as_object().is_some_and(|o| o.contains_key("Copy")))
                    {
                        return Some("references to pattern/mirror/chamfer faces");
                    }
                    if k == "Offset" && x.as_object().is_some_and(|o| o.contains_key("angle")) {
                        return Some("angled construction planes");
                    }
                    if k == "TangentArcs" {
                        return Some("arc-arc tangency (line-arc sketch fillets)");
                    }
                    if let Some(r) = scan(x) {
                        return Some(r);
                    }
                }
                None
            }
            serde_json::Value::Array(a) => a.iter().find_map(scan),
            _ => None,
        }
    }
    serde_json::to_value(&doc.features)
        .ok()
        .as_ref()
        .and_then(scan)
}

/// What in `doc` needs format v6, if anything (see [`FORMAT_VERSION`]).
/// Every run-6 addition that an older build would ignore or misread must be
/// listed here.
pub fn v6_feature(doc: &Document) -> Option<&'static str> {
    if doc.rollback.is_some() {
        return Some("a rolled-back timeline marker");
    }
    if !doc.configurations.is_empty() || doc.active_configuration.is_some() {
        return Some("configurations");
    }
    if !doc.assembly.is_empty() {
        return Some("components, occurrences and joints");
    }
    if doc.bodies.iter().any(|b| b.material.is_some()) {
        return Some("body materials");
    }
    if !doc.drawings.is_empty() {
        return Some("drawings");
    }
    for f in &doc.features {
        if matches!(f.kind, crate::FeatureKind::DirectEdit { .. }) {
            return Some("direct edits of imported bodies");
        }
        if matches!(f.kind, crate::FeatureKind::Countersink { .. }) {
            return Some("countersinks");
        }
        if let crate::FeatureKind::Revolve {
            axis,
            reverse,
            merge,
            ..
        } = &f.kind
            && (!axis.is_default() || *reverse || merge.is_some())
        {
            return Some("revolve axis/direction options");
        }
        if let crate::FeatureKind::Loft { via, .. } = &f.kind
            && !via.is_empty()
        {
            return Some("multi-section lofts");
        }
        if matches!(f.kind, crate::FeatureKind::AlignBody { .. }) {
            return Some("aligned bodies");
        }
        if matches!(f.kind, crate::FeatureKind::ScaleBody { .. }) {
            return Some("scaled bodies");
        }
        if matches!(f.kind, crate::FeatureKind::ShellFace { .. }) {
            return Some("shells from a face");
        }
        if matches!(f.kind, crate::FeatureKind::SheetMetal { .. }) {
            return Some("sheet-metal flanges");
        }
        if matches!(f.kind, crate::FeatureKind::SplitBody { .. }) {
            return Some("split bodies");
        }
        if matches!(f.kind, crate::FeatureKind::Thread { .. }) {
            return Some("modeled threads");
        }
        if matches!(f.kind, crate::FeatureKind::MoveBody { .. }) {
            return Some("body moves/copies");
        }
        if matches!(f.kind, crate::FeatureKind::Combine { .. }) {
            return Some("body combines");
        }
        if let crate::FeatureKind::Sketch { links, sketch, .. } = &f.kind {
            if !links.is_empty() {
                return Some("projected sketch geometry (projection links)");
            }
            if sketch.constraints.iter().any(|c| {
                matches!(
                    c,
                    rcad_sketch::Constraint::FixedX { .. } | rcad_sketch::Constraint::FixedY { .. }
                )
            }) {
                return Some("single-coordinate fixed constraints");
            }
        }
        if let crate::FeatureKind::Extrude {
            options, operation, ..
        } = &f.kind
        {
            if !options.is_plain() {
                return Some(
                    "extrusion options (symmetric, two sides, offset start, to plane, thin, taper)",
                );
            }
            if matches!(operation, crate::Operation::Intersect { .. }) {
                return Some("intersect extrusions");
            }
        }
    }
    sketch_v6_feature(doc)
}

/// What in `doc` needs format v7, if anything (see [`FORMAT_VERSION`]).
/// Every run-7 addition that an older build would ignore or misread must be
/// listed here. Each area keeps its checks in its own helper below so
/// parallel work merges cleanly.
pub fn v7_feature(doc: &Document) -> Option<&'static str> {
    assembly_v7_feature(doc)
        .or_else(|| drawing_v7_feature(doc))
        .or_else(|| part_v7_feature(doc))
}

/// Run-7 assembly additions (nesting, joint types, ...).
fn assembly_v7_feature(doc: &Document) -> Option<&'static str> {
    let a = &doc.assembly;
    if a.occurrences.iter().any(|o| o.owner.is_some()) {
        return Some("nested sub-assembly occurrences");
    }
    if a.components.iter().any(|c| c.bodies.is_empty()) {
        return Some("components without bodies");
    }
    if a.components.iter().any(|c| c.derived_from.is_some()) {
        return Some("independent component copies");
    }
    if a.occurrences.iter().any(|o| o.hidden) {
        return Some("hidden occurrences");
    }
    if a.active.is_some() {
        return Some("an active component");
    }
    if !a.explode.is_empty() || a.exploded {
        return Some("exploded-view offsets");
    }
    if a.joints
        .iter()
        .any(|j| j.kind.is_v7() || !j.extra.is_empty() || j.dir2.is_some())
    {
        return Some("cylindrical / pin-slot / planar / ball joints");
    }
    None
}

/// Run-7 drawing / configuration / property additions.
fn drawing_v7_feature(_doc: &Document) -> Option<&'static str> {
    None
}

/// Run-7 part-modeling additions (sheet metal, surfaces, ...).
fn part_v7_feature(_doc: &Document) -> Option<&'static str> {
    None
}

/// Run-6 sketch additions (worker "sketch"): sketch text (`Sketch::texts`) and
/// sketch pattern constraints (`PatternStep`, `PatternRotate`) need format v6;
/// a run-5 build would silently drop unknown fields or refuse with a misleading
/// message. Separate from the other v6 checks so it can be folded into
/// `v6_feature`.
pub fn sketch_v6_feature(doc: &Document) -> Option<&'static str> {
    let sketches = doc.features.iter().filter_map(|f| match &f.kind {
        crate::FeatureKind::Sketch { sketch, .. } => Some(sketch),
        _ => None,
    });
    for s in sketches {
        if !s.texts.is_empty() {
            return Some("sketch text");
        }
        if s.constraints.iter().any(|c| {
            matches!(
                c,
                rcad_sketch::Constraint::PatternStep { .. }
                    | rcad_sketch::Constraint::PatternRotate { .. }
            )
        }) {
            return Some("sketch patterns");
        }
    }
    None
}

/// The oldest format version that can hold `doc`: 7 / 6 / 5 when it uses a
/// run-7 / run-6 / run-5 feature, else 4.
pub fn required_version(doc: &Document) -> u32 {
    if v7_feature(doc).is_some() {
        7
    } else if v6_feature(doc).is_some() {
        6
    } else if v5_feature(doc).is_some() {
        5
    } else {
        4
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
    if nf.version < 5
        && let Some(what) = v5_feature(&nf.document)
    {
        return Err(DocError::File(format!(
            "a version {} file cannot contain {what} (v5 feature)",
            nf.version
        )));
    }
    if nf.version < 6
        && let Some(what) = v6_feature(&nf.document)
    {
        return Err(DocError::File(format!(
            "a version {} file cannot contain {what} (v6 feature)",
            nf.version
        )));
    }
    if nf.version < 7
        && let Some(what) = v7_feature(&nf.document)
    {
        return Err(DocError::File(format!(
            "a version {} file cannot contain {what} (v7 feature)",
            nf.version
        )));
    }
    nf.document.evaluation_order()?;
    nf.document.validate_bodies()?;
    // Configurations may only name features of this document (review run 6 #4).
    for c in &nf.document.configurations {
        for f in &c.suppressed {
            if nf.document.feature(*f).is_err() {
                return Err(DocError::File(format!(
                    "configuration '{}' suppresses missing feature {f}",
                    c.name
                )));
            }
        }
    }
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
