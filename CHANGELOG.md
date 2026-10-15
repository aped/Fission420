# Changelog

All notable changes to R-CAD. Versions follow [Semantic Versioning](https://semver.org/);
the native document format has its own version number (`"version"` in `*.rcad.json`), and
which format versions a build reads and writes is listed in
[`docs/FORMAT_SUPPORT.md`](docs/FORMAT_SUPPORT.md).

## [1.0.0] - unreleased

First stable release. From 1.0 on, the command API (`docs/COMMANDS.md`), the CLI and the
native format follow semantic versioning: a document saved by 1.x opens in every later 1.x
build, and breaking changes to commands or files wait for 2.0.

### What 1.0 contains

- **Sketching:** lines, rectangles, circles, arcs, ellipses, splines, slots, polygons, text;
  fillets, offsets, trim/extend, sketch patterns; projection and intersection of model
  edges; dimensions (driving and driven) and geometric constraints with a solver that
  reports residuals, remaining degrees of freedom and conflicts; regions from any
  arrangement of crossing curves.
- **Solids:** extrude, revolve, loft, sweep, holes, threads (faceted), rib, shell,
  fillets and chamfers (straight edges and whole cap loops), linear/circular patterns,
  mirror, body move/copy/scale/split/combine, sheet-metal contour flange with flat pattern,
  direct edits of imported STEP faces. Cuts that separate a body produce one body per piece,
  with stable piece identity across edits.
- **History:** parametric feature history with named parameters and expressions,
  suppression, rollback, reorder, Edit Feature, configurations, per-feature health with
  partial regeneration and safe-mode open, transactional commits, undo/redo.
- **Assemblies, drawings, inspection, mesh tools, print export:** see the README feature
  table.
- **Interchange:** STEP (exact B-rep subset, assemblies, surface bodies), STL, 3MF, DXF,
  SVG; native format v9 with migration from every earlier version.

### New in the 1.0 release cycle

- **Faster booleans and regeneration.** Intersection curves reuse a cached per-surface
  presearch grid (bit-identical results), tessellation maps use a faster hasher, the
  post-boolean result check reuses meshes and indexes profile polygons. Kernel-heavy
  regenerations are 2-7x faster; a plate cut regenerates in about 0.1 s. Bounds and
  tessellations of unchanged bodies are reused, so a parameter edit on the rounded
  enclosure template takes about a third less time; hover and click picking use a
  bounding-volume hierarchy (milliseconds to microseconds on large meshes).
- **Release binaries** are built with a `dist` profile (fat LTO, one codegen unit): 5-10 %
  faster and about a quarter smaller.
- **Edit Feature** reopens a prefilled dialog for fillets, chamfers, shells, patterns,
  mirror, loft, sweep, thread and align, not only extrude, revolve and hole.
- **Messages:** an action that cannot run says why over the 3D view; error banners say
  what failed and what to do next.
- **Discoverability:** menus show keyboard shortcuts, disabled buttons name their key,
  the empty-document card offers templates, and **File > About R-CAD** shows the version.
- **Status bar and title** show one save state (not saved yet / unsaved changes / saved).
- **File > Quit** (Ctrl+Q) with the unsaved-changes prompt; Esc closes About/Help/
  Preferences first; every dialog value shows its unit; faces and sketch planes have plain
  names ("Extrude1 end cap"); dragged panel widths are remembered.
- Icons the UI font could not draw (shown as empty boxes) are replaced.
- **CI:** a fast lint job, one test binary per crate (test build about 4x faster, test
  archive 10x smaller), sharded nextest runs, Linux, Windows and macOS all test-gated,
  and release archives with SHA-256 checksums. A Linux leg takes about 11 minutes, down
  from 35.

### Breaking changes since the 0.x prototype

- Documents must declare `"format": "r-cad"`. Files written under the project's former
  name are no longer recognised; re-save them with a 0.x build, or change the `format`
  field by hand (the rest of the file is unchanged).
- The former-name autosave locations and environment variables are no longer read; use
  `RCAD_RECOVERY_DIR` and `RCAD_AUTOSAVE_SECS`.
- Workspace crates are versioned together at 1.0.0 and are not published to crates.io.
