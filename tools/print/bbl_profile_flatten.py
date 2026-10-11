#!/usr/bin/env python3
"""Flatten a Bambu Studio vendor (BBL) profile JSON by resolving its `inherits` chain.

Vendor profiles under <flatpak>/files/share/BambuStudio/profiles/BBL/{machine,process,filament}
are inheritance chains: each file names its parent by preset *name* in `inherits`, and the parent
lives in the same directory (file name usually == preset name + ".json", but we index by the
`name` field to be safe). The CLI's --load-settings / --load-filaments need a self-contained
config, so this tool merges root -> leaf (child keys override parent keys) and writes one JSON.

Never modifies the vendor files. Stdlib only. Run with `python3 -I`.

Usage:
  bbl_profile_flatten.py <profile.json> -o <out.json> [--provenance <prov.json>] [--keys k1,k2]

Also resolves `include` lists (machine G-code templates, see flatten()).

Output JSON:
  - all merged keys; `inherits` set to "", `include` dropped; `name`/`from`/`setting_id` keep the
    leaf's values (the CLI logs e.g. "loaded filament 1 ... type system, name Generic PLA").
  --provenance writes {"chain":[leaf..root paths], "source":{key: path_of_file_that_supplied_it}}.
  --keys prints `key = value  (from <file>)` for the requested keys to stdout.
"""
import argparse
import json
import os
import sys


def index_dir(d):
    idx = {}
    for fn in os.listdir(d):
        if not fn.endswith(".json"):
            continue
        p = os.path.join(d, fn)
        try:
            with open(p, encoding="utf-8") as f:
                data = json.load(f)
        except (OSError, ValueError):
            continue
        name = data.get("name") if isinstance(data, dict) else None
        if name:
            idx.setdefault(name, p)
    return idx


def chain_for(path):
    """Return [leaf, parent, ..., root] as (path, data) tuples."""
    d = os.path.dirname(os.path.abspath(path))
    idx = None
    out = []
    seen = set()
    cur = os.path.abspath(path)
    while cur:
        if cur in seen:
            raise SystemExit(f"inherits cycle at {cur}")
        seen.add(cur)
        with open(cur, encoding="utf-8") as f:
            data = json.load(f)
        out.append((cur, data))
        parent = data.get("inherits") or ""
        if not parent:
            break
        cand = os.path.join(d, parent + ".json")
        if not os.path.isfile(cand):
            if idx is None:
                idx = index_dir(d)
            cand = idx.get(parent)
            if not cand:
                raise SystemExit(f"cannot resolve inherits '{parent}' from {cur}")
        cur = cand
    return out


INCLUDE_SKIP = {"name", "instantiation", "type", "from", "inherits", "setting_id"}


def resolve_name(d, name):
    cand = os.path.join(d, name + ".json")
    if os.path.isfile(cand):
        return cand
    cand = index_dir(d).get(name)
    if not cand:
        raise SystemExit(f"cannot resolve include '{name}' in {d}")
    return cand


def flatten(path):
    """Merge root -> leaf. Per file: its `include` templates (e.g. the P1S machine profile pulls
    machine_start_gcode etc. from "... template machine_start_gcode.json") are applied first,
    then the file's own keys; both override everything inherited from parents."""
    chain = chain_for(path)
    merged, source, included = {}, {}, []
    for p, data in reversed(chain):  # root first, leaf last
        for inc in data.get("include") or []:
            ip = resolve_name(os.path.dirname(p), inc)
            included.append(ip)
            with open(ip, encoding="utf-8") as f:
                idata = json.load(f)
            for k, v in idata.items():
                if k not in INCLUDE_SKIP:
                    merged[k] = v
                    source[k] = ip
        for k, v in data.items():
            if k == "include":
                continue
            merged[k] = v
            source[k] = p
    merged["inherits"] = ""
    return merged, source, [p for p, _ in chain] + [f"(include) {ip}" for ip in included]


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("profile")
    ap.add_argument("-o", "--out")
    ap.add_argument("--provenance")
    ap.add_argument("--keys", default="")
    a = ap.parse_args(argv)
    merged, source, chain = flatten(a.profile)
    if a.out:
        os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
        with open(a.out, "w", encoding="utf-8") as f:
            json.dump(merged, f, indent=4, sort_keys=True)
    if a.provenance:
        with open(a.provenance, "w", encoding="utf-8") as f:
            json.dump({"chain": chain, "source": source}, f, indent=2, sort_keys=True)
    print("chain (leaf -> root):")
    for p in chain:
        print("  " + p)
    for k in [k for k in a.keys.split(",") if k]:
        print(f"{k} = {json.dumps(merged.get(k))}  (from {source.get(k, '<absent>')})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
