#!/usr/bin/env python3
"""Finds the expression functions of Ignition that can run Python.

Scans every class of the image's common and gateway jars and of every
module file, keeps those that implement Ignition's expression `Function`
(through any chain of superclasses and interfaces the jars hold), and
prints each one whose class file, or a nested class of it, refers to
Jython or to the script manager (`org/python/...`, `ScriptManager`,
`runFunction`, `runCode`, `compileFunction`, `PythonInterpreter`,
`javax/script`). On 8.3.3 the result is `ScriptFunction` (common's
`runScript`), Perspective's `PerspectiveRunScriptFunction` (its
subclass), Vision's `ComponentRunScriptExpression` (`objectScript`, Vision
clients only) and `ColumnRearrangeFunction` (converts a Python list); see
docs/internal/fix-safety.md, "Expressions and resources iglint does not
read".

Usage: code_functions.py <lib/core dir> <modules dir>
(the directories of the image: /usr/local/bin/ignition/lib/core and
/usr/local/bin/ignition/user-lib/modules, copied out with `docker cp`).
"""

import io
import os
import re
import struct
import sys
import zipfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from registrations import ClassFile, module_jars  # noqa: E402

FUNCTION = "com.inductiveautomation.ignition.common.expressions.functions.Function"
PYTHON = re.compile(
    r"org/python/|org\.python\.|ScriptManager|runFunction|compileFunction|runCode|"
    r"PythonInterpreter|javax/script|ScriptEngine"
)


def hierarchy(cf, data):
    """(access flags, superclass, interfaces) of a class file."""
    pos = 10
    count = struct.unpack(">H", data[8:10])[0]
    i = 1
    while i < count:
        tag = data[pos]
        if tag == 1:
            pos += 3 + struct.unpack(">H", data[pos + 1:pos + 3])[0]
        elif tag in (3, 4):
            pos += 5
        elif tag in (5, 6):
            pos += 9
            i += 1
        elif tag in (7, 8, 16, 19, 20):
            pos += 3
        elif tag in (9, 10, 11, 12, 17, 18):
            pos += 5
        elif tag == 15:
            pos += 4
        i += 1
    access, _, sup = struct.unpack(">HHH", data[pos:pos + 6])
    n = struct.unpack(">H", data[pos + 6:pos + 8])[0]
    ifs = [cf.class_name(struct.unpack(">H", data[pos + 8 + 2 * k:pos + 10 + 2 * k])[0])
           for k in range(n)]
    return access, (cf.class_name(sup) if sup else None), ifs


def main():
    core, modules = sys.argv[1], sys.argv[2]
    classes = {}

    def add(data, origin):
        try:
            z = zipfile.ZipFile(io.BytesIO(data))
        except zipfile.BadZipFile:
            return
        for n in z.namelist():
            if not n.endswith(".class"):
                continue
            d = z.read(n)
            try:
                cf = ClassFile(d)
            except (ValueError, IndexError, struct.error):
                continue
            classes[cf.name] = (cf, hierarchy(cf, d), origin)

    for sub in ("common", "gateway"):
        folder = os.path.join(core, sub)
        for f in sorted(os.listdir(folder)):
            if f.endswith(".jar"):
                with open(os.path.join(folder, f), "rb") as fh:
                    add(fh.read(), sub + "/" + f)
    for _, _, jar, data, _, modl in module_jars(modules):
        add(data, modl + ":" + jar)

    memo = {}

    def is_function(name, depth=0):
        if name == FUNCTION:
            return True
        if name in memo:
            return memo[name]
        if name not in classes or depth > 50:
            return False
        _, sup, ifs = classes[name][1]
        result = any(is_function(x, depth + 1) for x in ([sup] if sup else []) + ifs)
        memo[name] = result
        return result

    def refers(cf):
        return {e[1] for e in cf.cp if e and e[0] == "utf8" and PYTHON.search(e[1])}

    functions = sorted(n for n in classes if is_function(n))
    print("expression function classes: %d" % len(functions))
    for name in functions:
        found = refers(classes[name][0])
        for other in classes:
            if other.startswith(name + "$"):
                found |= refers(classes[other][0])
        if found:
            print("%s\t%s" % (name, classes[name][2]))


if __name__ == "__main__":
    main()
