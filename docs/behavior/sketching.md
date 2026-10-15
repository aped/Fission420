# Sketch editing behaviour

This is the behaviour implemented in `crates/rcad/src/sketch_edit.rs` (logic),
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
| Select (S) | click selects (Shift adds/removes); drag a vertex or a circle rim to move it (an ellipse rim drags its minor radius); click an already-selected dimension (or double-click it) to edit; Del deletes the selection |
| Line (L) | click points to draw connected lines; clicking the chain's first point closes the loop; Enter or right-click ends the chain |
| Center Rect (R) | click the centre (snap to the origin for a centred rectangle), then a corner. Creates 4 lines with Horizontal/Vertical constraints and a symmetry constraint about the centre point |
| 2-Point Rect (Shift+R) | opposite corners; Horizontal/Vertical constraints |
| Circle (C) | centre, then a rim point |
| 3-Point Arc (A) | start point, end point, then a point the arc passes through (counter-clockwise storage; orientation follows the third point). Collinear points are refused |
| Ellipse (toolbar) | centre, then the end of one axis, then a point whose distance from that axis is the other radius. See [Ellipses and splines](#ellipses-and-splines) |
| Spline (toolbar) | click control points; Enter or right-click finishes an open spline (≥ 4 points), clicking the first point again closes it (≥ 3). Nothing is added before it is finished |
| Trim (T) | click the span of a line, arc or circle to remove: it is cut at the nearest crossings with other curves either side of the click (a curve with no crossing is deleted). A line cut in the middle splits in two (the new piece gets a fresh id and the first one's Horizontal/Vertical); a circle becomes an arc with the same id and centre (its diameter dimension becomes a radius); an arc cut in the middle splits into two arcs with an equal-radius constraint. Cut ends reuse an existing point at the crossing, else are held on the cutting curve (`PointOnLine` / `PointOnCircle`). Constraints that no longer apply are removed and counted in the status line (equal length, a length dimension spanning a split line, a tangency no piece still touches) |
| Extend (E) | click a line near its free end: the end moves along the line to the nearest curve its extension crosses; a connected end, an arc, or an extension that meets nothing is refused |
| Break (toolbar) | click the span of a line, arc or circle: it is split at the nearest crossings either side of the click and nothing is removed. Line pieces stay collinear (the far end of each new piece is held on the first piece's line, so the DOF count is unchanged); arc pieces share the centre and get an equal-radius constraint; a circle needs two crossings and becomes two arcs (the clicked span is the new one). Refused when the span has no crossing |
| Dimension (D) | click a line (horizontal line → `width`, vertical → `height`, other → aligned `length`), a circle (`diameter`), an arc (`radius`), an ellipse (first `major_radius`, then `minor_radius`), or two points (horizontal or vertical distance, whichever is larger). The inline editor opens immediately |

Trim, Extend and Break are `rcad_sketch::Sketch::{trim, extend, break_at}`; the editor and the
headless `sketch_op` command (docs/COMMANDS.md) run the same code, then solve; a change that does
not solve, or that collapses a curve to zero size (e.g. Horizontal plus Vertical on one line), is
refused and the sketch is unchanged.

Constraint buttons act on the selection and are disabled with the reason as hover text:
Horizontal / Vertical (lines, glyph `H`/`V`), Parallel / Perpendicular (two lines, `//` / `90`),
Equal (two lines → equal length, two arcs/circles → equal radius, `=`), Coincident (two points,
`C`), Midpoint (a line and a point: the point stays at the line's midpoint, `Mid`), On line (a
line and a point: the point stays on the line's infinite extension, `On`), Tangent (a line and
an arc sharing an endpoint, `T`), **Fix / Unfix** (selected points and the points of selected
curves are pinned where they are with `FixedPoint`; when all are already pinned they are
unpinned; never the origin, not projected geometry; glyph `Fix`), **Concentric** (two
circles/arcs with different centre points: centres coincide, 2 equations, `Con`), **Collinear**
(two lines that share no end point: the second's ends lie on the first's line, 2 equations;
lines joined end to end use Parallel; `Col`), **Symmetric** (two points and a line: the points
mirror each other about the line's infinite extension, 2 equations, `Sym`; two points that are
close to each other and near the line can solve onto the line itself, which the zero-size guard
refuses), and **Driven / Driving** (selected dimension, see Driven dimensions). **Construction** (X) toggles selected curves to reference
geometry: drawn dashed, still solved and dimensionable, ignored by profiles. Features are deleted from the browser context menu, the inspector or
Del (refused, with the dependents named, while anything uses them; one undo step).

## Typed input while drawing (Tab)

While a Line, rectangle or Circle gesture is in progress (after its first click), **Tab** opens
heads-up fields next to the cursor: Line → Length (mm) and Angle (deg, counter-clockwise from +x);
Center / 2-Point Rect → Width and Height; Circle → Diameter. Tab cycles the fields; **Enter**
places the next point from the typed values (an empty field keeps the cursor's value; a rectangle
keeps the cursor's quadrant) and adds a dimension for each typed length (length, width, height,
diameter: the Dimension tool's names); a typed angle places the point only. Values accept units and
parameter expressions as in the dimension field (the dimension takes the value; bind it afterwards by
typing the expression into the dimension). **Escape** closes the fields. Invalid input keeps them
open with the reason and adds nothing.

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
  solve, or a new constraint is redundant (over-constrains), it is **rejected** and the message
  names the conflicting constraints, e.g. "Rejected: this would over-constrain the sketch
  (redundant with: width = 80 mm (horizontal P1 to P2); …)". The sketch is unchanged.
* **Driven dimensions**: a new *dimension* that is redundant (the sketch already fixes that
  distance, diameter, radius or angle) is added **driven** instead of being refused: it adds no
  equation, shows its measured value in parentheses, follows the geometry after every solve
  (horizontal/vertical distances are signed), and cannot be edited or set as a parameter
  (refused by name). A dimension whose value conflicts (inconsistent) is still refused.
  `toggle_driven` (editor) turns a driven dimension driving only when it is no longer redundant.
  Stored as `"driven": true` on the dimension (absent = driving, so older files load unchanged).
* **Branch-flip guard**: a typed dimension value is solved directly; if an arc's sweep jumps by
  more than 90° (its centre crossed the chord: a different shape) or a line reverses direction,
  the edit is re-solved by continuation in 16 steps; if a step still jumps, the edit is refused
  ("would flip arc N"), the field stays open and the sketch is unchanged (`rcad_sketch::guard`).
* The inline editor opens with the value selected, so typing replaces it. **Enter** applies,
  **Escape** cancels (a freshly created dimension keeps its measured value). Invalid input
  ("abc", ≤ 0) or a value the solver cannot satisfy keeps the editor open with the error; the
  sketch keeps its last valid state.
* **Dimension entry** (the inline field) accepts:
  * a number with an optional unit: `25`, `25 mm`, `2.5 cm`, `1 in` (lengths, stored in mm);
    `30`, `30deg`, `30°`, `0.5 rad` (angles, stored in degrees);
  * an expression over user parameters (`w/2`, `w + 2 mm`, `max(w, 30)`; docs/PARAMETERS.md):
    the dimension takes its value now and is **bound** to the expression;
  * `name = expr`: creates the user parameter `name` (refused if it exists: type its name to
    use it) and binds the dimension to it.

  A typed number on a bound dimension unbinds it. The parameters and bindings are committed with
  the sketch at Finish in the same undo step (`sketch_op` `replace`; a new sketch is created
  first, so it takes two steps); later parameter edits drive the dimension. Invalid input (an
  unknown name, a unit that does not fit, a result ≤ 0, an existing parameter name) keeps the
  field open with the reason and leaves the sketch unchanged.
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
`cargo run --release -p rcad --example drag_latency`). Escape during a drag restores the
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

## Regions (several loops, islands)

A sketch may hold several disjoint closed loops (loops may not touch or cross; each connected
group of curves, and each circle, must close on its own). Every loop bounds one **region**: the
loop minus the loops directly inside it. Loops nest to any depth and there may be several outer
loops, e.g. two concentric rectangles with a circle inside the inner one give three regions: the
ring (outer minus inner rectangle), the inner rectangle minus the circle, and the circle.

* **Default (no region picked):** one outer loop with one level of holes is
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
  `Sketch::region_at(point)`; tests `crates/rcad-sketch/src/tests_regions_pick.rs`,
  `crates/rcad-commands/tests/regions.rs`, `crates/rcad/tests/region_pick_ui.rs`.

## Sketch text

A sketch can hold text items (`Sketch.texts`) next to its lines/arcs/circles. Each glyph contour
is a closed loop of the sketch and takes part in region extraction like any other loop.

* **Font.** A family name (`Liberation Sans`, case-insensitive, with bold/italic) is looked up in
  the installed fonts (`RCAD_FONT_DIRS`, `~/.local/share/fonts`, `~/.fonts`,
  `/usr/local/share/fonts`, `/usr/share/fonts`; no fontconfig), or an explicit font file path is
  given. A missing family is an error, never a silent substitute. TrueType (quadratic) and CFF
  (cubic) outlines are read with `ttf-parser`. Layout: one line, advance widths only (no kerning,
  ligatures, complex-script shaping or right-to-left). Control characters (line breaks) and
  characters the font has no glyph for are refused, naming them.
* **Stored outlines.** The glyph outlines (exact Béziers in font units), pen positions, cap
  height and the font's family/style/file name/FNV-1a 64 hash are stored in the document. Reload,
  regeneration and placement edits never need the font. Changing the string or font needs it
  installed; without it the edit is refused ("font not found") and the text is unchanged.
* **Height = cap height.** `height` mm is the font's cap height (OS/2 `sCapHeight`, else the top
  of `H`): a 10 mm `B8` has 10 mm tall capitals/digits. Anchor: `position` or a sketch point
  (`anchor`, follows the solved point); `halign` Left/Center/Right of the total advance, `valign`
  Baseline/Middle (half cap height)/Top (cap height); `rotation` radians CCW about the anchor.
* **Geometry.** Contours become polygons within 0.01 mm of the font curves (docs/TOLERANCES.md);
  the solids are exact extrusions of those polygons. Counters (holes of `B`, `8`, `O`, `A`) are
  found by geometric containment, not by the font's winding: `B` is one region with two holes and
  each counter is an island region. Text inside a rectangle gives the plate region (rectangle
  minus glyph outlines), the glyph regions and the counter regions.
* **Refusals.** Self-intersecting contours, contours that touch or cross other loops (overlapping
  letters, overlapping contours in some variable fonts, text crossing a sketch line) make
  `regions()` fail with the glyph named; such text can be moved or resized.
* **Identity.** A glyph loop's identity is (text id, character position, contour index), so a
  picked glyph region survives moves, height/rotation edits and solving, and string edits that
  keep that character's position and contour count; changing `8` to `9` (two counters to one)
  breaks references to that glyph region (`BrokenReference`, edit refused). Side faces of extruded
  text are named per polygon edge and break when the edge count changes (e.g. a height edit).
* API/commands: `Sketch::add_text` / `edit_text` / `remove_text` / `text_loops`,
  `rcad_sketch::font`; commands `add_sketch_text`, `edit_sketch_text`, `remove_sketch_text`
  (docs/COMMANDS.md). Tests: `crates/rcad-sketch/tests/text.rs`,
  `crates/rcad-commands/tests/sketch_text.rs`, `crates/rcad-commands/tests/text_emboss.rs`
  (raised/recessed "B8" on an enclosure top face in one multi-region feature, membership
  probes of strokes and counters, string edits that re-resolve or are refused by region name)
  (need the installed font Liberation Sans;
  `RCAD_SKIP_FONT_TESTS=1` skips them explicitly).

## Slot and regular polygon (Slot / Polygon tools)

Both tools add ordinary lines/arcs with constraints, so the result stays editable like drawn
geometry (`crates/rcad-sketch/src/shapes.rs`).

* **Slot** (centre to centre): click the first arc centre, the second, then a point beside the
  centre line (its distance is half the width). Adds a bottom line, an end arc, a top line and an
  end arc joined by four `Tangent` constraints and `EqualRadius`, with dimensions `slot_length`
  (aligned distance between the arc centres, L) and `slot_width` (between the two tangent points
  at the first end, W). Clicking an existing point reuses it as a centre. 3 DOF remain (position
  and direction). Profile area L·W + π(W/2)² (tests: `shapes.rs` unit tests,
  `crates/rcad/tests/sketch_shapes_ui.rs` extrudes it to that area × depth).
* **Regular polygon:** set *Sides* (3–64) and *Across flats* in the toolbar, click the centre,
  then a vertex (inscribed: diameter across corners `polygon_d`) or an edge midpoint
  (circumscribed: diameter across flats `polygon_flats`). N lines with vertices on a
  construction circle (`PointOnCircle`) and N−1 `EqualLength`; circumscribed adds an inner
  construction circle carrying the dimension and the first edge's midpoint on it. 3 DOF remain
  (centre and rotation). Areas (N/2)R² sin(2π/N) and N a² tan(π/N) are checked in the tests.
* Center rectangle already existed (Center Rect tool).

## Ellipses and splines

Free-form sketch curves (`crates/rcad-sketch/src/curves.rs`). Both are exact: the profile
carries rational cubic Bézier spans (`LoopSegment::Cubic`), the kernel builds each span as a
single-span NURBS edge, and STEP writes the rational / polynomial B-spline curves and the
extruded B-spline surfaces (checked by FreeCAD/OpenCascade: `tools/check_step_curves.sh`).
No polyline ever stands in for the geometry.

* **Ellipse** (`Entity::Ellipse`): centre point, axis end point (its distance from the centre is
  that semi-axis, its direction the orientation) and the other semi-axis `minor` (a solver
  unknown like a circle radius): 5 DOF. Dimensions: `EllipseRadius` (`major`: |axis end −
  centre|; otherwise `minor`). A full ellipse is a closed loop of its own (four exact quarter
  spans, so an extruded ellipse has four side faces where Fusion shows one). Area πab.
* **Spline** (`Entity::Spline`): uniform cubic B-spline whose control points are ordinary sketch
  points (2 DOF each; fix, dimension or constrain them like any point). Open: clamped, ≥ 4 control
  points, passes through the first and last, which join lines/arcs (shared or coincident
  points) into a closed loop. Closed: periodic, ≥ 3 control points, a loop of its own. Fusion's
  fit-point splines (through points) are not available; this is the control-point spline.
* **Regions**: ellipses and closed splines nest like circles (an elliptical hole in a rectangle
  is a region with one hole). Self-intersection, contact and containment tests use a polyline
  within δ = max(1e-6 mm, 1e-5 × the loop's own size) of the exact curve (convex-hull subdivision
  bound); contact counts within tolerance + δ, so curves closer than δ are refused, never
  silently accepted.
* **Templates**: `centered_ellipse` (`major_radius`, `minor_radius`, `center?`; the major axis is a
  horizontal construction line) and `spline` (`points`, `closed?`), see docs/COMMANDS.md.
* **Refused by name**: point-on-curve, tangent and equal constraints on ellipses/splines;
  Trim / Extend in a sketch that has an ellipse or spline (their intersections are not
  computed); offsetting them; patterning ellipses (splines pattern); projecting them; sheet-metal
  bases with them; sweeping or lofting their profiles; the exact prismatic (slab) boolean (Join /
  Cut use Truck's boolean with the independent result check instead, ADR 0005); side-face
  references to their curved faces; R12 DXF export (no ELLIPSE/SPLINE entity in R12; SVG export
  writes exact paths).
* **Volume**: extrusions use the exact boundary integral (πab·h to ~1e-9 relative).
* **Revolve** of an ellipse or spline profile is refused by name: Truck's mesher fails on many
  revolved rational cubic faces, so the body could not be displayed or exported.
* **Import**: DXF full `ELLIPSE` and `SPLINE`s that are exactly sketch splines (open,
  non-rational, degree 3, clamped uniform knots); SVG `<ellipse>` with rx ≠ ry under similarity
  transforms. Everything else stays refused (docs/formats/dxf.md, svg.md).
* Format: sketches with ellipses / splines are written as native format v8.

Tests: `crates/rcad-sketch/src/tests_curves.rs`, `crates/rcad-kernel/tests/ellipse_spline.rs`,
`crates/rcad-commands/tests/ellipse_spline.rs`, `crates/rcad-exchange/tests/curve_import.rs`,
`crates/rcad/tests/sketch_curves_ui.rs`.

## Sketch patterns (Pattern… action)

Select curves (lines, arcs, circles) and press **Pattern…** in the sketch toolbar: *Rectangular*
(instances and a step vector, optionally a second direction for a grid) or *Circular* (instances,
angle between instances, centre; the origin when the centre is (0, 0)). The copies are ordinary
geometry tied to their sources (`crates/rcad-sketch/src/pattern.rs`):

* `PatternStep {from, to, a, b, k}`: `to − from = k (b − a)`; the step is a construction line
  from the first source point whose length is the `pattern_spacing` dimension (its direction is
  1 free DOF: constrain it horizontal/vertical). Grid copies chain from the first direction.
* `PatternRotate {from, to, center, k, dim}`: `to = c + R(k θ)(from − c)`, θ = `pattern_angle`
  degrees (0 < θ < 360); all copies of one pattern share the one dimension (listed once, edited
  together; a feature parameter like any dimension).
* Copies are exact rigid images, so source constraints are not duplicated; circles copy their
  radius with `EqualRadius`. A copied arc would get one equation too many (the arc implies equal
  radii), so one of its exclusive points (the centre when possible) is tied along one axis only,
  chosen per copy where that equation is best conditioned. Copies follow source edits
  (dimension, drag); a pattern whose copies would overlap fails region extraction like any
  overlapping geometry, and a document edit causing it is refused (transaction).
* Changing the count means deleting the copies and patterning again (no count parameter).
* A sketch with pattern constraints is saved as format v6. Tests: `pattern.rs` unit tests (DOF,
  no redundancy, spacing/angle edits, per-copy axis choice at 90°),
  `crates/rcad-commands/tests/sketch_patterns.rs` (bolt circle, analytic volume, v6, parameter
  edit and refused overlap), `crates/rcad/tests/sketch_pattern_ui.rs` (dialog).

## Offset (Offset tool, `O`)

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
* Tests: `crates/rcad-sketch/src/tests_offset.rs` (analytic areas: rectangle ± d, L shape
  concave/convex, slot, rounded rectangle, circle; edits; refusals), the finite-difference check
  `solver.rs::offset_rows_match_finite_differences`, `crates/rcad-commands/tests/offset_lip.rs`
  (enclosure lip opening = lip outline offset by −`lip_wall`; lid volume = `lid_volume()` for the
  baseline and the variant), `crates/rcad/tests/sketch_offset_ui.rs`.

## Sketch corner fillet (Fillet tool)

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
* Tests: `crates/rcad-sketch/src/tests_fillet.rs` (areas: right corners r²(1 − π/4) each, an
  oblique corner t·r − (π − θ)r²/2; full constraint kept; width and radius edits; refusals),
  `crates/rcad-commands/tests/sketch_fillet.rs` (rounded plate extruded: (w·h − (4 − π)r²)·d,
  regenerated after width/radius edits), `crates/rcad/tests/sketch_fillet_ui.rs`.

## Project inside the sketch (P, Intersect)

* **Project (P)**, second row of the sketch bar: click a model face next to the edge to project;
  the face edge nearest the click is projected into the open sketch (the same associative
  projection as the inspector's Project edge below). **Intersect**: click a body; its section by
  the sketch plane is added. A projected edge comes in as construction (an open curve would make
  the profile fail: profiles need closed loops); a section's closed loops are profile geometry.
  Projected curves are drawn purple, are read-only (Delete, Trim,
  Extend, Break, drags and deleting their pins are refused with the reason) and can be
  dimensioned and constrained to at once. **Projection Link** (shown with the tools, on by
  default) off copies the curves unlinked.
* **Auto-project face edges** (Fusion's default, on): a new sketch on a planar face starts with
  that face's edges projected, as construction so they stay out of profiles. The checkbox (shown
  with the Project tool) turns it off for later sketches.
* **Unlink projection** (shown when a projected curve is selected) breaks its link: the curves
  stay as plain fixed geometry.
* Projection reads the model, so it is a document step: the sketch so far is committed first
  (`create_sketch` or `edit_sketch`), then `project` (and `unlink_projection`). The editor's local
  undo restarts after it (the document's Undo holds those steps). **Cancel Sketch undoes every
  document step the session made**, so cancelling still leaves the document as it was.
* Live probe: `editing.entities` counts drawn (non-projected) curves, `editing.projected` the
  projected ones.

## Projected edges and sections (Project edge / Intersect)

Sketch inspector, **Project geometry into this sketch** (commands: docs/COMMANDS.md `project`):

* **Project edge (pick in view)**, then click a planar face next to the edge: the face edge whose
  projection passes nearest to the click is projected along the sketch normal and stays
  associative. Parallel faces: lines, arcs and circles exactly (a hole rim made of arcs projects as
  one circle). Oblique faces: lines stay lines, a full circle becomes the exact ellipse (its major
  axis along the planes' intersection direction, semi-axes r and r·|cos φ|); an arc there would be
  an elliptical arc and is refused, as is a circle on a face perpendicular to the sketch. An
  upstream edit moves the curve. An edge of an extrusion cap keeps its identity (the profile entity
  that generates it), however far it moves; other edges follow only when one edge is clearly the
  nearest. A vanished, split or ambiguous edge is a broken reference naming the projection.
* **Intersect ‹body›**: the section of the body by the sketch plane at that point of history
  (outline and holes as lines, arcs and circles), associative like face projections. A closed loop
  cut obliquely through a cylinder becomes the exact ellipse; sections that need other curves
  (elliptical arcs, free-form faces) are refused by name, as are planes that hold a face of the body (project that face) or miss it.
* **Unlink** freezes either kind where the last regeneration put it. Format v8.

## Offset construction planes

"Sketch on offset plane…" (toolbar): choose the base (World XY / XZ / YZ, or the face selected
when the dialog opened — the default when one is selected) and the distance; OK opens the sketch
editor on the base moved along its normal by the distance (a face's outward normal; +Z, −Y, +X
for the world planes; negative values go the other way). The distance is the sketch's
`plane_offset` parameter: edit it in the inspector or bind it to an expression (e.g. a standoff
`floor + 3` above a cavity floor). The plane follows the base face when the model changes; if the
face no longer resolves, regeneration reports `BrokenReference` (never another face), and the
face's feature cannot be deleted while the sketch uses it. Mirror and Hole accept the same plane
form through the command API. Tests: `crates/rcad-commands/tests/offset_plane.rs`,
`crates/rcad/tests/offset_plane_ui.rs`.

## Undo model

* Inside a sketch: **Undo step / Redo step** (Ctrl+Z / Ctrl+Shift+Z) walk a local snapshot
  stack of the session (each committed tool action, constraint, dimension or drag is one step).
* In the document: the whole session is one entry ("Create sketch" or "Edit sketch Sketch1").
  Dimension edits made outside sketch mode are one entry each ("Set F1.width = 100").

## Template sketches and old files

Legacy template sketches (dimensioned rectangle / circle, and old `.rcad.json` files) open in the
same editor. They keep their fixed anchor point, so after an edit they still expose `center_x` /
`center_y` next to their dimensions; feature ids and downstream references are unchanged.
Editor-made sketches have an explicit fixed origin point instead (no `center_*` parameters).
