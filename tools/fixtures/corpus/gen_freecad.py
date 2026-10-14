# FreeCAD (FreeCADCmd) producer of the qualification corpus parts and
# assemblies (tools/fixtures/corpus/MANIFEST.md). Writes the files into
# $RCAD_OUT and prints "RCAD_CORPUS {json}" with FreeCAD's own measurement of
# each file *read back* (solids, volume, bbox), the independent reference.
# Run through tools/fixtures/corpus/generate.sh.
import json
import math
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
    assert "/home/" not in t.replace(head, "") and "/home/" not in head


def measure_step(name):
    s = Part.Shape()
    s.read(p(name))
    bb = s.BoundBox
    res[name] = {
        "solids": len(s.Solids),
        "volumes": sorted(round(x.Volume, 6) for x in s.Solids),
        "volume": round(sum(x.Volume for x in s.Solids), 6),
        "bbox": [round(v, 6) for v in (bb.XMin, bb.YMin, bb.ZMin, bb.XMax, bb.YMax, bb.ZMax)],
        "valid": s.isValid(),
    }


def shape_step(shape, name, unit="MM"):
    Part.exportUnits(unit)
    try:
        shape.exportStep(p(name))
    finally:
        Part.exportUnits("MM")
    sanitize(name)
    measure_step(name)


# P1: bracket with two through holes and a counterbore (mm).
b = Part.makeBox(60, 40, 10)
b = b.cut(Part.makeCylinder(4, 20, V(15, 20, -5)))
b = b.cut(Part.makeCylinder(4, 20, V(45, 20, -5)))
b = b.cut(Part.makeCylinder(7, 5, V(15, 20, 7)))
shape_step(b.removeSplitter(), "p1_bracket_holes_mm.step")

# P2: plate declared in inches (FreeCAD relabels: coordinates are inch numbers).
pl = Part.makeBox(2, 1.5, 0.25).cut(Part.makeCylinder(0.25, 1, V(1, 0.75, -0.5)))
shape_step(pl.removeSplitter(), "p2_plate_inch.step", unit="INCH")

# P3: three curved solids in one file (sphere, cone frustum, torus).
sph = Part.makeSphere(10)
cone = Part.makeCone(10, 4, 15, V(40, 0, 0))
tor = Part.makeTorus(12, 3, V(80, 0, 0))
shape_step(Part.Compound([sph, cone, tor]), "p3_curved_multi_mm.step")

# P4: a box rotated 30° about (1,1,0) and translated.
bx = Part.makeBox(30, 20, 10)
bx.Placement = App.Placement(V(100, 50, 25), App.Rotation(V(1, 1, 0), 30))
shape_step(bx, "p4_rotated_box_mm.step")

# P5: stepped shaft (two coaxial cylinders) with a filleted shoulder edge.
sh = Part.makeCylinder(10, 30).fuse(Part.makeCylinder(6, 20, V(0, 0, 30))).removeSplitter()
shape_step(sh, "p5_stepped_shaft_mm.step")

# P6: open shell (surface model): an L of two planar faces.
f1 = Part.Face(Part.makePolygon([V(0, 0, 0), V(30, 0, 0), V(30, 20, 0), V(0, 20, 0), V(0, 0, 0)]))
f2 = Part.Face(Part.makePolygon([V(30, 0, 0), V(30, 0, 15), V(30, 20, 15), V(30, 20, 0), V(30, 0, 0)]))
Part.Shell([f1, f2]).exportStep(p("p6_open_shell_mm.step"))
sanitize("p6_open_shell_mm.step")
back = Part.read(p("p6_open_shell_mm.step"))
res["p6_open_shell_mm.step"] = {"solids": len(back.Solids), "faces": len(back.Faces),
                                "area": round(back.Area, 6)}

# P7-P12: closed-form volumes.
# P7: 40x30x20 block, one vertical edge chamfered 5 mm: 24000 - 12.5*20 = 23750.
blk = Part.makeBox(40, 30, 20)
e = [x for x in blk.Edges if abs(x.Vertexes[0].X - 40) < 1e-9 and abs(x.Vertexes[0].Y - 30) < 1e-9
     and abs(x.Vertexes[1].X - 40) < 1e-9 and abs(x.Vertexes[1].Y - 30) < 1e-9][0]
shape_step(blk.makeChamfer(5, [e]), "p7_chamfer_block_mm.step")
# P8: same block, that edge filleted r 4: 24000 - (16 - 4 pi) * 20.
blk = Part.makeBox(40, 30, 20)
e = [x for x in blk.Edges if abs(x.Vertexes[0].X - 40) < 1e-9 and abs(x.Vertexes[0].Y - 30) < 1e-9
     and abs(x.Vertexes[1].X - 40) < 1e-9 and abs(x.Vertexes[1].Y - 30) < 1e-9][0]
shape_step(blk.makeFillet(4, [e]), "p8_fillet_block_mm.step")
# P9: tube Ro 15, Ri 10, h 30: 3750 pi.
shape_step(Part.makeCylinder(15, 30).cut(Part.makeCylinder(10, 40, V(0, 0, -5))).removeSplitter(),
           "p9_tube_mm.step")
# P10: right-triangle prism (0,0)-(40,0)-(0,30), h 10: 6000.
tri = Part.Face(Part.makePolygon([V(0, 0, 0), V(40, 0, 0), V(0, 30, 0), V(0, 0, 0)]))
shape_step(tri.extrude(V(0, 0, 10)), "p10_triangle_prism_mm.step")
# P11: frustum R 10, r 5, h 20 with a coaxial r 2 through hole: pi (3500/3 - 80).
fr = Part.makeCone(10, 5, 20).cut(Part.makeCylinder(2, 30, V(0, 0, -5))).removeSplitter()
shape_step(fr, "p11_holed_frustum_mm.step")
# P12: inch file, cylinder r 0.5 in, h 2 in rotated 90 deg about Y: 0.5 pi in^3.
cy = Part.makeCylinder(0.5, 2)
cy.Placement = App.Placement(V(1, 1, 1), App.Rotation(V(0, 1, 0), 90))
shape_step(cy, "p12_rotated_cylinder_inch.step", unit="INCH")

# A1: nested assembly Top{ Sub{box @ (10,0,0)} @ rot 90° Z + (0,50,0), cylinder }.
doc = App.newDocument("corpus")
top = doc.addObject("App::Part", "Top")
sub = doc.addObject("App::Part", "Sub")
top.addObject(sub)
box = doc.addObject("Part::Box", "Block")
box.Length, box.Width, box.Height = 20, 10, 5
box.Placement = App.Placement(V(10, 0, 0), App.Rotation())
sub.addObject(box)
sub.Placement = App.Placement(V(0, 50, 0), App.Rotation(V(0, 0, 1), 90))
cyl = doc.addObject("Part::Cylinder", "Pin")
cyl.Radius, cyl.Height = 3, 25
top.addObject(cyl)
doc.recompute()
Import.export([top], p("a1_nested_assembly_mm.step"))
sanitize("a1_nested_assembly_mm.step")
measure_step("a1_nested_assembly_mm.step")
App.closeDocument(doc.Name)

# A2: one part placed three times (App::Link instances) in an assembly.
doc = App.newDocument("corpus2")
asm = doc.addObject("App::Part", "Rack")
bolt = doc.addObject("Part::Cylinder", "Bolt")
bolt.Radius, bolt.Height = 2.5, 16
for i in range(3):
    ln = doc.addObject("App::Link", f"Bolt{i}")
    ln.LinkedObject = bolt
    ln.Placement = App.Placement(V(20 * i, 0, 0), App.Rotation())
    asm.addObject(ln)
bolt.Visibility = False
doc.recompute()
Import.export([asm], p("a2_repeated_links_mm.step"))
sanitize("a2_repeated_links_mm.step")
measure_step("a2_repeated_links_mm.step")
App.closeDocument(doc.Name)

# A3: two different parts, one rotated about X and translated (holed plate + boss).
doc = App.newDocument("corpus3")
asm = doc.addObject("App::Part", "Fixture")
plate = doc.addObject("Part::Feature", "Plate")
plate.Shape = Part.makeBox(50, 30, 6).cut(Part.makeCylinder(5, 10, V(25, 15, -2)))
boss = doc.addObject("Part::Feature", "Boss")
boss.Shape = Part.makeCylinder(8, 12)
boss.Placement = App.Placement(V(0, -20, 20), App.Rotation(V(1, 0, 0), 45))
asm.addObject(plate)
asm.addObject(boss)
doc.recompute()
Import.export([asm], p("a3_rotated_parts_mm.step"))
sanitize("a3_rotated_parts_mm.step")
measure_step("a3_rotated_parts_mm.step")
App.closeDocument(doc.Name)

# M1: binary STL of a cylinder mesh; M2: ASCII STL of a sphere mesh.
m = Mesh.Mesh(Part.makeCylinder(10, 20).tessellate(0.05))
m.write(p("m1_cylinder_binary.stl"), "STL")
m2 = Mesh.createSphere(8, 20)
m2.write(p("m2_sphere_ascii.ast"), "AST")
os.replace(p("m2_sphere_ascii.ast"), p("m2_sphere_ascii.stl"))
for name in ("m1_cylinder_binary.stl", "m2_sphere_ascii.stl"):
    mm = Mesh.Mesh(p(name))
    bb = mm.BoundBox
    res[name] = {"facets": mm.CountFacets, "volume": round(mm.Volume, 6),
                 "bbox": [round(v, 6) for v in (bb.XMin, bb.YMin, bb.ZMin, bb.XMax, bb.YMax, bb.ZMax)],
                 "solid": mm.isSolid()}
print("RCAD_CORPUS " + json.dumps(res, sort_keys=True))
