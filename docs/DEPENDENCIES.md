# Dependencies

Policy: MIT, Apache-2.0, BSD, ISC, Zlib (or dual with one of those). `Cargo.lock` is committed.
Versions in the tables are the ones resolved in `Cargo.lock`.

| crate | version | license | source | used by |
|---|---|---|---|---|
| truck-modeling | 0.6.0 | Apache-2.0 | https://github.com/ricosjp/truck | rcad-kernel |
| truck-shapeops | 0.4.0, R-CAD fork | Apache-2.0 | vendored in `third_party/truck-shapeops` from ricosjp/truck c23d2c25 via `[patch.crates-io]`; changes listed in its `R-CAD-CHANGES.md` | rcad-kernel |
| truck-meshalgo | 0.4.0 | Apache-2.0 | same | rcad-kernel |
| truck-topology | 0.6.0 | Apache-2.0 | same | rcad-kernel |
| serde | 1.0.229 | MIT OR Apache-2.0 | crates.io | all |
| thiserror | 2.0.21 | MIT OR Apache-2.0 | crates.io | all |
| rfd | 0.17.2 (features `xdg-portal`, `wayland`; no GTK) | MIT | https://github.com/PolyMeilex/rfd | rcad (Browse… file dialogs; on Linux through the XDG desktop portal over D-Bus, pure Rust) |
| pollster | 0.4.0 (transitive) | Apache-2.0 OR MIT | crates.io (via rfd) | rcad |
| rustc-hash | 2.1.3 | Apache-2.0 OR MIT | https://github.com/rust-lang/rustc-hash (already in the tree via truck-shapeops) | rcad-kernel (Fx hashing of the tessellation's vertex-weld and edge maps) |

(Updated as crates are added; see section below for the GUI stack.)

## Core and GUI stack

| crate | version | license | used by |
|---|---|---|---|
| nalgebra | 0.35.0 (default-features off, `std`) | Apache-2.0 | rcad-sketch (SVD rank) |
| serde_json | 1.0.151 | MIT OR Apache-2.0 | document, commands, cli |
| tempfile | 3.27.0 (dev) | MIT OR Apache-2.0 | tests |
| eframe (egui, egui-wgpu through it) | 0.36.2 (`default_fonts`, `wayland`, `x11`, `wgpu`) | MIT OR Apache-2.0 | rcad-viewport, rcad |
| wgpu | 30.0.1 (transitive, through eframe) | MIT OR Apache-2.0 | rcad-viewport |
| bytemuck | 1.25.2 | Zlib OR Apache-2.0 OR MIT | rcad-viewport |
| image | 0.25.10 (`png`) | MIT OR Apache-2.0 | rcad (screenshots) |
| egui_kittest | 0.36.2 (dev) | MIT OR Apache-2.0 | rcad UI interaction tests |
| kittest | 0.4.0 (dev, transitive) | MIT OR Apache-2.0 | rcad UI interaction tests |

## Sketch text

| crate | version | license | source | used by |
|---|---|---|---|---|
| ttf-parser | 0.25.1 (default-features off, `std`) | MIT OR Apache-2.0 | https://github.com/harfbuzz/ttf-parser | rcad-sketch (sketch text: glyph outlines and font names of installed TrueType/OpenType fonts; pure Rust, no dependencies) |

Sketch text reads fonts installed on the user's system (`/usr/share/fonts`, …); no font file is
committed or shipped. The resolved glyph outlines are stored in the user's document.

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
Binary distributions (CI artifacts) carry `THIRD_PARTY_LICENSES.txt`, generated per target by
`tools/ci/third_party_licenses.py` (crate list, shipped licence and font texts, canonical texts
for crates without licence files); see docs/CI.md.

## rcad-exchange (STL/DXF adapters)

No new third-party crates: `rcad-exchange` depends only on `rcad-sketch`, `serde` 1.0.229 and
`thiserror` 2.0.21 (both already listed), plus `serde_json` 1.0.151 as a dev-dependency.
External tools used only to generate/validate fixtures (not linked, not shipped, not a project
dependency): FreeCAD 1.1.4 flatpak (LGPL-2.1, `Mesh`, `Import.writeDXFShape`/`readDXF`) and
ezdxf 1.4.2 (MIT, https://github.com/mozman/ezdxf) in a throwaway venv under `artifacts/venv`.

## STEP import / multi-body export

No new crates. STEP import (`rcad-kernel/src/step_in/`) uses the ruststep parser through
truck-stepio's re-export (`truck_stepio::r#in::ruststep`), so there is no direct dependency.

| crate | version (Cargo.lock) | license | source | used by |
|---|---|---|---|---|
| truck-stepio | 0.3.0 (default features: `in`, `out`) | Apache-2.0 | https://crates.io/crates/truck-stepio | rcad-kernel (STEP writer; parser re-export) |
| ruststep | 0.4.0 (transitive via truck-stepio) | Apache-2.0 | https://crates.io/crates/ruststep | rcad-kernel import: Part 21 parser only (AST); entity conversion is our own code |
| ruststep-derive | 0.4.0 (transitive) | Apache-2.0 | https://crates.io/crates/ruststep-derive | (proc macros of ruststep) |

Test fixtures under `crates/rcad-kernel/tests/fixtures/step/` were generated with FreeCAD 1.1.4
(LGPL-2.1, external tool, not linked) from primitives in our own script
(`tools/fixtures/step/generate.py`); they contain no third-party design data.

## 3MF adapter (`rcad-exchange::threemf`)

New *direct* dependencies of `rcad-exchange`; all three were already in `Cargo.lock`
(transitively via `image`/`png` and `wayland-scanner`), so no new crate enters the build.

| crate | version (Cargo.lock) | license | source | used for |
|---|---|---|---|---|
| miniz_oxide | 0.8.9 (default `with-alloc`) | MIT OR Zlib OR Apache-2.0 | https://crates.io/crates/miniz_oxide | raw deflate/inflate of ZIP entries (bounded `decompress_to_vec_with_limit`) |
| crc32fast | 1.5.2 | MIT OR Apache-2.0 | https://crates.io/crates/crc32fast | ZIP entry CRC-32 |
| quick-xml | 0.41.0 (`default-features = false`) | MIT | https://crates.io/crates/quick-xml | streaming XML tokenizer + namespace resolution for the 3MF model and OPC relationships |

The ZIP container code (`rcad-exchange/src/zip.rs`) and the 3MF model handling are our own.
Fixtures: FreeCAD 1.1.4 (external tool) and Python stdlib generators under `tools/fixtures/threemf/`.

## Prismatic booleans (added 2026-10-12)

| crate | version | license | used by |
|---|---|---|---|
| i_overlay | 8.1.2 (default features: no rayon) | MIT OR Apache-2.0 | rcad-kernel `prismatic.rs`: exact 2-D region booleans on an integer grid (ADR 0006) |
| i_float / i_shape / i_tree / i_key_sort | 4.1.0 / 4.0.0 / 0.19.0 / 0.11.0 (transitive) | MIT | i_overlay |

Pure Rust, no C code. `i_overlay` is the boolean engine `geo` uses.

## Icons

Command icons are original line glyphs drawn in code with the egui painter
(`crates/rcad/src/shell/icons.rs`). No third-party icon assets are used.
