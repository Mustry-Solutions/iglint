# Runs inside IA's Jython 2.7 with Ignition's common jars (see run-oracle.sh
# --library-routes). Measures how scripts reach the names of a project
# library module other than by naming them: the members of Ignition's
# library module and package objects, and what each route of a list does.
#
# usage: jython jython_library_routes_oracle.py LIST_FILE OUTPUT_FILE
#
# LIST_FILE holds one path per line, each a ".cases" file: routes, each
# introduced by a line "### <name> <expect> <target>" and followed by the
# body of a function `def g(n):` of the library module `b` (indented by
# four spaces). <expect> and <target> are for the linter's replay test
# (`library_routes` in iglint_linter): `value` when the route may reach
# any name of the module <target> (`m` or `pkg.m`), `literal` when it names
# one; the oracle ignores them.
#
# The project library is loaded by Ignition's own ScriptManager
# (`addProjectScriptModules`, as a gateway loads a project's library):
# modules `m` and `pkg.m`, both `import re` and `def f(): return 1`, and
# `b` with the case. Each case runs `b.g('re')` from a compiled function
# after `m.f` and `pkg.m.f` loaded both modules, in a fresh ScriptManager.
#
# OUTPUT_FILE gets, first, the members of the module object `m` and of the
# package object `pkg` that are not names of their namespace (every name
# from `dir` of the object, its type and `object`, and every public Java
# method, field and bean property of the class and its superclasses, that
# `getattr` finds), sorted:
#
#   ### members
#   module <name>
#   package <name>
#
# then one record per case, in input order:
#
#   ### <name>
#   ok <value>          (or: raised <exception type>)
#
# where <value> is the repr of None, bools, numbers and strings, `[...]`
# for lists and tuples of such values, and `<type name>` otherwise.
import re

from java.lang import Class, String, Throwable
from java.lang.reflect import Modifier
from java.util import ArrayList
from org.apache.commons.lang3.tuple import Pair
from org.python.core import Py, PyObject
from jarray import array
from com.inductiveautomation.ignition.common.script import ScriptManager

_SM = Class.forName('com.inductiveautomation.ignition.common.script.ScriptManager')
_ADD = _SM.getDeclaredMethod('addProjectScriptModules', [Class.forName('java.util.List')])
_ADD.setAccessible(True)
PACKAGE = 'com.inductiveautomation.ignition.common.script.'
SOURCE = 'import re\ndef f():\n    return 1\n'


def manager(modules):
    sm = ScriptManager('oracle')
    l = ArrayList()
    for path, code in modules:
        l.add(Pair.of(path, code))
    _ADD.invoke(sm, [l])
    return sm


def invoke(f):
    return f.invoke(array([Py.java2py(None), Py.java2py(None)], PyObject), array([], String))


def error_name(e):
    if isinstance(e, Throwable):
        cause = e
        while cause.getCause() is not None:
            cause = cause.getCause()
        text = cause.toString()
        m = re.search(r'(\w+Error|\w+Exception)\b', text)
        return m.group(1) if m else cause.getClass().getSimpleName()
    return type(e).__name__


def describe(v):
    if isinstance(v, (type(None), bool, int, long, float, str, unicode)):
        return repr(v)
    if isinstance(v, (list, tuple)):
        return '[' + ', '.join([describe(x) for x in v]) + ']'
    return '<%s>' % type(v).__name__


def run(body, b=None):
    modules = [('m', SOURCE), ('pkg.m', SOURCE)]
    if b is not None:
        modules.append(('b', b))
    sm = manager(modules)
    try:
        f = sm.compileFunction('def t(self, event):\n    m.f\n    pkg.m.f\n' + body)
        return 'ok ' + describe(invoke(f))
    except (Exception, Throwable), e:
        return 'raised ' + error_name(e)


def java_names(name):
    names = set()
    c = Class.forName(PACKAGE + name)
    while c is not None:
        for method in Class.getMethods(c):
            if not Modifier.isPublic(method.getModifiers()):
                continue
            n = method.getName()
            names.add(n)
            for prefix in ('get', 'is'):
                if n.startswith(prefix) and len(n) > len(prefix) and method.getParameterCount() == 0:
                    names.add(n[len(prefix)].lower() + n[len(prefix) + 1:])
        for field in Class.getFields(c):
            names.add(field.getName())
        c = Class.getSuperclass(c)
    return names


def members():
    sm = manager([('m', SOURCE), ('pkg.m', SOURCE)])
    get = sm.compileFunction('def t(self, event):\n    m.f\n    return [m, pkg]\n')
    module, package = invoke(get)
    out = []
    for label, obj, java, expr in [('module', module, 'ProjectScriptModule', 'm'),
                                   ('package', package, 'ScriptPackage', 'pkg')]:
        own = set(obj.__dict__.keys())
        candidates = set(dir(obj)) | set(dir(type(obj))) | set(dir(object)) | java_names(java)
        for n in sorted(candidates):
            if n in own:
                continue
            if run('    return getattr(%s, %r)\n' % (expr, n)).startswith('ok'):
                out.append('%s %s' % (label, n))
    return out


def split_cases(data):
    cases = []
    name = None
    lines = []
    for line in data.splitlines(True):
        if line.startswith('### '):
            if name is not None:
                cases.append((name, ''.join(lines)))
            name = line[4:].split()[0]
            lines = []
        elif name is not None:
            lines.append(line)
    if name is not None:
        cases.append((name, ''.join(lines)))
    return cases


def main(list_file, out_file):
    records = ['### members'] + members()
    for path in open(list_file).read().split():
        for name, body in split_cases(open(path).read()):
            b = 'def g(n):\n' + body
            records.append('### ' + name)
            records.append(run("    return b.g('re')\n", b))
    out = open(out_file, 'w')
    out.write('\n'.join(records) + '\n')
    out.close()


if __name__ == '__main__':
    import sys
    main(sys.argv[1], sys.argv[2])
