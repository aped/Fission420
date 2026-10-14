#!/usr/bin/env bash
# Repair tools/fixtures/stl/broken_cube.stl (a 20 mm cube without its top
# face, one flipped side triangle, a duplicate and a degenerate triangle) with
# tools/fixtures/mesh_repair_workflow.json, then check both exports with independent
# readers: tools/check_stl.py (Python, exact float32 weld) and FreeCAD's Mesh module.
# Expected: the STL export of the unrepaired body is refused (strict mesh policy: open,
# non-manifold); the repaired export is closed, 12 facets, volume 8000 mm³, bounds [0,20]³,
# no non-manifolds, no self-intersections.
# Usage: tools/validate_mesh_repair.sh <rcad-cli>   (run from the repo root)
set -euo pipefail
CLI=$(readlink -f "${1:?usage: validate_mesh_repair.sh <rcad-cli>}")
ROOT=$(pwd)
OUT=artifacts/mesh_repair_validation
mkdir -p "$OUT"
rm -f artifacts/mesh_repair_original.stl artifacts/mesh_repair_repaired.stl
"$CLI" run tools/fixtures/mesh_repair_workflow.json > "$OUT/run.jsonl"
fail=0
grep -q '"command":"export_stl".*"ok":false' "$OUT/run.jsonl" && echo "PASS: unrepaired export refused" || { echo "FAIL: unrepaired export not refused"; fail=1; }
[ ! -e artifacts/mesh_repair_original.stl ] || { echo "FAIL: a file was written for the refused export"; fail=1; }
python3 -I tools/check_stl.py artifacts/mesh_repair_repaired.stl 8000 > "$OUT/repaired_check.json" || fail=1
cat "$OUT/repaired_check.json"
python3 -I - "$OUT/repaired_check.json" <<'PY' || fail=1
import json, sys
r = json.load(open(sys.argv[1]))
print(r)
checks = {
    "repaired closed (check_stl)": r.get("closed", r.get("watertight", False)),
    "repaired volume 8000 (check_stl)": abs(r["volume"] - 8000.0) < 1e-3,
}
for k, v in checks.items():
    print(("PASS: " if v else "FAIL: ") + k)
sys.exit(0 if all(checks.values()) else 1)
PY
if command -v flatpak >/dev/null; then
  for f in repaired; do
    tools/heavy.sh flatpak run --env=RCAD_STL="$ROOT/artifacts/mesh_repair_$f.stl" --filesystem="$ROOT" \
      --command=FreeCADCmd org.freecad.FreeCAD "$ROOT/tools/freecad_check_stl.py" 2>&1 \
      | tee "$OUT/freecad_$f.log" | grep -a "FREECAD_RESULT" || fail=1
  done
  python3 -I - "$OUT/freecad_repaired.log" <<'PY' || fail=1
import json, sys
def res(p):
    line = [l for l in open(p, errors="replace") if "FREECAD_RESULT" in l][0]
    return json.loads(line.split("FREECAD_RESULT ", 1)[1])
r = res(sys.argv[1])
checks = {
    "FreeCAD repaired solid": r["isSolid"],
    "FreeCAD repaired 12 facets": r["facets"] == 12,
    "FreeCAD repaired volume 8000": abs(r["volume"] - 8000.0) < 1e-3,
    "FreeCAD repaired manifold": not r["hasNonManifolds"],
    "FreeCAD repaired no self-intersections": not r["hasSelfIntersections"],
    "FreeCAD repaired bounds": all(abs(a - b) < 1e-6 for a, b in zip(r["bounds"], [0, 0, 0, 20, 20, 20])),
}
for k, v in checks.items():
    print(("PASS: " if v else "FAIL: ") + k)
sys.exit(0 if all(checks.values()) else 1)
PY
else
  echo "SKIP: FreeCAD (flatpak) not available"
fi
[ "$fail" = 0 ] && echo "ALL OK" || { echo "FAILED"; exit 1; }
