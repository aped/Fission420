# Continuous integration

`.github/workflows/ci.yml` runs the local gate, `cargo xtask check`, on four platforms and
publishes release binaries. Each platform is one call of the reusable workflow
`.github/workflows/check.yml`. Linux (Bazzite/Fedora atomic) stays the primary development
environment: nothing in the workflow changes how the project is built or tested locally.

## Matrix and triggers

| leg | runner | runs on | gating | artifact |
|---|---|---|---|---|
| ubuntu-24.04 | `ubuntu-24.04` | every push to `main`, `work/**`, `ci/**`; every PR | yes | `r-cad-linux-x86_64.tar.gz` (glibc 2.39 baseline, so it also runs on Debian 13 and current Fedora) |
| debian-13 | `ubuntu-24.04` + container `debian:trixie` | same | yes | none (it would need a newer glibc than the Ubuntu build; this leg only gates) |
| windows-x64 | `windows-2025` (MSVC) | `main`, `v*` tags, PRs into `main`, manual runs | **not yet** | `r-cad-windows-x86_64.zip` |
| macos-arm64 | `macos-15` (Apple silicon) | same | **not yet** | `r-cad-macos-arm64.tar.gz` |

**Cost controls.** These are kept although the repository is public, where standard runners
are free; they also keep a private fork or a future switch to private affordable.

- **Routine pushes run Linux only.** Windows and macOS, and the binary archives, come from
  `main`, tags, PRs into `main` and manual runs (*Actions → CI → Run workflow*, which runs all
  four legs and publishes).
- **Docs-only changes do not trigger CI:** `**.md`, `docs/**` and `LICENSE-*`
  are ignored. Path filters do not apply to tag pushes.
- **Superseded runs are cancelled** on work branches and PRs. Runs on `main` and tags always
  finish.
- **Retention:** binary archives and failure logs are kept for 7 days.

Each leg runs these steps:

1. `cargo fetch --locked`: the lockfile must be current.
2. `cargo build --release --locked -p rcad -p rcad-cli`.
3. `cargo xtask check`: layout, `fmt --check`, `clippy -D warnings`, `test --workspace --release`, headless acceptance, the example replays.
4. When publishing: package and upload the binaries plus `README.md`, `LICENSE-MIT`, `LICENSE-APACHE`, `THIRD_PARTY_LICENSES.txt`, the committed `examples/` and `BUILD_INFO.txt`. `BUILD_INFO.txt` records the commit, platform, toolchain and the outcome of the xtask check.

The Ubuntu leg also warns (it does not fail) when the replays rewrite committed `examples/`.

A **non-gating** leg runs everything and shows red when it fails, but it does not fail the
workflow (`continue-on-error`). Its artifact is uploaded whenever the release build
succeeded, and `BUILD_INFO.txt` states whether its tests passed. Gating legs upload only
after a passing check.

On a `v*` tag, the `release` job collects the artifacts into a **draft** GitHub release.
Builds are unsigned and there are no installers, by design.

To make a leg gating, set `gate: true` for it in `ci.yml` once the platform items below
that block it are fixed and one run is green.

### Toolchain

The toolchain is pinned to **1.99.0** (`RUST_TOOLCHAIN` in `check.yml`), which matches the
local toolchain (Homebrew `cargo`/`rustc` 1.99.0 is first on `PATH` on the dev machine). This
keeps clippy's `-D warnings` the same in CI and locally.

- **Effective MSRV is 1.95.** egui/eframe/egui_kittest 0.36 require it. Edition 2024 with let-chains alone would need 1.88.
- **Bumping:** change `RUST_TOOLCHAIN` in the same commit as any clippy fixes the new version needs.
- **No `rust-toolchain.toml`.** The Homebrew `cargo` ignores it, so it would only make rustup users diverge from the primary environment.

### Native build dependencies

The only C code compiled is `lzma-sys`, pulled in by `truck-shapeops → truck-meshalgo
(default features) → vtkio → xz2`. It is third-party and not an application-owned kernel. It
needs a C compiler:

- **Debian container:** `build-essential pkg-config liblzma-dev`. With these it links the system liblzma.

Test-only runtime dependency: the sketch-text tests (`rcad-sketch/tests/text.rs`,
`rcad-commands/tests/{sketch_text,text_emboss,rounded_enclosure}.rs`, `rcad/tests/sketch_text_ui.rs`)
need the **Liberation Sans** font and fail, with that message, when it is missing
(`RCAD_SKIP_FONT_TESTS=1` skips them explicitly). The Ubuntu image has it; the Debian container
installs `fonts-liberation` (2.1.5); macOS and Windows fetch the upstream 2.1.5 TTF release,
pinned by SHA-256, and point `RCAD_FONT_DIRS` at it. All are version 2.1.5, but the distro
packages are rebuilt from source and differ in bytes from upstream (and from each other); the
tests compare outlines with tolerances, so this does not matter.
- **Ubuntu, macOS and Windows:** the runner images already have what it needs. MSVC builds the bundled source.

Nothing else links natively at build time. Wayland, X11, xkbcommon, EGL and Vulkan are
loaded with `dlopen` at runtime. Fonts are embedded. The GUI needs those runtime libraries
only to *run*; tests never open a window.

### Tests on headless runners

All `rcad` tests use `egui_kittest` without wgpu and run on the app's fallback camera,
so they are pure CPU: no display, no GPU, no `WAYLAND_DISPLAY`/`DISPLAY`. The one GPU test,
`rcad-viewport/src/gpu_tests.rs`, is `#[ignore]` and skips when no adapter is present. Never
set `RCAD_WRITE_EXAMPLES` in CI, because it overwrites `examples/`.

### Licences in the archives

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

- **Windows/macOS type-check from Linux:**

  ```bash
  rustup toolchain install 1.99.0 --profile minimal -t aarch64-apple-darwin -t x86_64-pc-windows-msvc
  RUSTC=$(rustup which --toolchain 1.99.0 rustc) PKG_CONFIG_ALLOW_CROSS=1 \
    CARGO_TARGET_DIR=$PWD/target-ci-x \
    $(rustup which --toolchain 1.99.0 cargo) check --target aarch64-apple-darwin --workspace --all-targets
  ```

  `RUSTC` must be set because the Homebrew `rustc` comes first on `PATH` and has no
  cross targets. `PKG_CONFIG_ALLOW_CROSS` lets the `lzma-sys` build script resolve the host
  liblzma; that is enough for `check`, which never links.

  This works for macOS. The MSVC target still fails in `lzma-sys`, because cc-rs needs
  `lib.exe`/`cl.exe`, so on Windows only the real runner verifies the build.

- **Workflow lint:** `actionlint` (v1.7.12 was used; it is not installed system-wide).

## Platform assumptions: refactor plan (not yet applied)

These come from a read-only audit of the tree at `f0bcf6c`. No source code was changed
for them; three cheap build-config mitigations are applied and marked **applied**.
Severity: **T** = a test fails on that platform; **R** = runtime risk, no test failure
expected; **C** = cosmetic.

| id | sev | platforms | where | what breaks | planned fix |
|---|---|---|---|---|---|
| F1 | T | Windows | `crates/rcad-commands/src/scenarios.rs:245,251,297,303,419,425,441,447`; compared in `tests/scenario_examples.rs:22-29` | Workflow paths are built with `Path::join` and serialised as `artifacts\scenario_a.stl`, which never equals the committed `artifacts/scenario_a.stl`. This fails `scenario_{a,b,r}_example_is_current_and_replays`. | **applied** (`0538511`): a `portable(out, name)` helper joins the eight paths with `/`; committed workflows unchanged. Local `cargo xtask check` passes; not yet confirmed on a Windows runner (no CI run since). |
| F2 | T | Windows | `examples/imported_step_hole.rcad.json` embeds `crates/rcad-kernel/tests/fixtures/step/box_40x30x10_mm.step` byte-for-byte (length + FNV-64). `tests/imports.rs:505-516` compares the two. | With `core.autocrlf=true` (the runner default) the STEP fixture is checked out with CRLF, so the blob length and hash differ. | **applied:** a root `.gitattributes` (`* text=auto eol=lf`, mesh and archive files binary). The index was already all LF, so `git add --renormalize .` changed nothing. |
| F3 | T? | macOS, Windows | Committed workflows hold solved floats compared exactly: `examples/scenario_b_workflow.json:257` `18.027756377319946` (`hypot`, `scenarios.rs:121`) and `scenario_r_workflow.json` near-integers. The solver calls libm `hypot`/`atan2` (`rcad-sketch/src/solver.rs`). | glibc, Apple libm and the UCRT do not promise last-bit-identical `hypot`/`atan2`. This would show up in the same `scenario_examples.rs` tests and, less likely, in `printable_examples.rs::committed_printable_workflows_are_current`. **Checked on the first Windows run (and macOS): did not occur**; with `\\` normalised to `/` the regenerated and committed workflows were identical. | (No change needed while it stays absent.) Compare workflow JSON numbers with a relative tolerance (about 1e-9), or round solved values before serialising. |
| L1 | T? | macOS, Windows | `crates/rcad-commands/tests/gate_fuzz.rs:230,365,573`; `crates/rcad-kernel/src/ops/tests.rs:33` | Accept and refuse counts were tuned on one Linux run. Truck's circle/NURBS code uses libm `sin`/`cos`, so a borderline case can flip. | Watch the first runs. If one diverges, give the thresholds a documented margin. Never drop the case. |
| L2 | R | Windows | `crates/rcad-cli/src/main.rs:117`; `rcad` `--sync` mode `src/main.rs:382` | Geometry runs on the main thread, which gets 1 MiB on Windows (8 MiB on Linux). Deep Truck recursion or STEP nesting (limit 64) could overflow it. Tests are not affected: test threads get 2 MiB on every OS. | **applied (mitigation):** `.cargo/config.toml` links MSVC binaries with `/STACK:8388608`. Long term: run the CLI body on a spawned thread with an explicit stack size. |
| L3 | R | all | `crates/rcad-document/src/io.rs:86-127`; `xtask/src/main.rs` | The example replays write to `artifacts/` but never create it. Only the acceptance stage, which runs first, creates it. A replay run alone on a fresh checkout fails. | `create_dir_all(parent)` in the export commands, or document it. CI is unaffected because it runs the whole xtask. |
| L4 | R | Windows, macOS | `crates/rcad/src/recovery.rs:42-51` | The autosave directory is `RCAD_RECOVERY_DIR`, then `XDG_STATE_HOME`, then `HOME/.local/state`. Windows usually has no `HOME`, so autosave is silently off there. On macOS it lands in a Linux-style path. | Fall back to `%LOCALAPPDATA%` on Windows and `~/Library/Application Support` on macOS, with no new dependency. |
| L5 | R | Windows | `crates/rcad-document/src/io.rs:111-116` | Saving copies the target's permissions onto the temp file before the rename. On Windows a read-only target, or a target another process holds open, makes the rename fail. The previous file survives, so it is safe, but the save fails. | Do not copy the read-only bit on Windows; retry the rename briefly on `PermissionDenied`. Add a test for saving over a read-only file. |
| L6 | T? | slow runners | `crates/rcad/tests/exchange_ui.rs:375` (20 s); `tests/measure_imports.rs:28` (30 s); `tests/frame_pacing.rs:94-104` | Wall-clock limits tuned on a 9800X3D. A shared CI runner may be several times slower. | Raise them to about 120 s. They guard against hangs, not performance. |
| L7 | **T (confirmed, intermittent)** | macOS | `crates/rcad-commands/tests/export_determinism.rs:114` (`bracket_exports_and_saves_are_byte_identical`); `crates/rcad-document/src/io.rs:48-52` (cache rounding) | Seen in GitHub Actions run ID 38346875644 after two green macOS runs. The two saves of the mounting bracket differ at byte 13523 of 13701, inside `"cache": {"volume": 20245.180555535}`. The bracket's volume is a `TessellationEstimate`. The likely cause, inferred and not yet reproduced locally, is that its floating-point sum follows Truck's pointer-derived face order. Rounding the cache to 1e-9 does not absorb a summation-order difference of that size, and macOS's randomised allocation changes the order between sessions where glibc's allocator happened not to. This is a real save-determinism defect, not a test bug. | Sum per-face contributions in a canonical face order (the same canonical ordering `step.rs` uses), or round estimate-method volumes in the cache to their error bound. Then run the test in a loop on macOS. Do not loosen the byte-identity assertion. **applied:** the saved cache volume is rounded to 1e-6 mm³ (`crates/rcad-document/src/io.rs`, `CACHE_VOLUME_STEPS`); not yet re-run on macOS. |
| F4 | T | Debian container, macOS, Windows | `crates/rcad-sketch/src/font.rs:52-71` (`font_dirs`); the 17 tests listed under "Native build dependencies" | Seen in GitHub Actions run ID 38437546623: the Debian container has no fonts, so 17 sketch-text tests failed with "font family 'Liberation Sans' is not installed". macOS and Windows do not ship Liberation Sans either, and `font_dirs` searches only Linux directories. | **applied (CI):** `fonts-liberation` in the Debian container (and `tools/ci/container_check.sh`); a pinned upstream 2.1.5 download plus `RCAD_FONT_DIRS` on macOS and Windows. Local `tools/ci/container_check.sh debian:trixie` with the package: pass (131 test binaries, 0 failed; acceptance 855/855; examples unchanged). The macOS/Windows step was only simulated on Linux (download, checksum, extract). **Open (R):** the app finds no installed fonts on macOS or Windows; add `/Library/Fonts`, `/System/Library/Fonts`, `~/Library/Fonts` and `%WINDIR%\Fonts`, `%LOCALAPPDATA%\Microsoft\Windows\Fonts` to `font_dirs`. |
| C1 | C | Windows | `crates/rcad/src/main.rs:1` | The GUI binary opens a console window. | `#![cfg_attr(windows, windows_subsystem = "windows")]`, once a log location exists. |
| C2 | C | non-Linux | `crates/rcad/Cargo.toml`, `crates/rcad-viewport/Cargo.toml` | eframe's `wayland`/`x11` features do nothing there. They build fine. | None. |
| C3 | C | all | `tools/heavy.sh:7` (hard-coded `ROOT`); `tools/*freecad*`, `tools/print/slice_bambu.sh` (flatpak) | Local-only tooling. Neither tests nor CI use it. | None for CI. |

Checked and fine (audit):

- **Paths:** no absolute paths in Rust code or fixtures. No Windows-illegal, reserved or case-colliding file names.
- **Platform APIs:** no `cfg(unix)` code, `std::os::unix` use, symlinks or file locking.
- **External tools:** no test spawns one (FreeCAD and Bambu Studio checks are separate scripts).
- **Line endings:** the importers (DXF, ASCII STL, SVG, STEP) accept CRLF.
- **Exact comparisons:** cross-document float comparisons use tolerances, apart from F3. The fixed hashes are on literal bytes only.

### Status (2026-10-13)

CI lives on `main` of https://github.com/aped/R-CAD (public). The first three GitHub runs
ran on an earlier branch (`ci/github-actions`, since deleted). Their results still describe
the same code:

| run | layout | ubuntu-24.04 | debian-13 | macos-arm64 | windows-x64 (non-gating) |
|---|---|---|---|---|---|
| 1 (push, cold caches) | four legs on every push | pass, 27 min | pass, 28 min | pass, 17 min | fail: F1 only, 31 min |
| 2 (manual, warm) | current layout | pass, 9 min | pass, 9 min | pass, 11 min | fail: F1 only, 22 min |
| 3 (manual, warm) | current layout | pass | pass | **fail: L7** (`bracket_exports_and_saves_are_byte_identical`) | fail: F1 only |

CI changes after these runs (`dfe8dbe`, not yet exercised by a GitHub run):

- **Cache on failure:** `Swatinem/rust-cache@v2` saves only after a successful job, so the
  failing Windows leg never had a cache ("No cache found"; the cache list held debian, macOS
  and ubuntu entries only; "Post Run Swatinem/rust-cache" skipped) and built cold every time
  (Actions run ID 38362474951: job 31m26; release binaries 7m16, clippy 251.9 s, test stage 1109 s
  including the cold release test build; `arc_fuzz` 335 s against 178 s on ubuntu). `cache-on-failure: true` keeps it warm.
- **No fail-fast:** CI has no nextest, so xtask falls back to `cargo test`, which stopped at the
  first failing test binary: in Actions run ID 38362474951 (main `c4627c3`) the Windows log has 54
  `test result:` lines against 96 on ubuntu. The fallback now passes `--no-fail-fast` (nextest
  already did), so the test binaries that have never run on Windows will report on the next run.

Findings:

- **F1 confirmed on Windows; nothing else failed in the binaries that ran** (the test stage stopped at the first failing binary; see the no-fail-fast change above). Once `\\` is normalised to `/`,
  the three committed workflows compare equal, so the solver's floats are bit-identical on
  the UCRT. The timing-sensitive UI tests (L6) passed. xtask stops at the first failing
  stage, so acceptance and the example replays (and with them L2) have not run on Windows
  yet.
- **F3 and L1 did not occur on macOS or Windows.**
- **L7 confirmed on macOS, intermittently.** It passed in the first two runs and failed in the
  third. Fixed since (cache volume rounded to 1e-6 mm³, see L7 above).
- **Archives** (third run): Linux 13 MB, macOS 10 MB, Windows 11 MB, each with binaries,
  `LICENSE-*`, `THIRD_PARTY_LICENSES.txt`, the examples, `artifacts/` and `BUILD_INFO.txt`.
  The Linux archive was downloaded and checked: its CLI runs `examples/plate_workflow.json`.
  The Windows `.exe` files are PE images, and that archive's `BUILD_INFO.txt` reports the
  failed check.
- **Two workflow bugs found and fixed:** the Windows upload was skipped after a failed check
  (implicit `success()`; fixed with `!cancelled()`); archives had no `artifacts/`, so the
  CLI example exited 1 (L3; archives now ship one).

Local evidence from base `f0bcf6c` plus the CI change (logs in `artifacts/ci/` of the old
checkout, untracked):

| check | result |
|---|---|
| `cargo xtask check` on Bazzite, cargo 1.99.0 | PASS: 576 passed, 0 failed, 13 ignored; 5m44s; replays left `examples/` unchanged |
| `tools/ci/container_check.sh debian:trixie` | PASS: Debian 13, rustc 1.99.0, 576 / 0 / 13; 9m23s including the toolchain install |
| `cargo check --workspace --all-targets --target aarch64-apple-darwin` | PASS (type-check only) |
| the same for `x86_64-pc-windows-msvc` | not verifiable from Linux: `lzma-sys` needs MSVC `lib.exe` |
| `actionlint` 1.7.12 on `ci.yml` and `check.yml` | clean (shellcheck integration not run) |

Next, in value order:

1. ~~Fix L7~~ (applied, cache volume rounded to 1e-6 mm³); confirm macOS green.
2. ~~Fix F1~~ (applied, `0538511`); confirm Windows green, then make Windows and macOS gating.
3. Fix L4, L5 and L6.

## Naming and identity: done (2026-10-13)

The project was renamed from Fission420 to R-CAD. See docs/RENAME.md for the compatibility
rules for legacy names and files.

## Regression tests added with CI

These are headless, need no external tools, and use analytic expected values, not snapshots of our own output.

| file | covers |
|---|---|
| `crates/rcad-commands/tests/native_formats.rs` (`v4_printable_documents`, `v4_parameters_drive_a_loaded_document`, `committed_printable_examples`) | Frozen v4 documents with parameters and expressions (`tests/fixtures/native_v4/`) open, regenerate to analytic volumes, re-save identically, and still respond to parameter edits. The enclosure, coupon and mounting-bracket examples are now opened by a test, not only rewritten by the replays. |
| `crates/rcad-document/tests/native_fuzz.rs` | Every truncation of a v3 and a v4 file is a `DocError::File`. 600 deterministic byte mutations never panic, in loading or in regeneration. A v1–v3 file carrying parameters or expressions is refused. Malformed `version` headers are refused. |
| `crates/rcad-commands/tests/print_goldens.rs` | The committed, externally validated `examples/print/*` (3MF, STL, STEP) are read back. Mesh files must be closed and manifold, rest at z = 0, and have volumes inside the analytic chord-bound interval. STEP files must re-import as one closed solid with the exact volume. |
| `crates/rcad-kernel/tests/step_curved_roundtrip.rs` | STEP export then re-import of curved bodies (rounded rectangle, slot, D-shape, 270° sector, chamfered block) against analytic volume, bounds and a watertight mesh. `revolve_washer_roundtrip`: the half-turn washer, written with `SURFACE_OF_REVOLUTION` faces, reads back with the Pappus volume and bounds; the full-turn washer (a face reaching the seam angle) must either be refused with an error naming `SURFACE_OF_REVOLUTION` or read back with the exact volume. |
| `crates/rcad-exchange/tests/stl_fixtures.rs` (two sweeps) | Every prefix and byte flip of the binary and ASCII STL fixtures; binary prefixes must be `Truncated`. |
| `crates/rcad-exchange/tests/dxf_fixtures.rs` (two sweeps) | Every prefix and byte substitution of three real DXF fixtures, with partial import both on and off. Any accepted result must be finite. |

| `crates/rcad-document/tests/legacy_name.rs`, `crates/rcad/tests/recovery_legacy.rs` | Pre-rename compatibility: `fission420` / `*.f420.json` documents open and re-save as `r-cad`; other format names are refused; old autosave variables, directory and suffix still work (docs/RENAME.md). |

The STEP round-trip uses our own writer and reader, so it guards against regressions but
is not independent interoperability evidence. FreeCAD remains the independent check
(`tools/external_validation.sh`).
