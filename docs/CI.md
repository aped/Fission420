# Continuous integration

`.github/workflows/ci.yml` runs the local gate, `cargo xtask check`, on four platforms and
publishes release binaries. Each platform is one call of the reusable workflow
`.github/workflows/check.yml`. Linux (Bazzite/Fedora atomic) stays the primary development
environment: nothing in the workflow changes how the project is built or tested locally.

## Matrix and triggers

| leg | runner | runs on | gating | artifact |
|---|---|---|---|---|
| ubuntu-24.04 | `ubuntu-24.04` | every push to `main`, `work/**`, `ci/**`; every PR | yes | `fission420-linux-x86_64.tar.gz` (glibc 2.39 baseline, so it also runs on Debian 13 and current Fedora) |
| debian-13 | `ubuntu-24.04` + container `debian:trixie` | same | yes | none (it would need a newer glibc than the Ubuntu build; this leg only gates) |
| windows-x64 | `windows-2025` (MSVC) | `main`, `v*` tags, PRs into `main`, manual runs | **not yet** | `fission420-windows-x86_64.zip` |
| macos-arm64 | `macos-15` (Apple silicon) | same | **not yet** | `fission420-macos-arm64.tar.gz` |

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
2. `cargo build --release --locked -p f420-app -p f420-cli`.
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
- **Ubuntu, macOS and Windows:** the runner images already have what it needs. MSVC builds the bundled source.

Nothing else links natively at build time. Wayland, X11, xkbcommon, EGL and Vulkan are
loaded with `dlopen` at runtime. Fonts are embedded. The GUI needs those runtime libraries
only to *run*; tests never open a window.

### Tests on headless runners

All `f420-app` tests use `egui_kittest` without wgpu and run on the app's fallback camera,
so they are pure CPU: no display, no GPU, no `WAYLAND_DISPLAY`/`DISPLAY`. The one GPU test,
`f420-viewport/src/gpu_tests.rs`, is `#[ignore]` and skips when no adapter is present. Never
set `F420_WRITE_EXAMPLES` in CI, because it overwrites `examples/`.

### Licences in the archives

Fission420 is MIT OR Apache-2.0. The binaries also embed about 250 third-party crates, all
permissive or used under a permissive option (docs/DEPENDENCIES.md). Among them are Truck,
nalgebra and winit under Apache-2.0 only, and egui's bundled fonts under OFL-1.1, the Ubuntu
Font Licence and Hack's licence. Redistributing them requires their notices.
`tools/ci/third_party_licenses.py <target> <out>` writes `THIRD_PARTY_LICENSES.txt` for
each platform from `cargo metadata`. It lists every crate linked into `f420-app`/`f420-cli`
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
| F1 | T | Windows | `crates/f420-commands/src/scenarios.rs:245,251,297,303,419,425,441,447`; compared in `tests/scenario_examples.rs:22-29` | Workflow paths are built with `Path::join` and serialised as `artifacts\scenario_a.stl`, which never equals the committed `artifacts/scenario_a.stl`. This fails `scenario_{a,b,r}_example_is_current_and_replays`. | Build these workflow paths as `/`-joined strings (they are portable document data, not OS paths), or normalise separators in `scenarios::workflow_json`. |
| F2 | T | Windows | `examples/imported_step_hole.f420.json` embeds `crates/f420-kernel/tests/fixtures/step/box_40x30x10_mm.step` byte-for-byte (length + FNV-64). `tests/imports.rs:505-516` compares the two. | With `core.autocrlf=true` (the runner default) the STEP fixture is checked out with CRLF, so the blob length and hash differ. | **applied:** a root `.gitattributes` (`* text=auto eol=lf`, mesh and archive files binary). The index was already all LF, so `git add --renormalize .` changed nothing. |
| F3 | T? | macOS, Windows | Committed workflows hold solved floats compared exactly: `examples/scenario_b_workflow.json:257` `18.027756377319946` (`hypot`, `scenarios.rs:121`) and `scenario_r_workflow.json` near-integers. The solver calls libm `hypot`/`atan2` (`f420-sketch/src/solver.rs`). | glibc, Apple libm and the UCRT do not promise last-bit-identical `hypot`/`atan2`. This would show up in the same `scenario_examples.rs` tests and, less likely, in `printable_examples.rs::committed_printable_workflows_are_current`. Unconfirmed until a run. | Compare workflow JSON numbers with a relative tolerance (about 1e-9), or round solved values before serialising. |
| L1 | T? | macOS, Windows | `crates/f420-commands/tests/gate_fuzz.rs:230,365,573`; `crates/f420-kernel/src/ops/tests.rs:33` | Accept and refuse counts were tuned on one Linux run. Truck's circle/NURBS code uses libm `sin`/`cos`, so a borderline case can flip. | Watch the first runs. If one diverges, give the thresholds a documented margin. Never drop the case. |
| L2 | R | Windows | `crates/f420-cli/src/main.rs:117`; `f420-app` `--sync` mode `src/main.rs:382` | Geometry runs on the main thread, which gets 1 MiB on Windows (8 MiB on Linux). Deep Truck recursion or STEP nesting (limit 64) could overflow it. Tests are not affected: test threads get 2 MiB on every OS. | **applied (mitigation):** `.cargo/config.toml` links MSVC binaries with `/STACK:8388608`. Long term: run the CLI body on a spawned thread with an explicit stack size. |
| L3 | R | all | `crates/f420-document/src/io.rs:86-127`; `xtask/src/main.rs` | The example replays write to `artifacts/` but never create it. Only the acceptance stage, which runs first, creates it. A replay run alone on a fresh checkout fails. | `create_dir_all(parent)` in the export commands, or document it. CI is unaffected because it runs the whole xtask. |
| L4 | R | Windows, macOS | `crates/f420-app/src/recovery.rs:42-51` | The autosave directory is `F420_RECOVERY_DIR`, then `XDG_STATE_HOME`, then `HOME/.local/state`. Windows usually has no `HOME`, so autosave is silently off there. On macOS it lands in a Linux-style path. | Fall back to `%LOCALAPPDATA%` on Windows and `~/Library/Application Support` on macOS, with no new dependency. |
| L5 | R | Windows | `crates/f420-document/src/io.rs:111-116` | Saving copies the target's permissions onto the temp file before the rename. On Windows a read-only target, or a target another process holds open, makes the rename fail. The previous file survives, so it is safe, but the save fails. | Do not copy the read-only bit on Windows; retry the rename briefly on `PermissionDenied`. Add a test for saving over a read-only file. |
| L6 | T? | slow runners | `crates/f420-app/tests/exchange_ui.rs:375` (20 s); `tests/measure_imports.rs:28` (30 s); `tests/frame_pacing.rs:94-104` | Wall-clock limits tuned on a 9800X3D. A shared CI runner may be several times slower. | Raise them to about 120 s. They guard against hangs, not performance. |
| L7 | **T (confirmed, intermittent)** | macOS | `crates/f420-commands/tests/export_determinism.rs:114` (`bracket_exports_and_saves_are_byte_identical`); `crates/f420-document/src/io.rs:48-52` (cache rounding) | Seen in run 38346875644 after two green macOS runs. The two saves of the mounting bracket differ at byte 13523 of 13701, inside `"cache": {"volume": 20245.180555535}`. The bracket's volume is a `TessellationEstimate`. The likely cause, inferred and not yet reproduced locally, is that its floating-point sum follows Truck's pointer-derived face order. Rounding the cache to 1e-9 does not absorb a summation-order difference of that size, and macOS's randomised allocation changes the order between sessions where glibc's allocator happened not to. This is a real save-determinism defect, not a test bug. | Sum per-face contributions in a canonical face order (the same canonical ordering `step.rs` uses), or round estimate-method volumes in the cache to their error bound. Then run the test in a loop on macOS. Do not loosen the byte-identity assertion. |
| C1 | C | Windows | `crates/f420-app/src/main.rs:1` | The GUI binary opens a console window. | `#![cfg_attr(windows, windows_subsystem = "windows")]`, once a log location exists. |
| C2 | C | non-Linux | `crates/f420-app/Cargo.toml`, `crates/f420-viewport/Cargo.toml` | eframe's `wayland`/`x11` features do nothing there. They build fine. | None. |
| C3 | C | all | `tools/heavy.sh:7` (hard-coded `ROOT`); `tools/*freecad*`, `tools/print/slice_bambu.sh` (flatpak) | Local-only tooling. Neither tests nor CI use it. | None for CI. |

Checked and fine (audit):

- **Paths:** no absolute paths in Rust code or fixtures. No Windows-illegal, reserved or case-colliding file names.
- **Platform APIs:** no `cfg(unix)` code, `std::os::unix` use, symlinks or file locking.
- **External tools:** no test spawns one (FreeCAD and Bambu Studio checks are separate scripts).
- **Line endings:** the importers (DXF, ASCII STL, SVG, STEP) accept CRLF.
- **Exact comparisons:** cross-document float comparisons use tolerances, apart from F3. The fixed hashes are on literal bytes only.

### Status (2026-10-12)

Workflow and tests are on branch `ci/github-actions`, pushed to
https://github.com/aped/Fission420 (public since 2026-10-13).

**First GitHub run** ([38342360431](https://github.com/aped/Fission420/actions/runs/38342360431),
commit `8946b8d`, the original four-legs-on-every-push layout). Conclusion **success**:

| leg | result | duration |
|---|---|---|
| ubuntu-24.04 | pass: full `cargo xtask check` | 27 min |
| debian-13 | pass | 28 min |
| macos-arm64 | pass: every test, acceptance and every replay. **F3 and L1 did not occur** | 17 min |
| windows-x64 (non-gating) | **fail: exactly F1.** `scenario_{a,b,r}_example_is_current_and_replays`; all other tests passed, including the timing-sensitive UI tests (L6). Once `\\` is normalised to `/`, all three workflows compare equal, so the solver's floats are bit-identical on the UCRT as well: **F3 did not occur**. xtask stops at the first failing stage, so acceptance and the example replays (and with them L2) have not run on Windows yet | 31 min |

**Validation run of the current layout** ([38344936649](https://github.com/aped/Fission420/actions/runs/38344936649),
`workflow_dispatch`, warm caches). Conclusion success. Ubuntu and Debian took 9 min, macOS
11 min (all pass), Windows 22 min (F1 again).

- **Linux and macOS archives:** published. The Linux archive was downloaded and checked: it
  contains the binaries, the licence files, `THIRD_PARTY_LICENSES.txt` (260 crates), the
  examples and `BUILD_INFO.txt`, and runs `f420-cli run examples/plate_workflow.json`.
- **Windows archive:** not uploaded, because the upload step's implicit `success()` was
  false after the failed check. Fixed (`!cancelled()`).
- **L3 seen from the user's side:** an extracted archive without an `artifacts/` directory
  made the CLI exit 1 on that example. Archives now ship an empty `artifacts/`.

Next: F1 for Windows, and L7 for macOS. macOS is **not** ready to gate: L7 fails
intermittently there (see the verification run below).

Local evidence from base `f0bcf6c` plus this change (logs in `artifacts/ci/`, untracked):

| check | result |
|---|---|
| `cargo xtask check` on Bazzite, cargo 1.99.0 (`native_xtask.log`) | PASS: 576 passed, 0 failed, 13 ignored (summed `test result` lines); every stage ok; 5m44s; replays left `examples/` unchanged |
| `tools/ci/container_check.sh debian:trixie` (`debian13_xtask.log`) | PASS: Debian 13, rustc 1.99.0; release build plus `cargo xtask check`, 576 / 0 / 13; 9m23s including the toolchain install; `examples/` unchanged |
| `cargo check --workspace --all-targets --target aarch64-apple-darwin` (`crosscheck.log`) | PASS (type-check only; not linked or run) |
| the same for `x86_64-pc-windows-msvc` | not verifiable from Linux: `lzma-sys` needs MSVC `lib.exe` |
| `actionlint` 1.7.12 on `ci.yml` and `check.yml` | clean (shellcheck integration not run: shellcheck is not installed) |
| Ubuntu 24.04 leg | not run locally; covered by the Debian container run plus the runner image (`tools/ci/container_check.sh ubuntu:24.04` reproduces it) |

Next, in value order:

1. Add the remote, push, and read the Windows and macOS results.
2. Fix F1 and F3 (test and serialisation only, no geometry change), then promote Windows and macOS to gating.
3. Fix L4, L5 and L6.
4. Naming and identity cleanup (below): first the machine-independence and git-identity items, then the product rename once it is decided.

## Naming and identity cleanup: plan (not yet applied)

Counts are from `git grep -F` at `d49370c`. Nothing has been renamed yet. The product name
is your decision (see the open decisions at the end of this section). Everything else here
is mechanical once that is decided.

### Names in use

| name | where | hits | role |
|---|---|---|---|
| **RustCad** | `runs/run-{2,3,4,5}` specs only (`RustCad / Fission420 — Session N`, `RUSTCAD_SESSION_4.md`) | 8 in 4 files | working name in the specs; not in code or user-facing text |
| **Fission420** / **fission420** | README, docs, app title and window, licence holder, repo name, `FORMAT_NAME = "fission420"` (`crates/f420-document/src/io.rs:12`), recovery dir `fission420/recovery` (`crates/f420-app/src/recovery.rs`), archive names `fission420-<platform>` | 92 + 46 in 98 files | product name, plus the identifier written into every native file |
| **f420** / **F420** | 9 crate names (`f420-*`) and both binaries, the `.f420.json` extension, `F420_*` environment variables (`F420_RECOVERY_DIR`, `F420_AUTOSAVE_SECS`, `F420_WRITE_EXAMPLES`, …), code identifiers (`F420App`) | 1311 + 344 in 289 files | short prefix; part of the file format and the CLI |
| personal: `<user>` | `tools/heavy.sh:7` and `tools/print/validate_examples.sh:15,23` (hard-coded `ROOT` / target dir), `tools/live/g_enclosure_params_export.txt:6`, run reports (`cd <repo>/...`), and the status bar of `docs/screenshots/` 08, 18, 19, 20, 21 | 13 in 10 files + 5 images | the developer's machine paths leaked into tooling and published screenshots |
| git identity | **all 256 commits** authored `Your GitHub Username <your-github-email@example.com>` (placeholder global git config; `runs/STATUS.md:244`) | 256 commits | GitHub cannot attribute the history to an account |
| licence holder | `LICENSE-MIT`: "The Fission420 contributors" | 1 | placeholder until a name is chosen |

### Planned changes, in order

1. **Machine-independence. Done** (`ci/github-actions`, before the repository went public):
   `tools/heavy.sh` and `tools/print/validate_examples.sh` find the main checkout through
   `git rev-parse --git-common-dir`, so every worktree still shares one lock. The live script
   uses an `@ROOT@` placeholder. The five screenshots that showed a home path in the status
   bar (08, 18, 19, 20, 21) have it replaced by the repository-relative path. Older blobs
   still contain the home path until history is rewritten (step 2). The original plan was:
   - `tools/heavy.sh` and `tools/print/validate_examples.sh` derive `ROOT` from the script location (`$(cd "$(dirname "$0")/.." && pwd)`), as `tools/ci/container_check.sh` already does.
   - The live-walkthrough script types a path relative to the repository.
   - Recapture or crop the three screenshots so the status bar shows no home path.
   - Run reports under `runs/` are historical records: leave them as written, or redact the home path to `<repo>` (decision 3).
2. **Git identity.**
   - Set the real `user.name`/`user.email` for future commits.
   - For history, choose between a `.mailmap` (non-destructive; fixes `git log`/shortlog, but GitHub still shows the placeholder) and a rewrite with `git filter-repo --mailmap` plus a force-push. A rewrite is cheap now, while only `ci/github-actions` is pushed and the repo is private, and expensive later. Any rewrite needs coordinating with the run-5 session: rewrite its branch at a checkpoint, never under it.
3. **Product rename (only if decided).** The rename touches persisted data, so it is staged:
   - **Display name first:** README, docs, app title, window title, licence holder, archive names and the GitHub repo name. GitHub redirects the old URL after a repo rename.
   - **Native format stays readable.** The loader accepts both the old `"format": "fission420"` header and the new one, and keeps opening `.f420.json` files. The writer emits the new name only together with a format version bump, so older builds refuse new files with the existing "newer version" error rather than "not a document". Record the change in `docs/FORMAT_SUPPORT.md` and an ADR.
   - **Environment variables:** read the new prefix first and fall back to `F420_*` for one release. Note the fallback in `docs/COMMANDS.md`.
   - **Recovery directory:** look in the old directory too, so autosaves from before the rename are still found.
   - **Crate names last, or never.** The `f420-*` crates are workspace-internal and unpublished, so renaming them costs churn (around 1,300 hits, every import path, `Cargo.lock`) with no user benefit. Keeping `f420` as an internal codename is a legitimate outcome.

### Tests that guard the rename

| test | asserts |
|---|---|
| existing frozen native fixtures (`native_v1`…`native_v4`, `native_fuzz.rs`) | every pre-rename document still opens and regenerates to its analytic volume. These are the rename's main regression net and must stay unchanged |
| new: `legacy_format_name_is_accepted` (f420-document) | a file with the old header loads; a re-save writes the new header and the new version; the old reader path refuses the new file with the "newer version" error, not "not a document" |
| new: `wrong_format_name_is_refused` | an unrelated `format` value is still refused (the alias must not loosen the check) |
| new: env-var alias test (f420-app `recovery.rs`) | the new variable wins; the old one still works when the new one is unset |
| new: recovery migration test | an autosave in the old directory is offered for recovery |
| new xtask stage `naming` (alongside `layout`) | no tracked file outside `runs/` contains an absolute home path (`/var/home/`, `/home/<user>/`), and the retired product name appears only on an allow-list (the alias constant, migration code, changelog, historical `runs/`). The same mechanism as the run-file layout check |
| CI | all four legs; the Windows path-separator item (F1) touches the same workflow JSON, so do it first or in the same change |

### Open decisions

1. **Product name:** keep Fission420, adopt RustCad, or choose something else. Check crates.io, GitHub and trademark search for collisions before choosing any new name; that has not been checked.
2. **Git history:** rewrite with a force-push, or add a `.mailmap` only.
3. **Run records:** redact home paths in `runs/`, or keep them as historical evidence.
4. **Copyright line** for `LICENSE-MIT`.
5. **Crate prefix:** keep `f420` as an internal codename, or rename it with the product.

## Regression tests added with CI

These are headless, need no external tools, and use analytic expected values, not snapshots of our own output.

| file | covers |
|---|---|
| `crates/f420-commands/tests/native_formats.rs` (`v4_run4_printable_documents`, `v4_parameters_drive_a_loaded_document`, `committed_printable_examples`) | Frozen v4 documents with parameters and expressions (`tests/fixtures/native_v4/`) open, regenerate to analytic volumes, re-save identically, and still respond to parameter edits. The enclosure, coupon and mounting-bracket examples are now opened by a test, not only rewritten by the replays. |
| `crates/f420-document/tests/native_fuzz.rs` | Every truncation of a v3 and a v4 file is a `DocError::File`. 600 deterministic byte mutations never panic, in loading or in regeneration. A v1–v3 file carrying parameters or expressions is refused. Malformed `version` headers are refused. |
| `crates/f420-commands/tests/print_goldens.rs` | The committed, externally validated `examples/print/*` (3MF, STL, STEP) are read back. Mesh files must be closed and manifold, rest at z = 0, and have volumes inside the analytic chord-bound interval. STEP files must re-import as one closed solid with the exact volume. |
| `crates/f420-kernel/tests/step_curved_roundtrip.rs` | STEP export then re-import of curved bodies (rounded rectangle, slot, D-shape, 270° sector, chamfered block) against analytic volume, bounds and a watertight mesh. The revolved washer's `SURFACE_OF_REVOLUTION` is not supported by our importer; the test asserts that explicit refusal. |
| `crates/f420-exchange/tests/stl_fixtures.rs` (two sweeps) | Every prefix and byte flip of the binary and ASCII STL fixtures; binary prefixes must be `Truncated`. |
| `crates/f420-exchange/tests/dxf_fixtures.rs` (two sweeps) | Every prefix and byte substitution of three real DXF fixtures, with partial import both on and off. Any accepted result must be finite. |

The STEP round-trip uses our own writer and reader, so it guards against regressions but
is not independent interoperability evidence. FreeCAD remains the independent check
(`tools/external_validation.sh`).
