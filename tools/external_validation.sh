#!/usr/bin/env bash
# Independent checks of exported files (not the Rust writer/reader):
#   - tools/check_stl.py (pure-Python STL parser)
#   - FreeCAD 1.1 flatpak (OpenCascade) for STL mesh + STEP solid import, if installed.
# usage: tools/external_validation.sh [DIR]
# DIR (default artifacts/acceptance) must contain plate.stl/plate.step; scenario_a.* and
# scenario_b.* (from `f420-cli acceptance`) are checked too when present.
# Expected volumes are analytic (computed here, not read from Fission420 output):
#   plate      100 x 50 x 10, centred Ø10 through hole: 50000 - 250*pi
#   scenario_a 100 x 50 x 12, centred Ø10 through hole: 60000 - 300*pi
#   scenario_b_initial 5 mm prism over (0,0)(60,0)(60,30)(50,40)(0,40): 5*2350
#   scenario_b 5 mm prism, area 60*40 - 0.5*10*sqrt(12^2 - 10^2): 5*(2400 - 5*sqrt(44))
set -u -o pipefail  # a failing checker inside "| tee" must fail the script
DIR=$(realpath "${1:-artifacts/acceptance}")
REPO=$(cd "$(dirname "$0")/.." && pwd)
status=0
expected() { python3 -c "import math; print(repr($1))"; }

check_stem() { # stem, python expression for the expected volume
  local stem=$1 exp
  exp=$(expected "$2")
  echo "== $stem: python STL check (expected volume $exp)"
  python3 "$REPO/tools/check_stl.py" "$DIR/$stem.stl" "$exp" | tee "$DIR/${stem}_stl_check_python.json" || status=1
  if [ "$HAVE_FC" = 1 ]; then
    echo "== $stem: FreeCAD STL"
    run_fc "F420_STL=$DIR/$stem.stl" "$REPO/tools/freecad_check_stl.py" | tee "$DIR/${stem}_stl_check_freecad.txt" || status=1
    echo "== $stem: FreeCAD STEP"
    local out
    out=$(run_fc "F420_STEP=$DIR/$stem.step" "$REPO/tools/freecad_check_step.py") || status=1
    echo "$out" | tee "$DIR/${stem}_step_check_freecad.txt"
    python3 - "$out" "$exp" <<'PY' || status=1
import json, sys
r = json.loads(sys.argv[1].split(" ", 1)[1]); exp = float(sys.argv[2])
err = abs(r.get("volume", 0) - exp)
ok = r.get("ok") and r.get("isValid") and r.get("solids") == 1 and err <= max(0.001, exp * 1e-7)
print(f"STEP via OpenCascade: solids={r.get('solids')} valid={r.get('isValid')} volume_err={err:.3e} -> {'PASS' if ok else 'FAIL'}")
sys.exit(0 if ok else 1)
PY
  fi
}

HAVE_FC=0
if flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  HAVE_FC=1
  run_fc() { timeout 180 flatpak run --env="$1" --filesystem="$REPO" --filesystem="$DIR" --command=FreeCADCmd org.freecad.FreeCAD "$2" 2>&1 | grep FREECAD_RESULT; }
else
  echo "FreeCAD flatpak not installed: external STEP validation NOT RUN"
fi

# Keep the original file names for the plate (run-1 evidence paths).
exp_plate=$(expected "50000 - 250*math.pi")
echo "== plate: python STL check"
python3 "$REPO/tools/check_stl.py" "$DIR/plate.stl" "$exp_plate" | tee "$DIR/stl_check_python.json" || status=1
if [ "$HAVE_FC" = 1 ]; then
  echo "== plate: FreeCAD STL"
  run_fc "F420_STL=$DIR/plate.stl" "$REPO/tools/freecad_check_stl.py" | tee "$DIR/stl_check_freecad.txt" || status=1
  echo "== plate: FreeCAD STEP"
  out=$(run_fc "F420_STEP=$DIR/plate.step" "$REPO/tools/freecad_check_step.py") || status=1
  echo "$out" | tee "$DIR/step_check_freecad.txt"
  python3 - "$out" "$exp_plate" <<'PY' || status=1
import json, sys
r = json.loads(sys.argv[1].split(" ", 1)[1]); exp = float(sys.argv[2])
err = abs(r.get("volume", 0) - exp)
ok = r.get("ok") and r.get("isValid") and r.get("solids") == 1 and err <= max(0.001, exp * 1e-7)
print(f"STEP via OpenCascade: solids={r.get('solids')} valid={r.get('isValid')} volume_err={err:.3e} -> {'PASS' if ok else 'FAIL'}")
sys.exit(0 if ok else 1)
PY
fi

[ -f "$DIR/scenario_a.stl" ] && check_stem scenario_a "60000 - 300*math.pi"
[ -f "$DIR/scenario_b_initial.stl" ] && check_stem scenario_b_initial "5*(60*40 - 0.5*10*10)"
[ -f "$DIR/scenario_b.stl" ] && check_stem scenario_b "5*(2400 - 5*math.sqrt(44))"
exit $status
