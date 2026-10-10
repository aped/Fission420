# Headless command API

One API (`crates/f420-commands`, `Session::execute(&Command) -> Response`) is used by the
native app, the CLI, and the tests. Commands are JSON objects tagged by `cmd`.

## CLI

```bash
cargo build --release -p f420-cli
target/release/f420-cli run examples/plate_workflow.json      # JSON lines, one per command
target/release/f420-cli acceptance --out artifacts/acceptance # GOAL.md workflow + analytic checks
```

`run` exit codes: 0 all steps behaved as expected; 1 some step's `ok` differed from its
`expect_ok` (default `true`); 2 unreadable/invalid script; 64 usage.
`acceptance` exits 1 if any check fails and writes `acceptance_report.json`.

## Commands

| cmd | fields | notes |
|---|---|---|
| `new_document` | | replaces any open document (no prompt; headless) |
| `create_sketch` | `name?`, `plane`, `template` + template fields | `plane`: `"WorldXY"` or `{"Face":{"feature":N,"role":"ExtrudeEndCap"\|"ExtrudeStartCap"}}`; `template`: `"centered_rectangle"` (`width`,`height`) or `"centered_circle"` (`diameter`) |
| `extrude` | `name?`, `sketch`, `distance` | new body; distance > 0 mm; one base extrusion per document (product limit) |
| `cut_through_all` | `name?`, `sketch` | both directions along the sketch normal |
| `set_parameter` | `feature`, `name`, `value` | sketch dims: `width`, `height`, `diameter`; extrude: `distance` |
| `undo` / `redo` | | document-level snapshots |
| `save` | `path` | atomic (temp file + fsync + rename) |
| `open` | `path` | validates format/version/references, regenerates, compares with saved cache |
| `close` | | |
| `export_stl` | `path`, `chord?` (mm, default 0.01) | binary STL; refused if the tessellation is not watertight |
| `measure` / `inspect` | | measurements / full `summary` |

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
