#!/usr/bin/env bash
# Platform packages for a release from already built binaries (CI's package job; also
# usable locally). Nothing is signed or uploaded.
#
# usage: tools/package.sh <platform> <bin-dir> <out-dir>
#   platform: linux-x86_64 | linux-aarch64 | macos-universal | windows-x86_64
#   bin-dir:  holds rcad and rcad-cli (rcad.exe, rcad-cli.exe on Windows) and, when
#             available, THIRD_PARTY_LICENSES.txt
# env: VERSION (default: the workspace version), BUILD_INFO (a file copied in as
#      BUILD_INFO.txt, optional)
#
# Every platform gets r-cad-<version>-<platform>.tar.gz (.zip on Windows) holding both
# binaries, the licences, README, CHANGELOG and the examples. On top of that:
#   linux-*          share/applications/r-cad.desktop, the icon and install.sh in the archive
#   macos-universal  R-CAD.app (ad-hoc signed) in r-cad-<version>-macos-universal.dmg;
#                    needs macOS (iconutil, sips, codesign, hdiutil)
#   windows-x86_64   rcad.exe carries its icon and version resource from the build
# Each package gets a <file>.sha256 next to it.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PLATFORM=${1:?platform}
BIN=$(cd "${2:?bin-dir}" && pwd)
mkdir -p "${3:?out-dir}"
OUT=$(cd "$3" && pwd)
VERSION=${VERSION:-$(sed -n '/^\[workspace.package\]/,/^\[/ s/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml")}
NAME="r-cad-$VERSION-$PLATFORM"
exe=""
case "$PLATFORM" in
  linux-x86_64|linux-aarch64|macos-universal) ;;
  windows-x86_64) exe=".exe" ;;
  *) echo "unknown platform $PLATFORM" >&2; exit 64 ;;
esac

sha256() { # file -> "<hash>  <file>" in the file's directory
  local d f
  d=$(dirname "$1"); f=$(basename "$1")
  if command -v sha256sum >/dev/null; then (cd "$d" && sha256sum "$f"); else (cd "$d" && shasum -a 256 "$f"); fi > "$1.sha256"
  cat "$1.sha256"
}

# The common directory: binaries, documents, examples.
DIR="$OUT/$NAME"
rm -rf -- "${DIR:?}"
mkdir -p "$DIR/examples" "$DIR/artifacts"
cp "$BIN/rcad$exe" "$BIN/rcad-cli$exe" "$DIR/"
[ -f "$BIN/THIRD_PARTY_LICENSES.txt" ] && cp "$BIN/THIRD_PARTY_LICENSES.txt" "$DIR/"
cp "$ROOT/README.md" "$ROOT/CHANGELOG.md" "$ROOT/LICENSE-MIT" "$ROOT/LICENSE-APACHE" "$DIR/"
cp -r "$ROOT/examples/." "$DIR/examples/"
# The example workflows export into artifacts/, which the CLI does not create.
touch "$DIR/artifacts/.keep"
[ -n "${BUILD_INFO:-}" ] && cp "$BUILD_INFO" "$DIR/BUILD_INFO.txt"

case "$PLATFORM" in
  linux-*)
    mkdir -p "$DIR/share/applications" "$DIR/share/icons/hicolor/256x256/apps"
    cp "$ROOT/packaging/linux/r-cad.desktop" "$DIR/share/applications/"
    cp "$ROOT/assets/icon/r-cad-256.png" "$DIR/share/icons/hicolor/256x256/apps/r-cad.png"
    install -m 755 "$ROOT/packaging/linux/install.sh" "$DIR/install.sh"
    ;;
  macos-universal)
    APP="$OUT/dmg/R-CAD.app"
    rm -rf -- "$OUT/dmg"
    mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
    cp "$BIN/rcad" "$BIN/rcad-cli" "$APP/Contents/MacOS/"
    sed "s/@VERSION@/$VERSION/g" "$ROOT/packaging/macos/Info.plist" > "$APP/Contents/Info.plist"
    ICONSET="$OUT/r-cad.iconset"
    rm -rf -- "$ICONSET" && mkdir -p "$ICONSET"
    for s in 16 32 128 256 512; do
      sips -z "$s" "$s" "$ROOT/assets/icon/r-cad-1024.png" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
      d=$((s * 2))
      sips -z "$d" "$d" "$ROOT/assets/icon/r-cad-1024.png" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
    done
    iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/r-cad.icns"
    rm -rf -- "$ICONSET"
    # Ad-hoc signature: required for arm64 code; not a Developer ID (see docs/INSTALL.md).
    codesign --force --deep --sign - "$APP"
    codesign --verify --deep --strict "$APP"
    ln -s /Applications "$OUT/dmg/Applications"
    cp "$ROOT/README.md" "$ROOT/LICENSE-MIT" "$ROOT/LICENSE-APACHE" "$OUT/dmg/"
    [ -f "$DIR/THIRD_PARTY_LICENSES.txt" ] && cp "$DIR/THIRD_PARTY_LICENSES.txt" "$OUT/dmg/"
    cp -r "$DIR/examples" "$OUT/dmg/examples"
    rm -f "$OUT/$NAME.dmg"
    hdiutil create -quiet -volname "R-CAD $VERSION" -srcfolder "$OUT/dmg" -fs HFS+ \
      -format UDZO -ov "$OUT/$NAME.dmg"
    rm -rf -- "$OUT/dmg"
    sha256 "$OUT/$NAME.dmg"
    ;;
esac

if [ "$PLATFORM" = windows-x86_64 ]; then
  (cd "$OUT" && rm -f "$NAME.zip" && zip -qr "$NAME.zip" "$NAME")
  sha256 "$OUT/$NAME.zip"
else
  tar -C "$OUT" -czf "$OUT/$NAME.tar.gz" "$NAME"
  sha256 "$OUT/$NAME.tar.gz"
fi
