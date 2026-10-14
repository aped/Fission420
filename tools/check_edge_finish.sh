#!/usr/bin/env bash
# Independent FreeCAD (OpenCascade) validation of the edge-finishing fixtures
# (slanted-end chamfer, exact fillet) written by
#   RCAD_EVIDENCE_DIR=DIR cargo test --release -p rcad-kernel --lib chamfer fillet
# usage: tools/heavy.sh tools/check_edge_finish.sh DIR
# Expected volumes/bounds/probes are analytic (derived here from the test profiles).
set -u -o pipefail
DIR=$(realpath "${1:?usage: check_edge_finish.sh DIR}")
REPO=$(cd "$(dirname "$0")/.." && pwd)
if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: edge-finish STEP validation NOT RUN"; exit 2
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
pi, tan, rad = math.pi, math.tan, math.radians
env = {"math": math, "pi": pi, "tan": tan, "rad": rad, "atan2": math.atan2}
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
# Trapezoid prism (XZ profile, 30 deep along -Y), right end in by 25°, left end out by 15°,
# top/front (y = 0) edge chamfered 2.5: removed (d²/2)(40 - (ta+tb)(10 - d/3)).
TA="tan(rad(25))"; TB="tan(rad(-15))"
check chamfer_slanted_xz_d2.5.step \
  "30*10*(80 - 10*($TA + $TB))/2 - 0.5*2.5**2*(40 - ($TA + $TB)*(10 - 2.5/3))" \
  "[10*$TB, -30, 0, 40, 0, 10]" \
  "[[20,-0.2,9.8],[20,-2.7,9.95],[20,-0.05,7.4],[20,-15,5]]" "[False, True, True, True]"
# Plan trapezoid (end x = 40 - y tan 20°), 10 thick, top/front edge chamfered 3.
T="tan(rad(20))"
check chamfer_slanted_plan_d3.step "10*30*(80 - 30*$T)/2 - 0.5*9*(40 - $T)" \
  "[0,0,0,40,30,10]" "[[20,0.2,9.8],[20,3.2,9.95],[20,15,5]]" "[False, True, True]"
# 40x30x10 block, top/front edge filleted r = 2: removes (4 - pi) * 40.
check fillet_box_r2.step "12000 - (4 - pi)*40" "[0,0,0,40,30,10]" \
  "[[20,0.1,9.9],[20,1.8,9.95],[20,0.05,8.2],[20,15,5]]" "[False, True, True, True]"
# Prism along x of (y,z) (0,0)(30,0)(30,10)(5,10), top/slope edge (theta = 180° - atan2(10,5)) r = 3.
TH="(pi - atan2(10, 5))"
check fillet_obtuse_r3.step \
  "275*40 - ((3/tan($TH/2))*3 - (pi - $TH)*9/2)*40" "[0,0,0,40,30,10]" \
  "[[20,5.05,9.97],[20,20,5],[20,6.6687,9.98]]" "[False, True, True]"
exit $status
