# Runs inside the gateway (Jython 2.7) through the stubs-gen WebDev endpoint.
# Dumps the expression functions registered in the gateway function factory
# (which is what Perspective bindings and tag expressions use):
#   * name as registered (lookups are case-insensitive)
#   * category paths
#   * accepted argument counts, measured by calling initArgs() on a fresh copy
#     of the function with 0..PROBE_MAX constant arguments. initArgs is where
#     Ignition rejects a wrong argument count when an expression is compiled.
#   * the argument hint the Designer shows (parameter names only)
# The result is stored in `result`.

import jarray
from java.lang import Class, Throwable
from com.inductiveautomation.ignition.common.expressions import ConstantExpression, Expression
from com.inductiveautomation.ignition.gateway import IgnitionGateway

PROBE_MAX = 40
SAMPLES = [1, u"a"]


def _accepts(fn, count):
    errors = []
    for sample in SAMPLES:
        args = jarray.array([ConstantExpression(sample) for _ in range(count)], Expression)
        try:
            fn.copy().initArgs(args)
            return True, None
        except (Exception, Throwable) as e:
            errors.append(unicode(e))
    return False, errors[0]


def main():
    factory = IgnitionGateway.get().getExpressionFunctionFactory()
    categories = {}
    display = {}
    for cat in factory.getCategories():
        for fname in factory.getFunctionsInCategory(cat):
            categories.setdefault(str(fname).lower(), []).append(unicode(cat))
            display[str(fname).lower()] = str(fname)
    out = []
    for key in sorted(str(n) for n in factory.getFunctionNames()):
        fn = factory.getFunction(key)
        entry = {
            "key": key,
            "name": display.get(key.lower(), key),
            "categories": sorted(categories.get(key.lower(), [])),
            "java_class": str(Class.getName(fn.getClass())),
        }
        try:
            entry["arg_hint"] = unicode(fn.getArgDocString())
        except (Exception, Throwable) as e:
            entry["arg_hint"] = None
        accepted = []
        for n in range(0, PROBE_MAX + 1):
            ok, _ = _accepts(fn, n)
            if ok:
                accepted.append(n)
        entry["accepted_counts"] = accepted
        entry["probe_max"] = PROBE_MAX
        out.append(entry)
    return {
        "ignition_version": str(IgnitionGateway.get().getVersion()),
        "scope": "gateway",
        "functions": out,
    }


result = main()
