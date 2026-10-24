#!/usr/bin/env python3
"""Finds the expression functions Perspective adds for its own expressions.

Perspective bindings, expression transforms and the other expressions of a
view are compiled with `ExpressionBindingParseContext`, whose function
factory (`PerspectiveElementFunctionFactory`) extends the gateway's
factory with functions of its own. The factory's constructor is read at
the bytecode level: every `addFunction("<name>", "<category>", new
<Class>(...))` call is recorded. introspect/perspective_expression.py then
loads those classes inside the gateway and probes their argument counts
like introspect/expression.py does for the gateway's functions.

Usage: expression_contexts.py <modules dir> <out.json>
"""

import io
import json
import os
import struct
import sys
import zipfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from registrations import ClassFile, module_jars  # noqa: E402

FACTORIES = {
    "perspective": "com.inductiveautomation.perspective.gateway.expression.PerspectiveElementFunctionFactory",
}


def additions(cf):
    """(name, category, class) of every addFunction call of the constructor."""
    out = []
    for mname, _, code in cf.methods:
        if mname != "<init>" or code is None:
            continue
        strings = []
        new = None
        for _, op, ins in cf.instructions(code):
            if op in (0x12, 0x13):
                idx = ins[1] if op == 0x12 else struct.unpack(">H", ins[1:3])[0]
                s = cf.string(idx)
                if s is not None:
                    strings.append(s)
            elif op == 0xBB:  # new
                new = cf.class_name(struct.unpack(">H", ins[1:3])[0])
            elif op in (0xB6, 0xB7):
                _, name, desc = cf.ref(struct.unpack(">H", ins[1:3])[0])
                if name == "addFunction" and len(strings) >= 2 and new:
                    out.append({"name": strings[-2], "category": strings[-1], "java_class": new})
                    strings = []
                    new = None
    return out


def main():
    modules, out_path = sys.argv[1:3]
    wanted = {v.replace(".", "/") + ".class": k for k, v in FACTORIES.items()}
    found = {}
    for mid, scope, name, data, _, _ in module_jars(modules):
        if "G" not in (scope or ""):
            continue
        z = zipfile.ZipFile(io.BytesIO(data))
        for n in z.namelist():
            if n in wanted:
                found[wanted[n]] = {"module": mid, "factory": FACTORIES[wanted[n]],
                                    "functions": additions(ClassFile(z.read(n)))}
    missing = sorted(set(FACTORIES) - set(found))
    if missing:
        raise SystemExit("function factories not found: %s" % missing)
    with open(out_path, "w") as f:
        json.dump(found, f, indent=1, sort_keys=True)


if __name__ == "__main__":
    main()
