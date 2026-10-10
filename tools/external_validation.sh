#!/usr/bin/env bash
# Independent checks of exported files (not the Rust writer/reader):
#   - tools/check_stl.py (pure-Python STL parser)
#   - FreeCAD 1.1 flatpak (OpenCascade) for STL mesh + STEP solid import, if installed.
# usage: tools/external_validation.sh [DIR containing plate.stl and plate.step]
set -u
DIR=$(realpath "${1:-artifacts/acceptance}")
REPO=$(cd "$(dirname "$0")/.." && pwd)
EXPECTED=49214.60183660255
status=0
echo "== python STL check"
python3 "$REPO/tools/check_stl.py" "$DIR/plate.stl" "$EXPECTED" | tee "$DIR/stl_check_python.json" || status=1
if flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  run_fc() { timeout 180 flatpak run --env="$1" --filesystem="$REPO" --filesystem="$DIR" --command=FreeCADCmd org.freecad.FreeCAD "$2" 2>&1 | grep FREECAD_RESULT; }
  echo "== FreeCAD STL"
  run_fc "F420_STL=$DIR/plate.stl" "$REPO/tools/freecad_check_stl.py" | tee "$DIR/stl_check_freecad.txt" || status=1
  echo "== FreeCAD STEP"
  out=$(run_fc "F420_STEP=$DIR/plate.step" "$REPO/tools/freecad_check_step.py") || status=1
  echo "$out" | tee "$DIR/step_check_freecad.txt"
  python3 - "$out" "$EXPECTED" <<'PY' || status=1
import json, sys
r = json.loads(sys.argv[1].split(" ", 1)[1]); exp = float(sys.argv[2])
err = abs(r.get("volume", 0) - exp)
ok = r.get("ok") and r.get("isValid") and r.get("solids") == 1 and err <= max(0.001, exp * 1e-7)
print(f"STEP via OpenCascade: solids={r.get('solids')} valid={r.get('isValid')} volume_err={err:.3e} -> {'PASS' if ok else 'FAIL'}")
sys.exit(0 if ok else 1)
PY
else
  echo "FreeCAD flatpak not installed: external STEP validation NOT RUN"
fi
exit $status
