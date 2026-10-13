# FreeCAD (FreeCADCmd) script: the independent STEP fixture for direct editing of
# imported bodies (run 6), written to $RCAD_OUT/direct_edit_block_mm.step and
# re-imported for a verification line. Run through tools/fixtures/step/direct_edit.sh.
#
# Block [0,60]x[0,40]x[0,20] built from Part primitives (analytic values known):
#   - through hole  D8 at (15,20), axis Z
#   - blind hole    D6, 8 deep from the top (z = 12..20), at (30,12), flat floor
#   - boss          D10 x 5 on the top face (z = 20..25), at (45,25)
# V = 48000 - pi*16*20 - pi*9*8 + pi*25*5 = 48000 - 267*pi
import json
import math
import os
import re

import FreeCAD as App
import Part

OUT = os.environ["RCAD_OUT"]
NAME = "direct_edit_block_mm.step"
P = os.path.join(OUT, NAME)

V = App.Vector
block = Part.makeBox(60, 40, 20)
block = block.cut(Part.makeCylinder(4, 30, V(15, 20, -5)))
block = block.cut(Part.makeCylinder(3, 13, V(30, 12, 12)))
block = block.fuse(Part.makeCylinder(5, 6, V(45, 25, 19))).removeSplitter()
shape = block.Solids[0]
Part.exportUnits("MM")
shape.exportStep(P)

# Sanitize the header (no host paths); the DATA section is untouched.
with open(P, "r", encoding="latin-1") as f:
    text = f.read()
head, sep, rest = text.partition("DATA;")
head = re.sub(r"'(?:/[^'/]*)+/([^'/]+)'", r"'\1'", head)
head = head.replace(OUT + "/", "")
with open(P, "w", encoding="latin-1", newline="") as f:
    f.write(head + sep + rest)
for bad in ("/home/", "/var/home/", "/tmp/"):
    assert bad not in head + sep + rest, bad

s = Part.Shape()
s.read(P)
bb = s.optimalBoundingBox()
r = {
    "freecad": App.Version()[:4],
    "occ": Part.OCC_VERSION,
    "file": NAME,
    "solids": len(s.Solids),
    "faces": len(s.Faces),
    "isValid": s.isValid(),
    "volume": s.Volume,
    "expected_volume": 48000 - 267 * math.pi,
    "bbox": [bb.XMin, bb.YMin, bb.ZMin, bb.XMax, bb.YMax, bb.ZMax],
    "surfaces": sorted(type(f.Surface).__name__ for f in s.Faces),
}
print("RCAD_RESULTS " + json.dumps(r))
