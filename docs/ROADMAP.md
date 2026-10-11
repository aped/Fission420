# Roadmap

What exists is described in docs/COMMANDS.md and docs/FORMAT_SUPPORT.md. Items below are not
implemented; listing them here is not a claim of support. Ordered roughly by value per effort.

## Modeling
- Booleans with tools that have inner loops (Join/Cut of a region with holes). Truck 0.6
  `shapeops` fails for such tools (measured run 3); options: upstream fix, or decompose into
  per-loop operations where that is provably equivalent.
- Region picking inside a sketch (choose which closed region(s) to extrude; islands inside holes;
  several outer loops in one feature producing several bodies).
- Linear and circular patterns of features and bodies beyond a row of cuts; mirror of sketches and
  features across planes, with explicit copy semantics.
- General reference repair: re-attach with preview for every reference kind, and topological
  naming that survives face splits/merges (today: deterministic refusal + explicit re-attach).
- Sketches on curved faces; construction planes (offset, angled, through edges).
- Edge chamfer, then fillet (only where the kernel offers a robust path), shell, draft, loft,
  sweep.
- Exact operations on mesh bodies via mesh-to-B-rep reconstruction (explicitly out of scope so
  far).

## Interchange
- STEP: wider entity coverage (`BREP_WITH_VOIDS`, `MAPPED_ITEM` placements, shells/surfaces,
  colours/names round trip), AP242 specifics, assemblies as components (today: flattened
  placements of supported representations only, see docs/formats/step.md).
- 3MF import/export of multiple mesh objects with units (continuation backlog of run 3).
- SVG planar paths (units, transforms, unsupported-curve diagnostics).
- DXF: SPLINE/ELLIPSE, blocks/INSERT with transforms, dimension import as constraints.
- IGES, OBJ: possible later; DWG and F3D/F3Z: no plan (closed formats).

## Product
- Components and assembly constraints; drawings (2D projections with dimensions); later
  manufacturing workflows (CAM setup, toolpaths).
- Persistent undo history; project/workspace management; preferences.
