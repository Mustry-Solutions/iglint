#!/usr/bin/env python3
"""Generates crates/iglint_stubs/src/generated.rs from the committed facts in
crates/iglint_stubs/data. The tables are plain `static` data, so nothing is
parsed at run time.

Usage: codegen.py <data dir> <generated.rs>
"""

import json
import math
import os
import sys

SCOPE_BITS = {"gateway": 1, "client": 2, "designer": 4}
TYPE_BITS = {
    "object": "OBJECT", "array": "ARRAY", "string": "STRING", "number": "NUMBER",
    "Number": "NUMBER", "integer": "INTEGER", "boolean": "BOOLEAN", "null": "NULL",
    "date": "DATE", "dataset": "DATASET", "color": "COLOR",
}


def rs_str(s):
    out = ['"']
    for ch in s:
        if ch == "\\":
            out.append("\\\\")
        elif ch == '"':
            out.append('\\"')
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\t":
            out.append("\\t")
        elif ch == "\r":
            out.append("\\r")
        elif ord(ch) < 0x20 or ord(ch) == 0x7F:
            out.append("\\u{%x}" % ord(ch))
        else:
            out.append(ch)
    out.append('"')
    return "".join(out)


def rs_opt_str(s):
    return "None" if s is None else "Some(%s)" % rs_str(s)


def rs_scopes(scopes):
    return "Scopes::from_bits(%d)" % sum(SCOPE_BITS[s] for s in scopes)


def rs_strs(items):
    return "&[" + ", ".join(rs_str(x) for x in items) + "]"


def rs_f64(x):
    if isinstance(x, bool) or x is None:
        raise ValueError(x)
    x = float(x)
    if math.isinf(x) or math.isnan(x):
        raise ValueError(x)
    r = repr(x)
    return r if ("." in r or "e" in r) else r + ".0"


def json_text(v):
    return json.dumps(v, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


# ---------------------------------------------------------------------------
# system.*

def gen_param(p):
    parts = ["name: %s" % rs_opt_str(p.get("name")),
             "ty: %s" % rs_opt_str(p.get("type")),
             "required: %s" % ("true" if p["required"] else "false"),
             "default: %s" % rs_opt_str(p.get("default"))]
    return "Param { %s }" % ", ".join(parts)


def gen_overload(o):
    kind = {
        "positional": "OverloadKind::Positional",
        "keyword": "OverloadKind::Keyword",
        "unchecked": "OverloadKind::Unchecked",
    }.get(o["kind"])
    if o["kind"] == "python":
        kind = "OverloadKind::Python { varargs: %s, varkw: %s }" % (
            "true" if o.get("varargs") else "false", "true" if o.get("varkw") else "false")
    return "Overload { kind: %s, params: &[%s], returns: %s }" % (
        kind, ", ".join(gen_param(p) for p in o["params"]), rs_opt_str(o.get("returns")))


def gen_system(data, out):
    out.append("pub(crate) static PACKAGES: &[Package] = &[")
    for p in data["packages"]:
        out.append("    Package { path: %s, scopes: %s }," % (rs_str(p["path"]), rs_scopes(p["scopes"])))
    out.append("];\n")

    out.append("pub(crate) static FUNCTIONS: &[Function] = &[")
    for f in data["functions"]:
        variants = []
        for v in f["variants"]:
            variants.append(
                "FunctionVariant { scopes: %s, deprecated: %s, replacement: %s, hidden: %s, overloads: &[%s] }" % (
                    rs_scopes(v["scopes"]), "true" if v["deprecated"] else "false",
                    rs_opt_str(v.get("replacement")), "true" if v["hidden"] else "false",
                    ", ".join(gen_overload(o) for o in v["overloads"])))
        out.append("    Function { path: %s, variants: &[%s] }," % (rs_str(f["path"]), ", ".join(variants)))
    out.append("];\n")

    out.append("pub(crate) static CONSTANTS: &[Constant] = &[")
    for c in data["constants"]:
        variants = []
        for v in c["variants"]:
            t, val = v["type"], v["value"]
            if t == "bool":
                value = "ConstValue::Bool(%s)" % ("true" if val else "false")
            elif t == "int":
                value = "ConstValue::Int(%d)" % val
            elif t == "float":
                value = "ConstValue::Float(%s)" % rs_f64(val)
            else:
                value = "ConstValue::Str(%s)" % rs_str(val)
            variants.append("ConstantVariant { scopes: %s, value: %s }" % (rs_scopes(v["scopes"]), value))
        out.append("    Constant { path: %s, variants: &[%s] }," % (rs_str(c["path"]), ", ".join(variants)))
    out.append("];\n")

    out.append("pub(crate) static OBJECTS: &[ScriptObject] = &[")
    for o in data["objects"]:
        variants = []
        for v in o["variants"]:
            kind = "ObjectKind::Class" if v["kind"] == "class" else "ObjectKind::Instance"
            variants.append("ObjectVariant { scopes: %s, kind: %s, ty: %s }" % (
                rs_scopes(v["scopes"]), kind, rs_opt_str(v.get("type"))))
        out.append("    ScriptObject { path: %s, variants: &[%s] }," % (rs_str(o["path"]), ", ".join(variants)))
    out.append("];\n")


# ---------------------------------------------------------------------------
# Expression functions

def gen_expression(data, out):
    funcs = sorted(data["functions"], key=lambda f: f["name"].lower())
    out.append("pub(crate) static EXPRESSION_FUNCTIONS: &[ExpressionFunction] = &[")
    for f in funcs:
        a = f["arity"]
        arity = "None" if a is None else "Some(Arity { min: %d, max: %s, step: %d })" % (
            a["min"], "None" if a["max"] is None else "Some(%d)" % a["max"], a["step"])
        out.append("    ExpressionFunction { name: %s, key: %s, categories: %s, arity: %s, params: %s }," % (
            rs_str(f["name"]), rs_str(f["name"].lower()), rs_strs(f["categories"]), arity, rs_str(f["params"])))
    out.append("];\n")


# ---------------------------------------------------------------------------
# Perspective

class SchemaPool:
    """Emits one static per distinct schema node."""

    def __init__(self):
        self.ids = {}
        self.lines = []

    def node(self, n):
        if n is None:
            return None
        key = json_text(n)
        if key in self.ids:
            return self.ids[key]
        fields = []
        t = n.get("type")
        types = t if isinstance(t, list) else ([t] if t else [])
        bits = sorted(set(TYPE_BITS.get(x, "OTHER") for x in types))
        if bits:
            fields.append("types: TypeSet(%s)" % " | ".join("TypeSet::%s.0" % b for b in bits))
        props = n.get("properties") or {}
        if props:
            fields.append("properties: &[%s]" % ", ".join(
                "(%s, &%s)" % (rs_str(k), self.node(props[k])) for k in sorted(props)))
        defs = n.get("definitions") or {}
        if defs:
            fields.append("definitions: &[%s]" % ", ".join(
                "(%s, &%s)" % (rs_str(k), self.node(defs[k])) for k in sorted(defs)))
        req = n.get("required")
        if isinstance(req, list) and req:
            fields.append("required: %s" % rs_strs(sorted(req)))
        if isinstance(n.get("items"), dict):
            fields.append("items: Some(&%s)" % self.node(n["items"]))
        ap = n.get("additionalProperties")
        if ap is True:
            fields.append("additional_properties: Additional::Allowed")
        elif ap is False:
            fields.append("additional_properties: Additional::Forbidden")
        elif isinstance(ap, dict):
            fields.append("additional_properties: Additional::Schema(&%s)" % self.node(ap))
        if n.get("$ref"):
            fields.append("reference: Some(%s)" % rs_str(n["$ref"]))
        for jkey, field in (("oneOf", "one_of"), ("anyOf", "any_of"), ("allOf", "all_of")):
            if n.get(jkey):
                fields.append("%s: &[%s]" % (field, ", ".join("&" + self.node(x) for x in n[jkey])))
        if "enum" in n:
            fields.append("enum_values: %s" % rs_strs([json_text(x) for x in n["enum"]]))
        if "default" in n:
            fields.append("default: Some(%s)" % rs_str(json_text(n["default"])))
        if n.get("format"):
            fields.append("format: Some(%s)" % rs_str(str(n["format"])))
        for jkey in ("minimum", "maximum"):
            if isinstance(n.get(jkey), (int, float)) and not isinstance(n.get(jkey), bool):
                fields.append("%s: Some(%s)" % (jkey, rs_f64(n[jkey])))
        if n.get("pattern"):
            fields.append("pattern: Some(%s)" % rs_str(n["pattern"]))
        if n.get("deprecated") is True:
            fields.append("deprecated: true")
        name = "S%d" % len(self.ids)
        self.ids[key] = name
        fields.append("..Schema::EMPTY")
        self.lines.append("static %s: Schema = Schema { %s };" % (name, ", ".join(fields)))
        return name


def gen_perspective(data, out):
    pool = SchemaPool()
    comps = []
    for c in sorted(data["components"], key=lambda c: c["id"]):
        props = pool.node(c["props_schema"] or {})
        child = pool.node(c["child_position_schema"])
        events = ", ".join("ComponentEvent { name: %s, schema: %s }" % (
            rs_str(e["name"]), "None" if e["schema"] is None else "Some(&%s)" % pool.node(e["schema"]))
            for e in c["events"])
        exts = ", ".join("ExtensionFunction { name: %s, signature: %s }" % (
            rs_str(x["name"]), rs_str(x["signature"])) for x in c["extension_functions"])
        comps.append(
            "    ComponentType { id: %s, name: %s, palette_category: %s, module_id: %s, deprecated: %s, "
            "props: &%s, child_position: %s, events: &[%s], extension_functions: &[%s] }," % (
                rs_str(c["id"]), rs_str(c["name"]), rs_opt_str(c["palette_category"]),
                rs_str(c["module_id"]), "true" if c["deprecated"] else "false", props,
                "None" if child is None else "Some(&%s)" % child, events, exts))
    shared = []
    for name in sorted(data["shared_schemas"]):
        shared.append("    (%s, &%s)," % (rs_str(name), pool.node(data["shared_schemas"][name])))
    out.extend(pool.lines)
    out.append("")
    out.append("pub(crate) static COMPONENT_TYPES: &[ComponentType] = &[")
    out.extend(comps)
    out.append("];\n")
    out.append("pub(crate) static SHARED_SCHEMAS: &[(&str, &Schema)] = &[")
    out.extend(shared)
    out.append("];\n")


def gen_builtins(data, out):
    kinds = {"type": "BuiltinKind::Type", "function": "BuiltinKind::Function",
             "exception": "BuiltinKind::Exception", "constant": "BuiltinKind::Constant"}
    out.append("pub(crate) static BUILTINS: &[Builtin] = &[")
    for b in sorted(data["builtins"], key=lambda b: b["name"]):
        out.append("    Builtin { name: %s, kind: %s }," % (rs_str(b["name"]), kinds[b["kind"]]))
    out.append("];\n")


def main():
    data_dir, out_path = sys.argv[1:3]
    load = lambda n: json.load(open(os.path.join(data_dir, n)))
    system = load("system.json")
    out = [
        "// @generated by stubs-gen/codegen.py from crates/iglint_stubs/data. Do not edit.",
        "// Facts about Ignition %s (%s), introspected from a real gateway." % (
            system["ignition_version"], system["build"]),
        "",
        "use crate::*;",
        "",
        "pub(crate) const IGNITION_VERSION: &str = %s;" % rs_str(system["ignition_version"]),
        "pub(crate) const IGNITION_BUILD: &str = %s;" % rs_str(system["build"]),
        "pub(crate) const JYTHON_VERSION: &str = %s;" % rs_str(load("builtins.json")["jython_version"]),
        "",
    ]
    gen_system(system, out)
    gen_expression(load("expression.json"), out)
    gen_perspective(load("perspective.json"), out)
    gen_builtins(load("builtins.json"), out)
    with open(out_path, "w") as f:
        f.write("\n".join(out) + "\n")


if __name__ == "__main__":
    main()
