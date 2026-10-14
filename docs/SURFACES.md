# Surfaces and open-shell repair (run 7, bounded subset)

Exact B-rep surfaces (Truck faces with their exact planes, cylinders, NURBS) held as **surface
bodies**: bodies whose shells are open. A surface body is never reported as a solid:
measurements say `all_shells_closed: false` and the volume is *unavailable* ("open shell (surface
body): no enclosed volume"); every solid operation (cut, join, fillet, shell, combine, direct
edit, ...) refuses it ("open surface body; stitch, fill or thicken it first"). Every repair makes
a **new body** and keeps the input (hidden by default), so the original stays inspectable.

## Import

`import_surfaces {path, units_to_mm?}` (UI: Import dialog, STEP, "Import surface models (open
shells) as a surface body"): every shell of every `SHELL_BASED_SURFACE_MODEL` of the file (with
assembly placements) becomes one shell of a single surface body (`ImportSurfaces` feature, file
embedded). `import_step` on a file with only surface models is refused with a pointer to
`import_surfaces`; a file with solids and surface models imports the solids and notes the
surfaces in the import report (nothing is dropped silently). `OPEN_SHELL` inside a
`MANIFOLD_SOLID_BREP` stays an error (that is a malformed solid).

## Surfaces from sketches

* `surface_patch {sketch}` (UI: sketch inspector **Surface from sketch → Planar patch**): the
  closed profile (holes allowed) as a one-face planar surface body.
* `surface_extrude {sketch, distance, reverse?}` (**Extrude surface**): the closed profile, or
  else the open chain of lines, swept along the sketch normal into an open ruled surface (planar
  strips; exact cylindrical patches for arcs). No caps. Parameter `distance` (upstream edits
  regenerate later stitch/fill/thicken). UI: **Reverse (below the sketch plane)** checkbox.
  * **Orientation** (what a later thicken's side follows): every face's normal is the path tangent
    × the sketch normal, i.e. to the right of the direction of travel seen from the sketch's +z. A
    closed profile is run counter-clockwise, so it faces outward. An open chain is run from its
    end with the smaller sketch x (the smaller y when the ends' x agree within 1e-9 × the sketch
    scale), independent of entity ids and line directions: redrawing or reversing a line never
    flips the surface. A chain along +x faces −y.
  * `reverse` sweeps against the sketch normal (the surface lies below the sketch plane); it does
    **not** flip the normal.

## Analysis (before any repair)

`analyze_surface {body, search = 0.1}` (UI: **Analyse**) reports, without changing anything:

* free (boundary) edges with end points, length and owning face,
* **gaps**: pairs of free edges whose end points and midpoints are all within `search` mm, with
  the gap size (largest of those distances),
* closed **boundary loops** of the remaining free edges, with planarity (largest distance of
  vertices and edge midpoints from the Newell plane),
* **orientation conflicts**: face pairs using a shared edge in the same direction,
* non-manifold edges (used by more than two faces), shell and face counts.

`search` is limited to [0, 10] mm and at most 1000 gap records are returned (`gaps_total` counts
all); candidate pairs come from a spatial grid, not all pairs.

## Stitch

`stitch_surface {body, tolerance}`: vertices closer than the **healing tolerance** become one,
edges with the same end vertices and midpoints within it become one; face orientations are made
consistent (breadth-first; non-orientable sets are refused). The tolerance is shown in the
feature notes with the largest merged gap. Geometry is not moved: a straight edge whose end
vertices merged is rebuilt as the exact line between them; faces keep their surfaces, so after
healing a face can be off its boundary by at most the tolerance (the healed solid's volume is
within gap × face area of the ideal one; the gap-box test checks exactly that). A result with
no free edges is a closed solid (oriented outward by its signed volume); otherwise it stays an
open shell (possibly several pieces; a result of several separate *closed* pieces is refused,
because one body is one solid). A tolerance large enough to make an edge shared by three
faces, to collapse an edge, or to chain merges so a vertex moves farther than the tolerance, is
refused. Tolerance range [0, 1] mm.

## Fill (patch)

`fill_surface {body, tolerance}`: every closed boundary loop (lines, arcs, any edge curves) that
is **planar** within the tolerance (curved edges sampled at 16 points) gets an exact planar face
bounded by the loop's own edges; coplanar nested loops of opposite orientation become one face
with holes (e.g. the two end rings of an open tube): a loop is a hole of the smallest coplanar
loop that wholly contains it (every sample point inside, no boundary crossing). Same-orientation
or deeper nesting (an island inside a hole) is refused, and so are coplanar loops that partially
overlap (boundaries crossing, or points both inside and outside) or coincide.
Non-planar loops are refused (not skipped); a body with several shells or orientation conflicts
must be stitched first. If the result is closed it is a solid.

## Thicken

`thicken_surface {body, distance, reverse}`: a surface body of **one face**:

* planar face (any boundary, holes included): the exact prism along the face normal
  (`reverse`: against it). Never self-intersects.
* cylindrical face bounded by circles perpendicular to the axis and 0 or 2 straight rulings
  (a full tube or a sector patch): the exact annular sector between radius r and r ± d, extruded
  along the axis. Thickening **toward the axis** by d ≥ r collapses the offset surface and is
  refused with the largest possible thickness. The cylinder (axis, radius) is fitted from the
  face's samples and verified against all of them within the import consistency tolerance; a face
  that is not a cylinder within it is refused.

Other surfaces, several faces, trimmed cylinders, or a closed body are refused by name. The
result's volume and orientation are checked in closed form and by FreeCAD.

## Acceptance project 7

`examples/open_box_repair_workflow.json` / `examples/open_box_repair.rcad.json`, fixtures made
independently by FreeCAD (`tools/fixtures/step/open_shells.py`): the open box
[0,40]×[0,30]×[0,20] without its top, as one sewn shell (`open_box_mm.step`), as loose faces
(`loose_faces_mm.step`, 8 coincident edge pairs found by analysis) and with the +X wall moved
0.02 mm (`gap_box_mm.step`, three 0.02 mm gaps found). Results: filled open box V = 24000 exactly;
gap box stitched at 0.05 mm then filled, V within 0.02 × 600 mm³ (24012 here: the wall plane
stays at x = 40.02); stitching at 0.01 mm leaves it open and the fill refused; half tube
r 10 thickened 2 mm outward V = (π/2)(12² − 10²)·30, inward 4 mm (π/2)(10² − 6²)·30, inward
≥ 10 mm refused. External check: `tools/validate_sheet_surface.sh` (FreeCAD: one valid solid,
positive volume = outward orientation, volume and bounding box).

## Not implemented (tracked)

Non-planar patches (N-sided fill, boundary-blend), continuity (G1/G2) control, offset of general
surfaces and multi-face thicken (offset with mitred joints), swept/lofted surface bodies (only straight extrusion of sketches),
trimming and extending surfaces, surface-to-solid replace, sewing with geometry fitting (edges are
merged, not re-fitted), T-splines / freeform modelling, export of surface bodies to STEP as
`SHELL_BASED_SURFACE_MODEL` (STEP export of open bodies is not supported).
