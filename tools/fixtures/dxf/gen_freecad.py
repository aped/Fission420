# Generate DXF fixtures with FreeCAD's C++ DXF writer (Import.writeDXFShape), an independent
# writer. Run via tools/fixtures/dxf/gen_freecad.sh.
#
# Outputs (mm, $INSUNITS = 4 written by FreeCAD):
#   slot_freecad_r14.dxf   AC1014: lines (0,0)-(30,0), (30,20)-(0,20), (0,20)-(0,0) and the
#                          arc centre (30,10) r 10 from -90 deg to 90 deg (CCW).
#                          Closed contour, area 30*20 + pi*10^2/2 = 600 + 50*pi.
#   notch_freecad_r12.dxf  AC1009 (R12): 40 x 20 rectangle with a semicircular notch of radius 5
#                          centred at (20,20) cut into the top edge (the contour runs clockwise
#                          around the notch centre, the DXF ARC is still CCW from 180 to 360 deg).
#                          Area 40*20 - pi*5^2/2 = 800 - 12.5*pi.
import os

import Import
import Part
from FreeCAD import Vector as V

out = os.environ["OUT"]
half_pi = 1.5707963267948966
pi = 3.141592653589793


def seg(a, b):
    return Part.LineSegment(V(*a, 0), V(*b, 0)).toShape()


slot = Part.Wire([
    seg((0, 0), (30, 0)),
    Part.ArcOfCircle(Part.Circle(V(30, 10, 0), V(0, 0, 1), 10), -half_pi, half_pi).toShape(),
    seg((30, 20), (0, 20)),
    seg((0, 20), (0, 0)),
])
Import.writeDXFShape([slot], os.path.join(out, "slot_freecad_r14.dxf"), 14, False)

notch = Part.Wire([
    seg((0, 0), (40, 0)),
    seg((40, 0), (40, 20)),
    seg((40, 20), (25, 20)),
    # CCW arc from 180 deg (15,20) to 360 deg (25,20) through (20,15); traversed backwards.
    Part.ArcOfCircle(Part.Circle(V(20, 20, 0), V(0, 0, 1), 5), pi, 2 * pi).toShape(),
    seg((15, 20), (0, 20)),
    seg((0, 20), (0, 0)),
])
Import.writeDXFShape([notch], os.path.join(out, "notch_freecad_r12.dxf"), 12, False)
print("FREECAD_RESULT written slot_freecad_r14.dxf notch_freecad_r12.dxf")
