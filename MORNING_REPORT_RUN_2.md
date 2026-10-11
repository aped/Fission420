# Fission420 — morning report, run 2

Spec: `SESSION_2.md` (Night 2). Branch **`work/run-2`** (from run 1's `work/run-1` @ 68aba09).
Final commit: the commit that adds this report's final edit (see `git log -1`; verification
below was run at 7c9a323). Run window: started 2026-10-10 16:23:57 PDT, deadline
2026-10-11 00:23:57 PDT; finished at about 19:45 PDT (elapsed ≈ 3 h 20 min). The core
acceptance scenarios and all three stretch items were complete and verified, and the remaining
candidates (generalised face references, multi-body) are explicitly deferred by `SESSION_2.md`,
so the run stopped early rather than filling time.
Machine: AMD Ryzen 7 9800X3D (16 threads), RX 7900 GRE, 32 GB, Bazzite (Fedora 44 atomic),
kernel 7.2.7, GNOME/Wayland, rustc 1.99.0, release builds unless noted.

## 1. Summary

The prototype now has one coherent sketch → dimension → extrude → edit workflow driven from
the mouse and keyboard, and the original run-1 modelling (templates, cuts, revolve, exports,
bracket) still works and still passes its tests.

* **Sketch editor** (new): Create Sketch on world XY or on a selected extrusion cap (semantic
  `FaceRef`), Edit Sketch for existing sketches (including run-1 templates and old files). Tools:
  Select, Line/polyline, Center Rect, 2-Point Rect, Circle, 3-Point Arc, Dimension. Snapping to
  points (shared endpoints), H/V alignment (adds visible H/V constraints) and an adaptive grid;
  screen-space tolerances; constraint-respecting drags with explanations; inline dimension
  editing with units (Enter applies, Esc cancels); Horizontal / Vertical / Coincident / Tangent /
  Equal radius / Equal length / Parallel / Perpendicular / Midpoint / On line (selection-based,
  in the inspector), construction geometry (X); local undo; Finish = one document undo step,
  Cancel discards.
  Behaviour spec: `docs/behavior/sketching.md`.
* **Profiles**: any single closed loop of lines and arcs (or one circle) extrudes as an exact
  B-rep; open / self-intersecting / zero-length geometry is kept but refused with a reason.
* **UI**: command bar, browser (left), contextual inspector (right), status bar; tree ↔ view
  selection with GPU highlight of the selected feature's faces; contextual commands disabled
  with reasons; view presets (Top/Front/Right/Iso, keys 7/1/3/0, F fit) and an axis triad;
  Help (F1); unsaved-changes guard on New/Open/Close/window close; Ctrl+S/Ctrl+Shift+S/Ctrl+O/
  Ctrl+Z/Ctrl+Shift+Z; shortcuts ignored while typing; error banner with a next step; empty
  state; dimensions of the selected sketch drawn in 3D and editable in place.
* **Responsiveness**: profiling showed that exact measurement and display tessellation of
  boolean results — not the boolean — dominated (1.8 s + 0.38 s of a 2.4 s plate edit), because
  Truck's implicit intersection edges cost ~118 µs per evaluation. After every boolean the
  kernel now replaces those edges with exact, verified NURBS (ADR 0002): a plate-with-hole
  dimension edit takes **209 ms end to end including measurements** (was 2395 ms), undo 2 ms,
  the bracket example 2.7 s (was 69 s), `cargo xtask check` ~40 s (was 2m58s at the start of
  the run, ~5 min mid-run as tests grew). Measurement is
  additionally deferred to a background thread tagged by document id + revision, so slow cases
  never block geometry and never show stale numbers. Sketch interaction is local and
  sub-millisecond.
* **Stretch**: (1) blind circular pockets with an explicit Through all / Blind depth choice;
  (2) three-point arcs with Radius / Tangent / Equal radius and line+arc profiles;
  (3) measured kernel hardening: exact post-boolean edges (above), plus a preserved
  fragile-boolean regression case (the corner Ø6.5 + centred Ø20 sequence still needs the retry
  path; a test fails on purpose if a Truck upgrade changes that).
* **Also added**: Delete feature (dependency-checked, one undo step, Del); Parallel,
  Perpendicular, Equal length, Midpoint and On-line constraints; construction (reference)
  geometry; README.
* **Evidence of real-window behaviour**: the desktop session was locked all night
  (`loginctl … LockedHint=yes`), so all window evidence comes from a *private headless GNOME
  compositor* (`mutter --headless`) running the real release binary, with **real Wayland
  pointer/keyboard input** injected through that compositor's own RemoteDesktop D-Bus API.
  Six scripted walkthroughs (`tools/live/*.txt`: scenarios A, B, C, arcs, pocket/rename/delete,
  constraints) assert document state after their steps and pass (`tools/live_walkthrough.sh`).

## 2. Build, launch, demo, test commands (all run tonight)

```bash
cd <repo>
cargo xtask check                         # fmt, clippy -D warnings, tests (release), acceptance, 5 example replays
cargo build --release -p f420-app
target/release/f420-app                   # interactive (F1 = help)
target/release/f420-cli acceptance --out artifacts/acceptance
target/release/f420-cli run examples/scenario_a_workflow.json   # also plate, bracket, scenario_b, scenario_r
tools/external_validation.sh artifacts/acceptance               # Python STL + FreeCAD/OCC STL+STEP
tools/live_walkthrough.sh artifacts/live_walkthrough            # real-input walkthroughs in a private headless compositor
cargo run --release -p f420-app --example regen_profile        # kernel stage timings
cargo run --release -p f420-app --example drag_latency         # sketch drag solver cost
cargo test --release -p f420-app --lib engine::tests::timings -- --ignored --nocapture
```

## 3. Acceptance matrix (SESSION_2)

Legend: **UI** = egui_kittest driving the real `F420App::ui` with real egui pointer/key events
(GPU-less, fallback camera, same projection code); **Live** = real binary in a private headless
compositor with real Wayland input (`tools/live/*.txt`, assertions on the app's own state);
**Headless** = shared command API (`f420-cli acceptance`); **Ext** = FreeCAD/OpenCascade + Python.

| # | item | result | evidence |
|---|---|---|---|
| A.1 | New → Create Sketch XY → centred rectangle → dimension 80×50 → Finish → Extrude 10 | PASS (UI, Live, Headless) | `crates/f420-app/tests/sketch_workflow.rs::scenario_a_*`; `tools/live/a_plate_with_hole.txt`; acceptance A.1 |
| A.2 | select top face → Create Sketch → centred Ø10 → Finish → Cut Through All | PASS (UI, Live, Headless) | same; face pick resolves `F2 ExtrudeEndCap` |
| A.3 | width → 100 through the visible dimension; hole stays centred; thickness 12, sketch follows | PASS (UI, Live, Headless) | volume 49214.6018 / 59057.5222; hole frame origin (0,0,12) |
| A.4 | undo/redo, save/close/reopen, STL watertight, STEP valid | PASS (UI, Live, Headless, Ext) | `artifacts/run2/external_validation.log` (scenario_a STEP err 6.1e-7) |
| A.5 | volume = w·h·t − π·5²·t, bounds, topology | PASS | acceptance report; UI asserts |
| B.1 | line loop (0,0)(60,0)(60,30)(50,40)(0,40), extrude 5 → 2350 mm², 11750 mm³, closed | PASS (UI, Live, Headless, Ext) | STEP err 1.8e-12 |
| B.2 | edit a vertex / a driving dimension → correct regeneration | PASS (UI: drag → 11500, width 45 → 11625; Live same; Headless chamfer length) | `scenario_b_*`, `tools/live/b_line_loop.txt` |
| B.3 | reject self-intersecting / open loop, document unchanged | PASS (UI, Headless; Live: open loop kept & not extrudable) | |
| C.1 | drag unconstrained vertex/circle; Esc cancels; one undo per drag | PASS (unit, UI, Live) | `sketch_edit.rs` tests; `scenario_c_*`; `tools/live/c_failure_paths.txt` |
| C.2 | conflicting dimension, invalid numeric input, invalid profile: visible, actionable, last valid kept | PASS (unit, UI, Live) | live shots `c_failure_paths/shots/` |
| C.3 | tree ↔ view selection, highlight + inspector; camera gestures don't place geometry | PASS (UI, Live) | `selection_sync_and_contextual_commands`; live MMB orbit in Line tool |
| C.4 | quick successive edits, undo, open another document while busy; no late results | PASS (engine tests, UI threaded test) | `engine.rs` tests; `queued_edits_and_document_switch_while_busy` |
| C.5 | unsaved changes on close; failed save path; focus blocks shortcuts | PASS (UI; Live window-close via Alt+F4 → confirm) | |
| S1 | blind circular pocket, depth vs through-all, volume/closed/STEP | PASS (UI, Headless, Ext) | `blind_pocket_through_cut_dialog`; pocket STEP err 3.5e-7 |
| S2 | three-point arcs, minimum constraints, profile validation | PASS (unit, UI, Live, Headless, Ext) | `three_point_arc_*`; acceptance R.1–R.7; R STEP err ≤ 1.4e-6 |
| extra | Parallel / Perpendicular / Equal length / On line / Midpoint, construction geometry | PASS (unit, UI, Live) | `crates/f420-sketch` tests (83), `sketch_edit.rs` tests, `tools/live/f_constraints.txt` |
| extra | delete feature | PASS (command, UI, Live) | `crates/f420-commands/tests/commands.rs`, `delete_feature_from_browser_and_keyboard`, `tools/live/e_pocket_rename_delete.txt` |
| S3 | measured kernel hardening | PASS | `docs/adr/0002-exact-boolean-edges.md`, `docs/behavior/timings.md`; kernel tests incl. fragile-boolean regression; acceptance 440/0; FreeCAD 8/8 |
| run 1 | GOAL.md acceptance, plate/bracket examples, revolve, placement | PASS | `cargo xtask check`, `ui_workflow.rs`, `placement.rs` |

Not run / not proven: a human clicking on a physical display (the session was locked);
non-Linux builds; Fusion reference comparison; very large/small parts (absolute tolerances).

## 4. What a user can do now; limitations; regressions

Can do: everything in §1, plus run-1 features (templates dialog menu, placement drag tools,
revolve, STL/STEP export, bracket example).

Limitations (honest list):
* One base body per document (extrude or revolve). Cuts/pockets are circular, parallel to the
  extrusion, from a cap sketch, and must lie inside the host profile (exact containment check
  for line/arc hosts).
* Only extrusion caps can host sketches; side faces and cut faces explain that they cannot.
* No angle dimension, offset, trim/extend, fillet-in-sketch or pattern tools; Parallel accepts
  anti-parallel lines; an arc's radius changes only through its points or a Radius dimension.
* Sketch overlays are drawn on top of the body (no depth test) in the model view.
* STEP writes cylinders/arcs as exact rational B-splines (other tools show splines).
* The Truck boolean itself costs ~150–190 ms per cut; holes crossing holes or angled planes keep
  Truck's implicit edges (correct but slow path, seconds).
* Observed once, not reproduced (live or in a kittest pointer test): a toolbar click right after
  dismissing the unsaved-changes dialog did nothing; a second click worked
  (`crates/f420-app/tests/dialog_click.rs` guards it).
* Frame pacing on Wayland (upstream behaviour, egui #8326, quoted in eframe's source): eframe
  repaints only after a compositor frame callback. In the private headless compositor the frame
  clock occasionally went quiet while the app waited for a worker reply, so the reply was applied
  only at the next input event (traced: the engine finished and posted in time; no UI frame
  ran). The live harness therefore nudges the pointer while waiting. Whether this ever shows on
  a physical display (where mutter's clock follows vblank) is **unverified** — the desktop was
  locked all night.
* The glyphs ▾ ◐ → ● are missing from egui's default fonts and were replaced with ASCII.

Bugs found tonight by driving the real window with real input (all fixed, each with a test or
a live script): the toolbar shifting under the pointer when the Undo label changed; inline
dimension editors not selecting their text (typing appended); glyphs missing from the UI font;
**sketch clicks misplaced or dropped when a slow frame batched a release with the next
movement** (clicks are now derived from raw press/release events); **a cross-thread race on
Truck geometry** (shallow `Body` clones share mutex-protected curves; the measurement thread
now gets a deep copy); measurements left "pending" forever after a failure.

Known regressions vs run 1: none known. Behaviour changes: template dialogs moved into a
Templates menu, exports into an Export menu; Undo/Redo show the action in the tooltip (fixed
labels so the bar never shifts); commands queue while the worker is busy instead of being
disabled; the native format is now v2 (v1 files load unchanged; saves write v2, which run-1
binaries refuse).

## 5. Five-minute walkthrough (actual labels and bindings)

1. `cargo build --release -p f420-app && target/release/f420-app`. Empty state: click
   **New Sketch on XY** (or **New**, then **Create Sketch**).
2. Press **R** (Center Rect). Click the origin (the ring snaps), then click a corner. Press
   **D** (Dimension), click the bottom edge, type `80`, **Enter**; click the right edge, type
   `50`, **Enter**. The chip turns to `✔ Fully constrained (DOF 0)` and the lines turn white.
3. **Finish Sketch**. Click **Extrude…** (the sketch is preselected), type `10`, **Enter**.
4. Click the top face (it highlights; the inspector says `Extrude1 end cap (semantic F2
   ExtrudeEndCap)`), click **Create Sketch**. Press **C**, click the origin, click a rim point;
   **D**, click the circle, type `10`, **Enter**. **Finish Sketch**.
5. **Cut through all…**, **Enter** (or choose **Blind depth** for a pocket).
6. Click **Sketch1** in the browser: its dimensions appear in 3D. Click `80 mm`, type `100`,
   **Enter**. The status bar shows the geometry immediately, then the volume
   (49214.6018 mm³). Click **Extrude1**, set `distance (mm)` to 12, **Apply**.
7. **Undo** / **Redo** (Ctrl+Z / Ctrl+Shift+Z). **Ctrl+Shift+S** to save, **Close**, **Ctrl+O**.
8. Try failures: in a sketch, dimension the top edge of the rectangle again (rejected, culprit
   named); type `abc` into a dimension (stays open, error); drag a dimensioned corner (blocked,
   explained). Close the window with unsaved changes → **Save first / Discard changes / Keep
   editing**.
9. Arcs: **Create Sketch**, **L** line from (−20,0) to (20,0), **Esc**; **A**, click the two ends,
   then a point above; **D** on the arc → radius; **Finish Sketch**, **Extrude…**.
10. Navigation: middle-drag orbit, right-drag pan, wheel zoom to cursor, **F** fit, **7/1/3/0**
    views, **F1** help. In sketch mode the left button always belongs to the tool.

## 6. Screenshots (capture method)

`docs/screenshots/` (committed, JPEG). "Before" is the run-1 app on the real desktop at the start
of the run. Every "after" image is an in-app F12 capture of the real release binary in a private
headless mutter (virtual monitor 1280×800 unless noted), driven by real compositor input.

| image | shows |
|---|---|
| `00-before-run1.jpg` | run-1 UI at the start of the night |
| `01-sketch-dimensions.jpg` | active sketch: centred rectangle, 80/50 mm dimensions, inline editor |
| `02-face-selected.jpg` | top face selected: highlight + semantic description, Create Sketch enabled |
| `03-plate-hole-dims-3d.jpg` | plate with hole; selected sketch's dimensions editable in 3D |
| `04-pending-regeneration.jpg` | pending regeneration: last valid model shown, measurements marked pending (before the kernel speed-up) |
| `05-rejected-redundant-dimension.jpg` | recoverable error: redundant dimension rejected, culprits named |
| `06-unsaved-changes.jpg` | window close with unsaved changes |
| `07-three-point-arc.jpg` | three-point arc with radius dimension |
| `08-1920x1080-zoom.jpg` | 1920×1080 at 1.2× UI zoom (v1 example file opened) |
| `09-constraints-inspector.jpg` | Parallel + Equal length glyphs, construction line, constrain-selection grid |
| `10-blind-pocket.jpg` | blind pocket selected (floor + wall highlighted), depth parameter |
| `11-arc-d-shape-extruded.jpg` | line + arc profile extruded |

All raw captures (≈80 PNGs) with the scripts and logs that produced them are under
`artifacts/run2/` (untracked, per repository rules).

## 7. Timings (method: release build, this machine)

| measurement | value | method |
|---|---|---|
| sketch drag step (free vertex / resize / blocked + diagnosis) | 2 µs / 5 µs / 0.12 ms | `--example drag_latency` |
| UI CPU per frame (status bar EMA) | 0.2–0.5 ms | live sessions |
| plate+hole width edit → geometry + measurements | 209 / 211 ms (was 2395 / 2394) | engine timing harness, 7 runs |
| plate+hole undo (cached prefix) | 2.1 ms (was 517) | same |
| box / scenario-B edit round trip | 1.1 ms | same |
| plate boolean / measure / display tessellation | 194 / 0.9 / 2.1 ms (was 183 / 1818 / 380) | `--example regen_profile` |
| bracket example (5 holes) | 2.7 s (was 69 s) | `f420-cli run`, 3 runs (kernel worker) |
| `cargo xtask check` | ~40 s (was 2m58s at baseline) | `artifacts/run2/xtask_final*.log` |

## 8. Examples and migration

* Native documents: `examples/plate.f420.json`, `bracket.f420.json`, `scenario_a.f420.json`,
  `scenario_b.f420.json`, `scenario_r.f420.json` (v2). Frozen v1 fixture:
  `crates/f420-document/tests/fixtures/plate_v1.f420.json`.
* Replayable commands: `examples/*_workflow.json` (`f420-cli run …`, exit 0).
* Migration: format v2 adds sketch `origin`, `Distance`, arcs, `Radius`/`Tangent`/`EqualRadius`,
  pockets; it is additive, v1 loads unchanged (identity migration), saves write v2.
  `serde_json` `float_roundtrip` makes stored solved coordinates reload bit-exactly.

## 9. Tests and checks run

* `cargo xtask check` (final, 7c9a323): fmt, clippy `-D warnings`, **233 tests passed, 0 failed,
  2 ignored** (timing harnesses, run separately), acceptance **440 passed / 0 failed**, 5 example
  replays, 35.6 s — `artifacts/run2/xtask_final3.log`. Baseline at the start: 78 tests, 96/96,
  2m58s — `artifacts/run2/baseline_xtask.log`.
* External: `tools/external_validation.sh` on the final acceptance outputs — 8 models, all PASS
  (Python STL parser, FreeCAD 1.1 STL and STEP import: 1 valid solid each, STEP volume error
  ≤ 1.4e-6 mm³ against analytic values) — `artifacts/run2/external_validation_handoff.log`.
* Live real-input walkthroughs: **6/6 PASS on two consecutive runs** of the final build
  (scenario A, B, C, arcs, pocket / rename / delete, constraints; ~94 s per run) — logs per
  script under `artifacts/run2/live_handoff_{A,B}/`
  (`script.txt`, `result.log` with every assertion, `app.log` with the input trace, `shots/`).
* Independent code review of the app (13 findings): 12 fixed with tests, 1 (stuck measuring
  after a failed measurement) fixed earlier; a concurrency bug found while tracing (shallow
  `Body` clones shared Truck mutexes across threads) fixed with deep copies.
* Separation of evidence: kittest = widget/input logic with real egui events (no compositor);
  live = real window + compositor input routing (no human); scripted `--screenshot` = appearance.

## 10. Next three improvements (by practical value) and open decisions

1. **Semantic references beyond extrusion caps** (side faces keyed by the generating sketch
   edge, cut walls/floors), with cuts in any direction. The kernel already cuts from a side face
   exactly (kernel test); the document's product limit and face resolution are what stop it.
   This needs a small topological-naming design (ADR) to stay honest about broken references.
2. **Verify on a physical display** and, if needed, harden frame pacing: run
   `tools/live_walkthrough.sh` on the unlocked desktop (or click through §5) and watch for UI
   that only updates on mouse movement (egui #8326). If it appears, consider a timed repaint
   fallback while work is pending.
3. **More modelling**: multiple bodies / joins, linear patterns of holes, fillets on sketch
   corners (sketch-level arcs already exist), and non-circular cuts and pockets.

Decisions for you:
* Commit screenshots in-repo (now ~1.2 MB of JPEG under `docs/screenshots/`) or keep only
  `artifacts/`?
* Keep run-1 template dialogs (Templates menu) now that the editor covers them?
* Persist undo history in files (still not done by design)?
* Patch/vendor Truck (boolean fragility, STEP surfaces as B-splines)? Not needed for tonight's
  fixes; exact edge rebuilds live in the adapter (ADR 0002).

## 11. Housekeeping

* The five subagent worktrees under `.claude/worktrees/` (≈ 19 GB, mostly `target/` build
  caches) are fully merged into `work/run-2` and can be removed when convenient:
  `git worktree list`, then `git worktree remove --force <path>` and
  `git branch -d worktree-agent-<id>` for each. I left them in place rather than deleting them.
* Running FreeCAD validation rewrites the tracked `tools/__pycache__/*.pyc` files; restore with
  `git checkout -- tools/__pycache__` (consider untracking them).
* `SESSION_2.md` and `spikes/truck_spike/Cargo.lock` were untracked at the start and were left as
  they are.
