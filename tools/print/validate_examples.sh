#!/usr/bin/env bash
# Print-export validation of the REAL printable examples (rcad_commands::printables):
# enclosure baseline + variant (base + lid), clearance coupon (plate + peg), mounting bracket.
#   1. crates/rcad-commands/tests/print_examples.rs (ignored test) replays the example workflows
#      and writes placed per-body STLs, multi-object 3MFs, one STEP per body, and analytic
#      expectations (spec.json for the mesh checker, step_expected.json for FreeCAD);
#   2. tools/print/check_print_mesh.py (independent, stdlib Python) against spec.json;
#   3. FreeCAD 1.1.4: every STEP (one valid solid, exact volume, bounds) and every STL
#      (solid, no non-manifolds, no self-intersections);
#   4. Bambu Studio 2.8.2.61 headless slices (P1S 0.4 / 0.20mm Standard / Generic PLA) with
#      toolpath probes (tools/print/probes/*.json).
# Usage (repository root): tools/print/validate_examples.sh [outdir]
#   default outdir artifacts/run4/print/examples; logs <outdir>/*.log, summary <outdir>/SUMMARY.txt.
# All cargo, FreeCAD and slicer runs go through tools/heavy.sh; cargo uses
# CARGO_TARGET_DIR (default <main checkout>/target-print).
# Exit 1 if any step fails (every step still runs).
set -uo pipefail
ROOT=$(pwd)
OUT=${1:-artifacts/run4/print/examples}
mkdir -p "$OUT"
OUT=$(readlink -f "$OUT")
EXP=$OUT/out
MAIN="$(cd "$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")" && pwd)"
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$MAIN/target-print}
FAIL=0
SUM=$OUT/SUMMARY.txt
: > "$SUM"
note() { echo "$*" | tee -a "$SUM"; }
res() { if [ "$2" -ne 0 ]; then FAIL=1; note "  $1: FAIL (exit $2)"; else note "  $1: ok"; fi; }

note "# validate_examples.sh $(date -Is)  commit $(git rev-parse --short HEAD 2>/dev/null)"
note "## 1. exports (cargo test --test print_examples -- --ignored)"
rm -rf "$EXP"; mkdir -p "$EXP"
RCAD_PRINT_EXAMPLES_OUT=$EXP tools/heavy.sh cargo test -p rcad-commands --release \
  --test print_examples -- --ignored > "$OUT/export.log" 2>&1
res "export test" $?
ls "$EXP" | sed 's/^/    /' >> "$SUM"

note "## 2. independent mesh checks (tools/print/check_print_mesh.py)"
python3 -I tools/print/check_print_mesh.py "$EXP"/*.stl "$EXP"/*.3mf --spec "$EXP/spec.json" \
  --json "$OUT/mesh_check.json" > "$OUT/mesh_check.log" 2>&1
rc=$?
grep -E '^SUMMARY' "$OUT/mesh_check.log" | sed 's/^/  /' | tee -a "$SUM"
python3 -I - "$OUT/mesh_check.json" >> "$SUM" <<'EOF'
import json, sys
d = json.load(open(sys.argv[1]))
for f in d["files"]:
    for c in f["checks"]:
        k = c["check"]
        if c["status"] != "PASS" or k.endswith((" volume", " dims")) or k in ("file volume", "file dims", "unit", "objects"):
            print("    %-4s %-36s %s: %s" % (c["status"], f["basename"][:36], k[:60], c["detail"][:110]))
EOF
res "mesh checks" $rc

note "## 3. FreeCAD (STEP exact volume/validity, STL solidity/self-intersection)"
: > "$OUT/freecad.log"
for f in "$EXP"/*.step; do
  tools/heavy.sh timeout 300 flatpak run --env=RCAD_STEP="$f" --env=RCAD_EXPECT_SOLIDS=1 \
    --filesystem="$ROOT" --command=FreeCADCmd org.freecad.FreeCAD "$ROOT/tools/freecad_check_step.py" 2>&1 \
    | grep FREECAD_RESULT | sed "s|^|STEP $(basename "$f") |" >> "$OUT/freecad.log"
done
for f in "$EXP"/*.stl; do
  tools/heavy.sh timeout 300 flatpak run --env=RCAD_PRINT="$f" --filesystem="$ROOT" \
    --command=FreeCADCmd org.freecad.FreeCAD "$ROOT/tools/print/freecad_check_print.py" 2>&1 \
    | grep FREECAD_RESULT | sed "s|^|STL $(basename "$f") |" >> "$OUT/freecad.log"
done
python3 -I - "$OUT/freecad.log" "$EXP/step_expected.json" >> "$SUM" <<'EOF'
import json, sys
exp = json.load(open(sys.argv[2]))
bad, seen = 0, set()
for line in open(sys.argv[1]):
    kind, name, _, payload = line.split(" ", 3)
    r = json.loads(payload)
    if kind == "STEP":
        seen.add(name)
        e = exp.get(name)
        ok = r.get("ok") and r.get("isValid") and r.get("solids") == 1 and e is not None
        dv = r.get("volume", 0) - (e or {}).get("volume", 0)
        rel = abs(dv) / e["volume"] if e else 1
        db = max(abs(a - b) for a, b in zip(r.get("bounds", [0] * 6), (e or {}).get("bounds", [0] * 6)))
        ok = ok and rel <= 1e-6 and db <= 1e-4
        bad += not ok
        print("    %-4s STEP %-30s solids=%s valid=%s V=%.6f expected %.6f (rel %.1e) bounds dev %.1e" % (
            "PASS" if ok else "FAIL", name, r.get("solids"), r.get("isValid"), r.get("volume", 0),
            (e or {}).get("volume", 0), rel, db))
    else:
        for o in r.get("objects", []):
            ok = o["isSolid"] and not o["hasNonManifolds"] and not o["hasSelfIntersections"] and o["volume"] > 0
            bad += not ok
            print("    %-4s STL  %-55s solid=%s nonmanifold=%s selfintersect=%s V=%.4f" % (
                "PASS" if ok else "FAIL", name[:55], o["isSolid"], o["hasNonManifolds"],
                o["hasSelfIntersections"], o["volume"]))
        if "error" in r or not r.get("objects"):
            bad += 1; print("    FAIL STL %s %s" % (name, r.get("error")))
missing = set(exp) - seen
for m in sorted(missing):
    bad += 1; print("    FAIL STEP %s: no FreeCAD result" % m)
sys.exit(1 if bad else 0)
EOF
res "FreeCAD" $?

note "## 4. Bambu Studio slicing + toolpath probes"
slice() { # name probes models...
  local name=$1 probes=$2; shift 2
  local d=$OUT/slice_$name
  tools/print/slice_bambu.sh "$d" "$@" > "$OUT/slice_$name.log" 2>&1
  local rc=$?
  res "slice $name (exit $rc)" $rc
  if [ -f "$d/result.gcode.3mf" ]; then
    python3 -I tools/print/analyze_gcode.py "$d/result.gcode.3mf" --probes "$probes" \
      --log "$d/slice.log" --out "$d/analysis.json" > "$OUT/analysis_$name.log" 2>&1
    rc=$?
    python3 -I - "$d/analysis.json" >> "$SUM" <<'EOF'
import json, sys
a = json.load(open(sys.argv[1]))
pr = a.get("probes", [])
print("    layers %s (header %s), max_z %s, objects %s" % (a.get("layer_count"), a.get("header_layer_count"),
      a.get("header", {}).get("max_z_height"), [o.get("name") for o in a.get("plate_json", {}).get("bbox_objects", [])]))
for o in a.get("plate_json", {}).get("bbox_objects", []):
    print("      slicer bbox %-50s %s" % (o.get("name", "")[:50], [round(v, 4) for v in o.get("bbox", [])]))
for m in a.get("mesh_stat", []):
    keys = ("edges_fixed", "degenerate_facets", "facets_removed", "facets_reversed", "backwards_edges")
    print("      repair counters %-40s %s" % (m.get("part", "")[:40], {k: m.get(k) for k in keys}))
nb = [f for f in a.get("log", {}).get("findings", []) if not f.get("benign")]
print("    probes %d/%d pass; non-benign slicer log findings: %d" % (
      sum(1 for p in pr if p.get("ok")), len(pr), len(nb)))
for p in pr:
    if not p.get("ok"):
        print("      FAIL probe:", p.get("name"), json.dumps({k: v for k, v in p.items() if k not in ("spec",)})[:300])
for f in nb:
    print("      log:", f.get("text", "")[:200])
EOF
    res "probes $name" $rc
  fi
}
P=tools/print/probes
slice enclosure_baseline_3mf "$P/baseline_plate.json" "$EXP/enclosure_baseline.3mf"
slice enclosure_baseline_stl "$P/baseline_plate.json" "$EXP"/enclosure_baseline-Enclosure_base.stl "$EXP"/enclosure_baseline-Enclosure_lid.stl
slice enclosure_variant_3mf "$P/variant_plate.json" "$EXP/enclosure_variant.3mf"
slice coupon_3mf "$P/coupon_plate.json" "$EXP/coupon.3mf"
slice bracket_3mf "$P/bracket_plate.json" "$EXP/bracket.3mf"

note "## result: $([ $FAIL -eq 0 ] && echo ALL OK || echo FAILURES)"
exit $FAIL
