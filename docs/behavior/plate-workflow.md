# Plate-with-hole workflow: behaviour spec

Labels: **[spec]** from GOAL.md; **[assumed]** our product decision; **[observed]** measured in this repo.
No reference application was used (Fusion not available) → reference validation pending.

## Feature model
1. `Sketch` on `WorldXY`: dimensioned centred rectangle. Constraints: 2 horizontal, 2 vertical,
   horizontal distance `width`, vertical distance `height`, diagonal corners symmetric about a fixed
   origin point. 8 unknowns, 8 independent equations → solver reports FullyConstrained, DOF 0 **[observed]**.
2. `Extrude` (new body) along the sketch normal by `distance` > 0 **[spec]**.
3. `Sketch` on `Face(FaceRef{feature: <extrude>, role: ExtrudeEndCap})`: centred circle `diameter`.
4. `Cut` ThroughAll: both directions along the sketch normal; the cutter overshoots the body by
   max(1 mm, 5 % of the bbox diagonal) to avoid coplanar boolean faces (Truck limitation, ADR 0001).

## Semantic face references
A `FaceRef` stores *(originating feature, generated-face role)*, never a face index or mesh id.
Resolution: the end-cap frame is computed from the extrusion's definition (profile frame offset by
`distance`; start cap = profile frame with flipped normal). It is then **validated against the B-rep**:
exactly one planar face of the current body must coincide with that frame (`planar_faces_on`). Zero or
several matches → `BrokenReference` error; nothing is silently re-bound **[assumed]**.
Limit: roles exist only for extrusion caps; side faces, faces created by cuts, and faces that a later
feature splits into several coplanar pieces are not referenceable yet.

## Hole restriction (initial product limit, not a CAD rule)
A circular cut sketched on an extrusion face must lie strictly inside that extrusion's polygonal
profile, with clearance > `tol::MIN_LENGTH` (1 µm) to every edge. For a centred rectangle this is
`diameter < min(width, height)`. Equality (tangent) is rejected too **[assumed]**. Checked on every
regeneration, so it also rejects shrinking the plate below the hole.

## Transactions and history
Every edit is applied to a copy of the document, regenerated, and committed only on success.
Undo/redo swap whole-document snapshots (parameters, features, references) and regenerate.
A new edit clears redo. Rejected edits do not create history entries or bump `revision`.

## Persistence
`*.f420.json`: `{format: "fission420", version: 1, document, cache}`. `document` is authoritative;
`cache` (volume/bounds/face count at save time) is only compared after regeneration. Unknown
format/newer version/structural errors/dangling references are rejected with `File` errors.

## Measurements
- Volume: B-rep boundary integral (see ADR 0001); `Unavailable` for non-Z-ruled curved faces.
- Bounds: vertices + exact-curve samples (exact for this model's extremes, which lie on lines).
- Hole axis check: centroid of the inner loops on z=0 and z=d faces (Green's theorem on exact curves).
