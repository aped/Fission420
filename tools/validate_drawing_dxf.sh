#!/usr/bin/env bash
# External check of the drawing DXF of the 40 x 30 x 10 block with a Ø8 through hole
# (written by `cargo test -p rcad-commands --test it drawings::` to artifacts/drawings/sheets/block.dxf)
# with FreeCAD's DXF importer (flatpak org.freecad.FreeCAD) as the independent reader.
# Expected values are hand-derived for the three standard views at 1:1:
#   VISIBLE: 12 lines, total 2(40+10) + 2(40+30) + 2(30+10) = 320 mm, 1 circle (the hole, top view)
#   HIDDEN:  4 lines (the hole's silhouettes in the front and right views), total 4 x 10 = 40 mm
#   CENTER:  2 lines (centre mark), total 2 x 2 x (4 + 2) = 24 mm, plus the hole's
#            centre line in the front and right views, 10 + 2 x 2 = 14 mm each: 4 lines, 52 mm
# Usage: tools/validate_drawing_dxf.sh <block.dxf>   (absolute or relative path)
set -euo pipefail
DXF=$(readlink -f "${1:?usage: validate_drawing_dxf.sh <block.dxf>}")
ROOT=$(pwd)
command -v flatpak >/dev/null || { echo "SKIP: flatpak/FreeCAD not available"; exit 0; }
RES=$(tools/heavy.sh flatpak run --env=RCAD_DXF="$DXF" --filesystem="$ROOT" \
  --filesystem="$(dirname "$DXF")" --command=FreeCADCmd org.freecad.FreeCAD \
  "$ROOT/tools/freecad_check_drawing_dxf.py" 2>&1 | grep -a -o "FREECAD_RESULT.*")
echo "$RES"
python3 -I - "$RES" <<'PY'
import json, sys
r = json.loads(sys.argv[1].split("FREECAD_RESULT ", 1)[1])
assert r["ok"], r
L = r["layers"]
def near(a, b): return abs(a - b) < 1e-6
checks = [
    L["VISIBLE"]["lines"] == 12 and near(L["VISIBLE"]["line_length"], 320.0),
    L["VISIBLE"]["circles"] == 1,
    L["HIDDEN"]["lines"] == 4 and near(L["HIDDEN"]["line_length"], 40.0),
    L["CENTER"]["lines"] == 4 and near(L["CENTER"]["line_length"], 52.0),
]
print("OK" if all(checks) else "FAIL", checks)
sys.exit(0 if all(checks) else 1)
PY
