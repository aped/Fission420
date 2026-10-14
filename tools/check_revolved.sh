#!/usr/bin/env bash
# Independent FreeCAD (OpenCascade) validation of revolved Join/Cut results written by
#   RCAD_EVIDENCE_DIR=DIR cargo test --release -p rcad-kernel --test revolved_booleans
# usage: tools/heavy.sh tools/check_revolved.sh DIR
# Two independent checks per file:
# 1. FreeCAD reads one valid solid; bounds (1e-6 mm) and inside/outside probes hand-derived from
#    the test geometry (crates/rcad-kernel/tests/revolved_booleans.rs); volume against the closed
#    form to 5e-5 relative. OpenCascade's integration of our exports is off by up to ~1e-5
#    relative because the writer gives planar faces no pcurves and OCC's own pcurves for their
#    circular edges are approximations (see tools/check_direct_edit.sh). Our exact volumes are
#    asserted to 1e-6 relative in the kernel test.
# 2. Symmetric difference against the solid FreeCAD builds from Part primitives (cylinders,
#    cones, boxes) must vanish (<= 0.05 mm^3); a control file compared with the wrong solid must
#    differ by the groove's 38 pi mm^3, so a silently failing comparison cannot pass.
set -u -o pipefail
DIR=$(realpath "${1:?usage: check_revolved.sh DIR}")
REPO=$(cd "$(dirname "$0")/.." && pwd)
if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: revolved STEP validation NOT RUN"; exit 2
fi
status=0
check() { # file, expected volume (python expr), bounds (python list), probes JSON, expected inside (python list)
  local f=$1 vol=$2 bounds=$3 probes=$4 inside=$5 out
  [ -f "$DIR/$f" ] || { echo "$f: missing"; status=1; return; }
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
tol = exp_v * 5e-5; good = abs(g - exp_v) <= tol; ok = ok and good
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
# Groove r 9..11, z 10..12 in a Ø20 x 30 cylinder, four seam angles.
for seam in 0 0.7853981633974483 3.141592653589793 1.234; do
  check "revolved_groove_seam_${seam}.step" "2962*pi" "[-10,-10,0,10,10,30]" \
    "[[9.5,0,11],[8.5,0,11],[9.5,0,9],[0,9.5,13],[-6.7175144,-6.7175144,11],[-6.7175144,-6.7175144,9]]" \
    "[False, True, True, True, False, True]"
done
# Collar r 8..14, z 10..14 joined to the same cylinder.
check revolved_collar.step "3384*pi" "[-14,-14,0,14,14,30]" \
  "[[12,0,12],[12,0,9],[0,-13,13],[15,0,12],[0,0,20]]" "[True, False, True, False, True]"
# Countersink: triangle (1,1), (1,-4.5), (6.5,1) revolved, cut from a 20 x 20 x 10 block (z -10..0);
# removed r in [1, z + 5.5] for z in [-4.5, 0].
check revolved_countersink.step "4000 - 50.625*pi" "[-10,-10,-10,10,10,0]" \
  "[[0,0,-1],[3,0,-0.5],[5.3,0,-0.5],[2,0,-4],[1.2,0,-4],[0,0,-9]]" \
  "[True, False, True, True, False, True]"
# Ring boss r 8..12, z 28..34 joined to a 40 x 40 x 30 block.
check revolved_boss.step "48000 + 320*pi" "[-20,-20,0,20,20,34]" \
  "[[10,0,32],[5,0,32],[13,0,32],[10,0,29],[0,10,33.9],[0,0,31]]" \
  "[True, False, False, True, True, False]"
expected() { # file, expected-shape python, min sym diff, max sym diff
  local f=$1 expect=$2 lo=$3 hi=$4 out
  [ -f "$DIR/$f" ] || { echo "$f: missing"; status=1; return; }
  out=$(timeout 300 flatpak run --env="RCAD_STEP=$DIR/$f" --env="RCAD_EXPECT=$expect" \
    --filesystem="$DIR" --filesystem="$REPO" --command=FreeCADCmd org.freecad.FreeCAD \
    "$REPO/tools/freecad_check_expected.py" 2>&1 | grep FREECAD_RESULT) || { echo "$f: FreeCAD failed"; status=1; return; }
  python3 - "$f" "$out" "$lo" "$hi" <<'PY' || status=1
import json, sys
f, out, lo, hi = sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4])
r = json.loads(out.split(" ", 1)[1])
d = r.get("sym_diff", float("nan"))
ok = bool(r.get("ok") and r.get("isValid") and r.get("solids") == 1 and lo <= d <= hi)
print(f"  {f}: sym_diff {d:.3e} (accept [{lo:g}, {hi:g}]) volume {r.get('volume')} expected-solid {r.get('expected_volume')}")
print(f"{f} vs expected solid: {'PASS' if ok else 'FAIL'}" + ("" if r.get("ok") else f" ({r.get('error')})"))
sys.exit(0 if ok else 1)
PY
}
TOL=0.05
CYL="c=Part.makeCylinder(10,30)"
RING="Part.makeCylinder(11,2,V(0,0,10)).cut(Part.makeCylinder(9,2,V(0,0,10)))"
for seam in 0 0.7853981633974483 3.141592653589793 1.234; do
  expected "revolved_groove_seam_${seam}.step" "$CYL; exp=c.cut($RING)" 0 $TOL
done
expected revolved_collar.step \
  "$CYL; exp=c.fuse(Part.makeCylinder(14,4,V(0,0,10)).cut(Part.makeCylinder(8,4,V(0,0,10)))).removeSplitter()" 0 $TOL
expected revolved_countersink.step \
  "b=Part.makeBox(20,20,10,V(-10,-10,-10)); t=Part.makeCone(1,6.5,5.5,V(0,0,-4.5)).cut(Part.makeCylinder(1,5.5,V(0,0,-4.5))); exp=b.cut(t)" 0 $TOL
expected revolved_boss.step \
  "b=Part.makeBox(40,40,30,V(-20,-20,0)); exp=b.fuse(Part.makeCylinder(12,6,V(0,0,28)).cut(Part.makeCylinder(8,6,V(0,0,28)))).removeSplitter()" 0 $TOL
# Control: a groove export against the plain cylinder differs by 38 pi = 119.38 mm^3.
expected revolved_groove_seam_0.step "$CYL; exp=c" 118 121
exit $status
