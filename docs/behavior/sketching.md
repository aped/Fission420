# Sketch editing behaviour (run 2)

This is the behaviour implemented in `crates/f420-app/src/sketch_edit.rs` (logic),
`sketch_view.rs` (drawing) and `view3d.rs` / `panels.rs` (input routing and panels). It is an
original specification for this product, not a description of any other CAD system.

## Entering and leaving a sketch

| action | how |
|---|---|
| New sketch on world XY | toolbar **Create Sketch** with no face selected, or **New Sketch on XY** in the empty state (creates a document first if none is open) |
| New sketch on a cap face | select the extrusion's end/start cap in the 3D view (or use the extrude's inspector buttons **Sketch on end cap** / **Sketch on start cap**), then **Create Sketch**. Only extrusion caps carry a semantic reference (`FaceRef{feature, role}`); other faces explain why they cannot host a sketch |
| Edit an existing sketch | double-click it in the browser, its context menu **Edit Sketch**, or the inspector button |
| Finish | **Finish Sketch**: one `create_sketch` (template `custom`) or `edit_sketch` command = **one document undo step**. The session stays open until the reply arrives; if the document rejects it, the work is kept and the error is shown |
| Cancel | **Cancel Sketch**: discards the session; the document was never touched |

The camera turns to look along the plane normal and frames the sketch (F re-frames). The body
is dimmed, the sketch plane is shown as a translucent patch with red/green local axes, and the
GPU grid moves onto the sketch plane with adaptive spacing ({1, 2, 5}×10ᵏ mm, ≥ 14 points per
cell). The previous camera is restored on Finish/Cancel.

In sketch mode the **left button belongs to the tool**; camera navigation is middle-drag
(orbit), right-drag (pan) and wheel (zoom toward the pointer), so drawing and camera drags never
compete. Opening another document while a sketch is open discards the session with a message.

## Tools (keyboard shortcut)

| tool | gesture |
|---|---|
| Select (S) | click selects (Shift adds/removes); drag a vertex or a circle rim to move it; click an already-selected dimension (or double-click it) to edit; Del deletes the selection |
| Line (L) | click points to draw connected lines; clicking the chain's first point closes the loop; Enter or right-click ends the chain |
| Center Rect (R) | click the centre (snap to the origin for a centred rectangle), then a corner. Creates 4 lines with Horizontal/Vertical constraints and a symmetry constraint about the centre point |
| 2-Point Rect (Shift+R) | opposite corners; Horizontal/Vertical constraints |
| Circle (C) | centre, then a rim point |
| 3-Point Arc (A) | start point, end point, then a point the arc passes through (counter-clockwise storage; orientation follows the third point). Collinear points are refused |
| Dimension (D) | click a line (horizontal line → `width`, vertical → `height`, other → aligned `length`), a circle (`diameter`), an arc (`radius`), or two points (horizontal or vertical distance, whichever is larger). The inline editor opens immediately |

Constraint buttons act on the selection and are disabled with the reason as hover text:
Horizontal / Vertical (lines, glyph `H`/`V`), Parallel / Perpendicular (two lines, `//` / `90`),
Equal (two lines → equal length, two arcs/circles → equal radius, `=`), Coincident (two points,
`C`), Midpoint (a line and a point: the point stays at the line's midpoint, `Mid`), On line (a
line and a point: the point stays on the line's infinite extension, `On`), Tangent (a line and
an arc sharing an endpoint, `T`). **Construction** (X) toggles selected curves to reference
geometry: drawn dashed, still solved and dimensionable, ignored by profiles. Features are deleted from the browser context menu, the inspector or
Del (refused, with the dependents named, while anything uses them; one undo step).

## Snapping (deterministic priority)

All tolerances are **screen-space** (egui points) and converted to sketch millimetres with the
local scale at the cursor, so behaviour does not change with zoom:

1. **Existing point** within 10 pt (nearest wins; ties by point id). The new geometry *reuses*
   that point id, i.e. the endpoints are shared — an implicit coincidence that is visible as
   one merged vertex. The origin is snappable; hidden construction points (rectangle centres)
   are not.
2. **Horizontal / vertical alignment** with the previous point of the gesture within 8 pt. The
   new line gets a persistent Horizontal/Vertical constraint, drawn as an `H`/`V` glyph. If the
   constraint would be redundant (for example the closing edge of a loop), the line is added
   without it. A point snap that is exactly level/plumb also gets the constraint.
3. **Grid**: the position rounds to the visible grid (position only, no constraint).

Shift disables all snapping. The cursor shows the snap target (ring = point, dot = grid) and the
rubber band shows the live length / size / radius.

## Dimensions and constraints

* New constraints and dimensions are applied to a copy and solved first. If the copy does not
  solve, or the new item is redundant (over-constrains), it is **rejected** and the message
  names the conflicting constraints, e.g. "Rejected: this would over-constrain the sketch
  (redundant with: width = 80 mm (horizontal P1 to P2); …)". The sketch is unchanged.
* The inline editor opens with the value selected, so typing replaces it. **Enter** applies,
  **Escape** cancels (a freshly created dimension keeps its measured value). Invalid input
  ("abc", ≤ 0) or a value the solver cannot satisfy keeps the editor open with the error; the
  sketch keeps its last valid state.
* Dimension names are unique per sketch (`width`, `width2`, …) and become document parameters
  editable in the inspector and through the annotations in the 3D view.
* Keyboard shortcuts never fire while a text field has focus.

## Drags

A drag never bypasses constraints:

1. pin the dragged point (or circle radius) at the target and solve;
2. if that conflicts, try pinning only x or only y (slide along a constraint) and keep the
   closer result;
3. otherwise leave the sketch unchanged and say which constraints hold it
   ("Cannot move: held by width = 80 mm …; delete a constraint or dimension to free it").

The solve runs on the UI thread (typical step 2–5 µs, blocked-with-diagnosis ~0.12 ms; see
`cargo run --release -p f420-app --example drag_latency`). Escape during a drag restores the
start state; a completed drag is **one local undo step**.

## Status and profile

The overlay chip and inspector show the state as symbol + words + colour:
`✔ Fully constrained (DOF 0)`, `~ Under-constrained: N degree(s) of freedom`,
`✖ Conflicting constraints: …` / `✖ Over-constrained (redundant): …`. White geometry means fully
constrained, blue means degrees of freedom remain. The inspector also reports the profile:
"closed loop, 5 edges, area 2350 mm²", "circle Ø10 mm", or why it is not extrudable yet.

Sketches whose geometry is not a single closed loop (lines and arcs) or a single circle — open
chains, extra curves, self-intersections, zero-length edges — are **kept** when finished, are
marked `open` in the browser, and are refused by Extrude / Cut / Revolve with the reason. Nothing
is auto-closed.

## Regions (several loops, islands) — run 4

A sketch may hold several disjoint closed loops (loops may not touch or cross; each connected
group of curves, and each circle, must close on its own). Every loop bounds one **region**: the
loop minus the loops directly inside it. Loops nest to any depth and there may be several outer
loops, e.g. two concentric rectangles with a circle inside the inner one give three regions: the
ring (outer minus inner rectangle), the inner rectangle minus the circle, and the circle.

* **Default (no region picked):** unchanged from run 3: one outer loop with one level of holes is
  extruded whole; islands inside holes and several separate outer loops are refused with a
  reason ("pick one region").
* **Picked region:** Extrude can take one region. Its identity (`RegionRef`) is the sorted
  entity ids of its outer loop and, for each direct hole, the sorted entity ids of that hole:
  `{"outer":[1,2,3,4],"holes":[[5,6,7,8]]}`. It is never a list index or a click position.
  Dimension edits keep entity ids, so the same region regenerates with the new sizes. The identity
  changes when a bounding loop gains, loses or replaces an entity, or a hole directly inside the
  region is added or removed; a reference to the old identity then fails with `BrokenReference`
  "region … no longer exists" and the edit that caused it is refused (transaction). Another
  region is never substituted. Removing an island only breaks the region that had it as a hole
  (the region outside, whose direct hole is the loop around the island, is unchanged).
* **Join / Cut** of a region with holes are refused (`ProductLimit`, Truck 0.6 booleans with
  inner-loop tools); a single-loop region (e.g. an island) joins and cuts normally.
* **UI:** the Extrude dialog shows a "Region" list (outer-loop entity ids − hole ids, area,
  nesting depth) when the sketch has more than one region, plus "Extrudes: …". A click on the
  sketch plane inside a region in the 3D view picks it; the picked region is shaded amber on the
  sketch plane and the preview prism shows its holes. A sketch whose whole profile is refused
  only because of extra loops can still be extruded (pick a region).
* API: `Sketch::regions()` (each with `RegionRef`, depth, area), `Sketch::region_by_ref`,
  `Sketch::region_at(point)`; tests `crates/f420-sketch/src/tests_regions_pick.rs`,
  `crates/f420-commands/tests/regions.rs`, `crates/f420-app/tests/region_pick_ui.rs`.

## Offset (Offset tool, `O`) — run 4

Offsets a closed loop of lines and arcs, or a circle, by a signed distance (+ outward, away from
the enclosed area; − inward). In the editor: Offset tool (`O`), click any curve of the loop, type
the distance in the inline entry (a dashed preview shows the copy, or the reason it is refused),
Enter. One undo step.

* **Geometry (exact):** lines move parallel; arcs stay concentric (the copy **shares the source
  centre point**) with radius ± distance; a circle becomes a concentric circle. Joints that are
  tangent (G1) in the source are offset along the common normal and stay tangent. **Sharp joints
  are mitred** (the two offset curves are extended or trimmed to their intersection): convex
  corners of an outward offset become sharp corners (no rounding is added); concave corners are
  trimmed. Rectangle ± d therefore gives (w ± 2d)(h ± 2d); a rounded rectangle keeps rounded
  corners with radius r ± d.
* **Refused (nothing added):** distance 0; an arc or circle whose radius would reach zero (e.g.
  inward past a corner radius — corners are not sharpened automatically); an edge or arc that
  would vanish or reverse (distance too large for a feature, e.g. inward by more than half an arm
  width); sharp joints whose offset curves no longer meet; a copy that self-intersects or touches
  another loop. A collapsed or self-intersecting profile is never emitted. Loops must join
  through shared points (not `Coincident` pairs); construction curves are not offset.
* **Editable distance — solver constraint, not a one-off copy:** one `Offset` constraint holds
  the copy with a single named dimension (default `offset`, `offset2`, …), editable like any
  dimension and bindable to a parameter (e.g. `lip_wall`). Every copy vertex gets exactly two
  equations: at a tangent joint `p' = p + d·n` (n: the source line's normal or the arc's radial
  direction), at a sharp joint one per incident curve (distance from the source line = d, or
  distance from the arc centre = source radius ± d); the copy arcs' own equal-radius equations are
  dropped because the vertex equations imply them. So the copy adds **no degrees of freedom and
  no redundancy**, and it follows both distance edits and source edits (e.g. the source
  rectangle's width bound to a parameter). Chosen over "copy + Distance/Radius dimensions per
  edge" because that needs one dimension per edge (names must be unique, so a single parameter
  could not drive them) and produces redundant equations at tangent joints.
* With the copy the sketch has two loops: the default profile is the ring between them (outer
  minus inner); pick the inner region to extrude or cut the opening (see Regions).
* Tests: `crates/f420-sketch/src/tests_offset.rs` (analytic areas: rectangle ± d, L shape
  concave/convex, slot, rounded rectangle, circle; edits; refusals), the finite-difference check
  `solver.rs::offset_rows_match_finite_differences`, `crates/f420-commands/tests/offset_lip.rs`
  (enclosure lip opening = lip outline offset by −`lip_wall`; lid volume = `lid_volume()` for the
  baseline and the variant), `crates/f420-app/tests/sketch_offset_ui.rs`.

## Sketch corner fillet (Fillet tool) — run 4

A **sketch** corner fillet: it rounds a 2D corner of a sketch. It is not a solid-edge fillet
(Truck 0.6 has no fillet operation; solid fillets remain unavailable). Fillet tool (toolbar),
click the corner point where two lines meet, type the radius in the inline entry (dashed preview
of the arc, or the refusal reason), Enter. One undo step.

* Construction for interior angle θ: tangent distance t = r / tan(θ/2) along both lines; both lines
  are shortened to the tangent points; a tangent arc is added with `Tangent` constraints to both
  lines and a named `Radius` dimension (`fillet`, `fillet2`, …; editable, bindable to a
  parameter).
* The original corner point stays as a **construction point held on both lines** (`PointOnLine`),
  so dimensions and constraints on the corner (e.g. a width between corners, a fixed origin
  corner) keep their meaning: they measure the virtual sharp corner. A fully constrained sketch
  stays fully constrained.
* Refused (sketch unchanged): a point that is not the shared end of exactly two lines (arcs,
  circles, free points, three curves), collinear lines, radius ≤ 0, and a radius that does not fit
  (t must be shorter than both lines, measured to their other ends — two fillets on one side
  share its length).
* Tests: `crates/f420-sketch/src/tests_fillet.rs` (areas: right corners r²(1 − π/4) each, an
  oblique corner t·r − (π − θ)r²/2; full constraint kept; width and radius edits; refusals),
  `crates/f420-commands/tests/sketch_fillet.rs` (rounded plate extruded: (w·h − (4 − π)r²)·d,
  regenerated after width/radius edits), `crates/f420-app/tests/sketch_fillet_ui.rs`.

## Offset construction planes — run 4

"Sketch on offset plane…" (toolbar): choose the base (World XY / XZ / YZ, or the face selected
when the dialog opened — the default when one is selected) and the distance; OK opens the sketch
editor on the base moved along its normal by the distance (a face's outward normal; +Z, −Y, +X
for the world planes; negative values go the other way). The distance is the sketch's
`plane_offset` parameter: edit it in the inspector or bind it to an expression (e.g. a standoff
`floor + 3` above a cavity floor). The plane follows the base face when the model changes; if the
face no longer resolves, regeneration reports `BrokenReference` (never another face), and the
face's feature cannot be deleted while the sketch uses it. Mirror and Hole accept the same plane
form through the command API. Tests: `crates/f420-commands/tests/offset_plane.rs`,
`crates/f420-app/tests/offset_plane_ui.rs`.

## Undo model

* Inside a sketch: **Undo step / Redo step** (Ctrl+Z / Ctrl+Shift+Z) walk a local snapshot
  stack of the session (each committed tool action, constraint, dimension or drag is one step).
* In the document: the whole session is one entry ("Create sketch" or "Edit sketch Sketch1").
  Dimension edits made outside sketch mode are one entry each ("Set F1.width = 100").

## Template sketches and old files

Run-1 template sketches (dimensioned rectangle / circle, and old `.f420.json` files) open in the
same editor. They keep their fixed anchor point, so after an edit they still expose `center_x` /
`center_y` next to their dimensions; feature ids and downstream references are unchanged.
Editor-made sketches have an explicit fixed origin point instead (no `center_*` parameters).
