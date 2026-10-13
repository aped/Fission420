#!/usr/bin/env bash
# Regenerate crates/rcad-kernel/tests/fixtures/step/direct_edit_block_mm.step (run 6
# direct-edit fixture) with the FreeCAD flatpak (made with 1.1.4 / OCC 7.8.1).
# usage: tools/heavy.sh tools/fixtures/step/direct_edit.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
OUT="$REPO/crates/rcad-kernel/tests/fixtures/step"
flatpak run --filesystem="$REPO" --env=RCAD_OUT="$OUT" \
    --command=FreeCADCmd org.freecad.FreeCAD "$HERE/direct_edit.py" 2>&1 | grep RCAD_RESULTS
