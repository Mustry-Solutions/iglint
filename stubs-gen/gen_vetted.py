# Generates crates/iglint_python_semantic/src/generated/vetted_generated.rs,
# the machine-measured half of the vetted-code guard. Runs inside IA's
# Jython 2.7 (the inductiveautomation/ignition:8.3.3 image), because only
# the real interpreter and the image's classes can tell us, for every type
# a script can hold and every vetted Python module, which members and
# attributes lead to Java reflection, the interpreter or another module.
# See stubs-gen/run.sh for the command and docs/internal/fix-safety.md
# ("Machine-measured facts") for the argument.
#
# It emits two tables:
#
#   JAVA_REFLECTIVE_MEMBERS: the member names (Java method names and the
#   Jython bean-property and field spellings) whose declared type mentions
#   a stop type (reflection: java.lang.Class / ClassLoader, Module, Package,
#   java.lang.reflect.*, java.lang.invoke.*, Jython's reflected-argument
#   and bean handles; the interpreter: PySystemState, frames, code, modules,
#   PythonInterpreter, Ignition's ScriptManager, javax.script engines),
#   found by a transitive fixed point over the public members of every type
#   a script can hold (see SCAN SEEDS below). Naming any of these on any
#   value is the first hop out of plain data, so dynamic.rs treats it as
#   introspective and fails closed.
#
#   MODULE_VETTED_ATTRS: for every vetted Python module, the attributes that
#   are themselves vetted (a function, a class, or a constant of an
#   immutable vetted type). Every other attribute (a module, a
#   PySystemState such as `re.sys`, a Java package or class, or anything
#   else) is unknown code.
#
# usage: jython gen_vetted.py VETTED_RS SYSTEM_JSON OUT_RS

import re
import sys
import types

import jarray
from java.io import File, FileOutputStream
from java.lang import Class, ClassLoader
from java.lang import Throwable as JavaThrowable
from java.lang.reflect import (GenericArrayType, Modifier, ParameterizedType,
                               TypeVariable, WildcardType)
from java.net import URI, URL, URLClassLoader
from java.nio.file import Files, FileSystems
from java.util.jar import JarFile
from java.util.zip import ZipFile
from org.python.expose import ExposedType


# --- reading the vetted inputs from vetted.rs -------------------------------

def read_inputs(path):
    text = open(path, 'rb').read().decode('utf-8')
    modules = block_names(text, 'VETTED_MODULES')
    # os, sys and system are handled by vetted.rs directly, but their
    # module objects are vetted too; os.path is a submodule we measure.
    modules = sorted(set(modules + ['os.path']))
    java_classes = []
    for pkg, cls in java_entries(text, 'VETTED_JAVA_CLASSES'):
        java_classes.append('%s.%s' % (pkg, cls))
    packages = java_packages(text, 'VETTED_JAVA_PACKAGES')
    return modules, java_classes, packages


def block_names(text, const):
    # The tuple keys `("name", ...)` of a `const CONST: ... = &[ ... ];`.
    m = re.search(r'const %s[^=]*=\s*&\[(.*?)\n\];' % const, text, re.S)
    body = m.group(1)
    return re.findall(r'\(\s*"([^"]+)"', body)


def java_entries(text, const):
    m = re.search(r'const %s[^=]*=\s*&\[(.*?)\n\];' % const, text, re.S)
    body = m.group(1)
    out = []
    # Each top-level entry is ("package", &[ "Class", ... ]).
    for pkg, classes in re.findall(r'"([^"]+)",\s*&\[(.*?)\]', body, re.S):
        for cls in re.findall(r'"([^"]+)"', classes):
            out.append((pkg, cls))
    return out


def java_packages(text, const):
    # ("package", &[denied classes]) entries: package -> denied names.
    m = re.search(r'const %s[^=]*=\s*&\[(.*?)\n\];' % const, text, re.S)
    out = {}
    for pkg, denied in re.findall(r'"([^"]+)",\s*&\[(.*?)\]', m.group(1), re.S):
        out[pkg] = re.findall(r'"([^"]+)"', denied)
    return out


# --- the classes of the image ------------------------------------------------

IGNITION = '/usr/local/bin/ignition'
# Module jars are unpacked inside the container, never on the host.
UNPACK = '/tmp/iglint-vetted-jars'


def image_jars():
    """Every jar of the image: lib/core/* (common, gateway, client,
    designer, launch) and every jar inside every .modl of user-lib/modules,
    in a stable order."""
    out = []
    core = File(IGNITION + '/lib/core')
    for sub in sorted(core.listFiles(), key=lambda f: f.getName()):
        if sub.isDirectory():
            for f in sorted(sub.listFiles(), key=lambda f: f.getName()):
                if f.getName().endswith('.jar'):
                    out.append(f)
    File(UNPACK).mkdirs()
    modules = File(IGNITION + '/user-lib/modules')
    for f in sorted(modules.listFiles(), key=lambda f: f.getName()):
        if not f.getName().endswith('.modl'):
            continue
        z = ZipFile(f)
        entries = sorted([e for e in iter_enum(z.entries()) if e.getName().endswith('.jar')],
                         key=lambda e: e.getName())
        for e in entries:
            dst = File(UNPACK, '%s__%s' % (f.getName().replace(' ', '_'),
                                           e.getName().replace('/', '_')))
            copy(z.getInputStream(e), FileOutputStream(dst))
            out.append(dst)
        z.close()
    return out


def iter_enum(e):
    while e.hasMoreElements():
        yield e.nextElement()


def copy(ins, outs):
    buf = jarray.zeros(65536, 'b')
    while True:
        n = ins.read(buf)
        if n < 0:
            break
        outs.write(buf, 0, n)
    outs.close()
    ins.close()


def class_names(jars):
    """The binary names of every class in the JDK's runtime image and in
    `jars`, sorted."""
    names = set()
    fs = FileSystems.getFileSystem(URI.create('jrt:/'))
    it = Files.walk(fs.getPath('/modules')).iterator()
    while it.hasNext():
        p = str(it.next())
        if p.endswith('.class') and not p.endswith('-info.class'):
            names.add('.'.join(p.split('/')[3:])[:-6])
    for jar in jars:
        try:
            jf = JarFile(jar)
        except Exception:
            continue
        for e in iter_enum(jf.entries()):
            n = e.getName()
            if n.endswith('.class') and not n.endswith('-info.class') and not n.startswith('META-INF'):
                names.add(n[:-6].replace('/', '.'))
        jf.close()
    return sorted(names)


# --- the Java closure ---------------------------------------------------------

# Stop types. Holding one of these lets a script name classes, methods,
# fields, loaders, modules, frames or the interpreter itself, from which it
# reaches `org.python.core.Py`, `PySystemState`, every module and `eval`. A
# member whose declared type mentions one (as its type, an array element, a
# type argument or a bound) is a first hop; the closure does not continue
# past them.
STOP_TYPES = [
    # Java reflection.
    'java.lang.Class',
    'java.lang.ClassLoader',
    'java.lang.Module',
    'java.lang.ModuleLayer',
    'java.lang.Package',
    'java.lang.reflect.AccessibleObject',
    'java.lang.reflect.AnnotatedElement',
    'java.lang.reflect.Constructor',
    'java.lang.reflect.Executable',
    'java.lang.reflect.Field',
    'java.lang.reflect.GenericDeclaration',
    'java.lang.reflect.Member',
    'java.lang.reflect.Method',
    'java.lang.reflect.Parameter',
    'java.lang.reflect.Proxy',
    'java.lang.reflect.Type',
    'java.lang.invoke.MethodHandle',
    'java.lang.invoke.MethodHandles$Lookup',
    'java.lang.invoke.MethodType',
    'java.lang.invoke.VarHandle',
    # Jython's handles on Java reflection (a reflected function's
    # `argslist` holds the java.lang.reflect.Method objects).
    'org.python.core.ReflectedArgs',
    'org.python.core.PyReflectedField',
    'org.python.core.PyBeanProperty',
    'org.python.core.PyBeanEvent',
    'org.python.core.PyBeanEventProperty',
    # The interpreter: the `sys` module, frames, code, modules and their
    # namespaces, interpreters and script engines.
    'org.python.core.PySystemState',
    'org.python.core.PyFrame',
    'org.python.core.PyTraceback',
    'org.python.core.PyCode',
    'org.python.core.ThreadState',
    'org.python.core.PyModule',
    'org.python.core.PyStringMap',
    'org.python.util.PythonInterpreter',
    'javax.script.ScriptEngine',
    'javax.script.ScriptEngineManager',
    'javax.script.ScriptContext',
    'javax.script.Bindings',
    'com.inductiveautomation.ignition.common.script.ScriptManager',
]

# Stop types that a script also holds without naming any member, so their
# own members are first hops too (and are scanned): a Java class named in
# source (`ArrayList`, `type(x)`) is a class proxy exposing java.lang.Class's
# methods (verified: `String.getMethods()` works), and a Java method named
# without calling it (`String.valueOf`, `system.tag.readBlocking`) is a
# reflectedfunction exposing its Java fields (`argslist`, verified).
HELD_WITHOUT_A_HOP = [
    'java.lang.Class',
    'org.python.core.PyReflectedFunction',
    'org.python.core.PyReflectedConstructor',
]

# --- SCAN SEEDS: the types a plain value can have ----------------------------
#
# 1. java.lang.Object, Throwable and Thread (every Java value is an Object;
#    caught Java exceptions are Throwables; Thread.currentThread() is a
#    common static entry), and HELD_WITHOUT_A_HOP;
# 2. every vetted Java class (VETTED_JAVA_CLASSES) and every public class of
#    every vetted package (VETTED_JAVA_PACKAGES, minus the denied classes);
# 3. the Java types system.* functions return and system.* objects have,
#    in every scope, from crates/iglint_stubs/data/system.json (the stubs
#    generator reads them from a live gateway); the Python-normalised names
#    (`list`, `Dataset`, `any`, ...) stand for every Java type
#    stubs-gen/normalize.py maps to them (PYTHON_NAMES below), and the
#    system.bacnet.* classes for every public class of their packages;
# 4. the objects Ignition injects into scripts: INJECTED_PACKAGES (every
#    public class) and INJECTED_CLASSES.
#
# From the seeds, the closure follows every public member (methods, bean
# getters and fields, inherited ones included, static ones too): the
# declared return or field type, every type its generic signature mentions
# (type arguments, wildcard and type-variable bounds, array elements), the
# declared exception types (an `except` clause catches them), the argument
# types of the abstract methods of interface parameters (a Python callable
# passed for one is called with them) and the public member classes. A
# type typed `Object` stays `Object`. Jython types (classes declaring
# @ExposedType) are not scanned: Python code sees only their exposed
# members, not their Java ones (verified: `[].getClass` and
# `str.getProxyType` raise AttributeError); their introspection attributes
# are dynamic.rs's INTROSPECTION_ATTRS.

SEED_CLASSES = ['java.lang.Object', 'java.lang.Throwable', 'java.lang.Thread']

INJECTED_PACKAGES = [
    # Perspective: self, the view, page and session wrappers, property
    # trees, event objects (dot-reference maps), message payloads.
    'com.inductiveautomation.perspective.gateway.script',
    # Jython maps handed to scripts.
    'com.inductiveautomation.ignition.common.script.abc',
    # Qualified values and quality codes (tag reads, change scripts,
    # transforms).
    'com.inductiveautomation.ignition.common.model.values',
    # Tag event scripts.
    'com.inductiveautomation.ignition.gateway.tags.scripting',
    # Vision: component wrappers and AWT event objects.
    'com.inductiveautomation.factorypmi.application.script',
    'java.awt.event',
    # system.bacnet.* classes (constructed, enumerated, primitive types).
    'com.serotonin.bacnet4j.type',
]

INJECTED_CLASSES = [
    # Datasets.
    'com.inductiveautomation.ignition.common.Dataset',
    'com.inductiveautomation.ignition.common.BasicDataset',
    'com.inductiveautomation.ignition.common.script.builtin.DatasetUtilities$PyDataSet',
    # Loggers (system.util.getLogger).
    'com.inductiveautomation.ignition.common.util.LoggerEx',
    'org.slf4j.Logger',
    # Alarm pipeline and alarm event scripts.
    'com.inductiveautomation.ignition.common.alarming.AlarmEvent',
    'com.inductiveautomation.ignition.common.alarming.BasicAlarmEvent',
    'com.inductiveautomation.ignition.common.alarming.PyAlarmEvent',
    # Gateway and client tag change events.
    'com.inductiveautomation.ignition.common.tags.model.event.TagChangeEvent',
    'com.inductiveautomation.ignition.common.sqltags.model.event.TagChangeEvent',
    # Vision property change events.
    'java.beans.PropertyChangeEvent',
    # WebDev: request['servletRequest'], ['servletResponse'], ['context'],
    # the HTTP session.
    'jakarta.servlet.http.HttpServletRequest',
    'jakarta.servlet.http.HttpServletResponse',
    'jakarta.servlet.http.HttpSession',
    'com.inductiveautomation.ignition.gateway.model.GatewayContext',
    # SFC chart scopes.
    'com.inductiveautomation.sfc.api.PyChartScope',
    'com.inductiveautomation.sfc.api.PyFunctionalChartScope',
]

# The Java types stubs-gen/normalize.py (JAVA_TYPES) writes as each
# Python-level name, plus the Jython types for the names it does not map.
PYTHON_NAMES = {
    'str': ['java.lang.String', 'java.lang.CharSequence'],
    'int': ['java.lang.Integer'],
    'long': ['java.lang.Long'],
    'float': ['java.lang.Double', 'java.lang.Float'],
    'number': ['java.lang.Number'],
    'bool': ['java.lang.Boolean'],
    'Date': ['java.util.Date'],
    'Dataset': ['com.inductiveautomation.ignition.common.Dataset'],
    'PyDataset': ['com.inductiveautomation.ignition.common.script.builtin.DatasetUtilities$PyDataSet'],
    'list': ['java.util.List', 'java.util.Collection', 'org.python.core.PySequence',
             'org.python.core.PyList'],
    'dict': ['java.util.Map', 'org.python.core.PyDictionary'],
    'any': ['org.python.core.PyObject', 'java.lang.Object'],
    'type': ['org.python.core.PyType'],
    'callable': ['org.python.core.PyFunction'],
    'tuple': ['org.python.core.PyTuple'],
    'set': ['java.util.Set', 'org.python.core.PySet'],
    'bytes': [],
    'None': [],
}

# Named Java reflection API entry points (method and bean spellings), added
# unconditionally as defence in depth. None of these double as an ordinary
# Perspective member, so flagging them everywhere costs no real precision.
EXPLICIT_REFLECTION_API = [
    'forName', 'loadClass', 'defineClass', 'newInstance',
    'getClassLoader', 'classLoader',
    'getSystemClassLoader', 'systemClassLoader',
    'getContextClassLoader', 'contextClassLoader',
    'getMethods', 'methods', 'getDeclaredMethods', 'declaredMethods',
    'getMethod', 'getDeclaredMethod', 'declaredMethod',
    'getFields', 'fields', 'getDeclaredFields', 'declaredFields',
    'getField', 'getDeclaredField', 'declaredField',
    'getConstructors', 'constructors',
    'getDeclaredConstructors', 'declaredConstructors',
    'getConstructor', 'getDeclaredConstructor', 'declaredConstructor',
    'getSuperclass', 'superclass',
    'getEnclosingClass', 'enclosingClass',
    'getDeclaringClass', 'declaringClass',
    'getComponentType', 'componentType',
]


def system_types(path):
    """The Java class names system.* returns and holds, all scopes."""
    import json
    data = json.load(open(path))
    out = set()

    def add(t):
        if not t:
            return
        t = t.replace('[]', '')
        if t in PYTHON_NAMES:
            out.update(PYTHON_NAMES[t])
        elif '.' in t:
            out.add(t)
    for f in data['functions']:
        for v in f['variants']:
            for o in v['overloads']:
                add(o.get('returns'))
    for o in data['objects']:
        for v in o['variants']:
            add(v.get('type'))
    return data.get('build', ''), sorted(out)


def bean(name, nparams):
    # The JavaBeans property name Jython exposes for a no-argument getter.
    if nparams != 0:
        return None
    for pfx in ('get', 'is'):
        if name.startswith(pfx) and len(name) > len(pfx):
            rest = name[len(pfx):]
            if len(rest) >= 2 and rest[0].isupper() and rest[1].isupper():
                return rest
            return rest[0].lower() + rest[1:]
    return None


class Closure(object):
    def __init__(self, loader):
        self.loader = loader
        self.stop = []
        for n in STOP_TYPES:
            c = self.load(n)
            if c is not None:
                self.stop.append(c)
        self.names = {}      # member name -> 'declaring.Class.member' (smallest)
        self.queued = set()
        self.work = []
        self.scanned = set()
        self.hidden = set()  # Jython types: Java members not visible
        self.missing = set()  # seeds or members that do not load

    def load(self, name):
        try:
            return Class.forName(name, False, self.loader)
        except (Exception, JavaThrowable):
            return None

    def is_stop(self, c):
        for r in self.stop:
            if Class.isAssignableFrom(r, c):
                return True
        return False

    def mentioned(self, t, acc, depth=0):
        # The classes a (generic) type mentions.
        if t is None or depth > 16:
            return
        if isinstance(t, (Class, type)):
            # Jython hands java.lang.Class values out as Python types.
            comp = Class.getComponentType(t)
            while comp is not None:
                t = comp
                comp = Class.getComponentType(t)
            if not Class.isPrimitive(t):
                acc.append(t)
        elif isinstance(t, ParameterizedType):
            self.mentioned(t.getRawType(), acc, depth + 1)
            for a in t.getActualTypeArguments():
                self.mentioned(a, acc, depth + 1)
        elif isinstance(t, WildcardType):
            for b in list(t.getUpperBounds()) + list(t.getLowerBounds()):
                self.mentioned(b, acc, depth + 1)
        elif isinstance(t, GenericArrayType):
            self.mentioned(t.getGenericComponentType(), acc, depth + 1)
        elif isinstance(t, TypeVariable):
            for b in t.getBounds():
                self.mentioned(b, acc, depth + 1)

    def queue(self, c):
        try:
            if Class.isAnonymousClass(c) or Class.isLocalClass(c) or Class.isSynthetic(c):
                return
        except (Exception, JavaThrowable):
            pass
        n = Class.getName(c)
        if n not in self.queued:
            self.queued.add(n)
            self.work.append(c)

    def seed(self, name):
        c = self.load(name)
        if c is None:
            self.missing.add(name)
        else:
            self.queue(c)

    def record(self, name, where):
        if name not in self.names or where < self.names[name]:
            self.names[name] = where

    def types_of(self, typ, generic):
        acc = []
        self.mentioned(typ, acc)
        self.mentioned(generic, acc)
        return acc

    def follow(self, types):
        # Queues the plain types; whether any is a stop type.
        hit = False
        for t in types:
            if self.is_stop(t):
                hit = True
            else:
                self.queue(t)
        return hit

    def run(self):
        held = set(HELD_WITHOUT_A_HOP)
        while self.work:
            c = self.work.pop()
            cn = Class.getName(c)
            if self.is_stop(c) and cn not in held:
                continue
            if Class.getDeclaredAnnotation(c, ExposedType) is not None:
                self.hidden.add(cn)
                continue
            try:
                methods = Class.getMethods(c)
                fields = Class.getFields(c)
                nested = Class.getClasses(c)
            except (Exception, JavaThrowable):
                self.missing.add(cn)
                continue
            self.scanned.add(cn)
            for k in nested:
                self.queue(k)
            for m in methods:
                name = m.getName()
                try:
                    returns = self.types_of(m.getReturnType(), m.getGenericReturnType())
                    throws = list(m.getExceptionTypes())
                    params = list(m.getParameterTypes())
                except (Exception, JavaThrowable):
                    self.missing.add('%s.%s' % (cn, name))
                    continue
                if self.follow(returns):
                    where = '%s.%s' % (cn, name)
                    self.record(name, where)
                    b = bean(name, len(params))
                    if b:
                        self.record(b, where)
                self.follow(throws)
                for p in params:
                    if Class.isInterface(p) and not self.is_stop(p):
                        self.callback(p)
            for f in fields:
                try:
                    types = self.types_of(f.getType(), f.getGenericType())
                except (Exception, JavaThrowable):
                    self.missing.add('%s.%s' % (cn, f.getName()))
                    continue
                if self.follow(types):
                    self.record(f.getName(), '%s.%s' % (cn, f.getName()))

    def callback(self, iface):
        # A Python callable passed for an interface parameter is called with
        # the arguments of the interface's abstract methods.
        try:
            for am in Class.getMethods(iface):
                if Modifier.isAbstract(am.getModifiers()):
                    acc = []
                    for pt in am.getGenericParameterTypes():
                        self.mentioned(pt, acc)
                    for t in acc:
                        if not self.is_stop(t):
                            self.queue(t)
        except (Exception, JavaThrowable):
            pass


def public_member(closure, name):
    # A public top-level class or a public member class of one.
    for seg in name.split('.')[-1].split('$')[1:]:
        if not seg or seg[0].isdigit():
            return False
    c = closure.load(name)
    try:
        while c is not None:
            if not Modifier.isPublic(Class.getModifiers(c)):
                return False
            c = Class.getDeclaringClass(c)
    except (Exception, JavaThrowable):
        return False
    return True


def java_reflective_members(java_classes, packages, system_json):
    jars = image_jars()
    urls = jarray.array([f.toURI().toURL() for f in jars], URL)
    loader = URLClassLoader(urls, ClassLoader.getSystemClassLoader())
    closure = Closure(loader)
    build, returned = system_types(system_json)
    seeds = SEED_CLASSES + HELD_WITHOUT_A_HOP + list(java_classes) + returned + INJECTED_CLASSES
    for n in class_names(jars):
        pkg = n.rsplit('.', 1)[0] if '.' in n else ''
        top = n.split('$')[0]
        vetted = pkg in packages and top.rsplit('.', 1)[-1] not in packages[pkg]
        injected = any(pkg == p or pkg.startswith(p + '.') for p in INJECTED_PACKAGES)
        if (vetted or injected) and public_member(closure, n):
            seeds.append(n)
    for n in sorted(set(seeds)):
        closure.seed(n)
    closure.run()
    names = dict(closure.names)
    for n in ['class', 'getClass', '__class__'] + EXPLICIT_REFLECTION_API:
        names.setdefault(n, 'explicit')
    stats = {
        'build': build,
        'seeds': len(set(seeds)),
        'scanned': len(closure.scanned),
        'hidden': len(closure.hidden),
        'missing': sorted(closure.missing),
    }
    return sorted(names.items()), stats


# --- the Python module attribute measurement -------------------------------

try:
    from org.python.core import PySystemState
except Exception:
    PySystemState = None

IMMUTABLE = (int, long, float, complex, bool, str, unicode, type(None))


def constant_kind(val, depth=0):
    if isinstance(val, IMMUTABLE):
        return True
    if depth < 3 and isinstance(val, (tuple, frozenset)):
        return all(constant_kind(v, depth + 1) for v in val)
    return False


def import_module(name):
    mod = __import__(name)
    for part in name.split('.')[1:]:
        mod = getattr(mod, part)
    return mod


def module_vetted_attrs(modules):
    out = []
    for name in modules:
        try:
            mod = import_module(name)
        except Exception:
            out.append((name, []))
            continue
        vetted = []
        for attr in dir(mod):
            if attr.startswith('_') and attr not in ('__name__', '__doc__', '__file__'):
                continue
            try:
                val = getattr(mod, attr)
            except Exception:
                continue
            if isinstance(val, types.ModuleType):
                continue
            if PySystemState is not None and isinstance(val, PySystemState):
                continue
            kind = classify(val)
            if kind in ('function', 'class', 'constant'):
                vetted.append(attr)
        out.append((name, sorted(set(vetted))))
    return out


def classify(val):
    # A wrapped Java class (`base64.binascii` is one) or Java package
    # carries reflection and is unknown, not a vetted class.
    if isinstance(val, Class):
        return 'java'
    tname = str(type(val))
    if 'javapackage' in tname or 'javaclass' in tname or 'JavaClass' in tname:
        return 'java'
    if isinstance(val, (types.ClassType, type)):
        return 'class'
    # Jython exposes a Java method of a module (`math.sqrt`) as a
    # `reflectedfunction`, not a Python function; any plain callable that
    # is not a class is a function here (the module's functions were all
    # verified pure; attrgetter / methodcaller / Formatter stay on
    # vetted.rs's per-module denylist).
    if callable(val):
        return 'function'
    if constant_kind(val):
        return 'constant'
    # Anything else (a module-like object, an instance, a PySystemState
    # that slipped through) is unknown code.
    return 'other'


# --- emitting the generated Rust -------------------------------------------

def rust_str_list(items, indent):
    pad = ' ' * indent
    return '\n'.join('%s%s,' % (pad, rust_str(i)) for i in items)


def rust_str(s):
    return '"%s"' % s.replace('\\', '\\\\').replace('"', '\\"')


def main():
    vetted_rs, system_json, out_rs = sys.argv[1], sys.argv[2], sys.argv[3]
    modules, java_classes, packages = read_inputs(vetted_rs)
    members, stats = java_reflective_members(java_classes, packages, system_json)
    attrs = module_vetted_attrs(modules)

    signature = (['mod:%s' % m for m in modules]
                 + ['java:%s' % c for c in sorted(java_classes)]
                 + ['pkg:%s' % p for p in sorted(packages)]
                 + ['stubs:%s' % stats['build']])

    lines = []
    lines.append('// @generated by stubs-gen/gen_vetted.py; do not edit by hand.')
    lines.append('// Run `stubs-gen/run.sh --vetted` (needs the Ignition 8.3.3 image)')
    lines.append('// to regenerate. The test `generated_tables_match_vetted_inputs`')
    lines.append('// fails when the vetted lists or the stubs change without')
    lines.append('// regenerating.')
    lines.append('//')
    lines.append('// Java closure: %d seed types, %d types scanned, %d Jython types'
                 % (stats['seeds'], stats['scanned'], stats['hidden']))
    lines.append('// skipped (only their exposed members are visible), %d seeds or'
                 % len(stats['missing']))
    lines.append('// members that do not load%s' % (':' if stats['missing'] else '.'))
    for m in stats['missing']:
        lines.append('//   %s' % m)
    lines.append('')
    lines.append('/// The vetted module names, Java classes and packages and the stubs')
    lines.append('/// build this file was generated from. The regeneration test compares')
    lines.append('/// it with the live inputs.')
    lines.append('#[cfg_attr(not(test), allow(dead_code))]')
    lines.append('pub(crate) const GENERATED_FROM: &[&str] = &[')
    lines.append(rust_str_list(signature, 4))
    lines.append('];')
    lines.append('')
    lines.append('/// Member names (Java method, Jython bean-property and field')
    lines.append('/// spellings) whose declared type mentions a reflection or')
    lines.append('/// interpreter type, over every type a script can hold. Naming any of')
    lines.append('/// these, on any value or in any string literal, is a route out of')
    lines.append('/// plain data and is treated as introspective. Each entry names one')
    lines.append('/// member it was found on. Sorted for binary search.')
    lines.append('pub(crate) const JAVA_REFLECTIVE_MEMBERS: &[&str] = &[')
    for name, where in members:
        lines.append('    %s, // %s' % (rust_str(name), where))
    lines.append('];')
    lines.append('')
    lines.append('/// For every vetted Python module, the attributes that are')
    lines.append('/// themselves vetted (a function, class or constant of an immutable')
    lines.append('/// vetted type). Every other attribute is unknown code. Sorted.')
    lines.append('pub(crate) const MODULE_VETTED_ATTRS: &[(&str, &[&str])] = &[')
    for name, vetted in attrs:
        lines.append('    (%s, &[' % rust_str(name))
        if vetted:
            lines.append(rust_str_list(vetted, 8))
        lines.append('    ]),')
    lines.append('];')
    lines.append('')
    open(out_rs, 'wb').write(('\n'.join(lines) + '\n').encode('utf-8'))


if __name__ == '__main__':
    main()
