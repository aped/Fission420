# Installing and running R-CAD

Prebuilt packages are attached to each tagged GitHub release (`v1.0.0` and later), with
SHA-256 checksums (`SHA256SUMS`, plus a `.sha256` next to each file). They are built with the
`dist` profile (fat LTO, one codegen unit) after that platform's tests and example replays
passed. To build from source you need a stable Rust toolchain, 1.95 or newer (edition 2024;
CI pins 1.99.0). Primary target: Linux x86_64 on Wayland or X11 with a Vulkan/GL driver
(wgpu).

| platform | download | contents |
|---|---|---|
| Linux x86_64 | `r-cad-<version>-linux-x86_64.tar.gz` | `rcad`, `rcad-cli`, desktop entry, icon, `install.sh` (glibc 2.39 or newer) |
| Linux ARM64 | `r-cad-<version>-linux-aarch64.tar.gz` | the same, for 64-bit ARM |
| Linux (single file) | `r-cad-<version>-linux-x86_64.AppImage`, `…-linux-aarch64.AppImage` | the GUI as one executable file; no installation |
| macOS 11+ (Apple silicon and Intel) | `r-cad-<version>-macos-universal.dmg` | `R-CAD.app` (universal binary); the `.tar.gz` holds the bare binaries for terminal use |
| Windows 10/11 x64 | `r-cad-<version>-windows-x86_64.zip` | `rcad.exe` (GUI, with icon), `rcad-cli.exe` |

Every package also carries `README.md`, `CHANGELOG.md`, the licences,
`THIRD_PARTY_LICENSES.txt`, the example documents and workflows in `examples/`, an empty
`artifacts/` for the workflows' exports, and `BUILD_INFO.txt` (version, commit, platform,
toolchain, the CI run). The same packages, built with the ordinary release profile, are also
kept for 7 days as artifacts of CI runs on `main`.

The packages are not signed with a publisher certificate. Each platform's first-run prompt
and how to get past it is below.

### Verifying a download

Every package of a tagged release has a build provenance attestation, signed through
Sigstore by the release workflow: it ties the file's SHA-256 to this repository, the tag and
the CI run that built it. With the GitHub CLI:

```bash
gh attestation verify r-cad-<version>-linux-x86_64.tar.gz -R aped/R-CAD
```

The checksums alone (`sha256sum -c SHA256SUMS`) only show that the download is intact.

### Linux

```bash
sha256sum -c r-cad-<version>-linux-x86_64.tar.gz.sha256    # or: sha256sum -c SHA256SUMS
tar xzf r-cad-<version>-linux-x86_64.tar.gz
cd r-cad-<version>-linux-x86_64
./rcad --version
./install.sh                     # optional: ~/.local/bin + application menu entry
rcad                             # GUI (or ./rcad without installing)
rcad-cli run examples/plate_workflow.json     # writes into artifacts/
```

Or the AppImage: `chmod +x r-cad-<version>-linux-x86_64.AppImage` and run it. It needs FUSE
(present on most desktops; otherwise run it with `APPIMAGE_EXTRACT_AND_RUN=1`). Its launcher,
the AppImage runtime, contains LGPL-2.1 code (libfuse); the notice is in About R-CAD and in
`usr/share/doc/r-cad` inside the image.

`./install.sh --uninstall` removes it again; `PREFIX=/usr/local ./install.sh` (as root)
installs system-wide. Runtime libraries come from the system (Wayland or X11, xkbcommon,
Vulkan or GL); nothing else is needed.

### macOS

Open the `.dmg` and drag **R-CAD** to **Applications**. The app is ad-hoc signed but not
notarized, so the first launch is blocked: Control-click (or right-click) R-CAD in
Applications, choose **Open**, then **Open** again; or on macOS 15 allow it under **System
Settings → Privacy & Security**. From a terminal, `xattr -dr com.apple.quarantine
/Applications/R-CAD.app` does the same. The command-line tool is inside the bundle:
`/Applications/R-CAD.app/Contents/MacOS/rcad-cli`.

### Windows

Unzip anywhere and run `rcad.exe`. SmartScreen may show "Windows protected your PC" for an
unsigned download: choose **More info → Run anyway**. `rcad.exe` is a windowed program and
prints nothing to an interactive console; use `rcad-cli.exe` in a terminal
(`rcad-cli.exe --version`, `rcad-cli.exe run examples\plate_workflow.json`).

### Opening a document

`rcad <file.rcad.json>` opens that document at startup, so "Open with R-CAD" works from
Linux file managers and Windows Explorer.

## From a locally built archive

```bash
tar xzf r-cad-<version>-<rev>-linux-x86_64.tar.gz
cd r-cad-<version>-<rev>-linux-x86_64
sha256sum -c SHA256SUMS          # every file in the package
bin/rcad --version
bin/rcad                         # GUI
bin/rcad-cli run examples/plate_workflow.json   # headless command script (run from a copy)
```

The archive contains `bin/rcad` (GUI), `bin/rcad-cli` (headless scripts and the acceptance
suite), the user documentation in `docs/`, the native example documents and their replayable
workflows in `examples/`, `LICENSE-MIT`, `LICENSE-APACHE` and `THIRD_PARTY_LICENSES.txt`.
Runtime libraries are loaded from the system (Wayland/X11, xkbcommon, Vulkan or GL); no
other native library is linked. Archives are not code-signed.

## Building a release archive locally

```bash
tools/release.sh artifacts/release            # refuses a dirty tree; --allow-dirty for tests
```

It builds the `dist` profile (fat LTO) with `--locked` into `target-release/` (disk-backed), embeds the git revision in
`--version`, smoke-tests the staged binaries (version, a byte-identical example replay, the
drawing-set example) and writes `SHA256SUMS` plus `<name>.tar.gz.sha256`.

## Building for development

```bash
cargo build --release -p rcad -p rcad-cli
cargo xtask check          # layout, public text, fmt, clippy, tests, acceptance, example replays
target/release/rcad
```

Fonts for sketch text are discovered from the usual system font folders (Linux, macOS,
Windows) or `RCAD_FONT_DIRS`.
