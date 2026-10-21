# Runs inside the gateway (Jython 2.7) through the stubs-gen WebDev endpoint,
# appended to scripting.py (which provides walk_live and walk_hints).
#
# Rebuilds the Vision Client and Designer `system.*` trees without a client:
# run.sh passes REGISTRATIONS, the addScriptModule calls that
# stubs-gen/registrations.py found in the client and designer bytecode. Here
# the client and designer jars (core and from every module) are loaded in a
# private class loader, a fresh ScriptManager is created per scope (its
# constructor registers the scope-independent modules) and every registration
# is replayed with an instance allocated without running a constructor.
# Functions are only reflected, never called.
# The result is stored in `result`.

import os

from java.io import File, FileOutputStream
from java.lang import Class as _JClass
from java.lang import Throwable as _JThrowable
from java.net import URL, URLClassLoader
from java.util.zip import ZipFile
from com.inductiveautomation.ignition.common.script import ScriptManager
from com.inductiveautomation.ignition.common.script.hints import PropertiesFileDocProvider

IGNITION = "/usr/local/bin/ignition"
JAR_DIR = IGNITION + "/temp/iglint-stubs-jars"

CORE_REGISTRARS = {
    "client": [
        "com.inductiveautomation.ignition.client.model.ClientContextImpl",
        "com.inductiveautomation.ignition.client.ClientContextImpl",
        "com.inductiveautomation.factorypmi.application.runtime.ClientContextImpl",
        "com.inductiveautomation.factorypmi.application.FPMIApp",
    ],
    "designer": [
        "com.inductiveautomation.ignition.designer.DesignerContextImpl",
        "com.inductiveautomation.factorypmi.application.FPMIApp",
    ],
}
SCOPE_LETTER = {"client": "C", "designer": "D"}
NOT_MODULE_OBJECTS = ("DocProvider", "Context", "Manager", "Rpc", "java.lang.String")


def _extract_module_jars():
    jars = []
    modules = IGNITION + "/user-lib/modules"
    for f in sorted(os.listdir(modules)):
        if not f.endswith(".modl"):
            continue
        z = ZipFile(File(modules + "/" + f))
        try:
            entries = z.entries()
            while entries.hasMoreElements():
                e = entries.nextElement()
                name = e.getName()
                if not name.endswith(".jar"):
                    continue
                target = File(JAR_DIR + "/" + f.replace(" ", "_") + "/" + name)
                target.getParentFile().mkdirs()
                ins = z.getInputStream(e)
                outs = FileOutputStream(target)
                try:
                    buf = jarray_bytes(65536)
                    while True:
                        n = ins.read(buf)
                        if n < 0:
                            break
                        outs.write(buf, 0, n)
                finally:
                    outs.close()
                    ins.close()
                jars.append(target)
        finally:
            z.close()
    return jars


def jarray_bytes(n):
    import jarray
    return jarray.zeros(n, "b")


def _class_loader():
    files = []
    for sub in ("client", "designer"):
        d = IGNITION + "/lib/core/" + sub
        files.extend(File(d + "/" + f) for f in sorted(os.listdir(d)) if f.endswith(".jar"))
    files.extend(_extract_module_jars())
    urls = [f.toURI().toURL() for f in files]
    import jarray
    return URLClassLoader(jarray.array(urls, URL), ScriptManager.getClassLoader())


def _unsafe():
    cls = _JClass.forName("sun.misc.Unsafe")
    f = cls.getDeclaredField("theUnsafe")
    f.setAccessible(True)
    return f.get(None)


def _module_object(candidates):
    """Picks the registered object's (kind, class) from the bytecode
    candidates: the first value that is not a context, manager, RPC handle or
    doc provider."""
    for kind, cls in candidates:
        if any(x in cls for x in NOT_MODULE_OBJECTS):
            continue
        return kind, cls
    return None, None


def _registrations_for(scope):
    letter = SCOPE_LETTER[scope]
    hooks = set()
    for mid, by_scope in REGISTRATIONS["module_hooks"].items():
        for s, cls in by_scope.items():
            if letter in s:
                hooks.add(cls)
    core = CORE_REGISTRARS[scope]
    regs = [r for r in REGISTRATIONS["registrations"] if r["registrar"] in core and letter in r["jar_scopes"]]
    regs.sort(key=lambda r: core.index(r["registrar"]))
    regs += [r for r in REGISTRATIONS["registrations"] if r["registrar"] in hooks]
    return regs


def build_scope(scope, loader, unsafe):
    sm = ScriptManager("iglint-" + scope)
    log = []
    for r in _registrations_for(scope):
        kind, cname = _module_object(r["candidates"])
        entry = {"registrar": r["registrar"], "path": r["path"], "class": cname, "kind": kind}
        if cname is None:
            entry["error"] = "no module object found in bytecode"
            log.append(entry)
            continue
        try:
            cls = _JClass.forName(cname, False, loader)
            docs = PropertiesFileDocProvider()
            if kind == "class_literal":
                sm.addScriptModule(r["path"], cls, docs)
            else:
                sm.addScriptModule(r["path"], unsafe.allocateInstance(cls), docs)
            entry["ok"] = True
        except (Exception, _JThrowable) as e:
            entry["error"] = unicode(e)[:300]
        log.append(entry)
    live = []
    walk_live(sm.createLocalsMap()["system"], "system", live)
    hints = []
    walk_hints(sm.getHintsTree().children().get("system"), "system", hints)
    return {"scope": scope, "registrations": log, "live": live, "hints": hints}


def scopes_main():
    global PROBE_REQUIRED
    PROBE_REQUIRED = False  # never call client functions
    loader = _class_loader()
    unsafe = _unsafe()
    return {"scopes": [build_scope(s, loader, unsafe) for s in ("client", "designer")]}


result = scopes_main()
