# Generate STL fixtures with FreeCAD's Mesh module (independent writer: FreeCAD/OpenCascade).
#
# usage (from the repository root):
#   tools/fixtures/stl/gen_freecad.sh
# which runs:
#   flatpak run --env=OUT=<repo>/crates/f420-exchange/tests/fixtures/stl --filesystem=<repo> \
#     --command=FreeCADCmd org.freecad.FreeCAD tools/fixtures/stl/gen_freecad.py
#
# Outputs (all of a 20 x 10 x 5 mm box with one corner at the origin):
#   box_freecad_ascii.stl   ASCII ("AST" format)
#   box_freecad_binary.stl  binary ("STL" format), same mesh
#   box_open_freecad.stl    binary, the two +Z (top) facets removed -> 4 boundary edges
# and prints one FREECAD_RESULT line per file with FreeCAD's own diagnostics.
import json
import os

import Mesh
import Part

out = os.environ["OUT"]

shape = Part.makeBox(20, 10, 5)
# Planar faces tessellate exactly; the deflection only matters for curved faces.
mesh = Mesh.Mesh(shape.tessellate(0.01))

ascii_path = os.path.join(out, "box_freecad_ascii.stl")
binary_path = os.path.join(out, "box_freecad_binary.stl")
open_path = os.path.join(out, "box_open_freecad.stl")
mesh.write(ascii_path, "AST", "box20x10x5")
mesh.write(binary_path, "STL", "box20x10x5")

open_mesh = mesh.copy()
top = [f.Index for f in open_mesh.Facets if f.Normal.z > 0.9]
open_mesh.removeFacets(top)
open_mesh.write(open_path, "STL", "box_open")


def report(path):
    m = Mesh.Mesh(path)
    bb = m.BoundBox
    print("FREECAD_RESULT " + json.dumps({
        "file": os.path.basename(path),
        "points": m.CountPoints,
        "facets": m.CountFacets,
        "isSolid": m.isSolid(),
        "hasNonManifolds": m.hasNonManifolds(),
        "volume": m.Volume,
        "area": m.Area,
        "bounds": [bb.XMin, bb.YMin, bb.ZMin, bb.XMax, bb.YMax, bb.ZMax],
    }))


for p in (ascii_path, binary_path, open_path):
    report(p)
print("removed top facets:", top)
