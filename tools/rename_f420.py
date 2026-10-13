#!/usr/bin/env python3
"""The Fission420/f420 -> R-CAD/rcad rename, as one deterministic transform.

  tools/rename_f420.py tree            rename paths and rewrite text in the working tree
  tools/rename_f420.py patch < in.patch > out.patch
                                        rewrite a `git format-patch` made on the old (f420)
                                        history so it applies to the renamed tree (git am)
  tools/rename_f420.py check           list remaining f420/Fission420 mentions outside the
                                        allowed places (exit 1 if any)

Both modes use the same RULES, so a branch developed on the pre-rename history can be
ported commit by commit (see docs/RENAME.md). Standard library only.
"""
import os
import re
import subprocess
import sys

# Order matters: specific before general.
RULES = [
    ("f420-app", "rcad"),  # the app package, its directory and its binary
    ("f420_app", "rcad"),  # the app library crate
    ("Fission420", "R-CAD"),
    ("fission420", "r-cad"),
    ("F420App", "RcadApp"),
    ("F420_", "RCAD_"),
    ("f420.json", "rcad.json"),  # file extension, with or without the leading dot
    ("f420_", "rcad_"),
    ("f420-", "rcad-"),
    ("f420", "rcad"),  # anything left: test tags, scratch names
    ("F420", "RCAD"),
]

# Never rewritten: frozen legacy fixtures (their contents are compatibility evidence),
# externally validated exports, licence text, binaries, the lockfile (cargo regenerates
# it) and this script itself.
KEEP_CONTENT = [
    re.compile(r"^crates/[^/]+/tests/fixtures/"),
    re.compile(r"^examples/print/"),
    re.compile(r"^LICENSE-APACHE$"),
    re.compile(r"^Cargo\.lock$"),
    re.compile(r"^tools/rename_f420\.py$"),
    re.compile(r"^docs/RENAME\.md$"),
    re.compile(r"\.(png|jpe?g|stl|3mf|ttf|zip|gz)$", re.I),
]

# Mentions that are allowed to stay after the rename (compatibility code and docs).
ALLOWED_MENTIONS = KEEP_CONTENT + [
    re.compile(r"^crates/rcad-document/src/io\.rs$"),
    re.compile(r"^crates/rcad/src/recovery\.rs$"),
    re.compile(r"^crates/rcad-document/tests/legacy_name\.rs$"),
    re.compile(r"^crates/rcad/tests/recovery_legacy\.rs$"),
    re.compile(r"^docs/(FORMAT_SUPPORT|COMMANDS|UI|CI)\.md$"),
    re.compile(r"^README\.md$"),
]


def transform(text):
    for a, b in RULES:
        text = text.replace(a, b)
    return text


def tracked():
    out = subprocess.run(["git", "ls-files", "-z"], check=True, capture_output=True).stdout
    return [p for p in out.decode().split("\0") if p]


def tree():
    paths = tracked()
    for p in paths:
        if any(r.search(p) for r in KEEP_CONTENT):
            continue
        with open(p, "rb") as fh:
            data = fh.read()
        if b"\0" in data[:8192]:
            continue
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            continue
        new = transform(text)
        if new != text:
            with open(p, "w", encoding="utf-8", newline="") as fh:
                fh.write(new)
    # Renames last (contents of kept files are untouched, but their names follow the rules
    # so references in rewritten code still resolve).
    for p in tracked():
        q = transform(p)
        if q != p:
            os.makedirs(os.path.dirname(q) or ".", exist_ok=True)
            subprocess.run(["git", "mv", p, q], check=True)


def patch():
    """Like `tree`, but on a `git diff`/`format-patch` stream: paths are always renamed, and
    hunk content is rewritten except in files whose contents `tree` keeps (KEEP_CONTENT),
    so frozen fixtures added on the old history keep their original bytes."""
    out = []
    keep = False
    for line in sys.stdin.read().splitlines(keepends=True):
        if line.startswith("diff --git "):
            m = re.match(r"diff --git a/(\S+) b/(\S+)", line)
            paths = [transform(x) for x in m.groups()] if m else []
            keep = any(r.search(q) for q in paths for r in KEEP_CONTENT)
            out.append(transform(line))
        elif keep and not line.startswith(
            ("--- ", "+++ ", "rename from ", "rename to ", "copy from ", "copy to ")
        ):
            out.append(line)
        else:
            out.append(transform(line))
    sys.stdout.write("".join(out))


def check():
    bad = []
    pat = re.compile(r"f420|F420|[Ff]ission420")
    for p in tracked():
        if any(r.search(p) for r in ALLOWED_MENTIONS):
            continue
        if pat.search(p):
            bad.append(f"{p}: (path)")
            continue
        try:
            with open(p, encoding="utf-8") as fh:
                for i, line in enumerate(fh, 1):
                    if pat.search(line):
                        bad.append(f"{p}:{i}: {line.strip()[:120]}")
        except (UnicodeDecodeError, IsADirectoryError):
            pass
    print("\n".join(bad) if bad else "no stray f420/Fission420 mentions")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    {"tree": tree, "patch": patch, "check": check}[sys.argv[1]]()
