# Headless command API

One API (`crates/f420-commands`, `Session::execute(&Command) -> Response`) is used by the
native app, the CLI, and the tests. Commands are JSON objects tagged by `cmd`.

## CLI

```bash
cargo build --release -p f420-cli
target/release/f420-cli run examples/plate_workflow.json       # JSON lines, one per command
target/release/f420-cli run examples/bracket_workflow.json     # 120x60 bracket, 5 holes, STL+STEP
target/release/f420-cli run examples/scenario_a_workflow.json  # SESSION_2 A: editor sketches, plate + hole
target/release/f420-cli run examples/scenario_b_workflow.json  # SESSION_2 B: chamfered line loop
target/release/f420-cli run examples/scenario_r_workflow.json  # stretch: rounded rectangle + slot (3-point arcs)
target/release/f420-cli acceptance --out artifacts/acceptance  # GOAL.md workflow + scenarios A/B, analytic checks
tools/external_validation.sh artifacts/acceptance               # python STL + FreeCAD STL/STEP checks
```

`run` exit codes: 0 all steps behaved as expected; 1 some step's `ok` differed from its
`expect_ok` (default `true`); 2 unreadable/invalid script; 64 usage. Scripts run from the
repository root (paths are relative to the working directory).
`acceptance` exits 1 if any check fails and writes `acceptance_report.json`; the blind-pocket
stretch steps are prefixed `P.`, the three-point-arc stretch steps `R.`; steps of the
SESSION_2 scenarios are prefixed `A.` / `B.`.

The scenario workflows are generated from `f420_commands::scenarios` (the sketches are built
with the sketch editing API, like the interactive editor). After changing that module:
`F420_WRITE_EXAMPLES=1 cargo test -p f420-commands --test scenario_examples`; without the
variable the same test fails when the committed JSON is stale, and it replays the workflows.

## Commands

| cmd | fields | notes |
|---|---|---|
| `new_document` | | replaces any open document (no prompt; headless) |
| `create_sketch` | `name?`, `plane`, `template` + template fields | see below |
| `extrude` | `name?`, `sketch`, `distance`, `op?` (`new_body` default, `join`, `cut`), `target?` (body id), `through_all?`, `reverse?`, `body_name?` | ADR 0003. `new_body` allocates a body (default name `BodyN`). `join`/`cut` need a target: given, else the body owning the face the sketch is on, else the only B-rep body; ambiguous → `Validation` "choose a target body". Direction: along the sketch normal, except when the sketch plane holds a face of the target: then a cut goes into the material and a join away from it; `reverse` flips the default (refused for blind cut/join from a face). `through_all` (cut only; `distance` ignored): both sides of the sketch plane through the target's extent + margin. Refused: tool missing the target, join from a face whose profile crosses the face boundary (`ProductLimit`), disconnected join result, cut removing nothing, mesh targets (`ProductLimit`). The document keeps the last valid model |
| `revolve` | `name?`, `sketch`, `angle_deg` | new body; revolves about the sketch's local Y axis (WorldXZ → world Z); 0 < angle ≤ 360; profile strictly at x > 0 |
| `cut_through_all` | `name?`, `sketch` | both directions along the sketch normal; circular, parallel to the base extrusion, strictly inside its profile (product limit; for a line/arc host: centre inside the loop and clearance > `MIN_LENGTH` to every line segment and arc); not on revolved bodies |
| `pocket` | `name?`, `sketch`, `depth` | blind cut from the extrusion cap the sketch is on, `depth` mm into the material (against the cap's outward normal). Product limits: circular profile sketched on an end/start cap of the base extrusion, strictly inside it, 0 < depth < thickness (`ProductLimit` otherwise; a through hole is `cut_through_all`). Parameter `depth`; follows the cap when the thickness changes |
| `linear_pattern` | `name?`, `feature`, `count`, `spacing`, `direction` | repeats an Extrude **Join or Cut** (`feature`, instance 0) `count − 1` more times, `spacing` mm apart along `direction` given in the seed sketch's local 2D axes (normalized), on the seed's target body, with the seed's exact tool span. Parameters `count` (integer 2..=100) and `spacing` (mm). An instance that misses the body is refused naming it ("instance k of n"); a joined instance of a face-sketched seed must again lie inside a face (`ProductLimit` otherwise). Instance faces are not referenceable. New Body / body patterns: not yet |
| `circular_pattern` | `name?`, `feature`, `count`, `angle_deg`, `center` | repeats an Extrude Join/Cut `count − 1` more times, each rotated a further `angle_deg` (0 < a < 360) counter-clockwise about the seed sketch's normal through `center` (seed sketch 2D coordinates); refused when the instances reach or wrap past a full turn (`(count − 1) · angle ≥ 360°`, the last copy would land on the seed); parameters `count`, `angle_deg`; same per-instance refusals as `linear_pattern` |
| `mirror` | `name?`, `feature`, `plane` | adds the reflection of an Extrude Join/Cut's tool across `plane` (world XY/XZ/YZ through the origin, or a planar face reference) to the seed's target; same refusals as patterns; the mirrored copy's faces are not referenceable |
| `chamfer` | `name?`, `a`, `b` (face references), `distance` | equal-distance chamfer of the straight convex edge where planar faces `a` and `b` of one body meet (edge named by its faces, never by index). Refused: parallel faces, faces not meeting in an edge, concave edge, distance past either face, or when the cut would remove anything but the exact edge wedge (oblique edge ends, other features within reach) — verified by removed volume. Parameter `distance`. UI: select a face → **Chamfer edge…** |
| `set_parameter` | `feature`, `name`, `value` | sketch: every named dimension (`width`, `height`, `diameter`, or any name an editor sketch gives, e.g. `chamfer`) and, on template sketches only, `center_x`, `center_y`; extrude: `distance` (mm); pocket: `depth` (mm); linear pattern: `count`, `spacing` (mm); chamfer: `distance` (mm); revolve: `angle_deg` (stored in radians). Sketch edits store the **solved** coordinates in the definition |
| `edit_sketch` | `feature`, `sketch` | replace a sketch feature's definition (the editor's Finish Sketch); one undo step; plane and downstream references kept; the sketch is validated and solved first (conflicts are refused, naming the constraints); downstream features must still regenerate, otherwise nothing changes |
| `rename_feature` | `feature`, `name` | 1-128 printable characters, unique among features |
| `rename_body` | `body`, `name` | 1-128 printable characters, unique among bodies; undoable |
| `set_body_visible` | `body`, `visible` | undoable; hidden bodies stay evaluated and measurable; default exports skip them |
| `set_sketch_plane` | `feature`, `plane` | re-attach a sketch (repair of a broken/ambiguous face reference); local sketch geometry kept; one undo step |
| `import_step` | `path`, `units_to_mm?` | exact solids (subset: docs/formats/step.md) → one body per solid, named from the file; source bytes embedded (no path dependency); one undo step; `units_to_mm` only used when the file declares no length unit (else refused with "units required") |
| `import_stl` | `path`, `units_to_mm` (required: STL is unitless) | ASCII/binary → one mesh body (display, hide/show, save, STL export; exact operations and STEP export refused); diagnostics in `summary.bodies[].mesh`; bytes embedded |
| `import_dxf` | `path`, `plane`, `name?`, `units_to_mm?`, `allow_partial?`, `layers?` | 2D subset (docs/formats/dxf.md) → a new unconstrained sketch on `plane`; unitless files need `units_to_mm`; unsupported entities refuse the import unless `allow_partial` (then listed in the message) |
| `import_3mf` | `path` | every build item of a 3MF package (subset: docs/formats/threemf.md) → one mesh body each (units from the file, transforms applied); bytes embedded; colours/extensions reported, not kept |
| `export_3mf` | `path`, `chord?`, `bodies?`, `unit?` (`millimeter` default, `micron`…`meter`) | one object per body (default all visible): B-rep tessellated and checked watertight, mesh bodies as imported; never fused |
| `import_svg` | `path`, `plane`, `name?`, `units_to_mm?`, `allow_partial?` | SVG planar paths (subset: docs/formats/svg.md) → a new unconstrained sketch on `plane`; files without a physical size need `units_to_mm` (mm per user unit); y flipped (SVG y points down) |
| `export_svg` | `path`, `sketch` | the solved sketch's lines/arcs/circles, local 2D coordinates in mm with y negated |
| `export_dxf` | `path`, `sketch` | the solved sketch's lines/arcs/circles in its local 2D coordinates, mm, R12 ASCII |
| `delete_feature` | `feature` | removes a feature nothing depends on, explicitly or as a join/cut target of a body it creates (one undo step); refused naming the dependents otherwise; bodies and embedded files no longer created/used are dropped |
| `undo` / `redo` | | document-level snapshots |
| `save` | `path` | atomic (temp file + fsync + rename); always writes the current format version |
| `open` | `path` | validates format/version/references, regenerates, compares with saved cache |
| `close` | | |
| `export_stl` | `path`, `chord?` (mm, default 0.01), `bodies?` | binary STL of `bodies` (default: all visible bodies), each B-rep tessellated and checked watertight, mesh bodies written as imported; bodies are separate shells in one file, never fused; message names the committed revision |
| `export_step` | `path`, `bodies?` | AP214 via truck-stepio with exact NURBS edge rebuild, one solid per body (default: all visible bodies); mesh bodies refused; refused when an edge can't be written exactly |
| `measure` / `inspect` | | measurements / full `summary` |

### Sketch templates (`create_sketch`)

| `template` | fields | result |
|---|---|---|
| `centered_rectangle` | `width`, `height`, `center?` | run-1 template: fixed anchor point (parameters `center_x`/`center_y`), no `origin` |
| `centered_circle` | `diameter`, `center?` | run-1 template, same anchor rule |
| `polygon` | `vertices` (`[[x,y],...]`, ≥ 3) | editor-style sketch: origin point plus a closed loop of unconstrained lines (vertices stay editable) |
| `custom` | `sketch` (a full `Sketch` object) | a sketch as produced by the interactive editor; validated and solved before it is stored |

`center` = `[x, y]` in the plane's frame, default origin. `plane`: `"WorldXY"`, `"WorldXZ"`
(x = world X, y = world Z) or `{"Face":{"feature":N,"role":"ExtrudeEndCap"|"ExtrudeStartCap"}}`.

A usable profile is one circle, or one closed loop of lines and arcs (shared endpoints or
`Coincident` points, every vertex joining exactly two curves); construction entities are
skipped first. A sketch whose geometry is not
a single closed profile (open polyline or arc chain, extra dangling curve, self-intersection
including arc-line and arc-arc crossings or touching, zero-length line, zero-sweep arc, more
than one loop, lines/arcs mixed with circles) is still created; its
`FeatureSummary.profile_error` says why, and any extrude/revolve/cut of it fails with
`Validation`. Isolated points (such as the origin) are ignored by profile extraction.

`Sketch` JSON (`custom`, `edit_sketch`): `points` (`{id, x, y}` mm), `entities`
(`{"Line":{id,a,b}}`, `{"Circle":{id,center,radius}}`, `{"Arc":{id,center,start,end}}`),
`constraints` (`FixedPoint`, `Horizontal`, `Vertical`, `HorizontalDistance`,
`VerticalDistance`, `Distance` (aligned length, nonlinear), `SymmetricAboutPoint`, `Diameter`,
`Coincident`, `Radius` (`{entity, dim}`, arc or circle), `Tangent` (`{line, arc}`, sharing one
endpoint id), `EqualRadius` (`{a, b}`, arcs/circles), `Parallel` / `Perpendicular` /
`EqualLength` (`{a, b}`, two different lines; parallel includes anti-parallel), `PointOnLine`
(`{point, line}`, the point lies on the infinite line through the line's endpoints and may not be
one of them); dimensions carry `{"name", "value"}`, names unique per sketch), optional `origin`
(the point fixed at (0, 0)), optional `construction` (array of entity ids: construction geometry,
solved and constrained like any entity but ignored by profile extraction, so a construction
centre line or reference circle does not break the profile; omitted when empty).

Arcs: an `Arc` runs counter-clockwise from `start` to `end` about `center` (sweep in
(0, 2π)); all three are ordinary points. The entity always implies
`|start − center| = |end − center|` (part of every solve, never listed as a conflicting
constraint). The editor API builds one with `Sketch::add_arc_3pt(start, end, via)` (centre at
the circumcentre; endpoints stored swapped when `via` is on the clockwise side; refused for
coincident or collinear points or a radius above 1e4 × the span, `SketchError::DegenerateArc`).
Helpers in `f420_commands::sketches`: `rounded_rectangle(w, h, r)` (dimensions `width`,
`height`, `radius`; 8 tangents, the other corners `EqualRadius`), `slot(length, radius)`
(dimensions `length`, `radius`), `offset_circle(d, x, y)`. All fully constrained.

## Response

```json
{"ok": true, "command": "cut_through_all", "feature": 4,
 "measurements": {"shell_count": 1, "all_shells_closed": true, "face_count": 9, "edge_count": 21,
   "vertex_count": 14, "bounds": {"min": [-40,-25,0], "max": [40,25,10]},
   "volume": {"Value": {"volume": 39214.6018248, "error_estimate": 1.2e-5, "method": "BrepBoundaryIntegralZ"}}},
 "summary": {...}}
```

Failures: `"ok": false, "error": {"kind": "Validation"|"ProductLimit"|"BrokenReference"|"Cycle"|"SketchSolve"|"Kernel"|"UnknownFeature"|"UnknownParameter"|"NothingTo"|"File", "detail": ...}`.
A failed command never changes the document (transaction rollback).

Optional fields set only by a queueing front end (the app engine), never by `Session`, and
omitted from JSON when unset: `superseded: true` (the command was skipped because a newer
edit of the same parameter was queued right behind it; `ok` is true and nothing changed) and
`elapsed_ms` (worker time including display tessellation). See `crates/f420-app/src/engine.rs`.

`summary` (`DocSummary`): `doc_id` (changes on New/Open/Close), `open`, `path`, `dirty`,
`revision` (bumped by every commit, undo and redo), undo/redo labels, and per-feature
`FeatureSummary`: parameters, dependencies, plane/frame, solve status/DOF/residual,
`solve_conflicts` (constraint labels), `sketch` (solved geometry), `profile_error`, and for
extrusions the referenceable `faces` with their current frames.

## Native format

`{"format": "fission420", "version": 2, "document": {...}, "cache": {...}}`. Version 2 adds the
optional sketch `origin`, the `Distance` constraint and the `Pocket` feature; later run-2
additions (the `Arc` entity, `Radius` / `Tangent` / `EqualRadius` / `Parallel` / `Perpendicular` /
`EqualLength` / `PointOnLine` constraints, the sketch `construction` set) are serde-additive
and keep version 2, so a v2 file that uses them cannot be read by a run-1 or early run-2 binary
(it fails with a `File` parse error; nothing is overwritten). **Migration:** version-1 files are a
strict subset and load unchanged (identity migration); saving writes version 2. Files claiming
a newer version than the build supports are refused with a `File` error. Stored coordinates
are written with shortest round-trip float formatting and reload bit-exactly.
`crates/f420-document/tests/fixtures/plate_v1.f420.json` is a frozen v1 file used by the
migration test (the `examples/*.f420.json` files are re-saved as v2 by their workflows).
