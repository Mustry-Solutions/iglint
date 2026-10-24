# Runs inside IA's Jython 2.7 with Ignition's common jars (see run-oracle.sh
# --implicit). Runs small sources the way Ignition runs scripts and records
# which names the namespace of the code holds afterwards that the source
# does not bind by itself: the names an import statement (or any other
# statement) creates implicitly.
#
# usage: jython jython_implicit_names_oracle.py LIST_FILE OUTPUT_FILE
#
# LIST_FILE holds one path per line, each a ".cases" file: many small
# sources, each introduced by a line "### <name>".
#
# Every case runs in four contexts:
#
#   library   the source is the project library module `pkg.m`, loaded by
#             Ignition's own ScriptManager (`addProjectScriptModules`, as a
#             gateway loads a project's library) next to `pkg.other` (`Y = 1`);
#             the namespace is the module's `__dict__` after the first access
#             (`pkg.m` from a compiled function).
#   function  the source is the body of `def runAction(self, event):`,
#             compiled by `ScriptManager.compileFunction` (as Perspective
#             compiles scripts) and invoked once; the namespace is the
#             function's globals.
#   toplevel  the source is top-level code after a function body in the
#             same compiled script (`def runAction(self, event):\n    pass\n`
#             followed by the source at column 0, as an event file with
#             helper code after the function); the namespace is the
#             function's globals.
#   plain     the source is a plain Jython module imported from a file, for
#             comparison (Ignition never loads project code this way); its
#             `__file__` and `__name__` are shown as `<file>` and `<name>`.
#
# OUTPUT_FILE gets one record per case, in input order:
#
#   ### <name>
#   <context>: <entries>
#
# with one line per context. <entries> lists, sorted: every dunder name of
# the namespace, with the repr of its value (`__builtins__` and
# `__warningregistry__` without one); every other name that is not an
# identifier of the source and not one of Ignition's injected names
# (`system`, `fpmi`, `sys`, `pkg`), marked `+name`; and every `r_*` name the source binds, with its value (the
# sources record what they observed in such names). A context whose code
# raised adds `!<exception type>`; a source that does not compile there is
# recorded as `compile-error`.
import os
import re
import shutil
import sys
import tempfile

from java.lang import Class, String, Throwable
from java.util import ArrayList
from org.apache.commons.lang3.tuple import Pair
from org.python.core import Py, PyObject
from jarray import array
from com.inductiveautomation.ignition.common.script import ScriptManager

INJECTED = set(['system', 'fpmi', 'sys', 'pkg'])
WORD = re.compile(r'[A-Za-z_][A-Za-z0-9_]*')

_SM = Class.forName('com.inductiveautomation.ignition.common.script.ScriptManager')
_ADD = _SM.getDeclaredMethod('addProjectScriptModules', [Class.forName('java.util.List')])
_ADD.setAccessible(True)
_FUNC = Class.forName(
    'com.inductiveautomation.ignition.common.script.ScriptManager$ScriptFunctionImpl'
).getDeclaredField('func')
_FUNC.setAccessible(True)


def split_cases(data):
    cases = []
    name = None
    lines = []
    for line in data.splitlines(True):
        if line.startswith('### '):
            if name is not None:
                cases.append((name, ''.join(lines)))
            name = line[4:].rstrip('\r\n')
            lines = []
        elif name is not None:
            lines.append(line)
    if name is not None:
        cases.append((name, ''.join(lines)))
    return cases


def manager(modules):
    sm = ScriptManager('oracle')
    l = ArrayList()
    for path, code in modules:
        l.add(Pair.of(path, code))
    _ADD.invoke(sm, [l])
    return sm


def invoke(f, *args):
    # The overload with keywords: Jython does not pick the array overload
    # reliably (it may pass the array as one argument), so every function
    # compiled here takes exactly two arguments.
    return f.invoke(array([Py.java2py(a) for a in args], PyObject), array([], String))


def error_name(e):
    if isinstance(e, Throwable):
        cause = e
        while cause.getCause() is not None:
            cause = cause.getCause()
        text = cause.toString()
        m = re.search(r'(\w+Error|\w+Exception|\w+Warning)\b', text)
        return m.group(1) if m else cause.getClass().getSimpleName()
    return type(e).__name__


def describe(namespace, source, error):
    words = set(WORD.findall(source))
    out = []
    for key in sorted(namespace.keys()):
        key = str(key)
        if key.startswith('__') and key.endswith('__'):
            if key in ('__builtins__', '__warningregistry__'):
                out.append(key)
            else:
                out.append('%s=%r' % (key, namespace[key]))
        elif key.startswith('r_'):
            out.append('%s=%r' % (key, namespace[key]))
        elif key not in words and key not in INJECTED:
            out.append('+' + key)
    if error is not None:
        out.append('!' + error)
    return ' '.join(out)


def run_library(source):
    sm = manager([('pkg.m', source), ('pkg.other', 'Y = 1\n')])
    get = sm.compileFunction('def get(self, event):\n    return pkg.m\n')
    error = None
    module = None
    try:
        module = invoke(get, None, None)
    except (Exception, Throwable), e:
        error = error_name(e)
    if module is None:
        # The module did not load: read what its top level left behind.
        modules = sm.getModules()
        try:
            module = modules['pkg'].__dict__.get('m')
        except (Exception, Throwable):
            module = None
        if module is None:
            if error is not None and 'SyntaxError' in error:
                return 'compile-error'
            return '!' + (error or 'no module')
    return describe(module.__dict__, source, error)


def indent(source):
    return ''.join('    ' + l if l.strip() else l for l in source.splitlines(True))


def run_function(source, toplevel):
    sm = manager([('pkg.other', 'Y = 1\n')])
    if toplevel:
        text = 'def runAction(self, event):\n    pass\n' + source
    else:
        text = 'def runAction(self, event):\n' + (indent(source) or '    pass\n')
    try:
        f = sm.compileFunction(text)
    except (Exception, Throwable), e:
        name = error_name(e)
        if 'SyntaxError' in name:
            return 'compile-error'
        return '!' + name
    func = _FUNC.get(f)
    error = None
    try:
        invoke(f, None, None)
    except (Exception, Throwable), e:
        error = error_name(e)
    return describe(func.func_globals, source, error)


_counter = [0]


def run_plain(source, workdir):
    _counter[0] += 1
    name = 'implicit_case_%d' % _counter[0]
    f = open(os.path.join(workdir, name + '.py'), 'w')
    f.write(source.encode('utf-8'))
    f.close()
    # ScriptManager installs its own system state: import through it.
    state = Py.getSystemState()
    if workdir not in state.path:
        state.path.insert(0, workdir)
    error = None
    try:
        __import__(name)
    except SyntaxError:
        return 'compile-error'
    except (Exception, Throwable), e:
        error = error_name(e)
    module = state.modules.get(name)
    if module is None:
        return '!' + (error or 'no module')
    # The file name is the only difference between cases.
    ns = dict(module.__dict__)
    for key in ('__file__', '__name__'):
        if key in ns:
            ns[key] = '<%s>' % key[2:-2]
    return describe(ns, source, error)


def main():
    list_file, out_file = sys.argv[1], sys.argv[2]
    workdir = tempfile.mkdtemp()
    out = []
    for path in open(list_file).read().split():
        data = open(path).read().decode('utf-8')
        for name, source in split_cases(data):
            out.append('### %s\n' % name)
            out.append('library: %s\n' % run_library(source))
            out.append('function: %s\n' % run_function(source, False))
            out.append('toplevel: %s\n' % run_function(source, True))
            out.append('plain: %s\n' % run_plain(source, workdir))
    shutil.rmtree(workdir)
    f = open(out_file, 'w')
    f.write(''.join(out).encode('utf-8'))
    f.close()


main()
