# Installing and running R-CAD (Linux x86_64)

Prebuilt archives for Linux x86_64, Windows x64 and macOS ARM64 are attached to each tagged
GitHub release (`v1.0.0` and later), with SHA-256 checksums. To build from source you need a
stable Rust toolchain, 1.95 or newer (edition 2024; CI pins 1.99.0). Primary target: Linux
x86_64 on Wayland or X11 with a Vulkan/GL driver (wgpu).

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

It builds with `--locked` into `target-release/` (disk-backed), embeds the git revision in
`--version`, smoke-tests the staged binaries (version, a byte-identical example replay, the
drawing-set example) and writes `SHA256SUMS` plus `<name>.tar.gz.sha256`.

## Building for development

```bash
cargo build --release -p rcad -p rcad-cli
cargo xtask check          # fmt, clippy -D warnings, tests, acceptance, example replays
target/release/rcad
```

Fonts for sketch text are discovered from the usual system font folders (Linux, macOS,
Windows) or `RCAD_FONT_DIRS`.
