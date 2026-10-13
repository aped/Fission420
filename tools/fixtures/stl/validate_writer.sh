#!/usr/bin/env bash
# Independent validation of the STL writers with FreeCAD's Mesh module (tools/freecad_check_stl.py).
# Expected (analytic): 10 mm cube -> 8 points, 12 facets, solid, volume 1000, bounds 0..10.
# usage: tools/fixtures/stl/validate_writer.sh [OUT_DIR]   (default artifacts/exchange/stl)
set -euo pipefail
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
OUT=$(realpath -m "${1:-$REPO/artifacts/exchange/stl}")
mkdir -p "$OUT"
(cd "$REPO" && cargo run -q --release -p rcad-exchange --example stl_samples -- "$OUT")
status=0
for f in cube10_binary.stl cube10_ascii.stl; do
  r=$(timeout 180 flatpak run --env="RCAD_STL=$OUT/$f" --filesystem="$REPO" --filesystem="$OUT" \
    --command=FreeCADCmd org.freecad.FreeCAD "$REPO/tools/freecad_check_stl.py" 2>&1 | grep -o 'FREECAD_RESULT .*')
  echo "$f: $r"
  python3 - "$r" <<'PY' || status=1
import json, sys
r = json.loads(sys.argv[1].split(" ", 1)[1])
ok = (r["points"] == 8 and r["facets"] == 12 and r["isSolid"] and not r["hasNonManifolds"]
      and abs(r["volume"] - 1000.0) < 1e-6 and r["bounds"] == [0, 0, 0, 10, 10, 10])
print("PASS" if ok else "FAIL")
sys.exit(0 if ok else 1)
PY
done
exit $status
