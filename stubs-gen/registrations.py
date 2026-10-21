#!/usr/bin/env python3
"""Finds how Ignition registers `system.*` script modules per scope.

The Vision Client and Designer scopes cannot be introspected in a gateway, so
we read the registration code instead: every class that calls
`ScriptManager.addScriptModule("system.x", <object>, ...)` is scanned at the
bytecode level and the class of `<object>` is recorded. introspect/scopes.py
then loads those classes inside the gateway and reflects them exactly like
the gateway-scope functions.

Usage: registrations.py <lib/core dir> <modules dir> <out.json>
"""

import io
import json
import os
import re
import struct
import sys
import xml.etree.ElementTree as ET
import zipfile

# ---------------------------------------------------------------------------
# Minimal class-file reader: constant pool and method code.

OPCODE_LEN = {}
for op in range(256):
    OPCODE_LEN[op] = 1
for op in (0x10, 0x12, 0x15, 0x16, 0x17, 0x18, 0x19, 0x36, 0x37, 0x38, 0x39, 0x3A, 0xA9, 0xBC):
    OPCODE_LEN[op] = 2
for op in (0x11, 0x13, 0x14, 0x84, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xBB, 0xBD, 0xC0, 0xC1, 0xC6, 0xC7):
    OPCODE_LEN[op] = 3
for op in range(0x99, 0xA9):  # if*, goto, jsr
    OPCODE_LEN[op] = 3
OPCODE_LEN[0xC5] = 4  # multianewarray
OPCODE_LEN[0xB9] = 5  # invokeinterface
OPCODE_LEN[0xBA] = 5  # invokedynamic
OPCODE_LEN[0xC8] = 5  # goto_w
OPCODE_LEN[0xC9] = 5  # jsr_w


class ClassFile:
    def __init__(self, data):
        self.data = data
        self.cp = [None]
        pos = 10
        count = struct.unpack(">H", data[8:10])[0]
        i = 1
        while i < count:
            tag = data[pos]
            if tag == 1:
                n = struct.unpack(">H", data[pos + 1:pos + 3])[0]
                self.cp.append(("utf8", data[pos + 3:pos + 3 + n].decode("utf-8", "replace")))
                pos += 3 + n
            elif tag in (3, 4):
                self.cp.append(("num", None))
                pos += 5
            elif tag in (5, 6):
                self.cp.append(("num", None))
                self.cp.append(None)
                pos += 9
                i += 1
            elif tag == 7:
                self.cp.append(("class", struct.unpack(">H", data[pos + 1:pos + 3])[0]))
                pos += 3
            elif tag == 8:
                self.cp.append(("string", struct.unpack(">H", data[pos + 1:pos + 3])[0]))
                pos += 3
            elif tag in (9, 10, 11):
                c, nt = struct.unpack(">HH", data[pos + 1:pos + 5])
                self.cp.append(("ref", c, nt))
                pos += 5
            elif tag == 12:
                n, d = struct.unpack(">HH", data[pos + 1:pos + 5])
                self.cp.append(("nat", n, d))
                pos += 5
            elif tag == 15:
                self.cp.append(("handle", None))
                pos += 4
            elif tag == 16:
                self.cp.append(("mtype", None))
                pos += 3
            elif tag in (17, 18):
                self.cp.append(("dyn", None))
                pos += 5
            elif tag in (19, 20):
                self.cp.append(("module", None))
                pos += 3
            else:
                raise ValueError("bad constant tag %d" % tag)
            i += 1
        pos += 6  # access, this, super
        this_index = struct.unpack(">H", data[pos - 4:pos - 2])[0]
        self.name = self.class_name(this_index)
        n_if = struct.unpack(">H", data[pos:pos + 2])[0]
        pos += 2 + 2 * n_if
        pos = self._skip_members(pos)  # fields
        self.methods = []
        n_methods = struct.unpack(">H", data[pos:pos + 2])[0]
        pos += 2
        for _ in range(n_methods):
            _, name_i, desc_i, n_attr = struct.unpack(">HHHH", data[pos:pos + 8])
            pos += 8
            code = None
            for _ in range(n_attr):
                an, alen = struct.unpack(">HI", data[pos:pos + 6])
                if self.utf8(an) == "Code":
                    clen = struct.unpack(">I", data[pos + 10:pos + 14])[0]
                    code = data[pos + 14:pos + 14 + clen]
                pos += 6 + alen
            self.methods.append((self.utf8(name_i), self.utf8(desc_i), code))

    def _skip_members(self, pos):
        n = struct.unpack(">H", self.data[pos:pos + 2])[0]
        pos += 2
        for _ in range(n):
            n_attr = struct.unpack(">H", self.data[pos + 6:pos + 8])[0]
            pos += 8
            for _ in range(n_attr):
                alen = struct.unpack(">I", self.data[pos + 2:pos + 6])[0]
                pos += 6 + alen
        return pos

    def utf8(self, i):
        return self.cp[i][1]

    def class_name(self, i):
        return self.utf8(self.cp[i][1]).replace("/", ".")

    def string(self, i):
        e = self.cp[i]
        return self.utf8(e[1]) if e and e[0] == "string" else None

    def ref(self, i):
        _, c, nt = self.cp[i]
        _, n, d = self.cp[nt]
        return self.class_name(c), self.utf8(n), self.utf8(d)

    def instructions(self, code):
        pc = 0
        while pc < len(code):
            op = code[pc]
            if op == 0xAA:  # tableswitch
                p = (pc + 4) & ~3
                low, high = struct.unpack(">ii", code[p + 4:p + 12])
                length = p + 12 + 4 * (high - low + 1) - pc
            elif op == 0xAB:  # lookupswitch
                p = (pc + 4) & ~3
                npairs = struct.unpack(">i", code[p + 4:p + 8])[0]
                length = p + 8 + 8 * npairs - pc
            elif op == 0xC4:  # wide
                length = 6 if code[pc + 1] == 0x84 else 4
            else:
                length = OPCODE_LEN[op]
            yield pc, op, code[pc:pc + length]
            pc += length


def _return_type(desc):
    ret = desc[desc.index(")") + 1:]
    if ret.startswith("L") and ret.endswith(";"):
        return ret[1:-1].replace("/", ".")
    return None


def registrations_in_class(cf):
    """Yields (method, path, candidates, descriptor) for each addScriptModule
    call. Candidates are the classes of the values evaluated between the path
    constant and the call, in order; the first one is the module object."""
    for mname, _, code in cf.methods:
        if code is None:
            continue
        pending = None
        last_new = None
        locals_ = {}
        for _, op, ins in cf.instructions(code):
            if op in (0x3A, 0x4B, 0x4C, 0x4D, 0x4E):  # astore
                idx = ins[1] if op == 0x3A else op - 0x4B
                locals_[idx] = last_new
            if op in (0x19, 0x2A, 0x2B, 0x2C, 0x2D):  # aload
                idx = ins[1] if op == 0x19 else op - 0x2A
                if pending is not None and idx != 0 and locals_.get(idx):
                    pending["candidates"].append(("local", locals_[idx]))
                continue
            if op in (0x12, 0x13):
                idx = ins[1] if op == 0x12 else struct.unpack(">H", ins[1:3])[0]
                s = cf.string(idx)
                if s is not None and re.fullmatch(r"system(\.[A-Za-z0-9_]+)+", s):
                    pending = {"path": s, "candidates": []}
                    continue
                entry = cf.cp[idx]
                if pending is not None and entry and entry[0] == "class":
                    pending["candidates"].append(("class_literal", cf.class_name(idx)))
            elif op == 0xBB:  # new
                last_new = cf.class_name(struct.unpack(">H", ins[1:3])[0])
                if pending is not None:
                    pending["candidates"].append(("new", last_new))
            elif pending is None:
                continue
            elif op in (0xB2, 0xB4):  # getstatic, getfield
                _, _, desc = cf.ref(struct.unpack(">H", ins[1:3])[0])
                if desc.startswith("L"):
                    pending["candidates"].append(("field", desc[1:-1].replace("/", ".")))
            elif op in (0xB6, 0xB7, 0xB8, 0xB9):
                owner, name, desc = cf.ref(struct.unpack(">H", ins[1:3])[0])
                if name == "addScriptModule":
                    yield mname, pending["path"], pending["candidates"], desc
                    pending = None
                elif name != "<init>":
                    rt = _return_type(desc)
                    if rt:
                        pending["candidates"].append(("call", rt))


# ---------------------------------------------------------------------------
# Jar discovery per scope.

def module_jars(modules_dir):
    """(module_id, scope, jar_name, bytes, hooks) for every jar in every .modl."""
    for f in sorted(os.listdir(modules_dir)):
        if not f.endswith(".modl"):
            continue
        z = zipfile.ZipFile(os.path.join(modules_dir, f))
        root = ET.fromstring(z.read("module.xml"))
        mod = root.find("module")
        mid = mod.findtext("id")
        hooks = {h.get("scope"): h.text.strip() for h in mod.findall("hook")}
        for j in mod.findall("jar"):
            yield mid, j.get("scope"), j.text.strip(), z.read(j.text.strip()), hooks, f


def main():
    core, modules, out_path = sys.argv[1:4]
    jars = []  # (origin, scopes, name, bytes)
    for sub, scopes in (("common", "CDG"), ("client", "CD"), ("designer", "D"), ("gateway", "G")):
        d = os.path.join(core, sub)
        if not os.path.isdir(d):
            continue
        for f in sorted(os.listdir(d)):
            if f.endswith(".jar"):
                jars.append(("core/" + sub, scopes, f, open(os.path.join(d, f), "rb").read()))
    hooks_by_module = {}
    module_files = {}
    for mid, scope, name, data, hooks, modl in module_jars(modules):
        jars.append((mid, scope, name, data))
        hooks_by_module[mid] = hooks
        module_files.setdefault(mid, (modl, []))[1].append({"jar": name, "scope": scope})

    regs = []
    for origin, scopes, name, data in jars:
        try:
            z = zipfile.ZipFile(io.BytesIO(data))
        except zipfile.BadZipFile:
            continue
        for n in z.namelist():
            if not n.endswith(".class"):
                continue
            b = z.read(n)
            if b"addScriptModule" not in b:
                continue
            cf = ClassFile(b)
            for method, path, candidates, desc in registrations_in_class(cf):
                regs.append({
                    "origin": origin,
                    "jar": name,
                    "jar_scopes": scopes,
                    "registrar": cf.name,
                    "method": method,
                    "path": path,
                    "candidates": candidates,
                    "add_descriptor": desc,
                })
    json.dump({
        "registrations": regs,
        "module_hooks": hooks_by_module,
        "module_jars": {k: {"modl": v[0], "jars": v[1]} for k, v in module_files.items()},
    }, open(out_path, "w"), indent=1, sort_keys=True)


if __name__ == "__main__":
    main()
