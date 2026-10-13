#!/usr/bin/env bash
# Independent FreeCAD (OpenCascade) validation of the run-5 sweep fixtures written by
#   RCAD_EVIDENCE_DIR=DIR cargo test --release -p rcad-kernel --lib sweep::
# usage: tools/heavy.sh tools/check_sweep.sh DIR
# FreeCAD integrates the exact surfaces (the channel's torus segment volume is only
# estimated by our own measure, which has no exact path for tori). Expected values are
# analytic (Pappus).
set -u -o pipefail
DIR=$(realpath "${1:?usage: check_sweep.sh DIR}")
REPO=$(cd "$(dirname "$0")/.." && pwd)
if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: sweep STEP validation NOT RUN"; exit 2
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
# Handle: 8 x 4 rectangle along line 10 (+Z), 90° arc R 15 about (15,0,10)/+Y, line 10 (+X).
check sweep_handle.step "32*(20 + 7.5*pi)" "[-4,-2,0,25,2,29]" \
  "[[0,0,5],[4.2,0,5],[4.393398,0,20.606602],[1.423,0,23.577],[20,0,25],[20,0,29.2]]" \
  "[True, False, True, False, True, False]"
# Channel: Ø6 disc along the same path.
check sweep_channel_d6.step "9*pi*(20 + 7.5*pi)" "[-3,-3,0,25,3,28]" \
  "[[0,2.8,5],[0,3.2,5],[20,0,27.8],[20,0,28.2]]" "[True, False, True, False]"
exit $status
