# Dependencies

Policy: MIT, Apache-2.0, BSD, ISC, Zlib (or dual with one of those). `Cargo.lock` is committed.

| crate | version | license | source | used by |
|---|---|---|---|---|
| truck-modeling | 0.6.0 | Apache-2.0 | https://github.com/ricosjp/truck | f420-kernel |
| truck-shapeops | 0.4.0 | Apache-2.0 | same | f420-kernel |
| truck-meshalgo | 0.4.0 | Apache-2.0 | same | f420-kernel |
| serde | 1.x | MIT OR Apache-2.0 | crates.io | all |
| thiserror | 2.x | MIT OR Apache-2.0 | crates.io | all |

(Updated as crates are added; see section below for the GUI stack.)

## Added during run 1

| crate | version | license | used by |
|---|---|---|---|
| nalgebra | 0.35.0 (default-features off, `std`) | Apache-2.0 | f420-sketch (SVD rank) |
| serde_json | 1.0.151 | MIT OR Apache-2.0 | document, commands, cli |
| tempfile | 3.27 (dev) | MIT OR Apache-2.0 | tests |
| eframe / egui / egui-wgpu | 0.36.2 (`default_fonts`, `wayland`, `x11`, `wgpu`) | MIT OR Apache-2.0 | f420-viewport, f420-app |
| wgpu | 30.0.1 | MIT OR Apache-2.0 | f420-viewport |
| bytemuck | 1.25 | Zlib OR Apache-2.0 OR MIT | f420-viewport |
| image | 0.25.10 (`png`) | MIT OR Apache-2.0 | f420-app (screenshots) |

External tools used only for validation (not linked, not shipped): FreeCAD 1.1.4 flatpak (LGPL-2.1).
Full transitive license audit: see `artifacts/licenses.txt` (generated) / section below.

### Transitive C code (audited 2026-10-10)
- `lzma-sys` 0.1.20 (liblzma, 0BSD/public-domain sources via `cc`) ← `xz2` ← `vtkio` ← `truck-meshalgo`
  default feature `vtk`, re-enabled by `truck-shapeops` 0.4 which depends on meshalgo with default
  features. It is compression for VTK file I/O, not geometry; the kernel stays pure Rust. Removing it
  needs a Truck patch (`[patch]`) — deferred.
- `wayland-sys`, `renderdoc-sys`: OS/graphics integration (dlopen bindings), allowed by policy.
- Fonts in `epaint_default_fonts`: OFL-1.1 / Ubuntu Font Licence (font licences, permissive for embedding).
- `self_cell` (Apache-2.0 OR GPL-2.0-only) and `r-efi` (MIT OR Apache-2.0 OR LGPL) are used under their permissive option.
No package lacks a permissive option (`artifacts/licenses.txt`).
| egui_kittest | 0.36.2 (dev) | MIT OR Apache-2.0 | f420-app UI interaction tests |
| kittest | 0.4.0 (dev, transitive) | MIT OR Apache-2.0 | f420-app UI interaction tests |

## f420-exchange (run 3, STL/DXF adapters)

No new third-party crates: `f420-exchange` depends only on `f420-sketch`, `serde` 1.0.229 and
`thiserror` 2.0.21 (both already listed), plus `serde_json` 1.0.151 as a dev-dependency.
External tools used only to generate/validate fixtures (not linked, not shipped, not a project
dependency): FreeCAD 1.1.4 flatpak (LGPL-2.1, `Mesh`, `Import.writeDXFShape`/`readDXF`) and
ezdxf 1.4.2 (MIT, https://github.com/mozman/ezdxf) in a throwaway venv under `artifacts/venv`.

## STEP import / multi-body export (run 3)

No new crates. STEP import (`f420-kernel/src/step_in/`) uses the ruststep parser through
truck-stepio's re-export (`truck_stepio::r#in::ruststep`), so there is no direct dependency.

| crate | version (Cargo.lock) | license | source | used by |
|---|---|---|---|---|
| truck-stepio | 0.3.0 (default features: `in`, `out`) | Apache-2.0 | https://crates.io/crates/truck-stepio | f420-kernel (STEP writer; parser re-export) |
| ruststep | 0.4.0 (transitive via truck-stepio) | Apache-2.0 | https://crates.io/crates/ruststep | f420-kernel import: Part 21 parser only (AST); entity conversion is our own code |
| ruststep-derive | 0.4.0 (transitive) | Apache-2.0 | https://crates.io/crates/ruststep-derive | (proc macros of ruststep) |

Test fixtures under `crates/f420-kernel/tests/fixtures/step/` were generated with FreeCAD 1.1.4
(LGPL-2.1, external tool, not linked) from primitives in our own script
(`tools/fixtures/step/generate.py`); they contain no third-party design data.

## 3MF adapter (run 3, `f420-exchange::threemf`)

New *direct* dependencies of `f420-exchange`; all three were already in `Cargo.lock`
(transitively via `image`/`png` and `wayland-scanner`), so no new crate enters the build.

| crate | version (Cargo.lock) | license | source | used for |
|---|---|---|---|---|
| miniz_oxide | 0.8.9 (default `with-alloc`) | MIT OR Zlib OR Apache-2.0 | https://crates.io/crates/miniz_oxide | raw deflate/inflate of ZIP entries (bounded `decompress_to_vec_with_limit`) |
| crc32fast | 1.5.2 | MIT OR Apache-2.0 | https://crates.io/crates/crc32fast | ZIP entry CRC-32 |
| quick-xml | 0.41.0 (`default-features = false`) | MIT | https://crates.io/crates/quick-xml | streaming XML tokenizer + namespace resolution for the 3MF model and OPC relationships |

The ZIP container code (`f420-exchange/src/zip.rs`) and the 3MF model handling are our own.
Fixtures: FreeCAD 1.1.4 (external tool) and Python stdlib generators under `tools/fixtures/threemf/`.
