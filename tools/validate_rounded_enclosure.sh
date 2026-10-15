#!/usr/bin/env bash
# External validation of the rounded labelled enclosure acceptance test:
#   1. the acceptance test writes the native document, workflow, expected values and
#      STEP/STL/3MF exports (crates/rcad-commands/tests/it/rounded_enclosure.rs);
#   2. FreeCAD reads the STEP: 2 valid solids at the analytic volumes;
#   3. the independent Python mesh checker (tools/print/check_print_mesh.py) with a spec
#      derived from the expected values (volume tolerance = curved area x export chord);
#   4. Bambu Studio headless slice of the placed 3MF (P1S, no printer contact) and the
#      G-code/log analyser.
# Usage (repository root): tools/validate_rounded_enclosure.sh <outdir>
# Heavy steps go through tools/heavy.sh. Exit 1 if any step fails; each step still runs.
set -uo pipefail
OUT=${1:?usage: validate_rounded_enclosure.sh <outdir>}
mkdir -p "$OUT"; OUT=$(readlink -f "$OUT"); ROOT=$(pwd); FAIL=0
mark() { if [ "$1" -ne 0 ]; then FAIL=1; echo "   -> FAIL (exit $1)"; else echo "   -> ok"; fi; }
echo "== 1. acceptance test + exports"
RCAD_EVIDENCE_DIR=$OUT tools/heavy.sh cargo test -p rcad-commands --release \
  --test it rounded_enclosure:: > "$OUT/test.log" 2>&1; mark $?
echo "== 2. FreeCAD STEP"
tools/heavy.sh flatpak run --env=RCAD_STEP="$OUT/rounded_enclosure.step" --env=RCAD_EXPECT_SOLIDS=2 \
  --filesystem="$ROOT" --filesystem="$OUT" --command=FreeCADCmd org.freecad.FreeCAD \
  "$ROOT/tools/freecad_check_step.py" 2>&1 | grep FREECAD_RESULT > "$OUT/freecad_step.log"
python3 -I - "$OUT" <<'PY'; mark $?
import json, sys
out = sys.argv[1]
r = json.loads(open(out + "/freecad_step.log").read().split("FREECAD_RESULT ", 1)[1])
x = json.load(open(out + "/rounded_enclosure_expected.json"))
want = sorted([x["base_volume"], x["lid_volume"]])
ok = r["ok"] and r["isValid"] and r["solids"] == 2 and all(
    abs(a - b) <= 1e-4 * b for a, b in zip(r["solid_volumes"], want))
print("   FreeCAD:", r["solids"], "solids, valid", r["isValid"], "volumes", r["solid_volumes"], "want", want)
sys.exit(0 if ok else 1)
PY
echo "== 3. mesh checker"
python3 -I - "$OUT" <<'PY'
import json, math, sys
out = sys.argv[1]
x = json.load(open(out + "/rounded_enclosure_expected.json"))
chord, r, h, t = 0.01, x["corner_r"], x["height"], x["wall"]
f = 2.4
tb = (2*math.pi*r*h + 2*math.pi*(r-t)*(h-f) + 4*2*math.pi*3.5*(h-f) + 4*2*math.pi*1.25*8) * chord
tl = (2*math.pi*r*2.4 + 4*2*math.pi*3.0*1.2 + 4*2*math.pi*1.6*1.2) * chord
po = {"Base": {"volume": [x["base_volume"], tb]}, "Lid": {"volume": [x["lid_volume"], tl]}}
placed = {"Base": {"volume": [x["base_volume"], tb], "dims": [[x["length"], x["width"], h], 1e-4]},
          "Lid": {"volume": [x["lid_volume"], tl]}}
spec = {"files": {
  "rounded_enclosure.3mf": {"unit": "millimeter", "objects": 2, "names": ["Base", "Lid"], "closed": True, "per_object": po},
  "rounded_enclosure_placed.3mf": {"unit": "millimeter", "objects": 2, "names": ["Base", "Lid"], "closed": True,
     "min_z": [0, 1e-4], "within_bed": [[0, 0], [256, 256], 250], "per_object": placed},
  "rounded_enclosure.stl": {"closed": True}}}
json.dump(spec, open(out + "/spec.json", "w"), indent=1)
PY
python3 -I tools/print/check_print_mesh.py "$OUT/rounded_enclosure.3mf" "$OUT/rounded_enclosure_placed.3mf" \
  "$OUT/rounded_enclosure.stl" --spec "$OUT/spec.json" --json "$OUT/mesh_check.json" > "$OUT/mesh_check.txt" 2>&1
rc=$?; tail -1 "$OUT/mesh_check.txt"; mark $rc
echo "== 4. Bambu Studio slice (sliced, not printed)"
tools/print/slice_bambu.sh "$OUT/slice" "$OUT/rounded_enclosure_placed.3mf" > "$OUT/slice_run.log" 2>&1; mark $?
python3 -I tools/print/analyze_gcode.py "$OUT/slice/plate_1.gcode" --log "$OUT/slice/slice.log" \
  --layers --out "$OUT/slice/analysis.json" > "$OUT/slice/analysis.txt" 2>&1
rc=$?; tail -1 "$OUT/slice/analysis.txt"; mark $rc
[ $FAIL -eq 0 ] && echo "ALL OK" || echo "SOME STEPS FAILED"
exit $FAIL
