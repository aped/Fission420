#!/usr/bin/env bash
# Mesh edits (docs/MESH.md) checked by independent readers: replays
# tools/fixtures/mesh_edit_workflow.json (FreeCAD's ASCII sphere: two-sided plane cut
# at z = 2 with caps, decimation within 0.05 mm, scale/rotate/translate), then
# * tools/check_mesh_edit.py (Python stdlib): watertight halves whose volumes sum to the
#   source, cut planes at z = 2, decimation deviation measured by brute force <= 0.05 mm,
#   transformed volume and bounds against the source transformed in Python;
# * FreeCAD's Mesh module on every export: isSolid, no non-manifolds, no
#   self-intersections.
# Usage: tools/validate_mesh_edit.sh <rcad-cli>   (run from the repo root)
set -euo pipefail
CLI=$(readlink -f "${1:?usage: validate_mesh_edit.sh <rcad-cli>}")
ROOT=$(pwd)
OUT=artifacts/mesh_edit_validation
mkdir -p "$OUT"
rm -f artifacts/mesh_edit_{lower,upper,decimated,transformed}.stl
"$CLI" run tools/fixtures/mesh_edit_workflow.json > "$OUT/run.jsonl"
fail=0
if grep -q '"as_expected":false' "$OUT/run.jsonl"; then echo "FAIL: workflow step not as expected"; fail=1; fi
python3 -I tools/check_mesh_edit.py tools/fixtures/corpus/m2_sphere_ascii.stl \
  artifacts/mesh_edit_lower.stl artifacts/mesh_edit_upper.stl \
  artifacts/mesh_edit_decimated.stl artifacts/mesh_edit_transformed.stl \
  | tee "$OUT/python_check.txt" || fail=1
if command -v flatpak >/dev/null; then
  for f in lower upper decimated transformed; do
    tools/heavy.sh flatpak run --env=RCAD_STL="$ROOT/artifacts/mesh_edit_$f.stl" --filesystem="$ROOT" \
      --command=FreeCADCmd org.freecad.FreeCAD "$ROOT/tools/freecad_check_stl.py" 2>&1 \
      | grep -a "FREECAD_RESULT" > "$OUT/freecad_$f.log" || fail=1
  done
  python3 -I - "$OUT" <<'PY' || fail=1
import json, os, sys
ok = True
for f in ("lower", "upper", "decimated", "transformed"):
    line = open(os.path.join(sys.argv[1], f"freecad_{f}.log")).read()
    r = json.loads(line.split("FREECAD_RESULT ", 1)[1])
    for k, v in {"solid": r["isSolid"], "manifold": not r["hasNonManifolds"],
                 "no self-intersections": not r["hasSelfIntersections"]}.items():
        print(("PASS: " if v else "FAIL: ") + f"FreeCAD {f} {k} (volume {r['volume']:.6f})")
        ok &= v
sys.exit(0 if ok else 1)
PY
else
  echo "SKIP: FreeCAD (flatpak) not available"
fi
[ "$fail" = 0 ] && echo "ALL OK" || { echo "FAILED"; exit 1; }
