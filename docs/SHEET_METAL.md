# Sheet metal (bounded subset)

Two families exist. The **contour flange** (`sheet_metal`) is documented at the end. The
**sheet body** has a base face, edge flanges, a shared sheet rule, a flat
pattern display state and a flat-pattern DXF. Everything is exact B-rep (planes, exact circular
cylinders); nothing is meshed or approximated.

## Model

A sheet body is defined by its history, not reverse-engineered from a solid:

* `SheetBase` (command `sheet_base`): the closed profile of a sketch (lines and arcs, holes
  allowed) becomes a plate `thickness` thick along the sketch normal (`flip`: against it). It
  stores the **sheet rule** of the body.
* `SheetFlange` (command `sheet_flange`): a straight flange on one **straight edge of the base
  profile**, named by its sketch line entity (stable when the sketch is edited; a deleted line is a
  broken reference, never another edge).
* `SheetFlat` (created by `sheet_flat_state`): the flat pattern as a separate exact solid body.

Arbitrary thin solids are never treated as sheet metal: only bodies made by `sheet_base` have a
rule, flanges and a flat pattern. Sheet features must come before any other edit of the body (a
later cut, fillet, ... is fine; a flange after it is refused with the reason).

### Sheet rule

| field | meaning |
|---|---|
| `thickness` t | sheet thickness (mm) |
| `radius` R | **inside** bend radius of every bend (mm) |
| `k_factor` K | neutral axis at K·t from the inside surface, K in [0, 1] |
| `relief` | `rectangular` or `obround` |
| `relief_width`, `relief_depth` | relief size (default t × t); obround needs depth ≥ width/2 |

Edit it with `edit_sheet_rule` (atomic, any subset of fields) or as feature parameters of the
base (`thickness`, `radius`, `k_factor`, `relief_width`, `relief_depth`) with `set_parameter`,
or bind them to expressions with `set_expression`; flanges likewise (`angle_deg`, `length`,
`start_offset`, `end_offset`). Lengths take length expressions, `angle_deg` angles, and the
K-factor a plain ratio (format v9 when used). Every flange and the flat pattern regenerate from
the rule (`sheet_flanges.rs::sheet_rule_and_flange_lengths_take_expressions`).

### Edge flange conventions (stated)

* **Bend position: outside the base outline.** The bend starts at the selected edge (the edge is
  the bend's tangent line on the plate); the plate is not trimmed.
* **Length = straight leg beyond the bend**, from the bend's tangent line to the flange end,
  measured along the flange. This is the "between tangent points" convention; it is not an outside (mold-line) dimension.
* **Angle** θ in (0°, 180°]: 90° makes the flange perpendicular to the plate; **180° is an open
  hem**: the leg lies back over the plate with a gap of 2R (inside radius from the rule). Closed
  (zero-gap) and rolled/teardrop hems are not implemented.
* **Direction**: up = toward the base sketch normal (default), `down` = away from it. Up bends
  have the plate's top as their inside surface, down bends the bottom.
* **Width**: the whole edge minus `start_offset` / `end_offset` (measured along the edge from its
  start / end vertex in the counter-clockwise profile direction). An offset > 0 cuts the rule's
  **relief** into the plate beside the flange: `relief_width` along the edge, `relief_depth` into
  the plate from the bend line.

Geometry: the flange is the exact section — quarter (or θ) annulus between radii R and R + t plus
the straight leg — extruded along the edge; it is glued to the plate along their shared rectangle
(ADR 0012). Folded volume (checked in tests and by FreeCAD) =
w·t·(Σ straights) + Σ w·θ/2·((R+t)² − R²).

### Refusals (each with the feature and the reason)

* a flat pattern that overlaps itself: the flat outline is checked for any intersection or
  touching of non-neighbouring edges (collinear overlaps and end points on edges included). (An
  area comparison cannot detect overlapping strips: each strip adds exactly its own area to the
  outline's signed area, overlapping or not.)
* a relief that cuts into a hole of the plate (a circular hole is checked exactly: distance from
  its centre to every line and arc of the relief-cut outline; other holes as polygons),
* flange footprints on the base plane overlapping (self-overlapping / colliding flanges; this
  check is conservative for flanges leaning over the plate at > 90°: it refuses any overlap of
  their projections even if their heights would separate them),
* a flange running into the plate (e.g. at a concave corner), a flange reaching a **concave
  corner** without an offset,
* two flanges (with their reliefs) overlapping on one edge,
* **impossible reliefs**: an offset that leaves no room for the relief width, a relief that cuts
  through the plate (the outline would self-intersect),
* bend angle ≤ 0° or > 180°, length ≤ 0, offsets that leave no flange, a line that is not
  a straight edge of the base outline, an invalid rule (t, R, relief ≤ 0, K outside [0, 1]),
* a flange after a non-sheet edit of the body.

A refused command leaves the document unchanged (transaction).

## Flat pattern

Unfolding uses the **bend allowance** BA = θ·(R + K·t) (neutral axis), not the outside surface
length. In the base sketch's coordinates the plate stays in place and each flange unfolds outward
from its edge into a strip of width BA + length. Orientation is the base sketch's.

* **Display state** — `sheet_flat_state {base, flat}`: the first call creates a `SheetFlat`
  feature whose body is the exact flat solid (the flat outline extruded by t from the plate's
  bottom); the call toggles visibility between the folded body and the flat body (one undo step
  each). The flat-pattern feature stays after every flange of its sheet: a new flange is
  inserted before it, and a flange moved after it is refused (it would be out of date). The flat
  body is a separate body: mass properties of both are reported if both are
  selected — hide/omit the flat body for physical totals.
* **DXF** — `export_flat_pattern {feature, path}` with any sheet feature: R12 DXF, `$INSUNITS` 4
  (mm). Layers:
  * `CUT`: outline and holes (LINE / ARC / CIRCLE); collinear outline pieces are merged.
  * `BEND_UP` / `BEND_DOWN`: bend **centre lines** (middle of the bend zone, BA/2 from the edge),
    direction in the layer name.
  * `BEND_NOTES`: one TEXT per bend, e.g. `UP 90.0%%d R2 BA 3.770`.

The response message gives the flat extent and the bends. The export is refused when a non-sheet
feature (hole, cut, ...) changed the body after its last sheet feature: unfolding arbitrary edits
is not supported, and the pattern would silently miss them.

## Acceptance project 6 (U-channel)

`examples/sheet_u_channel_workflow.json` / `examples/sheet_u_channel.rcad.json`: base 60 × 40,
t 1, R 2, K 0.4, 90° up flanges of length 20 on the two 40 mm edges. Straights between tangent
points 60 + 20 + 20; flat length 100 + 2·(π/2)·(2 + 0.4·1) = **107.539822 mm**.
Verified in `crates/rcad-commands/tests/sheet_flanges.rs` and `sheet_surface_acceptance.rs`
(closed-form volume, bounding box, point membership, independent DXF parse; thickness/radius
edit to t 2, R 3; refused self-overlapping flanges) and externally by FreeCAD
(`tools/validate_sheet_surface.sh`: folded and flat STEP volume/bbox/validity, DXF closed CUT
wire area = 40·L, bend lines, `$INSUNITS`; also an open-hem plate).

## Not implemented (tracked)

Closed / rolled / teardrop hems, flanges on flange edges or on arcs, flange position options (material
inside / bend inside), partial flanges with tapered sides, miters / corner closing, lofted or
conical bends, forming tools, K-factor tables per material, bend deduction tables, unfolding of
arbitrary solids, flat-pattern drawings with bend tables, face references to sheet faces for
downstream sketches.

## Contour flange (`sheet_metal`)

`FeatureKind::SheetMetal`: a **contour flange** from an open chain of sketch lines, extruded
`width` along the sketch normal. The chain is one surface of the sheet; material lies to the
**right** of the chain's direction (`flip`: left). Every interior vertex becomes a cylindrical
bend of inner radius `radius`; straights between tangent points (a segment of length L with
setbacks s = r·tan(θ/2) keeps L − s₀ − s₁). Flat length Σ straights + Σ θ(R + K t);
`export_flat_pattern` writes CUT / BEND layers. Refused: arcs/circles/branches/closed chains,
flanges too short for their bends, 180° folds, collinear vertices, self-overlapping sections,
t/R ≤ 0, K outside [0, 1]. Its rule is its own (one feature per body).
