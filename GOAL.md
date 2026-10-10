# Goal for this run

Edit this file before starting. Defaults are an ambitious first overnight experiment, not a delivery guarantee. Change the product goal freely; retain measurable acceptance criteria.

## Product goal

Build a native, local-first Rust parametric CAD prototype for designing small mechanical parts on Linux. The first complete workflow is a dimensioned rectangular plate with a centered through-hole, editable through a feature history.

## Run budget and defaults

- Active run budget: 6 hours from the recorded start time.
- Maximum bounded implementation tasks: 24; stop when either time or task budget is exhausted.
- Reserve the final 30 minutes for integration, verification, and handoff.
- Maximum simultaneous agents: 3, including the coordinator, if supported and permitted. Serial execution is fine.
- External paid services: none. A runtime's existing model usage is subject to the human's configured budget; this file cannot enforce billing limits.
- Language: Rust; no C/C++ modeling kernel.
- Target: native Linux first; no browser requirement tonight.
- UI: egui/eframe + wgpu unless a brief compatibility check finds a concrete blocker.
- File storage: local, versioned native format. STL export is useful. STEP is stretch work and must preserve actual analytic geometry.
- Dependency policy: permissive licenses as described in AGENTS.md.
- Kernel approach: time-box a Truck feasibility spike; consider vcad if Truck fails a required operation. Reuse approved geometry libraries; an entirely new general-purpose kernel is not tonight's goal.
- Reference application: optional; do not wait for access or require sign-in. Specifications remain useful without it.

## Acceptance workflow

1. Launch the native app and create a new document.
2. Create an XY sketch with a rectangle centered at the origin, width 80 mm and height 50 mm. Show editable dimensions and actual constraint state. A dimensioned-rectangle tool is acceptable for the first pass; arbitrary freehand constraint solving is not required tonight.
3. Extrude the rectangle +10 mm along Z into one solid.
4. Create a circle of diameter 10 mm centered on the top face and cut through the plate. Its attachment must be represented as a semantic reference to the extrusion's top face, not a fixed face index or unexplained world-space shortcut.
5. Change the original width to 100 mm. The hole remains centered, and the resulting body stays valid.
6. Change extrusion depth to 12 mm. The attached sketch follows the top face and the cut still goes through all material. Restore depth to 10 mm.
7. Undo and redo the width change; save, close, and reload. Parameters, feature history, references, and geometry survive.
8. Export the body as STL; verify watertight tessellation. Implement STEP only if the chosen kernel supports genuine solid export and it does not displace core work.
9. Run the same model creation and edits headlessly through the shared command API, emitting structured measurements and failures.
10. Attempt a negative thickness and an out-of-scope hole diameter greater than the smaller plate dimension. Return an explicit validation error and preserve the previous document. Record this hole restriction as an initial product limit, not a universal CAD rule.

## Independent expected results

For the final 100 × 50 × 10 mm plate with a Ø10 mm through-hole:

- One connected solid; no accidental extra body.
- Bounds: X = [-50, 50], Y = [-25, 25], Z = [0, 10] mm.
- Hole axis: X = 0, Y = 0, parallel to Z.
- Volume: 50000 - 250*pi = 49214.60183660255 mm³.
- Analytic volume error target: absolute error <= max(0.001 mm³, expected volume × 1e-7). Report if unavailable rather than using a mesh volume and calling it analytic.
- Bounds target: absolute coordinate error <= 1e-6 mm for this simple model.
- Exported mesh volume: relative error <= 0.1%, with documented meshing settings; this does not substitute for B-rep validation.
- With width 80 mm: volume = 40000 - 250*pi = 39214.60183660255 mm³.
- With width 100 mm and depth 12 mm: volume = 60000 - 300*pi = 59057.52220392306 mm³.

## Priorities and fallback

Priority order: reproducible build → valid headless model and regeneration → persistence/undo → usable native UI → export/polish.

If the kernel spike cannot deliver the through-hole reliably within its time box, preserve its failing reproduction, build the remaining document/UI workflow around supported operations, and report the missing cut as incomplete. A narrower honest demo is useful; a hard-coded mesh pretending to be parametric CAD is not.

## Stretch goals, in order

Only start these after the acceptance workflow passes: interactive rectangle/circle placement; simple additional sketch constraints; revolve; an additional bracket example; independent STEP export validation. Do not start CAM, simulation, assemblies, cloud sync, photorealistic rendering, general fillets, or full Fusion UI parity tonight.
