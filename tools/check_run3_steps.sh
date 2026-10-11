#!/usr/bin/env bash
# Independent FreeCAD 1.1 (OpenCascade) validation of the run-3 scenario STEP exports
# written by `f420-cli acceptance --out DIR` (steps 3A, 3B, 3N, 3P).
# usage: tools/check_run3_steps.sh [DIR]   (default artifacts/run3/acceptance)
# Expected values are analytic (derived here, not read from Fission420 output); probe
# points check feature placement (inside material / inside a removed region).
set -u -o pipefail
DIR=$(realpath "${1:-artifacts/run3/acceptance}")
REPO=$(cd "$(dirname "$0")/.." && pwd)
if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: run-3 STEP validation NOT RUN"; exit 2
fi
status=0
check() { # file, expected sorted volumes (python list expr), bounds (python list), probes JSON, expected inside (python list)
  local f=$1 vols=$2 bounds=$3 probes=$4 inside=$5 out
  out=$(timeout 180 flatpak run --env="F420_STEP=$DIR/$f" --env="F420_PROBES=$probes" \
    --filesystem="$DIR" --filesystem="$REPO" --command=FreeCADCmd org.freecad.FreeCAD \
    "$REPO/tools/freecad_check_probes.py" 2>&1 | grep FREECAD_RESULT) || { echo "$f: FreeCAD failed"; status=1; return; }
  echo "$out" > "$DIR/${f%.step}_freecad.txt"
  python3 - "$f" "$out" "$vols" "$bounds" "$inside" <<'PY' || status=1
import json, math, sys
f, out = sys.argv[1], sys.argv[2]
exp_v = sorted(eval(sys.argv[3], {"math": math})); exp_b = eval(sys.argv[4]); exp_in = eval(sys.argv[5])
r = json.loads(out.split(" ", 1)[1])
ok = bool(r.get("ok") and r.get("isValid") and r.get("solids") == len(exp_v))
for g, e in zip(r.get("solid_volumes", []), exp_v):
    tol = max(0.001, e * 1e-7); good = abs(g - e) <= tol; ok = ok and good
    print(f"  {f}: solid volume {g:.9f} vs {e:.9f} err {abs(g-e):.2e} (tol {tol:.1e}) {'ok' if good else 'BAD'}")
berr = max(abs(a - b) for a, b in zip(r.get("bounds", [0]*6), exp_b)) if r.get("bounds") else 1e9
ok = ok and berr <= 1e-6
print(f"  {f}: bounds {r.get('bounds')} max err {berr:.2e}")
ok = ok and r.get("inside") == exp_in
print(f"  {f}: probes inside {r.get('inside')} expected {exp_in}")
print(f"{f}: solids={r.get('solids')} valid={r.get('isValid')} -> {'PASS' if ok else 'FAIL'}")
sys.exit(0 if ok else 1)
PY
}
# 3A: A = 40x30x10 + 20x10x5 boss at [5,25]x[5,15] on top - 10x5 slot [28,38]x[20,25] through;
#     B = 20x20x10 at x 50..70. Probes: boss (15,10,12) in; slot (33,22.5,5) out; B (60,10,5) in.
check bodies_a.step "[40*30*10 + 20*10*5 - 10*5*10, 20*20*10]" "[0,0,0,70,30,15]" \
  "[[15,10,12],[33,22.5,5],[60,10,5],[45,10,5]]" "[True, False, True, False]"
# 3A visible-only export: A alone (12000), B hidden.
check bodies_a_visible.step "[40*30*10]" "[0,0,0,40,30,10]" "[[20,15,5],[60,10,5]]" "[True, False]"
# 3B at length 70: 70x40x20 minus side pocket 15 deep from x=70, y 15..25, z 7..13.
check side_pocket.step "[70*40*20 - 10*6*15]" "[0,0,0,70,40,20]" \
  "[[65,20,10],[50,20,10],[65,12,10],[65,20,5]]" "[False, True, True, True]"
# 3N: 80x50x10 plate, Ø10 hole at (20,25), 10x6 slot [50,60]x[22,28].
check nested_loops.step "[(80*50 - 25*math.pi - 60)*10]" "[0,0,0,80,50,10]" \
  "[[20,25,5],[55,25,5],[20,31,5],[40,25,5]]" "[False, False, True, True]"
# 3P: 80x20x5 bar, Ø5 through holes at x = 10, 40, 70 (y = 10).
check pattern.step "[80*20*5 - 3*math.pi*2.5**2*5]" "[0,0,0,80,20,5]" \
  "[[10,10,2.5],[40,10,2.5],[70,10,2.5],[25,10,2.5],[55,10,2.5]]" "[False, False, False, True, True]"
exit $status
