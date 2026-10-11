#!/usr/bin/env bash
# Independent validation of f420_exchange::threemf::write with
#   (1) tools/fixtures/threemf/check_3mf.py (Python stdlib zipfile + ElementTree), and
#   (2) FreeCAD 1.1.4's 3MF reader (Mesh module; it merges all build items, ignores the
#       unit attribute and does not flatten components -- observed, see README.md).
# Expected (analytic, from crates/f420-exchange/examples/threemf_samples.rs):
#   2 objects, 2 build items; cube10: 8 vertices, 12 triangles, closed, V 1000 mm^3,
#   colour #FF8000FF; tet: 4 vertices, 4 triangles, closed, V 12^3/6 = 288 mm^3.
#   mm file: unit millimeter, volumes as above. inch file: unit inch, volumes / 25.4^3.
#   FreeCAD: 12 points, 16 facets, solid, volume 1288 (file units).
# usage: tools/fixtures/threemf/validate_writer.sh [OUT_DIR]   (default artifacts/exchange/threemf)
set -euo pipefail
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
OUT=$(realpath -m "${1:-$REPO/artifacts/exchange/threemf}")
mkdir -p "$OUT"
(cd "$REPO" && cargo run -q --release -p f420-exchange --example threemf_samples -- "$OUT")
status=0
py=$(python3 -I "$REPO/tools/fixtures/threemf/check_3mf.py" "$OUT/f420_two_mm.3mf" "$OUT/f420_two_inch.3mf")
echo "$py" | tee "$OUT/check_3mf_python.txt"
python3 - "$py" <<'PY' || status=1
import json, sys
ok = True
for line in sys.argv[1].splitlines():
    r = json.loads(line)
    s = 1.0 if r["file"] == "f420_two_mm.3mf" else 25.4
    want_unit = "millimeter" if s == 1.0 else "inch"
    o = {x["name"]: x for x in r["objects"]}
    good = (r["unit"] == want_unit and len(r["objects"]) == 2 and r["items"] == ["1", "2"]
            and o["cube10"]["vertices"] == 8 and o["cube10"]["triangles"] == 12
            and o["cube10"]["closed"] and abs(o["cube10"]["volume"] - 1000 / s**3) <= 1e-9 * 1000 / s**3
            and o["cube10"]["color"] == "#FF8000FF"
            and o["tet"]["vertices"] == 4 and o["tet"]["closed"] and o["tet"]["color"] is None
            and abs(o["tet"]["volume"] - 288 / s**3) <= 1e-9 * 288 / s**3)
    print(("PASS " if good else "FAIL ") + r["file"] + " (python)")
    ok &= good
sys.exit(0 if ok else 1)
PY
if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: FreeCAD 3MF check NOT RUN"; exit 2
fi
fc=$(timeout 180 flatpak run --env=F420_MODE=read --env="F420_3MF=$OUT/f420_two_mm.3mf:$OUT/f420_two_inch.3mf" \
  --filesystem="$REPO" --filesystem="$OUT" --command=FreeCADCmd org.freecad.FreeCAD \
  "$REPO/tools/fixtures/threemf/freecad_3mf.py" 2>&1 | grep -o 'FREECAD_RESULT .*')
echo "$fc" | tee "$OUT/check_3mf_freecad.txt"
python3 - "$fc" <<'PY' || status=1
import json, sys
ok = True
for line in sys.argv[1].splitlines():
    r = json.loads(line.split(" ", 1)[1])
    s = 1.0 if r["file"] == "f420_two_mm.3mf" else 25.4
    v = 1288 / s**3
    good = (r.get("points") == 12 and r.get("facets") == 16 and r.get("isSolid")
            and abs(r.get("volume", 0) - v) <= 1e-5 * v)  # FreeCAD meshes are float32
    print(("PASS " if good else "FAIL ") + r["file"] + " (FreeCAD)")
    ok &= good
sys.exit(0 if ok else 1)
PY
exit $status
