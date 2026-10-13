# Observed timings (run 2)

Measured 2026-10-10 with the timing harness in `crates/rcad/src/engine.rs`
(`#[ignore]`d test, so it never gates CI):

```bash
cargo test --release -p rcad --lib engine::tests::timings -- --ignored --nocapture
```

Machine: AMD Ryzen 7 9800X3D (8 cores / 16 threads, boost to 4.7 GHz), 32 GB RAM, Linux
7.2.7 (Bazzite / Fedora 44 base), rustc 1.99.0, release profile. Load average ~2 on 16
threads during the run (other builds were not running). Each row is 7 runs (undo: 5).

"Round trip" is `Engine::submit` until `Engine::poll` returns the reply, through the real
threaded engine (worker execution + display tessellation at `DISPLAY_CHORD` = 0.05 mm +
channel). "Worker" is `Reply::elapsed()` (the worker's own clock for the same work).

| operation | median | min | max |
|---|---|---|---|
| Plate 80×50×10 with Ø10 through cut: `set_parameter width` round trip (full regeneration incl. boolean cut + B-rep volume + display tessellation) | 2395 ms | 2356 ms | 2497 ms |
| same, worker time | 2395 ms | 2356 ms | 2497 ms |
| Plate: `undo` round trip (regeneration served from the prefix cache) | 517 ms | 379 ms | 521 ms |
| Plate full build via `Session` only (new, sketch, extrude, sketch, cut; no tessellation) | 2003 ms | 1984 ms | 2551 ms |
| Box (no boolean) `set_parameter width` round trip | 1.1 ms | 1.1 ms | 1.1 ms |
| Scenario B extrude (5-vertex editor loop, 5 mm) round trip | 1.1 ms | 1.1 ms | 1.1 ms |
| Scenario B extrude worker time | 0.2 ms | 0.2 ms | 0.3 ms |

Observations (labelled by confidence):

- Measured: worker time equals round-trip time to within 0.1 ms, so channel/queue overhead is
  negligible; the cost is entirely regeneration and tessellation.
- Measured: a parameter edit on the plate with a cut costs ~2.4 s; on a body without a boolean
  it costs ~1 ms. The slow path is the Truck boolean cut (plus B-rep volume integration of the
  result), not the sketch solver or the extrusion.
- Inferred (not separately timed): the ~0.5 s undo with a full cache hit is mostly display
  tessellation of the cut body, since undo performs no boolean and the cached measurements are
  reused. Splitting tessellation timing out of `execute()` would confirm it.
- Consequence for the UI: a dimension edit on a cut body cannot meet a 50 ms target with exact
  regeneration; the engine keeps the last valid body displayed and coalesces queued edits of
  the same parameter (`Reply::superseded`). A burst of edits of one parameter therefore costs
  at most two regenerations (the one already running when the burst arrives, plus the last
  value), not one per edit. Running kernel work is not interrupted.

## Deferred measurement (run 2, coordinator)

Profiling one plate-with-hole regeneration stage by stage
(`cargo run --release -p rcad --example regen_profile`, same machine):

| stage | time |
|---|---|
| extrude | 0.0 ms |
| `cut_through_all` (Truck boolean) | 155–183 ms |
| `measure` (B-rep boundary-integral volume, bounds, topology) | ~1805 ms |
| `tessellate_display` (0.05 mm chord) | ~377 ms |

So the earlier inference that the boolean dominates was wrong: exact measurement is ~75 %.
The threaded engine now defers it (see the module docs of `crates/rcad/src/engine.rs`):
the reply carries the new geometry and `summary.measuring`, a measurement thread (latest job
wins) computes the volume, and a follow-up reply tagged with document id + revision delivers
it; the UI shows "Measuring volume… (geometry is current)" meanwhile and ignores a follow-up
for any revision it no longer shows. Headless clients (CLI, acceptance, tests in Sync mode)
are unchanged and still receive measurements in every response; `save`/`measure` complete a
pending measurement synchronously.

| operation (threaded engine, release, 7 runs) | median | min | max |
|---|---|---|---|
| plate `set_parameter width` → geometry reply | 575 ms | 550 ms | 597 ms |
| same → measurements arrive | 2394 ms | 2365 ms | 3646 ms |

Remaining geometry latency is the boolean plus display tessellation (kernel-side; not
optimized this run).

## Kernel hardening: intersection-curve cost (run 2, kernel worker)

Same machine, release build. Root cause of the measured bottleneck above, then the fix
(ADR 0002) with before/after numbers.

### Where the time went (measured)

Temporary instrumentation (not committed) counted and timed every edge evaluation in
`measure.rs::eval_curve` for the 80×50×10 plate with a Ø10 through hole:

- The boolean result has **6 `IntersectionCurve` edges** (3 NURBS cylinder patches × top and
  bottom rim; each `Plane` ∩ `NurbsSurface`, polyline leaders of 29–30 points). All other
  edges are `Line`s.
- One `IntersectionCurve` evaluation (`search_triple` → un-hinted `double_projection`: a
  nearest-point search on each surface, then Newton) costs **~118 µs**. A `Line`/NURBS
  evaluation costs well under 1 µs.
- `measure` made **24,606** intersection-curve evaluations (2,893 ms of summed thread time
  over the 3 concurrent Richardson levels, 1.81 s wall):
  - `raw_bounds`: 257 samples × 12 edge uses (each edge is visited from both faces) = 3,084;
  - boundary-integral volume: 6 edges × (513 + 1,025 + 2,049) chord points = 21,522.
  - The 28,696 `Line` evaluations in the same call were negligible.
- `tessellate_display(0.05)`: **3,084** evaluations, 366 ms of its 383 ms, all in the
  `raw_bounds` call that sizes the minimum chord. Truck's own triangulation was ~17 ms.
- So the hypothesis was right: implicit intersection edges, evaluated thousands of times,
  accounted for ~99 % of measure and ~96 % of tessellate time.
- Exact arcs from line/arc loop extrusions (rational quadratic NURBS edges) are not slow: an
  80×50×10 rounded rectangle (r 8) measures in 0.97 ms and tessellates in 1.5 ms; after a Ø10
  cut, 1.9 ms and 2.6 ms.

### Fix

After every successful boolean, each `Plane` ∩ (NURBS surface extruded along the plane normal)
edge is replaced by its exact rational B-spline (projection of the generating curve onto the
plane; affine, hence exact), verified against the original curve and vertices, and kept as the
original on any mismatch (`crates/rcad-kernel/src/exact.rs`, ADR 0002, tolerances in
docs/TOLERANCES.md). Measurement code and its mathematics are unchanged.

### Before / after (release, 9800X3D)

| case | before | after |
|---|---|---|
| plate 80×50×10 Ø10: `cut_through_all` | 183 ms | 194 ms (+~10 ms rebuild/verify) |
| same: `measure` | 1818 ms | 0.9 ms |
| same: `tessellate_display(0.05)` | 380 ms | 1.6 ms |
| same: volume error vs 40000 − 250π | −1.2e-5 mm³ | +4e-11 mm³ |
| plate 100×50×10 Ø10: cut / measure / tessellate | 156 / 1809 / 377 ms | 167 / 1.0 / 1.7 ms |
| `rcad-cli run examples/bracket_workflow.json` (wall, 3 runs) | 68.4–69.7 s | 2.66–2.69 s |
| bracket final volume error vs 43200 − 853.5π | −1.1e-5 mm³ | +1.6e-10 mm³ |
| `rcad-cli run examples/scenario_r_workflow.json` (wall, 3 runs) | 6.27–6.94 s | 0.48 s |
| `rcad-cli acceptance` (440 checks, 0 failed both) | 76.5 s | 2.4 s |
| engine harness: plate `set_parameter width` geometry reply (median of 7) | 575 ms | 211 ms |
| engine harness: same, until measurements arrive | 2394 ms | 213 ms |
| engine harness: plate undo (cached prefix) | 517 ms | 2.1 ms |

Commands:

```bash
cargo run --release -p rcad --example regen_profile
target/release/rcad-cli run examples/bracket_workflow.json     # needs ./artifacts/
target/release/rcad-cli run examples/scenario_r_workflow.json
target/release/rcad-cli acceptance --out artifacts/acceptance
cargo test --release -p rcad --lib engine::tests::timings -- --ignored --nocapture
```

The remaining regeneration cost of a holed part is the Truck boolean itself
(~150–190 ms per cut); it was not changed.
