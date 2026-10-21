#!/usr/bin/env python3
"""Builds the Jython payload for the client/designer scope dump: the
registrations found by registrations.py, then scripting.py as a library,
then scopes.py.

Usage: payload.py <registrations.json> <introspect dir> <out.py>
"""

import sys

regs, introspect, out = sys.argv[1:4]
with open(regs) as f:
    data = f.read()
parts = [
    "IGLINT_LIBRARY = True\n",
    "import json as _json\n",
    "REGISTRATIONS = _json.loads(%r)\n" % data,
]
for name in ("scripting.py", "scopes.py"):
    with open("%s/%s" % (introspect, name)) as f:
        parts.append(f.read())
        parts.append("\n")
with open(out, "w") as f:
    f.write("".join(parts))
