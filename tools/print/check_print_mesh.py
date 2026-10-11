#!/usr/bin/env python3
"""Independent mesh validator for Fission420 print exports (STL, 3MF).

Python 3 stdlib only; shares no code with the Rust exporters. Run isolated:

    python3 -I tools/print/check_print_mesh.py FILE... [--spec spec.json]
        [--json out.json] [--tol 1e-3] [--allow-open | --strict]
        [--circle CX CY R --z-range Z0 Z1 --chord C [--circle-band 0.5]]

What it checks per file:
  STL  binary (84 + 50n bytes) or ASCII; header text; vertices welded by exact
       float32 equality; whole mesh plus each connected shell analysed.
       STL is unitless: coordinates are interpreted as mm.
  3MF  OPC package: _rels/.rels -> model part (fallback 3D/3dmodel.model),
       core-namespace XML, unit -> mm scale, objects, components (recursive,
       cycle-checked; production-extension p:path parts followed), build items
       with 3x4 row-vector transforms (p' = [x y z 1] . M), model metadata.
  Mesh triangles, vertices, bounds/dims (mm), signed volume (divergence
       theorem), area, edge incidence (boundary = 1 use, non-manifold >= 3),
       orientation (the 2 uses of an edge run in opposite directions),
       manifold vertices (single edge-connected fan), degenerate triangles
       (repeated vertex, or area / longest_edge^2 < 1e-12), closed, outward.
       Self-intersection is NOT checked.
  Placement: min z per object and for the plate; min_z_is_zero at 1e-4 mm.

Spec file (optional), matched by basename:
  {"files": {"name.stl": {
      "objects": N, "triangles": N, "unit": "millimeter", "names": [...],
      "volume": V | [V, abs_tol], "rel_volume_tol": r,
      "area": A | [A, abs_tol],
      "dims": [x,y,z] | [[x,y,z], tol], "min_z": z | [z, tol],
      "bounds": [[xmin,ymin,zmin],[xmax,ymax,zmax], tol],
      "closed": true|false,
      "within_bed": [[xmin,ymin],[xmax,ymax], max_height],
      "circle": {"center":[cx,cy], "r":R, "z":[z0,z1], "chord":C, "band":0.5}
                | [ {...}, ... ],
      "per_object": {"<name|id|shellN|index>": { same keys as above minus
                     objects/unit/names/within_bed/per_object }}}}}
  File-level volume/dims/min_z/... refer to the whole file (STL: whole mesh,
  3MF: all build items combined). STL "objects" = number of shells.

Exit: 0 all PASS, 1 any FAIL or malformed/unreadable input, 2 usage error.
"""
import argparse
import json
import math
import os
import posixpath
import struct
import sys
import zipfile
import xml.etree.ElementTree as ET

CORE_NS = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
PROD_NS = "http://schemas.microsoft.com/3dmanufacturing/production/2015/06"
REL_NS = "http://schemas.openxmlformats.org/package/2006/relationships"
MODEL_REL_TYPE = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"
UNIT_TO_MM = {"micron": 0.001, "millimeter": 1.0, "centimeter": 10.0,
              "inch": 25.4, "foot": 304.8, "meter": 1000.0}
MIN_Z_TOL = 1e-4          # mm, min_z_is_zero flag
DEGEN_REL = 1e-12         # area / longest_edge^2
CIRCLE_VERTEX_TOL = 1e-4  # mm, f32 rounding allowance for on-circle vertices
CIRCLE_SAGITTA_SLACK = 1e-5
ANGLE_CLUSTER_TOL = 1e-5  # rad, vertices at the "same angle"
SELF_INTERSECTION_NOTE = "self-intersection NOT checked"
IDENTITY = (1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0)


class InputError(Exception):
    """Malformed or unsupported input file."""


def q(tag):
    return "{%s}%s" % (CORE_NS, tag)


# --------------------------------------------------------------------------
# Small geometry helpers
# --------------------------------------------------------------------------

class UnionFind:
    def __init__(self, items=()):
        self.parent = {}
        for i in items:
            self.parent[i] = i

    def find(self, x):
        p = self.parent
        root = x
        while p[root] != root:
            root = p[root]
        while p[x] != root:
            p[x], x = root, p[x]
        return root

    def union(self, a, b):
        ra, rb = self.find(a), self.find(b)
        if ra != rb:
            self.parent[rb] = ra


def mat_parse(s):
    vals = s.split()
    if len(vals) != 12:
        raise InputError("transform must have 12 numbers, got %d: %r" % (len(vals), s))
    try:
        m = tuple(float(v) for v in vals)
    except ValueError:
        raise InputError("non-numeric transform %r" % s)
    if not all(math.isfinite(v) for v in m):
        raise InputError("non-finite transform %r" % s)
    return m


def mat_mul(a, b):
    """Row-vector composition: p . a . b  ==  p . mat_mul(a, b)."""
    ra = (a[0:3], a[3:6], a[6:9])
    rb = (b[0:3], b[3:6], b[6:9])
    out = []
    for r in ra:
        out.extend(sum(r[k] * rb[k][j] for k in range(3)) for j in range(3))
    t = a[9:12]
    out.extend(sum(t[k] * rb[k][j] for k in range(3)) + b[9 + j] for j in range(3))
    return tuple(out)


def mat_apply(m, p):
    x, y, z = p
    return (x * m[0] + y * m[3] + z * m[6] + m[9],
            x * m[1] + y * m[4] + z * m[7] + m[10],
            x * m[2] + y * m[5] + z * m[8] + m[11])


def mat_det(m):
    return (m[0] * (m[4] * m[8] - m[5] * m[7])
            - m[1] * (m[3] * m[8] - m[5] * m[6])
            + m[2] * (m[3] * m[7] - m[4] * m[6]))


class Welder:
    """Weld vertices by exact coordinate equality."""

    def __init__(self):
        self.index = {}
        self.verts = []
        self.inputs = 0

    def add(self, p):
        self.inputs += 1
        i = self.index.get(p)
        if i is None:
            i = len(self.verts)
            self.index[p] = i
            self.verts.append(p)
        return i


# --------------------------------------------------------------------------
# Loaders. Each returns a dict: format, info, notes, objects=[{name, id, type,
# verts (mm, placed), tris}], whole={verts, tris}
# --------------------------------------------------------------------------

def load_stl(path):
    with open(path, "rb") as f:
        data = f.read()
    info = {"file_size": len(data), "units": "STL is unitless; coordinates interpreted as mm"}
    notes = []
    binary = False
    if len(data) >= 84:
        n = struct.unpack_from("<I", data, 80)[0]
        info["header_text"] = data[:80].split(b"\0", 1)[0].decode("latin-1").rstrip()
        info["declared_triangles"] = n
        info["expected_size"] = 84 + 50 * n
        info["length_consistent"] = len(data) == 84 + 50 * n
        binary = info["length_consistent"]
    stripped = data.lstrip()
    if not binary:
        if stripped[:5].lower() == b"solid":
            info["encoding"] = "ascii"
            welder, tris, name = parse_ascii_stl(data)
            info["solid_name"] = name
            info.pop("declared_triangles", None)
            info.pop("expected_size", None)
            info.pop("length_consistent", None)
            info.pop("header_text", None)
        elif len(data) < 84:
            raise InputError("file too short for binary STL (%d bytes < 84)" % len(data))
        else:
            raise InputError("binary STL length inconsistent: size %d != 84 + 50*%d = %d"
                             % (len(data), info["declared_triangles"], info["expected_size"]))
    else:
        info["encoding"] = "binary"
        if data[:5].lower() == b"solid":
            notes.append("binary STL header begins with 'solid' (some readers misdetect it as ASCII)")
        n = info["declared_triangles"]
        welder = Welder()
        tris = []
        unpack = struct.Struct("<12f").unpack_from
        bad = 0
        attr_nonzero = 0
        for i in range(n):
            off = 84 + 50 * i
            vals = unpack(data, off)
            if data[off + 48:off + 50] != b"\0\0":
                attr_nonzero += 1
            if not all(math.isfinite(v) for v in vals[3:]):
                bad += 1
            tris.append((welder.add(vals[3:6]), welder.add(vals[6:9]), welder.add(vals[9:12])))
        if bad:
            raise InputError("%d triangles have non-finite coordinates" % bad)
        if attr_nonzero:
            notes.append("%d triangles have a non-zero attribute byte count" % attr_nonzero)
    info["triangles"] = len(tris)
    info["welded_vertices"] = len(welder.verts)
    verts = welder.verts
    shells = split_shells(tris)
    objects = []
    for k, st in enumerate(shells):
        objects.append({"name": "shell%d" % (k + 1), "id": str(k), "type": "shell",
                        "verts": verts, "tris": st})
    return {"format": "stl", "info": info, "notes": notes, "objects": objects,
            "whole": {"verts": verts, "tris": tris}}


def parse_ascii_stl(data):
    try:
        text = data.decode("ascii")
    except UnicodeDecodeError:
        raise InputError("not a valid binary STL (length mismatch) and not ASCII text")
    toks = text.split()
    if not toks or toks[0].lower() != "solid":
        raise InputError("ASCII STL must start with 'solid'")
    i = 1
    name_parts = []
    while i < len(toks) and toks[i].lower() not in ("facet", "endsolid"):
        name_parts.append(toks[i])
        i += 1
    welder = Welder()
    tris = []

    def expect(word):
        nonlocal i
        if i >= len(toks) or toks[i].lower() != word:
            raise InputError("ASCII STL: expected %r at token %d, got %r"
                             % (word, i, toks[i] if i < len(toks) else "EOF"))
        i += 1

    def num():
        nonlocal i
        try:
            v = float(toks[i])
        except (ValueError, IndexError):
            raise InputError("ASCII STL: bad number at token %d" % i)
        if not math.isfinite(v):
            raise InputError("ASCII STL: non-finite number at token %d" % i)
        i += 1
        return struct.unpack("<f", struct.pack("<f", v))[0]  # round to f32 like binary

    while i < len(toks) and toks[i].lower() == "facet":
        i += 1
        expect("normal")
        num(); num(); num()
        expect("outer")
        expect("loop")
        t = []
        for _ in range(3):
            expect("vertex")
            t.append(welder.add((num(), num(), num())))
        expect("endloop")
        expect("endfacet")
        tris.append(tuple(t))
    expect("endsolid")
    return welder, tris, " ".join(name_parts)


class ThreeMF:
    def __init__(self, path):
        try:
            self.zf = zipfile.ZipFile(path)
        except zipfile.BadZipFile as e:
            raise InputError("not a zip archive: %s" % e)
        self.names = set(self.zf.namelist())
        self.models = {}
        self.notes = []

    def read_xml(self, part):
        if part not in self.names:
            raise InputError("package part %r missing" % part)
        try:
            return ET.fromstring(self.zf.read(part))
        except ET.ParseError as e:
            raise InputError("XML parse error in %s: %s" % (part, e))

    def root_model_path(self):
        rels = "_rels/.rels"
        target = None
        if rels in self.names:
            root = self.read_xml(rels)
            for r in root.iter("{%s}Relationship" % REL_NS):
                if r.get("Type") == MODEL_REL_TYPE:
                    target = r.get("Target")
                    break
            if target is None:
                self.notes.append("_rels/.rels has no 3dmodel relationship; using fallback 3D/3dmodel.model")
        else:
            self.notes.append("_rels/.rels missing; using fallback 3D/3dmodel.model")
        if target is None:
            return "3D/3dmodel.model"
        return posixpath.normpath(target.lstrip("/"))

    def model(self, part):
        if part in self.models:
            return self.models[part]
        root = self.read_xml(part)
        if root.tag != q("model"):
            raise InputError("%s root element is %s, expected core <model>" % (part, root.tag))
        unit_attr = root.get("unit")
        unit = unit_attr if unit_attr is not None else "millimeter"
        if unit not in UNIT_TO_MM:
            raise InputError("%s: unknown unit %r" % (part, unit))
        m = {"part": part, "unit": unit, "unit_explicit": unit_attr is not None,
             "requiredextensions": root.get("requiredextensions", ""),
             "metadata": {}, "objects": {}, "items": []}
        for md in root.findall(q("metadata")):
            m["metadata"][md.get("name", "")] = (md.text or "").strip()
        res = root.find(q("resources"))
        if res is None:
            raise InputError("%s: no <resources>" % part)
        for obj in res.findall(q("object")):
            oid = obj.get("id")
            if oid is None:
                raise InputError("%s: object without id" % part)
            if oid in m["objects"]:
                raise InputError("%s: duplicate object id %s" % (part, oid))
            o = {"id": oid, "name": obj.get("name"), "type": obj.get("type", "model"),
                 "mesh": None, "components": []}
            mesh = obj.find(q("mesh"))
            comps = obj.find(q("components"))
            if mesh is not None and comps is not None:
                raise InputError("%s: object %s has both mesh and components" % (part, oid))
            if mesh is not None:
                o["mesh"] = self.parse_mesh(part, oid, mesh)
            elif comps is not None:
                for c in comps.findall(q("component")):
                    cid = c.get("objectid")
                    if cid is None:
                        raise InputError("%s: component without objectid in object %s" % (part, oid))
                    tr = mat_parse(c.get("transform")) if c.get("transform") else IDENTITY
                    cpath = c.get("{%s}path" % PROD_NS)
                    cpath = posixpath.normpath(cpath.lstrip("/")) if cpath else part
                    o["components"].append((cid, tr, cpath))
            else:
                raise InputError("%s: object %s has neither mesh nor components" % (part, oid))
            m["objects"][oid] = o
        build = root.find(q("build"))
        if build is not None:
            for it in build.findall(q("item")):
                oid = it.get("objectid")
                if oid is None:
                    raise InputError("%s: build item without objectid" % part)
                tr = mat_parse(it.get("transform")) if it.get("transform") else None
                ipath = it.get("{%s}path" % PROD_NS)
                ipath = posixpath.normpath(ipath.lstrip("/")) if ipath else part
                m["items"].append({"objectid": oid, "transform": tr, "path": ipath,
                                   "partnumber": it.get("partnumber")})
        self.models[part] = m
        return m

    @staticmethod
    def parse_mesh(part, oid, mesh):
        vs = mesh.find(q("vertices"))
        ts = mesh.find(q("triangles"))
        if vs is None or ts is None:
            raise InputError("%s: object %s mesh lacks vertices/triangles" % (part, oid))
        welder = Welder()
        remap = []
        for v in vs.findall(q("vertex")):
            try:
                p = (float(v.get("x")), float(v.get("y")), float(v.get("z")))
            except (TypeError, ValueError):
                raise InputError("%s: object %s has a bad vertex %r" % (part, oid, v.attrib))
            if not all(math.isfinite(c) for c in p):
                raise InputError("%s: object %s has a non-finite vertex" % (part, oid))
            remap.append(welder.add(p))
        n = len(remap)
        tris = []
        for t in ts.findall(q("triangle")):
            try:
                idx = (int(t.get("v1")), int(t.get("v2")), int(t.get("v3")))
            except (TypeError, ValueError):
                raise InputError("%s: object %s has a bad triangle %r" % (part, oid, t.attrib))
            for i in idx:
                if i < 0 or i >= n:
                    raise InputError("%s: object %s triangle index %d out of range (0..%d)"
                                     % (part, oid, i, n - 1))
            tris.append(tuple(remap[i] for i in idx))
        return {"verts": welder.verts, "tris": tris, "raw_vertices": n,
                "duplicate_vertices_merged": n - len(welder.verts)}

    def flatten(self, part, oid, mat, depth, stack, out, info):
        key = (part, oid)
        if key in stack:
            raise InputError("component cycle through object %s in %s" % (oid, part))
        model = self.model(part)
        obj = model["objects"].get(oid)
        if obj is None:
            raise InputError("%s: reference to missing object id %s" % (part, oid))
        info["max_depth"] = max(info["max_depth"], depth)
        if model["unit"] != info["root_unit"]:
            raise InputError("%s unit %r differs from root model unit %r (unsupported)"
                             % (part, model["unit"], info["root_unit"]))
        if obj["mesh"] is not None:
            mesh = obj["mesh"]
            base = len(out["verts"])
            out["verts"].extend(mat_apply(mat, p) for p in mesh["verts"])
            out["tris"].extend((a + base, b + base, c + base) for a, b, c in mesh["tris"])
            info["mesh_objects"].append(oid if part == info["root_part"] else "%s:%s" % (part, oid))
            info["duplicate_vertices_merged"] += mesh["duplicate_vertices_merged"]
            return
        for cid, ctr, cpath in obj["components"]:
            self.flatten(cpath, cid, mat_mul(ctr, mat), depth + 1, stack | {key}, out, info)


def load_3mf(path):
    pkg = ThreeMF(path)
    notes = pkg.notes
    info = {"package_parts": len(pkg.names)}
    if "[Content_Types].xml" not in pkg.names:
        notes.append("[Content_Types].xml missing")
    root_part = pkg.root_model_path()
    info["model_part"] = root_part
    model = pkg.model(root_part)
    info["unit"] = model["unit"]
    info["unit_explicit"] = model["unit_explicit"]
    if not model["unit_explicit"]:
        notes.append("model has no unit attribute; defaulting to millimeter")
    scale = UNIT_TO_MM[model["unit"]]
    info["scale_to_mm"] = scale
    info["requiredextensions"] = model["requiredextensions"]
    info["metadata"] = model["metadata"]
    info["resources"] = [{"id": o["id"], "name": o["name"], "type": o["type"],
                          "kind": "mesh" if o["mesh"] else "components",
                          "components": [{"objectid": c[0], "transform": list(c[1]),
                                          **({"path": c[2]} if c[2] != root_part else {})}
                                         for c in o["components"]]}
                         for o in model["objects"].values()]
    if not model["items"]:
        raise InputError("build has no items")
    objects = []
    items_info = []
    used_names = {}
    for k, it in enumerate(model["items"]):
        tr = it["transform"] or IDENTITY
        out = {"verts": [], "tris": []}
        finfo = {"max_depth": 0, "mesh_objects": [], "root_unit": model["unit"],
                 "root_part": root_part, "duplicate_vertices_merged": 0}
        pkg.flatten(it["path"], it["objectid"], tr, 0, frozenset(), out, finfo)
        if scale != 1.0:
            out["verts"] = [(x * scale, y * scale, z * scale) for x, y, z in out["verts"]]
        obj = pkg.model(it["path"])["objects"][it["objectid"]]
        base_name = obj["name"] or "object%s" % obj["id"]
        cnt = used_names.get(base_name, 0) + 1
        used_names[base_name] = cnt
        name = base_name if cnt == 1 else "%s#%d" % (base_name, cnt)
        det = mat_det(tr)
        ii = {"index": k, "objectid": it["objectid"], "name": name, "type": obj["type"],
              "transform": list(it["transform"]) if it["transform"] else None,
              "transform_det": det, "component_depth": finfo["max_depth"],
              "mesh_objects": finfo["mesh_objects"],
              "duplicate_vertices_merged": finfo["duplicate_vertices_merged"]}
        if it["path"] != root_part:
            ii["path"] = it["path"]
        if it["partnumber"]:
            ii["partnumber"] = it["partnumber"]
        items_info.append(ii)
        if det < 0:
            notes.append("item %d (%s) transform mirrors (det %.6g)" % (k, name, det))
        if abs(det) < 1e-12:
            notes.append("item %d (%s) transform is singular" % (k, name))
        if finfo["max_depth"] > 1:
            notes.append("item %d (%s) uses nested components (depth %d)" % (k, name, finfo["max_depth"]))
        if obj["type"] not in ("model",):
            notes.append("item %d (%s) object type %r" % (k, name, obj["type"]))
        objects.append({"name": name, "id": it["objectid"], "type": obj["type"],
                        "verts": out["verts"], "tris": out["tris"]})
    info["build_items"] = items_info
    wv, wt = [], []
    for o in objects:
        base = len(wv)
        wv.extend(o["verts"])
        wt.extend((a + base, b + base, c + base) for a, b, c in o["tris"])
    return {"format": "3mf", "info": info, "notes": notes, "objects": objects,
            "whole": {"verts": wv, "tris": wt}}


# --------------------------------------------------------------------------
# Analysis
# --------------------------------------------------------------------------

def split_shells(tris):
    uf = UnionFind({i for t in tris for i in t})
    for a, b, c in tris:
        uf.union(a, b)
        uf.union(a, c)
    groups = {}
    order = []
    for t in tris:
        r = uf.find(t[0])
        if r not in groups:
            groups[r] = []
            order.append(r)
        groups[r].append(t)
    return [groups[r] for r in order]


def analyze_mesh(verts, tris, max_examples=5):
    res = {"triangles": len(tris)}
    used = {i for t in tris for i in t}
    res["vertices"] = len(used)
    if not tris:
        res.update(empty=True, closed=False, manifold=False, outward=False)
        return res
    xs = [verts[i][0] for i in used]
    ys = [verts[i][1] for i in used]
    zs = [verts[i][2] for i in used]
    bmin = [min(xs), min(ys), min(zs)]
    bmax = [max(xs), max(ys), max(zs)]
    res["bounds_min"] = bmin
    res["bounds_max"] = bmax
    res["dims"] = [bmax[k] - bmin[k] for k in range(3)]
    res["min_z"] = bmin[2]
    res["max_z"] = bmax[2]
    res["min_z_is_zero"] = abs(bmin[2]) <= MIN_Z_TOL

    ox, oy, oz = verts[tris[0][0]]
    vol6 = 0.0
    area2 = 0.0
    degen_repeat = 0
    degen_zero = 0
    degen_examples = []
    edges = {}
    vtris = {}
    for ti, (a, b, c) in enumerate(tris):
        if a == b or b == c or a == c:
            degen_repeat += 1
            if len(degen_examples) < max_examples:
                degen_examples.append({"triangle": ti, "reason": "repeated vertex",
                                       "points": [verts[a], verts[b], verts[c]]})
        else:
            ax, ay, az = verts[a]
            bx, by, bz = verts[b]
            cx, cy, cz = verts[c]
            pax, pay, paz = ax - ox, ay - oy, az - oz
            pbx, pby, pbz = bx - ox, by - oy, bz - oz
            pcx, pcy, pcz = cx - ox, cy - oy, cz - oz
            vol6 += (pax * (pby * pcz - pbz * pcy)
                     + pay * (pbz * pcx - pbx * pcz)
                     + paz * (pbx * pcy - pby * pcx))
            e1x, e1y, e1z = bx - ax, by - ay, bz - az
            e2x, e2y, e2z = cx - ax, cy - ay, cz - az
            crx = e1y * e2z - e1z * e2y
            cry = e1z * e2x - e1x * e2z
            crz = e1x * e2y - e1y * e2x
            crn = math.sqrt(crx * crx + cry * cry + crz * crz)
            area2 += crn
            l2 = max(e1x * e1x + e1y * e1y + e1z * e1z,
                     e2x * e2x + e2y * e2y + e2z * e2z,
                     (cx - bx) ** 2 + (cy - by) ** 2 + (cz - bz) ** 2)
            if 0.5 * crn <= DEGEN_REL * l2:
                degen_zero += 1
                if len(degen_examples) < max_examples:
                    degen_examples.append({"triangle": ti, "reason": "zero area",
                                           "points": [verts[a], verts[b], verts[c]]})
        for u, v in ((a, b), (b, c), (c, a)):
            if u == v:
                continue
            if u < v:
                e = edges.get((u, v))
                if e is None:
                    edges[(u, v)] = [1, 0]
                else:
                    e[0] += 1
            else:
                e = edges.get((v, u))
                if e is None:
                    edges[(v, u)] = [0, 1]
                else:
                    e[1] += 1
        if not (a == b or b == c or a == c):
            for v in (a, b, c):
                vtris.setdefault(v, []).append(ti)

    boundary = nonmanifold = misoriented = 0
    ex_b, ex_n, ex_m = [], [], []
    for (u, v), (f, bk) in edges.items():
        tot = f + bk
        if tot == 1:
            boundary += 1
            if len(ex_b) < max_examples:
                ex_b.append([verts[u], verts[v]])
        elif tot >= 3:
            nonmanifold += 1
            if len(ex_n) < max_examples:
                ex_n.append({"edge": [verts[u], verts[v]], "uses": tot})
        elif f != 1:
            misoriented += 1
            if len(ex_m) < max_examples:
                ex_m.append([verts[u], verts[v]])

    nm_verts = 0
    ex_v = []
    for v, tl in vtris.items():
        if len(tl) < 2:
            continue
        uf = UnionFind(tl)
        by_w = {}
        for t in tl:
            for w in tris[t]:
                if w != v:
                    by_w.setdefault(w, []).append(t)
        for lst in by_w.values():
            for t in lst[1:]:
                uf.union(lst[0], t)
        roots = {uf.find(t) for t in tl}
        if len(roots) > 1:
            nm_verts += 1
            if len(ex_v) < max_examples:
                ex_v.append({"vertex": verts[v], "fans": len(roots)})

    volume = vol6 / 6.0
    res["volume"] = volume
    res["area"] = area2 / 2.0
    res["edges"] = len(edges)
    res["boundary_edges"] = boundary
    res["nonmanifold_edges"] = nonmanifold
    res["misoriented_edges"] = misoriented
    res["nonmanifold_vertices"] = nm_verts
    res["degenerate_triangles"] = degen_repeat + degen_zero
    res["degenerate_repeated_vertex"] = degen_repeat
    res["degenerate_zero_area"] = degen_zero
    res["orientation_consistent"] = misoriented == 0
    res["closed"] = boundary == 0 and nonmanifold == 0 and misoriented == 0
    res["manifold"] = res["closed"] and nm_verts == 0
    res["outward"] = volume > 0
    res["shells"] = len(split_shells(tris))
    # Euler characteristic V - E + F (2 per sphere-like closed shell)
    res["euler_characteristic"] = len(used) - len(edges) + len(tris)
    ex = {}
    if ex_b:
        ex["boundary_edges"] = ex_b
    if ex_n:
        ex["nonmanifold_edges"] = ex_n
    if ex_m:
        ex["misoriented_edges"] = ex_m
    if ex_v:
        ex["nonmanifold_vertices"] = ex_v
    if degen_examples:
        ex["degenerate_triangles"] = degen_examples
    if ex:
        res["examples"] = ex
    return res


def circle_check(verts, tris, cx, cy, r, z0, z1, chord, band=0.5):
    """Tessellation accuracy of a circular feature (axis parallel to z)."""
    zlo, zhi = min(z0, z1) - 1e-6, max(z0, z1) + 1e-6
    sel = {}
    used = {i for t in tris for i in t}
    for i in used:
        x, y, z = verts[i]
        if not (zlo <= z <= zhi):
            continue
        rd = math.hypot(x - cx, y - cy)
        if abs(rd - r) <= band:
            sel[i] = (rd, math.atan2(y - cy, x - cx))
    res = {"center": [cx, cy], "r": r, "z": [z0, z1], "chord": chord, "band": band,
           "selected_vertices": len(sel)}
    if not sel:
        res.update(ok=False, error="no vertices within band of the circle")
        return res
    res["max_vertex_error"] = max(abs(rd - r) for rd, _ in sel.values())
    # cluster angles
    angs = sorted({a for _, a in sel.values()})
    cluster_of = {}
    reps = []
    for a in angs:
        if reps and a - reps[-1][-1] <= ANGLE_CLUSTER_TOL:
            reps[-1].append(a)
        else:
            reps.append([a])
        cluster_of[a] = len(reps) - 1
    if len(reps) > 1 and (reps[0][0] + 2 * math.pi) - reps[-1][-1] <= ANGLE_CLUSTER_TOL:
        for a in reps[-1]:
            cluster_of[a] = 0
        reps[0] = [a - 2 * math.pi for a in reps[-1]] + reps[0]
        reps.pop()
    n = len(reps)
    centers = [sum(c) / len(c) for c in reps]
    gaps = []
    for k in range(n):
        g = centers[(k + 1) % n] - centers[k]
        if k == n - 1:
            g += 2 * math.pi
        gaps.append(g)
    res["distinct_angles"] = n
    if n > 1:
        res["min_angle_gap_deg"] = math.degrees(min(gaps))
        res["max_angle_gap_deg"] = math.degrees(max(gaps))
    # segments at each z level
    per_z = {}
    for i, (rd, a) in sel.items():
        per_z.setdefault(round(verts[i][2], 6), set()).add(cluster_of[a])
    zbest = max(per_z, key=lambda z: (len(per_z[z]), -abs(z)))
    res["segments"] = len(per_z[zbest])
    res["segments_at_z"] = zbest
    res["z_levels"] = len(per_z)
    # edges between angularly adjacent (or same-angle) selected vertices
    seen = set()
    max_sag = None
    worst = None
    evaluated = skipped = 0
    for a, b, c in tris:
        for u, v in ((a, b), (b, c), (c, a)):
            if u == v or u not in sel or v not in sel:
                continue
            k = (u, v) if u < v else (v, u)
            if k in seen:
                continue
            seen.add(k)
            cu, cv = cluster_of[sel[u][1]], cluster_of[sel[v][1]]
            d = (cv - cu) % n if n else 0
            if d not in (0, 1, n - 1):
                skipped += 1
                continue
            evaluated += 1
            mx = (verts[u][0] + verts[v][0]) / 2.0
            my = (verts[u][1] + verts[v][1]) / 2.0
            sag = r - math.hypot(mx - cx, my - cy)
            if max_sag is None or sag > max_sag:
                max_sag = sag
                worst = [verts[u], verts[v]]
    res["edges_evaluated"] = evaluated
    res["edges_skipped_non_adjacent"] = skipped
    if max_sag is None:
        res.update(ok=False, error="no adjacent-angle edges between selected vertices")
        return res
    res["max_sagitta"] = max_sag
    res["max_sagitta_edge"] = worst
    if n > 1:
        res["polygon_sagitta_from_max_gap"] = r * (1 - math.cos(max(gaps) / 2))
    res["sagitta_ok"] = max_sag <= chord + CIRCLE_SAGITTA_SLACK
    res["vertex_error_ok"] = res["max_vertex_error"] <= CIRCLE_VERTEX_TOL
    res["ok"] = res["sagitta_ok"] and res["vertex_error_ok"]
    return res


# --------------------------------------------------------------------------
# Reporting / assertions
# --------------------------------------------------------------------------

class Reporter:
    def __init__(self):
        self.fail = 0
        self.passed = 0
        self.warn = 0

    def line(self, status, file_label, what, detail, sink):
        if status == "FAIL":
            self.fail += 1
        elif status == "PASS":
            self.passed += 1
        elif status == "WARN":
            self.warn += 1
        print("  %-4s %s: %s" % (status, what, detail))
        sink.append({"status": status, "file": file_label, "check": what, "detail": detail})


def split_val(v, default_tol):
    """value | [value, tol] -> (value, tol)."""
    if isinstance(v, list) and len(v) == 2 and isinstance(v[1], (int, float)) \
            and (isinstance(v[0], (int, float, list))):
        if isinstance(v[0], list) or not isinstance(v[0], bool):
            return v[0], float(v[1])
    return v, default_tol


def fmt(x):
    if isinstance(x, float):
        return "%.6g" % x
    if isinstance(x, (list, tuple)):
        return "[" + ", ".join(fmt(e) for e in x) + "]"
    return str(x)


def assert_mesh_spec(rep, sink, label, scope, a, spec, tol, verts=None, tris=None):
    """Assertions common to file-level and per-object specs."""
    pre = scope + " "
    if "triangles" in spec:
        rep.line("PASS" if a["triangles"] == spec["triangles"] else "FAIL", label,
                 pre + "triangles", "%d (expected %d)" % (a["triangles"], spec["triangles"]), sink)
    if "closed" in spec:
        ok = bool(a.get("closed")) == bool(spec["closed"])
        rep.line("PASS" if ok else "FAIL", label, pre + "closed",
                 "%s (expected %s)" % (a.get("closed"), spec["closed"]), sink)
    if a.get("empty"):
        for k in ("volume", "area", "dims", "min_z", "bounds"):
            if k in spec:
                rep.line("FAIL", label, pre + k, "mesh is empty", sink)
        return
    if "volume" in spec:
        exp, t = split_val(spec["volume"], None)
        if "rel_volume_tol" in spec:
            rt = float(spec["rel_volume_tol"]) * abs(exp)
            t = rt if t is None else max(t, rt)
        if t is None:
            t = 1e-6 * max(1.0, abs(exp))
        d = abs(a["volume"] - exp)
        rep.line("PASS" if d <= t else "FAIL", label, pre + "volume",
                 "%.6f (expected %s +/- %s, diff %.3g)" % (a["volume"], fmt(exp), fmt(t), d), sink)
    if "area" in spec:
        exp, t = split_val(spec["area"], None)
        if t is None:
            t = 1e-6 * max(1.0, abs(exp))
        d = abs(a["area"] - exp)
        rep.line("PASS" if d <= t else "FAIL", label, pre + "area",
                 "%.6f (expected %s +/- %s, diff %.3g)" % (a["area"], fmt(exp), fmt(t), d), sink)
    if "dims" in spec:
        exp, t = split_val(spec["dims"], tol)
        d = max(abs(a["dims"][k] - exp[k]) for k in range(3))
        rep.line("PASS" if d <= t else "FAIL", label, pre + "dims",
                 "%s (expected %s +/- %s, max diff %.3g)" % (fmt(a["dims"]), fmt(exp), fmt(t), d), sink)
    if "min_z" in spec:
        exp, t = split_val(spec["min_z"], MIN_Z_TOL)
        d = abs(a["min_z"] - exp)
        rep.line("PASS" if d <= t else "FAIL", label, pre + "min_z",
                 "%.6g (expected %s +/- %s)" % (a["min_z"], fmt(exp), fmt(t)), sink)
    if "bounds" in spec:
        b = spec["bounds"]
        t = float(b[2]) if len(b) > 2 else tol
        d = max(max(abs(a["bounds_min"][k] - b[0][k]), abs(a["bounds_max"][k] - b[1][k]))
                for k in range(3))
        rep.line("PASS" if d <= t else "FAIL", label, pre + "bounds",
                 "%s..%s (expected %s..%s +/- %s)" % (fmt(a["bounds_min"]), fmt(a["bounds_max"]),
                                                     fmt(b[0]), fmt(b[1]), fmt(t)), sink)
    circles = spec.get("circle")
    if circles is not None and verts is not None:
        if isinstance(circles, dict):
            circles = [circles]
        for c in circles:
            run_circle(rep, sink, label, scope, verts, tris, c["center"][0], c["center"][1],
                       c["r"], c["z"][0], c["z"][1], c["chord"], c.get("band", 0.5), a)


def run_circle(rep, sink, label, scope, verts, tris, cx, cy, r, z0, z1, chord, band, a):
    cr = circle_check(verts, tris, cx, cy, r, z0, z1, chord, band)
    a.setdefault("circle_checks", []).append(cr)
    what = "%s circle c=(%s,%s) r=%s z=[%s,%s]" % (scope, fmt(cx), fmt(cy), fmt(r), fmt(z0), fmt(z1))
    if "error" in cr:
        rep.line("FAIL", label, what, cr["error"], sink)
        return
    print("       segments %d at z=%s (distinct angles %d, z levels %d), edges evaluated %d"
          % (cr["segments"], fmt(cr["segments_at_z"]), cr["distinct_angles"], cr["z_levels"],
             cr["edges_evaluated"]))
    if "polygon_sagitta_from_max_gap" in cr:
        print("       polygon sagitta from max angle gap %.3f deg: %.9g"
              % (cr["max_angle_gap_deg"], cr["polygon_sagitta_from_max_gap"]))
    rep.line("PASS" if cr["sagitta_ok"] else "FAIL", label, what + " max sagitta",
             "%.9g (limit chord %s + %g)" % (cr["max_sagitta"], fmt(chord), CIRCLE_SAGITTA_SLACK), sink)
    rep.line("PASS" if cr["vertex_error_ok"] else "FAIL", label, what + " max vertex error",
             "%.3g (limit %g)" % (cr["max_vertex_error"], CIRCLE_VERTEX_TOL), sink)


def print_analysis(name, a):
    if a.get("empty"):
        print("    %s: EMPTY mesh" % name)
        return
    print("    %s: %d tris, %d verts, %d shell(s)" % (name, a["triangles"], a["vertices"], a["shells"]))
    print("      bounds %s .. %s  dims %s mm" % (fmt(a["bounds_min"]), fmt(a["bounds_max"]), fmt(a["dims"])))
    print("      volume %.6f mm^3  area %.6f mm^2  min_z %.6g (min_z_is_zero=%s)"
          % (a["volume"], a["area"], a["min_z"], a["min_z_is_zero"]))
    print("      edges %d: boundary %d, non-manifold %d, misoriented %d; non-manifold vertices %d; "
          "degenerate %d (repeated %d, zero-area %d)"
          % (a["edges"], a["boundary_edges"], a["nonmanifold_edges"], a["misoriented_edges"],
             a["nonmanifold_vertices"], a["degenerate_triangles"], a["degenerate_repeated_vertex"],
             a["degenerate_zero_area"]))
    print("      closed=%s manifold=%s orientation_consistent=%s outward=%s euler=%d"
          % (a["closed"], a["manifold"], a["orientation_consistent"], a["outward"],
             a["euler_characteristic"]))


def mesh_quality(rep, sink, label, scope, a, required, allow_negative=False):
    """Strict mesh checks: FAIL when required, WARN otherwise."""
    st = "FAIL" if required else "WARN"
    if a.get("empty"):
        rep.line(st, label, scope + " mesh", "empty", sink)
        return
    problems = []
    if a["boundary_edges"]:
        problems.append("%d boundary edges (open)" % a["boundary_edges"])
    if a["nonmanifold_edges"]:
        problems.append("%d non-manifold edges" % a["nonmanifold_edges"])
    if a["misoriented_edges"]:
        problems.append("%d misoriented edges" % a["misoriented_edges"])
    if a["nonmanifold_vertices"]:
        problems.append("%d non-manifold vertices" % a["nonmanifold_vertices"])
    if not allow_negative and a["volume"] <= 0:
        problems.append("non-positive volume %.6g (inward orientation)" % a["volume"])
    rep.line(st if problems else "PASS", label, scope + " mesh closed/manifold/outward",
             "; ".join(problems) if problems else "ok", sink)
    if a["degenerate_triangles"]:
        rep.line("WARN", label, scope + " degenerate triangles", str(a["degenerate_triangles"]), sink)


def find_object(objs, key):
    for o in objs:
        if o["name"] == key:
            return o
    for o in objs:
        if o["id"] == str(key):
            return o
    if str(key).isdigit() and int(key) < len(objs):
        return objs[int(key)]
    return None


def process_file(path, fspec, args, rep):
    label = os.path.basename(path)
    sink = []
    out = {"file": path, "basename": label, "checks": sink,
           "self_intersection": SELF_INTERSECTION_NOTE}
    print("== %s" % path)
    ext = os.path.splitext(path)[1].lower()
    try:
        if ext == ".stl":
            model = load_stl(path)
        elif ext == ".3mf":
            model = load_3mf(path)
        else:
            raise InputError("unsupported extension %r (expected .stl or .3mf)" % ext)
    except (InputError, OSError, zipfile.BadZipFile, KeyError, struct.error) as e:
        rep.line("FAIL", label, "read", "malformed/unreadable: %s" % e, sink)
        out["malformed"] = str(e)
        return out
    out["format"] = model["format"]
    out["info"] = model["info"]
    out["notes"] = model["notes"]
    info = model["info"]
    if model["format"] == "stl":
        print("  STL %s; %s" % (info["encoding"], info["units"]))
        if info["encoding"] == "binary":
            print("  header: %r" % info["header_text"])
            print("  triangles %d, size %d = 84+50n: %s" % (info["declared_triangles"],
                                                          info["file_size"], info["length_consistent"]))
        else:
            print("  solid name: %r, triangles %d" % (info["solid_name"], info["triangles"]))
        print("  welded vertices (exact f32 equality): %d" % info["welded_vertices"])
    else:
        print("  3MF model part %s; unit %s%s (x%s to mm); requiredextensions=%r"
              % (info["model_part"], info["unit"], "" if info["unit_explicit"] else " (DEFAULTED)",
                 fmt(info["scale_to_mm"]), info["requiredextensions"]))
        for k, v in info["metadata"].items():
            print("  metadata %s = %r" % (k, v))
        for r in info["resources"]:
            print("  resource object id=%s name=%r type=%s %s" % (r["id"], r["name"], r["type"], r["kind"]))
        for it in info["build_items"]:
            print("  build item %d: objectid=%s name=%r transform=%s det=%.6g depth=%d"
                  % (it["index"], it["objectid"], it["name"],
                     fmt(it["transform"]) if it["transform"] else "identity",
                     it["transform_det"], it["component_depth"]))
    for n in model["notes"]:
        print("  NOTE %s" % n)
    print("  NOTE %s" % SELF_INTERSECTION_NOTE)

    whole = model["whole"]
    wa = analyze_mesh(whole["verts"], whole["tris"])
    out["whole"] = wa
    print("  whole %s:" % ("mesh" if model["format"] == "stl" else "plate"))
    print_analysis("all", wa)
    objs = []
    print("  objects (%s): %d" % ("shells" if model["format"] == "stl" else "build items", len(model["objects"])))
    for o in model["objects"]:
        a = analyze_mesh(o["verts"], o["tris"])
        a["name"] = o["name"]
        a["id"] = o["id"]
        a["type"] = o["type"]
        objs.append(a)
        print_analysis("%s (id %s)" % (o["name"], o["id"]), a)
    out["objects"] = objs
    if not wa.get("empty"):
        out["plate_min_z"] = wa["min_z"]
        out["plate_min_z_is_zero"] = wa["min_z_is_zero"]
        print("  plate min z %.6g (min_z_is_zero=%s); per object: %s"
              % (wa["min_z"], wa["min_z_is_zero"],
                 ", ".join("%s=%s" % (a["name"], fmt(a.get("min_z"))) for a in objs)))

    spec = fspec or {}
    strict = args.strict or bool(spec.get("closed"))
    # strict mesh quality
    if model["format"] == "stl":
        mesh_quality(rep, sink, label, "whole", wa, strict)
        for a in objs:
            if not a.get("empty") and a["volume"] < 0 and wa.get("closed"):
                rep.line("WARN", label, a["name"] + " volume",
                         "negative (%.6g): inward shell, possibly an internal void" % a["volume"], sink)
    else:
        for a in objs:
            req = strict and a["type"] in ("model", "solidsupport")
            mesh_quality(rep, sink, label, a["name"], a, req)

    # spec assertions
    if "unit" in spec:
        u = info.get("unit", "(stl: none, mm assumed)")
        rep.line("PASS" if u == spec["unit"] else "FAIL", label, "unit",
                 "%s (expected %s)" % (u, spec["unit"]), sink)
    if "objects" in spec:
        rep.line("PASS" if len(objs) == spec["objects"] else "FAIL", label, "objects",
                 "%d (expected %d)" % (len(objs), spec["objects"]), sink)
    if "names" in spec:
        got = [a["name"] for a in objs]
        ok = sorted(got) == sorted(spec["names"])
        rep.line("PASS" if ok else "FAIL", label, "names", "%s (expected %s)" % (got, spec["names"]), sink)
    assert_mesh_spec(rep, sink, label, "file", wa, spec, args.tol, whole["verts"], whole["tris"])
    for key, ospec in spec.get("per_object", {}).items():
        o = find_object(model["objects"], key)
        if o is None:
            rep.line("FAIL", label, "object %s" % key, "not found", sink)
            continue
        a = next(x for x in objs if x["name"] == o["name"])
        assert_mesh_spec(rep, sink, label, "object " + o["name"], a, ospec, args.tol, o["verts"], o["tris"])
    if "within_bed" in spec:
        (bx0, by0), (bx1, by1), hmax = spec["within_bed"]
        t = args.tol
        for a in objs:
            if a.get("empty"):
                continue
            bmin, bmax = a["bounds_min"], a["bounds_max"]
            ok = (bmin[0] >= bx0 - t and bmin[1] >= by0 - t and bmax[0] <= bx1 + t
                  and bmax[1] <= by1 + t and bmax[2] <= hmax + t and bmin[2] >= -t)
            rep.line("PASS" if ok else "FAIL", label, "%s within bed" % a["name"],
                     "%s..%s in [%s,%s]..[%s,%s] z<=%s" % (fmt(bmin), fmt(bmax), fmt(bx0), fmt(by0),
                                                         fmt(bx1), fmt(by1), fmt(hmax)), sink)
    if args.circle:
        cx, cy, r = args.circle
        z0, z1 = args.z_range
        run_circle(rep, sink, label, "file", whole["verts"], whole["tris"], cx, cy, r, z0, z1,
                   args.chord, args.circle_band, wa)
    return out


def main(argv=None):
    ap = argparse.ArgumentParser(description="Independent STL/3MF print-mesh validator.")
    ap.add_argument("files", nargs="+")
    ap.add_argument("--spec", help="JSON spec of expected values")
    ap.add_argument("--json", help="write the full report to this JSON file")
    ap.add_argument("--tol", type=float, default=1e-3, help="default length tolerance (mm), default 1e-3")
    g = ap.add_mutually_exclusive_group()
    g.add_argument("--strict", dest="strict", action="store_true", default=True,
                   help="open/non-manifold/inward meshes FAIL (default)")
    g.add_argument("--allow-open", dest="strict", action="store_false",
                   help="mesh-quality problems only WARN unless the spec says closed: true")
    ap.add_argument("--circle", nargs=3, type=float, metavar=("CX", "CY", "R"))
    ap.add_argument("--z-range", nargs=2, type=float, metavar=("Z0", "Z1"))
    ap.add_argument("--chord", type=float, help="tessellation chord (sagitta) tolerance, mm")
    ap.add_argument("--circle-band", type=float, default=0.5,
                    help="radial band around R for vertex selection, mm (default 0.5)")
    args = ap.parse_args(argv)
    if args.circle and (args.z_range is None or args.chord is None):
        ap.error("--circle requires --z-range and --chord")
    if not args.circle and (args.z_range is not None or args.chord is not None):
        ap.error("--z-range/--chord require --circle")
    spec = {}
    if args.spec:
        try:
            with open(args.spec) as f:
                spec = json.load(f)
        except (OSError, ValueError) as e:
            print("error: cannot read spec %s: %s" % (args.spec, e), file=sys.stderr)
            return 2
        if not isinstance(spec, dict) or not isinstance(spec.get("files", {}), dict):
            print("error: spec must be {\"files\": {basename: {...}}}", file=sys.stderr)
            return 2
    fspecs = spec.get("files", {})
    rep = Reporter()
    report = {"tool": "check_print_mesh.py", "self_intersection": SELF_INTERSECTION_NOTE,
              "strict": args.strict, "files": []}
    for p in args.files:
        try:
            report["files"].append(process_file(p, fspecs.get(os.path.basename(p)), args, rep))
        except (KeyError, TypeError, ValueError, IndexError) as e:
            # malformed spec entry for this file
            print("  FAIL spec: bad spec entry for %s: %r" % (os.path.basename(p), e))
            rep.fail += 1
            report["files"].append({"file": p, "spec_error": repr(e)})
    given = {os.path.basename(p) for p in args.files}
    for name in fspecs:
        if name not in given:
            print("WARN spec entry %s not among inputs" % name)
            rep.warn += 1
    print("SUMMARY: %d PASS, %d FAIL, %d WARN" % (rep.passed, rep.fail, rep.warn))
    report["summary"] = {"pass": rep.passed, "fail": rep.fail, "warn": rep.warn}
    if args.json:
        with open(args.json, "w") as f:
            json.dump(report, f, indent=1)
    return 1 if rep.fail else 0


if __name__ == "__main__":
    sys.exit(main())
