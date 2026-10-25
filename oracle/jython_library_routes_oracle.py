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
# one, `plain` when it calls or reads a name of the module, `unknown` when
# it runs code that is not vetted or reads a file; the oracle ignores them.
# In a file whose name starts with "runscript", the body of each route is
# an Ignition expression instead (literal arguments only), evaluated by
# Ignition's own expression parser and function factory: `runScript` is
# the gateway's `ScriptFunction` (common.jar), run with the library below;
# `perspective:` before the expression runs Perspective's
# `PerspectiveRunScriptFunction` instead, for an element whose `str()` is
# 'element' (its view, page and session: 'view', 'page', 'session').
#
# In a file whose name starts with "chains", each case brings its own
# library: sections introduced by a line "== <dotted module>" hold the
# code of each module, and the route is `b.g('re')` of the section `b`,
# or the expression of a section "== runScript", evaluated like the
# expressions above with that library (paths through the imports of
# library modules; the header's words after the name are for the
# linter's replay test, `every_chain_through_library_imports_counts`).
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
# then, the same way, the members of the other objects a dotted name can
# reach from a library module: a function, an old-style class, its unbound
# and bound method and an instance, a new-style class, its method and an
# instance, and a generator object (`<kind> <name>`; the name the test
# module defines, `meth`, left out):
#
#   function <name>
#   class <name>
#   ...
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
from com.inductiveautomation.ignition.common.expressions import (
    DefaultFunctionFactory, ExpressionParseContext, FunctionFactory)
from com.inductiveautomation.ignition.common.expressions.functions import (
    AbstractPollingFunction, ScriptFunction)
from com.inductiveautomation.ignition.common.expressions.parsing import ELParser
from com.inductiveautomation.ignition.common.model import CommonContext
from java.io import StringReader
from java.lang.reflect import InvocationHandler, Proxy

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
    return java_class_names(PACKAGE + name)


def java_class_names(class_name):
    names = set()
    c = Class.forName(class_name)
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


OBJECTS = ('def f():\n    pass\n'
           'class C:\n    def meth(self):\n        pass\n'
           'class N(object):\n    def meth(self):\n        pass\n'
           'def g():\n    yield 1\n')
OBJECT_KINDS = [
    ('function', 'objs.f', 'org.python.core.PyFunction'),
    ('class', 'objs.C', 'org.python.core.PyClass'),
    ('unbound-method', 'objs.C.meth', 'org.python.core.PyMethod'),
    ('bound-method', 'objs.C().meth', 'org.python.core.PyMethod'),
    ('instance', 'objs.C()', 'org.python.core.PyInstance'),
    ('type', 'objs.N', 'org.python.core.PyType'),
    ('type-method', 'objs.N.meth', 'org.python.core.PyMethod'),
    ('type-instance', 'objs.N()', 'org.python.core.PyObjectDerived'),
    ('generator', 'objs.g()', 'org.python.core.PyGenerator'),
]


def object_members():
    out = []
    sm = manager([('objs', OBJECTS)])
    for label, expr, java in OBJECT_KINDS:
        obj = invoke(sm.compileFunction('def t(self, event):\n    return %s\n' % expr))
        candidates = set(dir(obj)) | set(dir(type(obj))) | set(dir(object)) | java_class_names(java)
        candidates.discard('meth')
        probe = ('def t(self, event):\n'
                 '    o = %s\n'
                 '    found = []\n'
                 '    for n in %r:\n'
                 '        try:\n'
                 '            getattr(o, n)\n'
                 '            found.append(n)\n'
                 '        except:\n'
                 '            pass\n'
                 '    return found\n') % (expr, sorted(candidates))
        for n in sorted(invoke(sm.compileFunction(probe))):
            out.append('%s %s' % (label, n))
    return out


_PERSPECTIVE = []


def perspective():
    """Perspective's runScript function and element types, loaded from the
    module file and the gateway's jars."""
    if not _PERSPECTIVE:
        import glob
        import os
        import sys
        import zipfile
        modl = '/usr/local/bin/ignition/user-lib/modules/Perspective-module.modl'
        out = '/tmp/iglint-perspective'
        if not os.path.isdir(out):
            os.makedirs(out)
        z = zipfile.ZipFile(modl)
        for n in z.namelist():
            if n.startswith('perspective-gateway-') or n.startswith('perspective-common-'):
                path = os.path.join(out, n)
                f = open(path, 'wb')
                f.write(z.read(n))
                f.close()
                sys.path.append(path)
        for jar in sorted(glob.glob('/usr/local/bin/ignition/lib/core/gateway/*.jar')):
            sys.path.append(jar)
        from com.inductiveautomation.perspective.gateway.expression import PerspectiveRunScriptFunction
        from com.inductiveautomation.perspective.gateway.api import PerspectiveElement, View, Page, Session
        _PERSPECTIVE.extend([PerspectiveRunScriptFunction, PerspectiveElement, View, Page, Session])
    return _PERSPECTIVE


def proxy(iface, handler):
    return Proxy.newProxyInstance(iface.getClassLoader(), [iface], handler)


class Handler(InvocationHandler):
    """A Java object that answers only what runScript asks: the script
    manager of a context, the view, page and session of an element."""

    def __init__(self, label, sm=None):
        self.label = label
        self.sm = sm

    def invoke(self, p, method, args):
        n = method.getName()
        if n == 'getScriptManager':
            return self.sm
        if n == 'toString':
            return self.label
        if n == 'hashCode':
            return 1
        if n == 'equals':
            return False
        kinds = dict(zip(['getView', 'getPage', 'getSession'], perspective()[2:]))
        if n in kinds:
            return proxy(kinds[n], Handler(n[3:].lower()))
        raise Exception('unexpected call ' + n)


def run_expression(text, modules=None):
    """Evaluates the Ignition expression `text` with the gateway's function
    factory; `runScript` runs with the library of the routes (or with
    `modules`)."""
    sm = manager(modules or [('m', SOURCE), ('pkg.m', SOURCE)])
    use_perspective = text.startswith('perspective:')
    if use_perspective:
        text = text[len('perspective:'):]
    context = proxy(CommonContext, Handler('context', sm))
    field = AbstractPollingFunction.getDeclaredField('context')
    field.setAccessible(True)
    default = DefaultFunctionFactory.getSharedInstance()

    class Factory(FunctionFactory):
        def getFunction(self, name):
            f = default.getFunction(name)
            if isinstance(f, ScriptFunction):
                if use_perspective:
                    run_script, element = perspective()[:2]
                    f = run_script(proxy(element, Handler('element')))
                field.set(f, context)
            return f

    class Parse(ExpressionParseContext):
        def __init__(self):
            self.factory = Factory()

        def createBoundExpression(self, path):
            raise Exception('references are not supported')

        def getFunctionFactory(self):
            return self.factory

    try:
        e = ELParser(StringReader(text), Parse()).parse().value
        return 'ok ' + describe(e.execute().getValue())
    except (Exception, Throwable), e:
        return 'raised ' + error_name(e)


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


def split_modules(body):
    """The sections of a chain case: `(dotted module, code)` pairs, each
    introduced by a line "== <module>"."""
    sections = []
    for line in body.splitlines(True):
        if line.startswith('== '):
            sections.append((line[3:].strip(), []))
        elif sections:
            sections[-1][1].append(line)
    return [(name, ''.join(lines)) for name, lines in sections]


def run_chain(body):
    """A chain case: the library modules of its sections, loaded by the
    ScriptManager; the route is `b.g('re')` (module `b`), or the
    expression of a section "== runScript"."""
    sections = split_modules(body)
    modules = [(name, code) for name, code in sections if name != 'runScript']
    expression = [code for name, code in sections if name == 'runScript']
    if expression:
        return run_expression(expression[0].strip(), modules)
    sm = manager(modules)
    try:
        f = sm.compileFunction("def t(self, event):\n    return b.g('re')\n")
        return 'ok ' + describe(invoke(f))
    except (Exception, Throwable), e:
        return 'raised ' + error_name(e)


def main(list_file, out_file):
    records = ['### members'] + members() + object_members()
    for path in open(list_file).read().split():
        expressions = path.split('/')[-1].startswith('runscript')
        chains = path.split('/')[-1].startswith('chains')
        for name, body in split_cases(open(path).read()):
            records.append('### ' + name)
            if chains:
                records.append(run_chain(body))
            elif expressions:
                records.append(run_expression(body.strip()))
            else:
                b = 'def g(n):\n' + body
                records.append(run("    return b.g('re')\n", b))
    out = open(out_file, 'w')
    out.write('\n'.join(records) + '\n')
    out.close()


if __name__ == '__main__':
    import sys
    main(sys.argv[1], sys.argv[2])
