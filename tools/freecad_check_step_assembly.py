# Independent check of a hierarchical STEP assembly with FreeCAD/OpenCascade
# (not R-CAD's reader).
# flatpak run --env=RCAD_STEP=<abs path> --filesystem=<dir> --command=FreeCADCmd \
#   org.freecad.FreeCAD tools/freecad_check_step_assembly.py
# Prints "FREECAD_RESULT <json>":
#   shape: the flattened shape OCCT reads (Part.read): solid count, validity,
#          per-solid volumes and centres (world), sorted;
#   tree:  the document tree FreeCAD's importer builds (Import.insert): every
#          object with its type, label, parent chain and global placement, so the
#          product structure (assemblies as App::Part, reused parts as links) is
#          visible.
import os, json
import FreeCAD, Part

path = os.environ["RCAD_STEP"]
r = {"ok": True}
try:
    s = Part.Shape()
    s.read(path)
    r["shape"] = {
        "solids": len(s.Solids),
        "isValid": s.isValid(),
        "volume": s.Volume,
        "solid_volumes": sorted(round(x.Volume, 6) for x in s.Solids),
        "solid_centres": sorted(
            [round(c, 6) for c in (x.CenterOfMass.x, x.CenterOfMass.y, x.CenterOfMass.z)]
            for x in s.Solids
        ),
    }
    import Import
    doc = FreeCAD.newDocument("check")
    Import.insert(path, doc.Name)
    doc.recompute()
    objs = []
    for o in doc.Objects:
        parents = [p.Label for p in o.InList]
        e = {"type": o.TypeId, "label": o.Label, "parents": parents}
        try:
            gp = o.getGlobalPlacement()
            e["global_base"] = [round(gp.Base.x, 6), round(gp.Base.y, 6), round(gp.Base.z, 6)]
        except Exception:
            pass
        if hasattr(o, "LinkedObject") and o.TypeId.startswith("App::Link"):
            lo = o.LinkedObject
            e["links_to"] = lo.Label if lo else None
        if hasattr(o, "Shape") and not o.TypeId.startswith("App::Part"):
            try:
                e["solids"] = len(o.Shape.Solids)
                e["volume"] = round(o.Shape.Volume, 6)
            except Exception:
                pass
        objs.append(e)
    r["tree"] = objs
    r["parts"] = sorted(o.Label for o in doc.Objects if o.TypeId == "App::Part")
    r["links"] = len([o for o in doc.Objects if o.TypeId.startswith("App::Link")])
except Exception as ex:
    r = {"ok": False, "error": str(ex)}
print("FREECAD_RESULT " + json.dumps(r))
