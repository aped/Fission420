# Observed timings (run 2)

Measured 2026-10-10 with the timing harness in `crates/f420-app/src/engine.rs`
(`#[ignore]`d test, so it never gates CI):

```bash
cargo test --release -p f420-app --lib engine::tests::timings -- --ignored --nocapture
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
