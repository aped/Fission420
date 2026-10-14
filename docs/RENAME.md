# Rename to R-CAD (2026-10-13)

The project was called **Fission420** (crates `f420-*`, files `*.f420.json`) and is now
**R-CAD**. The **R** is for **Rust**, and it is said like "our": *our CAD*. This page
lists what changed and what still reads the old names.

## What changed

| | before | after | compatibility |
|---|---|---|---|
| product / repo | Fission420, `aped/Fission420` | R-CAD, `aped/R-CAD` | GitHub redirects the old repo URL |
| crates | `f420-app`, `f420-cli`, `f420-{commands,document,exchange,kernel,sketch,viewport}` | `rcad` (app; binary `rcad`), `rcad-cli`, `rcad-*` | none needed (workspace-internal, unpublished) |
| native files | `*.f420.json`, `"format": "fission420"` | `*.rcad.json`, `"format": "r-cad"` | old files open unchanged (`LEGACY_FORMAT_NAMES` in `crates/rcad-document/src/io.rs`). The extension was never checked. The format **version is unchanged**; the rename touches only the name. A build from before the rename refuses new files with "not a fission420 document" |
| autosave | `F420_RECOVERY_DIR`, `F420_AUTOSAVE_SECS`, `…/fission420/recovery`, `*.autosave.f420.json` | `RCAD_*`, `…/r-cad/recovery`, `*.autosave.rcad.json` | the old variables are read when the new ones are unset; the old directory and suffix are still searched at start-up (`crates/rcad/src/recovery.rs`) |
| other variables | `F420_TRACE_ENGINE`, `F420_PROBE`, `F420_WRITE_EXAMPLES`, … | `RCAD_*` | none (developer and test diagnostics) |

Tests that guard the compatibility: `crates/rcad-document/tests/legacy_name.rs` (a frozen
`fission420` document saved as `*.f420.json` opens and re-saves as `r-cad`; other names are
still refused), `crates/rcad/tests/recovery_legacy.rs`, and the frozen fixtures under
`crates/rcad-commands/tests/fixtures/native_v*/`. Their contents were never rewritten, so
they still say `fission420`.

Not renamed on purpose: the externally validated exports in `examples/print/` (their STEP
headers and 3MF metadata still name Fission420; regenerating them means re-running the
FreeCAD and slicer validation), and the screenshots (the app title bar in them reads
Fission420 until they are recaptured).

The whole rename is one deterministic transform, `tools/rename_f420.py`:

- `tree` applies it to a checkout.
- `patch` applies it to a patch made on the pre-rename history, so a branch from before
  the rename can be translated and applied to the renamed tree.
- `check` lists any `f420`/`Fission420` left outside the allowed compatibility code.
