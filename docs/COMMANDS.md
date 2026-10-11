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
target/release/f420-cli acceptance --out artifacts/acceptance  # GOAL.md workflow + scenarios A/B, analytic checks
tools/external_validation.sh artifacts/acceptance               # python STL + FreeCAD STL/STEP checks
```

`run` exit codes: 0 all steps behaved as expected; 1 some step's `ok` differed from its
`expect_ok` (default `true`); 2 unreadable/invalid script; 64 usage. Scripts run from the
repository root (paths are relative to the working directory).
`acceptance` exits 1 if any check fails and writes `acceptance_report.json`; steps of the
SESSION_2 scenarios are prefixed `A.` / `B.`.

The scenario workflows are generated from `f420_commands::scenarios` (the sketches are built
with the sketch editing API, like the interactive editor). After changing that module:
`F420_WRITE_EXAMPLES=1 cargo test -p f420-commands --test scenario_examples`; without the
variable the same test fails when the committed JSON is stale, and it replays both workflows.

## Commands

| cmd | fields | notes |
|---|---|---|
| `new_document` | | replaces any open document (no prompt; headless) |
| `create_sketch` | `name?`, `plane`, `template` + template fields | see below |
| `extrude` | `name?`, `sketch`, `distance` | new body; distance > 0 mm; one base extrusion per document (product limit); the sketch must be a usable profile (else `Validation`) |
| `revolve` | `name?`, `sketch`, `angle_deg` | new body; revolves about the sketch's local Y axis (WorldXZ → world Z); 0 < angle ≤ 360; profile strictly at x > 0 |
| `cut_through_all` | `name?`, `sketch` | both directions along the sketch normal; circular, parallel to the base extrusion, strictly inside it (product limit); not on revolved bodies |
| `set_parameter` | `feature`, `name`, `value` | sketch: every named dimension (`width`, `height`, `diameter`, or any name an editor sketch gives, e.g. `chamfer`) and, on template sketches only, `center_x`, `center_y`; extrude: `distance` (mm); revolve: `angle_deg` (stored in radians). Sketch edits store the **solved** coordinates in the definition |
| `edit_sketch` | `feature`, `sketch` | replace a sketch feature's definition (the editor's Finish Sketch); one undo step; plane and downstream references kept; the sketch is validated and solved first (conflicts are refused, naming the constraints); downstream features must still regenerate, otherwise nothing changes |
| `rename_feature` | `feature`, `name` | 1-128 printable characters, unique among features |
| `undo` / `redo` | | document-level snapshots |
| `save` | `path` | atomic (temp file + fsync + rename); always writes the current format version |
| `open` | `path` | validates format/version/references, regenerates, compares with saved cache |
| `close` | | |
| `export_stl` | `path`, `chord?` (mm, default 0.01) | binary STL; refused if the tessellation is not watertight; message names the committed revision |
| `export_step` | `path` | AP214 via truck-stepio with exact NURBS edge rebuild; refused when an edge can't be written exactly; message names the committed revision |
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

A sketch whose geometry is not a single closed profile (open polyline, extra dangling line,
self-intersection, zero-length edge, more than one loop, lines mixed with circles) is still
created; its `FeatureSummary.profile_error` says why, and any extrude/revolve/cut of it fails
with `Validation`. Isolated points (such as the origin) are ignored by profile extraction.

`Sketch` JSON (`custom`, `edit_sketch`): `points` (`{id, x, y}` mm), `entities`
(`{"Line":{id,a,b}}`, `{"Circle":{id,center,radius}}`), `constraints` (`FixedPoint`,
`Horizontal`, `Vertical`, `HorizontalDistance`, `VerticalDistance`, `Distance` (aligned length,
nonlinear), `SymmetricAboutPoint`, `Diameter`, `Coincident`; dimensions carry
`{"name", "value"}`, names unique per sketch), optional `origin` (the point fixed at (0, 0)).

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
optional sketch `origin` and the `Distance` constraint. **Migration:** version-1 files are a
strict subset and load unchanged (identity migration); saving writes version 2. Files claiming
a newer version than the build supports are refused with a `File` error. Stored coordinates
are written with shortest round-trip float formatting and reload bit-exactly.
`crates/f420-document/tests/fixtures/plate_v1.f420.json` is a frozen v1 file used by the
migration test (the `examples/*.f420.json` files are re-saved as v2 by their workflows).
