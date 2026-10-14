# Mesh bodies: inspection, repair and edits

Code: `crates/rcad-exchange/src/mesh.rs` (diagnosis, self-intersection check),
`mesh_query.rs` (grid, point-triangle distance, triangle-pair test), `mesh_repair.rs`
(repair), `mesh_edit.rs` (transform, plane cut, decimation, deviation measurement);
`FeatureKind::MeshRepair` / `FeatureKind::MeshEdit` (`crates/rcad-document`, evaluation in
`mesh_edit.rs`); commands `repair_mesh`, `edit_mesh`, `inspect_mesh`
(`crates/rcad-commands/src/mesh_repair.rs`, `mesh_edit.rs`); UI
`crates/rcad/src/mesh_repair_ui.rs`, `mesh_edit_ui.rs` (body inspector: "Mesh inspection",
"Mesh check / repair", "Mesh edit"). Exact definitions are in each module's docs; this page
summarises them.

## Diagnosis (shown in the inspector, "Mesh inspection")

Triangles, components, boundary / non-manifold / inconsistently oriented edges, degenerate
triangles, **duplicate triangles** (same three vertices as an earlier triangle, any order or
orientation), closed or not, signed volume only when closed (`MeshDiagnostics`). **Per
component** (`component_details`, the largest 256, largest first): triangles, bounds, closed
(the closed rule applied to that component's edges) and volume only when that component is
closed. Diagnosis never changes the mesh.

### Self-intersection check (`inspect_mesh`, "Check self-intersections")

On demand, read only (`SelfIntersectionReport`). Two triangles intersect when one passes
**properly** through the other (or coplanar ones overlap with area), every sign test decided
by more than `1e-9 ×` the bounds diagonal; touching contacts are not intersections.
Candidate pairs come from a uniform grid of triangle bounding boxes; each pair is tested
once. **Scope / limits:**

* pairs that share a vertex (after merging identical coordinates) are not tested, so a fold
  between neighbouring triangles is not detected;
* at most 2 000 000 triangles are indexed and 50 000 000 pair tests run; beyond that the
  report says `complete: false` and the UI labels the result "PARTIAL check (bounded; not a
  proof)";
* a complete check that finds nothing is floating-point evidence to the stated tolerance,
  labelled "not an exact proof". Interpenetrating separate shells are found.

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
splitting (decimation: see Mesh edits), remeshing, mesh-to-solid conversion, non-planar hole filling.
A closed result is **not** a proof of a valid printable solid; exports still run the
strict print-mesh check (the unrepaired body's STL export is refused).

Limits: 5 000 000 input triangles; weld tolerance at most 1 mm; loops up to 10 000 edges.

## Mesh edits (`edit_mesh`, "Mesh edit")

`edit_mesh {body, op, preview?, body_name?, hide_source?}`: **preview** returns
`Response.mesh_edit` (counts, the diagnosis after, measured deviation) and changes nothing;
**apply** adds a `MeshEdit` feature (format **v8**) making a new mesh body
(`BodySource::EditedMesh`; two for a two-sided cut) from the source, which is kept (hidden
with `hide_source`). One undo step; regenerated from the source on every evaluation. A failed
edit (or a parameter edit that makes it fail) is refused and the previous state kept.

| op (`"op"`) | parameters | definition |
|---|---|---|
| `transform` | `sx sy sz` (each in [1e-6, 1e6]), `angle_deg` about axis `axis` through the origin, `tx ty tz` mm | `p' = R(S p) + t`. Mirroring (negative scale) is refused. Presets (UI buttons, `MeshTransform::unit_conversion`): inch → mm ×25.4, m → mm ×1000, cm → mm ×10. Volume scales by `sx·sy·sz`. |
| `plane_cut` | `origin`, `normal`, `offset` (mm along the unit normal; editable), `keep` `below`/`above`/`both`, `cap` | Triangles crossing the plane are clipped exactly with shared cut vertices; on-plane distance `≤ 1e-9 ×` diagonal. With `cap` and a **closed** input the cut loops are triangulated in the plane (outer loops/holes by orientation, holes bridged, ear clipping), cap normal towards the removed side. Open input or `cap: false`: the result is open, loops counted (`cut_loops`), a note says so. Refused: the plane misses the mesh; nothing kept; loops that touch, run the wrong way or are open on a closed input; more than 20 000 loop vertices. |
| `decimate` | `target_triangles` (0 = none), `max_deviation` mm (> 0) | Quadric edge collapse (Garland-Heckbert, area-weighted plane quadrics, optimal point or cheaper endpoint/midpoint). Each collapse is refused if it moves a boundary or non-manifold vertex, breaks the link condition, leaves a vertex of valence < 3, duplicates a triangle, turns a face normal by more than ~78° or makes it degenerate, or would put a tracked source sample (every source vertex and triangle centroid) or a new-surface sample (new vertex, new centroids and edge midpoints) further than `max_deviation` from the other surface. Stops at the target or when nothing more can be collapsed (note). Input with degenerate or duplicate triangles is refused (repair first); limit 2 000 000 triangles. A target alone is not accepted: every decimation states its deviation bound (give a generous bound to reach a target). |

**Deviation measurement** (decimation, `DeviationReport`): exact point-to-triangle distances
(grid nearest-triangle search) from the source's vertices and triangle centroids to the
result surface, and from the result's vertices, edge midpoints and centroids to the source
surface; max and mean of each and the larger maximum. These are samples, so the maximum is a
lower bound on the true Hausdorff distance. A measured maximum above `max_deviation` refuses
the result. Bounds are therefore kept within the deviation, and boundaries exactly.

Change counts in the report: `vertices_moved`, `triangles_cut`, `triangles_removed`,
`triangles_added` (cap), `cut_loops`, `capped_loops`, `collapses`, `refused_topology`,
`refused_normal`, `refused_deviation`; the summary line repeats them.

Viewport preview (`RcadApp::mesh_edit_preview`): while the "Mesh edit" panel is shown, a
plane cut's plane is drawn across the body's bounds with an arrow towards the kept side; after
**Preview edit** the bounds of each result are drawn as boxes. The result meshes themselves are
not drawn before Apply.

Not done: drawing the previewed result meshes, arbitrary plane picking in the UI
(axis-aligned planes; the command takes any plane), remeshing, mirroring, self-intersection
removal.

Performance (release build, one 159 200-triangle UV sphere r 50): self-intersection check
83 ms (1.9 M pair tests); decimation at 0.05 mm 1.7 s → 4 492 triangles, measured deviation
0.049998 mm; two-sided oblique cut with caps 46 ms.

## Evidence

* `rcad-exchange` unit tests (`mesh_repair::tests`): clean cube only welds; flipped,
  duplicate, degenerate fixed; inside-out cube turned outward; tolerance weld closes a crack;
  planar hole filled, over-limit hole counted once; non-planar hole left open; bad options.
* `crates/rcad-commands/tests/mesh_repair.rs`: STL import → diagnosis → preview (no change)
  → apply (new closed body, 8000 mm³ mass-properties volume, source kept/hidden) → undo/redo
  → parameter edit and refusal → save/reopen v7; B-rep and unknown bodies refused.
* `crates/rcad/tests/mesh_repair_ui.rs`: the same through the inspector widgets.
* `rcad-exchange` `tests/mesh_edit.rs` (voxel solids with volumes known by counting cubes,
  a UV sphere): duplicates and per-component bounds/volume; self-intersection finds
  interpenetrating shells, none on a clean block, partial when bounded; transform volume ×
  25.4³ and rotated bounds, mirroring refused; plane cuts (through vertices, oblique, generic:
  halves sum to the whole; a ring whose cap has a hole: volume 4.8; two components; sphere),
  refusals and open results; decimation of a subdivided cube is exact (volume 64, bounds kept,
  < 80 of 192 triangles); sphere within 0.1 mm (re-measured independently), exact targets,
  early stop note; open plate boundary and area preserved. `mesh_query` unit tests: point-
  triangle regions, touching vs crossing pairs, grid search equals brute force.
* `crates/rcad-commands/tests/mesh_edit.rs`: STL import → inspect (read only) → transform
  preview/apply (inch preset) → two-sided cut (16000 + 48000 mm³) → decimation (64000 mm³,
  deviation ≤ 0.001) → undo/redo → cut offset parameter (24000 + 40000) and refused
  parameters → save v8 / reopen; B-rep and unknown bodies refused.
* `crates/rcad/tests/mesh_edit_ui.rs`: inspection, self-intersection check, plane cut
  preview/apply and the inch preset through the inspector widgets.
* External, edits: `tools/validate_mesh_edit.sh <rcad-cli>` replays
  `tools/fixtures/mesh_edit_workflow.json` on FreeCAD's ASCII sphere (corpus `m2`): two-sided
  cut at z = 2, decimation within 0.05 mm, scale (2, 1, 0.5) + 30° about Z + 10 mm X.
  `tools/check_mesh_edit.py` (stdlib Python, brute-force exact distances): halves watertight,
  1436.829 + 659.741 = 2096.570 mm³ (source), cut planes at z = 2; decimated 760 → 440
  triangles, measured deviation 0.0497 / 0.0497 mm (source→result / result→source) ≤ 0.05;
  transformed volume equal to the source, bounds equal to the source transformed in Python.
  FreeCAD 1.1.4 Mesh: all four solid, manifold, **no self-intersections**. ALL OK
  (`artifacts/mesh_edit_validation/`).
* External: `tools/validate_mesh_repair.sh <rcad-cli>` on `tools/fixtures/stl/broken_cube.stl`:
  unrepaired export refused; repaired STL checked by `tools/check_stl.py` (watertight,
  8000 mm³) and FreeCAD Mesh (isSolid, 12 facets, 8000 mm³, manifold, no self-intersections,
  bounds [0,20]³): ALL OK.
