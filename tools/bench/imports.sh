#!/usr/bin/env bash
# Wall-time of representative imports/exports through the release CLI (one fresh process per
# file; includes regeneration and measurement). Prints a table on stdout.
# usage: tools/bench/imports.sh
set -u -o pipefail
REPO=$(cd "$(dirname "$0")/../.." && pwd)
OUT="$REPO/artifacts/run3/bench"
mkdir -p "$OUT"
cargo build -q --release -p f420-cli || exit 1
CLI="$REPO/target/release/f420-cli"
F="$REPO/crates"
run() { # label, json step list
  local label=$1 json=$2
  echo "$json" > "$OUT/$label.json"
  "$CLI" run "$OUT/$label.json" > "$OUT/$label.out" || { echo "$label: FAILED"; return; }
  python3 - "$label" "$OUT/$label.out" <<'PY'
import json, sys
label, path = sys.argv[1], sys.argv[2]
for line in open(path):
    r = json.loads(line)["response"]
    if r["command"] in ("new_document",):
        continue
    print(f"{label:28s} {r['command']:16s} {r['elapsed_ms']:9.1f} ms  ok={r['ok']}")
PY
}
S="$F/f420-kernel/tests/fixtures/step"
run step_box '[{"cmd":"new_document"},{"cmd":"import_step","path":"'"$S"'/box_40x30x10_mm.step"},{"cmd":"export_step","path":"'"$OUT"'/box.step"}]'
run step_plate_hole '[{"cmd":"new_document"},{"cmd":"import_step","path":"'"$S"'/plate_hole_mm.step"}]'
run step_torus '[{"cmd":"new_document"},{"cmd":"import_step","path":"'"$S"'/torus_R20_r5_mm.step"}]'
run step_two_box_assembly '[{"cmd":"new_document"},{"cmd":"import_step","path":"'"$S"'/two_boxes_assembly_mm.step"}]'
run stl_binary '[{"cmd":"new_document"},{"cmd":"import_stl","path":"'"$F"'/f420-exchange/tests/fixtures/stl/box_freecad_binary.stl","units_to_mm":1}]'
run dxf_slot '[{"cmd":"new_document"},{"cmd":"import_dxf","path":"'"$F"'/f420-exchange/tests/fixtures/dxf/slot_freecad_r14.dxf","plane":"WorldXY","units_to_mm":1}]'
run svg_plate '[{"cmd":"new_document"},{"cmd":"import_svg","path":"'"$F"'/f420-exchange/tests/fixtures/svg/freecad_plate.svg","plane":"WorldXY"},{"cmd":"extrude","sketch":1,"distance":5}]'
run threemf_two_boxes '[{"cmd":"new_document"},{"cmd":"import_3mf","path":"'"$F"'/f420-exchange/tests/fixtures/threemf/freecad_two_boxes.3mf"},{"cmd":"export_3mf","path":"'"$OUT"'/two.3mf"}]'
# A larger model: a 200x100x5 plate with a row of 19 Ø4 holes (seed + linear pattern count 19), exported as STL, 3MF and STEP.
run large_pattern '[{"cmd":"new_document"},{"cmd":"create_sketch","plane":"WorldXY","template":"polygon","vertices":[[0,0],[200,0],[200,100],[0,100]]},{"cmd":"extrude","sketch":1,"distance":5},{"cmd":"create_sketch","plane":{"Face":{"feature":2,"role":"ExtrudeEndCap"}},"template":"centered_circle","diameter":4,"center":[10,10]},{"cmd":"extrude","sketch":3,"op":"cut","through_all":true},{"cmd":"linear_pattern","feature":4,"count":19,"spacing":10,"direction":[1,0]},{"cmd":"export_stl","path":"'"$OUT"'/large.stl"},{"cmd":"export_3mf","path":"'"$OUT"'/large.3mf"},{"cmd":"export_step","path":"'"$OUT"'/large.step"}]'
