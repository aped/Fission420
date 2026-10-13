# Format support

Status: run 3, 2026-10-11, branch `work/run-3` (checked against the code at `e703fbe`). This
page claims only what a test or an independent validation listed under
**Evidence** proves. A file extension in a dialog is not evidence. Per-format details:
[`formats/step.md`](formats/step.md), [`formats/stl.md`](formats/stl.md),
[`formats/dxf.md`](formats/dxf.md), [`formats/threemf.md`](formats/threemf.md),
[`formats/svg.md`](formats/svg.md); design: ADR 0003 (bodies, face references) and
ADR 0004 (format adapters).

## Matrix

| Format | Import | Export | Geometry type | Units | Reachable through |
|---|---|---|---|---|---|
| Native `.rcad.json` v1 | **yes** (migrated to v3 on load) | no (always saves v3) | parametric history | mm | `Open` |
| Native v2 (run 2) | **yes** (migrated) | no | parametric history | mm | `Open` |
| Native v3 (run 3) | **yes** | **yes** | history + bodies + embedded import sources | mm | `Open` / `Save` |
| STEP `.step/.stp` (ISO 10303-21; entity subset, tested with AP214 files) | **yes, exact B-rep subset** | **yes**, exact B-rep, one solid per body | B-rep solid | declared unit honoured; override only when none is declared | `import_step`, `export_step`; UI Import…, Export STEP… |
| STL `.stl` | **yes**, ASCII + binary | **yes**, binary | triangle mesh (mesh body) | none in file: unit choice required; export writes mm | `import_stl`, `export_stl` (print options: docs/PRINT_EXPORT.md); UI Import…, Export STL… |
| DXF `.dxf` (ASCII) | **yes**, 2D sketch subset | **yes**, R12 LINE/ARC/CIRCLE | 2D sketch (unconstrained) | `$INSUNITS` honoured; choice required when absent | `import_dxf`, `export_dxf`; UI Import…, Export DXF (sketch)… |
| 3MF `.3mf` | **yes**, core subset → one mesh body per build item | **yes**, one object per body (B-rep tessellated, solid-checked; meshes as imported, solid-checked unless `allow_open_meshes`) | triangle meshes | declared unit honoured (default mm); export unit selectable (default mm) | `import_3mf`, `export_3mf`; UI Import…, Export 3MF… |
| SVG `.svg` | **yes**: lines, rects, circles, circular arcs, `M L H V Z A` paths → unconstrained sketch; Béziers/elliptical arcs/text/`use` reported | **yes**: sketch lines/arcs/circles | 2D sketch | root physical size + viewBox, else unit choice required; y flipped | `import_svg`, `export_svg`; UI Import…, Export SVG (sketch)… |
| IGES, OBJ, DWG, F3D/F3Z | no (roadmap) | no | | | |

"Reachable through" lists the typed command (`crates/rcad-commands`, also `rcad-cli run`).
The desktop app's Import… dialog (`crates/rcad/src/exchange.rs`) accepts `.step/.stp`,
`.stl`, `.3mf`, `.dxf` and `.svg`, asks for units only when needed (always for STL; for
STEP/DXF/SVG after the file declared none; never for 3MF) and a plane for DXF/SVG. The Export
menu has STL, STEP, 3MF (body selection), DXF and SVG (one sketch) dialogs (docs/UI.md rows
Import/Export). App tests (`crates/rcad/tests/exchange_ui.rs`) drive import of all five
formats and export of STEP, STL, DXF and 3MF; Export SVG… is not driven by an app test (the
command is covered by acceptance 3S).

Not supported, by design: renaming a file to another extension, a hidden external converter
(FreeCAD/OpenCascade is used only as an independent validator, never at runtime), mesh to
B-rep reconstruction, recovered feature history from STEP.

## Geometry contract per format

| | B-rep (exact) | Mesh | 2D sketch |
|---|---|---|---|
| STEP import | `ImportStep` base feature, one body per `MANIFOLD_SOLID_BREP` (per placed instance) | | |
| STEP export | each selected B-rep body as one `MANIFOLD_SOLID_BREP`; explicitly selected mesh bodies refused, default selection skips (and names) them | refused | |
| STL import | never (no reconstruction) | `ImportMesh` base feature, one mesh body per file | |
| STL export | tessellated (chord, optional `angle_deg`; closed/manifold/oriented/outward checked, nothing written otherwise) | written as imported if a valid solid, else refused unless `allow_open_meshes` (labelled reference mesh) | |
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

Implemented is not the same as tested. Entities that occur in the committed third-party
fixtures (`crates/rcad-kernel/tests/fixtures/step/*.step`): `PLANE`, `CYLINDRICAL_SURFACE`
(cylinder, plate_hole), `SPHERICAL_SURFACE` (sphere), `TOROIDAL_SURFACE` (torus), `LINE`,
`CIRCLE`, `ITEM_DEFINED_TRANSFORMATION` (two_boxes_assembly). Rational B-spline surfaces and
curves are read only from our own export, re-imported in
`step_import.rs::imported_box_can_be_cut_measured_and_exported` (not independent).
Implemented in `crates/rcad-kernel/src/step_in/geom.rs` but exercised by no fixture or test:
`CONICAL_SURFACE`, `ELLIPSE`, `SURFACE_CURVE`/`SEAM_CURVE`, and B-spline curves/surfaces
from any other writer (including the non-rational simple-entity forms).

Tested fixtures (all FreeCAD 1.1.4 / OpenCascade 7.8.1 generated, provenance in
`crates/rcad-kernel/tests/fixtures/step/README.md`), each through the command API (scenario C):

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

**Run 4 print export** ([`PRINT_EXPORT.md`](PRINT_EXPORT.md)): strict solid checks (closed edges,
manifold vertices, orientation, positive volume; self-intersection not checked), `angle_deg`,
export-only placement/layout, P1S/manual bed fit, `separate_files`, `allow_open_meshes`,
per-body report. Evidence: `crates/rcad-commands/tests/print_export.rs`,
`crates/rcad-kernel/tests/mesh_checks.rs`, `tools/print/validate_print_exports.sh` (Python,
FreeCAD, Bambu Studio 2.8.2.61 slicing; logs `artifacts/run4/print/validation/`).

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

Fixtures (`crates/rcad-exchange/tests/fixtures/dxf/`, provenance in
`tools/fixtures/dxf/README.md`) are FreeCAD-written (`slot_freecad_r14.dxf`,
`notch_freecad_r12.dxf`) or hand-written from the public group-code reference; ezdxf 1.4.2 is
used only as a second reader of our exports and the fixtures (`ezdxf_check.py`), next to
FreeCAD's DXF importer (`validate_export.sh`).

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

## 3MF

`rcad_exchange::threemf::{read, write}` ([`formats/threemf.md`](formats/threemf.md)), wired by
the coordinator as `import_3mf` (base feature `Import3mf`, source embedded, one mesh body per
build item, display/export only) and `export_3mf` (`bodies` default all visible, `unit`,
`chord`, run-4 print options: [`PRINT_EXPORT.md`](PRINT_EXPORT.md); geometry only, no slicer settings). Command-level evidence: `crates/rcad-commands/tests/bodies.rs::threemf_import_and_multi_body_export_round_trip`
(FreeCAD two-box fixture imported; three-body export in inch re-imported with exact
volumes). UI: Import… and Export 3MF… (`exchange_ui.rs::threemf_import_export_and_svg_sketch`
imports the FreeCAD two-box fixture without a unit prompt and exports all visible bodies).
Read: ZIP stored/deflate, `_rels/.rels` start part, units micron..meter converted to mm, one
mesh per build item with item and nested component transforms (mirrors reverse winding),
object-level base-material colours; per-triangle colours, colour groups, textures and other
extension content are reported as warnings, required extensions other than materials and any
production-extension reference are refused; bounded archive/decompression/vertex/triangle/
item/depth/output limits; DOCTYPE refused. Write: one object + build item per mesh, welded
vertices, chosen unit, optional object colours. Evidence: FreeCAD-written two-object file and
hand-written packages (inch + rotated item, components + mirror, colours, malformed,
decompression bomb, lying size header); writer read back by FreeCAD and an independent Python
reader. FreeCAD 1.1.4's 3MF reader ignores the `unit` attribute and does not flatten
components (observed, `tools/fixtures/threemf/README.md`), so units/components are checked by
the Python reader (`check_3mf.py`) instead.

## SVG

`rcad_exchange::svg` ([`formats/svg.md`](formats/svg.md)), commands `import_svg` (new
unconstrained sketch on a plane or planar face) and `export_svg` (one sketch). Import:
`line`, `rect` (not rounded), `circle`/`ellipse` with `rx ≈ ry`, `polyline`/`polygon`, `path`
`M L H V Z` and circular `A`; `g` transforms. Accepted to 1e-5 relative: `rx`/`ry` that differ
by rounding (mean radius) and near-similarity transforms on circles/arcs. Units: root
`width`/`height` (> 0, `mm cm in pt pc`, exponents such as `1e2mm`) with `viewBox`;
`preserveAspectRatio` any alignment with `meet` (smaller ratio) or `slice` (larger ratio),
`none` only when both ratios agree; a missing/`auto` width or height follows the other through
the viewBox aspect ratio; no physical size → unit choice required. Refused: a non-identity
`transform` on the root `svg` (it applies in the embedding page's CSS space), DOCTYPE with an
internal subset, data after `Z` in a path. Unsupported and reported (import refused unless
`allow_partial`): Béziers, elliptical arcs, `use`, `text`, `image`, nested `svg`, circles/arcs
under a non-similarity transform, and shapes whose geometry uses relative lengths (`% em ex rem ch vw vh vmin vmax auto`); such a length
skips only its own element. Ignored silently: `defs`, metadata, styles and any element in a
non-SVG namespace (Inkscape `sodipodi:`/`inkscape:`, RDF). Export: lines, arcs, circles in
local sketch coordinates, mm, y negated; construction geometry and constraints are not
written.

Evidence: `crates/rcad-exchange/tests/svg_fixtures.rs` (15 tests: hand-written `plate_mm.svg`,
FreeCAD-generated `freecad_plate.svg`, the review fixes above); acceptance 3S (FreeCAD SVG →
sketch → 5 mm solid, own SVG export re-imported, STEP export); `tools/check_run3_steps.sh`
(FreeCAD validates the 3S STEP and reads the exported SVG with `importSVG`: total edge length
220 + 20π, 60 × 40); UI import in `exchange_ui.rs::threemf_import_export_and_svg_sketch`.

**Known losses**: Béziers, elliptical arcs, text, styling (fill/stroke are not read; export is
stroke only), design intent (constraints); sketch placement on export.

## Native format

**v5 (run 5)** = v4 plus feature suppression (`"suppressed": true`), face references to
pattern/mirror occurrences and chamfer faces (`FaceRole::Copy`, `FaceRole::Chamfer`), the
`TangentArcs` sketch constraint (line–arc sketch fillets), solid `Fillet`, `Loft` and `Sweep`
features, angled construction planes (`Offset.angle`), patterns of count 1 and bound
`plane_offset` on mirrors/holes. All additive. A document is written as
v5 only when it uses one of them; otherwise it stays v4, byte for byte as a run-4 save, so run-4
builds keep reading it, and they refuse a v5 file instead of evaluating suppressed features or
misreading references. A file labelled v4 that contains a v5 feature is refused. Frozen v4
documents (the examples at 8f2b797) are in `crates/rcad-commands/tests/fixtures/native_v4/`
(`suppression.rs::frozen_v4_fixtures_load_unchanged`: load, regenerate to the cached volume,
re-save as an identical v4 document).

**v6 (run 6)** = v5 plus extrusion options (`"options"` on an `Extrude`: symmetric, two
sides, start offset, up to a plane, thin, taper, further regions) and the `Intersect`
operation (`rcad_document::io::v6_feature` lists every run-6 addition). Written only when used,
otherwise v5/v4 as before; a file labelled v5 or older that contains a v6 feature is refused
(a run-5 build would silently ignore the unknown fields and build a different solid). Frozen
v5 documents (saved by run-6's build before the change, with run-5 features only) are in
`crates/rcad-commands/tests/fixtures/native_v5/` (`native_formats.rs::v5_run5_documents`:
open, analytic volume, re-save as an identical v5 document).

**Name.** Files say `"format": "r-cad"` and are named `*.rcad.json`. Files written before
the rename to R-CAD (2026-10-13) say `"format": "fission420"` and are usually named
`*.f420.json`. They load unchanged; no version changed with the name, and the extension is
never checked. Evidence: `crates/rcad-document/tests/legacy_name.rs`, plus every frozen
fixture under `crates/rcad-commands/tests/fixtures/native_v*/`, whose contents predate the
rename. See docs/RENAME.md.

**v4 (run 4)** = v3 plus named user parameters and feature expression bindings
(docs/PARAMETERS.md); v3 files load unchanged (frozen copies of the run-3 documents in
`crates/rcad-commands/tests/fixtures/native_v3/`, `native_formats.rs::v3_run3_examples`), saves
write v4, and an older build refuses a v4 file instead of dropping expressions.

v3 stores features, bodies (id, name, visibility, source) and the bytes of every imported
file as base64 blobs, so a document reopens after the source file is moved or deleted
(scenario C.3, D.6, D.11 delete the sources before reopening and compare the whole document
and every body). v1 and v2 files migrate on load (first solid feature gets body B1; feature
ids unchanged); newer versions are refused. Evidence: `native_formats.rs` opens the v1
fixture, frozen copies of the five run-2 v2 documents
(`crates/rcad-commands/tests/fixtures/native_v2/`; the committed `examples/*.rcad.json` are
re-saved as v3 whenever their workflows replay, e.g. in `cargo xtask check`), and every
committed example, checks the analytic volume, re-saves as v3 and reopens to an identical
document. Size limit 256 MiB per file, 128 MiB per import source.

## Evidence

Reproduce (from the repository root):

```sh
cargo run --release -p rcad-cli -- acceptance --out artifacts/acceptance   # scenarios C and D are the "C." / "D." steps
tools/external_validation.sh artifacts/acceptance                          # FreeCAD checks incl. tools/check_formats.sh
tools/check_run3_steps.sh                                                  # run-3 STEP exports incl. 3S + exported SVG via FreeCAD
cargo test --release -p rcad-commands --test imports --test native_formats --test bodies
cargo test --release -p rcad-kernel --test step_import --test step_export_multi
cargo test --release -p rcad-exchange
cargo test --release -p rcad --test exchange_ui
tools/check_step_multi.sh; tools/fixtures/stl/validate_writer.sh; tools/fixtures/dxf/validate_export.sh
tools/fixtures/threemf/validate_writer.sh
```

Test results below are from `cargo xtask check` (log `artifacts/run3/verify/xtask.log`).

| Evidence | Where | Result (2026-10-11) |
|---|---|---|
| Acceptance C.1-C.8, D.1-D.11, 3S | `crates/rcad-commands/src/acceptance/formats.rs`, `bodies.rs` (`scenario_svg`); reports `artifacts/acceptance/acceptance_report.json` (written by `cargo xtask check`) and `artifacts/run3/acceptance/acceptance_report.json` (a separate run, same results) | all checks PASS (855 total, all scenarios; C 120, D 90, 3S 10) |
| FreeCAD validation of C/D exports | `tools/check_formats.sh` (called by `tools/external_validation.sh`); per-file results `artifacts/acceptance/{scenario_c,scenario_d_slot}_step_check_freecad.txt`, `scenario_d_mesh_stl_check_*.{json,txt}`, `scenario_d_slot_dxf_check_freecad.txt`, `formats_check.txt`; log `artifacts/run3/verify/external_validation.log` | PASS: scenario_c 1 valid solid, V err 2.1e-7 mm³, bbox 40×30×10, hole loops at z 0/10 centred (20,15) area 9π, point probes; scenario_d_slot V err −2.9e-8; mesh STL V 1000; DXF area 800 + 50π |
| FreeCAD validation of 3S (SVG) | `tools/check_run3_steps.sh`; log `artifacts/run3/verify/check_run3_steps.log` | PASS: `svg_plate.step` V err 4.6e-7, bounds, probes; `svg_plate.svg` via `importSVG` length 220 + 20π, 60 × 40 |
| Command-level import edge cases | `crates/rcad-commands/tests/imports.rs`; 3MF: `bodies.rs::threemf_import_and_multi_body_export_round_trip` | 12 + 8 pass |
| UI import/export dialogs | `crates/rcad/tests/exchange_ui.rs` | 6 pass |
| Native v1/v2/v3 | `crates/rcad-commands/tests/native_formats.rs` (v1 fixture, 5 frozen v2 documents, 7 examples) | 4 pass |
| Kernel STEP import/export | `crates/rcad-kernel/tests/step_import.rs`, `step_export_multi.rs` | 13 + 4 pass (1 ignored: artifact writer for `tools/check_step_multi.sh`) |
| STL/DXF adapters | `crates/rcad-exchange/tests/stl_fixtures.rs`, `dxf_fixtures.rs`, `dxf_scale.rs` + unit tests in `src/{stl,mesh,dxf}`; validators `tools/fixtures/stl/validate_writer.sh`, `tools/fixtures/dxf/validate_export.sh`, logs `artifacts/run3/verify/stl_validate.log`, `artifacts/run3/verify/dxf_validate.log` | 7 + 13 + 2 pass |
| 3MF adapter | `crates/rcad-exchange/tests/threemf_fixtures.rs` + unit tests in `threemf.rs`/`zip.rs`; writer: `tools/fixtures/threemf/validate_writer.sh`, log `artifacts/run3/verify/threemf_validate.log` | 9 + 6 pass; writer checked by the Python stdlib reader and FreeCAD |
| SVG adapter | `crates/rcad-exchange/tests/svg_fixtures.rs` | 15 pass |
| Replayable workflows | `examples/imported_step_hole_workflow.json`, `examples/dxf_stl_import_workflow.json` (`rcad-cli run`, from the repository root) with saved documents `examples/*.rcad.json` | replayed by `imports.rs` |

`artifacts/` is not versioned; rerun the commands above to regenerate the logs.

## Known issues found by these checks

* Fixed after these checks (coordinator, run 3): `ExportStep` without `bodies` now writes the
  visible B-rep bodies and names skipped mesh bodies in its message
  (`imports.rs::export_step_default_skips_mesh_bodies_as_documented`); a blind cut sketched on
  a face now goes into the material by default and a reversed one is refused with "points away
  from the material; clear 'reverse'"
  (`imports.rs::blind_cut_from_face_goes_into_material_and_reverse_is_explained`).
* A face-sketched contour whose edge is coplanar with a side face of the target (e.g. the
  40 mm slot on the 40 mm wide box top) makes the Truck boolean fail; the command returns a
  clean error and keeps the model.
