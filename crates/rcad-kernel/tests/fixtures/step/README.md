# STEP fixtures

Independent STEP AP214 (`AUTOMOTIVE_DESIGN`) files for import tests. These
were made outside R-CAD, so they check the importer against another
kernel's output rather than against our own writer.

## Provenance

- Generated 2026-10-10 with FreeCAD 1.1.4 (build 45039, git
  `4fd3bf320d9566a27e60069fc8387448aaa3a094`), flatpak `org.freecad.FreeCAD`,
  OpenCascade 7.8.1 (`Part.OCC_VERSION`). The header says
  `Open CASCADE STEP processor 7.8`.
- Regenerate with `tools/fixtures/step/generate.sh`. It runs:
  1. `tools/fixtures/step/generate.py` in FreeCADCmd (`RCAD_MODE=generate`)
     to build the shapes from `Part` primitives and export them;
  2. `tools/fixtures/step/make_units.py` (python3) to write the inch and
     metre box conversions;
  3. `tools/fixtures/step/make_derived.py` (python3) to write the malformed
     files;
  4. `generate.py` again (`RCAD_MODE=verify`) to re-import every file and
     write `verification.json`.
- The FreeCAD invocation, with no lasting change to flatpak permissions:
  `flatpak run --filesystem=<repo> --env=RCAD_OUT=<out dir> --env=RCAD_MODE=generate|verify|all [--env=RCAD_RESULTS=<json>] --command=FreeCADCmd org.freecad.FreeCAD tools/fixtures/step/generate.py`
- Headers are sanitized. `FILE_NAME` has no host paths. Its timestamp
  changes on every run, so regenerating does not reproduce the files byte
  for byte. The DATA sections are deterministic.
- All files are in millimetres unless the name says otherwise. All are
  under 16 KB.

## Files and expected values

| File | Built from | Expected (analytic) |
|---|---|---|
| `box_40x30x10_mm.step` | `Part.makeBox(40,30,10)` | [0,40]x[0,30]x[0,10], V = 12000 |
| `cylinder_d20_h15_mm.step` | `Part.makeCylinder(10,15)`, axis +Z at origin | [-10,10]^2 x [0,15], V = 1500*pi = 4712.388980384690 |
| `sphere_r10_mm.step` | `Part.makeSphere(10, Vector(5,5,5))`, **centred at (5,5,5)** to catch placement bugs | [-5,15]^3, V = 4000*pi/3 = 4188.790204786391 |
| `torus_R20_r5_mm.step` | `Part.makeTorus(20,5)`, axis +Z at origin | [-25,25]^2 x [-5,5], V = 2*pi^2*20*25 = 9869.604401089358 |
| `plate_hole_mm.step` | `makeBox(60,40,8).cut(makeCylinder(5,12,(30,20,-2)))` | [0,60]x[0,40]x[0,8], V = 19200 - 200*pi = 18571.68146928204; 6 planes + 1 cylinder face |
| `two_boxes_compound_mm.step` | `Part.Compound([A, B])` via `Shape.exportStep` | A = box 10x10x10 at origin; B = box 20x10x5 with Placement translate (50,0,0) and rotate 30 deg about Z; V = 1000 each; union bbox [0,67.320508]x[0,18.660254]x[0,10]; B bbox [45,67.320508]x[0,18.660254]x[0,5] |
| `two_boxes_assembly_mm.step` | two `Part::Feature` objects (BoxA, BoxB with `obj.Placement`) via `Import.export` (OCAF) | same as above |
| `box_40x30x10_inch.step` | `make_units.py`: mm box with every length divided by 25.4, unit set to `CONVERSION_BASED_UNIT('INCH')` | same box as the mm file: 40x30x10 mm, V = 12000 mm^3 |
| `box_40x30x10_m.step` | `make_units.py`: lengths divided by 1000, unit `SI_UNIT($,.METRE.)` | same box: 40x30x10 mm, V = 12000 mm^3 |
| `freecad_relabel_inch.step` | `Part.exportUnits('INCH')` + `exportStep` of the 40x30x10 box | coordinates 40/30/10 in **inch**: 1016x762x254 mm, V = 196644768 mm^3 |
| `freecad_relabel_m.step` | `Part.exportUnits('M')` + `exportStep` | 40x30x10 **m**: V = 1.2e13 mm^3 |
| `truncated.step` | first 60% of the bytes of `box_40x30x10_mm.step` | must be rejected (it ends at `#106 = `, with no `ENDSEC`) |
| `garbage.step` | 5 lines of non-STEP text | must be rejected |
| `missing_unit.step` | mm box with the length unit removed (see below) | no `LENGTH_UNIT` is declared; how the importer handles it is a policy decision |

## FreeCAD re-import results (`verification.json`)

`Part.Shape().read()` followed by `isValid()`, `Volume` and
`optimalBoundingBox()`:

| File | Solids | Faces | Valid | Volume | BBox (min; max) | Surfaces |
|---|---|---|---|---|---|---|
| box_40x30x10_mm | 1 | 6 | yes | 12000.0 | 0,0,0; 40,30,10 | Plane |
| box_40x30x10_inch | 1 | 6 | yes | 12000.0 | 0,0,0; 40,30,10 | Plane |
| box_40x30x10_m | 1 | 6 | yes | 12000.0 | 0,0,0; 40,30,10 | Plane |
| cylinder_d20_h15_mm | 1 | 3 | yes | 4712.38898038469 | -10,-10,0; 10,10,15 | Cylinder, Plane |
| sphere_r10_mm | 1 | 1 | yes | 4188.790204786392 | -5,-5,-5; 15,15,15 | Sphere |
| torus_R20_r5_mm | 1 | 1 | yes | 9869.604401089357 | -25,-25,-5; 25,25,5 (+1e-7 tolerance) | Toroid |
| plate_hole_mm | 1 | 7 | yes | 18571.68146928204 | 0,0,0; 60,40,8 | Cylinder, Plane |
| two_boxes_compound_mm | 2 | 12 | yes | 1999.99999999998 (A 1000.0, B 999.99999999998) | 0,0,0; 67.32050808,18.66025404,10 | Plane |
| two_boxes_assembly_mm | 2 | 12 | yes | 2000.0 (A 1000.0, B 1000.0) | 0,0,0; 67.32050808,18.66025404,10 | Plane |
| freecad_relabel_inch | 1 | 6 | yes | 196644768.0 | 0,0,0; 1016,762,254 | Plane |
| freecad_relabel_m | 1 | 6 | yes | 1.2e13 | 0,0,0; 40000,30000,10000 | Plane |
| missing_unit | 1 | 6 | yes | 12000.0 | 0,0,0; 40,30,10 | Plane (OCC silently assumes mm) |
| truncated | read fails: `Error in reading STEP` | | | | | |
| garbage | read fails: `Error in reading STEP` | | | | | |

## Entity summary (DATA section)

Every file except the malformed ones also contains the usual product
skeleton (`APPLICATION_*`, `PRODUCT*`, `SHAPE_DEFINITION_REPRESENTATION`),
plus `AXIS2_PLACEMENT_3D`, `CARTESIAN_POINT`, `DIRECTION`, `VECTOR`,
`ADVANCED_FACE`, `FACE_BOUND`, `EDGE_LOOP`, `ORIENTED_EDGE`, `EDGE_CURVE`,
`VERTEX_POINT`, `CLOSED_SHELL` and `MANIFOLD_SOLID_BREP`.

**No file contains `PCURVE`, `SURFACE_CURVE`, `SEAM_CURVE`,
`DEFINITIONAL_REPRESENTATION`, any `B_SPLINE_*` entity or `MAPPED_ITEM`.**
Every face bound is a `FACE_BOUND`. OCC never writes `FACE_OUTER_BOUND`,
so the outer loop has to be inferred, as with the plate's top and bottom
faces. OCC writes seam edges as ordinary `EDGE_CURVE`s over a `LINE` (cylinder) or
a `CIRCLE` (torus), and the same edge appears twice in the face's loop.

| File | Surfaces | Curves | Topology notes | Units |
|---|---|---|---|---|
| box_40x30x10_mm | PLANE 6 | LINE 12 | 8 vertices | `(LENGTH_UNIT NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.))`, radian, steradian, uncertainty 1e-7 |
| cylinder_d20_h15_mm | PLANE 2, CYLINDRICAL_SURFACE 1 | CIRCLE 2, LINE 1 (seam) | caps bounded by full circles, seam edge used twice | mm |
| sphere_r10_mm | SPHERICAL_SURFACE 1 | none | **one face whose only bound is a `VERTEX_LOOP`** at the south pole (5,5,-5). OCC drops the seam and degenerate edges | mm |
| torus_R20_r5_mm | TOROIDAL_SURFACE 1 (R=20, r=5) | CIRCLE 2 (r=25 and r=5 seams) | one face, one vertex, two closed seam edges | mm |
| plate_hole_mm | PLANE 6, CYLINDRICAL_SURFACE 1 | LINE 13, CIRCLE 2 | top and bottom faces have 2 bounds (outer + hole) | mm |
| two_boxes_compound_mm | PLANE 12 | LINE 24 | one `SHAPE_REPRESENTATION` (not ADVANCED_BREP) holding 2 `MANIFOLD_SOLID_BREP`s | mm |
| two_boxes_assembly_mm | PLANE 12 | LINE 24 | 3 PRODUCTs; NAUO 2, CDSR 2, IDT 2, RRWT 2, 2 `ADVANCED_BREP_SHAPE_REPRESENTATION` | mm (3 contexts) |
| box_40x30x10_inch | PLANE 6 | LINE 12 | as mm box | `(CONVERSION_BASED_UNIT('INCH',#171) LENGTH_UNIT() NAMED_UNIT(#173))`, `#171 = LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#172)`, `#172` = mm SI_UNIT, `#173 = DIMENSIONAL_EXPONENTS(1,0,...)` |
| box_40x30x10_m | PLANE 6 | LINE 12 | as mm box | `SI_UNIT($,.METRE.)` |
| freecad_relabel_inch | PLANE 6 | LINE 12 | as mm box | OCC's own inch pattern: `LENGTH_MEASURE_WITH_UNIT(25.4,#mm)`, with an **untyped** 25.4 rather than `LENGTH_MEASURE(25.4)` |
| freecad_relabel_m | PLANE 6 | LINE 12 | as mm box | `SI_UNIT($,.METRE.)` |
| missing_unit | PLANE 6 | LINE 12 | as mm box | radian and steradian only, with no uncertainty |
| truncated | (partial) PLANE 2 | LINE 9 | cut mid-file | no unit section |

## Assembly and placement findings

- **Compound** (`Part.Compound` + `Shape.exportStep`): there is no assembly
  structure at all. It has one PRODUCT, one `SHAPE_REPRESENTATION` listing
  both breps, and no NAUO, IDT, CDSR or MAPPED_ITEM. **Box B's placement is
  baked into its geometry**: its points are in world coordinates, e.g.
  `(50,0,0)` and `(45,8.660254037844,0)`. OCC rounds them to 12
  significant digits, so B's volume comes back as 999.99999999998.
- **Assembly** (`Import.export` of two document objects): one root PRODUCT
  `fixtures` and child PRODUCTs `BoxA` and `BoxB`, linked by
  `NEXT_ASSEMBLY_USAGE_OCCURRENCE` -> `PRODUCT_DEFINITION_SHAPE` ->
  `CONTEXT_DEPENDENT_SHAPE_REPRESENTATION` ->
  `(REPRESENTATION_RELATIONSHIP(child ABSR, root SR)
  REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(IDT)
  SHAPE_REPRESENTATION_RELATIONSHIP())`.
  `ITEM_DEFINED_TRANSFORMATION('','',#11,#19)` maps the child's identity
  axis (`#11`) to `#19 = AXIS2_PLACEMENT_3D((50,0,0), axis (0,0,1),
  ref (0.866025403784,0.5,0))`. That is a 30 degree rotation about Z, and the
  ref direction is again rounded to 12 digits. **Box B's geometry is local**,
  i.e. untransformed: its points lie in [0,20]x[0,10]x[0,5]. There is no
  `MAPPED_ITEM`. The transform exists only in the IDT/CDSR chain. The root
  `SHAPE_REPRESENTATION` holds the placement axes `#11`, `#15` (identity,
  for A) and `#19`.

## Units findings

- FreeCAD 1.1.4 cannot write a *scaled* non-mm STEP. `Part.exportUnits('INCH'|'M')`
  sets OCC's `write.step.unit` and changes only the declared unit. The
  coordinate values stay 40/30/10, so the file describes a 40x30x10 inch or
  metre box. The same holds for OCAF export (`Import.export`) and for the
  `Mod/Import/hSTEP` `Unit` preference (tested; that file came out in mm
  with no change). These outputs are kept as `freecad_relabel_*.step`:
  correct files in their own right, but for a different box.
- `box_40x30x10_{inch,m}.step` therefore come from `make_units.py`. In the
  DATA section it divides `CARTESIAN_POINT` coordinates, `VECTOR`
  magnitudes, the uncertainty `LENGTH_MEASURE` and any
  circle/cylinder/sphere/cone/torus radii by 25.4 or 1000. It then swaps the
  length unit for the standard AP214 inch pattern or an SI metre. It asserts
  that the source has only entity types it handles, so it refuses input
  with pcurves. Numbers are Python `repr`, e.g. 40/25.4 is
  `1.5748031496062993`. FreeCAD re-imports both as exactly 40x30x10 mm,
  V = 12000.
- `missing_unit.step`: the length unit instance is deleted and dropped from
  `GLOBAL_UNIT_ASSIGNED_CONTEXT`. The `UNCERTAINTY_MEASURE_WITH_UNIT`
  referenced that unit, so it and the `GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT`
  partial are removed too. That leaves no dangling references and the file
  is valid Part 21. OCC accepts it and silently assumes mm.

## Direct-edit fixture

`direct_edit_block_mm.step` is made by `tools/fixtures/step/direct_edit.py`
(FreeCADCmd 1.1.4, OCC 7.8.1; run `tools/heavy.sh tools/fixtures/step/direct_edit.sh`),
from Part primitives: `makeBox(60,40,20)` minus `makeCylinder(4,30,(15,20,-5))` (Ø8
through hole), minus `makeCylinder(3,13,(30,12,12))` (Ø6 blind hole, z = 12..20, flat
floor), fused with `makeCylinder(5,6,(45,25,19))` (Ø10 boss, top at z = 25),
`removeSplitter()`. Expected: [0,60]x[0,40]x[0,25], V = 48000 - 267*pi =
47161.19476149153, 11 STEP faces (8 planes, 3 cylinders; 14 faces after import, which
splits each cylinder at its seam). FreeCAD's own re-import: 1 valid solid, volume
47161.19476149152.

## Surface / open-shell fixtures

Made by `tools/fixtures/step/open_shells.py` (FreeCADCmd 1.1.4, OCC 7.8.1; run
`tools/heavy.sh tools/fixtures/step/open_shells.sh`) from `Part.makeBox(40,30,20)`
without its top face, written as SHELL_BASED_SURFACE_MODEL / OPEN_SHELL:

* `open_box_mm.step`: the 5 faces sewn into one open shell (4 free edges on z = 20).
  Filling the top gives V = 24000 mm³.
* `loose_faces_mm.step`: the same 5 faces as separate open shells (8 coincident
  edge pairs to stitch, then the top loop).
* `gap_box_mm.step`: the loose faces with the +X wall moved to x = 40.02 (a 0.02 mm
  gap to three neighbours).
* `plate_mm.step`: one planar face [0,50]x[0,20] at z = 0 (normal +Z).
* `half_tube_mm.step`: cylinder surface r 10, axis Z, z 0..30, x >= 0 (normal away
  from the axis).

FreeCAD's own re-import (script output): no solids, open shells, areas 4000 / 4000 /
4000 / 1000 / 942.477796077 mm² (= 300π).
