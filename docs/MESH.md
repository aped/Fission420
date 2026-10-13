# Mesh bodies: diagnosis and repair

Status: run 7 (pull-forward of Run-8 §2 / coverage ME01, first slice). Code:
`crates/rcad-exchange/src/mesh.rs` (diagnosis), `crates/rcad-exchange/src/mesh_repair.rs`
(repair), `FeatureKind::MeshRepair` (`crates/rcad-document`), `repair_mesh`
(`crates/rcad-commands/src/mesh_repair.rs`), UI `crates/rcad/src/mesh_repair_ui.rs`
(body inspector, "Mesh check / repair").

## Diagnosis (unchanged from run 4, shown in the inspector)

Triangles, components, boundary / non-manifold / inconsistently oriented edges, degenerate
triangles, closed or not, signed volume only when closed (`MeshDiagnostics`; definitions in
`mesh.rs`). Diagnosis never changes the mesh.

## Repair

`repair_mesh {body, options, preview?, body_name?, hide_source?}` (format v7 when applied):

* **preview**: computes the repair and returns `Response.mesh_repair` (every count and the
  diagnosis before/after); no feature, no undo step, nothing changes.
* **apply**: a `MeshRepair` feature that makes a **new** mesh body from the source
  (`BodySource::RepairedMesh`); the source body is kept (hidden with `hide_source`). One undo
  step; regenerated from the source on every evaluation, so it follows a re-import.
  Parameter `weld_tolerance` is editable (0..1 mm).

Steps (all deterministic; see the module docs for exact definitions):

1. exact weld of bit-identical vertices (always; STL stores corners per triangle), then
   merging within `weld_tolerance` mm (union-find, transitive: stated);
2. remove degenerate triangles (repeated index or zero area, relative 1e-12);
3. remove duplicate triangles (same three vertices, any order/orientation; first kept);
4. orient consistently across manifold edges (BFS from each component's first triangle);
   closed components with negative volume are flipped outward;
5. fill boundary loops of at most `fill_holes_max_edges` edges that are planar within
   `max(weld_tolerance, 1e-3 × loop size)` and simple in projection (ear clipping);
   other loops are left open, counted and named in `notes`.

Not done (refused or out of scope): self-intersection removal, non-manifold edge
splitting, decimation/remeshing, mesh-to-solid conversion, non-planar hole filling.
A closed result is **not** a proof of a valid printable solid; exports still run the
strict print-mesh check (the unrepaired body's STL export is refused).

Limits: 5 000 000 input triangles; weld tolerance at most 1 mm; loops up to 10 000 edges.

## Evidence

* `rcad-exchange` unit tests (`mesh_repair::tests`): clean cube only welds; flipped,
  duplicate, degenerate fixed; inside-out cube turned outward; tolerance weld closes a crack;
  planar hole filled, over-limit hole counted once; non-planar hole left open; bad options.
* `crates/rcad-commands/tests/mesh_repair.rs`: STL import → diagnosis → preview (no change)
  → apply (new closed body, 8000 mm³ mass-properties volume, source kept/hidden) → undo/redo
  → parameter edit and refusal → save/reopen v7; B-rep and unknown bodies refused.
* `crates/rcad/tests/mesh_repair_ui.rs`: the same through the inspector widgets.
* External: `tools/validate_mesh_repair.sh <rcad-cli>` on `tools/fixtures/stl/broken_cube.stl`:
  unrepaired export refused; repaired STL checked by `tools/check_stl.py` (watertight,
  8000 mm³) and FreeCAD Mesh (isSolid, 12 facets, 8000 mm³, manifold, no self-intersections,
  bounds [0,20]³): ALL OK.
