#!/usr/bin/env bash
# Regenerate the surface/open-shell STEP fixtures in
# crates/rcad-kernel/tests/fixtures/step/ (open_box_mm, loose_faces_mm, gap_box_mm,
# plate_mm, half_tube_mm) with the FreeCAD flatpak (made with 1.1.4 / OCC 7.8.1).
# usage: tools/heavy.sh tools/fixtures/step/open_shells.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
OUT="$REPO/crates/rcad-kernel/tests/fixtures/step"
flatpak run --filesystem="$REPO" --env=RCAD_OUT="$OUT" \
    --command=FreeCADCmd org.freecad.FreeCAD "$HERE/open_shells.py" 2>&1 | grep RCAD_RESULTS
