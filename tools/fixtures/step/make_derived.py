#!/usr/bin/env python3
"""Deterministic malformed derivations of box_40x30x10_mm.step (python3).

usage: make_derived.py <fixture dir>

  truncated.step     first 60% of the bytes of box_40x30x10_mm.step, i.e.
                     cut inside the DATA section (no ENDSEC / END-ISO-10303-21;
                     the last instance is usually cut mid-record).
  garbage.step       a few lines of non-STEP text.
  missing_unit.step  box_40x30x10_mm.step with the length unit removed: its
                     instance is deleted and dropped from the
                     GLOBAL_UNIT_ASSIGNED_CONTEXT list. The uncertainty
                     measure references the length unit, so it and the
                     GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT(...) partial of the
                     complex context instance are removed too. The result
                     has no LENGTH_UNIT at all, no dangling #refs, and is
                     syntactically valid Part 21.
"""
import re
import sys

out = sys.argv[1]
src = f"{out}/box_40x30x10_mm.step"
data = open(src, "rb").read()

cut = int(len(data) * 0.6)
data_start = data.index(b"DATA;")
assert data_start < cut < data.index(b"ENDSEC;", data_start)
open(f"{out}/truncated.step", "wb").write(data[:cut])

open(f"{out}/garbage.step", "w", newline="\n").write(
    "This is not a STEP file.\n"
    "solid cube\n"
    "  facet normal 0 0 1\n"
    "endsolid cube\n"
    "#1 = ???(;\n"
)

text = data.decode("latin-1")


def drop_instance(text, pattern):
    m = re.search(r"^#(\d+) = " + pattern + r"[^;]*;\n", text, re.M)
    assert m, pattern
    return text[: m.start()] + text[m.end():], m.group(1)


text, lid = drop_instance(text, r"\( LENGTH_UNIT\(\)")
text, uid = drop_instance(text, r"UNCERTAINTY_MEASURE_WITH_UNIT\(")
text, n = re.subn(rf"GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT\(\(#{uid}\)\)\s*", "", text)
assert n == 1
text, n = re.subn(rf"(GLOBAL_UNIT_ASSIGNED_CONTEXT\s*\(\()#{lid},", r"\1", text)
assert n == 1
for ref in (lid, uid):
    assert not re.search(rf"#{ref}(?!\d)", text), f"#{ref} still referenced"
assert "LENGTH_UNIT" not in text
open(f"{out}/missing_unit.step", "w", encoding="latin-1", newline="").write(text)
print("derived: truncated.step garbage.step missing_unit.step")
