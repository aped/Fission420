#!/usr/bin/env python3
"""Deterministic unit conversion of box_40x30x10_mm.step (plain python3).

usage: make_units.py <fixture dir>

Why this exists: FreeCAD 1.1.4 (OCC 7.8.1) cannot write a scaled non-mm
STEP. Part.exportUnits('INCH'|'M') (and the hSTEP 'Unit' preference) only
relabel the declared length unit and leave the coordinates in mm-valued
numbers, so the result is a 40x30x10 *inch* (or metre) box. Those relabelled
files are kept as freecad_relabel_*.step; the true "same box, other unit"
fixtures are produced here instead.

Transformation, applied to the DATA section only:
  * every numeric in CARTESIAN_POINT coordinate lists, VECTOR magnitudes,
    the LENGTH_MEASURE of the uncertainty, and the radius parameters of
    CIRCLE / CYLINDRICAL_SURFACE / CONICAL_SURFACE / SPHERICAL_SURFACE /
    TOROIDAL_SURFACE is divided by the factor (25.4 or 1000);
  * the length unit instance ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,
    .METRE.) ) is replaced:
      metre: ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT($,.METRE.) )
      inch:  ( CONVERSION_BASED_UNIT('INCH',#c) LENGTH_UNIT() NAMED_UNIT(#d) )
             #c = LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#mm)
             #mm = ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) )
             #d = DIMENSIONAL_EXPONENTS(1.,0.,0.,0.,0.,0.,0.)
             with #c,#mm,#d appended after the highest existing instance id.
The source must contain only entity types whose length-valued attributes are
covered above (asserted); pcurves in parameter space are rejected.
Numbers are written with Python repr (shortest round-trip), e.g.
40/25.4 -> 1.574803149606299.
"""
import re
import sys

out = sys.argv[1]
SRC = f"{out}/box_40x30x10_mm.step"
MM_UNIT = "( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) )"
NUM = r"[-+]?(?:\d+\.?\d*|\.\d+)(?:[Ee][-+]?\d+)?"
ALLOWED = {
    "ADVANCED_BREP_SHAPE_REPRESENTATION", "ADVANCED_FACE", "APPLICATION_CONTEXT",
    "APPLICATION_PROTOCOL_DEFINITION", "AXIS2_PLACEMENT_3D", "CARTESIAN_POINT",
    "CLOSED_SHELL", "DIRECTION", "EDGE_CURVE", "EDGE_LOOP", "FACE_BOUND",
    "GEOMETRIC_REPRESENTATION_CONTEXT", "GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT",
    "GLOBAL_UNIT_ASSIGNED_CONTEXT", "LENGTH_MEASURE", "LENGTH_UNIT", "LINE",
    "MANIFOLD_SOLID_BREP", "NAMED_UNIT", "ORIENTED_EDGE", "PLANE",
    "PLANE_ANGLE_UNIT", "PRODUCT", "PRODUCT_CONTEXT", "PRODUCT_DEFINITION",
    "PRODUCT_DEFINITION_CONTEXT", "PRODUCT_DEFINITION_FORMATION",
    "PRODUCT_DEFINITION_SHAPE", "PRODUCT_RELATED_PRODUCT_CATEGORY",
    "REPRESENTATION_CONTEXT", "SHAPE_DEFINITION_REPRESENTATION", "SI_UNIT",
    "SOLID_ANGLE_UNIT", "UNCERTAINTY_MEASURE_WITH_UNIT", "VECTOR", "VERTEX_POINT",
    "CIRCLE", "CYLINDRICAL_SURFACE", "CONICAL_SURFACE", "SPHERICAL_SURFACE",
    "TOROIDAL_SURFACE",
}


def fmt(x):
    s = repr(x)
    if "e" in s or "E" in s:
        m, e = s.lower().split("e")
        if "." not in m:
            m += "."
        return f"{m}E{int(e)}"
    return s if "." in s else s + "."


def scale_nums(s, f):
    return re.sub(NUM, lambda m: fmt(float(m.group(0)) / f), s)


def convert(text, f):
    head, sep, data = text.partition("DATA;")
    used = set(re.findall(r"\b([A-Z_][A-Z0-9_]*)\(", data))
    assert used <= ALLOWED, f"unsupported entities: {sorted(used - ALLOWED)}"
    # CARTESIAN_POINT('',(x,y,z))
    data = re.sub(r"(CARTESIAN_POINT\('[^']*',\()([^)]*)(\))",
                  lambda m: m.group(1) + scale_nums(m.group(2), f) + m.group(3), data)
    # VECTOR('',#d,mag)
    data = re.sub(rf"(VECTOR\('[^']*',#\d+,)({NUM})(\))",
                  lambda m: m.group(1) + scale_nums(m.group(2), f) + m.group(3), data)
    # Uncertainty LENGTH_MEASURE(v)
    data = re.sub(rf"(LENGTH_MEASURE\()({NUM})(\))",
                  lambda m: m.group(1) + scale_nums(m.group(2), f) + m.group(3), data)
    # Radii: CIRCLE/CYL/SPH('',#a,r) ; CONICAL('',#a,r,angle) ; TOROIDAL('',#a,R,r)
    data = re.sub(rf"((?:CIRCLE|CYLINDRICAL_SURFACE|SPHERICAL_SURFACE|CONICAL_SURFACE)"
                  rf"\('[^']*',#\d+,\s*)({NUM})",
                  lambda m: m.group(1) + scale_nums(m.group(2), f), data)
    data = re.sub(rf"(TOROIDAL_SURFACE\('[^']*',#\d+,\s*)({NUM}),\s*({NUM})",
                  lambda m: m.group(1) + scale_nums(m.group(2), f) + ","
                  + scale_nums(m.group(3), f), data)
    return head + sep + data


def main():
    src = open(SRC, encoding="latin-1").read()
    assert src.count(MM_UNIT) == 1, "expected exactly one mm length unit"

    m_text = convert(src, 1000.0).replace(
        MM_UNIT, "( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT($,.METRE.) )")
    open(f"{out}/box_40x30x10_m.step", "w", encoding="latin-1",
         newline="").write(m_text)

    in_text = convert(src, 25.4)
    top = max(int(i) for i in re.findall(r"^#(\d+)\s*=", in_text, re.M))
    c, mm, d = top + 1, top + 2, top + 3
    in_text = in_text.replace(
        MM_UNIT,
        f"( CONVERSION_BASED_UNIT('INCH',#{c}) LENGTH_UNIT() NAMED_UNIT(#{d}) )")
    extra = (f"#{c} = LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#{mm});\n"
             f"#{mm} = {MM_UNIT};\n"
             f"#{d} = DIMENSIONAL_EXPONENTS(1.,0.,0.,0.,0.,0.,0.);\n")
    i = in_text.rindex("ENDSEC;")
    in_text = in_text[:i] + extra + in_text[i:]
    open(f"{out}/box_40x30x10_inch.step", "w", encoding="latin-1",
         newline="").write(in_text)
    print("units: box_40x30x10_m.step box_40x30x10_inch.step")


if __name__ == "__main__":
    main()
