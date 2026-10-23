# Generates crates/iglint_python_semantic/src/generated/vetted_generated.rs,
# the machine-measured half of the vetted-code guard. Runs inside IA's
# Jython 2.7 (the inductiveautomation/ignition:8.3.3 image), because only
# the real interpreter can tell us, for every vetted Java class and every
# vetted Python module, which members and attributes lead to Java
# reflection or to another module. See stubs-gen/run.sh for the command and
# docs/internal/fix-safety.md for the argument.
#
# It emits two tables:
#
#   JAVA_REFLECTIVE_MEMBERS: the member names (Java method names and the
#   Jython bean-property and field spellings) whose declared type is a
#   reflection-bearing type (java.lang.Class / ClassLoader / Thread /
#   StackTraceElement, java.lang.reflect.*, java.lang.invoke.*,
#   org.python.core interpreter handles), reached from every vetted Java
#   class (and java.lang.Object, which every value exposes) and
#   transitively through those reflection types. Naming any of these on any
#   value is the first hop out of plain data into reflection, so dynamic.rs
#   treats it as introspective and fails closed.
#
#   MODULE_VETTED_ATTRS: for every vetted Python module, the attributes that
#   are themselves vetted (a function, a class, or a constant of an
#   immutable vetted type). Every other attribute (a module, a
#   PySystemState such as `re.sys`, a Java package or class, or anything
#   else) is unknown code.
#
# usage: jython gen_vetted.py VETTED_RS OUT_RS

import re
import sys
import types

from java.lang import Class


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
    return modules, java_classes


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


# --- the Java reflection closure -------------------------------------------

# The reflection-bearing types. Only pure Java reflection: holding one of
# these lets a script name classes, methods, fields, loaders and threads,
# from which it reaches `org.python.core.Py`, `PySystemState`, the frame
# and `eval`. The org.python interpreter handles are deliberately NOT in
# this set: a script can reach one only through these Java types (so the
# first hop is already caught) or through an `org.python` import (caught by
# `is_reflection_import`), and `PyObject` is the base of every Jython value
# whose protocol methods (`__getitem__`, `get`, ...) are not reflection.
# The frame and code attributes (`f_locals`, `co_*`, ...) are covered by
# `INTROSPECTION_ATTRS` in dynamic.rs.
REFLECTION_SEEDS = [
    'java.lang.Class',
    'java.lang.ClassLoader',
    # NOTE: Thread, ThreadGroup and StackTraceElement are deliberately NOT
    # here. A Thread is an ordinary object until its ClassLoader is named
    # (`getContextClassLoader`, which IS flagged because its return type is
    # a ClassLoader), so `Thread.currentThread()` on its own, ubiquitous in
    # Ignition, is not a route. A StackTraceElement yields only line numbers
    # and class-NAME strings (turning a name into a class needs
    # `Class.forName`, flagged); its line numbers are protected by the
    # line-preserving safety net, not by this set.
    'java.lang.reflect.Method',
    'java.lang.reflect.Field',
    'java.lang.reflect.Constructor',
    'java.lang.reflect.AccessibleObject',
    'java.lang.reflect.Executable',
    'java.lang.reflect.Member',
    'java.lang.reflect.Parameter',
    'java.lang.reflect.Type',
    'java.lang.reflect.AnnotatedElement',
    'java.lang.reflect.GenericDeclaration',
    'java.lang.reflect.Proxy',
    'java.lang.invoke.MethodHandle',
    'java.lang.invoke.MethodHandles$Lookup',
    'java.lang.invoke.MethodType',
    'java.lang.invoke.VarHandle',
]

# The only types a plain script can hold directly without first naming a
# reflection member: every value is a java.lang.Object, exceptions are
# Throwable (vetted), and `Thread.currentThread()` is a common static
# entry. Scanning exactly these (plus the vetted classes) gives the
# first-hop names: a member on a plain value whose result is reflective.
# Reflection types themselves are NOT scanned; their accessors (`methods`,
# `classLoader`, ...) are reachable only on a reflection object already
# obtained through a first-hop name, so names that double as ordinary data
# members (`type`, `parent`) stay out. The distinctly-reflection Java API
# names below are added unconditionally as defence in depth, in case a
# reflection object enters through some other Java value.
SCAN_EXTRA_SEEDS = ['java.lang.Object', 'java.lang.Throwable', 'java.lang.Thread']

# Named Java reflection API entry points (method and bean spellings). None
# of these double as an ordinary Perspective member, so flagging them
# everywhere costs no real precision.
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


def load(name):
    try:
        return Class.forName(name)
    except Exception:
        return None


def cname(c):
    return Class.getName(c)


def unwrap(c):
    # The element type of an array type, following nesting.
    comp = Class.getComponentType(c)
    while comp is not None:
        c = comp
        comp = Class.getComponentType(c)
    return c


def bean(name):
    # The JavaBeans property name Jython exposes for a getter.
    for pfx in ('get', 'is'):
        if name.startswith(pfx) and len(name) > len(pfx):
            rest = name[len(pfx):]
            if len(rest) >= 2 and rest[0].isupper() and rest[1].isupper():
                return rest
            return rest[0].lower() + rest[1:]
    return None


def java_reflective_members(java_classes):
    seed_classes = []
    for n in REFLECTION_SEEDS:
        c = load(n)
        if c is not None:
            seed_classes.append(c)

    def is_rbt(c):
        c = unwrap(c)
        if cname(c) in REFLECTION_SEEDS:
            return True
        for r in seed_classes:
            if Class.isAssignableFrom(r, c):
                return True
        return False

    names = set()
    # Python-level introspection spellings that are always a route.
    names.update(['class', 'getClass', '__class__'])
    names.update(EXPLICIT_REFLECTION_API)

    # First-hop scan: only the types a plain value can be, never reflection
    # types (see the comment on SCAN_EXTRA_SEEDS).
    worklist = list(SCAN_EXTRA_SEEDS) + list(java_classes)
    seen = set()
    while worklist:
        n = worklist.pop()
        if n in seen:
            continue
        seen.add(n)
        c = load(n)
        if c is None:
            continue
        # Jython turns a java.lang.Class value into a Python type, so its
        # instance methods must be called unbound with the class as self.
        for m in Class.getMethods(c):
            if is_rbt(m.getReturnType()):
                names.add(m.getName())
                b = bean(m.getName())
                if b:
                    names.add(b)
        for f in Class.getFields(c):
            if is_rbt(f.getType()):
                names.add(f.getName())
    return sorted(names)


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
    vetted_rs, out_rs = sys.argv[1], sys.argv[2]
    modules, java_classes = read_inputs(vetted_rs)
    members = java_reflective_members(java_classes)
    attrs = module_vetted_attrs(modules)

    signature = ['mod:%s' % m for m in modules] + ['java:%s' % c for c in sorted(java_classes)]

    lines = []
    lines.append('// @generated by stubs-gen/gen_vetted.py; do not edit by hand.')
    lines.append('// Run `stubs-gen/run.sh --vetted` (needs the Ignition 8.3.3 image)')
    lines.append('// to regenerate. The test `generated_tables_match_vetted_inputs`')
    lines.append('// fails when the vetted lists change without regenerating.')
    lines.append('')
    lines.append('/// The vetted module names and Java classes this file was generated')
    lines.append('/// from, sorted. The regeneration test compares it with the live')
    lines.append('/// vetted lists.')
    lines.append('#[cfg_attr(not(test), allow(dead_code))]')
    lines.append('pub(crate) const GENERATED_FROM: &[&str] = &[')
    lines.append(rust_str_list(signature, 4))
    lines.append('];')
    lines.append('')
    lines.append('/// Member names (Java method, Jython bean-property and field')
    lines.append('/// spellings) whose value is or yields a reflection-bearing type.')
    lines.append('/// Naming any of these, on any value or in any string literal, is a')
    lines.append('/// route into Java reflection and is treated as introspective.')
    lines.append('/// Sorted for binary search.')
    lines.append('pub(crate) const JAVA_REFLECTIVE_MEMBERS: &[&str] = &[')
    lines.append(rust_str_list(members, 4))
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
