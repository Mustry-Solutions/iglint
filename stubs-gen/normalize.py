#!/usr/bin/env python3
"""Turns the raw gateway dumps (stubs-gen/out) into the committed facts in
crates/iglint_stubs/data.

Only facts are written: names, scopes, parameter names, types, arities,
required flags, deprecations with their replacement, constant values and
Perspective property structure with small defaults. All descriptive text
(descriptions, titles, examples, deprecation messages) and sample content
(large defaults, default component props) is dropped.

Usage: normalize.py <raw dir> <data dir>
"""

import html
import json
import os
import re
import sys

IGNITION_VERSION = "8.3.3"
SCOPES = ("gateway", "client", "designer")
RAW_KW = ["org.python.core.PyObject[]", "java.lang.String[]"]
NO_HINT = "com.inductiveautomation.ignition.common.script.hints.NoHint"
SCRIPTING_DOCS = ("https://www.docs.inductiveautomation.com/docs/8.3/appendix/"
                  "scripting-functions/{module_slug}/{function_slug}")
EXPRESSION_DOCS = ("https://www.docs.inductiveautomation.com/docs/8.3/appendix/"
                   "expression-functions/{category_slug}/{name}")

JAVA_TYPES = {
    "java.lang.String": "str",
    "java.lang.CharSequence": "str",
    "char": "str",
    "int": "int",
    "java.lang.Integer": "int",
    "short": "int",
    "long": "long",
    "java.lang.Long": "long",
    "double": "float",
    "float": "float",
    "java.lang.Double": "float",
    "java.lang.Float": "float",
    "java.lang.Number": "number",
    "boolean": "bool",
    "java.lang.Boolean": "bool",
    "java.util.Date": "Date",
    "com.inductiveautomation.ignition.common.Dataset": "Dataset",
    "com.inductiveautomation.ignition.common.script.builtin.DatasetUtilities$PyDataSet": "PyDataset",
    "java.util.List": "list",
    "java.util.Collection": "list",
    "org.python.core.PySequence": "list",
    "org.python.core.PyList": "list",
    "java.lang.Object[]": "list",
    "java.lang.String[]": "list",
    "java.util.Map": "dict",
    "org.python.core.PyDictionary": "dict",
    "org.python.core.PyObject": "any",
    "java.lang.Object": "any",
    "org.python.core.PyType": "type",
    "org.python.core.PyFunction": "callable",
    "byte[]": "bytes",
    "void": "None",
}


def simple_type(java):
    if java is None:
        return None
    return JAVA_TYPES.get(java, java)


def load(raw, name):
    path = os.path.join(raw, name)
    with open(path) as f:
        data = json.load(f)
    if isinstance(data, dict) and "error" in data and len(data) == 1:
        sys.exit("%s: gateway reported an error:\n%s" % (name, data["error"]))
    return data


# ---------------------------------------------------------------------------
# system.*

def hint_index(hints):
    methods, attrs = {}, {}
    for h in hints:
        if h["kind"] == "method":
            methods.setdefault(h["path"], []).append(h)
        elif h["kind"] == "attribute":
            attrs[h["path"]] = h
    return methods, attrs


def positional_overload(o, hints, siblings):
    """A plain Java overload. Names come from the Designer hint with the same
    parameter count, or else from a longer overload whose Java parameter
    types start with this overload's types (Ignition documents only the
    longest form of functions with trailing optional parameters)."""
    n = len(o["params"])
    by_count = {}
    for h in hints:
        by_count.setdefault(len(h["params"]), []).append(h)
    hint = by_count[n][0] if len(by_count.get(n, [])) == 1 else None
    names = [p["name"] for p in hint["params"]] if hint else None
    hint_types = [p["type"] for p in hint["params"]] if hint else None
    if hint is None:
        for sib in sorted(siblings, key=lambda x: len(x["params"])):
            m = len(sib["params"])
            if m > n and sib["params"][:n] == o["params"] and len(by_count.get(m, [])) == 1:
                longer = by_count[m][0]
                names = [p["name"] for p in longer["params"]][:n]
                hint_types = [p["type"] for p in longer["params"]][:n]
                break
    params = []
    for i, jt in enumerate(o["params"]):
        params.append({
            "name": names[i] if names else None,
            "type": (hint_types[i] if hint_types and hint_types[i] else simple_type(jt)),
            "required": True,
        })
    returns = hint["returns"] if hint and hint["returns"] else simple_type(o["returns"])
    return {"kind": "positional", "params": params, "returns": returns}


def keyword_overload(o, hints, required):
    names = o["keyword_args"]["names"]
    types = o["keyword_args"]["types"]
    by_name = {}
    for h in hints:
        if [p["name"] for p in h["params"]] == names:
            by_name = {p["name"]: p for p in h["params"]}
            returns = h["returns"]
            break
    else:
        returns = None
    params = []
    for n, jt in zip(names, types):
        hp = by_name.get(n)
        p = {
            "name": n,
            "type": hp["type"] if hp and hp["type"] else simple_type(jt),
            "required": n in required,
        }
        if hp and hp.get("default") is not None and len(hp["default"].split()) <= 3:
            # A default is a literal; longer texts are explanations.
            p["default"] = hp["default"]
        params.append(p)
    return {"kind": "keyword", "params": params, "returns": returns or simple_type(o["returns"])}


def function_facts(entry, hints, required):
    overloads = []
    seen = set()
    for o in entry["overloads"]:
        if o["params"] == RAW_KW:
            if "keyword_args" in o:
                ov = keyword_overload(o, hints, required)
            else:
                ov = {"kind": "unchecked", "params": [], "returns": simple_type(o["returns"])}
        else:
            ov = positional_overload(o, hints, entry["overloads"])
        key = json.dumps(ov, sort_keys=True)
        if key not in seen:
            seen.add(key)
            overloads.append(ov)
    overloads.sort(key=lambda ov: (ov["kind"] != "positional", len(ov["params"])))
    java_dep = all(o["java_deprecated"] for o in entry["overloads"])
    hint_dep = [h["deprecation"] for h in hints if h.get("deprecation")]
    replacement = None
    for d in hint_dep:
        if d.get("replacement") and d["replacement"] != "null":
            replacement = d["replacement"]
    return {
        "overloads": overloads,
        "deprecated": bool(java_dep or hint_dep),
        "replacement": replacement,
        "hidden": all(NO_HINT in o.get("other_annotations", []) for o in entry["overloads"]),
    }


def scope_tree(dump, gateway_required):
    methods, attrs = hint_index(dump["hints"])
    functions, constants, objects, packages = {}, {}, {}, set()
    for e in dump["live"]:
        p = e["path"]
        if e["kind"] == "package":
            packages.add(p)
        elif e["kind"] == "java_function":
            probe = e.get("required_probe")
            required = set(probe["required"]) if probe else gateway_required.get(p, set())
            functions[p] = function_facts(e, methods.get(p, []), required)
        elif e["kind"] == "python_function":
            sig = e["python_signature"]
            n_def = len(sig.get("defaults", []))
            args = sig.get("args", [])
            params = [{"name": a, "type": None, "required": i < len(args) - n_def}
                      for i, a in enumerate(args)]
            functions[p] = {"overloads": [{"kind": "python", "params": params, "returns": None,
                                           "varargs": sig.get("varargs") is not None,
                                           "varkw": sig.get("varkw") is not None}],
                            "deprecated": False, "replacement": None, "hidden": False}
        elif e["kind"] == "constant":
            constants[p] = {"type": e["value_type"], "value": e["value"]}
        else:
            kind = "class" if e["kind"] == "callable_object" else "object"
            hint = attrs.get(p)
            objects[p] = {"kind": kind, "type": hint["type"] if hint else e.get("java_class")}
    return functions, constants, objects, packages


def merge_scopes(per_scope, key):
    """Groups identical facts across scopes: path -> [{scopes, ...facts}]."""
    out = {}
    for scope in SCOPES:
        for path, facts in per_scope[scope][key].items():
            variants = out.setdefault(path, [])
            blob = json.dumps(facts, sort_keys=True)
            for v in variants:
                if v["_blob"] == blob:
                    v["scopes"].append(scope)
                    break
            else:
                variants.append(dict(facts, scopes=[scope], _blob=blob))
    for variants in out.values():
        for v in variants:
            del v["_blob"]
    return out


def build_system(raw):
    gateway = load(raw, "scripting.json")
    others = {s["scope"]: s for s in load(raw, "scopes.json")["scopes"]}
    gateway_required = {e["path"]: set(e["required_probe"]["required"])
                        for e in gateway["live"] if "required_probe" in e}
    per_scope = {}
    per_scope["gateway"] = dict(zip(("functions", "constants", "objects", "packages"),
                                    scope_tree(gateway, gateway_required)))
    for scope in ("client", "designer"):
        per_scope[scope] = dict(zip(("functions", "constants", "objects", "packages"),
                                    scope_tree(others[scope], gateway_required)))
    packages = {}
    for scope in SCOPES:
        for p in per_scope[scope]["packages"]:
            packages.setdefault(p, []).append(scope)
    functions = merge_scopes(per_scope, "functions")
    constants = merge_scopes(per_scope, "constants")
    objects = merge_scopes(per_scope, "objects")
    return {
        "ignition_version": IGNITION_VERSION,
        "build": gateway["ignition_version"],
        "docs_url_pattern": SCRIPTING_DOCS,
        "scopes": list(SCOPES),
        "packages": [{"path": p, "scopes": packages[p]} for p in sorted(packages)],
        "functions": [{"path": p, "variants": functions[p]} for p in sorted(functions)],
        "constants": [{"path": p, "variants": constants[p]} for p in sorted(constants)],
        "objects": [{"path": p, "variants": objects[p]} for p in sorted(objects)],
        "registrations": {s: others[s]["registrations"] for s in ("client", "designer")},
    }


# ---------------------------------------------------------------------------
# Expression functions

def arity(counts, probe_max):
    """(min, max or None, step) describing the accepted argument counts."""
    if not counts:
        return None
    lo = counts[0]
    unbounded = counts[-1] == probe_max
    if len(counts) == 1:
        return {"min": lo, "max": None if unbounded else lo, "step": 1}
    step = counts[1] - counts[0]
    if all(b - a == step for a, b in zip(counts, counts[1:])):
        return {"min": lo, "max": None if unbounded else counts[-1], "step": step}
    raise ValueError("irregular arity %r" % counts)


def build_expression(raw):
    dump = load(raw, "expression.json")
    funcs = []
    for f in dump["functions"]:
        funcs.append({
            "name": f["name"],
            "categories": [html.unescape(c) for c in f["categories"]],
            "arity": arity(f["accepted_counts"], f["probe_max"]),
            "params": f.get("arg_hint") or "",
        })
    funcs.sort(key=lambda f: f["name"].lower())
    # The functions Perspective's own expressions (bindings, transforms and
    # the other expressions of a view) add to the gateway's, or replace.
    contexts = {}
    for context, data in sorted(load(raw, "perspective_expression.json").items()):
        added = []
        for f in data["functions"]:
            added.append({
                "name": f["name"],
                "categories": [html.unescape(c) for c in f["categories"]],
                "arity": arity(f["accepted_counts"], f["probe_max"]),
                "params": f.get("arg_hint") or "",
                "replaces_gateway_function": f["replaces"] is not None,
            })
        added.sort(key=lambda f: f["name"].lower())
        contexts[context] = {"factory": data["factory"], "functions": added}
    return {
        "ignition_version": IGNITION_VERSION,
        "scope": "gateway",
        "arity_source": "initArgs probe with 0..%d constant arguments" % dump["functions"][0]["probe_max"],
        "docs_url_pattern": EXPRESSION_DOCS,
        "functions": funcs,
        "contexts": contexts,
    }


# ---------------------------------------------------------------------------
# Perspective components

PROSE_KEYS = {"description", "title", "example", "examples", "markdownDescription",
              "documentation", "docs", "deprecationMessage", "suggestions"}
SCHEMA_KEYS = {"type", "properties", "items", "enum", "default", "required",
               "additionalProperties", "patternProperties", "$ref", "definitions",
               "anyOf", "oneOf", "allOf", "not", "minimum", "maximum",
               "exclusiveMinimum", "exclusiveMaximum", "minItems", "maxItems",
               "minLength", "maxLength", "pattern", "format", "const", "deprecated",
               "extension", "visibleWhen", "dynamicSuggestions", "minProperties",
               "maxProperties", "uniqueItems", "multipleOf", "$id", "$schema", "id"}


def strip_schema(node, path="#"):
    """Keeps the structural JSON-schema keywords only. Unknown keywords are
    reported so a new Ignition release cannot leak prose silently."""
    if isinstance(node, list):
        return [strip_schema(x, path) for x in node]
    if not isinstance(node, dict):
        return node
    out = {}
    for k, v in node.items():
        if k in PROSE_KEYS:
            continue
        if k in ("properties", "patternProperties", "definitions"):
            out[k] = {pk: strip_schema(pv, path + "/" + pk) for pk, pv in v.items()}
        elif k == "default":
            # Small defaults are facts; large ones are sample content.
            if len(json.dumps(v)) <= MAX_DEFAULT_JSON and not (isinstance(v, str) and "\n" in v):
                out[k] = v
        elif k in ("const", "enum"):
            out[k] = v
        elif k == "extension":
            # Designer-only editor metadata; keep only flags, never text.
            out[k] = {ek: ev for ek, ev in v.items() if isinstance(ev, bool)}
            if not out[k]:
                del out[k]
        elif k in ("dynamicSuggestions", "visibleWhen"):
            continue
        elif k in SCHEMA_KEYS:
            out[k] = strip_schema(v, path)
        else:
            UNKNOWN_SCHEMA_KEYS.add(k)
    return out


UNKNOWN_SCHEMA_KEYS = set()
MAX_DEFAULT_JSON = 200


def build_perspective(raw):
    dump = load(raw, "perspective.json")
    comps = []
    for c in dump["components"]:
        comps.append({
            "id": c["id"],
            "name": c["name"],
            "palette_category": c["palette_category"],
            "module_id": c["module_id"],
            "deprecated": c["deprecated"],
            "props_schema": strip_schema(json.loads(c["schema"])) if c["schema"] else None,
            "child_position_schema": strip_schema(json.loads(c["child_position_schema"]))
            if c["child_position_schema"] else None,
            "events": [{"name": e["name"],
                        "schema": strip_schema(json.loads(e["schema"])) if e.get("schema") else None}
                       for e in c["events"]],
            "extension_functions": c["extension_functions"],
        })
    if UNKNOWN_SCHEMA_KEYS:
        sys.stderr.write("normalize: dropped unknown schema keys: %s\n" % sorted(UNKNOWN_SCHEMA_KEYS))
    shared = {name: strip_schema(json.loads(text)) for name, text in sorted(dump["shared_schemas"].items())
              if text is not None}
    return {"ignition_version": IGNITION_VERSION, "components": comps, "shared_schemas": shared,
            "shared_schema_ref_prefix": "urn:ignition-schema:schemas/"}


def build_builtins(raw):
    dump = load(raw, "builtins.json")
    return {"ignition_version": IGNITION_VERSION, "jython_version": dump["jython_version"],
            "builtins": dump["builtins"]}


def write(path, data):
    with open(path, "w") as f:
        json.dump(data, f, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
        f.write("\n")


def main():
    raw, data = sys.argv[1:3]
    os.makedirs(data, exist_ok=True)
    write(os.path.join(data, "system.json"), build_system(raw))
    write(os.path.join(data, "expression.json"), build_expression(raw))
    write(os.path.join(data, "perspective.json"), build_perspective(raw))
    write(os.path.join(data, "builtins.json"), build_builtins(raw))


if __name__ == "__main__":
    main()
