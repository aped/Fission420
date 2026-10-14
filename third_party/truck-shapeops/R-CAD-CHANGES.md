# R-CAD fork of truck-shapeops

Upstream: https://github.com/ricosjp/truck, crate `truck-shapeops` 0.4.0 as published on crates.io
(source commit c23d2c25c1cd83b615d35db72cc5a4432c693e17, `path_in_vcs = truck-shapeops`).
License: Apache-2.0 (`LICENSE`, copied from the upstream repository at that commit; upstream has no
NOTICE file). Used through `[patch.crates-io]` in the workspace `Cargo.toml`.

Why a fork of the release and not upstream master: master (88ed0052, checked 2026-10-13) panics in
`IntersectionCurve::subs` on plain cylinder cuts that 0.4.0 handles, and it does not fix revolved-tool
booleans. Fixes here are kept small and self-contained so each can be offered upstream as a PR.

## Changes (newest last)

- Vendored verbatim (only `.cargo-ok` and `Cargo.lock` from the registry package dropped).
- `src/lib.rs`: `#![cfg_attr(not(debug_assertions), deny(warnings))]` and `clippy::all` removed from
  the deny list, `dead_code` allowed. Registry dependencies build with capped lints; a path
  dependency does not, and rustc 1.99 reports the unused `Alternative` enum. No code change.
- `Cargo.toml`: empty `[workspace]` so `cargo test` inside the crate does not attach to an enclosing
  workspace.
- `src/transversal/integrate`: ricosjp/truck PR #134 (open, unmerged), ported to 0.4.0. Faces untouched
  by intersection curves are classified against orientation-applied polygons, with threshold 0 for an
  inverted (difference) operand and a 3-ray majority. Its two tests are included. On 0.4 the test
  helper asserts closedness on the topology, because 0.4's tessellation does not weld every seam
  (`Oriented` mesh for a correct closed solid). Fixes: a revolved ring cut entirely inside a block
  panicked ("This shell is not oriented and closed"). It now gives the block plus a void shell, with
  volume within 1.3e-6 of 48000 - 160π.
- Known pre-existing failure: `healing::tests::step_import` reads a fixture that is not in the
  crates.io package (fails identically on the verbatim crate).
