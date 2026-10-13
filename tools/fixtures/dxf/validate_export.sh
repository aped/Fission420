#!/usr/bin/env bash
# Independent validation of the DXF writer (and of the hand-written fixtures) with FreeCAD's
# C++ DXF importer. Expected values are analytic (below), not read from R-CAD output.
#
# usage: tools/fixtures/dxf/validate_export.sh [OUT_DIR]   (default artifacts/exchange/exports)
# Needs the org.freecad.FreeCAD flatpak. Exit status 1 on any mismatch.
set -euo pipefail
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
OUT=$(realpath -m "${1:-$REPO/artifacts/exchange/exports}")
FIX="$REPO/crates/rcad-exchange/tests/fixtures/dxf"
mkdir -p "$OUT"
(cd "$REPO" && cargo run -q --release -p rcad-exchange --example export_samples -- "$OUT")

files=(
  "$OUT/export_slot.dxf" "$OUT/export_bulge.dxf" "$OUT/export_notch.dxf"
  "$OUT/export_construction.dxf"
  "$FIX/lwpoly_bulge.dxf" "$FIX/polyline_bulge_r12_unitless.dxf" "$FIX/slot_mirrored_arc.dxf"
  "$FIX/rect_inch.dxf" "$FIX/slot_freecad_r14.dxf"
)
list=$(IFS=:; echo "${files[*]}")
timeout 300 flatpak run --env="RCAD_DXF=$list" --filesystem="$REPO" --filesystem="$OUT" \
  --command=FreeCADCmd org.freecad.FreeCAD "$REPO/tools/fixtures/dxf/freecad_read_dxf.py" 2>&1 \
  | grep -o 'FREECAD_RESULT .*' | tee "$OUT/freecad_read_dxf.txt"

python3 - "$OUT/freecad_read_dxf.txt" <<'PY'
import json, math, sys
pi = math.pi
# file -> (edge counts by FreeCAD curve type, area or None, bounds [xmin,ymin,xmax,ymax],
#          arcs [(cx, cy, r, sweep_deg)])
expect = {
    "export_slot.dxf": ({"Line": 3, "Circle": 1}, 600 + 50 * pi, [0, 0, 40, 20], [(30, 10, 10, 180)]),
    "export_bulge.dxf": ({"Line": 2, "Circle": 2}, 500.0, [0, 0, 30, 20],
                         [(-10, 10, 10 * math.sqrt(2), 90), (20, 10, 10, 180)]),
    "export_notch.dxf": ({"Line": 5, "Circle": 1}, 800 - 12.5 * pi, [0, 0, 40, 20], [(20, 20, 5, 180)]),
    "export_construction.dxf": ({"Line": 1, "Circle": 1}, None, [-10, -5, 10, 5], [(0, 0, 5, 360)]),
    "lwpoly_bulge.dxf": ({"Line": 2, "Circle": 2}, 500.0, [0, 0, 30, 20],
                         [(-10, 10, 10 * math.sqrt(2), 90), (20, 10, 10, 180)]),
    # Known FreeCAD 1.1.4 deviation (observed): the closing segment of a closed R12 POLYLINE
    # (here a bulge arc from the last vertex to the first) is not created. ezdxf 1.4.2 reads
    # all four segments, as does FreeCAD itself for the equivalent LWPOLYLINE (area 500).
    "polyline_bulge_r12_unitless.dxf": ({"Line": 2, "Circle": 1}, None, [0, 0, 30, 20],
                                        [(20, 10, 10, 180)]),
    # Known FreeCAD 1.1.4 deviation (observed): its importer ignores the ARC's (0,0,-1)
    # extrusion and places the arc at OCS (-30,10). The DXF arbitrary-axis algorithm (and
    # ezdxf 1.4.2, tools/fixtures/dxf/ezdxf_check.py) give WCS centre (30,10). We assert
    # FreeCAD's observed output so a change in either is noticed.
    "slot_mirrored_arc.dxf": ({"Line": 3, "Circle": 1}, None, [-40, 0, 30, 20], [(-30, 10, 10, 180)]),
    "slot_freecad_r14.dxf": ({"Line": 3, "Circle": 1}, 600 + 50 * pi, [0, 0, 40, 20], [(30, 10, 10, 180)]),
    # FreeCAD may or may not apply $INSUNITS=1 (inches); both scales are reported.
    "rect_inch.dxf": ({"Line": 4}, "inch", None, []),
}
status = 0
for line in open(sys.argv[1]):
    if "FREECAD_RESULT " not in line:
        continue
    r = json.loads(line[line.index("FREECAD_RESULT ") + len("FREECAD_RESULT "):])
    name = r["file"]
    kinds, area, bounds, arcs = expect[name]
    errs = []
    if "error" in r:
        errs.append("import error: " + r["error"])
    if r.get("edges") != kinds:
        errs.append(f"edges {r.get('edges')} != {kinds}")
    if area == "inch":
        a = r.get("area") or 0
        scale = "mm (FreeCAD applied inches)" if abs(a - 50.8 * 25.4) < 1e-6 else (
            "drawing units (FreeCAD ignored $INSUNITS)" if abs(a - 2.0) < 1e-9 else None)
        if scale is None:
            errs.append(f"area {a} is neither 2 in^2 nor 1290.32 mm^2")
        else:
            print(f"  {name}: FreeCAD read the rectangle in {scale}")
    elif area is not None and (r.get("area") is None or abs(r["area"] - area) > 1e-6):
        errs.append(f"area {r.get('area')} != {area}")
    if bounds is not None and any(abs(a - b) > 1e-6 for a, b in zip(r.get("bounds", []), bounds)):
        errs.append(f"bounds {r.get('bounds')} != {bounds}")
    got = sorted(r.get("arcs", []))
    if len(got) != len(arcs) or any(
        any(abs(g - e) > 1e-6 for g, e in zip(ga, ea)) for ga, ea in zip(got, sorted(arcs))
    ):
        errs.append(f"arcs {got} != {sorted(arcs)}")
    print(f"{'PASS' if not errs else 'FAIL'} {name}" + ("" if not errs else ": " + "; ".join(errs)))
    status |= bool(errs)
sys.exit(status)
PY
