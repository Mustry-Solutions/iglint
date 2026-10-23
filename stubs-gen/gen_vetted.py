# Generates crates/iglint_python_semantic/src/generated/vetted_generated.rs,
# the machine-measured half of the vetted-code guard. Runs inside IA's
# Jython 2.7 (the inductiveautomation/ignition:8.3.3 image), because only
# the real interpreter and the image's classes can tell us, for every type
# a script can hold and every vetted Python module, which members and
# attributes lead to Java reflection, the interpreter or another module.
# See stubs-gen/run.sh for the command and docs/internal/fix-safety.md
# ("Machine-measured facts") for the argument.
#
# It emits these tables:
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
#   MODULE_ATTRS and JAVA_PACKAGE_CLASSES: every attribute of every vetted
#   module (and os, sys) that imports, and the public top-level classes of
#   the vetted Java packages (and of the packages of the vetted classes):
#   an import is known to succeed only when it names one of them (SCR102's
#   safe removal, and vetting of Java classes, need that evidence).
#
#   JAVA_FILE_MEMBERS and JAVA_FILE_CLASSES: the member names whose declared
#   type mentions a file handle (java.io.File, java.nio.file.Path, a file
#   channel or descriptor, a URL or URI, a zip file, Ignition's project and
#   resource collection managers and resources), found by a second fixed
#   point over the same seeds in which the handle types are stops too (and
#   are scanned, so that navigating from a handle, `getParentFile`, is a
#   member too); and the vetted Java classes that are handle types or have a
#   public constructor whose signature mentions one. vetted.rs gives every
#   such class a policy (a file reader whose path argument must be a literal
#   outside the workspace, a writer, or a class that only takes handles) and
#   treats any member name on any value as a possible file read. See
#   docs/internal/fix-safety.md ("File reads").
#
#   PERSPECTIVE_MEMBERS, PERSPECTIVE_PROPERTY_TYPES and
#   PERSPECTIVE_DELEGATE_MEMBERS: the members of the objects Perspective
#   hands to scripts (component, view, page and session SafetyWrappers),
#   with the kind of their declared type, so that the access analysis can
#   classify every member that may reach other views.
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

# File handle types: holding one lets a script read the bytes or metadata
# of a file, or of a project resource as the gateway loaded it. A member
# whose declared type mentions one is a file handle source (JAVA_FILE_MEMBERS).
FILE_HANDLE_TYPES = [
    'java.io.File',
    'java.io.FileDescriptor',
    'java.io.FileInputStream',
    'java.io.FileReader',
    'java.io.RandomAccessFile',
    'java.nio.file.Path',
    'java.nio.file.FileSystem',
    'java.nio.file.FileStore',
    'java.nio.file.DirectoryStream',
    'java.nio.file.spi.FileSystemProvider',
    'java.nio.file.attribute.BasicFileAttributes',
    'java.nio.channels.FileChannel',
    'java.nio.channels.AsynchronousFileChannel',
    'java.util.zip.ZipFile',
    'java.net.URL',
    'java.net.URI',
    'java.net.URLConnection',
    # Ignition 8.3: project and gateway resources as the gateway loaded
    # them from the files a fix edits.
    'com.inductiveautomation.ignition.gateway.project.ProjectManager',
    'com.inductiveautomation.ignition.common.resourcecollection.Resource',
    'com.inductiveautomation.ignition.common.resourcecollection.ResourceCollection',
    'com.inductiveautomation.ignition.common.resourcecollection.Snapshot',
    'com.inductiveautomation.ignition.gateway.resourcecollection.ResourceCollectionManager',
    'com.inductiveautomation.ignition.gateway.resourcecollection.ResourceCollectionFileTree',
    'com.inductiveautomation.ignition.gateway.resourcecollection.ResourceDataFile',
    'com.inductiveautomation.ignition.gateway.resourcecollection.StreamingResource',
]

SCANNED_HANDLE_TYPES = [
    'java.io.File',
    'java.io.FileInputStream',
    'java.io.FileReader',
    'java.net.URL',
    'java.net.URI',
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

# The Vision client's injected objects (JAVA_FILE_MEMBERS does not seed them).
VISION_INJECTED = [
    'com.inductiveautomation.factorypmi.application.script',
    'java.awt.event',
    'java.beans.PropertyChangeEvent',
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


def system_types(path, scopes=None):
    """The Java class names system.* returns and holds, in every scope or,
    with `scopes`, in variants available in one of them."""
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
    def wanted(v):
        return scopes is None or any(s in scopes for s in v.get('scopes', []))
    for f in data['functions']:
        for v in f['variants']:
            if not wanted(v):
                continue
            for o in v['overloads']:
                add(o.get('returns'))
    for o in data['objects']:
        for v in o['variants']:
            if wanted(v):
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
    def __init__(self, loader, handles=None):
        self.loader = loader
        self.stop = []
        for n in STOP_TYPES:
            c = self.load(n)
            if c is not None:
                self.stop.append(c)
        # With `handles` (FILE_HANDLE_TYPES), only members whose type
        # mentions a handle type are recorded, and expansion stops at the
        # handle types, except SCANNED_HANDLE_TYPES: vetted code holds those
        # without a recorded hop (a constructor call with a literal path
        # outside the workspace), so the members that navigate from them
        # (`getParentFile`, `toPath`, `resolve`) are handle sources too.
        self.handles = None
        self.handle_names = []
        if handles is not None:
            self.handles = []
            for n in handles:
                c = self.load(n)
                if c is None:
                    raise Exception('file handle type %s does not load' % n)
                self.handles.append(c)
                self.handle_names.append(n)
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
        if self.handles is not None and self.is_handle(c) \
                and Class.getName(c) not in SCANNED_HANDLE_TYPES:
            return
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

    def needs_handle(self, m):
        # With `handles`: a method one of whose parameters is a handle type
        # that vetted code cannot hold without a recorded hop (not one of
        # SCANNED_HANDLE_TYPES) is not a first hop: its caller holds such a
        # handle already (`LastModification.update(Resource, String)`).
        if self.handles is None:
            return False
        for p in m.getGenericParameterTypes():
            acc = []
            self.mentioned(p, acc)
            for t in acc:
                if self.is_handle(t) and Class.getName(t) not in SCANNED_HANDLE_TYPES:
                    return True
        return False

    def is_handle(self, c):
        for r in self.handles or []:
            if Class.isAssignableFrom(r, c):
                return True
        return False

    def follow(self, types):
        # Queues the plain types; whether any is a stop type (with
        # `handles`: a handle type; the handle type itself is scanned).
        hit = False
        for t in types:
            if self.handles is not None:
                if self.is_handle(t):
                    hit = True
                    if Class.getName(t) in SCANNED_HANDLE_TYPES:
                        self.queue(t)
                elif not self.is_stop(t):
                    self.queue(t)
            elif self.is_stop(t):
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
                if self.follow(returns) and not self.needs_handle(m):
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
        'loader': loader,
        'classes': class_names(jars),
        'build': build,
        'seeds': len(set(seeds)),
        'scanned': len(closure.scanned),
        'hidden': len(closure.hidden),
        'missing': sorted(closure.missing),
    }
    return sorted(names.items()), stats


def java_file_members(loader, java_classes, packages, all_classes, system_json):
    """JAVA_FILE_MEMBERS and JAVA_FILE_CLASSES (see the header)."""
    closure = Closure(loader, FILE_HANDLE_TYPES)
    # Gateway scope only: Vision clients run only for Vision resources,
    # which are unknown code or (client event scripts) counted as possible
    # file reads by the linter, so Vision's objects are not seeds here.
    _, returned = system_types(system_json, ['gateway'])
    injected_classes = [c for c in INJECTED_CLASSES if c not in VISION_INJECTED]
    injected_packages = [p for p in INJECTED_PACKAGES if p not in VISION_INJECTED]
    seeds = SEED_CLASSES + HELD_WITHOUT_A_HOP + list(java_classes) + returned + injected_classes
    vetted = list(java_classes)
    for n in all_classes:
        pkg = n.rsplit('.', 1)[0] if '.' in n else ''
        top = n.split('$')[0]
        in_vetted = pkg in packages and top.rsplit('.', 1)[-1] not in packages[pkg]
        injected = any(pkg == p or pkg.startswith(p + '.') for p in injected_packages)
        if (in_vetted or injected) and public_member(closure, n):
            seeds.append(n)
            if in_vetted and '$' not in n:
                vetted.append(n)
    for n in sorted(set(seeds)):
        closure.seed(n)
    for n in SCANNED_HANDLE_TYPES:
        closure.seed(n)
    closure.run()
    classes = []
    for n in sorted(set(vetted)):
        c = closure.load(n)
        if c is None:
            continue
        if closure.is_handle(c):
            classes.append(n)
            continue
        for k in Class.getConstructors(c):
            acc = []
            for t in k.getGenericParameterTypes():
                closure.mentioned(t, acc)
            if any(closure.is_handle(t) for t in acc):
                classes.append(n)
                break
    return sorted(closure.names.items()), classes, len(closure.scanned)


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


# --- Perspective script objects ------------------------------------------------
#
# The objects Perspective hands to scripts (`self`, `self.view`, `self.page`,
# `self.session`, the `session` and `page` parameters of session and page
# events) are PyDelegateObjects over a SafetyWrapper: Python sees the
# property trees named by PropertyType (`props`, `custom`, ...), custom
# methods and the script-callable methods of the component's model delegate
# (components only), then the public Java members of the SafetyWrapper. The
# linter's access analysis (script_facts/access.rs) classifies every member
# measured here: a member of a page or session that hands out views, pages
# or components reaches other views. Kinds, from the declared type:
#
#   host     a Page or Session
#   element  a View, ViewModel, Component, ComponentModel or PerspectiveElement
#            (or a supertype of one other than Object), or a collection of them
#   tree     a PropertyTree
#   object   Object or PyObject (classified by hand in access.rs)
#   value    anything else (strings, numbers, void, data)

PERSPECTIVE_WRAPPERS = [
    ('component', 'com.inductiveautomation.perspective.gateway.script.ComponentModelScriptWrapper$SafetyWrapper'),
    ('page', 'com.inductiveautomation.perspective.gateway.script.PageScriptWrapper$SafetyWrapper'),
    ('session', 'com.inductiveautomation.perspective.gateway.script.SessionScriptWrapper$SafetyWrapper'),
    ('view', 'com.inductiveautomation.perspective.gateway.script.ViewModelScriptWrapper$SafetyWrapper'),
]
PERSPECTIVE_PROPERTY_TYPE = 'com.inductiveautomation.perspective.common.api.PropertyType'
PERSPECTIVE_DELEGATE = 'com.inductiveautomation.perspective.gateway.api.ComponentModelDelegate'
PERSPECTIVE_HOSTS = [
    'com.inductiveautomation.perspective.gateway.api.Page',
    'com.inductiveautomation.perspective.gateway.api.Session',
]
PERSPECTIVE_ELEMENTS = [
    'com.inductiveautomation.perspective.gateway.api.View',
    'com.inductiveautomation.perspective.gateway.model.ViewModel',
    'com.inductiveautomation.perspective.gateway.api.Component',
    'com.inductiveautomation.perspective.gateway.model.ComponentModel',
    'com.inductiveautomation.perspective.gateway.api.PerspectiveElement',
]
PERSPECTIVE_TREES = ['com.inductiveautomation.perspective.gateway.property.PropertyTree']
KIND_ORDER = ['value', 'object', 'tree', 'host', 'element']


def perspective_kind(closure, types, hosts, elements, trees):
    obj = closure.load('java.lang.Object')
    pyobj = closure.load('org.python.core.PyObject')
    kinds = ['value']
    for t in types:
        if any(Class.isAssignableFrom(h, t) for h in hosts):
            kinds.append('host')
        elif t != obj and any(Class.isAssignableFrom(e, t) or Class.isAssignableFrom(t, e)
                              for e in elements + hosts):
            kinds.append('element')
        elif any(Class.isAssignableFrom(r, t) for r in trees):
            kinds.append('tree')
        elif t == obj or (pyobj is not None and Class.isAssignableFrom(pyobj, t)):
            kinds.append('object')
    return max(kinds, key=KIND_ORDER.index)


def perspective_members(loader, all_classes):
    closure = Closure(loader)
    hosts = [closure.load(n) for n in PERSPECTIVE_HOSTS]
    elements = [closure.load(n) for n in PERSPECTIVE_ELEMENTS]
    trees = [closure.load(n) for n in PERSPECTIVE_TREES]
    if None in hosts + elements + trees:
        raise Exception('a Perspective type does not load')

    def members(c, skip):
        out = {}
        for m in Class.getMethods(c):
            if Class.getName(m.getDeclaringClass()) in skip:
                continue
            types = closure.types_of(m.getReturnType(), m.getGenericReturnType())
            kind = perspective_kind(closure, types, hosts, elements, trees)
            names = [m.getName()]
            b = bean(m.getName(), len(m.getParameterTypes()))
            if b:
                names.append(b)
            for n in names:
                old = out.get(n, 'value')
                out[n] = max([old, kind], key=KIND_ORDER.index)
        return out

    wrappers = []
    for short, name in PERSPECTIVE_WRAPPERS:
        c = closure.load(name)
        if c is None:
            raise Exception('%s does not load' % name)
        for n, k in sorted(members(c, ['java.lang.Object']).items()):
            wrappers.append((short, n, k))
    ptype = closure.load(PERSPECTIVE_PROPERTY_TYPE)
    property_types = sorted(str(e) for e in ptype.getEnumConstants())
    base = closure.load(PERSPECTIVE_DELEGATE)
    delegates = {}
    for n in all_classes:
        c = closure.load(n)
        if c is None or c == base:
            continue
        try:
            if not Class.isAssignableFrom(base, c) or not Modifier.isPublic(Class.getModifiers(c)):
                continue
            found = members(c, ['java.lang.Object', PERSPECTIVE_DELEGATE])
        except (Exception, JavaThrowable):
            continue
        for m, k in found.items():
            delegates[m] = max([delegates.get(m, 'value'), k], key=KIND_ORDER.index)
    return wrappers, property_types, sorted(delegates.items())


# --- what an import finds -------------------------------------------------------

def java_package_classes(java_classes, packages, all_classes, loader):
    """The public top-level classes of every vetted Java package and of the
    packages of the vetted Java classes: an import of a class outside
    these lists may fail."""
    wanted = set(packages) | set(c.rsplit('.', 1)[0] for c in java_classes)
    closure = Closure(loader)
    out = {}
    for n in all_classes:
        if '$' in n or '.' not in n:
            continue
        pkg, simple = n.rsplit('.', 1)
        if pkg not in wanted:
            continue
        c = closure.load(n)
        if c is None or not Modifier.isPublic(Class.getModifiers(c)):
            continue
        out.setdefault(pkg, []).append(simple)
    return [(p, sorted(out.get(p, []))) for p in sorted(wanted)]


def module_attrs(modules):
    """Every attribute of every vetted module (and `os`, `sys`) that
    imports in the image: `from m import n` of anything else fails."""
    out = []
    for name in sorted(set(modules) | set(['os', 'sys'])):
        try:
            mod = import_module(name)
        except Exception:
            continue
        out.append((name, sorted(set(dir(mod)))))
    return out


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
    wrappers, property_types, delegates = perspective_members(stats['loader'], stats['classes'])
    package_classes = java_package_classes(java_classes, packages, stats['classes'],
                                           stats['loader'])
    all_attrs = module_attrs(modules)
    file_members, file_classes, file_scanned = java_file_members(
        stats['loader'], java_classes, packages, stats['classes'], system_json)

    signature = (['mod:%s' % m for m in modules]
                 + ['java:%s' % c for c in sorted(java_classes)]
                 + ['pkg:%s' % p for p in sorted(packages)]
                 + ['stubs:%s' % stats['build']]
                 + ['perspective:%s=%s' % (short, name) for short, name in PERSPECTIVE_WRAPPERS]
                 + ['file-handle:%s' % n for n in FILE_HANDLE_TYPES])

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
    lines.append('/// these, on any value or in a string literal in a sink position, is a')
    lines.append('/// route out of plain data and is treated as introspective. Each entry')
    lines.append('/// names one member it was found on. Sorted for binary search.')
    lines.append('pub(crate) const JAVA_REFLECTIVE_MEMBERS: &[&str] = &[')
    for name, where in members:
        lines.append('    %s, // %s' % (rust_str(name), where))
    lines.append('];')
    lines.append('')
    lines.append('/// Member names (Java method, Jython bean-property and field')
    lines.append('/// spellings) whose declared type mentions a file handle (a file,')
    lines.append('/// path, channel, descriptor, URL or URI, or a project resource as')
    lines.append('/// the gateway loaded it), over every type a script can hold (%d'
                 % file_scanned)
    lines.append('/// types scanned). Naming any of these on any value may read the')
    lines.append('/// bytes or metadata of a workspace file. Sorted.')
    lines.append('pub(crate) const JAVA_FILE_MEMBERS: &[&str] = &[')
    for name, where in file_members:
        lines.append('    %s, // %s' % (rust_str(name), where))
    lines.append('];')
    lines.append('')
    lines.append('/// The vetted Java classes that are file handle types or have a public')
    lines.append('/// constructor whose signature mentions one. Sorted.')
    lines.append('#[cfg_attr(not(test), allow(dead_code))]')
    lines.append('pub(crate) const JAVA_FILE_CLASSES: &[&str] = &[')
    lines.append(rust_str_list(file_classes, 4))
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
    lines.append('/// Every attribute (`dir()`) of every vetted module, `os` and `sys`')
    lines.append('/// that imports in the image: an import of a module or name that is')
    lines.append('/// not here may fail. Sorted.')
    lines.append('pub(crate) const MODULE_ATTRS: &[(&str, &[&str])] = &[')
    for name, names in all_attrs:
        lines.append('    (%s, &[' % rust_str(name))
        if names:
            lines.append(rust_str_list(names, 8))
        lines.append('    ]),')
    lines.append('];')
    lines.append('')
    lines.append('/// The public top-level classes of every vetted Java package and of the')
    lines.append('/// packages of the vetted Java classes. Sorted.')
    lines.append('pub(crate) const JAVA_PACKAGE_CLASSES: &[(&str, &[&str])] = &[')
    for pkg, classes in package_classes:
        lines.append('    (%s, &[' % rust_str(pkg))
        if classes:
            lines.append(rust_str_list(classes, 8))
        lines.append('    ]),')
    lines.append('];')
    lines.append('')
    lines.append('/// The members of the Perspective script objects (component, page,')
    lines.append('/// session and view SafetyWrappers), in the Java method and bean')
    lines.append('/// spellings, with the kind of their declared type: `host` (a page or')
    lines.append('/// session), `element` (views, components), `tree` (a property tree),')
    lines.append('/// `object` (`Object` or `PyObject`) or `value`. Sorted.')
    lines.append('pub(crate) const PERSPECTIVE_MEMBERS: &[(&str, &str, &str)] = &[')
    for w, n, k in wrappers:
        lines.append('    (%s, %s, %s),' % (rust_str(w), rust_str(n), rust_str(k)))
    lines.append('];')
    lines.append('')
    lines.append('/// The property tree names every Perspective script object resolves')
    lines.append('/// before its members (`PropertyType`).')
    lines.append('pub(crate) const PERSPECTIVE_PROPERTY_TYPES: &[&str] = &[')
    lines.append(rust_str_list(property_types, 4))
    lines.append('];')
    lines.append('')
    lines.append('/// The script-callable members of every component model delegate')
    lines.append('/// (component-specific methods such as `refreshData`), with the kind')
    lines.append('/// of their declared type. Sorted.')
    lines.append('pub(crate) const PERSPECTIVE_DELEGATE_MEMBERS: &[(&str, &str)] = &[')
    for n, k in delegates:
        lines.append('    (%s, %s),' % (rust_str(n), rust_str(k)))
    lines.append('];')
    lines.append('')
    open(out_rs, 'wb').write(('\n'.join(lines) + '\n').encode('utf-8'))


if __name__ == '__main__':
    main()
