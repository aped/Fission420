# ADR 0004 — Format adapters, units, import diagnostics

Status: accepted (run 3, 2026-10-10)

## Decision
* Adapters are pure functions from bytes to typed results, with no document access:
  * `f420_kernel::import_step(text, &StepImportOptions) -> Result<StepImport>`: exact B-rep
    solids (Truck), units, names, warnings, per-solid planar face table. Lives in the kernel
    because it builds Truck types (only the kernel touches Truck).
  * `f420_exchange::stl::read(bytes) -> Result<StlMesh>` and `f420_exchange::mesh` diagnostics.
  * `f420_exchange::dxf::import(bytes, &DxfImportOptions) -> Result<DxfImport>` (to an
    unconstrained `f420_sketch::Sketch`) and `dxf::export(&Sketch, ...) -> String`.
* The document/command layer turns adapter results into one transaction (all-or-nothing).
  UI, CLI and tests use the same `Command::Import*` path. Work runs off the UI thread in the
  app engine, tagged with document id + revision like every other job.
* Units: internal unit is the millimetre. Conversion happens once, in the adapter, with the
  scale reported back (`unit_name`, `to_mm`, `source: Declared | Chosen | Assumed`). STL and
  unitless DXF require an explicit unit choice (default offered: mm, shown in the dialog).
* Bounds: every adapter enforces byte size, entity/triangle count and recursion limits and
  converts kernel panics to errors (`guarded`). Malformed input yields an error, never a panic.
* Unsupported content is reported, never silently dropped. Default policy is all-or-nothing;
  partial import requires an explicit `allow_partial` option and returns counts.
* Support is documented in docs/FORMAT_SUPPORT.md with fixture evidence; an extension filter is
  not support.
