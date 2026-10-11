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
Horizontal / Vertical (lines), Coincident (two points), Midpoint (a line and a point: the point
stays at the line's midpoint, glyph `Mid`), Tangent (a line and an arc sharing an endpoint),
Equal (two arcs/circles). Features are deleted from the browser context menu, the inspector or
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
