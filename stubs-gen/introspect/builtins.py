# Runs inside the gateway (Jython 2.7) through the stubs-gen WebDev endpoint.
# Dumps the names in Jython's __builtin__ module with a coarse kind.
# The result is stored in `result`.

import __builtin__
import sys


def _kind(value):
    if isinstance(value, type):
        if issubclass(value, BaseException):
            return "exception"
        return "type"
    if callable(value):
        return "function"
    return "constant"


result = {
    "jython_version": sys.version.split()[0],
    "builtins": [{"name": n, "kind": _kind(getattr(__builtin__, n))} for n in sorted(dir(__builtin__))],
}
