# 3MF support (`rcad_exchange::threemf`)

Wired into commands (`import_3mf`, `export_3mf`; document base feature
`Import3mf`, mesh bodies); desktop UI: Import… and Export 3MF…
(`crates/rcad/src/exchange.rs`, test `exchange_ui.rs::threemf_import_export_and_svg_sketch`). The commands
are listed under "Command integration" below (no colours: the document has no body colour yet).

Clean-room implementation from the public 3MF Core Specification 1.3 (3MF Consortium) and
ECMA-376 Part 2 (Open Packaging Conventions). Container: `crates/rcad-exchange/src/zip.rs`
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

`threemf::write_items(&[MeshObject], &[Option<ItemTransform>], unit)`: the same, with an
optional `transform` per build item (row-vector `ST_Matrix3D`; rotation rows written to 12
decimals, translation converted to the file unit; refused when non-finite or with a
determinant <= 0). Used by `export_3mf` with `item_transforms` (docs/PRINT_EXPORT.md): objects
in modeled coordinates, the export placement as each item's transform.

## Known losses
Units on import are converted to mm (the source unit is reported); per-triangle colours,
textures, colour groups, material names, thumbnails, print tickets, slice data, beam
lattices and production-extension structure are not imported; on export only object
colours are written; no components/instancing is written (each body is a full mesh);
exact B-rep geometry is tessellated by the caller (3MF is a mesh format).

## Evidence
* `crates/rcad-exchange/tests/it/threemf_fixtures.rs` (9 tests) on FreeCAD-written and
  hand-written fixtures (`tools/fixtures/threemf/README.md`): FreeCAD two-object file,
  inch + rotated item, components + mirror, colours/warnings, production refused,
  bad index, no build, DOCTYPE, decompression bound, lying size header, missing `.rels`,
  every prefix of two packages refused and every byte flip handled without panic;
  own-writer round trip (supplementary) in mm/inch/micron.
* Unit tests in `zip.rs` / `threemf.rs`: matrix convention, round trip with colour,
  truncation and bit-flip sweeps, declared-size limit.
* Writer validated independently: `tools/fixtures/threemf/validate_writer.sh` (Python stdlib
  reader + FreeCAD 1.1.4): PASS.

## Command integration

Commands in `crates/rcad-commands/src/lib.rs` (`Command` enum):

* `import_3mf {path}`: every build item of the package as a mesh body (display/export only, like
  STL), units from the file, source embedded as a blob behind one base feature
  `FeatureKind::Import3mf {blob, bodies}` (`crates/rcad-document/src/model.rs`), which regenerates
  the bodies through `rcad_document::import::threemf`. Body names come from the object names.
  Object colours read by `threemf::read` are dropped (`ThreeMfObject` has no colour).
* `export_3mf {path, chord?, bodies?, unit?, ...print options}`: `bodies` (default: all
  visible) as one object each, B-rep bodies tessellated at `chord` and solid-checked, mesh
  bodies as imported (solid-checked unless `allow_open_meshes`); `unit` default millimetre;
  print options (`item_transforms`, placement, ...) in docs/PRINT_EXPORT.md. Written by
  `crates/rcad-commands/src/print.rs` with `color: None`.
* `export_assembly {path, format: "3mf"}`: one object per placed body, named per occurrence,
  millimetres, default chord, solid-checked (`crates/rcad-commands/src/assembly.rs`, also
  `color: None`). No components or build-item hierarchy are written.
* `export_configurations {..., "3mf": true}`: `export_3mf` per configuration.
