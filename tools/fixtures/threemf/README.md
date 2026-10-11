# 3MF fixtures: provenance

Committed under `crates/f420-exchange/tests/fixtures/threemf/`, used by
`crates/f420-exchange/tests/threemf_fixtures.rs`. Expected values are analytic (stated in
the generators and asserted in the tests).

| file | writer | content | expected |
|---|---|---|---|
| `freecad_two_boxes.3mf` | FreeCAD 1.1.4 Mesh 3MF writer (`freecad_3mf.py`, `F420_MODE=generate`), deflated | `makeBox(10,10,10)` and `makeBox(20,10,5)` at (50,0,0), two objects, two items | mm; 12 triangles each, closed, V 1000 each; bounds [0,10]^3 and [50,70]x[0,10]x[0,5] |
| `box_inch_rotated.3mf` | `gen_handwritten.py` (Python stdlib) | box 1 x 0.5 x 0.25 in, item transform `0 1 0 -1 0 0 0 0 1 2 0 0` (+90 deg about Z, +2 in X) | x [38.1, 50.8], y [0, 25.4], z [0, 6.35] mm; V 0.125 in^3 |
| `components_mirror_stored.3mf` | hand-written, **stored** entries | tetrahedron (legs 10) used twice through `<components>`, once mirrored in X and moved +30 | one item "Pair": 2 components, 8 triangles, closed, V 2000/6, bounds [0,30]x[0,10]^2 |
| `colors.3mf` | hand-written | `<basematerials>` #FF000080 / #00FF00 for objects 1/2; a per-triangle `p1` on object 2; object 3 uses a materials-extension `m:colorgroup` (required extension `m`) | colours [255,0,0,128], [0,255,0,255], none; warnings for the triangle override and group 6 |
| `requires_production.3mf` | hand-written | `requiredextensions="p"` (production) | refused (Unsupported) |
| `bad_index.3mf` | hand-written | triangle index 8 with 8 vertices | refused (Malformed) |
| `no_build.3mf` | hand-written | empty `<build/>` | refused (Empty) |
| `doctype.3mf` | hand-written | `<!DOCTYPE>` with an internal entity | refused |
| `padded_2mib.3mf` | hand-written | 2 MiB XML comment before the resources (deflates to ~3 KB) | read by default; `LimitExceeded` with `max_model_bytes` = 1 MiB |
| `lying_size.3mf` | hand-written, then the model's declared uncompressed size patched to 1000 bytes in both headers | | refused; never inflated past the declared size |
| `no_rels.3mf` | hand-written without `_rels/.rels` | 1 mm cube | read with a warning (default part name) |

Regenerate: `python3 -I tools/fixtures/threemf/gen_handwritten.py crates/f420-exchange/tests/fixtures/threemf`
and `flatpak run --env=F420_MODE=generate --env=F420_OUT=<that dir> --filesystem=<repo>
--command=FreeCADCmd org.freecad.FreeCAD tools/fixtures/threemf/freecad_3mf.py`
(zip timestamps differ between runs; the XML is deterministic).

## FreeCAD 1.1.4's reading of the fixtures (observed 2026-10-11, `F420_MODE=read`)

| file | FreeCAD result | note |
|---|---|---|
| `box_inch_rotated.3mf` | 8 points, 12 facets, V 0.125, bounds x [1.5, 2], y [0, 1], z [0, 0.25] | applies the item transform with the same row-vector convention as ours; **ignores `unit`** (values stay in inches) |
| `components_mirror_stored.3mf` | 4 points, 4 facets, V 166.67 | **does not flatten components** (only one tetrahedron) |
| `colors.3mf` | 24 points, 36 facets, V 3000 | merges all items into one mesh |
| `freecad_two_boxes.3mf` | 16 points, 24 facets, V 2000 | |
| `no_rels.3mf` | 8 points, 12 facets, V 1 | |

FreeCAD is therefore a usable independent check of geometry, item transforms and
deflate/stored packaging, but not of units or components; `check_3mf.py` (stdlib
`zipfile` + `ElementTree`) checks units, objects, items and colours of our writer.

## Writer validation

`tools/fixtures/threemf/validate_writer.sh` writes `f420_two_mm.3mf` / `f420_two_inch.3mf`
(`crates/f420-exchange/examples/threemf_samples.rs`: 10 mm cube coloured #FF8000 + tetrahedron
legs 12) and checks them with `check_3mf.py` (unit, 2 objects/items, closed, V 1000 and 288
mm^3 scaled by 25.4^-3 for inch, colour) and FreeCAD (12 points, 16 facets, solid, V 1288 in
file units). Result 2026-10-11: all PASS (`artifacts/threemf_validate_writer.log`).
