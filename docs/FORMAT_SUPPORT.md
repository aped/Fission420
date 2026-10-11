# Format support

Status: run 3, 2026-10-10/11, branch `work/run-3` (format-verification worker on top of
`fa738cc`). This page claims only what a test or an independent validation listed under
**Evidence** proves. A file extension in a dialog is not evidence. Per-format details:
[`formats/step.md`](formats/step.md), [`formats/stl.md`](formats/stl.md),
[`formats/dxf.md`](formats/dxf.md); design: ADR 0003 (bodies, face references) and
ADR 0004 (format adapters).

## Matrix

| Format | Import | Export | Geometry type | Units | Reachable through |
|---|---|---|---|---|---|
| Native `.f420.json` v1 | **yes** (migrated to v3 on load) | no (always saves v3) | parametric history | mm | `Open` |
| Native v2 (run 2) | **yes** (migrated) | no | parametric history | mm | `Open` |
| Native v3 (run 3) | **yes** | **yes** | history + bodies + embedded import sources | mm | `Open` / `Save` |
| STEP `.step/.stp` (ISO 10303-21; entity subset, tested with AP214 files) | **yes, exact B-rep subset** | **yes**, exact B-rep, one solid per body | B-rep solid | declared unit honoured; override only when none is declared | `import_step`, `export_step` |
| STL `.stl` | **yes**, ASCII + binary | **yes**, binary | triangle mesh (mesh body) | none in file: unit choice required | `import_stl`, `export_stl` |
| DXF `.dxf` (ASCII) | **yes**, 2D sketch subset | **yes**, R12 LINE/ARC/CIRCLE | 2D sketch (unconstrained) | `$INSUNITS` honoured; choice required when absent | `import_dxf`, `export_dxf` |
| 3MF `.3mf` | **adapter only**: core-subset reader (not reachable from commands/UI) | **adapter only**: multi-object mesh writer | triangle meshes, one per build item | declared unit honoured (default mm); writer takes a unit | `f420_exchange::threemf` (Rust API, no command) |
| SVG `.svg` | no (later backlog) | no | | | |
| IGES, OBJ, DWG, F3D/F3Z | no (roadmap) | no | | | |

"Reachable through" lists the typed command (`crates/f420-commands`, also `f420-cli run`).
At commit `fa738cc` the desktop app exposes Export STL/STEP dialogs but **no import
dialogs**; UI import is tracked separately and is not claimed here.

Not supported, by design: renaming a file to another extension, a hidden external converter
(FreeCAD/OpenCascade is used only as an independent validator, never at runtime), mesh to
B-rep reconstruction, recovered feature history from STEP.

## Geometry contract per format

| | B-rep (exact) | Mesh | 2D sketch |
|---|---|---|---|
| STEP import | `ImportStep` base feature, one body per `MANIFOLD_SOLID_BREP` (per placed instance) | | |
| STEP export | each selected B-rep body as one `MANIFOLD_SOLID_BREP`; mesh bodies refused | refused | |
| STL import | never (no reconstruction) | `ImportMesh` base feature, one mesh body per file | |
| STL export | tessellated (chord tolerance, watertightness checked; not written if not watertight) | written as imported | |
| DXF import | | | new sketch on a chosen plane or planar face, no constraints except the origin |
| DXF export | | | a sketch's lines/arcs/circles in its local 2D coordinates |

Imported B-rep bodies take part in exact operations (sketch on their planar faces, Extrude
New/Join/Cut with them as target, export). Mesh bodies render, can be renamed, hidden, saved
and exported as meshes; every exact operation on them is refused with an explanation.

## Units

| Format | Behaviour | Evidence |
|---|---|---|
| Native | always mm | |
| STEP | `GLOBAL_UNIT_ASSIGNED_CONTEXT` length unit (SI with prefix, `CONVERSION_BASED_UNIT` such as INCH) converted to mm at import; all contexts must agree. No unit declared: refused ("units required") unless `units_to_mm` is given; a declared unit always wins over `units_to_mm`. Export writes mm. | C.6; `imports.rs::declared_step_unit_wins_over_override_and_bad_overrides_are_refused`; kernel `step_import.rs` |
| STL | file is unitless: `units_to_mm` is required (no silent mm default); must be finite and > 0. Export writes mm. | D.1, `imports.rs` |
| DXF | `$INSUNITS` 1-16, 21-24 converted to mm; 0/missing: refused unless `units_to_mm` is given; a declared unit always wins. Export writes `$INSUNITS = 4` (mm). | D.10, `dxf_fixtures.rs` |

## STEP

**Import subset** (details and every entity type: [`formats/step.md`](formats/step.md)):
`MANIFOLD_SOLID_BREP`/`CLOSED_SHELL` with `PLANE`, `CYLINDRICAL_SURFACE`,
`CONICAL_SURFACE`, `SPHERICAL_SURFACE`, `TOROIDAL_SURFACE` (ring), B-spline/NURBS surfaces;
`LINE`, `CIRCLE`, `ELLIPSE`, B-spline/NURBS curves, `SURFACE_CURVE`/`SEAM_CURVE`; assembly
placements flattened through `ITEM_DEFINED_TRANSFORMATION` chains (rigid only). All-or-nothing:
the first unsupported entity aborts with its type and `#id`. Unsupported (explicit error):
`BREP_WITH_VOIDS`, open shells, faceted/surface models, `MAPPED_ITEM`, offset/extrusion/
revolution surfaces, `TRIMMED_CURVE`, polylines, pcurve-only edges, non-rigid placements.
Assemblies are not supported as assemblies: placed solids become independent bodies.

Tested fixtures (all FreeCAD 1.1.4 / OpenCascade 7.8.1 generated, provenance in
`crates/f420-kernel/tests/fixtures/step/README.md`), each through the command API (scenario C):

| Fixture | Parses | Solid validation (1 closed shell) | Volume vs analytic | Bounds | Downstream edit tested |
|---|---|---|---|---|---|
| `box_40x30x10_mm` | yes | yes | 12000, exact (Z integral) | exact | sketch on imported top face, Ø6 through-all cut; Join boss + side-face blind cut; DXF pocket (`imports.rs`) |
| `cylinder_d20_h15_mm` | yes | yes | 1500π, err 1e-9 | 1e-6 | top face offered for sketching |
| `plate_hole_mm` (planes + cylinder) | yes | yes | 19200 − 200π, err 1e-10 | 1e-6 | second Ø6 hole (`imports.rs`); existing hole axis/diameter checked |
| `sphere_r10_mm` (centre 5,5,5) | yes | yes | labelled **estimate** 4187.72 ± 3.15 contains 4000π/3 | 1e-6 | none (no planar face) |
| `torus_R20_r5_mm` | yes (kernel test only) | yes | labelled estimate | not checked: extremes are not on edges | none |
| `box_40x30x10_inch`, `_m` | yes | yes | 12000 | 40×30×10 mm | |
| `freecad_relabel_inch` | yes | yes | 196 644 768 (1016×762×254 mm) | | |
| `missing_unit` | refused without a unit; with 25.4 → 1016×762×254 | | | | |
| `two_boxes_assembly_mm` (IDT placement) | yes, 2 bodies named BoxA/BoxB | yes | 1000 each | B placed at [45, 67.3205]×[0, 18.6603]×[0,5] | |
| `two_boxes_compound_mm` (baked placement) | yes, 2 bodies | yes | 1000 each | same | |
| `truncated`, `garbage`, missing file | refused, bounded message, document and revision unchanged (also into a non-empty document) | | | | |

"Rendering" is not claimed by these checks; the kernel tests tessellate the cylinder,
sphere, torus and the imported box with hole watertight (`step_import.rs`).

**Export**: AP214 (`AUTOMOTIVE_DESIGN`) via the Truck writer with exact-edge replacement;
cylinders and other curved faces are written as rational B-spline surfaces (the analytic
surface type is lost). One `MANIFOLD_SOLID_BREP` per selected body, one PRODUCT.

**Known losses**: colours, layers, PMI, product structure beyond names and flattened
placements, the source unit (geometry is converted to mm), the original face partition of
periodic faces, analytic surface types on export.

## STL

ASCII and binary detected by exact length (`84 + 50n`), not by the `solid` token; vertices
welded by exact coordinate equality; diagnostics: triangles, vertices, bounds, boundary /
non-manifold / misoriented edges, components, area, and a volume **only when closed**.
Limits: 512 MiB, 10 M triangles; malformed, non-finite, truncated inputs are errors with a
line number or the expected/actual size.

Scenario D evidence: the FreeCAD box (20×10×5) as ASCII and binary gives identical
diagnostics (12 triangles, 8 vertices, closed, V 1000, A 700); at 25.4 mm/unit, bounds
508×254×127 and V 1000·25.4³; the open box (2 facets removed) imports with an "open or
inconsistent: 4 boundary" note, `closed = false`, no volume and no B-rep measurements;
malformed/NaN/truncated/unterminated files are refused with the document unchanged; STEP
export and Cut of a mesh body are refused and nothing is written; the mesh body exports as a
684-byte binary STL validated by a Python reader and FreeCAD (V 1000, solid).

**Known losses**: no units, colour, topology or exact geometry; binary STL is f32
(~7 significant digits); ASCII export exists in the crate but is not a command option.

## DXF

Import: LINE, ARC, CIRCLE, LWPOLYLINE and 2D POLYLINE (bulges become exact arcs); WCS XY
plane only (OCS (0,0,−1) handled); layers selectable; unsupported entities (SPLINE, ELLIPSE,
INSERT/blocks, TEXT, DIMENSION, HATCH, 3D entities, paper space, …) refuse the file with
type/count/line unless `allow_partial`, which skips and lists them. Endpoints within 1e-6 mm
are joined; no constraints are inferred, so the sketch reports `UnderConstrained`.
Export: R12 ASCII LINE/ARC/CIRCLE in local sketch coordinates (mm); construction geometry
omitted; no placement, constraints or dimensions.

Scenario D evidence: FreeCAD `slot_freecad_r14.dxf` imports to 3 lines + 1 arc at the
analytic coordinates (error 0); edited (left edge moved to x = −10) and extruded 5 mm:
V = 5(800 + 50π) exact; STEP export validated by FreeCAD; DXF export read back by our
importer (local geometry error 0) **and** by FreeCAD's DXF importer (3 lines + arc
(30,10) r10 180°, closed, area 800 + 50π); `rect_inch.dxf` → 50.8×25.4 mm (declared inch
wins over a 1 mm override); unitless `notch_freecad_r12.dxf` refused without a unit, then
imported at 1 mm/unit and extruded: V = 3(800 − 12.5π). A DXF contour imported onto an
imported STEP face cuts the imported body (`imports.rs`).

**Known losses**: splines/ellipses/text/dimensions/hatches/blocks, widths, thickness,
linetype and colour; design intent (constraints); sketch placement on export.

## 3MF (adapter only)

`f420_exchange::threemf::{read, write}` ([`formats/threemf.md`](formats/threemf.md)).
**Not reachable from the command API, the CLI or the UI yet**, so a user cannot import or
export 3MF in this build; the proposed commands are in that page.
Read: ZIP stored/deflate, `_rels/.rels` start part, units micron..meter converted to mm, one
mesh per build item with item and nested component transforms (mirrors reverse winding),
object-level base-material colours; per-triangle colours, colour groups, textures and other
extension content are reported as warnings, required extensions other than materials and any
production-extension reference are refused; bounded archive/decompression/vertex/triangle/
item/depth/output limits; DOCTYPE refused. Write: one object + build item per mesh, welded
vertices, chosen unit, optional object colours. Evidence: FreeCAD-written two-object file and
hand-written packages (inch + rotated item, components + mirror, colours, malformed,
decompression bomb, lying size header); writer read back by FreeCAD and an independent Python
reader. FreeCAD's own 3MF reader ignores the `unit` attribute and does not flatten
components (observed), so units/components are checked by the Python reader instead.

## Native format

v3 stores features, bodies (id, name, visibility, source) and the bytes of every imported
file as base64 blobs, so a document reopens after the source file is moved or deleted
(scenario C.3, D.6, D.11 delete the sources before reopening and compare the whole document
and every body). v1 and v2 files migrate on load (first solid feature gets body B1; feature
ids unchanged); newer versions are refused. Evidence: `native_formats.rs` opens the v1
fixture and every committed v2/v3 example, checks the analytic volume, re-saves as v3 and
reopens to an identical document. Size limit 256 MiB per file, 128 MiB per import source.

## Evidence

Reproduce (from the repository root):

```sh
cargo run --release -p f420-cli -- acceptance --out artifacts/acceptance   # scenarios C and D are the "C." / "D." steps
tools/external_validation.sh artifacts/acceptance                          # FreeCAD checks incl. tools/check_formats.sh
cargo test --release -p f420-commands --test imports --test native_formats
cargo test --release -p f420-kernel --test step_import --test step_export_multi
cargo test --release -p f420-exchange
tools/check_step_multi.sh; tools/fixtures/stl/validate_writer.sh; tools/fixtures/dxf/validate_export.sh
```

| Evidence | Where | Result (2026-10-10) |
|---|---|---|
| Acceptance C.1-C.8, D.1-D.11 | `crates/f420-commands/src/acceptance/formats.rs`; report `artifacts/acceptance_formats/acceptance_report.json` | all checks PASS (650 total with A/B/P/R) |
| FreeCAD validation of C/D exports | `tools/check_formats.sh`; logs `artifacts/acceptance_formats/{scenario_c,scenario_d_slot}_step_check_freecad.txt`, `scenario_d_mesh_stl_check_*.{json,txt}`, `scenario_d_slot_dxf_check_freecad.txt`, `formats_check.txt`, `artifacts/external_validation_formats.log` | PASS: scenario_c 1 valid solid, V err 2.1e-7 mm³, bbox 40×30×10, hole loops at z 0/10 centred (20,15) area 9π, point probes; scenario_d_slot V err −2.9e-8; mesh STL V 1000; DXF area 800 + 50π |
| Command-level import edge cases | `crates/f420-commands/tests/imports.rs` | 10 pass, 2 ignored (backend issues below) |
| Native v1/v2/v3 | `crates/f420-commands/tests/native_formats.rs` | 7 pass |
| Kernel STEP import/export | `crates/f420-kernel/tests/step_import.rs`, `step_export_multi.rs`; `artifacts/format_tests_kernel_exchange.log` | 13 + 4 pass |
| STL/DXF adapters | `crates/f420-exchange/tests/{stl,dxf}_*`; `artifacts/format_tests_exchange.log` | 35 pass |
| 3MF adapter | `crates/f420-exchange/tests/threemf_fixtures.rs` + unit tests in `threemf.rs`/`zip.rs`; writer: `tools/fixtures/threemf/validate_writer.sh`, log `artifacts/threemf_validate_writer.log` | 9 + 6 pass; writer PASS (Python stdlib reader and FreeCAD) |
| Replayable workflows | `examples/imported_step_hole_workflow.json`, `examples/dxf_stl_import_workflow.json` (`f420-cli run`, from the repository root) with saved documents `examples/*.f420.json` | replayed by `imports.rs` |

`artifacts/` is not versioned; rerun the commands above to regenerate the logs.

## Known issues found by these checks

* `ExportStep` without `bodies` includes visible **mesh** bodies and therefore refuses the
  whole export, although its doc comment says "all visible B-rep bodies". Workaround: pass
  `bodies` explicitly. (`imports.rs::export_step_default_skips_mesh_bodies_as_documented`,
  ignored.)
* A blind (distance) cut sketched on a face and not reversed reports "the extrusion does
  not intersect the target body" instead of the intended "the cut points away from the
  material; reverse it". (`imports.rs::unreversed_blind_cut_from_face_says_reverse_it`,
  ignored.)
* A face-sketched contour whose edge is coplanar with a side face of the target (e.g. the
  40 mm slot on the 40 mm wide box top) makes the Truck boolean fail; the command returns a
  clean error and keeps the model.
