#!/usr/bin/env bash
# Regenerate the FreeCAD-made STL fixtures (needs the org.freecad.FreeCAD flatpak).
set -euo pipefail
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
OUT="$REPO/crates/f420-exchange/tests/fixtures/stl"
mkdir -p "$OUT"
timeout 180 flatpak run --env="OUT=$OUT" --filesystem="$REPO" --command=FreeCADCmd \
  org.freecad.FreeCAD "$REPO/tools/fixtures/stl/gen_freecad.py" 2>&1 | grep -E 'FREECAD_RESULT|removed'
python3 "$REPO/tools/fixtures/stl/gen_python.py" "$OUT"
