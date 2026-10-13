# FreeCAD (FreeCADCmd) script: generate the STEP fixtures in
# crates/rcad-kernel/tests/fixtures/step/ and verify each by re-import.
#
# Run through tools/fixtures/step/generate.sh, which passes:
#   RCAD_OUT      absolute output directory
#   RCAD_RESULTS  absolute path of the JSON verification report to write
#
# Every fixture is built from Part primitives so its analytic values
# (volume, bounding box) are known exactly; see the README next to the
# fixtures for those values and the recorded re-import results.
import json
import math
import os
import re

import FreeCAD as App
import Import
import Part

OUT = os.environ["RCAD_OUT"]
RESULTS = os.environ.get("RCAD_RESULTS")


def path(name):
    return os.path.join(OUT, name)


def sanitize(fname):
    """Drop absolute host paths from the STEP header (FILE_NAME carries the
    output path). Keeps only the basename; the DATA section is untouched."""
    p = path(fname)
    with open(p, "r", encoding="latin-1") as f:
        text = f.read()
    head, sep, rest = text.partition("DATA;")
    head = re.sub(r"'(?:/[^'/]*)+/([^'/]+)'", r"'\1'", head)
    head = head.replace(OUT + "/", "")
    with open(p, "w", encoding="latin-1", newline="") as f:
        f.write(head + sep + rest)
    for bad in ("/home/", "/var/home/", "/tmp/"):
        assert bad not in head + sep + rest, (fname, bad)


def export_shape(shape, fname, unit="MM"):
    Part.exportUnits(unit)
    try:
        shape.exportStep(path(fname))
    finally:
        Part.exportUnits("MM")
    sanitize(fname)


def verify(fname):
    s = Part.Shape()
    try:
        s.read(path(fname))
        bb = s.optimalBoundingBox()  # BoundBox is loose on curved faces
        return {
            "ok": True,
            "solids": len(s.Solids),
            "faces": len(s.Faces),
            "isValid": s.isValid(),
            "volume": s.Volume,
            "solid_volumes": [sol.Volume for sol in s.Solids],
            "bbox": [bb.XMin, bb.YMin, bb.ZMin, bb.XMax, bb.YMax, bb.ZMax],
            "surfaces": sorted({type(f.Surface).__name__ for f in s.Faces}),
        }
    except Exception as e:  # noqa: BLE001 - report, don't abort
        return {"ok": False, "error": str(e)}


MODE = os.environ.get("RCAD_MODE", "all")  # generate | verify | all
results = {"freecad": App.Version()[:4], "occ": Part.OCC_VERSION, "files": {}}


def generate():
    # 1. Box 40x30x10 at origin.
    export_shape(Part.makeBox(40, 30, 10), "box_40x30x10_mm.step")

    # 2. Cylinder r=10 h=15, axis +Z through the origin.
    export_shape(Part.makeCylinder(10, 15), "cylinder_d20_h15_mm.step")

    # 3. Sphere r=10 centred at (5,5,5) (off-origin to catch placement bugs).
    export_shape(Part.makeSphere(10, App.Vector(5, 5, 5)), "sphere_r10_mm.step")

    # 4. Torus R=20 r=5, axis +Z through the origin.
    export_shape(Part.makeTorus(20, 5), "torus_R20_r5_mm.step")

    # 5. Plate 60x40x8 with a through hole d=10 at (30,20).
    plate = Part.makeBox(60, 40, 8).cut(
        Part.makeCylinder(5, 12, App.Vector(30, 20, -2)))
    export_shape(plate, "plate_hole_mm.step")

    # 6. Two boxes. B keeps its transform as a Placement (TopLoc_Location),
    #    not baked into the geometry.
    def box_a():
        return Part.makeBox(10, 10, 10)

    def box_b():
        b = Part.makeBox(20, 10, 5)
        b.Placement = App.Placement(App.Vector(50, 0, 0),
                                    App.Rotation(App.Vector(0, 0, 1), 30))
        return b

    # 6a. One Part.Compound written with the plain STEPControl_Writer.
    export_shape(Part.Compound([box_a(), box_b()]), "two_boxes_compound_mm.step")

    # 6b. Two document objects written through OCAF (Import.export).
    doc = App.newDocument("fixtures")
    oa = doc.addObject("Part::Feature", "BoxA")
    oa.Shape = box_a()
    ob = doc.addObject("Part::Feature", "BoxB")
    ob.Shape = Part.makeBox(20, 10, 5)
    ob.Placement = App.Placement(App.Vector(50, 0, 0),
                                 App.Rotation(App.Vector(0, 0, 1), 30))
    doc.recompute()
    Import.export([oa, ob], path("two_boxes_assembly_mm.step"))
    sanitize("two_boxes_assembly_mm.step")
    App.closeDocument(doc.Name)

    # 7. Non-millimetre length units via Part.exportUnits (OCC write.step.unit).
    #    FreeCAD 1.1.4 only relabels the unit: coordinates stay 40/30/10, so
    #    these are a 40x30x10 inch box and a 40x30x10 metre box. Kept as
    #    real-world exporter output; the scaled "same box in other units"
    #    fixtures come from make_units.py.
    export_shape(Part.makeBox(40, 30, 10), "freecad_relabel_inch.step",
                 unit="INCH")
    export_shape(Part.makeBox(40, 30, 10), "freecad_relabel_m.step", unit="M")


if MODE in ("generate", "all"):
    generate()

if MODE in ("verify", "all"):
    for name in sorted(os.listdir(OUT)):
        if name.endswith(".step"):
            results["files"][name] = verify(name)
    if RESULTS:
        with open(RESULTS, "w") as f:
            json.dump(results, f, indent=1)
    print("RCAD_RESULTS " + json.dumps(results))
