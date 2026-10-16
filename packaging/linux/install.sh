#!/bin/sh
# Install R-CAD from this unpacked release archive for the current user: binaries in
# ~/.local/bin, the desktop entry and the icon under ~/.local/share, so R-CAD appears in
# the application menu. PREFIX=/usr/local (as root) installs system-wide.
#   ./install.sh              install
#   ./install.sh --uninstall  remove what install placed
set -eu
PREFIX=${PREFIX:-$HOME/.local}
HERE=$(cd "$(dirname "$0")" && pwd)
BIN="$PREFIX/bin"
APPS="$PREFIX/share/applications"
ICONS="$PREFIX/share/icons/hicolor/256x256/apps"
refresh() {
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPS" 2>/dev/null || true
  command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
}
if [ "${1:-}" = "--uninstall" ]; then
  rm -f "$BIN/rcad" "$BIN/rcad-cli" "$APPS/r-cad.desktop" "$ICONS/r-cad.png"
  refresh
  echo "R-CAD removed from $PREFIX"
  exit 0
fi
mkdir -p "$BIN" "$APPS" "$ICONS"
install -m 755 "$HERE/rcad" "$HERE/rcad-cli" "$BIN/"
install -m 644 "$HERE/share/icons/hicolor/256x256/apps/r-cad.png" "$ICONS/"
# Absolute Exec: launchers do not always search ~/.local/bin.
sed "s|^Exec=rcad |Exec=$BIN/rcad |" "$HERE/share/applications/r-cad.desktop" > "$APPS/r-cad.desktop"
chmod 644 "$APPS/r-cad.desktop"
refresh
echo "R-CAD installed: $BIN/rcad (menu entry: $APPS/r-cad.desktop)"
