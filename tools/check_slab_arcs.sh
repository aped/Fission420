#!/usr/bin/env bash
# Independent FreeCAD (OpenCascade) validation of the slab-construction fixtures
# (ADR 0007) written by
#   RCAD_EVIDENCE_DIR=DIR cargo test --release -p rcad-commands --test slab_arcs --test rim_notch
# usage: tools/heavy.sh tools/check_slab_arcs.sh DIR
# Expected volumes, bounds and probe classifications are analytic (derived here, not read
# from R-CAD output). Checks per file: STEP readable, exactly one solid, isValid
# (BRepCheck), solid volume, bounds, point probes inside/outside, and the surface types.
set -u -o pipefail
DIR=$(realpath "${1:?usage: check_slab_arcs.sh DIR}")
REPO=$(cd "$(dirname "$0")/.." && pwd)
if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: slab-arc STEP validation NOT RUN"; exit 2
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
pi = math.pi
def disc_union(a, b, d):
    if d >= a + b: return pi * (a*a + b*b)
    if d <= abs(a - b): return pi * max(a, b)**2
    lens = (a*a*math.acos((d*d + a*a - b*b)/(2*d*a)) + b*b*math.acos((d*d + b*b - a*a)/(2*d*b))
            - 0.5*math.sqrt((-d+a+b)*(d+a-b)*(d-a+b)*(d+a+b)))
    return pi*(a*a + b*b) - lens
rr = lambda w, h, r: w*h - (4 - pi)*r*r
f, out = sys.argv[1], sys.argv[2]
env = {"math": math, "pi": pi, "disc_union": disc_union, "rr": rr}
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
# Canonical: 80x50x10, Ø10 then concentric Ø16 through at (40,25).
check canonical_engulfed_d16.step "40000 - 640*pi" "[0,0,0,80,50,10]" \
  "[[40,25,5],[46,25,5],[34.5,25,5],[48.2,25,5],[40,32.9,9.9],[40,16.8,0.1],[45.2,25,0.5]]" \
  "[False, False, False, True, False, True, False]"
# Off-centre: Ø16 at (44,25) through over the Ø10 at (40,25).
check offset_overlap_d16.step "40000 - 10*disc_union(5, 8, 4)" "[0,0,0,80,50,10]" \
  "[[34.8,25,5],[40,25,5],[44,25,9],[35.5,25,5],[52.2,25,5]]" "[True, False, False, False, True]"
# Seedhole repro: 60x30x8 block (centred), 10x10x5 boss at (15,0), Ø4 x2 at (15,0) and (16,0.5).
check seedhole_two_d4.step "14400 + 500 - 2*disc_union(2, 2, math.hypot(1, 0.5))" "[-30,-15,0,30,15,13]" \
  "[[15.5,0.25,12],[15.5,0.25,10.5],[13.3,0,12],[17.6,1.4,12],[19.5,-4.5,12]]" \
  "[False, True, False, False, True]"
# Rounded pockets (r4) 3 deep over the Ø10 hole, the second sharing the corner circle (51,31).
check rounded_pockets_over_hole.step \
  "40000 - 250*pi - 3*(rr(25,20,4) - 25*pi) - 3*(rr(23,18,4) - (64 - 2*(1 - pi/4)*16))" "[0,0,0,80,50,10]" \
  "[[54.5,34.5,8.5],[47.5,27.5,8.5],[56,26,8.5],[46,36,8.5],[51,31,6.8],[40,25,3],[45.3,25,3]]" \
  "[False, False, True, True, True, False, True]"
# Holed region: [20,60]x[10,40] minus Ø12 at (40,25), 4 deep: a round island stands.
check holed_region_round_island.step "40000 - 4*(1200 - 36*pi)" "[0,0,0,80,50,10]" \
  "[[40,25,8],[22,12,8],[40,25,5],[47,25,8]]" "[True, False, True, False]"
# Rim notch on the baseline tray (120x80x30, walls 2, 2 dividers, notch 16x8 on compartment 1).
cw=$(python3 -c "print((120-4-4)/3)")
check "rim_notch_120x80x30_WorldXY_0_0.step" "49408" "[0,0,0,120,80,30]" \
  "[[$(python3 -c "print(2+$cw/2)"),1,26],[$(python3 -c "print(2+$cw/2)"),1,21.8],[$(python3 -c "print(2+$cw/2-8.2)"),1,26],[$(python3 -c "print(2+$cw/2)"),3,26],[$(python3 -c "print(2+$cw+1)"),40,26]]" \
  "[False, True, True, False, True]"
exit $status
