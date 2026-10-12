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
target/release/f420-cli run examples/enclosure_workflow.json       # run 4: parametric enclosure base + lid (docs/PARAMETERS.md)
target/release/f420-cli run examples/clearance_coupon_workflow.json # run 4: peg/hole fit test, 0.15-0.45 mm per side
target/release/f420-cli run examples/mounting_bracket_workflow.json # run 4: L bracket, patterned holes, chamfer
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
| `extrude` | `name?`, `sketch`, `distance`, `op?` (`new_body` default, `join`, `cut`), `target?` (body id), `through_all?`, `reverse?`, `body_name?`, `region?` (`{"outer":[entity ids],"holes":[[entity ids],…]}`, run 4: extrude only that sketch region, docs/behavior/sketching.md "Regions"; absent = the whole profile as before; a region that no longer exists → `BrokenReference`; a region with holes is refused for join/cut) | ADR 0003. `new_body` allocates a body (default name `BodyN`). `join`/`cut` need a target: given, else the body owning the face the sketch is on, else the only B-rep body; ambiguous → `Validation` "choose a target body". Direction: along the sketch normal, except when the sketch plane holds a face of the target: then a cut goes into the material and a join away from it; `reverse` flips the default (a reversed blind cut from a face is refused; a join from a face ignores `reverse`, so older files that set it to point away still load). `through_all` (cut only; `distance` ignored): both sides of the sketch plane through the target's extent + margin. Refused: tool missing the target, join from a face whose profile crosses the face boundary (`ProductLimit`), disconnected join result, cut removing nothing, mesh targets (`ProductLimit`). The document keeps the last valid model. Run 4 (ADR 0005): every Join/Cut result (also pattern/mirror copies, Cut Through All and Pocket) is checked against the nominal operation; a wrong kernel result is refused as `GeometryCheck` (known case: a cut that engulfs an existing hole). A Join from a face overlaps into the support by min(0.5, distance/2, half the clear material depth under the profile); less than 0.002 mm of clear material is a `ProductLimit` |
| `revolve` | `name?`, `sketch`, `angle_deg` | new body; revolves about the sketch's local Y axis (WorldXZ → world Z); 0 < angle ≤ 360; profile strictly at x > 0 |
| `cut_through_all` | `name?`, `sketch` | both directions along the sketch normal; circular, parallel to the base extrusion, strictly inside its profile (product limit; for a line/arc host: centre inside the loop and clearance > `MIN_LENGTH` to every line segment and arc); not on revolved bodies |
| `pocket` | `name?`, `sketch`, `depth` | blind cut from the extrusion cap the sketch is on, `depth` mm into the material (against the cap's outward normal). Product limits: circular profile sketched on an end/start cap of the base extrusion, strictly inside it, 0 < depth < thickness (`ProductLimit` otherwise; a through hole is `cut_through_all`). Parameter `depth`; follows the cap when the thickness changes |
| `linear_pattern` | `name?`, `feature`, `count`, `spacing`, `direction` | repeats an Extrude **Join or Cut** (`feature`, instance 0) `count − 1` more times, `spacing` mm apart along `direction` given in the seed sketch's local 2D axes (normalized), on the seed's target body, with the seed's exact tool span. Parameters `count` (integer 2..=100) and `spacing` (mm). An instance that misses the body is refused naming it ("instance k of n"); a joined instance of a face-sketched seed must again lie inside a face (`ProductLimit` otherwise). Instance faces are not referenceable. New Body / body patterns: not yet |
| `circular_pattern` | `name?`, `feature`, `count`, `angle_deg`, `center` | repeats an Extrude Join/Cut `count − 1` more times, each rotated a further `angle_deg` (0 < a < 360) counter-clockwise about the seed sketch's normal through `center` (seed sketch 2D coordinates); refused when the instances reach or wrap past a full turn (`(count − 1) · angle ≥ 360°`, the last copy would land on the seed); parameters `count`, `angle_deg`; same per-instance refusals as `linear_pattern` |
| `mirror` | `name?`, `feature`, `plane` | adds the reflection of an Extrude Join/Cut's tool across `plane` (world XY/XZ/YZ through the origin, or a planar face reference) to the seed's target; same refusals as patterns; the mirrored copy's faces are not referenceable |
| `chamfer` | `name?`, `a`, `b` (face references), `distance` | equal-distance chamfer of the straight convex edge where planar faces `a` and `b` of one body meet (edge named by its faces, never by index). Refused: parallel faces, faces not meeting in an edge, concave edge, distance past either face, or when the cut would remove anything but the exact edge wedge (oblique edge ends, other features within reach) — verified by removed volume. Parameter `distance`. UI: select a face → **Chamfer edge…** |
| `set_parameter` | `feature`, `name`, `value` | sketch: every named dimension (`width`, `height`, `diameter`, or any name an editor sketch gives, e.g. `chamfer`) and, on template sketches only, `center_x`, `center_y`; extrude: `distance` (mm); pocket: `depth` (mm); linear pattern: `count`, `spacing` (mm); chamfer: `distance` (mm); revolve: `angle_deg` (stored in radians). Sketch edits store the **solved** coordinates in the definition |
| `hole` | `name?`, `plane` (a face of the target, or a world plane holding one), `center` (plane mm), `diameter`, `depth?` (omit: through all), `counterbore?` `[diameter, depth]`, `target?` | run 4: round hole into the material, optional counterbore (wider, shallower), built as counterbore cut then hole cut, each checked (ADR 0005); parameters `diameter`, `depth`, `cbore_diameter`, `cbore_depth`, `center_x`, `center_y` (bindable); `linear_pattern`/`circular_pattern`/`mirror` accept a hole as seed. Countersinks are not supported |
| `shell` | `name?`, `extrude` (a New Body extrusion), `open?` (`ExtrudeEndCap` default, `ExtrudeStartCap`), `thickness` | run 4, **scoped** shell (docs/SHELL.md): the profile offset inward by `thickness` is cut from the open cap to `distance − thickness` deep (checked like a Cut, ADR 0005): walls and floor of `thickness`. Refused (`ProductLimit`): not a New Body extrusion, inner loops / picked region, `thickness ≥ distance`, thickness not fitting the profile (walls meet, corner radius collapses). Parameter `thickness` (bindable). Wall thickness equals `thickness` only where no other feature changed the walls. UI: select the extrusion → **Shell…** |
| `set_user_parameter` | `name`, `expression`, `unit?` (`mm` default, `unitless`, `deg`), `comment?` | create or redefine a named parameter; every bound value is re-derived and the model regenerated in one undo step; errors (`Expression`) name the parameter or bound slot and leave the document unchanged (docs/PARAMETERS.md) |
| `set_user_parameters` | `parameters` (list of `{name, expression, unit?, comment?}`) | several parameters in one transaction / undo step (only the final combination must be valid) |
| `rename_user_parameter` | `name`, `new_name` | rewrites every dependent expression in the same step |
| `delete_user_parameter` | `name` | refused while used (users named) |
| `set_expression` | `feature`, `name`, `expression` | bind a feature parameter to an expression; a plain number sets it and unbinds; `""` unbinds keeping the value. A bound slot refuses `set_parameter` |
| `edit_sketch` | `feature`, `sketch` | replace a sketch feature's definition (the editor's Finish Sketch); one undo step; plane and downstream references kept; the sketch is validated and solved first (conflicts are refused, naming the constraints); downstream features must still regenerate, otherwise nothing changes |
| `rename_feature` | `feature`, `name` | 1-128 printable characters, unique among features |
| `rename_body` | `body`, `name` | 1-128 printable characters, unique among bodies; undoable |
| `set_body_visible` | `body`, `visible` | undoable; hidden bodies stay evaluated and measurable; default exports skip them |
| `set_sketch_plane` | `feature`, `plane` | re-attach a sketch (repair of a broken/ambiguous face reference); local sketch geometry kept; one undo step |
| `import_step` | `path`, `units_to_mm?` | exact solids (subset: docs/formats/step.md) → one body per solid, named from the file; source bytes embedded (no path dependency); one undo step; `units_to_mm` only used when the file declares no length unit (else refused with "units required") |
| `import_stl` | `path`, `units_to_mm` (required: STL is unitless) | ASCII/binary → one mesh body (display, hide/show, save, STL export; exact operations and STEP export refused); diagnostics in `summary.bodies[].mesh`; bytes embedded |
| `import_dxf` | `path`, `plane`, `name?`, `units_to_mm?`, `allow_partial?`, `layers?` | 2D subset (docs/formats/dxf.md) → a new unconstrained sketch on `plane`; unitless files need `units_to_mm`; unsupported entities refuse the import unless `allow_partial` (then listed in the message) |
| `import_3mf` | `path` | every build item of a 3MF package (subset: docs/formats/threemf.md) → one mesh body each (units from the file, transforms applied); bytes embedded; colours/extensions reported, not kept |
| `export_3mf` | `path`, `chord?`, `bodies?`, `unit?` (`millimeter` default, `micron`…`meter`), print fields (below) | one object per body (default all visible; hidden bodies only when listed, named in the message): B-rep tessellated and required to be a valid closed outward solid, mesh bodies as imported (must be valid solids unless `allow_open_meshes`); never fused; geometry only (no Bambu printer/process/filament settings); `separate_files` refused; report in `response.export` |
| `import_svg` | `path`, `plane`, `name?`, `units_to_mm?`, `allow_partial?` | SVG planar paths (subset: docs/formats/svg.md) → a new unconstrained sketch on `plane`; files without a physical size need `units_to_mm` (mm per user unit); y flipped (SVG y points down) |
| `export_svg` | `path`, `sketch` | the solved sketch's lines/arcs/circles, local 2D coordinates in mm with y negated |
| `export_dxf` | `path`, `sketch` | the solved sketch's lines/arcs/circles in its local 2D coordinates, mm, R12 ASCII |
| `delete_feature` | `feature` | removes a feature nothing depends on, explicitly or as a join/cut target of a body it creates (one undo step); refused naming the dependents otherwise; bodies and embedded files no longer created/used are dropped |
| `undo` / `redo` | | document-level snapshots |
| `save` | `path` | atomic (temp file + fsync + rename); always writes the current format version |
| `open` | `path` | validates format/version/references, regenerates, compares with saved cache |
| `close` | | |
| `export_stl` | `path`, `chord?` (mm, default 0.01), `bodies?`, print fields (below) | binary STL in mm (STL has no unit field; the slicer must read mm) of `bodies` (default: all visible bodies; hidden ones only when listed, named in the message), B-rep tessellations and mesh bodies checked as for 3MF; bodies are separate shells in one file (or one file each with `separate_files`), never fused; message names the committed revision; report in `response.export` |
| `export_step` | `path`, `bodies?` | AP214 via truck-stepio with exact NURBS edge rebuild, one solid per body (default: all visible bodies); mesh bodies refused; refused when an edge can't be written exactly |
| `measure` / `inspect` | | measurements / full `summary` |

### Print export fields (`export_stl`, `export_3mf`; all optional, docs/PRINT_EXPORT.md)

| field | JSON | meaning |
|---|---|---|
| `angle_deg` | number in (0, 90) | max angle between adjacent facet normals within a curved face; the chord is refined until the measured angle meets it, refused if that needs a chord below the kernel minimum |
| `placement` | `{"bodies": [{"body": 2, "orient": {"face_down": {"feature": 6, "role": "ExtrudeEndCap"}}}, {"body": 1, "orient": "as_modeled"}], "layout": {"gap": 10}}` | export-only: proper rotation putting the face on the bed, lowest point to z = 0, optional side-by-side layout along X centred on the bed; the document is not changed. Absent: document coordinates |
| `bed` | `{"preset": "bambu_p1s"}` or `{"width": 256, "depth": 256, "height": 250}` | fit report per body and for the layout (P1S values from the installed Bambu Studio profile, incl. its excluded corner) |
| `separate_files` | bool (STL only) | one file per body: `<stem>-<sanitized body name>.stl` next to `path` |
| `allow_open_meshes` | bool | export imported meshes that are not valid solids, labelled "open reference mesh" |

`response.export` (`ExportReport`): `format`, `units`, `revision`, `chord`, `angle_deg`,
`selection`, `placement`, `bodies[]` (`id`, `name`, `kind` solid|mesh, `visible`, `label`,
`triangles`, `chord`, `max_facet_angle_deg`, `transform`, `bounds_min/max`, `dims`,
`mesh_check`, `fit`, `file`), `files[]` (`path`, `bytes`, `bodies`), `bed`, `layout_fit`,
`warnings`. Refusals write nothing and put the failing `mesh_check` on the response.

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
`EqualLength` (`{a, b}`, two different lines; parallel includes anti-parallel), `Offset`
(`{pairs: [{source, copy, sign, tangent_next?}], dim}`, run 4: a loop copy kept at `dim.value`
mm from its source; created with `Sketch::offset_loop` / the editor's Offset tool,
docs/behavior/sketching.md "Offset"), `PointOnCircle` (`{point, circle}`, run 4), `PointOnLine`
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

Failures: `"ok": false, "error": {"kind": "Validation"|"ProductLimit"|"BrokenReference"|"Cycle"|"SketchSolve"|"Kernel"|"GeometryCheck"|"Expression"|"UnknownFeature"|"UnknownParameter"|"NothingTo"|"File", "detail": ...}`.
`GeometryCheck {feature, detail}` (run 4): the kernel's result failed the independent check
(ADR 0005). `Expression {name, detail}` (run 4): a parameter or bound expression is invalid.
`Cycle` is also returned when an edit (e.g. re-attaching a sketch to a face of a later feature
of the same body) would reorder that body's modifiers (run 4: modifiers of one body keep
history order).
A failed command never changes the document (transaction rollback).

Optional fields set only by a queueing front end (the app engine), never by `Session`, and
omitted from JSON when unset: `superseded: true` (the command was skipped because a newer
edit of the same parameter was queued right behind it; `ok` is true and nothing changed) and
`elapsed_ms` (worker time including display tessellation). See `crates/f420-app/src/engine.rs`.

`summary` (`DocSummary`): `doc_id` (changes on New/Open/Close), `open`, `path`, `dirty`,
`revision` (bumped by every commit, undo and redo), undo/redo labels, and per-feature
`FeatureSummary`: parameters, dependencies, plane/frame, solve status/DOF/residual,
`solve_conflicts` (constraint labels), `sketch` (solved geometry), `profile_error`, and for
extrusions the referenceable `faces` with their current frames, and (run 4) `expressions`.
`BodySummary` (run 4) adds `planes` (every planar face's plane with its outward normal, used
by the extrude preview), `bounds` (exact, also while measurement is deferred) and
`measure_error` (a stable per-body measurement failure, not retried until the body's
geometry changes). `DocSummary.parameters` (run 4) lists user parameters with value,
`depends_on` and `used_by`.

## Native format

Current: **version 4** (run 4) = v3 (ADR 0003: bodies, blobs, operations) + `parameters` and
`features[].expressions` (docs/PARAMETERS.md). v1–v3 load; saves write v4. History of
earlier versions:

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
