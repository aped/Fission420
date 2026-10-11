# 3MF support (`f420_exchange::threemf`)

Status: run 3, adapter only. **Not wired into commands, the document model or the UI**:
no `ImportThreeMf`/`Export3mf` command exists yet (see "Command integration" below).

Clean-room implementation from the public 3MF Core Specification 1.3 (3MF Consortium) and
ECMA-376 Part 2 (Open Packaging Conventions). Container: `crates/f420-exchange/src/zip.rs`
(own ZIP reader/writer; deflate by `miniz_oxide`, CRC-32 by `crc32fast`); XML tokenizing by
`quick-xml` (all three were already in `Cargo.lock`, see docs/DEPENDENCIES.md).

## Read: `threemf::read(bytes, &ThreeMfLimits) -> Result<ThreeMfImport>`
* Package: ZIP stored (0) or deflate (8) entries; start part from `_rels/.rels`
  (relationship type `.../2013/01/3dmodel`; missing `.rels` → default `3D/3dmodel.model`
  with a warning). Part names are matched ASCII case-insensitively. Refused: ZIP64,
  encrypted entries, other compression methods, multi-disk archives.
* Model: core namespace elements only; `unit` micron/millimeter/centimeter/inch/foot/meter
  (default millimeter; anything else → `UnsupportedUnits`); `<object>` with `<mesh>` or
  `<components>` (not both); `<build><item>` with optional `transform`.
* Output: **one `ImportedObject` per build item**, positions in mm, item and (nested)
  component transforms applied (`ST_Matrix3D` row-vector convention `p' = [x y z 1]·M`,
  confirmed against FreeCAD's reader); a negative-determinant (mirroring) transform
  reverses triangle winding so outward stays outward (warning); singular transforms are
  refused. Each object carries `mesh::diagnose` results (closed, boundary edges, volume only
  when closed, …), its object id, type and name.
* Colour: the object-level `pid`/`pindex` into a core `<basematerials>` `displaycolor`
  (`#RRGGBB[AA]`). **Not imported, reported as warnings**: per-triangle properties
  (`pid`/`p1..p3` on triangles), materials-extension colour groups, textures, composites,
  multi-properties. Material names are not kept.
* Extensions: elements in other namespaces are ignored and counted in `warnings`.
  `requiredextensions` naming the materials extension → accepted with a warning; any other
  required extension (production, beam lattice, slice, …) → `Unsupported`. Production
  `p:path` references → `Unsupported`.
* Metadata: `<metadata name>` text, truncated to 1000 characters.
* Errors: `Malformed` (XML error with byte offset, DOCTYPE, bad numbers/indices, duplicate
  ids, unknown references, index beyond the vertex count, object with neither or both mesh
  and components), `NonFinite`, `Empty` (no build items), `Truncated`, `LimitExceeded`.

### Limits (`ThreeMfLimits::default()`)
Archive 256 MiB, 10 000 entries; decompressed model (and `.rels`) 1 GiB, checked against
the declared size **before** inflating, and inflation stops at the declared size (a header
that under-declares is refused, not followed); 10 M vertices and 10 M triangles in the
file; 10 000 build items; component depth 16 (also catches cycles); 20 M triangles after
flattening (instancing cannot multiply output unboundedly). XML is streamed (no DOM);
DOCTYPE is refused, so there is no entity expansion.

## Write: `threemf::write(&[MeshObject], ThreeMfUnit) -> Result<Vec<u8>>`
One `<object type="model">` per mesh with its name, exactly-equal vertices welded, one
`<build><item>` per object without transform, coordinates converted from mm to the chosen
unit (shortest round-trip decimal), optional object colours as one `<basematerials>`
group (`displaycolor` `#RRGGBBAA`). Package parts: `[Content_Types].xml`, `_rels/.rels`,
`3D/3dmodel.model`, deflated, fixed timestamps (byte-for-byte deterministic). Refused:
no objects, an object without triangles, out-of-range indices, non-finite coordinates.
Output meshes are not repaired: a non-manifold input stays non-manifold (the caller decides
whether to export it).

## Known losses
Units on import are converted to mm (the source unit is reported); per-triangle colours,
textures, colour groups, material names, thumbnails, print tickets, slice data, beam
lattices and production-extension structure are not imported; on export only object
colours are written; no components/instancing is written (each body is a full mesh);
exact B-rep geometry is tessellated by the caller (3MF is a mesh format).

## Evidence
* `crates/f420-exchange/tests/threemf_fixtures.rs` (9 tests) on FreeCAD-written and
  hand-written fixtures (`tools/fixtures/threemf/README.md`): FreeCAD two-object file,
  inch + rotated item, components + mirror, colours/warnings, production refused,
  bad index, no build, DOCTYPE, decompression bound, lying size header, missing `.rels`,
  every prefix of two packages refused and every byte flip handled without panic;
  own-writer round trip (supplementary) in mm/inch/micron.
* Unit tests in `zip.rs` / `threemf.rs`: matrix convention, round trip with colour,
  truncation and bit-flip sweeps, declared-size limit.
* Writer validated independently: `tools/fixtures/threemf/validate_writer.sh` (Python stdlib
  reader + FreeCAD 1.1.4): PASS.

## Command integration (not done; proposed API)
```rust
/// Import every build item of a 3MF package as a mesh body (one ImportMesh-like base
/// feature per file, source embedded as a blob, name from the object name). Units come
/// from the file (always declared, default millimeter).
Command::Import3mf { path: PathBuf }
/// Write `bodies` (default: all visible) as one 3MF object each: B-rep bodies
/// tessellated at `chord` (watertightness checked as for STL), mesh bodies as imported;
/// `unit` (default millimeter); optional per-body colour when the document has one.
Command::Export3mf { path: PathBuf, #[serde(default)] chord: Option<f64>,
                     #[serde(default)] bodies: Option<Vec<BodyId>>,
                     #[serde(default)] unit: Option<ThreeMfUnit> }
```
Needed in `f420-document`: a `FeatureKind::ImportMesh` variant (or a format tag on the
existing one) that regenerates N mesh bodies from a 3MF blob via `threemf::read`, keeping
per-body colour if the body model gains one.
