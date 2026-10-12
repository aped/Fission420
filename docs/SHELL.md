# Shell (scoped)

Status: run 4. A **scoped** shell, not a general one: it hollows one kind of body (a New Body
extrusion of a single closed profile) into walls and a floor of one thickness, open at one cap.
It is the enclosure-base shortcut done honestly, with the limits below stated and enforced.

## What it does

`FeatureKind::Shell { extrude, open, thickness }` (command `shell`, UI: select a New Body
extrusion → inspector **Shell…**):

1. The extrusion's profile (one closed loop of lines/arcs, or a circle) is offset **inward** by
   `thickness` with the sketch offset geometry (`f420_sketch::offset_profile`, see
   docs/behavior/sketching.md "Offset": parallel lines, concentric arcs, tangent joints stay
   tangent, sharp corners mitred).
2. That inner profile is cut from the `open` cap (`ExtrudeEndCap` or `ExtrudeStartCap`) into the
   material, `distance − thickness` deep, through the same checked path as an Extrude Cut
   (`apply_tool`, ADR 0005 result check: the result must equal the analytic boolean).

So every side wall is `thickness` thick (measured perpendicular to the original wall; rounded
corners become concentric arcs of radius r − t) and the floor is `thickness` thick. Parameter
`thickness` (mm) is editable and bindable to an expression; the extrusion's own parameters
(distance, sketch dimensions) still drive the body, and the shell follows on regeneration.

## Refused (ProductLimit, document unchanged)

* `extrude` is not a New Body extrusion (a Join, Cut, through-all, or another feature kind).
* The profile has inner loops (holes) or the extrusion uses a picked region.
* `thickness ≥ distance` (no floor would remain).
* The thickness does not fit the profile: the inward offset collapses (an edge vanishes or
  reverses — e.g. thickness ≥ half the smallest width — or a corner arc's radius reaches zero:
  thickness ≥ a corner radius; corners are not sharpened automatically) or self-intersects.
* `open` is not one of the two caps. If the cap no longer resolves to exactly one planar face of
  the body when the shell regenerates (a feature before the shell changed it), the shell fails
  with `BrokenReference`.

## Not a general shell

* Only the extrusion's own side walls and the far cap are offset. Features **after** the shell
  apply normally to the hollow body (a hole through a wall, a boss on the floor); features
  **before** the shell that changed the body (joins, cuts, chamfers on that body) are not
  offset: wall thickness equals `thickness` only where no other feature changed the walls.
* No multi-face removal, no thickening outward, no variable thickness, no shell of revolved,
  imported or mesh bodies.

## Evidence

`crates/f420-commands/tests/shell.rs`: 80×50×25 open-top shell t = 2 equals the enclosure base
19592 mm³ with point probes of the four walls, floor, cavity and open top; parameter edits to
100×65×30, t = 2.4 (36823.296 mm³) with probes; undo/redo; start-cap opening; a rounded
rectangle 60×40 r 6 shell matching w·h − (4 − π)r² for outer and offset profiles; refusals
(thickness ≥ distance, walls meeting, corner radius collapse, inner loops, non-New-Body
source). `crates/f420-app/tests/shell_ui.rs`: Shell… in the inspector, thickness edited from
the inspector.
