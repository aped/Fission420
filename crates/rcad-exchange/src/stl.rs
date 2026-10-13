//! STL (stereolithography) triangle meshes: reader (ASCII and binary) and writers.
//!
//! # Encoding detection
//! The leading `solid` token is not trusted (many binary writers start their 80-byte header
//! with it). A file is **binary** when it is at least 84 bytes long and
//! `84 + 50 * n == len`, `n` being the little-endian `u32` triangle count at byte 80.
//! Otherwise it is parsed as **ASCII** when it starts (after whitespace) with `solid` and is
//! valid text; anything else is reported as a binary file whose length disagrees with its
//! header ([`ExchangeError::Truncated`]).
//!
//! # Vertex welding
//! STL stores three independent corners per triangle. [`read`] welds corners whose
//! coordinates are **exactly equal** after parsing (bitwise equal `f64`, with `-0.0` treated
//! as `0.0`). No tolerance is applied: near-coincident corners stay separate and show up as
//! boundary edges in [`crate::mesh::diagnose`], which is the honest answer for a mesh that
//! does not close exactly.
//!
//! # Units
//! STL has no unit. The reader returns the file's numbers unchanged; the caller chooses a
//! unit and calls [`StlMesh::scale`] with its millimetre factor (ADR 0004).

use crate::{ExchangeError, Result, check_scale};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const FMT: &str = "STL";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum StlEncoding {
    Ascii,
    Binary,
}

/// Reader limits. Exceeding one is an [`ExchangeError::LimitExceeded`], checked before any
/// large allocation.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct StlLimits {
    pub max_bytes: u64,
    pub max_triangles: u64,
}

impl Default for StlLimits {
    /// 512 MiB, 10 million triangles (a 10 M triangle binary STL is ~477 MiB).
    fn default() -> Self {
        Self {
            max_bytes: 512 * 1024 * 1024,
            max_triangles: 10_000_000,
        }
    }
}

/// An indexed triangle mesh read from (or to be written to) STL.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StlMesh {
    /// ASCII: the name after `solid`. Binary: the 80-byte header as text when it is
    /// printable ASCII (trimmed), else `None`.
    pub name: Option<String>,
    pub encoding: StlEncoding,
    /// Welded vertex positions (exact equality, see the module docs). File units until
    /// [`StlMesh::scale`] is applied.
    pub positions: Vec<[f64; 3]>,
    /// Vertex indices per triangle, in file order and file winding.
    pub triangles: Vec<[u32; 3]>,
    /// One normal per triangle as written in the file (often unit, sometimes zero).
    /// A non-finite normal in the file is stored as `[0, 0, 0]` ("unknown").
    pub facet_normals: Vec<[f64; 3]>,
}

impl StlMesh {
    /// Multiply all positions by `to_mm` (the millimetres per file unit). Normals are
    /// unchanged (uniform positive scale). Errors when `to_mm` is not finite and > 0.
    pub fn scale(&mut self, to_mm: f64) -> Result<()> {
        check_scale("STL unit scale", to_mm)?;
        for p in &mut self.positions {
            for c in p.iter_mut() {
                *c *= to_mm;
            }
        }
        Ok(())
    }

    /// The three corner positions of triangle `i`; `None` for an out-of-range index.
    pub fn triangle(&self, i: usize) -> Option<[[f64; 3]; 3]> {
        let t = self.triangles.get(i)?;
        Some([
            *self.positions.get(t[0] as usize)?,
            *self.positions.get(t[1] as usize)?,
            *self.positions.get(t[2] as usize)?,
        ])
    }
}

/// Read an ASCII or binary STL. See the module docs for detection and welding.
pub fn read(bytes: &[u8], limits: &StlLimits) -> Result<StlMesh> {
    if bytes.len() as u64 > limits.max_bytes {
        return Err(ExchangeError::LimitExceeded {
            format: FMT.into(),
            what: "bytes".into(),
            limit: limits.max_bytes,
            actual: bytes.len() as u64,
        });
    }
    if bytes.len() >= 84 {
        let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as u64;
        if 84 + 50 * n == bytes.len() as u64 {
            return read_binary(bytes, n, limits);
        }
    }
    let text_start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let looks_ascii = bytes[text_start..].len() >= 5
        && bytes[text_start..text_start + 5].eq_ignore_ascii_case(b"solid");
    if looks_ascii && let Ok(text) = std::str::from_utf8(bytes) {
        return read_ascii(text, limits);
    }
    if bytes.len() < 84 {
        return Err(ExchangeError::Truncated {
            format: FMT.into(),
            message: format!(
                "{} bytes: shorter than a binary STL header (84 bytes) and not ASCII STL",
                bytes.len()
            ),
        });
    }
    let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as u64;
    Err(ExchangeError::Truncated {
        format: FMT.into(),
        message: format!(
            "binary header declares {n} triangles ({} bytes expected) but the file has {} bytes",
            84 + 50 * n,
            bytes.len()
        ),
    })
}

/// Welds exactly equal positions.
struct Welder {
    map: HashMap<[u64; 3], u32>,
    positions: Vec<[f64; 3]>,
}

impl Welder {
    fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::with_capacity(capacity),
            positions: Vec::with_capacity(capacity),
        }
    }

    fn index(&mut self, p: [f64; 3]) -> Result<u32> {
        // -0.0 == 0.0: normalise before hashing the bits.
        let p = p.map(|c| if c == 0.0 { 0.0 } else { c });
        let key = p.map(f64::to_bits);
        if let Some(&i) = self.map.get(&key) {
            return Ok(i);
        }
        let i = u32::try_from(self.positions.len()).map_err(|_| ExchangeError::LimitExceeded {
            format: FMT.into(),
            what: "vertices".into(),
            limit: u32::MAX as u64,
            actual: self.positions.len() as u64 + 1,
        })?;
        self.positions.push(p);
        self.map.insert(key, i);
        Ok(i)
    }
}

fn read_binary(bytes: &[u8], n: u64, limits: &StlLimits) -> Result<StlMesh> {
    if n > limits.max_triangles {
        return Err(ExchangeError::LimitExceeded {
            format: FMT.into(),
            what: "triangles".into(),
            limit: limits.max_triangles,
            actual: n,
        });
    }
    // n <= max_triangles and 84 + 50 n == len <= max_bytes, so these fit in usize.
    let n = n as usize;
    let header = &bytes[..80];
    let name = {
        let end = header
            .iter()
            .rposition(|&b| b != 0 && b != b' ')
            .map_or(0, |i| i + 1);
        let h = &header[..end];
        (!h.is_empty() && h.iter().all(|b| (0x20..0x7f).contains(b)))
            .then(|| String::from_utf8_lossy(h).trim().to_owned())
    };
    let mut welder = Welder::new(n / 2 + 3);
    let mut triangles = Vec::with_capacity(n);
    let mut facet_normals = Vec::with_capacity(n);
    for i in 0..n {
        let rec = &bytes[84 + 50 * i..84 + 50 * (i + 1)];
        let f = |k: usize| {
            let o = 4 * k;
            f32::from_le_bytes([rec[o], rec[o + 1], rec[o + 2], rec[o + 3]]) as f64
        };
        let normal = [f(0), f(1), f(2)];
        let mut tri = [0u32; 3];
        for (c, slot) in tri.iter_mut().enumerate() {
            let p = [f(3 + 3 * c), f(4 + 3 * c), f(5 + 3 * c)];
            if p.iter().any(|v| !v.is_finite()) {
                return Err(ExchangeError::NonFinite {
                    format: FMT.into(),
                    line: None,
                    message: format!("triangle {i} corner {c} is {p:?}"),
                });
            }
            *slot = welder.index(p)?;
        }
        triangles.push(tri);
        facet_normals.push(finite_or_zero(normal));
    }
    Ok(StlMesh {
        name,
        encoding: StlEncoding::Binary,
        positions: welder.positions,
        triangles,
        facet_normals,
    })
}

fn finite_or_zero(n: [f64; 3]) -> [f64; 3] {
    if n.iter().all(|v| v.is_finite()) {
        n
    } else {
        [0.0; 3]
    }
}

fn malformed(line: usize, message: impl Into<String>) -> ExchangeError {
    ExchangeError::Malformed {
        format: FMT.into(),
        line: Some(line),
        message: message.into(),
    }
}

/// Three numbers after a keyword on line `line`.
fn parse_xyz<'a>(
    mut it: impl Iterator<Item = &'a str>,
    line: usize,
    what: &str,
) -> Result<[f64; 3]> {
    let mut v = [0.0; 3];
    for c in &mut v {
        let tok = it
            .next()
            .ok_or_else(|| malformed(line, format!("{what}: expected 3 numbers")))?;
        let x: f64 = tok
            .parse()
            .map_err(|_| malformed(line, format!("{what}: '{tok}' is not a number")))?;
        *c = x;
    }
    if let Some(extra) = it.next() {
        return Err(malformed(
            line,
            format!("{what}: unexpected extra token '{extra}'"),
        ));
    }
    Ok(v)
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    /// Before `solid` / after `endsolid`.
    Outside,
    /// Inside a solid, expecting `facet` or `endsolid`.
    Solid,
    /// After `facet normal`, expecting `outer loop`.
    Facet,
    /// Inside `outer loop`; number of vertices read.
    Loop(usize),
    /// After `endloop`, expecting `endfacet`.
    EndLoop,
}

fn read_ascii(text: &str, limits: &StlLimits) -> Result<StlMesh> {
    let mut name: Option<String> = None;
    let mut welder = Welder::new(1024);
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let mut facet_normals: Vec<[f64; 3]> = Vec::new();
    let mut state = State::Outside;
    let mut normal = [0.0; 3];
    let mut corners = [[0.0; 3]; 3];
    let mut solids = 0usize;
    let mut last_line = 0usize;
    for (idx, raw) in text.lines().enumerate() {
        let line = idx + 1;
        last_line = line;
        let mut toks = raw.split_whitespace();
        let Some(kw) = toks.next() else { continue };
        let kw = kw.to_ascii_lowercase();
        match (state, kw.as_str()) {
            (State::Outside, "solid") => {
                if solids == 0 {
                    let rest = raw.trim_start();
                    let n = rest[5.min(rest.len())..].trim();
                    name = (!n.is_empty()).then(|| n.to_owned());
                }
                solids += 1;
                state = State::Solid;
            }
            (State::Solid, "facet") => {
                match toks.next() {
                    Some(t) if t.eq_ignore_ascii_case("normal") => {}
                    _ => return Err(malformed(line, "expected 'facet normal nx ny nz'")),
                }
                normal = finite_or_zero(parse_xyz(toks, line, "facet normal")?);
                state = State::Facet;
            }
            (State::Solid, "endsolid") => state = State::Outside,
            (State::Facet, "outer") => {
                match toks.next() {
                    Some(t) if t.eq_ignore_ascii_case("loop") => {}
                    _ => return Err(malformed(line, "expected 'outer loop'")),
                }
                state = State::Loop(0);
            }
            (State::Loop(k), "vertex") => {
                if k >= 3 {
                    return Err(malformed(
                        line,
                        "more than 3 vertices in a facet (only triangles are valid STL)",
                    ));
                }
                let p = parse_xyz(toks, line, "vertex")?;
                if p.iter().any(|v| !v.is_finite()) {
                    return Err(ExchangeError::NonFinite {
                        format: FMT.into(),
                        line: Some(line),
                        message: format!("vertex {p:?}"),
                    });
                }
                corners[k] = p;
                state = State::Loop(k + 1);
            }
            (State::Loop(k), "endloop") => {
                if k != 3 {
                    return Err(malformed(
                        line,
                        format!("facet has {k} vertices, expected 3"),
                    ));
                }
                state = State::EndLoop;
            }
            (State::EndLoop, "endfacet") => {
                if triangles.len() as u64 >= limits.max_triangles {
                    return Err(ExchangeError::LimitExceeded {
                        format: FMT.into(),
                        what: "triangles".into(),
                        limit: limits.max_triangles,
                        actual: triangles.len() as u64 + 1,
                    });
                }
                let mut tri = [0u32; 3];
                for (slot, p) in tri.iter_mut().zip(corners) {
                    *slot = welder.index(p)?;
                }
                triangles.push(tri);
                facet_normals.push(normal);
                state = State::Solid;
            }
            (s, other) => {
                let expected = match s {
                    State::Outside => "'solid'",
                    State::Solid => "'facet' or 'endsolid'",
                    State::Facet => "'outer loop'",
                    State::Loop(_) => "'vertex' or 'endloop'",
                    State::EndLoop => "'endfacet'",
                };
                let shown: String = other.chars().take(32).collect();
                return Err(malformed(
                    line,
                    format!("unexpected '{shown}', expected {expected}"),
                ));
            }
        }
    }
    if state != State::Outside {
        return Err(ExchangeError::Truncated {
            format: FMT.into(),
            message: format!(
                "ASCII STL ends inside a solid (line {last_line}; missing endsolid/endfacet)"
            ),
        });
    }
    Ok(StlMesh {
        name,
        encoding: StlEncoding::Ascii,
        positions: welder.positions,
        triangles,
        facet_normals,
    })
}

/// Normal for writing: the stored facet normal if present and non-zero, else the unit
/// right-hand-rule normal of the corners (zero for a degenerate triangle).
fn write_normal(mesh: &StlMesh, i: usize, c: &[[f64; 3]; 3]) -> [f64; 3] {
    if let Some(n) = mesh.facet_normals.get(i)
        && n.iter().any(|v| *v != 0.0)
        && n.iter().all(|v| v.is_finite())
    {
        return *n;
    }
    let u = sub(c[1], c[0]);
    let v = sub(c[2], c[0]);
    let n = cross(u, v);
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len > 0.0 && len.is_finite() {
        n.map(|x| x / len)
    } else {
        [0.0; 3]
    }
}

pub(crate) fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// All triangles with resolved corners; errors on an out-of-range index.
fn corners(mesh: &StlMesh) -> Result<Vec<[[f64; 3]; 3]>> {
    (0..mesh.triangles.len())
        .map(|i| {
            mesh.triangle(i).ok_or_else(|| {
                ExchangeError::InvalidOption(format!(
                    "triangle {i} references a vertex outside the {} positions",
                    mesh.positions.len()
                ))
            })
        })
        .collect()
}

/// Binary STL. The header is `"binary STL from R-CAD: <name>"` (never starting with
/// `solid`), coordinates are rounded to `f32` (the format's precision), attribute bytes 0.
/// Errors on out-of-range indices, non-finite coordinates or more than `u32::MAX` triangles.
pub fn write_binary(mesh: &StlMesh) -> Result<Vec<u8>> {
    let tris = corners(mesh)?;
    let n = u32::try_from(tris.len())
        .map_err(|_| ExchangeError::InvalidOption("more than u32::MAX triangles".into()))?;
    let mut out = Vec::with_capacity(84 + 50 * tris.len());
    let mut header = format!(
        "binary STL from R-CAD: {}",
        mesh.name.as_deref().unwrap_or("mesh")
    )
    .into_bytes();
    header.retain(|b| (0x20..0x7f).contains(b));
    header.resize(80, b' ');
    out.extend_from_slice(&header);
    out.extend_from_slice(&n.to_le_bytes());
    for (i, c) in tris.iter().enumerate() {
        check_finite(c, i)?;
        let nrm = write_normal(mesh, i, c);
        for v in nrm.iter().chain(c.iter().flatten()) {
            out.extend_from_slice(&(*v as f32).to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    Ok(out)
}

fn check_finite(c: &[[f64; 3]; 3], i: usize) -> Result<()> {
    if c.iter().flatten().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(ExchangeError::NonFinite {
            format: FMT.into(),
            line: None,
            message: format!("triangle {i} has a non-finite corner"),
        })
    }
}

/// ASCII STL with full `f64` precision (shortest round-trip decimal form). The solid name is
/// `mesh.name` with whitespace replaced by `_` (or `mesh`).
pub fn write_ascii(mesh: &StlMesh) -> Result<String> {
    use std::fmt::Write as _;
    let tris = corners(mesh)?;
    let name: String = mesh
        .name
        .as_deref()
        .unwrap_or("mesh")
        .chars()
        .map(|c| {
            if c.is_whitespace() || !c.is_ascii() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut s = String::with_capacity(64 + 260 * tris.len());
    let _ = writeln!(s, "solid {name}");
    for (i, c) in tris.iter().enumerate() {
        check_finite(c, i)?;
        let n = write_normal(mesh, i, c);
        let _ = writeln!(s, "  facet normal {:e} {:e} {:e}", n[0], n[1], n[2]);
        s.push_str("    outer loop\n");
        for p in c {
            let _ = writeln!(s, "      vertex {:e} {:e} {:e}", p[0], p[1], p[2]);
        }
        s.push_str("    endloop\n  endfacet\n");
    }
    let _ = writeln!(s, "endsolid {name}");
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tet() -> StlMesh {
        StlMesh {
            name: Some("t".into()),
            encoding: StlEncoding::Binary,
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            triangles: vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
            facet_normals: vec![],
        }
    }

    #[test]
    fn writers_round_trip() {
        let m = tet();
        let b = read(&write_binary(&m).unwrap(), &StlLimits::default()).unwrap();
        assert_eq!(b.encoding, StlEncoding::Binary);
        // Welding numbers vertices in first-use order; compare resolved corners.
        let same = |x: &StlMesh| {
            x.triangles.len() == m.triangles.len()
                && (0..m.triangles.len()).all(|i| x.triangle(i) == m.triangle(i))
        };
        assert!(same(&b));
        assert_eq!(b.positions.len(), 4);
        let a = read(write_ascii(&m).unwrap().as_bytes(), &StlLimits::default()).unwrap();
        assert_eq!(a.encoding, StlEncoding::Ascii);
        assert!(same(&a));
        assert_eq!(a.name.as_deref(), Some("t"));
        // Computed unit normal of the slanted face.
        let k = 1.0 / 3f64.sqrt();
        for (x, y) in a.facet_normals[3].iter().zip([k, k, k]) {
            assert!((x - y).abs() < 1e-15);
        }
    }

    #[test]
    fn bad_index_and_scale_are_errors() {
        let mut m = tet();
        assert!(m.scale(0.0).is_err());
        assert!(m.scale(f64::NAN).is_err());
        m.scale(25.4).unwrap();
        assert_eq!(m.positions[1], [25.4, 0.0, 0.0]);
        m.triangles.push([0, 1, 9]);
        assert!(write_binary(&m).is_err());
        assert!(write_ascii(&m).is_err());
    }

    #[test]
    fn negative_zero_welds_with_zero() {
        let s = "solid z\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\n\
                 endloop\nendfacet\nfacet normal 0 0 1\nouter loop\nvertex -0 -0 0\nvertex 0 1 0\n\
                 vertex 1 0 0\nendloop\nendfacet\nendsolid z\n";
        let m = read(s.as_bytes(), &StlLimits::default()).unwrap();
        assert_eq!(m.positions.len(), 3);
        assert_eq!(m.triangles, vec![[0, 1, 2], [0, 2, 1]]);
    }

    #[test]
    fn tiny_and_garbage_inputs_are_errors() {
        let l = StlLimits::default();
        assert!(matches!(
            read(b"", &l),
            Err(ExchangeError::Truncated { .. })
        ));
        // "solid" alone is an unterminated ASCII STL.
        assert!(matches!(
            read(b"solid", &l),
            Err(ExchangeError::Truncated { .. })
        ));
        assert!(read(&[0xff; 83], &l).is_err());
        assert!(read(&[0x00; 200], &l).is_err());
        // Binary count huge vs length: mismatch error, no allocation.
        let mut b = vec![0u8; 84];
        b[80..84].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(read(&b, &l), Err(ExchangeError::Truncated { .. })));
        // ASCII with >3 vertices.
        let s = "solid q\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\n\
                 vertex 1 1 0\nendloop\nendfacet\nendsolid\n";
        assert!(matches!(
            read(s.as_bytes(), &l),
            Err(ExchangeError::Malformed { line: Some(7), .. })
        ));
    }
}
