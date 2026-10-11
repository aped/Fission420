# Fission420

A local-first parametric CAD prototype for small mechanical parts, written in Rust: an exact
B-rep kernel adapter over [Truck](https://github.com/ricosjp/truck), a constraint-based sketch
solver, a feature history with semantic face references, transactional undo/redo, a versioned
native file format, STL/STEP export, a headless command API, and a native egui/wgpu app.

Linux (Wayland/X11) is the target. Not affiliated with any commercial CAD vendor.

## Quick start

```bash
cargo build --release -p f420-app
target/release/f420-app            # F1 = help
```

1. **New Sketch on XY** → **R** (centre rectangle): click the origin, click a corner →
   **D** (dimension): click an edge, type a value, Enter → **Finish Sketch**.
2. **Extrude…**, Enter. Click the top face → **Create Sketch** → **C** circle → **Finish** →
   **Cut through all…** (or **Blind depth**).
3. Select the sketch in the browser and click a dimension in the 3D view to change it; undo with
   Ctrl+Z.

Navigation: middle-drag orbit, right-drag pan, wheel zoom, **F** fit, **7/1/3/0** views.

## Headless

```bash
cargo build --release -p f420-cli
target/release/f420-cli run examples/scenario_a_workflow.json     # JSON command script
target/release/f420-cli acceptance --out artifacts/acceptance     # analytic acceptance checks
```

## Checks

```bash
cargo xtask check                              # fmt, clippy, tests, acceptance, example replays (~40 s)
tools/external_validation.sh artifacts/acceptance   # independent STL/STEP checks (Python, FreeCAD)
tools/live_walkthrough.sh                      # real-input UI walkthroughs in a private headless compositor
```

## Documentation

| file | contents |
|---|---|
| `runs/run-2/MORNING_REPORT_RUN_2.md` | latest run: what works, evidence, limitations, walkthrough |
| `runs/` | run specs, status log and reports for every run (see `runs/README.md`) |
| `docs/behavior/sketching.md` | sketch editor behaviour (tools, snapping, drags, undo) |
| `docs/COMMANDS.md` | command API and native format |
| `docs/TOLERANCES.md` | every tolerance and why |
| `docs/behavior/timings.md` | measured timings and the kernel bottleneck analysis |
| `docs/adr/` | architecture decisions (kernel choice, exact boolean edges) |
| `docs/DEPENDENCIES.md` | dependency licences |
| `runs/STATUS.md` | task log and current state |

## Limits (prototype)

One base body per document; cuts and pockets are circular; sketches attach to the world planes
or to extrusion caps; no fillets, assemblies or drawings. See the latest run report for the full,
current list.

Licence: MIT OR Apache-2.0.
