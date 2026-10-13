//! 3MF (3D Manufacturing Format) core-specification subset: multi-object
//! triangle-mesh import and export with explicit units.
//!
//! Clean-room implementation from the public 3MF Core Specification (v1.3,
//! 3MF Consortium) and ECMA-376 Part 2 (Open Packaging Conventions), using
//! [`crate::zip`] for the container and `quick-xml` for XML tokenization.
//!
//! **Read** ([`read`]): the start part named by `_rels/.rels`; `<model unit>`
//! (micron, millimeter, centimeter, inch, foot, meter; default millimeter);
//! `<object>` meshes (`<vertices>`, `<triangles>`) and `<components>`
//! (flattened with their transforms); every `<build><item>` becomes one
//! [`ImportedObject`] in millimetres with the item and component transforms
//! applied (a mirroring transform reverses the triangle winding so the mesh
//! stays outward). Colour: the object-level `pid`/`pindex` into a core
//! `<basematerials>` `displaycolor`; per-triangle properties, colour groups,
//! textures and other material-extension content are **not** imported and are
//! counted in `warnings`. Required extensions other than the materials
//! extension (e.g. production: multi-file models; beam lattice; slices) are
//! refused. Limits ([`ThreeMfLimits`]) bound the archive, the decompressed
//! model, vertices, triangles, build items, component depth and the
//! flattened output.
//!
//! **Write** ([`write`]): one `<object type="model">` per [`MeshObject`]
//! (exactly-equal vertices welded), one build item each with no transform,
//! coordinates converted from mm to the chosen unit, optional object colours
//! as one `<basematerials>` group. Deflate-compressed package with
//! `[Content_Types].xml`, `_rels/.rels` and `3D/3dmodel.model`.
use crate::mesh::{self, MeshDiagnostics};
use crate::zip::{self, ZipLimits, ZipReader, ZipWriter};
use crate::{ExchangeError, Result};
use quick_xml::NsReader;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write as _;

const FORMAT: &str = "3MF";
pub const CORE_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
pub const MATERIAL_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/material/2015/02";
const MODEL_REL_TYPE: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
const MODEL_PART: &str = "3D/3dmodel.model";

/// `<model unit>` values of the core specification.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ThreeMfUnit {
    Micron,
    Millimeter,
    Centimeter,
    Inch,
    Foot,
    Meter,
}

impl ThreeMfUnit {
    pub fn to_mm(self) -> f64 {
        match self {
            Self::Micron => 0.001,
            Self::Millimeter => 1.0,
            Self::Centimeter => 10.0,
            Self::Inch => 25.4,
            Self::Foot => 304.8,
            Self::Meter => 1000.0,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Micron => "micron",
            Self::Millimeter => "millimeter",
            Self::Centimeter => "centimeter",
            Self::Inch => "inch",
            Self::Foot => "foot",
            Self::Meter => "meter",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [
            Self::Micron,
            Self::Millimeter,
            Self::Centimeter,
            Self::Inch,
            Self::Foot,
            Self::Meter,
        ]
        .into_iter()
        .find(|u| u.name() == s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreeMfLimits {
    pub max_archive_bytes: usize,
    pub max_archive_entries: usize,
    /// Decompressed size of the model part (and of `_rels/.rels`).
    pub max_model_bytes: usize,
    /// Vertices / triangles summed over all mesh objects in the file.
    pub max_vertices: usize,
    pub max_triangles: usize,
    pub max_build_items: usize,
    pub max_component_depth: usize,
    /// Triangles after flattening items and components (instancing multiplies).
    pub max_output_triangles: usize,
}

impl Default for ThreeMfLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: 256 << 20,
            max_archive_entries: 10_000,
            max_model_bytes: 1 << 30,
            max_vertices: 10_000_000,
            max_triangles: 10_000_000,
            max_build_items: 10_000,
            max_component_depth: 16,
            max_output_triangles: 20_000_000,
        }
    }
}

/// A mesh to export. Positions in mm.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshObject {
    pub name: Option<String>,
    pub positions: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
    /// sRGB + alpha, written as the object's base material `displaycolor`.
    pub color: Option<[u8; 4]>,
}

/// One build item, flattened and converted to mm.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportedObject {
    /// The item's object `name`, if any.
    pub name: Option<String>,
    /// The `<object id>` the build item references.
    pub object_id: u32,
    /// `<object type>`: model, solidsupport, support, surface or other.
    pub object_type: String,
    /// mm, item/component transforms applied.
    pub positions: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
    /// Object-level base material colour (sRGB + alpha).
    pub color: Option<[u8; 4]>,
    pub diagnostics: MeshDiagnostics,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThreeMfImport {
    pub unit: ThreeMfUnit,
    pub objects: Vec<ImportedObject>,
    /// `<metadata name>` and text (each truncated to 1000 characters).
    pub metadata: Vec<(String, String)>,
    /// Everything read but not imported (colours, extensions, ...).
    pub warnings: Vec<String>,
}

fn malformed(msg: impl Into<String>) -> ExchangeError {
    ExchangeError::Malformed {
        format: FORMAT.into(),
        line: None,
        message: msg.into(),
    }
}

fn limit(what: &str, limit: usize, actual: usize) -> ExchangeError {
    ExchangeError::LimitExceeded {
        format: FORMAT.into(),
        what: what.into(),
        limit: limit as u64,
        actual: actual as u64,
    }
}

// ------------------------------------------------------------------ writing

fn xml_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => o.push(c),
        }
    }
    o
}

/// Weld exactly equal positions (first-use order), remapping triangles.
fn weld(positions: &[[f64; 3]], triangles: &[[u32; 3]]) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
    let key = |p: &[f64; 3]| p.map(|v| if v == 0.0 { 0u64 } else { v.to_bits() });
    let mut map: HashMap<[u64; 3], u32> = HashMap::new();
    let mut out = Vec::new();
    let mut remap = vec![u32::MAX; positions.len()];
    let tris = triangles
        .iter()
        .map(|t| {
            t.map(|i| {
                let i = i as usize;
                if remap[i] == u32::MAX {
                    let p = positions[i];
                    remap[i] = *map.entry(key(&p)).or_insert_with(|| {
                        out.push(p);
                        (out.len() - 1) as u32
                    });
                }
                remap[i]
            })
        })
        .collect();
    (out, tris)
}

/// A 3MF package with one object (and build item) per mesh. Positions are mm
/// and are written in `unit`.
pub fn write(objects: &[MeshObject], unit: ThreeMfUnit) -> Result<Vec<u8>> {
    // Coordinates are written to a fixed 1e-9 mm resolution in the file's unit
    // (9 decimals in mm, 11 in inch, 12 in m, 6 in micron): far below printing
    // resolution and fine enough that unit round trips stay exact to ~1e-9 mm,
    // while floating-point noise of recomputed geometry (~1e-14 mm) changes the
    // text only for a coordinate within that noise of a rounding boundary
    // (odds ~1e-5 per coordinate that differs at all between regenerations).
    let decimals = (9.0 + unit.to_mm().log10()).ceil().clamp(0.0, 12.0) as usize;
    if objects.is_empty() {
        return Err(ExchangeError::InvalidOption(
            "3MF export needs at least one mesh".into(),
        ));
    }
    let scale = 1.0 / unit.to_mm();
    let colored: Vec<(usize, [u8; 4])> = objects
        .iter()
        .enumerate()
        .filter_map(|(i, o)| o.color.map(|c| (i, c)))
        .collect();
    let mat_id = objects.len() + 1;
    let mut x = String::new();
    let _ = writeln!(x, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        x,
        r#"<model unit="{}" xml:lang="en-US" xmlns="{CORE_NS}">"#,
        unit.name()
    );
    let _ = writeln!(x, r#" <metadata name="Application">R-CAD</metadata>"#);
    let _ = writeln!(x, " <resources>");
    if !colored.is_empty() {
        let _ = writeln!(x, r#"  <basematerials id="{mat_id}">"#);
        for (i, c) in &colored {
            let name = objects[*i]
                .name
                .clone()
                .unwrap_or_else(|| format!("Object {}", i + 1));
            let _ = writeln!(
                x,
                r##"   <base name="{}" displaycolor="#{:02X}{:02X}{:02X}{:02X}"/>"##,
                xml_escape(&name),
                c[0],
                c[1],
                c[2],
                c[3]
            );
        }
        let _ = writeln!(x, "  </basematerials>");
    }
    for (i, o) in objects.iter().enumerate() {
        if o.triangles.is_empty() {
            return Err(ExchangeError::InvalidOption(format!(
                "3MF export: object {} has no triangles",
                i + 1
            )));
        }
        if let Some(t) = o
            .triangles
            .iter()
            .find(|t| t.iter().any(|&v| v as usize >= o.positions.len()))
        {
            return Err(ExchangeError::InvalidOption(format!(
                "3MF export: object {} triangle {t:?} references a missing vertex",
                i + 1
            )));
        }
        if o.positions.iter().flatten().any(|v| !v.is_finite()) {
            return Err(ExchangeError::InvalidOption(format!(
                "3MF export: object {} has a non-finite coordinate",
                i + 1
            )));
        }
        let (pos, tris) = weld(&o.positions, &o.triangles);
        let _ = write!(x, r#"  <object id="{}" type="model""#, i + 1);
        if let Some(n) = &o.name {
            let _ = write!(x, r#" name="{}""#, xml_escape(n));
        }
        if let Some(k) = colored.iter().position(|(j, _)| *j == i) {
            let _ = write!(x, r#" pid="{mat_id}" pindex="{k}""#);
        }
        let _ = writeln!(x, ">\n   <mesh>\n    <vertices>");
        for p in &pos {
            let _ = writeln!(
                x,
                r#"     <vertex x="{}" y="{}" z="{}"/>"#,
                fixed(p[0] * scale, decimals),
                fixed(p[1] * scale, decimals),
                fixed(p[2] * scale, decimals)
            );
        }
        let _ = writeln!(x, "    </vertices>\n    <triangles>");
        for t in &tris {
            let _ = writeln!(
                x,
                r#"     <triangle v1="{}" v2="{}" v3="{}"/>"#,
                t[0], t[1], t[2]
            );
        }
        let _ = writeln!(x, "    </triangles>\n   </mesh>\n  </object>");
    }
    let _ = writeln!(x, " </resources>\n <build>");
    for i in 0..objects.len() {
        let _ = writeln!(x, r#"  <item objectid="{}"/>"#, i + 1);
    }
    let _ = writeln!(x, " </build>\n</model>");

    let content_types = concat!(
        r#"<?xml version="1.0" encoding="UTF-8"?>"#,
        "\n",
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#,
        r#"<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#,
        r#"<Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>"#,
        "</Types>\n"
    );
    let rels = format!(
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8"?>"#,
            "\n",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
            r#"<Relationship Target="/{}" Id="rel0" Type="{}"/>"#,
            "</Relationships>\n"
        ),
        MODEL_PART, MODEL_REL_TYPE
    );
    let mut z = ZipWriter::default();
    z.add("[Content_Types].xml", content_types.as_bytes())?;
    z.add("_rels/.rels", rels.as_bytes())?;
    z.add(MODEL_PART, x.as_bytes())?;
    z.finish()
}

// ------------------------------------------------------------------ reading

/// Row-vector affine transform: `p' = [x y z 1] * M` with `m[0..3]` the
/// linear rows and `m[3]` the translation (3MF core spec, `ST_Matrix3D`).
type Matrix = [[f64; 3]; 4];
const IDENTITY: Matrix = [
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
    [0.0, 0.0, 0.0],
];

fn apply(m: &Matrix, p: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|c| p[0] * m[0][c] + p[1] * m[1][c] + p[2] * m[2][c] + m[3][c])
}

/// `inner` first, then `outer`.
fn compose(inner: &Matrix, outer: &Matrix) -> Matrix {
    let mut r = [[0.0; 3]; 4];
    for (row, out) in r.iter_mut().enumerate() {
        for (c, v) in out.iter_mut().enumerate() {
            *v = (0..3).map(|k| inner[row][k] * outer[k][c]).sum::<f64>()
                + if row == 3 { outer[3][c] } else { 0.0 };
        }
    }
    r
}

fn det(m: &Matrix) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

fn parse_matrix(s: &str) -> Result<Matrix> {
    let v: Vec<f64> = s
        .split_ascii_whitespace()
        .map(|t| t.parse::<f64>())
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| malformed(format!("transform \"{s}\" is not 12 numbers")))?;
    if v.len() != 12 || v.iter().any(|x| !x.is_finite()) {
        return Err(malformed(format!(
            "transform \"{s}\" must be 12 finite numbers"
        )));
    }
    let m: Matrix = std::array::from_fn(|r| std::array::from_fn(|c| v[3 * r + c]));
    let d = det(&m);
    let scale = m[..3].iter().flatten().map(|x| x.abs()).fold(0.0, f64::max);
    if d.abs() <= 1e-12 * scale.powi(3) || scale == 0.0 {
        return Err(malformed(format!("transform \"{s}\" is singular")));
    }
    Ok(m)
}

fn parse_color(s: &str) -> Option<[u8; 4]> {
    let h = s.strip_prefix('#')?;
    if !(h.len() == 6 || h.len() == 8) || !h.is_ascii() {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?, if h.len() == 8 { b(6)? } else { 255 }])
}

#[derive(Default)]
struct Object {
    name: Option<String>,
    otype: String,
    pid: Option<u32>,
    pindex: Option<u32>,
    positions: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
    components: Vec<(u32, Matrix)>,
    has_mesh: bool,
    /// Triangles carrying their own pid/p1..p3.
    own_props: usize,
}

struct Attrs(Vec<(Vec<u8>, String)>);

impl Attrs {
    fn of(e: &BytesStart<'_>) -> Result<Self> {
        let mut v = Vec::new();
        for a in e.attributes() {
            let a = a.map_err(|err| malformed(format!("bad attribute: {err}")))?;
            let val = a
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map_err(|err| malformed(format!("bad attribute value: {err}")))?;
            v.push((a.key.as_ref().to_vec(), val.into_owned()));
        }
        Ok(Self(v))
    }
    fn get(&self, k: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| n == k.as_bytes())
            .map(|(_, v)| v.as_str())
    }
    fn f64(&self, k: &str, el: &str) -> Result<f64> {
        let s = self
            .get(k)
            .ok_or_else(|| malformed(format!("<{el}> without {k}")))?;
        let v: f64 = s
            .trim()
            .parse()
            .map_err(|_| malformed(format!("<{el}> {k}=\"{s}\" is not a number")))?;
        if !v.is_finite() {
            return Err(ExchangeError::NonFinite {
                format: FORMAT.into(),
                line: None,
                message: format!("<{el}> {k}=\"{s}\""),
            });
        }
        Ok(v)
    }
    fn u32(&self, k: &str, el: &str) -> Result<Option<u32>> {
        self.get(k)
            .map(|s| {
                s.trim()
                    .parse::<u32>()
                    .map_err(|_| malformed(format!("<{el}> {k}=\"{s}\" is not an index")))
            })
            .transpose()
    }
}

/// The start part named by `_rels/.rels` (default `3D/3dmodel.model` with a
/// warning when the relationships part is missing).
fn model_part_name(z: &ZipReader<'_>, warnings: &mut Vec<String>) -> Result<String> {
    let Some(e) = z.find("_rels/.rels") else {
        warnings.push("no _rels/.rels: using the default model part 3D/3dmodel.model".into());
        return Ok(MODEL_PART.into());
    };
    let bytes = z.read(e)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| malformed("_rels/.rels is not UTF-8"))?;
    let mut r = NsReader::from_str(text);
    let mut targets = Vec::new();
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if e.local_name().as_ref() == b"Relationship" {
                    let a = Attrs::of(&e)?;
                    if a.get("Type") == Some(MODEL_REL_TYPE)
                        && let Some(t) = a.get("Target")
                    {
                        targets.push(t.trim_start_matches('/').to_string());
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(err) => return Err(malformed(format!("_rels/.rels: {err}"))),
        }
    }
    match targets.len() {
        0 => Err(malformed("_rels/.rels names no 3D model part")),
        1 => Ok(targets.remove(0)),
        n => {
            warnings.push(format!("{n} 3D model relationships; using the first"));
            Ok(targets.remove(0))
        }
    }
}

/// Read a 3MF package: every build item as one mm mesh.
pub fn read(bytes: &[u8], limits: &ThreeMfLimits) -> Result<ThreeMfImport> {
    let z = ZipReader::new(
        bytes,
        ZipLimits {
            max_archive_bytes: limits.max_archive_bytes,
            max_entries: limits.max_archive_entries,
            max_entry_bytes: limits.max_model_bytes,
        },
    )?;
    let mut warnings = Vec::new();
    let part = model_part_name(&z, &mut warnings)?;
    let entry = z
        .find(&part)
        .ok_or_else(|| malformed(format!("model part {part} is missing from the package")))?;
    let model = z.read(entry)?;
    let text = std::str::from_utf8(&model).map_err(|_| malformed("model part is not UTF-8"))?;
    parse_model(text, limits, warnings)
}

fn parse_model(
    text: &str,
    limits: &ThreeMfLimits,
    mut warnings: Vec<String>,
) -> Result<ThreeMfImport> {
    let mut r = NsReader::from_str(text);
    let mut unit = None;
    let mut stack: Vec<Vec<u8>> = Vec::new(); // core-namespace local names (others: b"")
    let mut objects: HashMap<u32, Object> = HashMap::new();
    let mut cur: Option<(u32, Object)> = None;
    let mut materials: HashMap<u32, Vec<Option<[u8; 4]>>> = HashMap::new();
    let mut cur_mat: Option<u32> = None;
    let mut other_resources: HashMap<u32, String> = HashMap::new();
    let mut items: Vec<(u32, Matrix)> = Vec::new();
    let mut metadata: Vec<(String, String)> = Vec::new();
    let mut meta_text: Option<(String, String)> = None;
    let (mut n_vertices, mut n_triangles) = (0usize, 0usize);
    let mut foreign: HashMap<String, usize> = HashMap::new();
    loop {
        let pos = r.buffer_position();
        let (ns, ev) = r
            .read_resolved_event()
            .map_err(|e| malformed(format!("XML error near byte {pos}: {e}")))?;
        let core = matches!(&ns, ResolveResult::Bound(n) if n.as_ref() == CORE_NS.as_bytes());
        let ns_name = match &ns {
            ResolveResult::Bound(n) => String::from_utf8_lossy(n.as_ref()).into_owned(),
            _ => String::new(),
        };
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(ev, Event::Empty(_));
                let name = e.local_name().as_ref().to_vec();
                let parent = stack.last().map(|v| v.as_slice());
                if stack.is_empty() {
                    if !(core && name == b"model") {
                        return Err(malformed(format!(
                            "root element is not a 3MF core <model> (namespace \"{ns_name}\")"
                        )));
                    }
                    let a = Attrs::of(e)?;
                    let u = a.get("unit").unwrap_or("millimeter");
                    unit = Some(ThreeMfUnit::parse(u).ok_or_else(|| {
                        ExchangeError::UnsupportedUnits {
                            format: FORMAT.into(),
                            message: format!("unit \"{u}\""),
                        }
                    })?);
                    check_required_extensions(&a, &mut warnings)?;
                } else if !core {
                    *foreign.entry(ns_name.clone()).or_default() += 1;
                } else {
                    let a = Attrs::of(e)?;
                    let el = String::from_utf8_lossy(&name).into_owned();
                    match (parent, name.as_slice()) {
                        (Some(b"model"), b"metadata") => {
                            meta_text =
                                Some((a.get("name").unwrap_or("").to_string(), String::new()));
                            if empty {
                                metadata.push(meta_text.take().unwrap_or_default());
                            }
                        }
                        (Some(b"model"), b"resources") | (Some(b"model"), b"build") => {}
                        (Some(b"resources"), b"object") => {
                            let id = a
                                .u32("id", &el)?
                                .ok_or_else(|| malformed("<object> without id"))?;
                            if objects.contains_key(&id)
                                || materials.contains_key(&id)
                                || cur.as_ref().is_some_and(|(c, _)| *c == id)
                            {
                                return Err(malformed(format!("duplicate resource id {id}")));
                            }
                            cur = Some((
                                id,
                                Object {
                                    name: a.get("name").map(str::to_string),
                                    otype: a.get("type").unwrap_or("model").to_string(),
                                    pid: a.u32("pid", &el)?,
                                    pindex: a.u32("pindex", &el)?,
                                    ..Default::default()
                                },
                            ));
                        }
                        (Some(b"resources"), b"basematerials") => {
                            let id = a
                                .u32("id", &el)?
                                .ok_or_else(|| malformed("<basematerials> without id"))?;
                            if objects.contains_key(&id) || materials.contains_key(&id) {
                                return Err(malformed(format!("duplicate resource id {id}")));
                            }
                            materials.insert(id, Vec::new());
                            cur_mat = Some(id);
                        }
                        (Some(b"basematerials"), b"base") => {
                            let c = a.get("displaycolor").and_then(parse_color);
                            if let Some(m) = cur_mat.and_then(|id| materials.get_mut(&id)) {
                                m.push(c);
                            }
                        }
                        (Some(b"object"), b"mesh") => {
                            if let Some((_, o)) = &mut cur {
                                o.has_mesh = true;
                            }
                        }
                        (Some(b"mesh"), b"vertices") | (Some(b"mesh"), b"triangles") => {}
                        (Some(b"vertices"), b"vertex") => {
                            n_vertices += 1;
                            if n_vertices > limits.max_vertices {
                                return Err(limit("vertices", limits.max_vertices, n_vertices));
                            }
                            let p = [a.f64("x", &el)?, a.f64("y", &el)?, a.f64("z", &el)?];
                            if let Some((_, o)) = &mut cur {
                                o.positions.push(p);
                            }
                        }
                        (Some(b"triangles"), b"triangle") => {
                            n_triangles += 1;
                            if n_triangles > limits.max_triangles {
                                return Err(limit("triangles", limits.max_triangles, n_triangles));
                            }
                            let v = |k| {
                                a.u32(k, "triangle")?
                                    .ok_or_else(|| malformed(format!("<triangle> without {k}")))
                            };
                            let t = [v("v1")?, v("v2")?, v("v3")?];
                            let own = a.get("pid").is_some()
                                || ["p1", "p2", "p3"].iter().any(|k| a.get(k).is_some());
                            if let Some((_, o)) = &mut cur {
                                o.triangles.push(t);
                                o.own_props += own as usize;
                            }
                        }
                        (Some(b"object"), b"components") => {}
                        (Some(b"components"), b"component") => {
                            if a.get("path").is_some() {
                                return Err(zip::unsupported(
                                    "production extension",
                                    "components referencing other model parts (p:path)",
                                ));
                            }
                            let id = a
                                .u32("objectid", &el)?
                                .ok_or_else(|| malformed("<component> without objectid"))?;
                            let m = a.get("transform").map(parse_matrix).transpose()?;
                            if let Some((_, o)) = &mut cur {
                                o.components.push((id, m.unwrap_or(IDENTITY)));
                            }
                        }
                        (Some(b"build"), b"item") => {
                            if a.0.iter().any(|(k, _)| k.ends_with(b":path")) {
                                return Err(zip::unsupported(
                                    "production extension",
                                    "build items referencing other model parts (p:path)",
                                ));
                            }
                            if items.len() >= limits.max_build_items {
                                return Err(limit(
                                    "build items",
                                    limits.max_build_items,
                                    items.len() + 1,
                                ));
                            }
                            let id = a
                                .u32("objectid", &el)?
                                .ok_or_else(|| malformed("<item> without objectid"))?;
                            let m = a.get("transform").map(parse_matrix).transpose()?;
                            items.push((id, m.unwrap_or(IDENTITY)));
                        }
                        (Some(b"resources"), _) => {
                            // A core resource we do not import (none in core 1.3
                            // besides objects and base materials).
                            if let Some(id) = a.u32("id", &el)? {
                                other_resources.insert(id, el.clone());
                            }
                            *foreign.entry(format!("core <{el}>")).or_default() += 1;
                        }
                        (p, _) => {
                            return Err(malformed(format!(
                                "unexpected <{el}> inside <{}>",
                                p.map(|p| String::from_utf8_lossy(p).into_owned())
                                    .unwrap_or_default()
                            )));
                        }
                    }
                }
                if !empty {
                    stack.push(if core || stack.is_empty() {
                        name
                    } else {
                        Vec::new()
                    });
                } else if core && name == b"object" {
                    finish_object(&mut cur, &mut objects)?;
                }
            }
            Event::End(ref e) => {
                let name = e.local_name().as_ref().to_vec();
                if let Some(top) = stack.pop()
                    && top == name
                {
                    match name.as_slice() {
                        b"object" => finish_object(&mut cur, &mut objects)?,
                        b"basematerials" => cur_mat = None,
                        b"metadata" => {
                            if let Some(m) = meta_text.take() {
                                metadata.push(m);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Event::Text(t) => {
                if let Some((_, v)) = &mut meta_text
                    && v.len() < 1000
                {
                    v.push_str(&t.decode().map_err(|e| malformed(format!("{e}")))?);
                }
            }
            Event::GeneralRef(g) => {
                if let Some((_, v)) = &mut meta_text {
                    let c = match g.as_ref() {
                        b"lt" => Some('<'),
                        b"gt" => Some('>'),
                        b"amp" => Some('&'),
                        b"quot" => Some('"'),
                        b"apos" => Some('\''),
                        _ => g.resolve_char_ref().ok().flatten(),
                    };
                    if let Some(c) = c {
                        v.push(c);
                    }
                }
            }
            Event::DocType(_) => {
                return Err(malformed("DOCTYPE declarations are not allowed in 3MF"));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let unit = unit.ok_or_else(|| malformed("no <model> element"))?;
    if !stack.is_empty() {
        return Err(ExchangeError::Truncated {
            format: FORMAT.into(),
            message: "model XML ends inside an element".into(),
        });
    }
    for (ns, n) in &foreign {
        warnings.push(format!("ignored {n} element(s) from {ns}"));
    }
    for m in metadata.iter_mut() {
        if m.1.chars().count() > 1000 {
            m.1 = m.1.chars().take(1000).collect();
        }
    }
    if items.is_empty() {
        return Err(ExchangeError::Empty {
            format: FORMAT.into(),
            message: "the model has no build items".into(),
        });
    }
    let to_mm = unit.to_mm();
    let mut out = Vec::new();
    let mut total = 0usize;
    let mut mirrored = 0usize;
    let mut own_props = 0usize;
    for (id, m) in &items {
        let root = objects
            .get(id)
            .ok_or_else(|| malformed(format!("build item references unknown object {id}")))?;
        let mut positions = Vec::new();
        let mut triangles = Vec::new();
        flatten(
            &objects,
            *id,
            m,
            0,
            limits,
            &mut total,
            &mut mirrored,
            &mut own_props,
            &mut positions,
            &mut triangles,
        )?;
        for p in &mut positions {
            *p = p.map(|v| v * to_mm);
        }
        let color = match (root.pid, root.pindex) {
            (Some(pid), idx) => match materials.get(&pid) {
                Some(bases) => {
                    let k = idx.unwrap_or(0) as usize;
                    match bases.get(k) {
                        Some(c) => *c,
                        None => {
                            return Err(malformed(format!(
                                "object {id}: pindex {k} outside basematerials {pid}"
                            )));
                        }
                    }
                }
                None => {
                    warnings.push(format!(
                        "object {id}: property group {pid} ({}) is not imported",
                        other_resources
                            .get(&pid)
                            .map(String::as_str)
                            .unwrap_or("materials extension or unknown")
                    ));
                    None
                }
            },
            _ => None,
        };
        let diagnostics = mesh::diagnose(&positions, &triangles);
        if root.otype != "model" {
            warnings.push(format!(
                "object {id} has type \"{}\" (imported as a mesh like any other)",
                root.otype
            ));
        }
        out.push(ImportedObject {
            name: root.name.clone(),
            object_id: *id,
            object_type: root.otype.clone(),
            positions,
            triangles,
            color,
            diagnostics,
        });
    }
    if mirrored > 0 {
        warnings.push(format!(
            "{mirrored} mirroring transform(s): triangle winding reversed to keep outward orientation"
        ));
    }
    if own_props > 0 {
        warnings.push(format!(
            "{own_props} triangle(s) carry their own material/colour; only object colours are imported"
        ));
    }
    Ok(ThreeMfImport {
        unit,
        objects: out,
        metadata,
        warnings,
    })
}

fn check_required_extensions(a: &Attrs, warnings: &mut Vec<String>) -> Result<()> {
    let Some(req) = a.get("requiredextensions") else {
        return Ok(());
    };
    for prefix in req.split_ascii_whitespace() {
        let key = format!("xmlns:{prefix}");
        let ns = a.get(&key).ok_or_else(|| {
            malformed(format!(
                "required extension prefix \"{prefix}\" is not declared on <model>"
            ))
        })?;
        if ns == MATERIAL_NS {
            warnings.push(
                "required materials extension: geometry imported, colour groups/textures/composites are not".into(),
            );
        } else {
            return Err(zip::unsupported(
                "required 3MF extension",
                format!("{ns} is not supported"),
            ));
        }
    }
    Ok(())
}

fn finish_object(
    cur: &mut Option<(u32, Object)>,
    objects: &mut HashMap<u32, Object>,
) -> Result<()> {
    let Some((id, o)) = cur.take() else {
        return Ok(());
    };
    if o.has_mesh && !o.components.is_empty() {
        return Err(malformed(format!(
            "object {id} has both a mesh and components"
        )));
    }
    if !o.has_mesh && o.components.is_empty() {
        return Err(malformed(format!(
            "object {id} has neither a mesh nor components"
        )));
    }
    if let Some(t) = o
        .triangles
        .iter()
        .find(|t| t.iter().any(|&v| v as usize >= o.positions.len()))
    {
        return Err(malformed(format!(
            "object {id}: triangle {t:?} references a vertex beyond {}",
            o.positions.len()
        )));
    }
    objects.insert(id, o);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn flatten(
    objects: &HashMap<u32, Object>,
    id: u32,
    m: &Matrix,
    depth: usize,
    limits: &ThreeMfLimits,
    total: &mut usize,
    mirrored: &mut usize,
    own_props: &mut usize,
    positions: &mut Vec<[f64; 3]>,
    triangles: &mut Vec<[u32; 3]>,
) -> Result<()> {
    if depth > limits.max_component_depth {
        return Err(limit(
            "component nesting levels (or a component cycle)",
            limits.max_component_depth,
            depth,
        ));
    }
    let o = objects
        .get(&id)
        .ok_or_else(|| malformed(format!("component references unknown object {id}")))?;
    if o.has_mesh {
        *total += o.triangles.len();
        if *total > limits.max_output_triangles {
            return Err(limit(
                "triangles after flattening",
                limits.max_output_triangles,
                *total,
            ));
        }
        *own_props += o.own_props;
        let flip = det(m) < 0.0;
        *mirrored += flip as usize;
        let base = positions.len() as u32;
        positions.extend(o.positions.iter().map(|p| apply(m, *p)));
        triangles.extend(o.triangles.iter().map(|t| {
            let t = t.map(|v| v + base);
            if flip { [t[0], t[2], t[1]] } else { t }
        }));
    } else {
        for (child, cm) in &o.components {
            flatten(
                objects,
                *child,
                &compose(cm, m),
                depth + 1,
                limits,
                total,
                mirrored,
                own_props,
                positions,
                triangles,
            )?;
        }
    }
    Ok(())
}

/// `v` with `decimals` decimals, trailing zeros (and a bare point) removed;
/// never "-0".
fn fixed(v: f64, decimals: usize) -> String {
    let t = format!("{v:.decimals$}");
    let t = if t.contains('.') {
        t.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        t
    };
    if t == "-0" { "0".to_string() } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(s: f64) -> MeshObject {
        let p = |x: f64, y: f64, z: f64| [x * s, y * s, z * s];
        let positions = vec![
            p(0., 0., 0.),
            p(1., 0., 0.),
            p(1., 1., 0.),
            p(0., 1., 0.),
            p(0., 0., 1.),
            p(1., 0., 1.),
            p(1., 1., 1.),
            p(0., 1., 1.),
        ];
        let triangles = vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        MeshObject {
            name: Some("cube <&>".into()),
            positions,
            triangles,
            color: None,
        }
    }

    #[test]
    fn matrix_convention_and_composition() {
        // Rotate 90 deg about Z then translate (10, 0, 0): (1,0,0) -> (10,1,0).
        let m = parse_matrix("0 1 0 -1 0 0 0 0 1 10 0 0").unwrap();
        assert_eq!(apply(&m, [1.0, 0.0, 0.0]), [10.0, 1.0, 0.0]);
        let t = parse_matrix("1 0 0 0 1 0 0 0 1 0 0 5").unwrap();
        // Inner rotation+translation, then +5 z.
        assert_eq!(apply(&compose(&m, &t), [1.0, 0.0, 0.0]), [10.0, 1.0, 5.0]);
        assert!(parse_matrix("0 0 0 0 0 0 0 0 0 1 2 3").is_err());
        assert!(parse_matrix("1 0 0 0 1 0 0 0 1 0 0").is_err());
        assert!(parse_matrix("1 0 0 0 1 0 0 0 1 0 0 nan").is_err());
    }

    #[test]
    fn write_read_round_trip_in_inches_with_colour() {
        let mut a = cube(25.4);
        a.color = Some([255, 0, 0, 255]);
        let b = MeshObject {
            name: None,
            ..cube(12.7)
        };
        let z = write(&[a, b], ThreeMfUnit::Inch).unwrap();
        let r = read(&z, &ThreeMfLimits::default()).unwrap();
        assert_eq!(r.unit, ThreeMfUnit::Inch);
        assert_eq!(r.objects.len(), 2);
        assert_eq!(r.objects[0].name.as_deref(), Some("cube <&>"));
        assert_eq!(r.objects[0].color, Some([255, 0, 0, 255]));
        assert_eq!(r.objects[1].color, None);
        let v0 = r.objects[0].diagnostics.signed_volume.unwrap();
        assert!((v0 - 25.4f64.powi(3)).abs() < 1e-9 * v0);
        assert!(r.objects[1].diagnostics.closed);
    }

    #[test]
    fn garbage_and_truncations_are_errors() {
        let z = write(&[cube(1.0)], ThreeMfUnit::Millimeter).unwrap();
        for n in [0, 10, z.len() / 2, z.len() - 1] {
            assert!(read(&z[..n], &ThreeMfLimits::default()).is_err());
        }
        assert!(read(b"solid not a zip", &ThreeMfLimits::default()).is_err());
    }
}
