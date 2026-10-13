//! ASCII DXF group-code reader: tokenizer and section splitter.
//!
//! A DXF file is a flat sequence of (group code, value) line pairs. Codes used here (public
//! DXF reference): `0` structure/entity type, `2` section/table/block name, `9` header
//! variable name, `8` layer, `10/20/30`, `11/21/31` points, `38` elevation, `39` thickness,
//! `40/41/42/43` radius/width/bulge, `50/51` angles (degrees), `66` entities-follow flag,
//! `67` paper space, `70` flags, `90` vertex count, `210/220/230` extrusion direction.
//! No recursion: sections and entities are split in a single linear pass.

use crate::{ExchangeError, Result};
use std::collections::HashMap;

pub(crate) const FMT: &str = "DXF";

#[derive(Clone, Debug)]
pub(crate) struct Pair {
    pub code: i32,
    pub value: String,
    /// 1-based line of the group code.
    pub line: usize,
}

/// One entity of the ENTITIES section: its `0` pair's value (upper case) and the
/// following pairs up to the next `0`.
#[derive(Clone, Debug)]
pub(crate) struct RawEntity {
    pub kind: String,
    pub line: usize,
    pub pairs: Vec<Pair>,
}

pub(crate) fn malformed(line: usize, message: impl Into<String>) -> ExchangeError {
    ExchangeError::Malformed {
        format: FMT.into(),
        line: Some(line),
        message: message.into(),
    }
}

impl RawEntity {
    pub fn layer(&self) -> &str {
        self.pairs
            .iter()
            .find(|p| p.code == 8)
            .map_or("0", |p| p.value.trim())
    }

    /// First value of `code` as a finite f64.
    pub fn f(&self, code: i32) -> Result<Option<f64>> {
        match self.pairs.iter().find(|p| p.code == code) {
            None => Ok(None),
            Some(p) => parse_f64(p).map(Some),
        }
    }

    pub fn f_or(&self, code: i32, default: f64) -> Result<f64> {
        Ok(self.f(code)?.unwrap_or(default))
    }

    /// Required finite f64.
    pub fn f_req(&self, code: i32) -> Result<f64> {
        self.f(code)?.ok_or_else(|| {
            malformed(
                self.line,
                format!("{} is missing group code {code}", self.kind),
            )
        })
    }

    pub fn int(&self, code: i32) -> Result<Option<i64>> {
        match self.pairs.iter().find(|p| p.code == code) {
            None => Ok(None),
            Some(p) => parse_int(p).map(Some),
        }
    }
}

pub(crate) fn parse_f64(p: &Pair) -> Result<f64> {
    let t = p.value.trim();
    let v: f64 = t.parse().map_err(|_| {
        malformed(
            p.line,
            format!(
                "group {}: '{}' is not a number",
                p.code,
                t.chars().take(32).collect::<String>()
            ),
        )
    })?;
    if !v.is_finite() {
        return Err(ExchangeError::NonFinite {
            format: FMT.into(),
            line: Some(p.line),
            message: format!("group {} value '{t}'", p.code),
        });
    }
    Ok(v)
}

pub(crate) fn parse_int(p: &Pair) -> Result<i64> {
    let t = p.value.trim();
    t.parse::<i64>().map_err(|_| {
        malformed(
            p.line,
            format!(
                "group {}: '{}' is not an integer",
                p.code,
                t.chars().take(32).collect::<String>()
            ),
        )
    })
}

/// The parsed file structure.
#[derive(Debug, Default)]
pub(crate) struct DxfFile {
    /// Header variable name (e.g. `$INSUNITS`) -> its value pairs.
    pub header: HashMap<String, Vec<Pair>>,
    /// Layer names from TABLES/LAYER, in file order.
    pub table_layers: Vec<String>,
    pub entities: Vec<RawEntity>,
    pub has_entities_section: bool,
}

const BINARY_SENTINEL: &[u8] = b"AutoCAD Binary DXF";

/// Split `bytes` into group-code pairs, stopping at `0/EOF`.
pub(crate) fn tokenize(bytes: &[u8], max_pairs: u64) -> Result<Vec<Pair>> {
    if bytes.starts_with(BINARY_SENTINEL) {
        return Err(ExchangeError::Malformed {
            format: FMT.into(),
            line: None,
            message: "binary DXF is not supported (save as ASCII DXF)".into(),
        });
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let mut pairs = Vec::new();
    let mut lines = bytes.split(|&b| b == b'\n').enumerate();
    while let Some((i, code_line)) = lines.next() {
        let code_txt = String::from_utf8_lossy(code_line);
        let code_txt = code_txt.trim();
        if code_txt.is_empty() {
            // Trailing blank lines after the last pair.
            if lines
                .clone()
                .all(|(_, l)| l.iter().all(u8::is_ascii_whitespace))
            {
                break;
            }
            return Err(malformed(i + 1, "empty group code line"));
        }
        let code: i32 = code_txt.parse().map_err(|_| {
            malformed(
                i + 1,
                format!(
                    "'{}' is not a group code (is this an ASCII DXF file?)",
                    code_txt.chars().take(32).collect::<String>()
                ),
            )
        })?;
        let Some((_, value_line)) = lines.next() else {
            return Err(ExchangeError::Truncated {
                format: FMT.into(),
                message: format!("group code {code} at line {} has no value", i + 1),
            });
        };
        let value = String::from_utf8_lossy(value_line)
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        if pairs.len() as u64 >= max_pairs {
            return Err(ExchangeError::LimitExceeded {
                format: FMT.into(),
                what: "group-code pairs".into(),
                limit: max_pairs,
                actual: max_pairs + 1,
            });
        }
        let is_eof = code == 0 && value.trim() == "EOF";
        pairs.push(Pair {
            code,
            value,
            line: i + 1,
        });
        if is_eof {
            break;
        }
    }
    Ok(pairs)
}

/// Split pairs into sections. `max_entities` bounds the ENTITIES section.
pub(crate) fn parse(pairs: &[Pair], max_entities: u64) -> Result<DxfFile> {
    let mut f = DxfFile::default();
    let mut i = 0;
    let n = pairs.len();
    let mut saw_eof = false;
    while i < n {
        let p = &pairs[i];
        if p.code == 0 && p.value.trim() == "EOF" {
            saw_eof = true;
            break;
        }
        if p.code == 999 {
            i += 1;
            continue;
        }
        if !(p.code == 0 && p.value.trim() == "SECTION") {
            return Err(malformed(
                p.line,
                format!(
                    "expected '0/SECTION', found group {} '{}'",
                    p.code,
                    p.value.trim().chars().take(32).collect::<String>()
                ),
            ));
        }
        let name = match pairs.get(i + 1) {
            Some(q) if q.code == 2 => q.value.trim().to_ascii_uppercase(),
            _ => return Err(malformed(p.line, "SECTION without a '2' name")),
        };
        let start = i + 2;
        let end = (start..n)
            .find(|&k| pairs[k].code == 0 && pairs[k].value.trim() == "ENDSEC")
            .ok_or_else(|| ExchangeError::Truncated {
                format: FMT.into(),
                message: format!("section {name} (line {}) has no ENDSEC", p.line),
            })?;
        let body = &pairs[start..end];
        match name.as_str() {
            "HEADER" => parse_header(body, &mut f),
            "TABLES" => parse_tables(body, &mut f),
            "ENTITIES" => {
                f.has_entities_section = true;
                parse_entities(body, max_entities, &mut f)?;
            }
            // BLOCKS definitions are never imported (INSERT is reported unsupported);
            // CLASSES, OBJECTS, THUMBNAILIMAGE, ACDSDATA are irrelevant to 2D geometry.
            _ => {}
        }
        i = end + 1;
    }
    if !saw_eof && f.entities.is_empty() && !f.has_entities_section {
        return Err(ExchangeError::Truncated {
            format: FMT.into(),
            message: "no ENTITIES section and no EOF marker".into(),
        });
    }
    Ok(f)
}

fn parse_header(body: &[Pair], f: &mut DxfFile) {
    let mut cur: Option<String> = None;
    for p in body {
        if p.code == 9 {
            let name = p.value.trim().to_ascii_uppercase();
            f.header.entry(name.clone()).or_default();
            cur = Some(name);
        } else if let Some(c) = &cur {
            f.header.entry(c.clone()).or_default().push(p.clone());
        }
    }
}

fn parse_tables(body: &[Pair], f: &mut DxfFile) {
    // Layer records: '0/LAYER' followed (before the next '0') by '2/<name>'. The table
    // header itself is '0/TABLE' '2/LAYER', so it is not mistaken for a record.
    let mut i = 0;
    while i < body.len() {
        if body[i].code == 0 && body[i].value.trim().eq_ignore_ascii_case("LAYER") {
            let mut k = i + 1;
            while k < body.len() && body[k].code != 0 {
                if body[k].code == 2 {
                    f.table_layers.push(body[k].value.trim().to_owned());
                    break;
                }
                k += 1;
            }
        }
        i += 1;
    }
}

fn parse_entities(body: &[Pair], max_entities: u64, f: &mut DxfFile) -> Result<()> {
    let mut cur: Option<RawEntity> = None;
    for p in body {
        if p.code == 0 {
            if let Some(e) = cur.take() {
                f.entities.push(e);
            }
            if f.entities.len() as u64 >= max_entities {
                return Err(ExchangeError::LimitExceeded {
                    format: FMT.into(),
                    what: "entities".into(),
                    limit: max_entities,
                    actual: max_entities + 1,
                });
            }
            cur = Some(RawEntity {
                kind: p.value.trim().to_ascii_uppercase(),
                line: p.line,
                pairs: Vec::new(),
            });
        } else if p.code == 999 {
            continue;
        } else if let Some(e) = cur.as_mut() {
            e.pairs.push(p.clone());
        } else {
            return Err(malformed(
                p.line,
                "ENTITIES section data before the first entity",
            ));
        }
    }
    if let Some(e) = cur.take() {
        f.entities.push(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_errors_have_lines() {
        let e = tokenize(b"0\nSECTION\nxx\nHEADER\n", 100).unwrap_err();
        assert!(
            matches!(e, ExchangeError::Malformed { line: Some(3), .. }),
            "{e:?}"
        );
        let e = tokenize(b"0\nSECTION\n2", 100).unwrap_err();
        assert!(matches!(e, ExchangeError::Truncated { .. }), "{e:?}");
        let e = tokenize(b"AutoCAD Binary DXF\r\n\x1a\x00", 100).unwrap_err();
        assert!(matches!(e, ExchangeError::Malformed { .. }));
        let e = tokenize(b"0\nA\n0\nB\n0\nC\n", 2).unwrap_err();
        assert!(matches!(e, ExchangeError::LimitExceeded { .. }));
    }

    #[test]
    fn section_without_endsec() {
        let pairs = tokenize(b"0\r\nSECTION\r\n2\r\nENTITIES\r\n0\r\nLINE\r\n", 100).unwrap();
        assert!(matches!(
            parse(&pairs, 10),
            Err(ExchangeError::Truncated { .. })
        ));
    }

    #[test]
    fn entity_limit() {
        let mut s = String::from("0\nSECTION\n2\nENTITIES\n");
        for _ in 0..5 {
            s.push_str("0\nPOINT\n8\n0\n");
        }
        s.push_str("0\nENDSEC\n0\nEOF\n");
        let pairs = tokenize(s.as_bytes(), 1000).unwrap();
        assert_eq!(parse(&pairs, 5).unwrap().entities.len(), 5);
        assert!(matches!(
            parse(&pairs, 4),
            Err(ExchangeError::LimitExceeded { .. })
        ));
    }
}
