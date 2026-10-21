# Runs inside the gateway (Jython 2.7) through the stubs-gen WebDev endpoint.
# Dumps facts about the gateway-scope `system.*` scripting API:
#   * the live package tree (members, Java overloads, annotations, constants)
#   * the completion descriptors that the Designer uses for hints
#     (parameter names, types, optional flags, defaults, deprecations)
# Description texts are deliberately not dumped.
# The caller provides `system`; the result is stored in `result`.

import re

from java.lang import Class as JavaClass
from java.lang import Deprecated as JavaDeprecated
from java.lang import Thread, Throwable
from java.util import Date as JavaDate
from com.inductiveautomation.ignition.common.script import ImmutableScriptPackage
from com.inductiveautomation.ignition.gateway import IgnitionGateway

PROBE_REQUIRED = True
PACKAGE_INTERNALS = set(["clear", "dict", "getDict", "getName", "name", "setDict", "setName", "toString"])


def _accessible_field(cls, name):
    while cls is not None:
        try:
            f = cls.getDeclaredField(name)
            f.setAccessible(True)
            return f
        except Exception:
            cls = cls.getSuperclass()
    return None


def _class_name(c):
    # Jython hands out java.lang.Class values as Python types; call through
    # java.lang.Class so arrays print as `Foo[]`.
    if c is None:
        return None
    return str(JavaClass.getTypeName(c))


def _annotation(m, simple_name):
    for a in m.getAnnotations():
        if a.annotationType().getSimpleName() == simple_name:
            return a
    return None


def java_overloads(fn):
    """Java methods behind a reflected function (one per overload)."""
    out = []
    argslist = getattr(fn, "argslist", None)
    if argslist is None:
        return None
    field = None
    for ra in argslist:
        if ra is None:
            continue
        if field is None:
            field = _accessible_field(ra.getClass(), "method")
        m = field.get(ra)
        o = {
            "declaring_class": _class_name(m.getDeclaringClass()),
            "params": [_class_name(p) for p in m.getParameterTypes()],
            "varargs": bool(m.isVarArgs()),
            "returns": _class_name(m.getReturnType()),
            "static": bool(ra.isStatic),
            "java_deprecated": m.isAnnotationPresent(JavaDeprecated),
        }
        kw = _annotation(m, "KeywordArgs")
        if kw is not None:
            o["keyword_args"] = {
                "names": [str(n) for n in kw.names()],
                "types": [_class_name(t) for t in kw.types()],
            }
        je = _annotation(m, "JythonElement")
        if je is not None:
            o["doc_bundle_prefix"] = str(je.docBundlePrefix())
        sf = _annotation(m, "ScriptFunction")
        if sf is not None:
            o["doc_bundle_prefix"] = str(sf.docBundlePrefix())
        others = [str(a.annotationType().getName()) for a in m.getAnnotations()
                  if a.annotationType().getSimpleName() not in ("KeywordArgs", "JythonElement", "ScriptFunction", "Deprecated")]
        if others:
            o["other_annotations"] = sorted(others)
        out.append(o)
    out.sort(key=lambda o: (len(o["params"]), o["params"]))
    return out


_MISSING_HINT = re.compile(
    r"missing|required|must be (provided|specified)|can ?not be (blank|null|none|empty)|is null|not be empty",
    re.IGNORECASE)


def _dummy(type_name):
    """A harmless, non-empty value of the declared keyword type."""
    t = (type_name or "").split(".")[-1]
    if t.endswith("[]") or t in ("List", "PyList", "PySequence", "Collection", "Set"):
        return [u"x"]
    if t in ("Map", "PyDictionary", "PyStringMap", "Dict"):
        return {}
    if t in ("Integer", "int", "Long", "long", "Short", "Double", "double", "Float", "float", "Number"):
        return 1
    if t in ("Boolean", "boolean"):
        return False
    if t == "Date":
        return JavaDate()
    return u"x"


def _missing_param(message, names, supplied):
    """The single not-yet-supplied parameter that an error message reports as
    missing, or None."""
    if not message or not _MISSING_HINT.search(message):
        return None
    hits = [n for n in names if n not in supplied
            and re.search(r"(?<![A-Za-z0-9_])" + re.escape(n) + r"(?![A-Za-z0-9_])", message)]
    return hits[0] if len(hits) == 1 else None


def _call_with_timeout(fn, kwargs, seconds):
    box = {}

    def target():
        try:
            fn(**kwargs)
            box["outcome"] = ("ok", None)
        except (Exception, Throwable) as e:
            box["outcome"] = ("error", unicode(e))

    t = Thread(target)
    t.setDaemon(True)
    t.start()
    t.join(seconds * 1000)
    return box.get("outcome", ("timeout", None))


def probe_required(fn, names, types):
    """Discovers required keyword parameters: call the function with only the
    parameters found so far and read which parameter the error reports as
    missing. Runs only inside the throwaway gateway."""
    by_name = dict(zip(names, types))
    required = []
    kwargs = {}
    outcome = None
    for _ in range(len(names) + 1):
        status, message = _call_with_timeout(fn, kwargs, 10)
        name = _missing_param(message, names, kwargs) if status == "error" else None
        if name is None:
            outcome = status
            break
        required.append(name)
        kwargs[name] = _dummy(by_name[name])
    return {"required": required, "stopped_with": outcome}


def _python_signature(fn):
    import inspect
    try:
        target = fn
        if inspect.ismethod(target):
            target = target.im_func
        args, varargs, varkw, defaults = inspect.getargspec(target)
        return {"args": list(args), "varargs": varargs, "varkw": varkw,
                "defaults": [repr(d) for d in (defaults or ())]}
    except Exception as e:
        return {"error": str(e)}


def _constant(value):
    if isinstance(value, bool):
        return "bool", value
    if isinstance(value, (int, long)):
        return "int", int(value)
    if isinstance(value, float):
        return "float", value
    if isinstance(value, basestring):
        return "str", unicode(value)
    return None, None


def walk_live(pkg, path, out):
    d = pkg.getDict()
    for key in sorted(d.keys()):
        name = str(key)
        if name in PACKAGE_INTERNALS:
            continue
        value = d[key]
        full = path + "." + name
        tname = str(type(value))
        entry = {"path": full, "py_type": tname}
        if isinstance(value, ImmutableScriptPackage):
            entry["kind"] = "package"
            out.append(entry)
            walk_live(value, full, out)
            continue
        overloads = java_overloads(value) if hasattr(value, "argslist") else None
        if overloads is not None:
            entry["kind"] = "java_function"
            entry["overloads"] = overloads
            kw = [o["keyword_args"] for o in overloads if "keyword_args" in o]
            if kw and PROBE_REQUIRED:
                entry["required_probe"] = probe_required(value, kw[0]["names"], kw[0]["types"])
        elif callable(value) and (hasattr(value, "func_code") or hasattr(value, "im_func")):
            entry["kind"] = "python_function"
            entry["python_signature"] = _python_signature(value)
        else:
            ctype, cval = _constant(value)
            if ctype is not None:
                entry["kind"] = "constant"
                entry["value_type"] = ctype
                entry["value"] = cval
            elif callable(value):
                entry["kind"] = "callable_object"
            else:
                entry["kind"] = "object"
            try:
                entry["java_class"] = str(value.getClass().getName())
            except Exception:
                pass
        out.append(entry)


def _type_name(t):
    if t is None:
        return None
    try:
        return str(t.getName())
    except Exception:
        return str(t)


def _deprecation(dep):
    if dep is None:
        return None
    rep = dep.getReplacement()
    return {"replacement": None if rep is None else str(rep)}


def walk_hints(node, path, out):
    for a in node.attributes():
        out.append({
            "kind": "attribute",
            "path": path + "." + str(a.getName()),
            "type": _type_name(a.getType()),
            "deprecation": _deprecation(a.getDeprecation()),
        })
    for m in node.methods():
        params = []
        for p in m.getParameters():
            default = p.getDefault()
            params.append({
                "name": str(p.getName()),
                "type": _type_name(p.getType()),
                "optional": bool(p.getOptional()),
                "default": None if default is None else unicode(default),
                "kind": str(p.getKind()),
            })
        out.append({
            "kind": "method",
            "path": path + "." + str(m.getName()),
            "location": None if m.getLocation() is None else str(m.getLocation()),
            "descriptor_kind": str(m.getKind()),
            "params": params,
            "returns": _type_name(m.getReturnType()),
            "deprecation": _deprecation(m.getDeprecation()),
        })
    children = node.children()
    for key in sorted(children.keySet()):
        out.append({"kind": "package", "path": path + "." + str(key)})
        walk_hints(children.get(key), path + "." + str(key), out)


def main():
    gw = IgnitionGateway.get()
    live = []
    walk_live(system, "system", live)
    hints = []
    root = gw.getScriptManager().getHintsTree()
    walk_hints(root.children().get("system"), "system", hints)
    return {
        "ignition_version": str(gw.getVersion()),
        "scope": "gateway",
        "live": live,
        "hints": hints,
    }


if not globals().get("IGLINT_LIBRARY"):
    result = main()
