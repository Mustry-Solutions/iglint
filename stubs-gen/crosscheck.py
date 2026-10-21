#!/usr/bin/env python3
"""Compares the generated system.* facts with the type stubs of
ignition-devs/ignition-api (MIT, branch 8.3) and writes CROSSCHECK.md.

Only names and argument counts are compared; nothing from either side is
copied besides function names.

Usage: crosscheck.py <ignition-api checkout> <system.json> <CROSSCHECK.md>
"""

import ast
import json
import os
import subprocess
import sys


def api_functions(root):
    """path -> list of (min, max or None, names) per overload, plus constants."""
    stubs = os.path.join(root, "stubs", "stubs", "system")
    functions, constants, modules = {}, set(), set()
    for dirpath, _, files in os.walk(stubs):
        for f in sorted(files):
            if not f.endswith(".pyi") or f == "__version__.pyi":
                continue
            rel = os.path.relpath(os.path.join(dirpath, f), os.path.dirname(stubs))
            parts = rel[:-4].split(os.sep)
            if parts[-1] == "__init__":
                parts = parts[:-1]
            module = ".".join(parts)
            modules.add(module)
            tree = ast.parse(open(os.path.join(dirpath, f)).read())
            for node in tree.body:
                if isinstance(node, ast.FunctionDef):
                    a = node.args
                    positional = a.posonlyargs + a.args
                    n_req = len(positional) - len(a.defaults)
                    kwonly_req = [k.arg for k, d in zip(a.kwonlyargs, a.kw_defaults) if d is None]
                    functions.setdefault(module + "." + node.name, []).append({
                        "min": n_req,
                        "max": None if a.vararg else len(positional),
                        "names": [x.arg for x in positional] + [k.arg for k in a.kwonlyargs],
                        "kwonly_required": kwonly_req,
                    })
                elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
                    constants.add(module + "." + node.target.id)
                elif isinstance(node, ast.Assign):
                    for t in node.targets:
                        if isinstance(t, ast.Name) and t.id != "__all__":
                            constants.add(module + "." + t.id)
    return functions, constants, modules


def our_arity(variants):
    """(min, max or None, keyword names or None, accepts_keywords)."""
    lo, hi, names, kw = None, 0, None, False
    for v in variants:
        for o in v["overloads"]:
            n = len(o["params"])
            if o["kind"] == "unchecked":
                return 0, None, None, True
            if o["kind"] == "keyword":
                kw = True
                names = [p["name"] for p in o["params"]]
                req = sum(1 for p in o["params"] if p["required"])
                lo = req if lo is None else min(lo, req)
                hi = None if hi is None else max(hi, n)
            else:
                lo = n if lo is None else min(lo, n)
                hi = None if hi is None else max(hi, n)
    return lo or 0, hi, names, kw


def fmt_range(lo, hi):
    if hi is None:
        return "%d+" % lo
    return str(lo) if lo == hi else "%d-%d" % (lo, hi)


def main():
    api_root, system_json, out_path = sys.argv[1:4]
    try:
        commit = subprocess.check_output(["git", "-C", api_root, "rev-parse", "HEAD"], text=True).strip()
    except Exception:
        commit = "unknown"
    api, api_constants, api_modules = api_functions(api_root)
    data = json.load(open(system_json))
    ours = {f["path"]: f for f in data["functions"]}
    our_constants = {c["path"] for c in data["constants"]}
    our_objects = {o["path"] for o in data["objects"]}
    our_packages = {p["path"] for p in data["packages"]}

    def public(path):
        return not any(seg.startswith("_") for seg in path.split("."))

    def hidden(path):
        return all(v["hidden"] for v in ours[path]["variants"])

    only_ours = sorted(p for p in ours if public(p) and p not in api)
    only_ours_visible = [p for p in only_ours if not hidden(p)]
    only_ours_hidden = [p for p in only_ours if hidden(p)]
    only_api = sorted(p for p in api if public(p) and p not in ours)
    only_api_known_pkg = [p for p in only_api if p.rsplit(".", 1)[0] in our_packages]
    only_api_unknown_pkg = sorted({p.rsplit(".", 1)[0] for p in only_api if p.rsplit(".", 1)[0] not in our_packages})

    arity_rows, name_rows = [], []
    for p in sorted(set(ours) & set(api)):
        lo, hi, names, kw = our_arity(ours[p]["variants"])
        alo = min(o["min"] for o in api[p])
        ahi = None if any(o["max"] is None for o in api[p]) else max(o["max"] for o in api[p])
        if (lo, hi) != (alo, ahi):
            note = []
            if alo < lo:
                note.append("ignition-api accepts fewer arguments")
            if alo > lo:
                note.append("gateway accepts fewer arguments")
            if (ahi is None) != (hi is None) or (ahi is not None and hi is not None and ahi != hi):
                note.append("upper bound differs")
            arity_rows.append((p, fmt_range(lo, hi), fmt_range(alo, ahi), "; ".join(note),
                               "keyword" if kw else "positional"))
        if kw and names:
            api_names = max((o["names"] for o in api[p]), key=len)
            if api_names != names:
                missing = [n for n in names if n not in api_names]
                extra = [n for n in api_names if n not in names]
                if missing or extra:
                    name_rows.append((p, ", ".join(missing) or "-", ", ".join(extra) or "-"))

    const_only_ours = sorted(c for c in our_constants if public(c) and c not in api_constants)
    const_only_api = sorted(c for c in api_constants
                            if public(c) and c not in our_constants and c not in our_objects
                            and c not in ours and c not in our_packages
                            and c.rsplit(".", 1)[0] in our_packages)

    L = []
    L.append("# Cross-check against ignition-api")
    L.append("")
    L.append("Generated by `stubs-gen/crosscheck.py`. Compares the generated `system.*` facts")
    L.append("(`crates/iglint_stubs/data/system.json`, Ignition %s, all scopes) with the type" % data["ignition_version"])
    L.append("stubs of [ignition-devs/ignition-api](https://github.com/ignition-devs/ignition-api)")
    L.append("branch `8.3`, commit `%s` (`stubs/stubs/system/**/*.pyi`)." % commit)
    L.append("Only names and argument counts are compared.")
    L.append("")
    L.append("Arity on our side: plain Java functions accept exactly the parameter count of one")
    L.append("of their overloads; keyword functions accept the required parameters (measured on")
    L.append("the gateway) up to all declared names. On the ignition-api side: parameters")
    L.append("without a default are required.")
    L.append("")
    L.append("## Summary")
    L.append("")
    L.append("| | Count |")
    L.append("|---|---|")
    L.append("| Functions in the gateway data | %d |" % len(ours))
    L.append("| Functions in ignition-api | %d |" % len(api))
    L.append("| In both | %d |" % len(set(ours) & set(api)))
    L.append("| Only in the gateway data, visible in the Designer | %d |" % len(only_ours_visible))
    L.append("| Only in the gateway data, hidden (`@NoHint`) | %d |" % len(only_ours_hidden))
    L.append("| Only in ignition-api, package exists on the gateway | %d |" % len(only_api_known_pkg))
    L.append("| Only in ignition-api, package not installed in the image | %d functions in %d packages |" % (
        len(only_api) - len(only_api_known_pkg), len(only_api_unknown_pkg)))
    L.append("| Arity differs | %d |" % len(arity_rows))
    L.append("| Keyword names differ | %d |" % len(name_rows))
    L.append("| Constants only in the gateway data | %d |" % len(const_only_ours))
    L.append("| Constants only in ignition-api | %d |" % len(const_only_api))
    L.append("")

    def section(title, items):
        L.append("## " + title)
        L.append("")
        if not items:
            L.append("None.")
        for i in items:
            L.append("- `%s`" % i)
        L.append("")

    section("Packages in ignition-api that are not installed in the image", only_api_unknown_pkg)
    section("Functions only in ignition-api (package exists on the gateway)", only_api_known_pkg)
    section("Functions only in the gateway data, visible in the Designer", only_ours_visible)
    section("Functions only in the gateway data, hidden (`@NoHint`)", only_ours_hidden)
    L.append("## Arity differences")
    L.append("")
    L.append("| Function | Kind | Gateway | ignition-api | Note |")
    L.append("|---|---|---|---|---|")
    for p, ours_r, api_r, note, kind in arity_rows:
        L.append("| `%s` | %s | %s | %s | %s |" % (p, kind, ours_r, api_r, note))
    L.append("")
    L.append("## Keyword name differences")
    L.append("")
    L.append("| Function | Only on the gateway | Only in ignition-api |")
    L.append("|---|---|---|")
    for p, a, b in name_rows:
        L.append("| `%s` | %s | %s |" % (p, a, b))
    L.append("")
    section("Constants only in the gateway data", const_only_ours)
    section("Constants only in ignition-api", const_only_api)
    with open(out_path, "w") as f:
        f.write("\n".join(L))


if __name__ == "__main__":
    main()
