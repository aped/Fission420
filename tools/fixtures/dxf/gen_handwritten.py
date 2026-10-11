#!/usr/bin/env python3
"""Hand-written ASCII DXF fixtures, built from the public DXF reference group codes
(AutoCAD DXF Reference: HEADER $ACADVER/$INSUNITS/$MEASUREMENT, ENTITIES LINE/ARC/CIRCLE/
LWPOLYLINE/POLYLINE/VERTEX/SEQEND/TEXT/SPLINE/INSERT, BLOCKS BLOCK/ENDBLK). Python stdlib only;
independent of the Rust reader/writer. Every expected value is derived analytically below.

usage: gen_handwritten.py OUT_DIR
"""
import math
import os
import sys

out = sys.argv[1]
T = math.tan(math.pi / 8)  # bulge of a 90 degree arc: tan(sweep / 4)


def num(v):
    return repr(float(v))


def doc(header, entities, blocks=None):
    """Group-code pairs -> DXF text. `header`: list of (name, code, value)."""
    lines = []

    def g(code, value):
        lines.append(f"{code:>3}")
        lines.append(str(value))

    if header is not None:
        g(0, "SECTION")
        g(2, "HEADER")
        for name, code, value in header:
            g(9, name)
            g(code, value)
        g(0, "ENDSEC")
    if blocks:
        g(0, "SECTION")
        g(2, "BLOCKS")
        for code, value in blocks:
            g(code, value)
        g(0, "ENDSEC")
    g(0, "SECTION")
    g(2, "ENTITIES")
    for code, value in entities:
        g(code, value)
    g(0, "ENDSEC")
    g(0, "EOF")
    return "\n".join(lines) + "\n"


def line(a, b, layer="0", z=(0.0, 0.0)):
    return [(0, "LINE"), (8, layer), (10, num(a[0])), (20, num(a[1])), (30, num(z[0])),
            (11, num(b[0])), (21, num(b[1])), (31, num(z[1]))]


def write(name, text):
    with open(os.path.join(out, name), "w", newline="\n") as f:
        f.write(text)


# (b) LWPOLYLINE with bulges, AC1015, mm.
#   vertices (0,0) b=0; (20,0) b=1 (CCW semicircle centre (20,10) r 10, bulging to x = 30);
#   (20,20) b=0; (0,20) b=-tan(pi/8) (closing segment to (0,0): CW quarter arc, centre
#   (-10,10), r = 10*sqrt(2), bulging into the square to x = 10*sqrt(2) - 10).
#   Area = 20*20 + pi*10^2/2 - (r^2/2)(pi/2 - 1) with r^2 = 200, i.e. 400 + 50*pi - (50*pi - 100) = 500.
mm_header = [("$ACADVER", 1, "AC1015"), ("$INSUNITS", 70, 4), ("$MEASUREMENT", 70, 1)]
lw = [(0, "LWPOLYLINE"), (8, "0"), (100, "AcDbEntity"), (100, "AcDbPolyline"), (90, 4), (70, 1),
      (43, "0.0")]
for (x, y), b in [((0, 0), 0.0), ((20, 0), 1.0), ((20, 20), 0.0), ((0, 20), -T)]:
    lw += [(10, num(x)), (20, num(y))]
    if b != 0.0:
        lw += [(42, num(b))]
lw += [(210, "0.0"), (220, "0.0"), (230, "1.0")]
write("lwpoly_bulge.dxf", doc(mm_header, lw))

# (c1) the same contour as an old-style R12 POLYLINE/VERTEX/SEQEND, with NO units declared
#   (R12 has no $INSUNITS): import must demand a unit choice.
pl = [(0, "POLYLINE"), (8, "0"), (66, 1), (10, "0.0"), (20, "0.0"), (30, "0.0"), (70, 1)]
for (x, y), b in [((0, 0), 0.0), ((20, 0), 1.0), ((20, 20), 0.0), ((0, 20), -T)]:
    pl += [(0, "VERTEX"), (8, "0"), (10, num(x)), (20, num(y)), (30, "0.0")]
    if b != 0.0:
        pl += [(42, num(b))]
    pl += [(70, 0)]
pl += [(0, "SEQEND"), (8, "0")]
write("polyline_bulge_r12_unitless.dxf", doc([("$ACADVER", 1, "AC1009")], pl))

# (c2) inches: 2 in x 1 in rectangle of LINEs, $INSUNITS = 1, $MEASUREMENT = 0.
#   In mm: 50.8 x 25.4, area 1290.32 mm^2.
in_header = [("$ACADVER", 1, "AC1015"), ("$INSUNITS", 70, 1), ("$MEASUREMENT", 70, 0)]
rect = []
pts = [(0, 0), (2, 0), (2, 1), (0, 1)]
for i in range(4):
    rect += line(pts[i], pts[(i + 1) % 4])
write("rect_inch.dxf", doc(in_header, rect))

# (e) mirrored arc: the slot of slot_freecad_r14.dxf (area 600 + 50*pi) with the ARC written
#   in an OCS with extrusion (0,0,-1). Arbitrary-axis algorithm: Ax = (0,1,0) x (0,0,-1)
#   = (-1,0,0), Ay = (0,1,0) -> WCS (x, y) = (-x_ocs, y_ocs). WCS centre (30,10) is OCS
#   (-30,10); the WCS arc from (30,0) via (40,10) to (30,20) is the OCS CCW arc from
#   90 deg to 270 deg about (-30,10).
mir = line((0, 0), (30, 0)) + [
    (0, "ARC"), (8, "0"), (10, "-30.0"), (20, "10.0"), (30, "0.0"), (40, "10.0"),
    (50, "90.0"), (51, "270.0"), (210, "0.0"), (220, "0.0"), (230, "-1.0"),
] + line((30, 20), (0, 20)) + line((0, 20), (0, 0))
write("slot_mirrored_arc.dxf", doc(mm_header, mir))

# (d) unsupported entities. Layer PROFILE: 10 x 10 square of LINEs (area 100) and a CIRCLE
#   (centre (5,5) r 2) on layer HOLES. Layer NOTES: TEXT, SPLINE, INSERT of block "B"
#   (whose LINE is a block definition and must never be imported), one LINE at z = 5
#   (not in the XY plane).
sq = []
pts = [(0, 0), (10, 0), (10, 10), (0, 10)]
for i in range(4):
    sq += line(pts[i], pts[(i + 1) % 4], layer="PROFILE")
sq += [(0, "CIRCLE"), (8, "HOLES"), (10, "5.0"), (20, "5.0"), (30, "0.0"), (40, "2.0")]
notes = [
    (0, "TEXT"), (8, "NOTES"), (10, "0.0"), (20, "12.0"), (30, "0.0"), (40, "2.5"), (1, "LABEL"),
    (0, "SPLINE"), (8, "NOTES"), (100, "AcDbEntity"), (100, "AcDbSpline"),
    (70, 8), (71, 3), (72, 8), (73, 4),
    (40, "0.0"), (40, "0.0"), (40, "0.0"), (40, "0.0"),
    (40, "1.0"), (40, "1.0"), (40, "1.0"), (40, "1.0"),
    (10, "0.0"), (20, "20.0"), (30, "0.0"), (10, "3.0"), (20, "25.0"), (30, "0.0"),
    (10, "6.0"), (20, "15.0"), (30, "0.0"), (10, "9.0"), (20, "20.0"), (30, "0.0"),
    (0, "INSERT"), (8, "NOTES"), (2, "B"), (10, "50.0"), (20, "50.0"), (30, "0.0"),
] + line((0, 30), (10, 30), layer="NOTES", z=(5.0, 5.0))
blocks = [(0, "BLOCK"), (8, "0"), (2, "B"), (70, 0), (10, "0.0"), (20, "0.0"), (30, "0.0"),
          (3, "B")] + line((0, 0), (1, 1)) + [(0, "ENDBLK"), (8, "0")]
write("unsupported_entities.dxf", doc(mm_header, sq + notes, blocks))
print("hand-written DXF fixtures written to", out)
