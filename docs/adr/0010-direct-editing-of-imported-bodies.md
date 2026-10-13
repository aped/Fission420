# ADR 0010: Direct editing of imported (STEP) bodies

Status: accepted (run 6, 2026-10-13). ADR 0005 (independent result check) applies to
every result.

## Context

An imported STEP body has no feature history (ADR 0003: none is recovered or implied).
Users still need to change it: move a face, open up or shrink a hole, delete a hole or
a boss. These edits must be exact (no faceting, no mesh CSG), parametric (bindable to
expressions, regenerated from the embedded STEP bytes) and must keep face references
meaningful or break them explicitly.

## Decision

### Kernel (`rcad-kernel/src/direct.rs`)

All edits work on the body's compressed topology (`Solid::compress` → edit →
`Solid::extract`). Face indices are positions in `face_iter` order.

* `offset_face(body, face, d)`: planar face moved by `d` along its outward normal `n`.
  **Precondition:** every face touching it (along an edge or at a vertex) is a wall
  along `n`: a plane containing `n`, or a curved face whose outward normal is
  perpendicular to `n` everywhere sampled (boundary samples plus a 7 x 7 parameter grid,
  to 1e-6). Every other edge ending on the face is a straight segment along `n`.
  **Construction:** the plane, its edges and its vertices are translated (affine, exact);
  the side edges are rebuilt as exact segments. Topology and face order unchanged.
  **Refused:** oblique neighbour ("not perpendicular"), a side edge that would shrink below
  `MIN_LENGTH` or flip (a face would degenerate or the topology change, e.g. a boss
  swallowed by its base face, a blind hole losing its wall), non-planar face.
* `resize_cylinder(body, face, diameter)`: the *feature* of a cylindrical face is the
  set of coaxial cylinder faces of the same radius connected to it (a STEP cylinder is
  split in two at import), plus *caps* (planar faces perpendicular to the axis bounded
  only by those cylinders). Every other face it meets (*pierced*) must be a plane
  perpendicular to the axis. Supported: through hole (no cap, two pierced planes), blind
  hole with a flat floor, boss with a flat top. The feature's faces, edges and vertices
  are mapped by the radial scaling about the axis (affine: circles stay exact circles;
  a revolved surface gets its generatrix scaled in the revolution's own frame so its
  placement stays rigid). **Clearance (growing):** the new circle must stay more than
  `MIN_LENGTH` from every other boundary edge of every pierced face, by exact distance to
  segments and circular arcs (free-form edges are refused).
* `remove_cylinder(body, face)`: deletes the feature's faces and the inner loops it
  leaves on the pierced planes; each such loop must consist only of the feature's edges
  and must not be the face's only loop. Removed faces are reported by old index
  (`DirectEdit::removed_faces`, `renumber`).
* **Checks on every result:** every touched edge lies on the surfaces of both its faces
  (catches a moved edge leaving a bounded patch), one closed shell with positive volume,
  and `verify_prisms_boolean` against the nominal prism the edit adds or removes (offset:
  the face region swept by `d`, Join for `d > 0`, Cut otherwise; resize: the disk or ring
  between the radii over the axial span; removal: the disk). Interference with geometry
  away from the edited face (e.g. a hole growing into an internal void) therefore fails
  the probe check rather than producing a self-intersecting solid.
* `replace_face(body, face, origin, normal)` (task d of SESSION_6 §C): the planar face
  replaced by another plane within 89° of it. **Precondition:** one convex loop of
  straight edges, every touching face planar, exactly one straight side edge per
  vertex. Each vertex slides along its side edge's line to the new plane (`t > 0`, the
  edge keeps `MIN_LENGTH`), the face's edges become the exact segments between the moved
  vertices, the face's surface becomes the new plane (surface normal on the old side so
  the stored wire orientation stays valid). **Checks:** the face stays a convex loop,
  every neighbour loop keeps its orientation and stays simple, edges on surfaces, one
  closed shell, and point probes on both sides of the face and of every neighbour
  against the expected solid (inside the column over the face, bounded by the
  neighbours' planes: material below the new plane where there was material or the
  region above the old plane; elsewhere the old body). Document: `DirectOp::ReplaceFace
  { distance, angle }` relative to the imported face frame (pivot = its centroid, tilt
  about its x axis, +angle turns +y toward the normal, then offset along the tilted
  normal); the planar-face table's frame follows, so sketches on the face follow it.
* Not supported (named refusals): offset with oblique neighbours (use replace),
  replace with curved neighbours, inner loops or non-convex faces,
  cones / countersinks, holes ending on curved or tilted faces, partial-loop features
  (a hole breaking through a side wall), bodies with several shells.

### Document (`rcad-document/src/direct.rs`, `FeatureKind::DirectEdit`)

* `FeatureKind::DirectEdit { face: FaceRef, op: DirectOp }`, `DirectOp ∈ {OffsetFace
  { distance }, ResizeCylinder { diameter }, RemoveCylinder}`; parameters `distance`,
  `diameter` (bindable, `SlotKind::Length`). Its target body is the face's body, so it is
  ordered after the import and any earlier modifier of that body. It is *direct-edit
  history*: the summary kind is `direct_edit`; nothing is presented as recovered feature
  history.
* Faces are named by `FaceRole::Imported { body, face }` with `face` = **import index**
  (position right after import; deterministic for the same bytes). The role was
  planar-only for sketches; its meaning is unchanged for them (sketch planes still
  resolve only planar imported faces), and direct edits use it for cylinders too.
  Serde-unchanged.
* `ImportedFaceIds` maps current face positions to import indices while the body has
  been changed only by its import and direct edits (`valid_at` = last such feature).
  Offset/resize keep it; a removal retires the removed indices. A direct edit naming a
  retired index is a `BrokenReference` ("no longer exists"); after any other feature has
  changed the body, a direct edit is a `BrokenReference` ("can no longer be identified"):
  indices are never re-guessed on changed topology.
* The imported planar face table (sketch planes) follows the edits: an offset face's
  frame moves with it (sketches on it move too, the frame's in-plane axes and origin
  are kept); faces removed by an edit leave the table (references to them break).
* `Evaluation::imported_face_ids` exposes the valid maps, for picking a face in the UI
  and for the `imported_faces` command.
* Native format **v6**, written only when a `DirectEdit` exists (`io::v6_feature`);
  v5/v4 builds refuse such a file. Regeneration re-imports the embedded STEP blob and
  replays the edits, so save/reopen and undo/redo need no source file.

## Consequences

* The FreeCAD fixture `direct_edit_block_mm.step` (tools/fixtures/step/direct_edit.py)
  is edited exactly; volumes match the analytic values to 1e-6 relative in the kernel and
  command tests, and FreeCAD re-reads every edited export as one valid solid with probes
  on both sides of each edited face (`tools/check_direct_edit.sh`). FreeCAD's own volume
  of these exports is ~1e-6 relative off even for the unedited re-export (it builds
  approximate pcurves for the planar faces' circular edges), so that check uses 5e-6.
* Our STEP writer emits imported cylinders as `SURFACE_OF_REVOLUTION`, which our own
  importer does not read yet: an edited body cannot be re-imported into R-CAD (FreeCAD
  reads it). Recorded as a gap. Attempted (run 6, not kept): reading
  `SURFACE_OF_REVOLUTION` (LINE or B-spline generatrix, AXIS1_PLACEMENT) as a Truck
  revolved curve with the generatrix reversed for the ISO normal. The re-imported
  `direct_composed` export then measured the right exact volume and classified its holes
  and boss correctly, but its tessellation at chord 0.01 had 128 misoriented / 128
  non-manifold edges (mesh volume 53344.2 vs 53550.7), so one revolved face meshes
  reversed; anchoring the generatrix elsewhere made it worse. Repro: export the
  `edits_compose_and_export` body, import it with that branch, `check_mesh(tessellate(..,
  0.01))`. Left unsupported rather than import a body that meshes wrong.
* Direct edits must come before modeled features on an imported body.

## Addendum: countersink (task 2, same machinery)

`rcad_kernel::countersink(body, axis_point, axis, hole_d, d, angle)` inserts an exact cone
at the entry of an existing round hole: the entry-loop vertices move `h = (R − r) /
tan(angle/2)` down the hole wall, the entry-loop arcs are translated with them, their
radial images (`R / r`, affine) become the new inner loop of the entry face, and each
entry arc gets one conical face, the homotopy of its lower and upper arcs (identical
knots and weights, so every generator is a straight line of the exact cone; the kernel
test samples it on a 21 x 21 grid to 1e-9). Wires of compressed faces are stored as for a
non-inverted face, so the effective (outward) loop is reversed when a face's
`orientation` is false; the new faces respect that. Refused: hole wall not reaching
deeper than `h`, countersink not clear of the entry face's other edges (exact distance),
other faces meeting the entry loop, free-form entry edges. `FeatureKind::Countersink
{ hole, diameter, angle }` applies it to a Hole feature's hole (its last tool's frame and
circle); counterbored holes are refused; patterns/mirrors of the hole do not copy it.

Independent check: `tools/check_direct_edit_expected.sh` has FreeCAD build each expected
solid from Part primitives (`makeCone` for countersinks) and computes the symmetric
difference with the exported STEP: 0.000 mm³ for every direct edit and both countersink
exports, and the control (the countersunk export against the block without the
countersink) differs by the countersink volume (269.0 vs 268.08 mm³ analytic, OCC's
own integration error on these exports).
