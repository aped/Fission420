# Rename to R-CAD and history rewrite (2026-10-13)

The project was called **Fission420** (crates `f420-*`, files `*.f420.json`) and is now
**R-CAD**. The **R** is for **Rust**, and it is said like "our": *our CAD*. This page
records what changed and how to bring work from the old history over.

## What changed

| | before | after | compatibility |
|---|---|---|---|
| product / repo | Fission420, `aped/Fission420` | R-CAD, `aped/R-CAD` | GitHub redirects the old repo URL |
| crates | `f420-app`, `f420-cli`, `f420-{commands,document,exchange,kernel,sketch,viewport}` | `rcad` (app; binary `rcad`), `rcad-cli`, `rcad-*` | none needed (workspace-internal, unpublished) |
| native files | `*.f420.json`, `"format": "fission420"` | `*.rcad.json`, `"format": "r-cad"` | old files open unchanged (`LEGACY_FORMAT_NAMES` in `crates/rcad-document/src/io.rs`). The extension was never checked. The format **version is unchanged**; the rename touches only the name. A build from before the rename refuses new files with "not a fission420 document" |
| autosave | `F420_RECOVERY_DIR`, `F420_AUTOSAVE_SECS`, `…/fission420/recovery`, `*.autosave.f420.json` | `RCAD_*`, `…/r-cad/recovery`, `*.autosave.rcad.json` | the old variables are read when the new ones are unset; the old directory and suffix are still searched at start-up (`crates/rcad/src/recovery.rs`) |
| other variables | `F420_TRACE_ENGINE`, `F420_PROBE`, `F420_WRITE_EXAMPLES`, … | `RCAD_*` | none (developer and test diagnostics) |
| internal material | run records committed under `runs/`; `AGENTS.md`, `CLAUDE.md` and per-run scripts tracked | everything internal in the git-ignored `.meta/` (`agents/`, `runs/`, `tools/`, `logs/`, `notes/`); Claude Code loads the agent instructions through an ignored `CLAUDE.local.md` (`@.meta/agents/AGENTS.md`) | `runs/` removed from the entire history; `cargo xtask check` refuses `runs/` or `AGENTS.md` in the root. `AGENTS.md`, `CLAUDE.md` and the run-specific scripts were then removed from the whole history too (second rewrite, 2026-10-13) |
| history | 261 commits, placeholder author `Your GitHub Username` | 247 commits, author and committer `Andrew <542314+aped@users.noreply.github.com>` | commits that only touched `runs/` disappeared; home paths and the five affected screenshots were redacted in every commit |

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
- `patch` applies it to a patch made on the old history.
- `check` lists any `f420`/`Fission420` left outside the allowed compatibility code.

## Porting work from the old history (e.g. `work/run-5`)

The rewritten history shares no commits with the old one, so `git merge` and
`git rebase` do not work across them. Port the work as a patch instead.

The common base is old `f0bcf6c` ("Live tray authoring (part 1)…"). In the new history it
is the commit with the same subject.

1. Reach a checkpoint in the old checkout: everything committed, `cargo xtask check` green.
2. Make one tree diff of the work, without run records. A tree diff keeps any conflict
   resolutions made in merge commits; a per-commit `format-patch` would drop them.
   ```bash
   cd <old checkout>
   git diff --binary f0bcf6c work/run-5 -- . ':!runs' > /path/run5.patch
   ```
3. Translate it to the new names and apply it on a branch of the new `main`:
   ```bash
   cd <R-CAD checkout>
   git switch -c port/run-5 origin/main
   python3 tools/rename_f420.py patch < /path/run5.patch > /path/run5-rcad.patch
   git apply --index --reject /path/run5-rcad.patch   # .rej files mark the hunks to merge by hand
   ```
4. Resolve the hunks listed below, then run `python3 tools/rename_f420.py check`,
   `cargo xtask check`, commit, and push the branch for review.

### Expected conflicts (old `f0bcf6c..work/run-5` against the new `main`)

As of 2026-10-13: 36 run-5 commits plus uncommitted edits to `f420-commands/src/lib.rs`,
`f420-document/src/{io,model,regen}.rs`, touching 99 files. The new `main` changed 29 of
the old files since `f0bcf6c`, then renamed everything.

| file (new name) | both sides changed | resolution |
|---|---|---|
| `crates/rcad-commands/tests/fixtures/native_v4/{enclosure,clearance_coupon,mounting_bracket}.rcad.json` | both added the same frozen v4 fixtures | byte-identical on both sides; take either. Run-5 also adds `plate` and `scenario_r` |
| `crates/rcad-commands/tests/native_formats.rs` | run-5: `check()` expects version 4 ("written as v4 unless a v5 feature is used"); main: new v4 tests (`v4_run4_printable_documents`, `v4_parameters_drive_a_loaded_document`, `committed_printable_examples`) | keep both. The new tests go through `check()`, so they follow run-5's version rule |
| `crates/rcad-document/src/io.rs` | run-5: `FORMAT_VERSION = 5`, `v5_feature`, `required_version` (feature suppression and pattern/mirror face references); main: `"r-cad"` name plus `LEGACY_FORMAT_NAMES` in the header check | keep both. The rename deliberately left the version at 4 so that run-5's v5 keeps its meaning. Check that `parse` still runs the legacy-name check before the version checks |
| `tools/print/validate_examples.sh` | run-5: tray slices; main: `CARGO_TARGET_DIR` default from `git rev-parse --git-common-dir` instead of the home path | keep both |
| `crates/rcad/src/recovery.rs` | run-5: `dropped` field (review run 4 #19); main: `legacy_dir`, `recovery_dirs`, `autosave_interval`, legacy suffix | different hunks; keep both |
| `xtask/src/main.rs` | run-5: new stages; main: the layout stage checks for `.meta/runs/` and refuses root `runs`/`AGENTS.md` | keep both |
| `docs/COMMANDS.md`, `docs/FORMAT_SUPPORT.md`, `docs/UI.md` | run-5: v5 and suppression docs; main: format-name and recovery-compatibility notes | keep both |

Port-time moves (these are run-only, so keep them out of the tracked tree):

- `tools/run5_final_verify.sh` (and any other `tools/runN_*`) goes to `.meta/tools/`.
- Changes run-5 made to `AGENTS.md` (if any) go into `.meta/agents/AGENTS.md`.

After applying, fix these leftovers. `rename_f420.py check` does not catch them, because
they are not old names:

- **`runs/` references** in comments, which now dangle. Known in run-5 code:
  `crates/rcad-commands/tests/review_run4.rs:1`, plus any added since. Find them with
  `git grep -n "runs/" -- crates tools xtask docs`.
- **Hard-coded home paths** in run-5's copies of `tools/heavy.sh`,
  `tools/live/g_enclosure_params_export.txt` and `tools/print/validate_examples.sh`.
  Main already replaced them; take main's side.
- **New examples** (`organizer_tray*.f420.json` and their workflows) arrive as
  `*.rcad.json` with `"format": "r-cad"`. Check them once with `cargo xtask check`.

### Notes for the run-5 agent

- Keep working in the old checkout until the next checkpoint; nothing there was changed.
  Port at a checkpoint, not mid-feature.
- Run-5's `bbd19c9` (save cache volume rounded to 1e-6 mm³) fixes the intermittent macOS
  failure of `bracket_exports_and_saves_are_byte_identical` (docs/CI.md L7). Porting it
  is what lets macOS gate.
- In the R-CAD checkout, internal material lives in `.meta/` (git-ignored). Until the
  swap-over below, run-5's records stay where they are, in Fission420's `runs/`.

## Swap-over (once the run-5 agent has finished)

Do it in one sitting, in this order, so nothing is written to both places:

1. **Stop:** the run-5 agent reports done (final verification, report written, tree clean
   and committed on `work/run-5`), and no other session is building in Fission420.
2. **Port:** port `work/run-5` to a `port/run-5` branch in R-CAD as above, run
   `cargo xtask check`, and review.
3. **Move internal records:**
   ```bash
   rsync -a <Fission420>/runs/ <R-CAD>/.meta/runs/
   ```
   Then copy Fission420's `artifacts/` subtrees worth keeping into `.meta/logs/`.
4. **Switch:** from here on, start agents in the R-CAD checkout. Archive the Fission420
   checkout read-only; don't delete it until the port is merged and pushed.
