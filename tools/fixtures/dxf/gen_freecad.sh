#!/usr/bin/env bash
# Regenerate the DXF fixtures: FreeCAD-written ones (needs the org.freecad.FreeCAD flatpak)
# and the hand-written ones (Python stdlib only).
set -euo pipefail
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
OUT="$REPO/crates/rcad-exchange/tests/fixtures/dxf"
mkdir -p "$OUT"
timeout 180 flatpak run --env="OUT=$OUT" --filesystem="$REPO" --command=FreeCADCmd \
  org.freecad.FreeCAD "$REPO/tools/fixtures/dxf/gen_freecad.py" 2>&1 | grep FREECAD_RESULT
python3 "$REPO/tools/fixtures/dxf/gen_handwritten.py" "$OUT"
