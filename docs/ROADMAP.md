# Roadmap

What exists is described in docs/COMMANDS.md and docs/FORMAT_SUPPORT.md. Items below are not
implemented; listing them here is not a claim of support. Ordered roughly by value per effort.

## Modeling
- Booleans with tools that have inner loops (Join/Cut of a region with holes). Truck 0.6
  `shapeops` fails for such tools (measured run 3); options: upstream fix, or decompose into
  per-loop operations where that is provably equivalent.
- Region picking inside a sketch (choose which closed region(s) to extrude; islands inside holes;
  several outer loops in one feature producing several bodies).
- Patterns and mirrors of New Body extrusions, whole bodies and sketches
  (today: linear/circular patterns and mirrors of Extrude Join/Cut features only), and referenceable faces
  on pattern/mirror instances.
- General reference repair: re-attach with preview for every reference kind, and topological
  naming that survives face splits/merges (today: deterministic refusal + explicit re-attach).
- Sketches on curved faces; construction planes (offset, angled, through edges).
- Fillet: not feasible through Truck 0.6 booleans — a tool whose arc is tangent to both faces
  returns no result at every tolerance (spike: `rcad-kernel` `ops::tests::spike_fillet_tangent_arc_tool`,
  ignored, run with `--ignored`). Needs direct B-rep face construction (replace the edge by a
  cylindrical face and trim the neighbours) or a kernel with fillets. Chamfers of straight convex
  edges exist (docs/COMMANDS.md `chamfer`); vertex blends and chamfer chains do not.
- Shell, draft, loft, sweep.
- Exact operations on mesh bodies via mesh-to-B-rep reconstruction (explicitly out of scope so
  far).

## Interchange
- STEP: wider entity coverage (`BREP_WITH_VOIDS`, `MAPPED_ITEM` placements, shells/surfaces,
  colours/names round trip), AP242 specifics, assemblies as components (today: flattened
  placements of supported representations only, see docs/formats/step.md).
- 3MF: colours/materials kept on bodies, UI dialogs (today: command/CLI import/export of mesh
  objects with units).
- SVG: Bézier curves (exact as sketch splines once the sketcher has them), elliptical arcs,
  `use` expansion, text to outlines (today: lines/rects/circles/circular arcs, docs/formats/svg.md).
- DXF: SPLINE/ELLIPSE, blocks/INSERT with transforms, dimension import as constraints.
- IGES, OBJ: possible later; DWG and F3D/F3Z: no plan (closed formats).

## Product
- Components and assembly constraints; drawings (2D projections with dimensions); later
  manufacturing workflows (CAM setup, toolpaths).
- Persistent undo history; project/workspace management; preferences.
