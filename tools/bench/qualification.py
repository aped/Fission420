#!/usr/bin/env python3
"""Performance qualification fixtures.

Generates three command workflows, runs each in a fresh release `rcad-cli`
process under GNU time (peak RSS), and writes a Markdown table with the
per-command `elapsed_ms` the CLI reports:

* history100: a 300 x 200 x 10 plate + 49 hole pairs (sketch + cut) = 100
  features; save, close, reopen (full regeneration), an upstream width edit
  (full regeneration), undo.
* assembly200: one component with 200 linked occurrences (grid); BOM, mass
  properties, assembly 3MF export, save/reopen.
* mesh1m: a binary STL torus of 1 000 000 triangles (generated here, ~50 MB;
  its seam vertices differ in float32 rounding): import, repair preview, repair
  with a 0.001 mm weld (closes the seam), STL export of the repaired body.

usage: tools/bench/qualification.py <rcad-cli> <out-dir>
These are measurements on the recorded machine and revision, not promises.
"""
import json, math, os, platform, struct, subprocess, sys, time

cli, out = sys.argv[1], sys.argv[2]
os.makedirs(out, exist_ok=True)


def history100():
    steps = [{"cmd": "new_document"},
             {"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
              "width": 300.0, "height": 200.0, "center": [150.0, 100.0]},
             {"cmd": "extrude", "sketch": 1, "distance": 10.0}]
    f = 3
    for i in range(49):
        x, y = 12.0 + (i % 7) * 40.0, 15.0 + (i // 7) * 26.0
        steps.append({"cmd": "create_sketch",
                      "plane": {"Face": {"feature": 2, "role": "ExtrudeEndCap"}},
                      "template": "centered_circle", "diameter": 6.0, "center": [x, y]})
        steps.append({"cmd": "cut_through_all", "sketch": f})
        f += 2
    path = os.path.join(out, "history100.rcad.json")
    steps += [{"cmd": "save", "path": path}, {"cmd": "close"}, {"cmd": "open", "path": path},
              {"cmd": "set_parameter", "feature": 1, "name": "width", "value": 310.0},
              {"cmd": "undo"}]
    return steps


def assembly200():
    steps = [{"cmd": "new_document"},
             {"cmd": "create_sketch", "plane": "WorldXY", "template": "centered_rectangle",
              "width": 8.0, "height": 8.0},
             {"cmd": "extrude", "sketch": 1, "distance": 5.0},
             {"cmd": "create_component", "name": "Block", "bodies": [1], "part_number": "B-1"}]
    for i in range(1, 200):
        steps.append({"cmd": "add_occurrence", "component": 1,
                      "translate": [(i % 20) * 12.0, (i // 20) * 12.0, 0.0]})
    path = os.path.join(out, "assembly200.rcad.json")
    steps += [{"cmd": "bom"}, {"cmd": "mass_properties"},
              {"cmd": "export_assembly", "path": os.path.join(out, "assembly200.3mf"),
               "format": "3mf"},
              {"cmd": "save", "path": path}, {"cmd": "close"}, {"cmd": "open", "path": path}]
    return steps


def torus_stl(path, nu=1000, nv=500, R=40.0, r=10.0):
    with open(path, "wb") as fh:
        fh.write(b"R-CAD qualification torus".ljust(80, b" "))
        fh.write(struct.pack("<I", 2 * nu * nv))
        def p(i, j):
            u, v = 2 * math.pi * i / nu, 2 * math.pi * j / nv
            return ((R + r * math.cos(v)) * math.cos(u), (R + r * math.cos(v)) * math.sin(u),
                    r * math.sin(v))
        for i in range(nu):
            for j in range(nv):
                a, b, c, d = p(i, j), p(i + 1, j), p(i + 1, j + 1), p(i, j + 1)
                for t in ((a, b, c), (a, c, d)):
                    fh.write(struct.pack("<12fH", 0, 0, 0, *t[0], *t[1], *t[2], 0))


def mesh1m():
    stl = os.path.join(out, "torus_1m.stl")
    if not os.path.exists(stl):
        torus_stl(stl)
    return [{"cmd": "new_document"},
            {"cmd": "import_stl", "path": stl, "units_to_mm": 1.0},
            {"cmd": "repair_mesh", "body": 1, "options": {}, "preview": True},
            # float32 seam vertices differ in the last bits: a 0.001 mm weld closes it.
            {"cmd": "repair_mesh", "body": 1, "options": {"weld_tolerance": 0.001}},
            {"cmd": "export_stl", "path": os.path.join(out, "torus_1m_out.stl"), "bodies": [2]}]


def run(name, steps):
    wf = os.path.join(out, name + "_workflow.json")
    with open(wf, "w") as fh:
        json.dump(steps, fh)
    t0 = time.time()
    p = subprocess.run(["/usr/bin/time", "-f", "MAXRSS_KB %M", cli, "run", wf],
                       capture_output=True, text=True)
    wall = time.time() - t0
    rss = next((int(l.split()[1]) for l in p.stderr.splitlines() if l.startswith("MAXRSS_KB")), 0)
    rows = []
    for line in p.stdout.splitlines():
        r = json.loads(line)["response"]
        rows.append((r["command"], r.get("elapsed_ms", 0.0), r["ok"]))
    return wall, rss, rows, p.returncode


rev = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True,
                     text=True).stdout.strip()
cpu = next((l.split(":", 1)[1].strip() for l in open("/proc/cpuinfo") if l.startswith("model name")), "?")
mem = next((int(l.split()[1]) // 1024 // 1024 for l in open("/proc/meminfo") if l.startswith("MemTotal")), 0)
lines = [f"# Performance qualification ({time.strftime('%Y-%m-%d %H:%M')})", "",
         f"Revision {rev}, release build, {cpu}, {mem} GB RAM, {platform.platform()}.",
         "One fresh `rcad-cli run` process per fixture; `elapsed_ms` as reported per command",
         "(includes regeneration/measurement where the command triggers it). Peak RSS from GNU time.", ""]
for name, gen in (("history100", history100), ("assembly200", assembly200), ("mesh1m", mesh1m)):
    wall, rss, rows, rc = run(name, gen())
    agg = {}
    for c, ms, ok in rows:
        a = agg.setdefault(c, [0, 0.0, 0.0, True])
        a[0] += 1; a[1] += ms; a[2] = max(a[2], ms); a[3] &= ok
    lines += [f"## {name}", "", f"wall {wall:.1f} s, peak RSS {rss / 1024:.0f} MB, exit {rc}", "",
              "| command | count | total ms | max ms | all ok |", "|---|---|---|---|---|"]
    for c, (n, tot, mx, ok) in agg.items():
        lines.append(f"| {c} | {n} | {tot:.1f} | {mx:.1f} | {ok} |")
    lines.append("")
    print(f"{name}: wall {wall:.1f}s rss {rss/1024:.0f}MB exit {rc}", flush=True)
with open(os.path.join(out, "report.md"), "w") as fh:
    fh.write("\n".join(lines) + "\n")
print("\n".join(lines))
