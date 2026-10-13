#!/usr/bin/env bash
# Independent FreeCAD (OpenCascade) validation of the run-5 loft fixtures written by
#   RCAD_EVIDENCE_DIR=DIR cargo test --release -p rcad-kernel --lib loft
# usage: tools/heavy.sh tools/check_loft.sh DIR
# FreeCAD integrates the exact surfaces (the cone frustum's volume is only estimated
# by our own measure, which has no exact path for cones). Expected values analytic.
set -u -o pipefail
DIR=$(realpath "${1:?usage: check_loft.sh DIR}")
REPO=$(cd "$(dirname "$0")/.." && pwd)
if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: loft STEP validation NOT RUN"; exit 2
fi
status=0
check() { # file, expected volume (python expr), bounds (python list), probes JSON, expected inside (python list)
  local f=$1 vol=$2 bounds=$3 probes=$4 inside=$5 out
  out=$(timeout 300 flatpak run --env="RCAD_STEP=$DIR/$f" --env="RCAD_PROBES=$probes" \
    --filesystem="$DIR" --filesystem="$REPO" --command=FreeCADCmd org.freecad.FreeCAD \
    "$REPO/tools/freecad_check_probes.py" 2>&1 | grep FREECAD_RESULT) || { echo "$f: FreeCAD failed"; status=1; return; }
  echo "$out" > "$DIR/${f%.step}_freecad.txt"
  python3 - "$f" "$out" "$vol" "$bounds" "$inside" <<'PY' || status=1
import json, math, sys
env = {"math": math, "pi": math.pi}
f, out = sys.argv[1], sys.argv[2]
exp_v = eval(sys.argv[3], env); exp_b = eval(sys.argv[4], env); exp_in = eval(sys.argv[5], env)
r = json.loads(out.split(" ", 1)[1])
ok = bool(r.get("ok") and r.get("isValid") and r.get("solids") == 1)
g = (r.get("solid_volumes") or [float("nan")])[0]
tol = max(0.001, exp_v * 1e-7); good = abs(g - exp_v) <= tol; ok = ok and good
print(f"  {f}: solids={r.get('solids')} valid={r.get('isValid')} volume {g:.9f} vs {exp_v:.9f} err {abs(g-exp_v):.2e} (tol {tol:.1e}) {'ok' if good else 'BAD'}")
berr = max(abs(a - b) for a, b in zip(r.get("bounds", [0]*6), exp_b)) if r.get("bounds") else 1e9
ok = ok and berr <= 1e-6
print(f"  {f}: bounds {[round(x, 9) for x in r.get('bounds', [])]} max err {berr:.2e}")
ok = ok and r.get("inside") == exp_in
print(f"  {f}: probes inside {r.get('inside')} expected {exp_in}")
print(f"{f}: {'PASS' if ok else 'FAIL'}")
sys.exit(0 if ok else 1)
PY
}
# 60x40 (z = 0) to 70x50 (z = 30), centred: sides 60 + z/3 by 40 + z/3.
check loft_tray_blank.step "88000" "[-35,-25,0,35,25,30]" \
  "[[32.4,0,15],[32.6,0,15],[0,22.4,15],[0,22.6,15],[0,0,1]]" "[True, False, True, False, True]"
# Ø40 (z = 0) to Ø30 (z = 30): r(z) = 20 - z/6.
check loft_frustum_d40_d30.step "pi*30/3*(400 + 300 + 225)" "[-20,-20,0,20,20,30]" \
  "[[17.4,0,15],[17.6,0,15],[0,-19.7,1],[0,-19.9,1],[0,0,29]]" "[True, False, True, False, True]"
exit $status
