# Continuous integration

`.github/workflows/ci.yml` runs the local gate, `cargo xtask check`, on four platforms and
publishes release archives. Each platform is one call of the reusable workflow
`.github/workflows/check.yml`, which splits the gate into parallel jobs. Linux
(Bazzite/Fedora atomic) stays the primary development environment: `cargo xtask check` with
no arguments still runs everything in one process.

## Pipeline

```
ci.yml
├─ ubuntu-24.04 ── check.yml: lint
│                  build ─┬─ test 1/2, test 2/2 ─┬─ package
│                         └─ acceptance+examples ┘
├─ debian-13 ───── check.yml: build ─┬─ test 1/2, test 2/2
│                                    └─ acceptance+examples
├─ macos-arm64 ─── same as Ubuntu without lint   (main, v* tags, PRs into main, manual)
├─ windows-x64 ─── same as Ubuntu without lint   (same; not gating yet)
└─ release (v* tags): needs every leg; draft GitHub release with SHA256SUMS
```

| job | runs | `cargo xtask` | notes |
|---|---|---|---|
| lint | Ubuntu leg only | `check --stage lint`: layout, public text, `fmt --check`, `clippy --workspace --all-targets -D warnings` | Dev profile; fails within a few minutes. Own cache (`lint-…`). |
| build | every leg | `check --stage doctest` (last) | `cargo fetch --locked`; release binaries; `cargo nextest archive --workspace --release` → artifact `tests-<leg>`; binaries plus `THIRD_PARTY_LICENSES.txt` → artifact `bin-<leg>` (tarred, so the executable bit survives). `RCAD_GIT_REV` is the short commit, so `--version` reports it. |
| test k/N | every leg, N shards | none (nextest directly) | Downloads the archive, runs `cargo-nextest nextest run --archive-file … --workspace-remap . --profile ci --partition hash:k/N`. No Rust toolchain, no compilation. JUnit report → artifact `junit-<leg>-<k>`. |
| acceptance + examples | every leg | `check --stage replay` with `RCAD_CLI=stage/rcad-cli` | Runs the prebuilt CLI (no release build). `rcad-cli acceptance` reads its fixtures from the source tree through compile-time paths, which match because the checkout path is the same as in the build job. First checks that `rcad --version` and `rcad-cli --version` report the version and commit. Ubuntu warns when the replays rewrite committed `examples/`. |
| package | publishing legs | none | Ubuntu runner for every platform; assembles `r-cad-<version>-<platform>.tar.gz` (`.zip` for Windows) and its `.sha256`. |
| release | `v*` tags | none | Checks every `.sha256`, writes `SHA256SUMS`, creates a **draft** release titled `R-CAD <version>` with all archives and checksums. Publishing the draft is a manual step. |

Each job fails on the first failing command; nothing is retried or masked except the
nextest retries listed under [Tests](#tests-nextest).

### Triggers and cost

- **Routine pushes** (`work/**`, `ci/**`, PRs) run the two Linux legs. Windows, macOS and the
  archives come from `main`, `v*` tags, PRs into `main` and manual runs (*Actions → CI → Run
  workflow*, which runs all four legs and publishes).
- **Docs-only changes do not trigger CI:** `**.md`, `docs/**` and `LICENSE-*`. Path filters
  do not apply to tag pushes.
- **Superseded runs are cancelled** on work branches and PRs. Runs on `main` and tags always
  finish.
- **Retention:** test archives and staged binaries 1 day (they only feed later jobs of the
  same run); archives, JUnit reports and failure logs 7 days.
- A push runs 9 Linux jobs (lint; per Linux leg build, two shards, replay) and, on `main`,
  10 more for Windows and macOS (including packaging). Standard runners are free for this
  public repository; the job count stays under the 20 concurrent jobs (5 macOS) of the free
  plan.

### Gating

| leg | runner | gating | archive |
|---|---|---|---|
| ubuntu-24.04 | `ubuntu-24.04` | yes | `r-cad-<version>-linux-x86_64.tar.gz` (glibc 2.39 baseline, so it also runs on Debian 13 and current Fedora) |
| debian-13 | `ubuntu-24.04` + container `debian:trixie` | yes | none (it would need a newer glibc than the Ubuntu build; this leg only gates) |
| macos-arm64 | `macos-15` (Apple silicon) | yes | `r-cad-<version>-macos-arm64.tar.gz` |
| windows-x64 | `windows-2025` (MSVC) | **not yet** | `r-cad-<version>-windows-x86_64.zip` |

A **non-gating** leg runs everything and shows red when it fails, but its jobs carry
`continue-on-error`, so the workflow still succeeds. It packages whenever its build
succeeded; `BUILD_INFO.txt` then says the checks were not required and links the run. A
gating leg packages only after its tests and replays passed.

To change a leg's gating, set `gate:` for it in `ci.yml`. Windows becomes gating once one
full Windows run is green (until now no Windows run got past clippy; see the status below).

### Caching

`Swatinem/rust-cache@v2`, one cache per job type and leg (`lint-<leg>`, `build-<leg>`).
Only `main` writes caches (`save-if`); other branches and PRs restore `main`'s. This keeps the
10 GB repository cache from filling with per-branch copies that evict each other. The build
cache keeps `cache-on-failure`, so a failing leg on `main` still has warm dependencies on
the next run. Test and replay jobs compile nothing and need no cache. The cost: a branch
that changes `Cargo.lock` rebuilds the changed dependencies on every push until it merges.

## Tests (nextest)

CI installs `cargo-nextest` (pinned, `NEXTEST_VERSION` in `check.yml`) with
`taiki-e/install-action` (pinned tag). `.config/nextest.toml` defines two profiles:

- **`default`** (local): no fail-fast; tests running over 60 s are reported, never killed.
- **`ci`**: no fail-fast; tests are reported every 60 s and **killed after 15 minutes**
  (`terminate-after = 15`). That is far above the slowest legitimate test (see the timings
  below), so only a hung test is stopped, long before the job timeout. **No blanket
  retries.** The threaded-engine UI tests with wall-clock guards (`exchange_ui`,
  `measure_imports`, `frame_pacing`; L6 below) get one retry each, by filter. A test that
  passes on retry is reported as `FLAKY` in the log, the final summary and `junit.xml`; it
  is not silently green. JUnit output: `target/nextest/ci/junit.xml`.

`cargo xtask check` picks the profile: `NEXTEST_PROFILE` if set (nextest reads it itself),
else `ci` when `CI` is set (GitHub Actions sets it), else the default profile. Without
nextest installed it falls back to `cargo test --workspace --release --no-fail-fast`, which
also runs the doc tests; with nextest, the doc tests run separately
(`cargo test --workspace --release --doc`), because nextest does not run them.

**One test binary per crate.** Each crate's integration tests are modules of a single
binary, `tests/it/main.rs` with one `mod` per file in `tests/it/` (shared helpers such as
`rcad-commands`' `support` and `arc_corpus` are declared once there). Linking one release
binary per test file against Truck and egui was most of the build time and of the archive's
size. Measured on the development machine, a clean `cargo nextest archive --workspace
--release` (fresh target dir, 4 build jobs) took 996 s and wrote 1.18 GB with 199 test
binaries that hold tests, and 244 s and 116 MB with 23 (the unit-test binaries, six `it`
binaries and the eight separate ones below); the same 1345 tests run in both. The machine
was shared with other builds during both runs, more heavily during the first, so the ratio
is approximate. A test's full name starts with its file's module, so filters read `cargo test -p rcad-commands --test it arc_fuzz::` or, in nextest,
`package(rcad-commands) & test(/^arc_fuzz::/)`. Tests that change process-wide state
stay separate binaries in `tests/`, because plain `cargo test` runs all tests of one binary
as threads of one process: `rcad-exchange/tests/cancel.rs`,
`rcad-commands/tests/{cancel_regen,import_cancel}.rs` and
`rcad/tests/{cancel_regen_ui,import_cancel_ui,stale_state}.rs` set or race the
process-wide cancellation flag, `rcad/tests/recovery.rs` sets environment variables and
`rcad/tests/file_picker_ui.rs` replaces the process-wide file-dialog backend. A new test
that does the same belongs next to them, not in `tests/it/`. (nextest runs every test in
its own process, so it is not affected either way.)

**Sharding.** The build job compiles the test binaries once into a `cargo nextest archive`;
each shard downloads it and runs a `hash:k/N` partition (stable as tests are added). The
slowest test files start first (`priority` in the `ci` profile), so none of them is left
running alone at the end of a shard. Tests resolve fixtures through `CARGO_MANIFEST_DIR` and
write to `CARGO_TARGET_TMPDIR`, both compiled in as absolute paths under the checkout; the
checkout path is the same on every runner of one platform, and the shard creates
`target/tmp` before running.

**Why two shards.** Measured on the development machine (16 threads, shared with other
builds) from the archive of this tree: 1330 tests, 1658 test-seconds in total, 238 s wall
clock. One test dominates: `cross_process_determinism` (213 s), then `arc_fuzz::arc_case_07`
(136 s), `gate_fuzz` (80 s) and `rounded_enclosure` (62 s). Summed test time per shard:

| partition | shard sums (s) | longest test in the heaviest shard |
|---|---|---|
| `hash:k/2` | 720, 938 | 213 |
| `hash:k/3` | 380, 563, 715 | 213 |
| `slice:k/3` | 787, 454, 417 | 213 |
| `count:k/3` (per binary) | 736, 464, 457 | 213 |

On a 4-vCPU runner a shard takes about max(sum / 4, longest test) times the runner's
slowdown. With two shards that is about 235 s of local time against about 215 s with
three: the single 213 s test bounds both, so a third runner (another archive download and
about a minute of setup) buys almost nothing. Revisit when the slowest tests get faster.

The archive is about 120 MB (zstd). The build job uploads it once (artifact
compression off, it is already compressed) and each shard downloads it.

**Expected wall clock (Linux leg, warm cache).** Before the test binaries were merged
(Actions run ID 38595093305, `ubuntu-24.04`): release binaries 88 s, compiling the release
test binaries about 820 s, running them with `cargo test` 1050 s, acceptance 105 s, replays
about 50 s; 35 min in one job. With one test binary per crate the test build should shrink
roughly in proportion to the local measurement above (to about 4–6 min), so the build job
is about 8–10 min (binaries, archive, upload, doc tests) → shards about 6–9 min each in
parallel with the replay job (about 4 min) → package 1 min: about **15–20 min** end to end,
now bounded by the slowest tests rather than by linking. Lint reports within about 3 min.
These are estimates until a GitHub run of this layout confirms them.

## `cargo xtask check` stages

```bash
cargo xtask check                          # everything, in order (the local gate)
cargo xtask check --stage lint             # layout, public text, fmt, clippy -D warnings
cargo xtask check --stage test             # release tests (nextest + doc tests, or cargo test)
cargo xtask check --stage doctest          # doc tests only
cargo xtask check --stage replay           # headless acceptance, example replays
RCAD_CLI=target/release/rcad-cli cargo xtask check --stage replay   # prebuilt CLI
cargo xtask public-text                    # the public-text check alone
```

`--stage` may be repeated; stages always run in the order above and stop at the first
failure. `replay` creates `artifacts/` itself (the workflows export into it).

### Toolchain

The toolchain is pinned to **1.99.0** (`RUST_TOOLCHAIN` in `check.yml`), which matches the
local toolchain (Homebrew `cargo`/`rustc` 1.99.0 is first on `PATH` on the dev machine). This
keeps clippy's `-D warnings` the same in CI and locally.

- **Effective MSRV is 1.95** (`rust-version` in `Cargo.toml`). egui/eframe/egui_kittest 0.36 require it.
- **Bumping:** change `RUST_TOOLCHAIN` in the same commit as any clippy fixes the new version needs.
- **No `rust-toolchain.toml`.** The Homebrew `cargo` ignores it, so it would only make rustup users diverge from the primary environment.
- **Test jobs** need no Rust toolchain: they run the archived binaries with `cargo-nextest`.

### Native build dependencies

The only C code compiled is `lzma-sys`, pulled in by `truck-shapeops → truck-meshalgo
(default features) → vtkio → xz2`. It is third-party and not an application-owned kernel. It
needs a C compiler:

- **Debian container:** `build-essential pkg-config liblzma-dev` (`DEBIAN_PACKAGES` in `check.yml`; keep `tools/ci/container_check.sh` in step). With these it links the system liblzma.
- **Ubuntu, macOS and Windows:** the runner images already have what it needs. MSVC builds the bundled source.

Test-only runtime dependency: the sketch-text tests (`rcad-sketch/tests/it/text.rs`,
`rcad-commands/tests/it/{sketch_text,text_emboss,rounded_enclosure}.rs`, `rcad/tests/it/sketch_text_ui.rs`)
need the **Liberation Sans** font and fail, with that message, when it is missing
(`RCAD_SKIP_FONT_TESTS=1` skips them explicitly). The Ubuntu image has it; the Debian container
installs `fonts-liberation` (2.1.5); on macOS and Windows the local action
`.github/actions/test-font` fetches the upstream 2.1.5 TTF release, pinned by SHA-256, and
points `RCAD_FONT_DIRS` at it. All are version 2.1.5, but the distro packages are rebuilt from
source and differ in bytes from upstream (and from each other); the tests compare outlines
with tolerances, so this does not matter.

Nothing else links natively at build time. Wayland, X11, xkbcommon, EGL and Vulkan are
loaded with `dlopen` at runtime. Fonts are embedded. The GUI needs those runtime libraries
only to *run*; tests never open a window.

### Tests on headless runners

All `rcad` tests use `egui_kittest` without wgpu and run on the app's fallback camera,
so they are pure CPU: no display, no GPU, no `WAYLAND_DISPLAY`/`DISPLAY`. The one GPU test,
`rcad-viewport/src/gpu_tests.rs`, is `#[ignore]` and skips when no adapter is present. Never
set `RCAD_WRITE_EXAMPLES` in CI, because it overwrites `examples/`.

### Release archives

Each archive `r-cad-<version>-<platform>` holds `rcad` and `rcad-cli`, `README.md`,
`CHANGELOG.md`, `LICENSE-MIT`, `LICENSE-APACHE`, `THIRD_PARTY_LICENSES.txt`, the committed
`examples/`, an empty `artifacts/` (the example workflows export into it) and
`BUILD_INFO.txt` (version, commit, platform, toolchain, whether the checks gated it, the CI
run). Next to each archive is `<archive>.sha256`; a tagged release adds `SHA256SUMS` for all
of them. Builds are unsigned and there are no installers, by design. `tools/release.sh`
builds the Linux archive locally with the same naming and checksums.

R-CAD is MIT OR Apache-2.0. The binaries also embed about 250 third-party crates, all
permissive or used under a permissive option (docs/DEPENDENCIES.md). Among them are Truck,
nalgebra and winit under Apache-2.0 only, and egui's bundled fonts under OFL-1.1, the Ubuntu
Font Licence and Hack's licence. Redistributing them requires their notices.
`tools/ci/third_party_licenses.py <target> <out>` writes `THIRD_PARTY_LICENSES.txt` for
each platform from `cargo metadata`. It lists every crate linked into `rcad`/`rcad-cli`
and includes the licence texts each one ships, plus the canonical texts for the 30 or so
crates (Truck, egui) whose packages ship none.

## Reproducing locally

- **Linux legs (podman, no root):**

  ```bash
  tools/heavy.sh tools/ci/container_check.sh debian:trixie   # debian-13 leg
  tools/heavy.sh tools/ci/container_check.sh ubuntu:24.04    # stricter than the runner image
  ```

  The cache directory (`target-ci-<image>/`: toolchain, registry, target) is on disk, never in `/tmp`.

- **The CI test profile and sharding:**

  ```bash
  cargo nextest archive --workspace --release --archive-file artifacts/ci/tests.tar.zst
  CI=1 cargo xtask check --stage test                     # whole suite, ci profile
  cargo nextest run --archive-file artifacts/ci/tests.tar.zst --workspace-remap . \
    --profile ci --partition hash:1/2                      # one shard
  ```

- **Windows/macOS clippy from Linux** (type-check only, nothing links):

  ```bash
  rustup toolchain install 1.99.0 --profile minimal -c clippy -t aarch64-apple-darwin -t x86_64-pc-windows-msvc
  # A stand-in C "compiler" for lzma-sys: it only creates the object and archive files.
  printf '#!/bin/sh\nfor a; do case "$a" in -Fo*) : > "${a#-Fo}";; -out:*) : > "${a#-out:}";; esac; done\n' > target/fakecc
  chmod +x target/fakecc
  PATH=$(dirname "$(rustup which --toolchain 1.99.0 cargo)"):$PATH \
    CC_x86_64_pc_windows_msvc=$PWD/target/fakecc AR_x86_64_pc_windows_msvc=$PWD/target/fakecc \
    PKG_CONFIG_ALLOW_CROSS=1 CARGO_TARGET_DIR=$PWD/target-ci-x \
    cargo clippy --workspace --all-targets --target x86_64-pc-windows-msvc -- -D warnings
  # macOS: the same with --target aarch64-apple-darwin (lzma-sys then resolves the host
  # liblzma through pkg-config; PKG_CONFIG_ALLOW_CROSS allows it).
  ```

  The toolchain's directory comes first on `PATH` because the Homebrew `cargo`/`rustc` have
  no cross targets. `lzma-sys` compiles C with MSVC's `cl.exe`, which Linux lacks; the stand-in
  is enough because `clippy` never links. This catches Unix-only APIs (`std::os::unix`) and
  `cfg` mistakes; it does not run anything.

- **Workflow lint:** `actionlint` 1.7.12 with shellcheck 0.11.0 (`actionlint -shellcheck
  <path>`); neither is installed system-wide. Pinned release binaries, checked against the
  published checksums, are enough.

## Platform items

From an audit of the tree at `f0bcf6c` and from the GitHub runs since. Severity: **T** = a
test fails on that platform; **R** = runtime risk, no test failure expected; **C** = cosmetic.

| id | sev | platforms | where | what breaks | status |
|---|---|---|---|---|---|
| F1 | T | Windows | `crates/rcad-commands/src/scenarios.rs`; compared in `tests/it/scenario_examples.rs` | Workflow paths built with `Path::join` serialised as `artifacts\scenario_a.stl`, never equal to the committed `artifacts/scenario_a.stl`. | **applied** (`0538511`): a `portable(out, name)` helper joins the paths with `/`. Not yet confirmed on a Windows runner (W1 stopped every Windows run since at clippy). |
| F2 | T | Windows | `examples/imported_step_hole.rcad.json` embeds a STEP fixture byte-for-byte; `tests/it/imports.rs` compares them. | With `core.autocrlf=true` (the runner default) the fixture is checked out with CRLF. | **applied:** root `.gitattributes` (`* text=auto eol=lf`, mesh and archive files binary). |
| F3 | T? | macOS, Windows | Committed workflows hold solved floats compared exactly (`hypot`/`atan2` through libm). | glibc, Apple libm and the UCRT do not promise last-bit-identical results. | Checked on the first Windows and macOS runs: did not occur. If it ever does, compare workflow numbers with a relative tolerance (about 1e-9). |
| F4 | T | Debian container, macOS, Windows | `crates/rcad-sketch/src/font.rs` (`font_dirs`); the sketch-text tests | No Liberation Sans on those systems. | **applied:** `fonts-liberation` in the container, the pinned download on macOS/Windows (`.github/actions/test-font`), and `font_dirs` searches the macOS and Windows font folders. |
| W1 | T (build) | Windows | `crates/rcad-commands/tests/it/persistence_failures.rs` | Used `std::os::unix` and `Permissions::from_mode` unconditionally, so clippy failed to compile the test on Windows (Actions run ID 38595093305). | **applied:** the two read-only-directory tests and the import are `#[cfg(unix)]`; they still run on Linux and macOS. `clippy --all-targets -D warnings` for `x86_64-pc-windows-msvc` and `aarch64-apple-darwin` passes from Linux (see Reproducing locally). |
| M1 | T | macOS | `crates/rcad/tests/recovery.rs` (`autosave_crash_recover_and_save`) | Compared the canonical path the app reopens (`/private/var/folders/…`) with the raw temporary path (`/var/folders/…`, a symlink). | **applied:** the test canonicalizes its temporary directory. |
| M2 | T (race) | slow runners; seen on macOS | `crates/rcad/tests/stale_state.rs` (`import_cancel_then_other_format`) | The test queued `NewDocument` and opened the Import form without waiting. A document switch closes every dialog except a pending import, so a late `NewDocument` reply closed the form before its path was typed ("No nodes found matching … Import file path"; the tree showed no dialog and *Sketch1* already present). | **applied:** `queue_slow_work` waits for the new document before queueing the slow edits, as `new_document_while_import_pending` already did. No retry. |
| L1 | T? | macOS, Windows | `crates/rcad-commands/tests/it/gate_fuzz.rs`; `crates/rcad-kernel/src/ops/tests.rs` | Accept/refuse counts tuned on one Linux run; Truck's circle/NURBS code uses libm `sin`/`cos`. | Did not occur on macOS. If one diverges, give the threshold a documented margin; never drop the case. |
| L2 | R | Windows | `crates/rcad-cli/src/main.rs`; `rcad --sync` | Geometry on the main thread, which gets 1 MiB on Windows. | **applied (mitigation):** `.cargo/config.toml` links MSVC binaries with `/STACK:8388608`. Long term: run the CLI body on a thread with an explicit stack size. |
| L3 | R | all | `xtask/src/main.rs` | The example replays write to `artifacts/` but never create it. | **applied:** `cargo xtask check --stage replay` creates `artifacts/`; release archives ship an empty one. |
| L4 | R | Windows, macOS | `crates/rcad/src/recovery.rs` (`recovery_dirs`) | The autosave directory is `RCAD_RECOVERY_DIR`, then `XDG_STATE_HOME`, then `HOME/.local/state`. Windows usually has no `HOME`, so autosave is silently off; on macOS it lands in a Linux-style path. | Open. Fall back to `%LOCALAPPDATA%` on Windows and `~/Library/Application Support` on macOS. |
| L5 | R | Windows | `crates/rcad-document/src/io.rs` (atomic save) | Copies the target's permissions onto the temp file before the rename; a read-only target or one held open by another process makes the rename fail (the previous file survives). | Open. Do not copy the read-only bit on Windows; retry the rename briefly on `PermissionDenied`. |
| L6 | T? | slow runners | `crates/rcad/tests/it/exchange_ui.rs` (20 s), `tests/it/measure_imports.rs` (30 s), `tests/it/frame_pacing.rs` | Wall-clock limits tuned on a 9800X3D. | **CI mitigation:** one retry each in the nextest `ci` profile, reported as `FLAKY`. Still worth raising the limits to about 120 s; they guard against hangs, not performance. |
| L7 | T (intermittent) | macOS | `crates/rcad-commands/tests/it/export_determinism.rs` (`bracket_exports_and_saves_are_byte_identical`) | Two saves of the mounting bracket differed in the cached `TessellationEstimate` volume (Actions run ID 38346875644), likely a summation order that follows allocation order. | **applied:** the saved cache volume is rounded to 1e-6 mm³ (`CACHE_VOLUME_STEPS` in `crates/rcad-document/src/io.rs`). Not failed on macOS since; the canonical-order sum is the real fix. |
| C1 | C | Windows | `crates/rcad/src/main.rs` | The GUI binary opens a console window. | `#![cfg_attr(windows, windows_subsystem = "windows")]` once a log location exists. |
| C2 | C | non-Linux | `crates/rcad/Cargo.toml`, `crates/rcad-viewport/Cargo.toml` | eframe's `wayland`/`x11` features do nothing there. | None. |
| C3 | C | all | `tools/heavy.sh`, `tools/*freecad*`, `tools/print/slice_bambu.sh` | Local-only tooling. | None for CI. |

Checked and fine (audit): no absolute paths in Rust code or fixtures; no Windows-illegal,
reserved or case-colliding file names; no symlinks or file locking; no test spawns an
external tool (FreeCAD and Bambu Studio checks are separate scripts); the importers accept
CRLF; cross-document float comparisons use tolerances apart from F3.

### Status

| run | ubuntu-24.04 | debian-13 | macos-arm64 | windows-x64 |
|---|---|---|---|---|
| early runs (one job per leg) | pass, 9–27 min | pass, 9–28 min | pass twice, then L7 | F1 |
| Actions run ID 38595093305 (`48e0dfa`, one job per leg) | pass, 35 min (test stage 1787 s, of which about 820 s compiling the test binaries) | pass | M1, M2 (fixed since) | W1 at clippy after 4 min (fixed since) |

The split pipeline described above has not run on GitHub yet when this was written; the
first run on `main` confirms (or corrects) the wall-clock estimate and is the first Windows
run to get past clippy. Next, in value order:

1. Confirm macOS green with the M1/M2 fixes (it is gating now).
2. Get one full Windows run green, then set `gate: true` for it.
3. Fix L4 and L5; raise the L6 limits.

## Regression tests added with CI

These are headless, need no external tools, and use analytic expected values, not snapshots of our own output.

| file | covers |
|---|---|
| `crates/rcad-commands/tests/it/native_formats.rs` (`v4_printable_documents`, `v4_parameters_drive_a_loaded_document`, `committed_printable_examples`) | Frozen v4 documents with parameters and expressions (`tests/fixtures/native_v4/`) open, regenerate to analytic volumes, re-save identically, and still respond to parameter edits. The enclosure, coupon and mounting-bracket examples are now opened by a test, not only rewritten by the replays. |
| `crates/rcad-document/tests/it/native_fuzz.rs` | Every truncation of a v3 and a v4 file is a `DocError::File`. 600 deterministic byte mutations never panic, in loading or in regeneration. A v1–v3 file carrying parameters or expressions is refused. Malformed `version` headers are refused. |
| `crates/rcad-commands/tests/it/print_goldens.rs` | The committed, externally validated `examples/print/*` (3MF, STL, STEP) are read back. Mesh files must be closed and manifold, rest at z = 0, and have volumes inside the analytic chord-bound interval. STEP files must re-import as one closed solid with the exact volume. |
| `crates/rcad-kernel/tests/it/step_curved_roundtrip.rs` | STEP export then re-import of curved bodies (rounded rectangle, slot, D-shape, 270° sector, chamfered block) against analytic volume, bounds and a watertight mesh. `revolve_washer_roundtrip`: the half-turn washer, written with `SURFACE_OF_REVOLUTION` faces, reads back with the Pappus volume and bounds; the full-turn washer (a face reaching the seam angle) must either be refused with an error naming `SURFACE_OF_REVOLUTION` or read back with the exact volume. |
| `crates/rcad-exchange/tests/it/stl_fixtures.rs` (two sweeps) | Every prefix and byte flip of the binary and ASCII STL fixtures; binary prefixes must be `Truncated`. |
| `crates/rcad-exchange/tests/it/dxf_fixtures.rs` (two sweeps) | Every prefix and byte substitution of three real DXF fixtures, with partial import both on and off. Any accepted result must be finite. |

The STEP round-trip uses our own writer and reader, so it guards against regressions but
is not independent interoperability evidence. FreeCAD remains the independent check
(`tools/external_validation.sh`).
