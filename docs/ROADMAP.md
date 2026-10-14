# Roadmap

What exists is described in docs/COMMANDS.md and docs/FORMAT_SUPPORT.md. Items below are not
implemented (or only partly); listing them here is not a claim of support. Ordered roughly by
value per effort. Autodesk Fusion is the behavioural reference where behaviour is open.

## Modeling
- Fillets: corner blends of arbitrary edge sets and variable radii (today: single straight
  prismatic edges, and whole edge loops of a cap face with tori, sphere corners and mitred
  corners, docs/FILLETS.md); shell of a body whose loop is already filleted (today: shell first,
  then the loop fillet).
- Exact helical threads and a coil feature (today: faceted threads within 0.01 mm, ADR 0011).
- Draft of existing faces (today: taper at extrusion time only); direct replace-face on
  imported bodies.
- Cuts that leave an enclosed void (one body with a cavity); pieces from tapered, multi-region,
  pattern/mirror, combine, revolve and hole cuts (today: single-region straight cuts and Split
  Body make one body per piece, ADR 0003).
- Patterns of New Body extrusions, whole bodies, sketches and fillet features; referenceable
  faces on pattern/mirror instances.
- Revolve of sections meeting the axis at a point or at an angle (cone tips, poles), and partial
  turns of axis-touching sections.
- Sketches on curved faces; 3D sketches; loft guide rails; helical sweeps.
- Faster curved booleans (seconds to tens of seconds today; Truck's implicit intersection curves
  dominate where no exact replacement exists, `crates/rcad-kernel/src/exact.rs`).
- Exact operations on mesh bodies via mesh-to-B-rep reconstruction (out of scope so far).

## Interchange
- STEP: colours/names round trip, AP242 specifics, solids and surface bodies in one file.
- 3MF: colours/materials kept on bodies.
- SVG: Bézier curves, elliptical arcs, `use` expansion, text to outlines.
- DXF: non-uniform/rational SPLINEs and elliptical arcs, R2000+ export of ellipses/splines,
  blocks/INSERT with transforms, dimension import as constraints.
- Cancellation of long imports (today: bounded by size and entity limits).
- IGES, OBJ: possible later; DWG and F3D/F3Z: no plan (closed formats).

## Product
- Drawings: radius/angular/chain dimensions, GD&T, broken and auxiliary views.
- Sheet metal: closed/rolled hems, flanges on flanges or arcs, miters, K-factor tables.
- Manufacturing workflows (CAM setup, toolpaths), simulation, cloud collaboration: separate
  programs, not planned here.
- Persistent undo history; project/workspace management; preferences.
