#!/usr/bin/env bash
# Independent FreeCAD (OpenCascade) validation of the direct-edit STEP exports written by
#   RCAD_EVIDENCE_DIR=DIR cargo test --release -p rcad-kernel --lib direct
# usage: tools/heavy.sh tools/check_direct_edit.sh DIR
# Source: tools/fixtures/step/direct_edit.py (block 60x40x20, D8 through hole, D6x8 blind
# hole, D10x5 boss); expected volumes are analytic. Checks: one valid solid, volume, bounds,
# and point probes on both sides of every edited face.
# Volume tolerance 5e-6 relative: OpenCascade's integration of these exports is off by
# ~1e-6 relative even for the UNEDITED re-export (direct_unedited.step, 47161.2325 vs
# 48000 - 267 pi = 47161.1948): the writer gives planar faces no pcurves, and the pcurves
# OCC builds for their circular edges are approximations (its disk areas are off by ~1e-4
# relative while cylinder areas are exact). The probes decide the edits; our own exact
# volumes are asserted to 1e-6 relative in the kernel tests.
set -u -o pipefail
DIR=$(realpath "${1:?usage: check_direct_edit.sh DIR}")
REPO=$(cd "$(dirname "$0")/.." && pwd)
if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: direct-edit STEP validation NOT RUN"; exit 2
fi
status=0
check() { # file, expected volume (python expr), bounds (python list), probes JSON, expected inside
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
tol = exp_v * 5e-6; good = abs(g - exp_v) <= tol; ok = ok and good
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
V0="48000 - 267*pi"; TOP="(2400 - 50*pi)"
# Baseline: the imported fixture re-exported unedited.
check direct_unedited.step "$V0" "[0,0,0,60,40,25]" \
  "[[15,20,10],[30,12,15],[30,12,11.9],[45,25,24.9],[45,25,25.1]]" "[False, False, True, True, False]"
check direct_offset_top_up3.step "$V0 + 3*$TOP" "[0,0,0,60,40,25]" \
  "[[10,5,22.9],[10,5,23.1],[15,20,22],[45,25,24.9],[30,12,15],[30,12,11.9]]" \
  "[True, False, False, True, False, True]"
check direct_resize_through_d12.step "$V0 - pi*(36-16)*20" "[0,0,0,60,40,25]" \
  "[[20.9,20,10],[21.1,20,10],[15,25.9,1],[15,26.1,1]]" "[False, True, False, True]"
check direct_remove_boss.step "$V0 - pi*25*5" "[0,0,0,60,40,20]" \
  "[[45,25,19.9],[45,25,20.1],[45,25,24]]" "[True, False, False]"
# offset top +3, through hole to D10 (23 deep), blind hole removed (11 deep after the offset).
check direct_composed.step "$V0 + 3*$TOP - pi*(25-16)*23 + pi*9*11" "[0,0,0,60,40,25]" \
  "[[19.9,20,10],[20.1,20,10],[30,12,15],[30,12,22.9],[10,5,22.9],[15,20,22.9]]" \
  "[False, True, True, True, True, False]"
# Through the command API (crates/rcad-commands/tests/direct_edit.rs, RCAD_EVIDENCE_DIR):
# top +4 (bound parameter "lift"), through hole D12 (24 deep), blind hole removed (12 deep).
if [ -f "$DIR/direct_cmd_lift4.step" ]; then
  check direct_cmd_lift4.step "$V0 + 4*$TOP - pi*20*24 + pi*9*12" "[0,0,0,60,40,25]" \
    "[[20.9,20,10],[21.1,20,10],[30,12,15],[10,5,23.9],[10,5,24.1]]" \
    "[False, True, True, True, False]"
fi
# Countersink (kernel direct::tests, RCAD_EVIDENCE_DIR): D16 x 90 deg on the D8 through hole
# at (15,20) from the top face: cone 4 deep, removes 256 pi / 3.
if [ -f "$DIR/csink_block_d16.step" ]; then
  check csink_block_d16.step "$V0 - 256*pi/3" "[0,0,0,60,40,25]" \
    "[[21.8,20,19],[22.2,20,19],[18.9,20,10],[19.1,20,10],[15,27.9,19.95]]" \
    "[False, True, False, True, False]"
fi
exit $status
