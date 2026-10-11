#!/usr/bin/env bash
# Independent validation of the SESSION_3 scenario C/D exports (written by
# `f420-cli acceptance`, crates/f420-commands/src/acceptance/formats.rs) with FreeCAD 1.1
# (OpenCascade) and a pure-Python STL reader -- no Fission420 reader is involved.
# usage: tools/check_formats.sh [DIR]   (default artifacts/acceptance)
# Exit 0 = all PASS, 1 = a check failed, 2 = FreeCAD not installed (FreeCAD checks NOT RUN).
#
# Expected values are analytic (computed here, not read from Fission420 output):
#   scenario_c.step     FreeCAD box 40x30x10 imported, centred Ø6 through hole cut from its
#                       imported top face: 1 valid solid, V = 12000 - 90*pi,
#                       bbox [0,40]x[0,30]x[0,10], hole axis x=20,y=15 parallel to Z
#                       (circular inner loops of area 9*pi at z=0 and z=10 centred on it;
#                       point (20,15,5) outside the material, (23.01,15,5) inside).
#   scenario_d_slot.step  FreeCAD DXF slot (3 lines + arc r10 at (30,10)) with its left edge
#                       moved to x=-10, extruded 5: V = 5*(800 + 50*pi),
#                       bbox [-10,40]x[0,20]x[0,5].
#   scenario_d_slot.dxf  the edited sketch exported: 3 lines + 1 arc (30,10) r10 180 deg,
#                       closed, area 800 + 50*pi, bounds [-10,0]-[40,20].
#   scenario_d_mesh.stl  the FreeCAD 20x10x5 box STL re-exported as a mesh body: 12 facets,
#                       closed, V = 1000.
# Tolerances: volume max(0.001, V*1e-7) mm^3 (GOAL.md); bounds 1e-6 mm (OCC's
# optimalBoundingBox adds the 1e-7 shape tolerance); loop centre 1e-6 mm; loop area 1e-6 rel.
set -u -o pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
DIR=$(realpath "${1:-$REPO/artifacts/acceptance}")
status=0

echo "== scenario_d_mesh.stl: python STL check (expected volume 1000)"
if [ -f "$DIR/scenario_d_mesh.stl" ]; then
  python3 "$REPO/tools/check_stl.py" "$DIR/scenario_d_mesh.stl" 1000 \
    | tee "$DIR/scenario_d_mesh_stl_check_python.json" || status=1
else
  echo "MISSING $DIR/scenario_d_mesh.stl"; status=1
fi

if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: FreeCAD checks of scenario C/D NOT RUN"
  exit 2
fi
fc() { # env assignments... script
  local args=()
  while [ $# -gt 1 ]; do args+=("--env=$1"); shift; done
  timeout 300 flatpak run "${args[@]}" --filesystem="$REPO" --filesystem="$DIR" \
    --command=FreeCADCmd org.freecad.FreeCAD "$1" 2>&1 | grep -o 'FREECAD_RESULT .*'
}

check_step() { # stem, python expected-dict expression
  local stem=$1 expect=$2 probes=${3:-[]} out
  echo "== $stem.step: FreeCAD (OpenCascade) solid check"
  if [ ! -f "$DIR/$stem.step" ]; then echo "MISSING $DIR/$stem.step"; status=1; return; fi
  out=$(fc "F420_STEP=$DIR/$stem.step" "F420_PROBES=$probes" "$REPO/tools/freecad_check_step_detail.py") \
    || { echo "FreeCAD run failed"; status=1; return; }
  echo "$out" | tee "$DIR/${stem}_step_check_freecad.txt"
  python3 - "$out" "$expect" <<'PY' || status=1
import json, math, sys
pi = math.pi
r = json.loads(sys.argv[1].split(" ", 1)[1])
e = eval(sys.argv[2], {"pi": pi, "math": math})
errs = []
if not (r.get("ok") and r.get("isValid")):
    errs.append(f"not a valid shape: {r.get('error')}")
if r.get("solids") != 1:
    errs.append(f"solids {r.get('solids')} != 1")
v = r.get("volume", 0.0)
tol = max(0.001, e["volume"] * 1e-7)
if abs(v - e["volume"]) > tol:
    errs.append(f"volume {v} vs {e['volume']} (err {v - e['volume']:.3e} > {tol:.1e})")
b = r.get("bounds", [])
if len(b) != 6 or max(abs(x - y) for x, y in zip(b, e["bounds"])) > 1e-6:
    errs.append(f"bounds {b} != {e['bounds']}")
if "hole" in e:
    cx, cy, d, zs = e["hole"]
    loops = r.get("z_inner_loops", [])
    area = pi * d * d / 4
    if [round(l["z"], 6) for l in loops] != zs:
        errs.append(f"inner loops at z {[l['z'] for l in loops]} != {zs}")
    for l in loops:
        if abs(l["cx"] - cx) > 1e-6 or abs(l["cy"] - cy) > 1e-6:
            errs.append(f"hole centre ({l['cx']}, {l['cy']}) != ({cx}, {cy})")
        if abs(l["area"] - area) > 1e-6 * area:
            errs.append(f"hole area {l['area']} != {area}")
for x, y, z, inside in r.get("probes", []):
    want = e["probes"][(x, y, z)]
    if inside != want:
        errs.append(f"point ({x},{y},{z}) inside={inside}, expected {want}")
print(f"  volume err {v - e['volume']:.3e} mm^3 (tol {tol:.1e}); bounds {b}")
print(("PASS" if not errs else "FAIL: " + "; ".join(errs)))
sys.exit(1 if errs else 0)
PY
}

check_step scenario_c \
  '{"volume": 12000 - 90*pi, "bounds": [0,0,0,40,30,10], "hole": (20, 15, 6, [0.0, 10.0]),
    "probes": {(20,15,5): False, (23.01,15,5): True, (22.99,15,5): False, (20,15,10.01): False}}' \
  '[[20,15,5],[23.01,15,5],[22.99,15,5],[20,15,10.01]]'
check_step scenario_d_slot \
  '{"volume": 5*(800 + 50*pi), "bounds": [-10,0,0,40,20,5],
    "probes": {(39.9,10,2.5): True, (39.9,1,2.5): False, (-9.9,0.1,4.9): True}}' \
  '[[39.9,10,2.5],[39.9,1,2.5],[-9.9,0.1,4.9]]'

echo "== scenario_d_mesh.stl: FreeCAD Mesh check"
if [ -f "$DIR/scenario_d_mesh.stl" ]; then
  out=$(fc "F420_STL=$DIR/scenario_d_mesh.stl" "$REPO/tools/freecad_check_stl.py") || status=1
  echo "$out" | tee "$DIR/scenario_d_mesh_stl_check_freecad.txt"
  python3 - "$out" <<'PY' || status=1
import json, sys
r = json.loads(sys.argv[1].split(" ", 1)[1])
ok = (r["facets"] == 12 and r["points"] == 8 and r["isSolid"] and not r["hasNonManifolds"]
      and abs(r["volume"] - 1000) <= 0.001
      and max(abs(a - b) for a, b in zip(r["bounds"], [0, 0, 0, 20, 10, 5])) <= 1e-6)
print("PASS" if ok else f"FAIL {r}")
sys.exit(0 if ok else 1)
PY
fi

echo "== scenario_d_slot.dxf: FreeCAD DXF importer"
if [ -f "$DIR/scenario_d_slot.dxf" ]; then
  out=$(fc "F420_DXF=$DIR/scenario_d_slot.dxf" "$REPO/tools/fixtures/dxf/freecad_read_dxf.py") || status=1
  echo "$out" | tee "$DIR/scenario_d_slot_dxf_check_freecad.txt"
  python3 - "$out" <<'PY' || status=1
import json, math, sys
r = json.loads(sys.argv[1].split(" ", 1)[1])
errs = []
if r.get("edges") != {"Line": 3, "Circle": 1}:
    errs.append(f"edges {r.get('edges')}")
area = 800 + 50 * math.pi
if not r.get("closed") or abs((r.get("area") or 0) - area) > 1e-6:
    errs.append(f"closed {r.get('closed')} area {r.get('area')} != {area}")
if max(abs(a - b) for a, b in zip(r.get("bounds", [9e9] * 4), [-10, 0, 40, 20])) > 1e-6:
    errs.append(f"bounds {r.get('bounds')}")
arcs = r.get("arcs", [])
if len(arcs) != 1 or max(abs(a - b) for a, b in zip(arcs[0], [30, 10, 10, 180])) > 1e-6:
    errs.append(f"arcs {arcs}")
print("PASS" if not errs else "FAIL: " + "; ".join(errs))
sys.exit(1 if errs else 0)
PY
fi
exit $status
