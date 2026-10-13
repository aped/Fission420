#!/usr/bin/env bash
# Independent FreeCAD (OpenCascade) check of the run-6 direct-edit and countersink STEP
# exports against expected solids FreeCAD builds itself from Part primitives (the same
# construction as tools/fixtures/step/direct_edit.py, with the edit applied analytically):
# the symmetric difference of the read solid and the expected one must vanish. A control
# case (expected solid without the countersink) must NOT vanish, so a silently failing
# boolean cannot pass.
# Files from: RCAD_EVIDENCE_DIR=DIR cargo test --release -p rcad-kernel --lib direct
#             RCAD_EVIDENCE_DIR=DIR cargo test --release -p rcad-commands --test direct_edit
# usage: tools/heavy.sh tools/check_direct_edit_expected.sh DIR
set -u -o pipefail
DIR=$(realpath "${1:?usage: check_direct_edit_expected.sh DIR}")
REPO=$(cd "$(dirname "$0")/.." && pwd)
if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: direct-edit expected-solid check NOT RUN"; exit 2
fi
status=0
check() { # file, expected-shape python, min sym diff, max sym diff
  local f=$1 expect=$2 lo=$3 hi=$4 out
  if [ ! -f "$DIR/$f" ]; then echo "$f: missing (not generated)"; status=1; return; fi
  out=$(timeout 300 flatpak run --env="RCAD_STEP=$DIR/$f" --env="RCAD_EXPECT=$expect" \
    --filesystem="$DIR" --filesystem="$REPO" --command=FreeCADCmd org.freecad.FreeCAD \
    "$REPO/tools/freecad_check_expected.py" 2>&1 | grep FREECAD_RESULT) || { echo "$f: FreeCAD failed"; status=1; return; }
  python3 - "$f" "$out" "$lo" "$hi" <<'PY' || status=1
import json, sys
f, out, lo, hi = sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4])
r = json.loads(out.split(" ", 1)[1])
d = r.get("sym_diff", float("nan"))
ok = bool(r.get("ok") and r.get("isValid") and r.get("solids") == 1 and lo <= d <= hi)
print(f"  {f}: valid={r.get('isValid')} solids={r.get('solids')} sym_diff {d:.3e} (accept [{lo:g}, {hi:g}]) "
      f"volume {r.get('volume')} expected-solid volume {r.get('expected_volume')}")
print(f"{f}: {'PASS' if ok else 'FAIL'}" + ("" if r.get("ok") else f" ({r.get('error')})"))
sys.exit(0 if ok else 1)
PY
}
# Fixture pieces: block of height H, through hole radius RT, blind hole (r 3, floor z 12,
# up to the top) or none, boss r 5 from z 19 to 25.
blk() { # H RT BLIND(0/1) BOSS(0/1)
  local s="b=Part.makeBox(60,40,$1).cut(Part.makeCylinder($2,40,V(15,20,-5)))"
  [ "$3" = 1 ] && s="$s.cut(Part.makeCylinder(3,30,V(30,12,12)))"
  [ "$4" = 1 ] && s="$s.fuse(Part.makeCylinder(5,6,V(45,25,19)))"
  echo "$s.removeSplitter(); exp=b"
}
TOL=0.05   # mm^3: far below any edit tested (smallest analytic change here: 85 mm^3)
check direct_unedited.step "$(blk 20 4 1 1)" 0 $TOL
check direct_offset_top_up3.step "$(blk 23 4 1 1)" 0 $TOL
check direct_resize_through_d12.step "$(blk 20 6 1 1)" 0 $TOL
check direct_remove_boss.step "$(blk 20 4 1 0)" 0 $TOL
check direct_composed.step "$(blk 23 5 0 1)" 0 $TOL
check direct_cmd_lift4.step "$(blk 24 6 0 1)" 0 $TOL
CONE="; exp=b.cut(Part.makeCone(4,9,5,V(15,20,16)))"
check csink_block_d16.step "$(blk 20 4 1 1)$CONE" 0 $TOL
# Countersink feature through the command API (rcad-commands tests/countersink.rs): plate
# 60x40x10, D8 hole at (30,20), D12 x 82 deg countersink from the top.
check csink_plate_d8_d12_82.step "h=2/math.tan(math.radians(41)); b=Part.makeBox(60,40,10).cut(Part.makeCylinder(4,20,V(30,20,-5))); exp=b.cut(Part.makeCone(4,4+(h+1)*2/h,h+1,V(30,20,10-h)))" 0 $TOL
# Tapered tools (kernel taper::tests, RCAD_EVIDENCE_DIR=DIR): block 40x30x10; a 10x10
# pocket at (20,15) 5 deep drafted 10 deg (side 10 - 2 w tan10 at depth w), and a 10x10
# boss on top 5 tall drafted 5 deg. FreeCAD lofts the expected frusta (extended past the
# faces along the same draft so no faces coincide).
SQ="def sq(z,s,c=(20,15)): return Part.makePolygon([V(c[0]-s/2,c[1]-s/2,z),V(c[0]+s/2,c[1]-s/2,z),V(c[0]+s/2,c[1]+s/2,z),V(c[0]-s/2,c[1]+s/2,z),V(c[0]-s/2,c[1]-s/2,z)])
b=Part.makeBox(40,30,10)"
if [ -f "$DIR/taper_cut_pocket.step" ]; then
  check taper_cut_pocket.step "$SQ
t=math.tan(math.radians(10)); exp=b.cut(Part.makeLoft([sq(5,10-10*t),sq(11,10+2*t)],True))" 0 $TOL
  check taper_join_boss.step "$SQ
t=math.tan(math.radians(5)); exp=b.fuse(Part.makeLoft([sq(9.5,10+t),sq(15,10-10*t)],True)).removeSplitter()" 0 $TOL
fi
# Replace face (kernel direct::tests): Truck block 40x30x10 whose top is tilted 20 deg about
# the centroid's X axis and moved 2 mm along the tilted normal: z_top(y) = 10 + 2/cos20 +
# (y - 15) tan20; FreeCAD extrudes that trapezoid along X.
if [ -f "$DIR/replace_top_tilt20_up2.step" ]; then
  check replace_top_tilt20_up2.step "a=math.radians(20); zt=lambda y: 10+2/math.cos(a)+(y-15)*math.tan(a)
exp=Part.Face(Part.makePolygon([V(0,0,0),V(0,30,0),V(0,30,zt(30)),V(0,0,zt(0)),V(0,0,0)])).extrude(V(40,0,0))" 0 $TOL
fi
# Control: the countersunk export against the block WITHOUT the countersink must differ
# by the countersink's volume, 256 pi / 3 = 268.08 mm^3.
check csink_block_d16.step "$(blk 20 4 1 1)" 266 270
exit $status
