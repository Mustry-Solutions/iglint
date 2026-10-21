# Runs inside the gateway (Jython 2.7) through the stubs-gen WebDev endpoint.
# Dumps every component type registered in the Perspective component registry:
# id, palette data, deprecation, the props JSON schema, the default props and
# component events. Schemas are dumped verbatim here; normalize.py strips all
# descriptive text before anything is committed.
# The result is stored in `result`.

import re

from java.lang import Class
from com.inductiveautomation.ignition.gateway import IgnitionGateway


def _schema_text(schema):
    if schema is None:
        return None
    text = unicode(schema)
    # JsonSchema.toString() renders as `"#" : {...}`.
    idx = text.find(u"{")
    return text[idx:] if idx >= 0 else None


def _event(e):
    out = {"java_class": str(Class.getName(e.getClass()))}
    for attr in ("name", "getName"):
        if hasattr(e, attr):
            v = getattr(e, attr)
            out["name"] = unicode(v() if callable(v) else v)
            break
    for attr in ("schema", "getSchema"):
        if hasattr(e, attr):
            v = getattr(e, attr)
            out["schema"] = _schema_text(v() if callable(v) else v)
            break
    return out


def main():
    gw = IgnitionGateway.get()
    module = gw.getModuleManager().getModule("com.inductiveautomation.perspective")
    loader = module.getClassloader()
    pc_class = loader.loadClass("com.inductiveautomation.perspective.gateway.api.PerspectiveContext")
    get = [m for m in pc_class.getMethods() if m.getName() == "get"][0]
    context = get.invoke(None, gw)
    registry = context.getComponentRegistry().get()
    components = []
    for cid in sorted(str(k) for k in registry.keySet()):
        d = registry.get(cid)
        components.append({
            "id": cid,
            "name": unicode(d.name()),
            "palette_category": None if d.paletteCategory() is None else unicode(d.paletteCategory()),
            "module_id": unicode(d.moduleId()),
            "deprecated": bool(d.deprecated()),
            "schema": _schema_text(d.schema()),
            "default_properties": None if d.defaultProperties() is None else unicode(d.defaultProperties()),
            "child_position_schema": _schema_text(d.childPositionSchema()),
            "events": [_event(e) for e in d.events()],
            "extension_functions": [
                {"name": unicode(f.getName()), "signature": unicode(f.getSignature())}
                for f in d.extensionFunctions()
            ],
        })
    # Shared schemas referenced as urn:ignition-schema:schemas/<name>, plus the
    # component meta schema and the view props schema.
    names = set(["meta-schema.json", "view-props-schema.json"])
    for c in components:
        for text in [c["schema"], c["child_position_schema"]] + [e.get("schema") for e in c["events"]]:
            for m in re.finditer(r"urn:ignition-schema:schemas/([A-Za-z0-9_.-]+)", text or u""):
                names.add(m.group(1))
    shared = {}
    pending = sorted(names)
    while pending:
        name = pending.pop()
        if name in shared:
            continue
        text = _resource(loader, "schemas/" + name)
        shared[name] = text
        for m in re.finditer(r"urn:ignition-schema:schemas/([A-Za-z0-9_.-]+)", text or u""):
            if m.group(1) not in shared:
                pending.append(m.group(1))
    return {"ignition_version": str(gw.getVersion()), "components": components, "shared_schemas": shared}


def _resource(loader, name):
    stream = loader.getResourceAsStream(name)
    if stream is None:
        return None
    try:
        from java.io import ByteArrayOutputStream
        import jarray
        out = ByteArrayOutputStream()
        buf = jarray.zeros(8192, "b")
        while True:
            n = stream.read(buf)
            if n < 0:
                break
            out.write(buf, 0, n)
        return out.toString("UTF-8")
    finally:
        stream.close()


result = main()
