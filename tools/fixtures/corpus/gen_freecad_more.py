# FreeCAD (FreeCADCmd) producer of the second set of qualification corpus files
# (tools/fixtures/corpus/MANIFEST.md): an assembly written in inches, two-level
# nested occurrences that are both rotated and translated, the uncoloured base of
# the colour test (colours are added by gen_handwritten.py: headless FreeCAD writes
# no colours) and a multi-shell binary STL. Prints "RCAD_CORPUS {json}" with
# FreeCAD's own read-back measurements (the independent reference).
# Run through tools/fixtures/corpus/generate.sh.
import json
import os
import re

import FreeCAD as App
import Import
import Mesh
import Part

OUT = os.environ["RCAD_OUT"]
V = App.Vector
res = {}


def p(name):
    return os.path.join(OUT, name)


def sanitize(name):
    t = open(p(name), encoding="latin-1").read()
    head, sep, rest = t.partition("DATA;")
    head = re.sub(r"'(?:/[^'/]*)+/([^'/]+)'", r"'\1'", head)
    open(p(name), "w", encoding="latin-1", newline="").write(head + sep + rest)


def measure_step(name):
    s = Part.Shape()
    s.read(p(name))
    bb = s.BoundBox
    t = open(p(name), encoding="latin-1").read()
    res[name] = {
        "solids": len(s.Solids),
        "volumes": sorted(round(x.Volume, 6) for x in s.Solids),
        "volume": round(sum(x.Volume for x in s.Solids), 6),
        "bbox": [round(v, 6) for v in (bb.XMin, bb.YMin, bb.ZMin, bb.XMax, bb.YMax, bb.ZMax)],
        "valid": s.isValid(),
        "inch_unit": "'INCH'" in t.upper(),
    }


def export_assembly(objs, name, unit="MM"):
    Part.exportUnits(unit)
    try:
        Import.export(objs, p(name))
    finally:
        Part.exportUnits("MM")
    sanitize(name)
    measure_step(name)


# A4: assembly in inches: Top{ Sub{block 2x1x0.5 in @ (1,0,0)} rotated 90 deg about Z
# and moved (0,2,0) in, pin r 0.25 in h 1 in at (3,0,0) in }.
doc = App.newDocument("more1")
top = doc.addObject("App::Part", "TopInch")
sub = doc.addObject("App::Part", "SubInch")
top.addObject(sub)
blk = doc.addObject("Part::Box", "BlockInch")
blk.Length, blk.Width, blk.Height = 2, 1, 0.5
blk.Placement = App.Placement(V(1, 0, 0), App.Rotation())
sub.addObject(blk)
sub.Placement = App.Placement(V(0, 2, 0), App.Rotation(V(0, 0, 1), 90))
pin = doc.addObject("Part::Cylinder", "PinInch")
pin.Radius, pin.Height = 0.25, 1
pin.Placement = App.Placement(V(3, 0, 0), App.Rotation())
top.addObject(pin)
doc.recompute()
export_assembly([top], "a4_assembly_inch.step", unit="INCH")
App.closeDocument(doc.Name)

# A5: two nested levels, each rotated and translated: Root (identity; FreeCAD does
# not write the exported root's own placement) holds Outer (rot 30 deg about Z,
# +(100, 0, 0)), which contains Inner (rot 90 deg about X, +(0, 20, 5)) containing a
# 20x10x4 box at (5, 0, 0); Outer also holds a cylinder r 4 h 12 at (0, -30, 0).
doc = App.newDocument("more2")
root = doc.addObject("App::Part", "Root")
outer = doc.addObject("App::Part", "Outer")
root.addObject(outer)
inner = doc.addObject("App::Part", "Inner")
outer.addObject(inner)
bx = doc.addObject("Part::Box", "NestedBox")
bx.Length, bx.Width, bx.Height = 20, 10, 4
bx.Placement = App.Placement(V(5, 0, 0), App.Rotation())
inner.addObject(bx)
inner.Placement = App.Placement(V(0, 20, 5), App.Rotation(V(1, 0, 0), 90))
cy = doc.addObject("Part::Cylinder", "NestedPin")
cy.Radius, cy.Height = 4, 12
cy.Placement = App.Placement(V(0, -30, 0), App.Rotation())
outer.addObject(cy)
outer.Placement = App.Placement(V(100, 0, 0), App.Rotation(V(0, 0, 1), 30))
doc.recompute()
export_assembly([root], "a5_nested_transforms_mm.step")
App.closeDocument(doc.Name)

# C0: base of the colour test: two separate blocks in one part (24000 + 1000).
cb = Part.Compound([Part.makeBox(40, 30, 20), Part.makeBox(10, 10, 10, V(60, 0, 0))])
cb.exportStep(p("c0_blocks_uncoloured_mm.step"))
sanitize("c0_blocks_uncoloured_mm.step")
measure_step("c0_blocks_uncoloured_mm.step")

# M3: binary STL, two shells (a box and a disjoint sphere) in one mesh.
mesh = Mesh.Mesh(Part.makeBox(20, 20, 10).tessellate(0.01))
ball = Mesh.createSphere(6, 16)
ball.translate(50, 10, 6)
mesh.addMesh(ball)
mesh.write(p("m3_two_shells_binary.stl"), "STL")
mm = Mesh.Mesh(p("m3_two_shells_binary.stl"))
bb = mm.BoundBox
res["m3_two_shells_binary.stl"] = {
    "facets": mm.CountFacets,
    "volume": round(mm.Volume, 6),
    "bbox": [round(v, 6) for v in (bb.XMin, bb.YMin, bb.ZMin, bb.XMax, bb.YMax, bb.ZMax)],
    "components": len(mm.getSeparateComponents()),
    "solid": mm.isSolid(),
}
print("RCAD_CORPUS " + json.dumps(res, sort_keys=True))
