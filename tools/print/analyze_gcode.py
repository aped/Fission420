#!/usr/bin/env python3
"""Analyse Bambu Studio G-code (plain .gcode or a sliced .gcode.3mf) and check geometric probes.

Python 3 stdlib only; run as `python3 -I tools/print/analyze_gcode.py ...`.

Usage:
  analyze_gcode.py <file.gcode|file.gcode.3mf> [--plate N] [--probes spec.json]
                   [--log slice.log] [--out result.json] [--layers]
Exit status: 0 all probes pass, 1 a probe failed (or --strict-log and the log has errors),
2 usage/parse error.

What the parser relies on (verified on Bambu Studio 2.8.2.61 output, see
artifacts/run4/print/slicer/profiles.md):
  - `; HEADER_BLOCK_START` .. `END`: "; model printing time: ..; total estimated time: ..",
    "; total layer number: N", "; total filament length [mm] : ..", "; total filament weight [g] : ..",
    "; max_z_height: ..". (The "volume [cm^3]" figure is mm^3 despite the label; reported raw.)
  - `; CONFIG_BLOCK_START` .. `END`: "; key = value" (line widths, extruder_offset, ...).
  - Layer start: "; CHANGE_LAYER" then "; Z_HEIGHT: z" and "; LAYER_HEIGHT: h". Bambu does NOT
    emit ";LAYER_CHANGE" or ";Z:" (those are Prusa/Orca). Anything before the first CHANGE_LAYER
    (machine start G-code incl. the nozzle load line) is excluded from layers.
  - "; FEATURE: <name>" (Outer wall, Inner wall, Bottom surface, Sparse infill, Custom, ...)
    and "; LINE_WIDTH: w" per feature: w is used as the rasterisation width.
  - "; OBJECT_ID: n" before each object's moves on a layer. M624/M625 or EXCLUDE_OBJECT labels
    are only emitted when label_object is enabled (it is not with the CLI defaults:
    slice_info.config label_object_enabled=false); M624 is parsed if present.
  - Moves: G0/G1 and G2/G3 arcs (arc fitting is on by default; arcs are linearised at
    <=0.1 mm chords; helical z-hops "G3 Z.. I.. J.. P1" are travel). M82/M83, G90/G91, G92.
  - "; MACHINE_END_GCODE_START" ends the object section.
  An extrusion = a move with positive E and XY motion, inside a layer, feature != Custom.

Coordinate frames (probe "frame"):
  "plate"  (default) the build-plate frame the model/3MF is placed in, i.e. the frame of
           our exported 3MF and of Metadata/plate_N.json bbox. Computed as
           gcode_xy + extruder_offset (Bambu writes gcode = plate - extruder_offset; the P1S
           profile has extruder_offset 0x2, so G-code Y is 2 mm less than plate Y).
  "gcode"  raw G-code coordinates.
  "model_bbox"  xy are offsets from the min corner of the object's exact model bbox taken from
           Metadata/plate_N.json (3MF input only). Optional "object": name or index (default:
           union of all objects). Best frame: independent of where the slicer placed the part.
  "extrusion_bbox"  xy are offsets from the min corner of all extrusion in the first
           `bbox_layers` layers (default 3; optional "object_id" = G-code OBJECT_ID). Works on
           plain .gcode; note the first-layer outer wall centreline sits ~w/2 + elefant_foot
           (0.25 + 0.15 mm) inside the model edge, so this origin is ~0.4 mm inside model_bbox.
Probe z: "z" selects the layer whose slab (Z_HEIGHT - LAYER_HEIGHT, Z_HEIGHT] contains z;
"z_range": [z0, z1] selects every layer with z0 <= Z_HEIGHT <= z1.

Probe kinds (spec: {"probes":[...]}; each may carry "name"):
  extrusion_near       {"z", "xy":[x,y], "radius"}: some extrusion centreline within radius.
  no_extrusion_in_rect {"z"|"z_range", "rect":[x0,y0,x1,y1], "margin"?:0}: no extrusion
                       centreline passes through rect shrunk by margin, on every selected layer.
  filled_rect          {"z"|"z_range", "rect", "min_coverage", "mode"?:"each"|"union"|"mean",
                       "cell"?:0.25}: rasterises extrusions at their LINE_WIDTH onto a grid of
                       cell-centre samples inside rect; coverage = covered/total. "each": every
                       layer >= min; "union": cells covered by any selected layer; "mean".
  layer_count          {"min"?, "max"?, "equals"?}
  max_z                {"value", "tol"?:1e-3}: top layer Z_HEIGHT.
Also reports mesh_stat repair counters from Metadata/model_settings.config (3MF) and, with
--log, slicer-log findings (see scan_log()).
"""
import argparse
import json
import math
import os
import re
import sys
import zipfile

# ----------------------------------------------------------------------------- input

def load_input(path, plate=None):
    """Return (gcode_text, meta) where meta holds 3MF side files if present."""
    meta = {"source": os.path.abspath(path)}
    if zipfile.is_zipfile(path):
        z = zipfile.ZipFile(path)
        names = z.namelist()
        plates = sorted(n for n in names if re.fullmatch(r"Metadata/plate_\d+\.gcode", n))
        if not plates:
            raise SystemExit(f"{path}: no Metadata/plate_N.gcode inside")
        name = f"Metadata/plate_{plate}.gcode" if plate else plates[0]
        if name not in names:
            raise SystemExit(f"{path}: {name} not found (have {plates})")
        meta["plate_gcode"] = name
        n = re.search(r"plate_(\d+)", name).group(1)
        pj = f"Metadata/plate_{n}.json"
        if pj in names:
            meta["plate_json"] = json.loads(z.read(pj).decode("utf-8"))
        if "Metadata/model_settings.config" in names:
            meta["mesh_stat"] = parse_mesh_stat(z.read("Metadata/model_settings.config").decode("utf-8"))
        if "Metadata/slice_info.config" in names:
            si = z.read("Metadata/slice_info.config").decode("utf-8")
            meta["slice_info"] = dict(re.findall(r'<metadata key="([^"]+)" value="([^"]*)"', si))
        return z.read(name).decode("utf-8", "replace"), meta
    with open(path, encoding="utf-8", errors="replace") as f:
        return f.read(), meta


def parse_mesh_stat(xml):
    out = []
    for m in re.finditer(r'<part id="(\d+)"[^>]*>(.*?)</part>', xml, re.S):
        name = re.search(r'<metadata key="name" value="([^"]*)"', m.group(2))
        st = re.search(r"<mesh_stat ([^/]*)/>", m.group(2))
        if st:
            d = {k: int(v) for k, v in re.findall(r'(\w+)="(-?\d+)"', st.group(1))}
            d["part"] = name.group(1) if name else m.group(1)
            d["repaired"] = any(d.get(k, 0) for k in
                                ("edges_fixed", "degenerate_facets", "facets_removed",
                                 "facets_reversed", "backwards_edges"))
            out.append(d)
    return out

# ----------------------------------------------------------------------------- parse

WORD = re.compile(r"([A-Za-z])\s*([-+]?(?:\d+\.?\d*|\.\d+))")


def arc_points(x0, y0, x1, y1, i, j, cw, turns=0, chord=0.1):
    cx, cy = x0 + i, y0 + j
    r = math.hypot(i, j)
    a0 = math.atan2(y0 - cy, x0 - cx)
    a1 = math.atan2(y1 - cy, x1 - cx)
    if cw:
        sweep = a0 - a1
        if sweep <= 1e-9:
            sweep += 2 * math.pi
        sweep = -sweep
    else:
        sweep = a1 - a0
        if sweep <= 1e-9:
            sweep += 2 * math.pi
    if turns > 1:
        sweep += math.copysign(2 * math.pi * (turns - 1), sweep)
    n = max(1, int(math.ceil(abs(sweep) * r / chord)))
    pts = [(cx + r * math.cos(a0 + sweep * k / n), cy + r * math.sin(a0 + sweep * k / n))
           for k in range(1, n + 1)]
    pts[-1] = (x1, y1)
    return pts


def parse(text):
    header, config = {}, {}
    layers = []
    cur = None
    x = y = z = 0.0
    abs_xyz, rel_e = True, False
    feature, width, obj = None, None, None
    in_header = in_config = False
    ended = False
    objects_seen = set()
    m624 = set()
    for raw in text.splitlines():
        line = raw.strip()
        if not line:
            continue
        if line.startswith(";"):
            c = line[1:].strip()
            if c == "HEADER_BLOCK_START":
                in_header = True
            elif c == "HEADER_BLOCK_END":
                in_header = False
            elif c == "CONFIG_BLOCK_START":
                in_config = True
            elif c == "CONFIG_BLOCK_END":
                in_config = False
            elif in_header:
                for part in c.split(";"):
                    if ":" in part:
                        k, v = part.split(":", 1)
                        header[k.strip()] = v.strip()
            elif in_config:
                if " = " in c:
                    k, v = c.split(" = ", 1)
                    config[k.strip()] = v.strip()
            elif c == "CHANGE_LAYER":
                cur = {"index": len(layers), "z": None, "height": None, "segs": [],
                       "objects": set(), "features": {}}
                layers.append(cur)
                obj = None
            elif c.startswith("Z_HEIGHT:") and cur is not None:
                cur["z"] = float(c.split(":", 1)[1])
            elif c.startswith("LAYER_HEIGHT:") and cur is not None:
                cur["height"] = float(c.split(":", 1)[1])
            elif c.startswith("FEATURE:"):
                feature = c.split(":", 1)[1].strip()
            elif c.startswith("LINE_WIDTH:"):
                width = float(c.split(":", 1)[1])
            elif c.startswith("OBJECT_ID:"):
                obj = c.split(":", 1)[1].strip()
                objects_seen.add(obj)
            elif c == "MACHINE_END_GCODE_START":
                ended = True
            continue
        code = line.split(";", 1)[0].strip()
        if not code:
            continue
        head = code.split(None, 1)[0].upper()
        if head == "M82":
            rel_e = False
        elif head == "M83":
            rel_e = True
        elif head == "G90":
            abs_xyz = True
        elif head == "G91":
            abs_xyz = False
        elif head == "M624":
            m624.add(code)
        elif head == "G92":
            w = dict((k.upper(), float(v)) for k, v in WORD.findall(code[3:]))
            x, y, z = w.get("X", x), w.get("Y", y), w.get("Z", z)
        elif head in ("G0", "G1", "G2", "G3"):
            w = dict((k.upper(), float(v)) for k, v in WORD.findall(code[len(head):]))
            if abs_xyz:
                nx, ny, nz = w.get("X", x), w.get("Y", y), w.get("Z", z)
            else:
                nx, ny, nz = x + w.get("X", 0), y + w.get("Y", 0), z + w.get("Z", 0)
            e = w.get("E", 0.0)
            # Absolute E would need tracking; Bambu always emits M83. Treat as relative only.
            extruding = (rel_e and e > 0 and cur is not None and not ended
                         and feature != "Custom" and (nx != x or ny != y or head in ("G2", "G3")))
            if extruding:
                if head in ("G2", "G3") and ("I" in w or "J" in w):
                    pts = arc_points(x, y, nx, ny, w.get("I", 0), w.get("J", 0), head == "G2",
                                     int(w.get("P", 0)))
                else:
                    pts = [(nx, ny)]
                px, py = x, y
                wd = width or 0.42
                for qx, qy in pts:
                    cur["segs"].append((px, py, qx, qy, wd, obj))
                    px, py = qx, qy
                if obj is not None:
                    cur["objects"].add(obj)
                cur["features"][feature] = cur["features"].get(feature, 0) + 1
            x, y, z = nx, ny, nz
    return header, config, layers, sorted(objects_seen), sorted(m624)

# ----------------------------------------------------------------------------- geometry

def seg_dist(px, py, x0, y0, x1, y1):
    dx, dy = x1 - x0, y1 - y0
    L2 = dx * dx + dy * dy
    t = 0.0 if L2 == 0 else max(0.0, min(1.0, ((px - x0) * dx + (py - y0) * dy) / L2))
    return math.hypot(px - (x0 + t * dx), py - (y0 + t * dy))


def clip_len(x0, y0, x1, y1, r):
    """Length of segment inside axis-aligned rect r (Liang-Barsky)."""
    rx0, ry0, rx1, ry1 = r
    dx, dy = x1 - x0, y1 - y0
    t0, t1 = 0.0, 1.0
    for p, q in ((-dx, x0 - rx0), (dx, rx1 - x0), (-dy, y0 - ry0), (dy, ry1 - y0)):
        if p == 0:
            if q < 0:
                return 0.0
        else:
            t = q / p
            if p < 0:
                t0 = max(t0, t)
            else:
                t1 = min(t1, t)
            if t0 > t1:
                return 0.0
    return (t1 - t0) * math.hypot(dx, dy)


def bbox_of(segs):
    if not segs:
        return None
    xs = [s[0] for s in segs] + [s[2] for s in segs]
    ys = [s[1] for s in segs] + [s[3] for s in segs]
    return [min(xs), min(ys), max(xs), max(ys)]


def raster(segs, rect, cell):
    x0, y0, x1, y1 = rect
    nx = max(1, int(round((x1 - x0) / cell)))
    ny = max(1, int(round((y1 - y0) / cell)))
    sx, sy = (x1 - x0) / nx, (y1 - y0) / ny
    covered = set()
    for a, b, c, d, w, _ in segs:
        h = w / 2.0
        lo_x, hi_x = min(a, c) - h, max(a, c) + h
        lo_y, hi_y = min(b, d) - h, max(b, d) + h
        if hi_x < x0 or lo_x > x1 or hi_y < y0 or lo_y > y1:
            continue
        i0 = max(0, int(math.floor((lo_x - x0) / sx - 0.5)))
        i1 = min(nx - 1, int(math.ceil((hi_x - x0) / sx - 0.5)))
        j0 = max(0, int(math.floor((lo_y - y0) / sy - 0.5)))
        j1 = min(ny - 1, int(math.ceil((hi_y - y0) / sy - 0.5)))
        for i in range(i0, i1 + 1):
            px = x0 + (i + 0.5) * sx
            for j in range(j0, j1 + 1):
                if (i, j) in covered:
                    continue
                if seg_dist(px, y0 + (j + 0.5) * sy, a, b, c, d) <= h:
                    covered.add((i, j))
    return covered, nx * ny

# ----------------------------------------------------------------------------- log scan

LOG_PATTERNS = re.compile(
    r"repair|non-manifold|manifold|open edge|auto-repaired|flipped|reversed|degenerate|"
    r"\[error\]|\[warning\]|\[fatal\]|too close|conflict|outside|collision|empty layer|"
    r"floating|return_code=-", re.I)
LOG_BENIGN = [
    (re.compile(r"cli mode, Current BambuStudio Version"), "version banner"),
    (re.compile(r"Invalid OpenGL version|Failed to create GLFW window"),
     "no GL context in headless CLI: only the plate thumbnail render is skipped"),
    (re.compile(r"no filament colors found in projects"), "STL input has no colours"),
    (re.compile(r"tree support default to organic support"), "support disabled anyway"),
    (re.compile(r"can not find system preset file: .*_full/"),
     "CLI probes machine_full/process_full dirs that the flatpak does not ship"),
    (re.compile(r"disable prime tower for only one filament"), "single filament"),
    (re.compile(r"error_message=Success"), "success record"),
    (re.compile(r"gcode path conflicts check takes"), "timing line of the (passed) conflict check"),
    # BambuStudio src/libslic3r/Fill/FillFloatingConcentric.cpp: a log-only check after
    # clipping a concentric line against the floating (unsupported) area; the path is still
    # merged and used. 2026-10-12: the same bracket 3MF bytes sliced 6x logged it in 1 run;
    # 2 of the 6 runs (that one and another) produced different toolpaths at a few layers
    # (slicer-side nondeterminism, not input geometry); runs/run-4/ADDENDUM_2026-10-12.md.
    # Only this exact variant is benign; the other ZFiller messages stay findings.
    (re.compile(r"ZFiller: encounter idx from clip: \d+$"),
     "Bambu-internal log-only check in floating concentric fill (nondeterministic on identical input)"),
]


def scan_log(path):
    """Grep a slicer log for warnings/errors/repair messages. Returns list of findings
    {line, text, benign, reason}. Only lines containing a pattern in LOG_PATTERNS are listed;
    '[info]' lines are listed only for repair/geometry keywords, not for the generic tags."""
    out = []
    rc = None
    with open(path, encoding="utf-8", errors="replace") as f:
        for n, line in enumerate(f, 1):
            line = line.rstrip("\n")
            m = re.search(r"return_code=(-?\d+)", line)
            if m:
                rc = int(m.group(1))
            if "[trace]" in line or "[debug]" in line:
                continue
            # match on the text with file paths and object/file names masked, so a fixture
            # called e.g. "neg_box_flipped.stl" does not itself trigger "flipped"
            masked = re.sub(r"(?:/[^\s,;:]+)+|[\w.+-]+\.(?:stl|3mf|step|stp|obj|json|gcode)\b",
                            "<path>", line, flags=re.I)
            if not LOG_PATTERNS.search(masked):
                continue
            if line.startswith(("command:", "  /", "  (include)")) or " = " in line[:60]:
                continue  # our own preamble (paths/profile values), not slicer output
            reason = next((r for p, r in LOG_BENIGN if p.search(masked)), None)
            out.append({"line": n, "text": line[-300:], "benign": reason is not None,
                        "reason": reason})
    return {"findings": out, "slicer_return_code": rc,
            "non_benign": sum(1 for f in out if not f["benign"])}

# ----------------------------------------------------------------------------- probes

def offset_xy(config):
    v = config.get("extruder_offset", "0x0").split(",")[0]
    try:
        a, b = v.split("x")
        return float(a), float(b)
    except ValueError:
        return 0.0, 0.0


class Ctx:
    def __init__(self, layers, config, meta):
        self.layers = [l for l in layers if l["z"] is not None]
        self.off = offset_xy(config)
        self.meta = meta

    def to_plate(self, segs):
        ox, oy = self.off
        return [(a + ox, b + oy, c + ox, d + oy, w, o) for a, b, c, d, w, o in segs]

    def origin(self, p):
        fr = p.get("frame", "plate")
        if fr == "plate":
            return (0.0, 0.0), "plate"
        if fr == "gcode":
            return (self.off[0], self.off[1]), "plate"  # gcode -> plate shift
        if fr == "model_bbox":
            pj = self.meta.get("plate_json")
            if not pj:
                raise ValueError("frame model_bbox needs a .gcode.3mf with Metadata/plate_N.json")
            objs = pj.get("bbox_objects", [])
            sel = p.get("object")
            if sel is None:
                bb = pj["bbox_all"]
            elif isinstance(sel, int):
                bb = objs[sel]["bbox"]
            else:
                bb = next(o["bbox"] for o in objs if o.get("name") == sel)
            return (bb[0], bb[1]), "plate"
        if fr == "extrusion_bbox":
            n = int(p.get("bbox_layers", 3))
            segs = []
            for l in self.layers[:n]:
                segs += [s for s in l["segs"] if p.get("object_id") in (None, s[5])]
            bb = bbox_of(self.to_plate(segs))
            if not bb:
                raise ValueError("no extrusion in first layers")
            return (bb[0], bb[1]), "plate"
        raise ValueError(f"unknown frame {fr}")

    def select(self, p):
        if "z" in p:
            zq = float(p["z"])
            for l in self.layers:
                h = l["height"] or 0.2
                if l["z"] - h - 1e-6 < zq <= l["z"] + 1e-6:
                    return [l]
            return []
        if "z_range" in p:
            z0, z1 = map(float, p["z_range"])
            return [l for l in self.layers if z0 - 1e-6 <= l["z"] <= z1 + 1e-6]
        raise ValueError("probe needs z or z_range")


def run_probe(ctx, p, n_layers):
    kind = p.get("kind")
    res = {"name": p.get("name", kind), "kind": kind, "spec": p}
    try:
        if kind == "layer_count":
            ok = ((p.get("min") is None or n_layers >= p["min"]) and
                  (p.get("max") is None or n_layers <= p["max"]) and
                  (p.get("equals") is None or n_layers == p["equals"]))
            res.update(ok=ok, value=n_layers)
            return res
        if kind == "max_z":
            top = max(l["z"] for l in ctx.layers)
            res.update(ok=abs(top - float(p["value"])) <= float(p.get("tol", 1e-3)), value=top)
            return res
        (ox, oy), _ = ctx.origin(p)
        sel = ctx.select(p)
        if not sel:
            res.update(ok=False, error="no layer matches z/z_range")
            return res
        res["layers_z"] = [l["z"] for l in sel]
        if kind == "extrusion_near":
            px, py = ox + p["xy"][0], oy + p["xy"][1]
            r = float(p["radius"])
            best = min((seg_dist(px, py, a, b, c, d) for a, b, c, d, _, _ in
                        ctx.to_plate(sel[0]["segs"])), default=float("inf"))
            res.update(ok=best <= r, min_distance=round(best, 4), point_plate=[px, py])
        elif kind == "no_extrusion_in_rect":
            m = float(p.get("margin", 0))
            r = [ox + p["rect"][0] + m, oy + p["rect"][1] + m,
                 ox + p["rect"][2] - m, oy + p["rect"][3] - m]
            bad = []
            for l in sel:
                L = sum(clip_len(a, b, c, d, r) for a, b, c, d, _, _ in ctx.to_plate(l["segs"]))
                if L > 0:
                    bad.append({"z": l["z"], "length_inside": round(L, 3)})
            res.update(ok=not bad, rect_plate=r, violations=bad)
        elif kind == "filled_rect":
            r = [ox + p["rect"][0], oy + p["rect"][1], ox + p["rect"][2], oy + p["rect"][3]]
            cell = float(p.get("cell", 0.25))
            mode = p.get("mode", "each")
            mn = float(p["min_coverage"])
            per, union, total = [], set(), 1
            for l in sel:
                cov, total = raster(ctx.to_plate(l["segs"]), r, cell)
                per.append({"z": l["z"], "coverage": round(len(cov) / total, 4)})
                union |= cov
            if mode == "each":
                val = min(x["coverage"] for x in per)
            elif mode == "mean":
                val = sum(x["coverage"] for x in per) / len(per)
            elif mode == "union":
                val = len(union) / total
            else:
                raise ValueError(f"unknown mode {mode}")
            res.update(ok=val >= mn, value=round(val, 4), mode=mode, rect_plate=r, per_layer=per)
        else:
            raise ValueError(f"unknown probe kind {kind}")
    except (ValueError, KeyError, IndexError, StopIteration) as e:
        res.update(ok=False, error=f"{type(e).__name__}: {e}")
    return res

# ----------------------------------------------------------------------------- main

CONFIG_KEYS = ["printer_settings_id", "print_settings_id", "filament_settings_id", "layer_height",
               "initial_layer_print_height", "line_width", "outer_wall_line_width",
               "inner_wall_line_width", "initial_layer_line_width", "nozzle_diameter",
               "printable_area", "printable_height", "bed_exclude_area", "extruder_offset",
               "elefant_foot_compensation", "wall_loops", "bottom_shell_layers",
               "top_shell_layers", "sparse_infill_density", "enable_arc_fitting", "curr_bed_type"]


def main(argv=None):
    ap = argparse.ArgumentParser(description="Bambu G-code analyser (see module docstring)")
    ap.add_argument("gcode")
    ap.add_argument("--plate", type=int)
    ap.add_argument("--probes")
    ap.add_argument("--log")
    ap.add_argument("--out")
    ap.add_argument("--layers", action="store_true", help="include per-layer detail")
    ap.add_argument("--strict-log", action="store_true",
                    help="exit 1 if the slicer log has non-benign findings")
    a = ap.parse_args(argv)
    try:
        text, meta = load_input(a.gcode, a.plate)
    except (OSError, zipfile.BadZipFile) as e:
        print(f"analyze_gcode: {e}", file=sys.stderr)
        return 2
    header, config, layers, objects, m624 = parse(text)
    ctx = Ctx(layers, config, meta)
    if not ctx.layers:
        print(f"analyze_gcode: {a.gcode}: no '; CHANGE_LAYER' / '; Z_HEIGHT:' layers found "
              "(not Bambu Studio G-code?)", file=sys.stderr)
        return 2
    ox, oy = ctx.off
    all_segs = [s for l in ctx.layers for s in l["segs"]]
    rep = {
        "input": meta.get("source"),
        "plate_gcode": meta.get("plate_gcode"),
        "header": header,
        "config": {k: config[k] for k in CONFIG_KEYS if k in config},
        "frames": {"plate_minus_gcode_offset": [ox, oy]},
        "layer_count": len(ctx.layers),
        "header_layer_count": int(header["total layer number"]) if "total layer number" in header else None,
        "z_values": [l["z"] for l in ctx.layers],
        "extrusion_moves": len(all_segs),
        "extrusion_bbox_plate": bbox_of(ctx.to_plate(all_segs)),
        "objects_gcode": objects,
        "object_labels_m624": m624,
        "plate_json": meta.get("plate_json"),
        "mesh_stat": meta.get("mesh_stat"),
        "slice_info": meta.get("slice_info"),
    }
    if a.layers:
        rep["layers"] = [{"index": l["index"], "z": l["z"], "height": l["height"],
                          "extrusions": len(l["segs"]), "objects": sorted(l["objects"]),
                          "bbox_plate": bbox_of(ctx.to_plate(l["segs"])),
                          "features": l["features"]} for l in ctx.layers]
    fail = False
    if a.log:
        rep["log"] = scan_log(a.log)
        if a.strict_log and (rep["log"]["non_benign"] or (rep["log"]["slicer_return_code"] or 0) != 0):
            fail = True
    if rep["mesh_stat"] and any(m["repaired"] for m in rep["mesh_stat"]):
        rep["mesh_repaired"] = True
    if a.probes:
        try:
            with open(a.probes, encoding="utf-8") as f:
                spec = json.load(f)
        except (OSError, ValueError) as e:
            print(f"analyze_gcode: probes: {e}", file=sys.stderr)
            return 2
        rep["probes"] = [run_probe(ctx, p, len(ctx.layers)) for p in spec.get("probes", [])]
        rep["probes_passed"] = sum(1 for r in rep["probes"] if r["ok"])
        rep["probes_failed"] = [r["name"] for r in rep["probes"] if not r["ok"]]
        fail = fail or bool(rep["probes_failed"])
    rep["ok"] = not fail
    out = json.dumps(rep, indent=1)
    if a.out:
        with open(a.out, "w", encoding="utf-8") as f:
            f.write(out + "\n")
        print(json.dumps({"ok": rep["ok"], "layer_count": rep["layer_count"],
                          "probes_failed": rep.get("probes_failed"),
                          "log_non_benign": rep.get("log", {}).get("non_benign"),
                          "out": a.out}))
    else:
        print(out)
    return 1 if fail else 0


if __name__ == "__main__":
    sys.exit(main())
