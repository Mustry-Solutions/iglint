# Runs inside the gateway (Jython 2.7) through the stubs-gen WebDev endpoint,
# after `CONTEXTS = {...}` (stubs-gen/expression_contexts.py: the functions
# Perspective's own function factory adds to the gateway's). Loads each
# function class from its module's class loader, constructs it with null
# arguments (only initArgs is called, which checks the argument count) and
# probes the accepted counts like introspect/expression.py.
# The result is stored in `result`.

import jarray
from java.lang import Class, Object, Throwable
from com.inductiveautomation.ignition.common.expressions import ConstantExpression, Expression
from com.inductiveautomation.ignition.gateway import IgnitionGateway

PROBE_MAX = 40
SAMPLES = [1, u"a", False]


def _accepts(fn, count):
    for sample in SAMPLES:
        args = jarray.array([ConstantExpression(sample) for _ in range(count)], Expression)
        try:
            fn.initArgs(args)
            return True
        except (Exception, Throwable):
            pass
    return False


def main():
    gw = IgnitionGateway.get()
    loaders = dict((m.getInfo().getId(), m.getClassloader()) for m in gw.getModuleManager().getModules())
    gateway = gw.getExpressionFunctionFactory()
    out = {}
    for context, data in sorted(CONTEXTS.items()):
        loader = loaders[data["module"]]
        functions = []
        for f in data["functions"]:
            cls = Class.forName(f["java_class"], True, loader)
            con = cls.getDeclaredConstructors()[0]
            con.setAccessible(True)
            nulls = jarray.array([None] * len(con.getParameterTypes()), Object)
            fn = con.newInstance(nulls)
            replaced = gateway.getFunction(f["name"])
            functions.append({
                "name": f["name"],
                "categories": [f["category"]],
                "java_class": f["java_class"],
                "replaces": None if replaced is None else str(Class.getName(replaced.getClass())),
                "arg_hint": unicode(fn.getArgDocString()),
                "accepted_counts": [n for n in range(0, PROBE_MAX + 1) if _accepts(fn, n)],
                "probe_max": PROBE_MAX,
            })
        out[context] = {"factory": data["factory"], "functions": functions}
    return out


result = main()
