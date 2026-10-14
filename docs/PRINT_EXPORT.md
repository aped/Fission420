# Print export (STL / 3MF)

Status: command API (`export_stl`, `export_3mf` with optional print fields), kernel mesh
checks, independent validators, Bambu Studio slicing and the desktop Export STL / Export 3MF
dialogs (see "Desktop dialogs") are implemented and tested. Nothing here has been
physically printed: "sliced" means Bambu Studio produced G-code whose toolpaths were checked.

Code: `crates/rcad-commands/src/print.rs` (selection, tessellation, checks, placement, bed fit,
report, atomic writes), `crates/rcad-kernel/src/mesh.rs` (`check_mesh`, `MeshCheck`,
`weld_exact`, `max_facet_angle_deg`, `RigidTransform`). Tests:
`crates/rcad-commands/tests/print_export.rs`, `crates/rcad-kernel/tests/mesh_checks.rs`.

## Units

* **STL has no unit field.** Coordinates are written in millimetres (the header says
  `units=mm`, which slicers ignore). The slicer must interpret the file as mm; Bambu Studio,
  PrusaSlicer and Orca do by default. The response's `export.units` states this.
* **STL import** (the other direction): the Import dialog shows "Interpreted as mm: 1 file
  unit = 1 mm" for the chosen unit (mm default; cm, m, inch), and the import notes repeat the
  scale. A mesh imported in the wrong unit can be converted afterwards with the mesh
  transform presets inch → mm / m → mm (docs/MESH.md).
* **3MF declares its unit** in the model's `unit` attribute (`millimeter` by default; `unit`
  selects micron…meter and coordinates are converted). Reported dimensions are always mm.
* **A geometry-only 3MF carries no Bambu printer, process or filament settings.** It holds mesh
  objects (one per body, named after the body) and build items; Bambu Studio applies whatever
  profiles are selected when it is opened or sliced.

## Selection and hidden bodies

`bodies` absent: every **visible** body. Hidden bodies are not exported and are named in
`export.selection` / the message (`default: all visible bodies; hidden not exported: 'Lid'`).
An explicit `bodies` list exports exactly those bodies, hidden ones included, and says so
(`explicit selection, includes hidden: 'Lid'`). Bodies are never fused: STL writes one shell per
body (or one file per body with `separate_files`), 3MF one object per body.

Every export reads the session's committed model; `export.revision` names the revision. A
session executes commands sequentially, so geometry, checks and report come from one revision.

## Tessellation

* `chord` (mm, default 0.01 = `tol::MESH_CHORD`): Truck's chordal tolerance. Measured on an
  exported Ø20 × 10 cylinder (independent reader, `print_export.rs::cylinder_tessellation_accuracy_from_exported_bytes`,
  numbers in `artifacts/print/tests/cylinder/accuracy.json`):

  | chord | segments (min for sagitta) | max sagitta (Python checker) | volume (exact 3141.593) | triangles | STL bytes |
  |---|---|---|---|---|---|
  | 0.02 | 84 (50) | 0.01749 | 3138.161 | 5354 | 267 784 |
  | 0.005 | 168 (100) | 0.004495 | 3140.734 | 21452 | 1 072 684 |

  (sagitta and volume from `artifacts/print/validation/mesh_check.txt` /
  `exports/workflow.jsonl`; the Rust test asserts the same bounds on its own export.)

  Vertices lie on the circle within f32 rounding (≤ 2e-5 mm); every lateral edge's sagitta is
  ≤ chord; the volume lies in [π(r−s)²h, πr²h]. Triangle growth: Truck meshes the lateral face
  on an isotropic parameter grid (also subdividing along the straight rulings), so the count is
  ~ 1/chord rather than ~ 1/√chord: 4× tighter → 4× triangles (asserted ≤ 6×, and ≤ 8·N_min²).
  Planar faces are exact at any chord.
* `angle_deg` (optional, 0 < a < 90): maximum angle between the normals of adjacent triangles
  of the same B-rep face (`max_facet_angle_deg`; model edges between different faces excluded).
  Enforced by measurement: the body is tessellated, the angle measured, and the chord reduced
  (by (a/measured)² · 0.9, at most 8 times) until it holds. Refused when it would need a chord
  below the kernel minimum (`max(1e-3, 1e-5 · diagonal)` mm) or exceed 20 M triangles. The
  chord actually used and the measured angle are reported per body.
* Mesh bodies (imported STL/3MF) are written as imported; chord/angle do not apply.

## Mesh checks (`MeshCheck`, every exported body)

Performed (topology by vertex index; imported meshes are first welded on exact coordinates):

| field | meaning |
|---|---|
| `closed_edges`, `boundary_edges`, `nonmanifold_edges` | every edge used by exactly two triangles |
| `orientation_consistent`, `misoriented_edges` | the two uses of every edge run in opposite directions |
| `nonmanifold_vertices` | triangles around each vertex form one edge-connected fan (catches shells touching at a point) |
| `outward`, `mesh_volume` | signed volume > 0 (outward winding) |
| `invalid_triangles` | repeated or out-of-range vertex index |
| `degenerate_triangles` | zero area (`|ab × ac| ≤ 1e-12 · longest²`); warning only |
| `shells` | edge-connected components |
| `self_intersection` | always `"not checked"`: no triangle-triangle test is done. FreeCAD's `hasSelfIntersections` is run on the exports by `tools/print/validate_print_exports.sh` as an external check |

Not checked: self-intersection (in the app), minimum wall thickness, overhangs, printability.
A valid closed mesh can still be the wrong shape; the acceptance checks compare volumes,
bounds and slicer toolpaths against analytic values for that reason.

**Strict policy.** A B-rep tessellation must be a valid solid (`is_valid_solid`: closed,
consistently and outwardly oriented, no non-manifold vertex, no invalid triangle) or the export
is refused with the problems listed and **nothing written** (`response.mesh_check` holds the
failing check). An imported mesh body must also be a valid solid, unless `allow_open_meshes` is
set: then it is written and labelled `open reference mesh (not a printable solid)` with a
warning. Note from the slicer runs: Bambu Studio 2.8.2.61 silently slices a mesh with flipped
triangles (no repair message), so these checks are the only guard against such files.

## Placement (export only)

`placement` absent: document coordinates are written unchanged. With `placement`:

1. Orientation per body: `"as_modeled"` (default) or `{"face_down": FaceRef}`: the proper
   rotation (det +1, Rodrigues, shortest arc; a half turn about X for a +Z normal) taking that
   planar face's outward normal to −Z. The face must belong to the body, be referenceable in the
   current model, and be a lowest face after rotation (else refused, naming how far the part
   extends below it).
2. Translate so the body's lowest point is z = 0.
3. XY: without `layout` the XY centre of the body's bounding box is kept; with
   `layout: {gap}` bodies are placed side by side along +X in export order with `gap` mm between
   bounding boxes, the group centred on the bed centre (or the origin without a bed).

The transform is reported per body (`transform.rotation` row-major, `translation`). Placement
never edits features, parameters or the document revision (tested: summary before/after equal).
A proper rotation plus translation preserves volume, distances and handedness (tested on
pairwise distances and volume of a placed cylinder; reflections are refused by construction).

### 3MF build-item transforms (`item_transforms`)

3MF only (refused for STL). With `placement`, each object is written in its **modeled**
coordinates and the placement (proper rotation + translation) becomes its build item's
`transform` attribute (3MF core row-vector convention, translation in the file's unit), so a
slicer still sees one movable object per body with its own name and transform. Without
`placement` no transform is written. Readers apply the transform; the geometry is the same as
the baked export. Desktop dialog: "Placement as 3MF item transforms" (shown for 3MF when
"Place on bed" is on).

Evidence (`crates/rcad-commands/tests/print_item_transforms.rs`, files under
`artifacts/print/tests/item_transforms/`): the placed enclosure (lid face down, X layout,
gap 10, P1S bed) in mm and inch, with and without item transforms, read back with the 3MF
reader: both objects closed, on z = 0, at x = 43 / 133 (analytic: 170 mm row centred on
128), y centred on 128, dims 80×50×25 / 80×50×5, volumes 19592 / 9404 mm³.
Independent: `tools/print/check_print_mesh.py` (Python zipfile + XML, applies unit and item
transforms itself) on all four files: 8 PASS, 0 FAIL, same bounds and volumes; it shows the
lid's item transform `[1 0 0 0 -1 0 0 0 -1 …]` (half turn about X). Bambu Studio 2.8.2.61:
the **mm** item-transform file slices (exit 0, 125 layers, object heights 25 / 5) and passes
all 17 enclosure baseline toolpath probes (`tools/print/probes/baseline_plate.json`), as does
a baked **inch** file. **Finding:** an **inch** file *with* item transforms is refused by
Bambu Studio ("no object fully inside the print volume", exit 206) although the Python
checker and our reader place it correctly per the core spec (transforms in model units);
the export therefore warns for item transforms in a non-mm unit.

## Bed and fit

`bed`: `{"preset": "bambu_p1s"}` or `{"width": W, "depth": D, "height": H}` (mm, 0 < v ≤ 10000,
area origin at (0, 0)). Preset values come from the installed Bambu Studio profiles, not from
memory or marketing material:

| value | source file (Bambu Studio 2.8.2.61 flatpak, `share/BambuStudio/profiles/BBL/machine/`) |
|---|---|
| printable_area `0x0,256x0,256x256,0x256` | `fdm_bbl_3dp_001_common.json` (inherited by `Bambu Lab P1S 0.4 nozzle.json`) |
| printable_height `250` | `fdm_machine_common.json` (root of the chain) |
| bed_exclude_area `0x0,18x0,18x28,0x28` | `Bambu Lab P1S 0.4 nozzle.json` |

Flatpak location: `/var/lib/flatpak/app/com.bambulab.BambuStudio/x86_64/stable/68a634d4d733ce7acf79009095db6f7ca76625f0eb810eef1e92dc7adeba7852/files`.
Full chain and per-key provenance: `artifacts/print/slicer/profiles.md`,
`tools/print/bbl_profile_flatten.py`.

Per body, `fit.fits_size` compares the exported dimensions with the printable volume;
`fit.within_bed` (placed exports only) checks the placed bounds against the area, the height
and the excluded areas (bounding-box overlap, conservative). `layout_fit` checks the whole
group. A misfit is a warning in the message, not a refusal: geometry is never scaled or moved
to make it fit.

## Files and atomicity

STL `separate_files`: one file per body, `<stem>-<body>.stl` next to `path` (ASCII letters,
digits, `-`, `_` kept; other runs → `_`; at most 64 characters; a case-insensitive collision
gets `-<body id>`). All bytes are produced and all checks pass before the first write; each
file is written atomically (`write_atomic`: temp file, fsync, rename); if a later file fails,
files newly created by this export are removed. A refused export writes nothing.
`export.files` lists path, byte size and bodies of each file.

## Deterministic output (2026-10-12)

The same document exported (or saved) twice gives the same bytes, also across processes,
although Truck orders topology by pointer-derived ids and carries ~1e-15 mm of noise that
differs between regenerations. `crates/rcad-commands/tests/export_determinism.rs` builds the
three printables twice and compares STL, 3MF, STEP and `.rcad.json` byte for byte.
* Tessellation: faces visited in a canonical geometric order, loops started at a canonical
  edge, planar faces re-triangulated to their constrained 3-D Delaunay triangulation by edge
  flips, with exact ties (a rectangle's two diagonals) broken by vertex key
  (`mesh.rs::canonical_planar_diagonals`).
* Print exports: `canonical_mesh` (vertices and triangles sorted by 1e-6 mm keys); STL facet
  normals from the written f32 positions; 3MF coordinates at a fixed 1e-9 mm resolution.
* STEP: canonical entity order, planar faces as forward faces on a canonically placed plane,
  REALs rounded to 1e-9. The header time stamp is `SOURCE_DATE_EPOCH` when set, otherwise the
  export time (ISO 8601 UTC, seconds): it is the only line that can differ.
* Native file: the informational `cache` (volume, bounds) is rounded to 1e-9.
A value lying within ~1e-14 of a rounding boundary can still flip; the odds are about 1e-5
per coordinate that differs between regenerations at all.

## Response (`Response.export`, `ExportReport`)

`format`, `units`, `revision`, `chord`, `angle_deg`, `selection`, `placement`, `bodies[]`
(`id`, `name`, `kind` `solid`|`mesh`, `visible`, `label`, `triangles`, `chord`,
`max_facet_angle_deg`, `transform`, `bounds_min`, `bounds_max`, `dims`, `mesh_check`, `fit`,
`file`), `files[]`, `bed`, `layout_fit`, `warnings`. The message repeats revision, selection
policy, placement and units.

## Independent validation and slicing

* `tools/print/check_print_mesh.py` (Python stdlib, run with `python3 -I`; no project code):
  binary/ASCII STL and 3MF (OPC rels, unit, objects, components, build transforms), per object
  bounds/dims, signed volume, edge incidence, orientation, manifold vertices, degenerate
  triangles, min z, bed bounds, circle sagitta/vertex error; expected values from a JSON spec.
  Self-tests: `tools/print/make_test_meshes.py`.
* `tools/print/freecad_check_print.py`: FreeCAD 1.1.4 Mesh (isSolid, non-manifolds,
  self-intersections, volume, bounds). FreeCAD's 3MF reader merges objects and ignores the
  unit attribute, so it only cross-checks mm files as a whole.
* `tools/print/slice_bambu.sh`: Bambu Studio 2.8.2.61 CLI, headless, `--orient 0 --arrange 0`
  (positions kept), machine `Bambu Lab P1S 0.4 nozzle`, process `0.20mm Standard @BBL X1C`
  (compatible with P1S 0.4; the P1S default), filament `Generic PLA`, flattened from the
  flatpak's BBL profiles into the output dir; isolated `--datadir`; never sends to a printer.
* `tools/print/analyze_gcode.py`: layer count, objects, extrusion probes per layer
  (`extrusion_near`, `no_extrusion_in_rect`, `filled_rect`), slicer-log and mesh_stat repair
  counters.
* `tools/print/validate_print_exports.sh <rcad-cli>`: runs
  `tools/print/enclosure_print_workflow.json` (an export fixture built with plain commands,
  baseline and variant sizes entered as numbers, not the named-parameter example),
  then all of the above with `tools/print/enclosure_spec.json` and
  `tools/print/probes/{baseline,variant}_plate.json`.

### Results (2026-10-11, commit of this doc; `artifacts/print/validate.log`)

Exports from `enclosure_print_workflow.json` (built with `--release`,
binary copied to `artifacts/print/bin/rcad-cli`):

* Python checker: 91 PASS, 0 FAIL (`validation/mesh_check.txt`): unit `millimeter`, 2 named
  objects per 3MF, all objects closed/manifold/outward, volumes 19592 / 9404 (baseline) and
  36823.296 / 15148.48 (variant) within 0.5 mm³, dims 80×50×25 / 80×50×5 and 100×65×30 /
  100×65×5 within 1e-4 mm, min z = 0, inside the P1S area; cylinder sagitta within chord.
* FreeCAD 1.1.4: every STL `isSolid`, no non-manifolds, **no self-intersections**, volumes
  as above (`validation/freecad.txt`). The merged as-modeled 3MF reports a self-intersection
  because base and lid overlap in model space and FreeCAD merges 3MF objects (informational).
* Bambu Studio 2.8.2.61 (P1S 0.4 / 0.20mm Standard @BBL X1C / Generic PLA, `--arrange 0`):
  baseline and variant, each as 3MF and as two STLs, slice with exit 0; `mesh_stat` repair
  counters all 0 (no edges fixed, facets reversed/removed, degenerate facets); no non-benign
  log findings; objects kept at the exported positions (plate bbox equals ours). 125 / 150
  layers. Toolpath probes (17 baseline, 15 variant, all pass): extrusion on all four base
  walls at mid-height, no extrusion inside the cavity above the floor, floor footprint fully
  covered, lid plate covered, all four lip walls present at z 3.5, lip opening empty, and the
  clearance step outside the lip empty (`validation/slice_*/analysis.json`).

### Real printable examples (`tools/print/validate_examples.sh`, 2026-10-11)

`crates/rcad-commands/tests/print_examples.rs` (ignored test, run by the driver) replays
`printables::{enclosure,coupon,bracket}_workflow` (baseline, then `ENCLOSURE_VARIANT`
parameters), exports placed STLs, multi-object 3MFs (lid face down on its plate top, lip up;
base on its floor; X layout, gap 10, P1S bed) and per-body STEP, and writes the expected
values from the analytic formulas (never from the export report). Results
(`artifacts/print/examples/SUMMARY.txt`):

* Python: 124 PASS, 0 FAIL. Mesh volumes: base 19592.000000 / lid 9404.000000 (baseline),
  36823.296000 / 15148.480000 (variant), coupon plate 5053.577 (range 5053.64 ± 2.16 from the
  chord), peg 281.920 (281.80 ± 1.44), bracket 20246.219 (20246.6 ± 2.22).
* FreeCAD STEP: each one valid solid; volume vs analytic: enclosure exact to ≤ 2.6e-13
  relative, coupon plate 5051.980578 (8e-11), peg 282.743339 (7e-10), bracket 20244.834111
  (1.7e-11). FreeCAD STL: all solid, no non-manifolds, no self-intersections.
* Bambu Studio: all slices exit 0, repair counters 0; layers 125 / 150 / 50 / 200; probes
  enclosure 17 + 17 (3MF, STL) baseline and 15 variant, coupon 14, bracket 13, all pass
  (cavity empty, walls/floor/lip present, lip opening and clearance step empty, coupon holes
  and bracket holes empty). Finding: slicing the bracket intermittently logs
  `[error] ZFiller: encounter idx from clip: 16/22` with exit 0, no repairs and passing probes.
  Investigated 2026-10-12: the message is a log-only check in Bambu Studio's
  `src/libslic3r/Fill/FillFloatingConcentric.cpp` (after clipping a concentric line against
  the floating, unsupported area; the path is still used). The **same 3MF bytes** sliced six
  times logged it once. Excluding `M73` progress lines: three runs are identical, one differs
  only in its header filament totals, and two (the one that logged it and one that did not)
  differ in toolpaths at a few layers, mostly "Floating vertical shell" and sparse infill. So
  the slicer's output varies on identical input; the message is a symptom of that, not of the
  input geometry.
  `analyze_gcode.py` classifies exactly this message as benign; other `ZFiller` messages
  remain findings.

This is CAD/mesh/slicer validation only: nothing has been physically printed, and slicing
says nothing about fit, strength or print quality.

## Desktop dialogs (`crates/rcad/src/exchange.rs`, `PrintForm`)

Export STL… / Export 3MF… show: path; units (STL: "coordinates in mm; the slicer must interpret
mm"; 3MF: the declared unit, geometry only); chord (default 0.01 mm) and optional max facet
angle; the body selection (all visible / selected, hidden-body policy stated with names);
a per-body table (solid / mesh closed|OPEN, modeled size, orientation "As modeled" or
"Selected face down" using the face currently selected in the viewport, enabled only for its
own body); "Place on bed (export only)", "Lay out side by side along X" + gap; bed None / Bambu
Lab P1S / Manual; "One STL file per body"; "Allow open reference meshes". The dialog stays open
after OK and shows the export's per-body report (triangles, size, mesh checks, fit, files,
warnings) or the refusal. Test: `crates/rcad/tests/print_export_ui.rs` (builds
`printables::enclosure_workflow`, selects the lid plate's top face, exports a placed 3MF of
both bodies on the P1S bed and re-reads it: lid min z 0, lip up with the lip's outer size,
base on its floor, 10 mm gap, analytic volumes, document unchanged).

## Not done / limits

* App dialogs: no live pre-export preview of placed sizes/triangle counts (modeled sizes are
  shown before export, the exact per-body report after it); one selected face at a time is
  used for "Selected face down".
* Minimum-thickness checks are not implemented. Self-intersection: the export check does
  not run it; mesh bodies can be checked on demand in the inspector (`inspect_mesh`,
  docs/MESH.md), bounded and not a proof.
* Truck's isotropic lateral-face grid makes curved parts heavier than necessary (see above).
* Layout is a single row along X; no rotation about Z, no packing; it may not fit even when a
  different arrangement would (reported, not fixed).
* The P1S preset is a constant recorded from the profile files; `tools/print/bbl_profile_flatten.py`
  re-derives it from an installed Bambu Studio, but the app does not read the profile at runtime.
