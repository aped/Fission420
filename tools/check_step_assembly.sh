#!/usr/bin/env bash
# Independent check of the hierarchical STEP assembly export (export_assembly
# format "step_assembly") with FreeCAD 1.1 flatpak (OpenCascade), not the
# Rust writer/reader.
# usage: tools/check_step_assembly.sh
# Replays examples/hinge_assembly_workflow.json (hinge at 90 deg; screw height 8),
# which writes artifacts/hinge_assembly_tree.step, then requires (analytic values
# computed here, not read from R-CAD output):
#   * 7 valid solids; volumes LeafA 40*30*4 - pi*2^2*4, LeafB 4800, pin 2.5*2.5*10,
#     4 screws 4*4*8;
#   * exact centres: pin (-5,0,5); Wing screws rotated 90 deg about the axis
#     through (-5,0): (-5,25,12), (-13,35,12), (3,35,12); leaf B (-5,25,6);
#     independent screw copy (20,40,4);
#   * the product tree FreeCAD builds: App::Part "Wing" under "Assembly" with
#     LeafB and three Screw parts; LeafA, Pin and "Screw copy" directly under
#     "Assembly"; Wing placed at (-5, 5, 0).
set -u -o pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
DIR="$REPO/artifacts/step_assembly"
mkdir -p "$DIR"
(cd "$REPO" && cargo run --release -q -p rcad-cli -- run examples/hinge_assembly_workflow.json >"$DIR/replay.log") || exit 1
cp "$REPO/artifacts/hinge_assembly_tree.step" "$DIR/hinge_tree.step" || exit 1
if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: external STEP assembly validation NOT RUN"
  exit 2
fi
out=$(timeout 300 flatpak run --env="RCAD_STEP=$DIR/hinge_tree.step" \
  --filesystem="$DIR" --filesystem="$REPO/tools:ro" --command=FreeCADCmd org.freecad.FreeCAD \
  "$REPO/tools/freecad_check_step_assembly.py" 2>&1 | grep FREECAD_RESULT) || exit 1
echo "$out" > "$DIR/hinge_tree_freecad.txt"
python3 - "$out" <<'PY'
import json, math, sys
r = json.loads(sys.argv[1].split(" ", 1)[1])
ok = bool(r.get("ok"))
sh = r.get("shape", {})
exp_v = sorted([4800 - 16 * math.pi, 4800.0, 62.5] + [128.0] * 4)
got_v = sh.get("solid_volumes", [])
ok &= sh.get("solids") == 7 and sh.get("isValid") is True and len(got_v) == 7
for g, e in zip(got_v, exp_v):
    good = abs(g - e) <= max(1e-3, e * 1e-7)
    ok &= good
    print(f"  volume {g:.6f} vs {e:.6f}: {'ok' if good else 'BAD'}")
centres = [tuple(c) for c in sh.get("solid_centres", [])]
for want in [(-5, 0, 5), (-5, 25, 12), (-13, 35, 12), (3, 35, 12), (-5, 25, 6), (20, 40, 4)]:
    hit = any(all(abs(c[i] - want[i]) <= 1e-6 for i in range(3)) for c in centres)
    ok &= hit
    print(f"  centre {want}: {'found' if hit else 'MISSING'}")
tree = r.get("tree", [])
def under(label_prefix, parent):
    return [o for o in tree if o["label"].startswith(label_prefix) and parent in o["parents"]]
checks = {
    "Wing is an App::Part under Assembly": any(o["type"] == "App::Part" and o["label"] == "Wing" and "Assembly" in o["parents"] for o in tree),
    "LeafB under Wing": len(under("LeafB", "Wing")) == 1,
    "3 Screw under Wing": len([o for o in under("Screw", "Wing") if not o["label"].startswith("Screw copy")]) == 3,
    "LeafA, Pin, Screw copy under Assembly": all(len(under(n, "Assembly")) == 1 for n in ["LeafA", "Pin", "Screw copy"]),
    "Wing placed at (-5, 5, 0)": any(o["label"] == "Wing" and all(abs(a - b) <= 1e-6 for a, b in zip(o.get("global_base", []), [-5, 5, 0])) for o in tree),
}
for k, v in checks.items():
    ok &= v
    print(f"  {k}: {'ok' if v else 'FAIL'}")
print(f"hierarchical STEP via OpenCascade/FreeCAD: parts={r.get('parts')} -> {'PASS' if ok else 'FAIL'}")
sys.exit(0 if ok else 1)
PY
