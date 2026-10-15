# Roadmap after 1.0

What 1.0 contains is listed in CHANGELOG.md and described in docs/COMMANDS.md and
docs/FORMAT_SUPPORT.md. Items below are not implemented (or only partly); listing them here is
not a claim of support or a promise of a release. Additions that keep 1.x documents and
commands working can land in 1.x; anything that breaks them waits for 2.0. Ordered roughly by
value per effort. Autodesk Fusion is the behavioural reference where behaviour is open.

## Modeling
- Fillets: corner blends of arbitrary edge sets and variable radii (today: single straight
  prismatic edges, and whole edge loops of a cap face with tori, sphere corners and mitred
  corners, also on hole loops, docs/FILLETS.md); shell of a body whose loop is already
  filleted (today: shell first, then the loop fillet).
- Exact helical threads and a coil feature (today: faceted threads within 0.01 mm, ADR 0011).
- Draft of existing faces (today: taper at extrusion time only); replacing a face of an imported
  body by a curved or arbitrary surface (today: Tilt face replaces a planar face by another
  plane, ADR 0010).
- Cuts that leave an enclosed void (one body with a cavity); pieces from tapered, multi-region,
  pattern/mirror, combine, revolve and hole cuts (today: single-region straight cuts and Split
  Body make one body per piece, ADR 0003).
- Patterns of New Body extrusions, whole bodies, sketch features and fillet features (today:
  seeds are Extrude Join/Cut and Hole features, whose instances' planar faces are referenceable).
- Revolve of sections meeting the axis at a point or at an angle (cone tips, poles), and partial
  turns of axis-touching sections.
- Sketches on curved faces; 3D sketches; loft guide rails; helical sweeps.
- Faster curved booleans (revolved Join/Cut still takes seconds to tens of seconds; Truck's
  implicit intersection curves dominate where no exact replacement exists,
  `crates/rcad-kernel/src/exact.rs`).
- Piece identity of split bodies by volume overlap (today: stored seed points inside each
  piece, with bounding-box order only for ids without seeds, ADR 0003).
- Revolve of ellipse/spline sections (refused today: Truck cannot mesh the revolved faces).
- Exact operations on mesh bodies via mesh-to-B-rep reconstruction (out of scope so far).

## Interchange
- STEP: colours round trip (names of solids are kept), AP242 export (today AP214), solids and
  surface bodies in one file.
- 3MF: colours/materials kept on bodies.
- SVG import: Bézier curves, elliptical arcs, `use` expansion, text to outlines.
- DXF: non-uniform/rational SPLINEs and elliptical arcs on import, R2000+ export of ellipses/splines,
  blocks/INSERT with transforms, dimension import as constraints.
- IGES, OBJ: possible later; DWG and F3D/F3Z: no plan (closed formats).

## Product
- Drawings: radius/angular/chain dimensions, GD&T, broken and auxiliary views.
- Sheet metal: closed/rolled hems (today: open hem), flanges on flanges or arcs, miters, K-factor tables.
- Manufacturing workflows (CAM setup, toolpaths), simulation, cloud collaboration: separate
  programs, not planned here.
- Persistent undo history (today: in memory, 256 steps); project/workspace management (today:
  Open Recent); more preferences (today: key layer, theme, sketch auto-projection, remembered
  panel widths).
