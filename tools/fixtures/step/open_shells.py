# FreeCAD (FreeCADCmd) script: independent STEP fixtures for surface bodies and
# open-shell repair (docs/SURFACES.md), written to $RCAD_OUT and
# re-imported for a verification line. Run through tools/fixtures/step/open_shells.sh.
#
#   open_box_mm.step      box [0,40]x[0,30]x[0,20] without its top face: one sewn
#                         open shell of 5 planar faces. Filling the top gives
#                         V = 40*30*20 = 24000 mm^3.
#   loose_faces_mm.step   the same 5 faces NOT sewn (separate faces, each its own
#                         boundary): stitching at a tiny tolerance must find 8
#                         coincident free-edge pairs, then the top hole.
#   gap_box_mm.step       the loose faces with the +X wall moved 0.02 mm outward
#                         (x = 40.02): a 0.02 mm gap to the floor and the two side
#                         walls, which still end at x = 40.
#   plate_mm.step         a single planar face [0,50]x[0,20] at z = 0 (thicken).
#   half_tube_mm.step     half of a cylinder surface, radius 10, axis Z, z 0..30,
#                         x >= 0 (thicken; inward collapse at t >= 10).
import json
import os

import FreeCAD as App
import Part

OUT = os.environ["RCAD_OUT"]
V = App.Vector
Part.exportUnits("MM")
res = {}


def faces_of_box():
    box = Part.makeBox(40, 30, 20)
    out = []
    for f in box.Faces:
        c = f.CenterOfMass
        if abs(c.z - 20) < 1e-9:
            continue  # the top
        out.append(f)
    return out


def write(name, shape):
    p = os.path.join(OUT, name)
    shape.exportStep(p)
    back = Part.read(p)
    res[name] = {
        "shells": len(back.Shells),
        "faces": len(back.Faces),
        "solids": len(back.Solids),
        "area": round(back.Area, 9),
        "closed": [s.isClosed() for s in back.Shells],
        "bbox": [round(v, 9) for v in (back.BoundBox.XMin, back.BoundBox.YMin, back.BoundBox.ZMin,
                                         back.BoundBox.XMax, back.BoundBox.YMax, back.BoundBox.ZMax)],
    }


fs = faces_of_box()
write("open_box_mm.step", Part.Shell(fs))
write("loose_faces_mm.step", Part.Compound([f.copy() for f in fs]))
gap = []
for f in fs:
    if abs(f.CenterOfMass.x - 40) < 1e-9:
        g = f.copy()
        g.translate(V(0.02, 0, 0))
        gap.append(g)
    else:
        gap.append(f.copy())
write("gap_box_mm.step", Part.Compound(gap))
write("plate_mm.step", Part.Face(Part.makePolygon([V(0, 0, 0), V(50, 0, 0), V(50, 20, 0), V(0, 20, 0), V(0, 0, 0)])))
# Half tube: the cylinder surface (radius 10, axis Z) trimmed to u in [-pi/2, pi/2]
# (x >= 0), v in [0, 30].
surf = Part.Cylinder()
surf.Radius = 10
half = surf.toShape(-1.5707963267948966, 1.5707963267948966, 0, 30)
write("half_tube_mm.step", half)
print("RCAD_RESULTS " + json.dumps(res, sort_keys=True))
