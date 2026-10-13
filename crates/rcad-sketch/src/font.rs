//! Installed-font discovery and outline extraction for sketch text
//! ([`crate::text`]), without fontconfig or any C library.
//!
//! Discovery scans font directories for `.ttf`, `.otf`, `.ttc` and `.otc` files
//! and reads only each file's table directory, `name` and `OS/2` tables (a few
//! kilobytes per file). Directories, in order: `RCAD_FONT_DIRS` (`:`-separated),
//! `$XDG_DATA_HOME/fonts` (default `~/.local/share/fonts`), `~/.fonts`,
//! `/usr/local/share/fonts`, `/usr/share/fonts`; on macOS also
//! `~/Library/Fonts`, `/Library/Fonts`, `/System/Library/Fonts`; on Windows
//! `%LOCALAPPDATA%\\Microsoft\\Windows\\Fonts` and `%WINDIR%\\Fonts`
//! (`RCAD_FONT_DIRS` is `;`-separated there). Missing directories are skipped.
//! The index is built once per process.
//!
//! Matching ([`FontSpec`]): the family name compares case-insensitively with the
//! typographic family (name ID 16) or the legacy family (name ID 1); the style
//! prefers an exact bold/italic match, then "Regular". Ties break on the path
//! (then the collection index), so the choice is deterministic for a given set of
//! installed files. There is no fallback to another family: a family that is not
//! installed is an error ([`crate::SketchError::FontNotFound`]).
//!
//! Outlines come from `ttf-parser` (pure Rust, MIT OR Apache-2.0): TrueType
//! quadratic and CFF cubic Béziers, composite glyphs resolved. Layout is the
//! horizontal advance only (no kerning, ligatures, shaping or bidi); a character
//! without a glyph is refused, never replaced by `.notdef`.

use crate::SketchError;
use crate::text::{FontSpec, GlyphContour, OutlineSeg, ResolvedFont, ResolvedGlyph, ResolvedText};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Font files larger than this are not loaded (allocation bound), bytes.
pub const MAX_FONT_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// At most this many font files are indexed (directory-walk bound).
const MAX_INDEXED_FILES: usize = 20_000;
/// Directory recursion bound.
const MAX_DIR_DEPTH: usize = 8;

/// One face of an installed font file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledFont {
    pub path: PathBuf,
    /// Face index inside a collection (`.ttc`), else 0.
    pub index: u32,
    /// Typographic family (name ID 16) or family (ID 1).
    pub family: String,
    /// Typographic subfamily (ID 17) or subfamily (ID 2), e.g. "Bold Italic".
    pub style: String,
    pub bold: bool,
    pub italic: bool,
}

/// Font directories searched, in priority order (see the module docs).
pub fn font_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    if let Some(extra) = std::env::var_os("RCAD_FONT_DIRS") {
        v.extend(std::env::split_paths(&extra).filter(|p| !p.as_os_str().is_empty()));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match std::env::var_os("XDG_DATA_HOME") {
        Some(x) if !x.is_empty() => v.push(PathBuf::from(x).join("fonts")),
        _ => {
            if let Some(h) = &home {
                v.push(h.join(".local/share/fonts"));
            }
        }
    }
    if let Some(h) = &home {
        v.push(h.join(".fonts"));
    }
    v.push(PathBuf::from("/usr/local/share/fonts"));
    v.push(PathBuf::from("/usr/share/fonts"));
    // macOS (docs/CI.md F4): user, local and system font folders.
    if cfg!(target_os = "macos") {
        if let Some(h) = &home {
            v.push(h.join("Library/Fonts"));
        }
        v.push(PathBuf::from("/Library/Fonts"));
        v.push(PathBuf::from("/System/Library/Fonts"));
    }
    // Windows: per-user fonts, then the system Fonts folder.
    if cfg!(windows) {
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            v.push(
                PathBuf::from(l)
                    .join("Microsoft")
                    .join("Windows")
                    .join("Fonts"),
            );
        }
        let win = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
        v.push(PathBuf::from(win).join("Fonts"));
    }
    v
}

/// Every installed face (cached for the process).
pub fn installed_fonts() -> &'static [InstalledFont] {
    static INDEX: OnceLock<Vec<InstalledFont>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut files = Vec::new();
        for d in font_dirs() {
            walk(&d, 0, &mut files);
        }
        files.sort();
        files.dedup();
        let mut out = Vec::new();
        for f in files {
            if let Ok(faces) = index_file(&f) {
                out.extend(faces);
            }
        }
        out
    })
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DIR_DEPTH || out.len() >= MAX_INDEXED_FILES {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        if out.len() >= MAX_INDEXED_FILES {
            return;
        }
        if p.is_dir() {
            walk(&p, depth + 1, out);
        } else if is_font_file(&p) {
            out.push(p);
        }
    }
}

fn is_font_file(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "ttf" | "otf" | "ttc" | "otc"
        )
    })
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
        *b.get(at + 2)?,
        *b.get(at + 3)?,
    ]))
}

fn read_at(f: &mut File, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

/// Faces of one file from its table directories plus `name`/`OS/2` tables.
fn index_file(path: &Path) -> std::io::Result<Vec<InstalledFont>> {
    let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "not an sfnt font");
    let mut f = File::open(path)?;
    let size = f.metadata()?.len();
    let head = read_at(&mut f, 0, 12.min(size as usize))?;
    let dirs: Vec<u64> = if head.get(0..4) == Some(b"ttcf") {
        let n = be32(&head, 8).ok_or_else(bad)?.min(256) as usize;
        let offs = read_at(&mut f, 12, 4 * n)?;
        (0..n)
            .map(|i| be32(&offs, 4 * i).map(u64::from).ok_or_else(bad))
            .collect::<Result<_, _>>()?
    } else {
        vec![0]
    };
    let mut out = Vec::new();
    for (index, dir) in dirs.into_iter().enumerate() {
        let hdr = read_at(&mut f, dir, 12)?;
        let n = be16(&hdr, 4).ok_or_else(bad)? as usize;
        let recs = read_at(&mut f, dir + 12, 16 * n)?;
        let table = |tag: &[u8; 4], f: &mut File| -> std::io::Result<Option<Vec<u8>>> {
            for i in 0..n {
                let r = &recs[16 * i..16 * i + 16];
                if &r[0..4] == tag {
                    let off = u64::from(be32(r, 8).ok_or_else(bad)?);
                    let len = u64::from(be32(r, 12).ok_or_else(bad)?);
                    if off.saturating_add(len) > size || len > 4 * 1024 * 1024 {
                        return Err(bad());
                    }
                    return read_at(f, off, len as usize).map(Some);
                }
            }
            Ok(None)
        };
        let Some(name) = table(b"name", &mut f)? else {
            continue;
        };
        let os2 = table(b"OS/2", &mut f)?;
        let Some(names) = ttf_parser::name::Table::parse(&name) else {
            continue;
        };
        let get = |id: u16| -> Option<String> {
            names
                .names
                .into_iter()
                .filter(|n| n.name_id == id && n.is_unicode())
                // English (US) first, then any language, deterministic order.
                .min_by_key(|n| (n.language_id != 0x409, n.language_id))
                .and_then(|n| n.to_string())
                .filter(|s| !s.trim().is_empty())
        };
        let Some(family) = get(ttf_parser::name_id::TYPOGRAPHIC_FAMILY)
            .or_else(|| get(ttf_parser::name_id::FAMILY))
        else {
            continue;
        };
        let style = get(ttf_parser::name_id::TYPOGRAPHIC_SUBFAMILY)
            .or_else(|| get(ttf_parser::name_id::SUBFAMILY))
            .unwrap_or_else(|| "Regular".into());
        let lower = style.to_ascii_lowercase();
        // fsSelection bit 0 italic, bit 5 bold (OS/2 offset 62).
        let sel = os2.as_deref().and_then(|t| be16(t, 62)).unwrap_or(0);
        out.push(InstalledFont {
            path: path.to_path_buf(),
            index: index as u32,
            family,
            bold: sel & (1 << 5) != 0 || lower.contains("bold"),
            italic: sel & 1 != 0 || lower.contains("italic") || lower.contains("oblique"),
            style,
        });
    }
    Ok(out)
}

/// The installed face for `spec` (family match; see the module docs), or
/// `FontNotFound` naming what was looked for.
pub fn find_font(spec: &FontSpec) -> Result<InstalledFont, SketchError> {
    if let Some(p) = &spec.path {
        let path = PathBuf::from(p);
        let faces = index_file(&path)
            .map_err(|e| SketchError::FontNotFound(format!("font file {p}: {e}")))?;
        let want = spec.family.trim();
        return faces
            .into_iter()
            .find(|f| want.is_empty() || f.family.eq_ignore_ascii_case(want))
            .ok_or_else(|| {
                SketchError::FontNotFound(format!("font file {p} has no face of family '{want}'"))
            });
    }
    let want = spec.family.trim();
    if want.is_empty() {
        return Err(SketchError::FontNotFound(
            "no font family or file given".into(),
        ));
    }
    let score = |f: &InstalledFont| -> u32 {
        let mut s = 0;
        if f.bold != spec.bold {
            s += 4;
        }
        if f.italic != spec.italic {
            s += 4;
        }
        let l = f.style.to_ascii_lowercase();
        let plain = matches!(
            l.as_str(),
            "regular" | "bold" | "italic" | "bold italic" | "book"
        );
        if !plain {
            s += 1;
        }
        s
    };
    installed_fonts()
        .iter()
        .filter(|f| f.family.eq_ignore_ascii_case(want))
        .min_by(|a, b| (score(a), &a.path, a.index).cmp(&(score(b), &b.path, b.index)))
        .cloned()
        .ok_or_else(|| {
            SketchError::FontNotFound(format!(
                "font family '{want}' is not installed (searched {}); stored text keeps its \
                 outlines, but changing its string or font needs an installed font",
                font_dirs()
                    .iter()
                    .map(|d| d.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

/// FNV-1a 64-bit hash, lower-case hex (content identity of a font file, not a
/// security hash).
pub fn fnv1a64_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

struct Builder {
    contours: Vec<GlyphContour>,
    cur: Option<GlyphContour>,
}

impl Builder {
    fn finish(&mut self) {
        if let Some(c) = self.cur.take()
            && !c.segs.is_empty()
        {
            self.contours.push(c);
        }
    }
    fn push(&mut self, s: OutlineSeg) {
        if let Some(c) = &mut self.cur {
            c.segs.push(s);
        }
    }
}

impl ttf_parser::OutlineBuilder for Builder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish();
        self.cur = Some(GlyphContour {
            start: [f64::from(x), f64::from(y)],
            segs: vec![],
        });
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.push(OutlineSeg::Line {
            to: [f64::from(x), f64::from(y)],
        });
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.push(OutlineSeg::Quad {
            c: [f64::from(x1), f64::from(y1)],
            to: [f64::from(x), f64::from(y)],
        });
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.push(OutlineSeg::Cubic {
            c1: [f64::from(x1), f64::from(y1)],
            c2: [f64::from(x2), f64::from(y2)],
            to: [f64::from(x), f64::from(y)],
        });
    }
    fn close(&mut self) {
        self.finish();
    }
}

/// Resolve `text` with the font `spec`: per-character outlines in font units,
/// pen positions (advance widths) and the font's identity. Control characters
/// (including line breaks) and characters without a glyph are refused.
pub fn resolve(text: &str, spec: &FontSpec) -> Result<ResolvedText, SketchError> {
    let face_info = find_font(spec)?;
    let path = &face_info.path;
    let size = std::fs::metadata(path)
        .map_err(|e| SketchError::FontNotFound(format!("{}: {e}", path.display())))?
        .len();
    if size > MAX_FONT_FILE_BYTES {
        return Err(SketchError::Text(format!(
            "font file {} is {size} bytes (limit {MAX_FONT_FILE_BYTES})",
            path.display()
        )));
    }
    let data = std::fs::read(path)
        .map_err(|e| SketchError::FontNotFound(format!("{}: {e}", path.display())))?;
    resolve_with_data(text, &data, &face_info)
}

/// [`resolve`] on already-loaded font bytes (`face` names the face to use).
pub fn resolve_with_data(
    text: &str,
    data: &[u8],
    face: &InstalledFont,
) -> Result<ResolvedText, SketchError> {
    let f = ttf_parser::Face::parse(data, face.index).map_err(|e| {
        SketchError::Text(format!(
            "font {} cannot be parsed: {e}",
            face.path.display()
        ))
    })?;
    let upem = f64::from(f.units_per_em());
    if upem <= 0.0 {
        return Err(SketchError::Text("font has units_per_em = 0".into()));
    }
    let cap = f
        .capital_height()
        .filter(|h| *h > 0)
        .map(f64::from)
        .or_else(|| {
            f.glyph_index('H')
                .and_then(|g| f.glyph_bounding_box(g))
                .map(|r| f64::from(r.y_max))
                .filter(|h| *h > 0.0)
        })
        .ok_or_else(|| {
            SketchError::Text("font declares no cap height and has no 'H' glyph".into())
        })?;
    if text.is_empty() {
        return Err(SketchError::Text("text is empty".into()));
    }
    let n = text.chars().count();
    if n > crate::text::MAX_GLYPHS {
        return Err(SketchError::Text(format!(
            "text has {n} characters (limit {})",
            crate::text::MAX_GLYPHS
        )));
    }
    let mut missing = Vec::new();
    let mut glyphs = Vec::with_capacity(n);
    let mut pen = 0.0;
    for ch in text.chars() {
        if ch.is_control() {
            return Err(SketchError::Text(format!(
                "character U+{:04X} is a control character (sketch text is one line)",
                ch as u32
            )));
        }
        let Some(gid) = f.glyph_index(ch) else {
            missing.push(ch);
            continue;
        };
        let mut b = Builder {
            contours: vec![],
            cur: None,
        };
        // `None` for glyphs without an outline (space): no contours.
        let _ = f.outline_glyph(gid, &mut b);
        b.finish();
        glyphs.push(ResolvedGlyph {
            ch,
            x: pen,
            contours: b.contours,
        });
        pen += f64::from(f.glyph_hor_advance(gid).unwrap_or(0));
    }
    if !missing.is_empty() {
        return Err(SketchError::Text(format!(
            "font '{}' has no glyph for {}",
            face.family,
            missing
                .iter()
                .map(|c| format!("'{c}' (U+{:04X})", *c as u32))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(ResolvedText {
        font: ResolvedFont {
            family: face.family.clone(),
            style: face.style.clone(),
            file: face
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            index: face.index,
            hash: format!("fnv1a64:{}", fnv1a64_hex(data)),
            units_per_em: upem,
            cap_height: cap,
        },
        advance: pen,
        glyphs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_reference_values() {
        assert_eq!(fnv1a64_hex(b""), "cbf29ce484222325");
        assert_eq!(fnv1a64_hex(b"a"), "af63dc4c8601ec8c");
    }

    #[test]
    fn unknown_family_is_refused_not_substituted() {
        let e = find_font(&FontSpec::family("No Such Family 0xDEAD")).unwrap_err();
        assert!(matches!(e, SketchError::FontNotFound(_)), "{e}");
    }
}
