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
