# STATUS — Fission420 run 2 (spec: SESSION_2.md)

## Run budget (recorded once; do not reset on resume)
- Start: 2026-10-10T16:23:57-07:00 (branch work/run-2 from work/run-1 @ 68aba09)
- Deadline: 2026-10-11T00:23:57-07:00 (8 h)
- Integration reserve begins: 2026-10-10T22:53-07:00 (final ~90 min: no new features)
- Supersedes: GOAL.md acceptance (run 1, kept passing) as the *primary* target; SESSION_2.md scenarios A/B/C are this run's acceptance. AGENTS.md rules unchanged.
- Workers: max 3 agents incl. coordinator (GOAL.md). Isolated git worktrees, merged by the coordinator.

## Baseline (observed at start)
- `cargo xtask check`: PASS, 78 tests, acceptance 96/96, 2m58s (artifacts/run2/baseline_xtask.log)
- Screenshots: artifacts/run2/before_empty.png, artifacts/run2/before_demo.png (scripted, real window via --screenshot)

## Plan and ownership
| Area | Owner | Files | Acceptance |
|---|---|---|---|
| Backend API skeleton (origin, Distance, drag, optional profiles, EditSketch/Rename/Polygon/Custom, doc_id, format v2) | coordinator | f420-sketch, f420-document, f420-commands | DONE 57273b8; existing tests green; scenario B headless 11750 mm^3 |
| Backend hardening, headless scenarios A/B, examples, engine coalescing + timings, (stretch: blind pocket) | worker "backend" | f420-sketch/document/commands/cli tests, engine.rs, examples, docs | sketch/doc/engine tests; `f420-cli acceptance` A./B. steps |
| Viewport: view presets, axis triad, selection highlight sets, feature edges, adaptive sketch-plane grid, zoom-to-cursor | worker "viewport" | f420-viewport | unit tests; screenshot |
| Sketch editor + UI restructure (command bar / tree / inspector / status), selection sync, contextual commands, shortcuts, dirty handling, help, empty state | coordinator | f420-app (app.rs, new sketch_edit.rs, sketch_view.rs) | egui_kittest UI tests for A, B, C; screenshots |

## Design decisions (material)
- Sketch edit session is UI-local: Create/Edit Sketch copies the sketch; tools mutate the copy and solve locally (µs–ms, UI thread); Finish sends ONE command (create_sketch template=custom, or edit_sketch) → one document undo entry. Cancel discards. Inside the editor Ctrl+Z/Ctrl+Shift+Z walk a local snapshot stack.
- Editor sketches carry an explicit fixed `origin` point; templates keep their movable anchor (center_x/center_y). Template sketches open in the same editor (no conversion needed: they are ordinary constraint sketches).
- Snapping to points reuses the point id (shared endpoint = implicit coincidence, visible as a merged vertex); H/V snapping while drawing adds explicit Horizontal/Vertical constraints (shown as glyphs). Grid snap does not create constraints.
- Sketches whose geometry is not a single closed profile are kept (profile_error recorded) and refused by extrude/cut with a structured error; nothing is auto-closed.
- Drags never bypass constraints: pin target → else slide on one axis → else blocked with the blocking constraints named.
- Face → sketch: viewport pick is matched against the semantic faces (extrude end/start caps) the document reports; other faces explain why they cannot host a sketch. The regenerator re-validates the semantic reference.
- Native format v2 (additive); v1 files load unchanged; serde_json float_roundtrip for exact reloads.

## Progress log
| time | item | result |
|---|---|---|
| 16:24 | baseline xtask | PASS |
| 16:35 | backend skeleton 57273b8 | PASS (sketch/doc/commands tests, acceptance) |
| 16:45 | viewport worker merged (578f23e) | 36 viewport tests |
| 16:50 | sketch editor core 7d96ae8 | 7 unit tests |
| 16:58 | UI restructure ca4b332 | run-1 UI tests updated + pass |
| 17:02 | scenario A/B/C kittest suite 965894b | 6/6 pass |
| 17:20 | live-window input (headless mutter + RemoteDesktop) d788d6e; fixes found live | scenario A executed live, screenshots artifacts/run2/live_a/shots |

## Next
- Merge backend worker (headless A/B acceptance, engine coalescing, timings).
- Live scenario B/C walkthrough; regeneration latency (2.4 s plate+hole set_parameter) profiling.
- Integration reserve from 22:53: xtask check, external validation, report.

## Evidence locations (run 2)
- UI tests: crates/f420-app/tests/sketch_workflow.rs (A/B/C), ui_workflow.rs + placement.rs (run 1, updated)
- Live real-input sessions: tools/headless_session.sh + tools/live_input.py; artifacts/run2/live_a/{a*.txt scripts, app.log input trace, shots/}
- Desktop session is locked (loginctl LockedHint=yes) so --screenshot on the user's display hangs; all window captures use a private headless mutter.

---

# (archived) STATUS — Fission420 run 1

## Run budget (recorded once; do not reset on resume)
- Start: 2026-10-10T00:34:03-07:00
- Deadline: 2026-10-10T08:34:03-07:00 (8 h; human's /goal instruction overrides GOAL.md's 6 h default)
- Integration reserve begins: 2026-10-10T08:04:03-07:00
- Task budget: 24 bounded tasks. Used: 23 (see log). Run closed at 2026-10-10T01:45-07:00: acceptance goal met and stretch items 1, 3, 4, 5 done; ~6.8 h of time and 1 task unused (remaining stretch item, freeform sketch constraints, is larger than one bounded task).

## Environment (observed)
- Bazzite (Fedora 44 atomic), kernel 7.2.7, x86_64, 16 threads, Wayland (WAYLAND_DISPLAY=wayland-0, DISPLAY=:0)
- rustc/cargo 1.99.0 (Homebrew), network to crates.io OK
- Git: global identity is a placeholder ("Your GitHub Username"); commits use it unchanged rather than fabricating one.

## Task log
| # | Task | Result |
|---|------|--------|
| T1 | Runway: git init, branch work/run-1, status | done |
| T2 | Truck kernel spike (+ parallel vcad probe) | done: Truck chosen, ADR 0001 |
| T3 | f420-kernel adapter (extrude, through-all cut, B-rep volume/bounds, tessellation, STL) | done, 7 tests |
| T4 | f420-sketch solver (worker) | done, 15 tests |
| T5 | f420-viewport wgpu widget (worker) | done, 14 tests, screenshot artifacts/viewport_smoke.png |
| T6 | f420-document: features, semantic FaceRef, regen, transactions, undo, native format | done |
| T7 | f420-commands + f420-cli + acceptance runner | done, 94/94 checks |
| T8 | Document tests (rollback, refs, cycles, corrupt files, determinism) | done, 7 tests |
| T9 | xtask check, commands tests, example script/doc, independent STL checkers (python + FreeCAD) | done |
| T10 | Docs + license audit | done |
| T11 | Native UI on command API (worker) | done: kittest UI tests 6/6, artifacts/ui_demo.png |
| T12 | STEP export (exact NURBS edge rebuild around truck-stepio bug), FreeCAD validation | done, OCC volume err 5e-7 |
| T13 | Independent review (worker) + fixes: containment frame, WorldXY cuts, chord bound, id overflow, temp files | done, regression tests |
| T14 | UI integration: dev profile, Export STEP dialog, xtask full pass | done (57 tests, acceptance 96/96) |
| T15 | UI tests for depth-follow and oversize hole; MORNING_REPORT checkpoint | done |
| T16 | Template centres + center_x/center_y parameters | done |
| T17 | Revolve (WorldXZ, Pappus-checked), volume classification for revolved faces | done |
| T18 | Interactive placement in viewport (worker) | done, kittest drag tests, screenshots |
| T19 | Bracket example, boolean retry (seam × tolerance), final-body-only measurement | done, FreeCAD-validated |
| T20 | Regeneration prefix cache | done, equivalence test |
| T21 | Revolve UI forms, deg units, command docs | done |
| T22 | Live-window input via xdotool/XWayland | NOT achieved (XTEST did not reach window; stopped to avoid permission prompts); added --shot-dir F12 capture |
| T23 | Final verification + handoff | done: xtask all pass (78 tests, 96/96), external validation pass |

## Acceptance checklist (GOAL.md)
Headless: `target/release/f420-cli acceptance` → artifacts/acceptance/acceptance_report.json (96/96).
UI: crates/f420-app/tests/ui_workflow.rs + placement.rs (egui_kittest drives real widgets) + artifacts/ui_*.png.
1-7, 9, 10: PASS headless and via UI widget tests. 8: STL PASS (Rust, Python, FreeCAD); STEP PASS (FreeCAD/OCC, err 5e-7).
Pending: manual live-window mouse interaction by a human; Fusion reference validation.

## Commands
- `cargo xtask check` (fmt, clippy -D warnings, test --release, acceptance)
- `target/release/f420-cli acceptance --out artifacts/acceptance`
- `target/release/f420-cli run examples/plate_workflow.json`
- `python3 tools/check_stl.py artifacts/plate.stl 49214.60183660255`

## Current commit
See `git log -1` on work/run-1 (final checkpoint after T23).

## Next task (new run)
See MORNING_REPORT.md §6: freeform sketching + constraints UI; generalized semantic references (ADR 0002); kernel hardening.

## Blockers / notes
- liblzma C code transitively via truck-meshalgo vtk feature (documented in DEPENDENCIES).
