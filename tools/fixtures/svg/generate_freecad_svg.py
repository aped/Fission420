# Generates crates/f420-exchange/tests/fixtures/svg/freecad_plate.svg with FreeCAD 1.1.4 (Draft importSVG.export):
# flatpak run --env=F420_OUT=<abs path> --filesystem=<dir> --command=FreeCADCmd org.freecad.FreeCAD tools/fixtures/svg/generate_freecad_svg.py
# The printed 'area' is NOT a reference: Part.Face here gets inner wires with the outer wire's
# orientation, so FreeCAD adds them (2643.36). The test uses the analytic area 2400 - 36pi - (80 + 16pi).
import FreeCAD, Part, os, json
out = os.environ["F420_OUT"]
doc = FreeCAD.newDocument("svg")
# 60 x 40 plate outline with a Ø12 hole at (20, 20) and a slot made of two lines + two arcs.
outer = Part.makePolygon([FreeCAD.Vector(0,0,0), FreeCAD.Vector(60,0,0), FreeCAD.Vector(60,40,0), FreeCAD.Vector(0,40,0), FreeCAD.Vector(0,0,0)])
hole = Part.Wire(Part.makeCircle(6, FreeCAD.Vector(20,20,0)))
a1 = Part.ArcOfCircle(Part.Circle(FreeCAD.Vector(38,20,0), FreeCAD.Vector(0,0,1), 4), 1.5707963267948966, 4.71238898038469)
a2 = Part.ArcOfCircle(Part.Circle(FreeCAD.Vector(48,20,0), FreeCAD.Vector(0,0,1), 4), -1.5707963267948966, 1.5707963267948966)
slot = Part.Wire([Part.Edge(a1), Part.makeLine(FreeCAD.Vector(38,16,0), FreeCAD.Vector(48,16,0)), Part.Edge(a2), Part.makeLine(FreeCAD.Vector(48,24,0), FreeCAD.Vector(38,24,0))])
face = Part.Face([outer, hole, slot])
objs = []
for n, w in [("Outer", outer), ("Hole", hole), ("Slot", slot)]:
    o = doc.addObject("Part::Feature", n); o.Shape = w; objs.append(o)
doc.recompute()
import importSVG
importSVG.export(objs, out)
print("FREECAD_RESULT " + json.dumps({"ok": os.path.exists(out), "area": face.Area}))
