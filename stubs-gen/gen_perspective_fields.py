# Generates crates/iglint_project/src/generated/perspective_fields.rs: which
# JSON fields of Perspective bindings and transforms Ignition parses as an
# expression or a property path, and which members of the component script
# object return the component's name. Runs inside IA's Jython 2.7 (the
# inductiveautomation/ignition:8.3.3 image) against the image's own jars;
# needs no gateway. See stubs-gen/perspective-fields.sh for the command and
# docs/internal/fix-safety.md ("Machine-measured facts") for the argument.
#
# Expression fields are measured, not read from documentation:
#
#   1. Every binding and transform type registered by Perspective's
#      BindingRegistryImpl is constructed through the registry, the way a
#      view loads it, with a BindingContext whose target element records
#      every property reference it is asked to create.
#   2. The configurations are generated from the module's JSON schemas
#      (schemas/binding-<type>.json, schemas/transform-<type>.json): a base
#      configuration, plus one variant per alternative of every oneOf /
#      anyOf and per value (and absence) of every enumerated string.
#   3. In each variant, one JSON position at a time (every value, a sample
#      key in every free-form object, and every identifier-like string
#      constant of the binding classes as an extra key of every object) is
#      replaced by a marker `{this.custom.iglintprobeN}`. A field is an
#      expression when the marker's reference reaches the element, and a
#      property path when the raw marker text does.
#   4. Completeness: every method in any jar of the image that calls an
#      expression or property-reference parser (PerspectiveExpression.create,
#      PerspectiveExpressionFactory.apply, PerspectiveTransformExpression
#      .create, ELParserHarness.parse, PerspectiveElement
#      .createPropertyReference) must either have been on the stack of a
#      recorded marker, or be one of the classified non-configuration
#      callers in NOT_CONFIGURATION below. Otherwise the generator fails. So
#      an event action, a component configuration or a binding type whose
#      fields are parsed as expressions cannot be missed silently.
#
# Component name members are measured by a return-value flow analysis of the
# bytecode: a method returns the name when a value it returns may derive from
# a call of getName() on a Perspective element (PerspectiveElement,
# ComponentModel, ...), directly or through methods that return it, string
# building and wrappers. It runs over the public members of the component
# script object (ComponentModelScriptWrapper$SafetyWrapper) and of every
# component model delegate, in Java and bean spellings.
#
# usage: jython gen_perspective_fields.py OUT_RS PATH_OF_THIS_SCRIPT

import json
import re
import sys
from itertools import combinations

import jarray
from java.io import File, FileOutputStream
from java.lang import Class, ClassLoader, Object, Thread
from java.lang import Throwable as JavaThrowable
from java.lang.reflect import InvocationHandler, Modifier, Proxy
from java.net import URL, URLClassLoader
from java.util import ArrayList, HashMap, HashSet, Optional
from java.util.zip import ZipFile
from org.python.objectweb.asm import ClassReader, ClassVisitor, MethodVisitor, Opcodes, Type

IGNITION = '/usr/local/bin/ignition'
UNPACK = '/tmp/iglint-perspective-fields-jars'
P = 'com.inductiveautomation.perspective.'
REGISTRY = P + 'gateway.binding.BindingRegistryImpl'
BINDING_CONTEXT = P + 'gateway.api.BindingContext'
ELEMENT = P + 'gateway.api.PerspectiveElement'
BINDING_CONFIG = P + 'common.config.BindingConfig'
JSON_PARSER = 'com.inductiveautomation.ignition.common.gson.JsonParser'
COMPONENT_WRAPPER = P + 'gateway.script.ComponentModelScriptWrapper$SafetyWrapper'
DELEGATE = P + 'gateway.api.ComponentModelDelegate'
MARKER = 'iglintprobe'

# The parsers. A call of any of these from configuration means the value is
# an expression or a property path.
SINKS = [
    ('com/inductiveautomation/perspective/gateway/binding/PerspectiveExpression', 'create'),
    ('com/inductiveautomation/perspective/gateway/binding/PerspectiveExpressionFactory', 'apply'),
    ('com/inductiveautomation/perspective/gateway/binding/transforms/expression/PerspectiveTransformExpression', 'create'),
    ('com/inductiveautomation/ignition/common/expressions/parsing/ELParserHarness', 'parse'),
    ('com/inductiveautomation/perspective/gateway/api/PerspectiveElement', 'createPropertyReference'),
]

# Callers of the parsers that do not read view configuration, by class name
# prefix, with the reason. Everything else must be reached by the probe.
NOT_CONFIGURATION = [
    (P + 'gateway.binding.PerspectiveExpression.', 'the parser itself (create)'),
    (P + 'gateway.binding.PerspectiveExpressionFactory.', 'the parser interface (bridge method)'),
    (P + 'gateway.binding.transforms.expression.PerspectiveTransformExpression.',
     'the transform parser itself (create)'),
    (P + 'gateway.expression.BoundPropertyExpression.',
     'a `{...}` property reference inside an expression being parsed'),
    (P + 'gateway.expression.PropertyFunction.',
     'the `property(path)` expression function: its path is a run-time value'),
    (P + 'gateway.model.', 'implementations of createPropertyReference on views, pages, components'),
    (P + 'gateway.property.', 'property references themselves'),
    (P + 'designer.', 'Designer-only code: not run by the gateway'),
    ('com.inductiveautomation.perspective.gateway.binding.BindingRegistryImpl$Missing',
     'placeholders for unknown types'),
]

# Transform types whose schema file is named differently.
SCHEMA_ALIASES = {('transform', 'expression'): 'transform-expr'}

# Types without a schema: the minimal configuration Perspective writes.
NO_SCHEMA = {
    ('transform', 'script'): {'type': 'object', 'properties': {'code': {'type': 'string', 'default': 'return value'}}},
}


# --- jars ---------------------------------------------------------------------

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


def image_jars():
    """lib/core/common, lib/core/gateway, and every jar of every module."""
    out = []
    for sub in ['common', 'gateway']:
        d = File(IGNITION + '/lib/core/' + sub)
        for f in sorted(d.listFiles(), key=lambda f: f.getName()):
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


def class_bytes(jars):
    """(jar, entry name) of every class, first jar wins."""
    seen = {}
    for jar in jars:
        try:
            z = ZipFile(jar)
        except Exception:
            continue
        for e in iter_enum(z.entries()):
            n = e.getName()
            if n.endswith('.class') and not n.startswith('META-INF') and n not in seen:
                seen[n] = jar
        z.close()
    return seen


def read_class(jar, name, visitor):
    z = ZipFile(jar)
    try:
        ClassReader(z.getInputStream(z.getEntry(name))).accept(visitor, ClassReader.SKIP_FRAMES)
    finally:
        z.close()


# --- static scan: parser call sites and string constants ---------------------

class ScanMethod(MethodVisitor):
    def __init__(self, owner, where, out):
        MethodVisitor.__init__(self, Opcodes.ASM9)
        self.owner = owner
        self.where = where
        self.out = out

    def visitMethodInsn(self, op, owner, name, desc, itf):
        if (owner, name) in self.out['sink_set'] and self.where[1] != 'bridge':
            self.out['callers'].add(self.where[0])

    def visitLdcInsn(self, value):
        if isinstance(value, basestring):
            self.out['strings'].setdefault(self.owner, set()).add(value)


class ScanClass(ClassVisitor):
    def __init__(self, out):
        ClassVisitor.__init__(self, Opcodes.ASM9)
        self.out = out

    def visit(self, version, access, name, sig, sup, itfs):
        self.name = name

    def visitMethod(self, access, name, desc, sig, exc):
        bridge = 'bridge' if access & Opcodes.ACC_BRIDGE else ''
        where = ('%s.%s' % (self.name.replace('/', '.'), name), bridge)
        return ScanMethod(self.name, where, self.out)


def scan(classes):
    out = {'sink_set': set(SINKS), 'callers': set(), 'strings': {}}
    for entry, jar in sorted(classes.items()):
        try:
            read_class(jar, entry, ScanClass(out))
        except (Exception, JavaThrowable):
            pass
    return out


# --- probing ------------------------------------------------------------------

# Values the stubs return for common Java types, so that code walking them
# goes on.
EMPTY = {
    'java.lang.String': lambda: 'iglint',
    'java.util.Map': HashMap,
    'java.util.List': ArrayList,
    'java.util.Collection': ArrayList,
    'java.util.Set': HashSet,
    'java.lang.Iterable': ArrayList,
}


class Recorder(InvocationHandler):
    def __init__(self, probe):
        self.probe = probe

    def invoke(self, proxy, method, args):
        n = method.getName()
        if n == 'createPropertyReference':
            self.probe.record(args[0])
            return None
        if n == 'getTargetElement':
            return self.probe.element
        if n in ('toString', 'getName', 'getDiagnosticPath', 'getQualifiedPath'):
            return 'iglint'
        if n == 'hashCode':
            return 1
        if n == 'equals':
            return proxy is args[0]
        rt = method.getReturnType()
        if Class.isPrimitive(rt):
            if Class.getName(rt) == 'boolean':
                return False
            if Class.getName(rt) == 'void':
                return None
            return 0
        if Class.getName(rt) == 'java.util.Optional':
            return Optional.empty()
        empty = EMPTY.get(Class.getName(rt))
        if empty is not None:
            return empty()
        if Class.getName(rt).startswith('com.inductiveautomation.'):
            # A session, gateway context, ...: another recording stub, so
            # that construction goes on to the next field.
            return self.probe.stub(rt)
        return None


class Probe(object):
    def __init__(self, loader):
        self.loader = loader
        handler = Recorder(self)
        self.element = Proxy.newProxyInstance(loader, jarray.array([self.load(ELEMENT)], Class),
                                              handler)
        self.context = Proxy.newProxyInstance(
            loader, jarray.array([self.load(BINDING_CONTEXT)], Class), handler)
        self.parser = self.load(JSON_PARSER)
        self.config_class = self.load(BINDING_CONFIG)
        reg_class = self.load(REGISTRY)
        self.registry = reg_class.newInstance()
        self.types = {}
        self.factories = {}
        for kind, field in [('binding', 'bindings'), ('transform', 'transforms')]:
            f = reg_class.getDeclaredField(field)
            f.setAccessible(True)
            m = f.get(self.registry)
            self.types[kind] = sorted(str(k) for k in m.keySet())
            for k in m.keySet():
                self.factories[(kind, str(k))] = m.get(k)
        self.recorded = []
        self.frames = set()
        self.stubs = {}

    def stub(self, t):
        n = Class.getName(t)
        if not Class.isInterface(t):
            return None
        if n not in self.stubs:
            self.stubs[n] = Proxy.newProxyInstance(self.loader, jarray.array([t], Class),
                                                   Recorder(self))
        return self.stubs[n]

    def load(self, name):
        return Class.forName(name, True, self.loader)

    def factory_package(self, kind, type_name, trace):
        """The package of the type's implementation: the factory's, or for
        a factory registered as a constructor reference, that of the first
        class below the registry on the stack of the failure."""
        name = Class.getName(self.factories[(kind, type_name)].getClass())
        if '$$Lambda' in name:
            below = [c for c in trace if c.startswith(P + 'gateway.binding.')
                     and not c.startswith(REGISTRY)]
            if not below:
                raise Exception('%s %s: no implementation class on %r' % (kind, type_name, trace))
            # The outermost frame below the registry.
            name = below[-1]
        return name.rsplit('.', 1)[0]

    def record(self, text):
        text = unicode(text)
        if MARKER not in text:
            return
        self.recorded.append(text)
        for f in Thread.currentThread().getStackTrace():
            self.frames.add('%s.%s' % (f.getClassName(), f.getMethodName()))

    def run(self, kind, type_name, config):
        """The marker texts recorded while building `config`."""
        self.recorded = []
        self.last_error = None
        text = json.dumps(config)
        try:
            if kind == 'binding':
                cfg = self.config_class.newInstance()
                self.config_class.getField('type').set(cfg, type_name)
                self.config_class.getField('config').set(
                    cfg, self.parser.parseString(text).getAsJsonObject())
                made = self.registry.createBinding(self.context, cfg)
                try:
                    made.resolveReferences()
                except (Exception, JavaThrowable):
                    pass
            else:
                obj = dict(config)
                obj['type'] = type_name
                made = self.registry.createTransform(
                    self.context, self.parser.parseString(json.dumps(obj)).getAsJsonObject())
            # The registry turns a configuration error into a placeholder
            # (BrokenBinding, BrokenTransform) that publishes the error.
            name = Class.getName(made.getClass())
            if '.Broken' in name or '$Missing' in name:
                text = []
                for f in made.getClass().getDeclaredFields():
                    f.setAccessible(True)
                    m = re.search(r'Error_\w+\("(.*)"\)', unicode(f.get(made)))
                    if m:
                        text.append(m.group(1))
                self.last_error = ' '.join(text) or name
        except (Exception, JavaThrowable), e:
            self.last_error = unicode(e)
            self.last_trace = []
            if isinstance(e, JavaThrowable):
                self.last_trace = [f.getClassName() for f in e.getStackTrace()]
        return list(self.recorded)


# --- configurations from the schemas -----------------------------------------

class Schema(object):
    def __init__(self, doc):
        self.doc = doc

    def resolve(self, s):
        seen = 0
        while isinstance(s, dict) and '$ref' in s and seen < 16:
            ref = s['$ref']
            seen += 1
            if ref.startswith('#/'):
                node = self.doc
                for part in ref[2:].split('/'):
                    node = node[part]
                rest = dict((k, v) for k, v in s.items() if k != '$ref')
                merged = dict(node)
                merged.update(rest)
                s = merged
            else:
                # An external schema (style properties): free-form.
                return {'type': 'object'}
        return s


def alternatives(s):
    for key in ('oneOf', 'anyOf'):
        if key in s:
            return key, s[key]
    return None, None


def leaf_string(s):
    d = s.get('default')
    if isinstance(d, basestring) and d:
        return d
    for key in ('enum', 'suggestions'):
        vals = [v for v in s.get(key, []) if isinstance(v, basestring) and v]
        if vals:
            return vals[0]
    for key in ('const', 'constant'):
        if isinstance(s.get(key), basestring):
            return s[key]
    return '1'


def build(schema, s, path, choice):
    """An instance of schema node `s` at instance path `path` (a tuple).
    `choice` maps (path, 'alt') to an alternative index and (path, 'enum')
    to a string value or None (absent). Returns (value, points): points
    are the discriminators met, as (path, kind, options)."""
    s = schema.resolve(s)
    points = []
    key, alts = alternatives(s)
    if alts:
        i = choice.get((path, 'alt'), 0)
        points.append((path, 'alt', range(len(alts))))
        alt = schema.resolve(alts[i])
        merged = dict((k, v) for k, v in s.items() if k not in ('oneOf', 'anyOf'))
        props = dict(merged.get('properties', {}))
        props.update(alt.get('properties', {}))
        merged.update(dict((k, v) for k, v in alt.items() if k != 'properties'))
        if props:
            merged['properties'] = props
        if 'type' not in merged and props:
            merged['type'] = 'object'
        value, more = build(schema, merged, path, choice)
        return value, points + more
    t = s.get('type')
    if isinstance(t, list):
        # A field that also takes a string: the string form (expressions
        # are strings; some readers require one).
        if 'string' in t:
            t = 'string'
            d = s.get('default')
            if isinstance(d, (int, float)) and not isinstance(d, bool):
                s = dict(s)
                s['default'] = str(d)
        else:
            t = [x for x in t if x != 'null'][0] if [x for x in t if x != 'null'] else 'null'
    if t is None:
        if 'properties' in s:
            t = 'object'
        elif 'default' in s:
            d = s['default']
            t = {dict: 'object', list: 'array', bool: 'boolean', int: 'number',
                 float: 'number'}.get(type(d), 'string')
        else:
            t = 'string'
    if t == 'object':
        props = s.get('properties')
        if not props:
            points.append((path, 'free', None))
            return {'k1': '1'}, points
        out = {}
        for name in sorted(props):
            v, more = build(schema, props[name], path + (name,), choice)
            points.extend(more)
            if v is not ABSENT:
                out[name] = v
        return out, points
    if t == 'array':
        items = s.get('items')
        if isinstance(items, dict):
            v, more = build(schema, items, path + (0,), choice)
            points.extend(more)
            return ([v] if v is not ABSENT else []), points
        return ['1'], points
    if t == 'string':
        options = [v for v in s.get('enum', []) + s.get('suggestions', [])
                   if isinstance(v, basestring)]
        for key in ('constant', 'const'):
            if isinstance(s.get(key), basestring):
                options.append(s[key])
        options = with_java_constants(options)
        if options:
            points.append((path, 'enum', options + [None]))
        if (path, 'enum') in choice:
            v = choice[(path, 'enum')]
            return (ABSENT if v is None else v), points
        return leaf_string(s), points
    if t in ('number', 'integer'):
        d = s.get('default')
        return (d if isinstance(d, (int, float)) and not isinstance(d, bool) else 1), points
    if t == 'boolean':
        points.append((path, 'enum', [True, False, None]))
        if (path, 'enum') in choice:
            v = choice[(path, 'enum')]
            return (ABSENT if v is None else v), points
        d = s.get('default')
        return (d if isinstance(d, bool) else False), points
    return None, points


# The constant names of the Java enums of the configuration classes. The
# schemas give the spelling the Designer suggests; some fields are read
# with Enum.valueOf and need the constant's own spelling.
JAVA_ENUMS = []


def with_java_constants(options):
    lower = set(o.lower() for o in options)
    out = list(options)
    for constants in JAVA_ENUMS:
        if lower & set(c.lower() for c in constants):
            out.extend(c for c in constants if c not in out)
    return out


class Absent(object):
    pass


ABSENT = Absent()


def variants(schema):
    """The base configuration and one variant per discriminator value:
    [(config, choice)]."""
    base, points = build(schema, schema.doc, (), {})
    out = [(base, {})]
    seen = set([json.dumps(base, sort_keys=True)])
    queue = list(points)
    done = set()
    while queue:
        path, kind, options = queue.pop(0)
        if kind == 'free' or (path, kind) in done:
            continue
        done.add((path, kind))
        for o in options:
            choice = {(path, kind): o}
            v, more = build(schema, schema.doc, (), choice)
            key = json.dumps(v, sort_keys=True)
            if key not in seen:
                seen.add(key)
                out.append((v, choice))
            for p in more:
                if (p[0], p[1]) not in done:
                    queue.append(p)
    return out


def nodes(value, path=()):
    """Every position of an instance: (path, value)."""
    yield path, value
    if isinstance(value, dict):
        for k in sorted(value):
            for x in nodes(value[k], path + (k,)):
                yield x
    elif isinstance(value, list):
        for i, v in enumerate(value):
            for x in nodes(v, path + (i,)):
                yield x


def put(value, path, new):
    """A copy of `value` with the position `path` set to `new` (a missing
    last key is added)."""
    import copy as _copy
    if not path:
        return new
    out = _copy.deepcopy(value)
    node = out
    for p in path[:-1]:
        node = node[p]
    if new is ABSENT:
        del node[path[-1]]
    else:
        node[path[-1]] = new
    return out


def pattern(path, free):
    """The instance path as a table pattern: `a.b[].c`, `*` for a key of a
    free-form object (`free`: the paths of free-form objects)."""
    out = ''
    for i, p in enumerate(path):
        if isinstance(p, int):
            out += '[]'
        else:
            seg = '*' if path[:i] in free else p
            out += ('.' if out else '') + seg
    return out


def free_containers(schema, choice):
    _, points = build(schema, schema.doc, (), choice)
    return set(p for p, kind, _ in points if kind == 'free')


# How deep values below a free-form key are probed: every combination of
# objects and lists of up to this many levels.
NEST_DEPTH = 3


def nestings(depth):
    """Every sequence of 'object' / 'list' steps of length 1 .. depth-1
    (the key itself is the first level)."""
    out = []
    frontier = [()]
    for _ in range(depth - 1):
        frontier = [f + (s,) for f in frontier for s in ('object', 'list')]
        out.extend(frontier)
    return out


def nested(steps):
    """A value nested along `steps` with '1' at the bottom: (value, path
    of the bottom below the value, paths of the free-form objects)."""
    value = '1'
    path = ()
    containers = []
    for i, step in enumerate(reversed(steps)):
        value = {'k%d' % (len(steps) - i + 1): value} if step == 'object' else [value]
    node_path = ()
    for i, step in enumerate(steps):
        if step == 'object':
            containers.append(node_path)
            node_path += ('k%d' % (i + 2),)
        else:
            node_path += (0,)
    return value, node_path, containers


def probe_type(probe, kind, type_name, schema, extra_keys):
    """{(pattern, sink): set of variant indexes}, {pattern: set of variant
    indexes where the position exists} and the variants."""
    found = {}
    present = {}
    vs = variants(schema)
    counter = [0]

    def attempt(vi, config, path, free, schema_position=True):
        # A key the schema does not list counts only where it is parsed:
        # alone it may mean nothing (an end date without a start date).
        pat = pattern(path, free)
        if schema_position:
            present.setdefault(pat, set()).add(vi)
        contexts = [config]
        # A free-form key may be used only when another field names it
        # (an indirect tag path's `{key}` placeholder).
        for i, p in enumerate(path):
            if path[:i] in free:
                for q in string_leaves(config):
                    if q[:len(path[:i + 1])] != path[:i + 1]:
                        contexts.append(put(config, q, '{%s}' % p))
        for context in contexts:
            counter[0] += 1
            m = '%s%d' % (MARKER, counter[0])
            for text in ['{this.custom.%s}' % m, 'this.custom.%s' % m]:
                got = [g for g in probe.run(kind, type_name, put(context, path, text)) if m in g]
                if got:
                    sink = 'expression'
                    if text[0] != '{' or any('{' in g for g in got):
                        sink = 'property'
                    found.setdefault((pat, sink), set()).add(vi)
                    present.setdefault(pat, set()).add(vi)
                    return

    # Configurations Perspective accepts: each variant with the repairs of
    # the base, and its own when it still fails. A variant that cannot be
    # repaired (an invalid value of the varied field, or construction
    # needing more of a gateway than the stubs give) is not probed.
    base_repairs = repair(probe, kind, type_name, schema, vs[0][0], vs[0][1], 2)[0] or {}
    accepted = []
    rejected = []
    others = []
    for _, choice in vs:
        others.extend(choice.items())
    for vi, (config, choice) in enumerate(vs):
        config = apply_repairs(config, choice, base_repairs)
        fixes, error = repair(probe, kind, type_name, schema, config, choice, 1)
        if fixes is None:
            # Combined with another alternative (a real-time date range
            # with tags given as an expression, ...).
            for key, value in others:
                if key in choice:
                    continue
                both = dict(choice)
                both[key] = value
                trial = apply_repairs(build(schema, schema.doc, (), both)[0], both, base_repairs)
                probe.run(kind, type_name, trial)
                if probe.last_error is None:
                    fixes, config, choice = {}, trial, both
                    break
        if fixes is None:
            rejected.append((vi, error))
            accepted.append((config, choice))
            continue
        accepted.append((apply_repairs(config, choice, fixes), choice))
    vs = accepted
    if len(rejected) == len(vs):
        # Nothing to probe. Acceptable only when the type's code calls no
        # parser at all (a script transform needs a script manager).
        probe.run(kind, type_name, vs[0][0])
        package = probe.factory_package(kind, type_name, probe.last_trace)
        callers = [c for c in probe.callers if c.startswith(package + '.')]
        if callers:
            raise Exception('%s %s: no configuration is accepted (%s) and %s parse'
                            % (kind, type_name, rejected, callers))
        UNCONSTRUCTED.append((kind, type_name, package, rejected[0][1]))
        return {}, {}, vs, rejected
    skip = set(vi for vi, _ in rejected)
    for vi, (config, choice) in enumerate(vs):
        if vi in skip:
            continue
        free = free_containers(schema, choice)
        objects = []
        for p, v in nodes(config):
            if p:
                attempt(vi, config, p, free)
            if isinstance(v, dict):
                objects.append(p)
        for p in objects:
            # A value below a free-form key may itself be an object or a
            # list; and keys the schema does not list may be read.
            if p in free:
                # Objects and lists nested up to NEST_DEPTH below the key.
                for steps in nestings(NEST_DEPTH):
                    q = p + ('k1',)
                    shape, inner, containers = nested(steps)
                    f2 = set(free) | set(q + c for c in containers)
                    attempt(vi, put(config, q, shape), q + inner, f2)
                continue
            for k in extra_keys:
                if k not in value_at(config, p):
                    attempt(vi, config, p + (k,), free, False)
    return found, present, vs, rejected


def string_leaves(config):
    return [p for p, v in nodes(config) if p and isinstance(v, basestring)]


def repair(probe, kind, type_name, schema, base, choice, max_size):
    """Changes that make `base` acceptable to Perspective, where the
    schema's defaults are not (an empty expression, a constant spelled
    differently, an empty optional name, a list whose items need a
    gateway): ({path: value}, None), or (None, error). Searches changes of
    up to `max_size` positions: every other value of an enumerated string
    and of strings with the same key, the empty string, a plain
    expression, an emptied list or object, and no value; never the
    variant's own discriminator."""
    probe.run(kind, type_name, base)
    if probe.last_error is None:
        return {}, None
    error = probe.last_error
    options = {}
    _, points = build(schema, schema.doc, (), choice)
    for path, kind_, opts in points:
        if kind_ == 'enum':
            options[path] = [o for o in opts if isinstance(o, basestring)]
    by_key = {}
    for path, opts in options.items():
        if path and not isinstance(path[-1], int):
            by_key.setdefault(path[-1], []).extend(opts)
    fixed = set(path for (path, _) in choice)
    alts = []
    for p, v in nodes(base):
        if p and p not in fixed and isinstance(v, (list, dict)):
            for c in ([] if isinstance(v, list) else {}, ABSENT):
                if c != v:
                    alts.append((p, c))
    for p in string_leaves(base):
        if p in fixed:
            continue
        current = value_at(base, p)
        same = by_key.get(p[-1], []) if not isinstance(p[-1], int) else []
        for c in options.get(p, []) + same + ['', '1', ABSENT]:
            if c != current and (p, c) not in alts:
                alts.append((p, c))
    for size in range(1, max_size + 1):
        for combo in combinations(alts, size):
            paths = [p for p, _ in combo]
            if any(a[:len(b)] == b for a in paths for b in paths if a is not b):
                continue
            trial = base
            for p, c in combo:
                trial = put(trial, p, c)
            probe.run(kind, type_name, trial)
            if probe.last_error is None:
                return dict(combo), None
    return None, error


def apply_repairs(config, choice, repairs):
    chosen = set(path for (path, _) in choice)
    for p, v in sorted(repairs.items()):
        if p not in chosen and value_at(config, p) is not ABSENT:
            config = put(config, p, v)
    return config


def value_at(config, path):
    node = config
    for p in path:
        if isinstance(node, dict) and p in node:
            node = node[p]
        elif isinstance(node, list) and isinstance(p, int) and p < len(node):
            node = node[p]
        else:
            return ABSENT
    return node


def condition(on, present, vs):
    """None when the field is parsed in every variant where it exists;
    otherwise the enumerated field (path, values) whose value decides it."""
    if present <= on:
        return None
    paths = set()
    for config, choice in vs:
        for (path, kind) in choice:
            if kind == 'enum':
                paths.add(path)
    for path in sorted(paths):
        values_on = set(json_value(value_at(vs[i][0], path)) for i in on)
        values_off = set(json_value(value_at(vs[i][0], path)) for i in present - on)
        if not (values_on & values_off):
            return ('.'.join(str(p) for p in path), sorted(values_on))
    raise Exception('no enumerated field decides whether the field is parsed: '
                    'parsed in variants %r of %r' % (sorted(on), sorted(present)))


def json_value(v):
    return 'absent' if v is ABSENT else json.dumps(v)


def schema_for(kind, type_name, loader_jars):
    if (kind, type_name) in NO_SCHEMA:
        return Schema(NO_SCHEMA[(kind, type_name)])
    name = SCHEMA_ALIASES.get((kind, type_name), '%s-%s' % (kind, type_name))
    for jar in loader_jars:
        try:
            z = ZipFile(jar)
        except Exception:
            continue
        e = z.getEntry('schemas/%s.json' % name)
        if e is not None:
            data = read_all(z.getInputStream(e))
            z.close()
            return Schema(json.loads(data))
        z.close()
    raise Exception('no schema for %s %s' % (kind, type_name))


def read_all(ins):
    buf = jarray.zeros(65536, 'b')
    chunks = []
    while True:
        n = ins.read(buf)
        if n < 0:
            break
        chunks.append(buf.tostring()[:n])
    ins.close()
    return ''.join(chunks).decode('utf-8')


IDENT = re.compile(r'^[A-Za-z][A-Za-z0-9]*$')
STATS = []
UNCONSTRUCTED = []


def measure_fields(loader, jars, scanned):
    probe = Probe(loader)
    probe.callers = sorted(scanned['callers'])
    for owner in sorted(scanned['strings']):
        if not owner.startswith('com/inductiveautomation/perspective/common/config/'):
            continue
        try:
            c = Class.forName(owner.replace('/', '.'), False, loader)
            if Class.isEnum(c):
                JAVA_ENUMS.append([str(x.name()) for x in Class.getEnumConstants(c)])
        except (Exception, JavaThrowable):
            pass
    keys = set()
    for owner, strings in scanned['strings'].items():
        if (owner.startswith('com/inductiveautomation/perspective/common/config/binding/')
                or owner.startswith('com/inductiveautomation/perspective/gateway/binding/')
                or owner == 'com/inductiveautomation/perspective/common/config/BindingConfig'):
            keys.update(s for s in strings if IDENT.match(s))
    keys = sorted(keys)
    rows = []
    for kind in ('binding', 'transform'):
        for type_name in probe.types[kind]:
            schema = schema_for(kind, type_name, jars)
            found, present, vs, rejected = probe_type(probe, kind, type_name, schema, keys)
            STATS.append((kind, type_name, len(vs), rejected))
            for (pat, sink), ids in sorted(found.items()):
                try:
                    cond = condition(ids, present[pat], vs)
                except Exception, e:
                    raise Exception('%s %s %s: %s; not parsed in: %s' % (
                        kind, type_name, pat, e,
                        [json.dumps(vs[i][0]) for i in sorted(present[pat] - ids)]))
                rows.append((kind, type_name, pat, sink, cond))
    return rows, probe


def check_coverage(scanned, probe):
    """Fails unless every Perspective method that calls a parser was
    reached by a marker or is classified. Returns the callers outside
    Perspective (other resource kinds), by package."""
    missing = []
    others = {}
    for caller in sorted(scanned['callers']):
        if caller in probe.frames:
            continue
        if not caller.startswith(P):
            pkg = '.'.join(caller.split('.')[:4])
            others[pkg] = others.get(pkg, 0) + 1
            continue
        if any(caller.startswith(prefix) for prefix, _ in NOT_CONFIGURATION):
            continue
        missing.append(caller)
    if missing:
        raise Exception('parser callers not reached by the probe and not classified: %s'
                        % ', '.join(missing))
    return sorted(others.items())


# --- component name members: return-value flow over the bytecode ---------

def bean(name, nparams):
    # The JavaBeans property name Jython exposes for a no-argument getter
    # (the same rule as gen_vetted.py).
    if nparams != 0:
        return None
    for pfx in ('get', 'is'):
        if name.startswith(pfx) and len(name) > len(pfx):
            rest = name[len(pfx):]
            if len(rest) >= 2 and rest[0].isupper() and rest[1].isupper():
                return rest
            return rest[0].lower() + rest[1:]
    return None


class MethodCode(MethodVisitor):
    """Records a method's instructions as tuples, for the flow analysis."""
    def __init__(self, out):
        MethodVisitor.__init__(self, Opcodes.ASM9)
        self.out = out
        self.handlers = set()

    def visitInsn(self, op):
        self.out.append(('insn', op))

    def visitIntInsn(self, op, operand):
        self.out.append(('int', op))

    def visitVarInsn(self, op, var):
        self.out.append(('var', op, var))

    def visitTypeInsn(self, op, t):
        self.out.append(('type', op))

    def visitFieldInsn(self, op, owner, name, desc):
        self.out.append(('field', op, desc))

    def visitMethodInsn(self, op, owner, name, desc, itf):
        self.out.append(('call', op, owner, name, desc))

    def visitInvokeDynamicInsn(self, name, desc, bsm, args):
        self.out.append(('indy', desc))

    def visitJumpInsn(self, op, label):
        self.out.append(('jump', op, label))

    def visitLabel(self, label):
        self.out.append(('label', label))

    def visitLdcInsn(self, value):
        from java.lang import Long, Double
        self.out.append(('ldc', 2 if isinstance(value, (Long, Double)) or
                         (isinstance(value, (long, float)) and not isinstance(value, int)) else 1))

    def visitIincInsn(self, var, inc):
        pass

    def visitTableSwitchInsn(self, lo, hi, dflt, labels):
        self.out.append(('switch',))

    def visitLookupSwitchInsn(self, dflt, keys, labels):
        self.out.append(('switch',))

    def visitMultiANewArrayInsn(self, desc, dims):
        self.out.append(('multi', dims))

    def visitTryCatchBlock(self, start, end, handler, t):
        self.handlers.add(handler)


class ClassCode(ClassVisitor):
    def __init__(self):
        ClassVisitor.__init__(self, Opcodes.ASM9)
        self.methods = {}
        self.super_name = None
        self.interfaces = []

    def visit(self, version, access, name, sig, sup, itfs):
        self.name = name
        self.super_name = sup
        self.interfaces = list(itfs or [])

    def visitMethod(self, access, name, desc, sig, exc):
        if access & Opcodes.ACC_ABSTRACT:
            return None
        code = []
        mv = MethodCode(code)
        self.methods[(name, desc)] = (access, code, mv)
        return mv


def value_size(desc):
    return 2 if desc in ('J', 'D') else 1


# Stack effects of the zero-operand instructions: (pops, pushes) in slots;
# the pushed value is derived from the popped ones.
def zero_operand_effects():
    o = Opcodes
    e = {}
    for op in (o.NOP,):
        e[op] = (0, 0)
    for op in (o.ACONST_NULL, o.ICONST_M1, o.ICONST_0, o.ICONST_1, o.ICONST_2, o.ICONST_3,
               o.ICONST_4, o.ICONST_5, o.FCONST_0, o.FCONST_1, o.FCONST_2):
        e[op] = (0, 1)
    for op in (o.LCONST_0, o.LCONST_1, o.DCONST_0, o.DCONST_1):
        e[op] = (0, 2)
    for op in (o.IALOAD, o.FALOAD, o.AALOAD, o.BALOAD, o.CALOAD, o.SALOAD):
        e[op] = (2, 1)
    for op in (o.LALOAD, o.DALOAD):
        e[op] = (2, 2)
    for op in (o.IASTORE, o.FASTORE, o.AASTORE, o.BASTORE, o.CASTORE, o.SASTORE):
        e[op] = (3, 0)
    for op in (o.LASTORE, o.DASTORE):
        e[op] = (4, 0)
    for op in (o.IADD, o.ISUB, o.IMUL, o.IDIV, o.IREM, o.ISHL, o.ISHR, o.IUSHR, o.IAND,
               o.IOR, o.IXOR, o.FADD, o.FSUB, o.FMUL, o.FDIV, o.FREM, o.FCMPL, o.FCMPG):
        e[op] = (2, 1)
    for op in (o.LADD, o.LSUB, o.LMUL, o.LDIV, o.LREM, o.LAND, o.LOR, o.LXOR, o.DADD,
               o.DSUB, o.DMUL, o.DDIV, o.DREM):
        e[op] = (4, 2)
    for op in (o.LSHL, o.LSHR, o.LUSHR):
        e[op] = (3, 2)
    for op in (o.LCMP, o.DCMPL, o.DCMPG):
        e[op] = (4, 1)
    for op in (o.INEG, o.FNEG, o.I2F, o.F2I, o.I2B, o.I2C, o.I2S, o.ARRAYLENGTH):
        e[op] = (1, 1)
    for op in (o.LNEG, o.DNEG, o.L2D, o.D2L):
        e[op] = (2, 2)
    for op in (o.I2L, o.I2D, o.F2L, o.F2D):
        e[op] = (1, 2)
    for op in (o.L2I, o.L2F, o.D2I, o.D2F):
        e[op] = (2, 1)
    for op in (o.POP, o.MONITORENTER, o.MONITOREXIT):
        e[op] = (1, 0)
    e[o.POP2] = (2, 0)
    return e


ZERO = zero_operand_effects()
RETURNS = None


class NameFlow(object):
    """Which methods may return a value derived from a Perspective
    element's name (`PerspectiveElement.getName()` and its
    implementations), over the classes of the image."""

    def __init__(self, loader, classes):
        self.loader = loader
        self.classes = classes          # entry name -> jar
        self.code = {}                  # internal class name -> ClassCode
        self.summary = {}               # (class, name, desc) -> bool
        self.busy = set()
        self.subclasses = None
        self.element = Class.forName(ELEMENT, False, loader)
        self.changed = False

    def class_code(self, internal):
        if internal not in self.code:
            entry = internal + '.class'
            cc = None
            if entry in self.classes:
                cc = ClassCode()
                try:
                    read_class(self.classes[entry], entry, cc)
                except (Exception, JavaThrowable):
                    cc = None
            self.code[internal] = cc
        return self.code[internal]

    def is_element(self, internal):
        try:
            c = Class.forName(internal.replace('/', '.'), False, self.loader)
            return Class.isAssignableFrom(self.element, c)
        except (Exception, JavaThrowable):
            return False

    def implementations(self, owner, name, desc, virtual):
        """(class, name, desc) of the bodies a call may run."""
        out = []
        c = owner
        while c is not None:
            cc = self.class_code(c)
            if cc is None:
                break
            if (name, desc) in cc.methods:
                out.append((c, name, desc))
                break
            c = cc.super_name
        if virtual:
            for sub in self.subclasses_of(owner):
                cc = self.class_code(sub)
                if cc is not None and (name, desc) in cc.methods:
                    out.append((sub, name, desc))
        return out

    def subclasses_of(self, owner):
        if self.subclasses is None:
            self.subclasses = {}
            for entry in self.classes:
                internal = entry[:-6]
                if not internal.startswith('com/inductiveautomation/'):
                    continue
                cc = self.class_code(internal)
                if cc is None:
                    continue
                for parent in [cc.super_name] + cc.interfaces:
                    if parent:
                        self.subclasses.setdefault(parent, []).append(internal)
        out = []
        todo = [owner]
        seen = set()
        while todo:
            c = todo.pop()
            for sub in self.subclasses.get(c, []):
                if sub not in seen:
                    seen.add(sub)
                    out.append(sub)
                    todo.append(sub)
        return out

    def returns_name(self, key):
        if key in self.summary:
            return self.summary[key]
        if key in self.busy:
            return False
        self.busy.add(key)
        result = self.analyse(key)
        self.busy.discard(key)
        if self.summary.get(key) != result:
            self.changed = True
        self.summary[key] = result
        return result

    def call_returns_name(self, op, owner, name, desc):
        if name == 'getName' and desc == '()Ljava/lang/String;' and self.is_element(owner):
            return True
        if not owner.startswith('com/inductiveautomation/'):
            return False
        virtual = op in (Opcodes.INVOKEVIRTUAL, Opcodes.INVOKEINTERFACE)
        return any(self.returns_name(k) for k in self.implementations(owner, name, desc, virtual))

    def analyse(self, key):
        owner, name, desc = key
        cc = self.class_code(owner)
        if cc is None or (name, desc) not in cc.methods:
            return False
        access, code, mv = cc.methods[(name, desc)]
        # Locals: flow-insensitive taint, iterated to a fixed point.
        nlocals = {}
        returned = [False]
        for _ in range(4):
            before = dict(nlocals)
            self.run(code, mv.handlers, nlocals, returned)
            if nlocals == before:
                break
        return returned[0]

    def run(self, code, handlers, local, returned):
        o = Opcodes
        stack = []      # list of (taint, size)
        saved = {}      # label -> stack at a jump to it
        dead = False

        def pop(slots):
            taint = False
            while slots > 0 and stack:
                t, sz = stack.pop()
                taint = taint or t
                slots -= sz
            return taint

        def push(taint, size):
            stack.append((taint, size))

        for ins in code:
            kind = ins[0]
            if kind == 'label':
                label = ins[1]
                if label in handlers:
                    stack = [(False, 1)]
                    dead = False
                elif dead:
                    stack = list(saved.get(label, []))
                    dead = False
                continue
            if dead:
                continue
            if kind == 'insn':
                op = ins[1]
                if op in ZERO:
                    pops, pushes = ZERO[op]
                    t = pop(pops)
                    if pushes:
                        push(t, pushes)
                elif op in (o.IRETURN, o.LRETURN, o.FRETURN, o.DRETURN, o.ARETURN):
                    t = pop(2 if op in (o.LRETURN, o.DRETURN) else 1)
                    if op == o.ARETURN and t:
                        returned[0] = True
                    dead = True
                elif op in (o.RETURN, o.ATHROW):
                    dead = True
                elif op == o.DUP:
                    if stack:
                        stack.append(stack[-1])
                elif op == o.DUP_X1:
                    a = stack.pop(); b = stack.pop()
                    stack.extend([a, b, a])
                elif op == o.DUP_X2:
                    a = stack.pop(); b = stack.pop()
                    if b[1] == 2:
                        stack.extend([a, b, a])
                    else:
                        c = stack.pop()
                        stack.extend([a, c, b, a])
                elif op == o.DUP2:
                    a = stack[-1]
                    if a[1] == 2:
                        stack.append(a)
                    else:
                        b = stack[-2]
                        stack.extend([b, a])
                elif op == o.DUP2_X1:
                    a = stack.pop()
                    if a[1] == 2:
                        b = stack.pop()
                        stack.extend([a, b, a])
                    else:
                        b = stack.pop(); c = stack.pop()
                        stack.extend([b, a, c, b, a])
                elif op == o.DUP2_X2:
                    a = stack.pop()
                    if a[1] == 2:
                        b = stack.pop()
                        if b[1] == 2:
                            stack.extend([a, b, a])
                        else:
                            c = stack.pop()
                            stack.extend([a, c, b, a])
                    else:
                        b = stack.pop(); c = stack.pop()
                        if c[1] == 2:
                            stack.extend([b, a, c, b, a])
                        else:
                            d = stack.pop()
                            stack.extend([b, a, d, c, b, a])
                elif op == o.SWAP:
                    a = stack.pop(); b = stack.pop()
                    stack.extend([a, b])
                else:
                    raise Exception('unknown opcode %d' % op)
            elif kind == 'int':
                if ins[1] == o.NEWARRAY:
                    pop(1)
                push(False, 1)
            elif kind == 'var':
                op, var = ins[1], ins[2]
                if op in (o.ILOAD, o.FLOAD, o.ALOAD):
                    push(local.get(var, False), 1)
                elif op in (o.LLOAD, o.DLOAD):
                    push(local.get(var, False), 2)
                elif op in (o.ISTORE, o.FSTORE, o.ASTORE, o.LSTORE, o.DSTORE):
                    t = pop(2 if op in (o.LSTORE, o.DSTORE) else 1)
                    local[var] = local.get(var, False) or t
            elif kind == 'type':
                op = ins[1]
                if op == o.NEW:
                    push(False, 1)
                elif op == o.ANEWARRAY:
                    pop(1)
                    push(False, 1)
                elif op == o.CHECKCAST:
                    t = pop(1)
                    push(t, 1)
                elif op == o.INSTANCEOF:
                    pop(1)
                    push(False, 1)
            elif kind == 'field':
                op, fdesc = ins[1], ins[2]
                size = value_size(fdesc)
                if op == o.GETSTATIC:
                    push(False, size)
                elif op == o.PUTSTATIC:
                    pop(size)
                elif op == o.GETFIELD:
                    pop(1)
                    push(False, size)
                elif op == o.PUTFIELD:
                    pop(size)
                    pop(1)
            elif kind == 'call':
                op, owner, mname, mdesc = ins[1], ins[2], ins[3], ins[4]
                t = Type.getMethodType(mdesc)
                taint = False
                for a in reversed(list(t.getArgumentTypes())):
                    taint = pop(a.getSize()) or taint
                if op != o.INVOKESTATIC:
                    taint = pop(1) or taint
                rsize = t.getReturnType().getSize()
                if rsize:
                    push(taint or self.call_returns_name(op, owner, mname, mdesc), rsize)
            elif kind == 'indy':
                t = Type.getMethodType(ins[1])
                taint = False
                for a in reversed(list(t.getArgumentTypes())):
                    taint = pop(a.getSize()) or taint
                rsize = t.getReturnType().getSize()
                if rsize:
                    push(taint, rsize)
            elif kind == 'jump':
                op, label = ins[1], ins[2]
                if op in (o.IFEQ, o.IFNE, o.IFLT, o.IFGE, o.IFGT, o.IFLE, o.IFNULL, o.IFNONNULL):
                    pop(1)
                elif op in (o.IF_ICMPEQ, o.IF_ICMPNE, o.IF_ICMPLT, o.IF_ICMPGE, o.IF_ICMPGT,
                            o.IF_ICMPLE, o.IF_ACMPEQ, o.IF_ACMPNE):
                    pop(2)
                elif op == o.JSR:
                    push(False, 1)
                merged = saved.get(label)
                if merged is None or len(merged) != len(stack):
                    saved[label] = list(stack)
                else:
                    saved[label] = [(a[0] or b[0], a[1]) for a, b in zip(merged, stack)]
                if op == o.GOTO:
                    dead = True
            elif kind == 'ldc':
                push(False, ins[1])
            elif kind == 'switch':
                pop(1)
                dead = True
            elif kind == 'multi':
                pop(ins[1])
                push(False, 1)


def public_methods(c, skip):
    out = []
    for m in Class.getMethods(c):
        if Class.getName(m.getDeclaringClass()) in skip:
            continue
        out.append(m)
    return out


def component_name_members(loader, classes):
    """The member names (Java and bean spellings) of the component script
    object and of every component model delegate whose value may derive
    from the component's name."""
    flow = NameFlow(loader, classes)
    targets = []
    wrapper = Class.forName(COMPONENT_WRAPPER, False, loader)
    for m in public_methods(wrapper, ['java.lang.Object']):
        targets.append(m)
    base = Class.forName(DELEGATE, False, loader)
    for entry in sorted(classes):
        if not entry.startswith('com/inductiveautomation/') or '$' in entry:
            continue
        try:
            c = Class.forName(entry[:-6].replace('/', '.'), False, loader)
            if c == base or not Class.isAssignableFrom(base, c):
                continue
            if not Modifier.isPublic(Class.getModifiers(c)):
                continue
            targets.extend(public_methods(c, ['java.lang.Object', DELEGATE]))
        except (Exception, JavaThrowable):
            continue
    names = set()
    analysed = 0
    # Summaries assumed false while a method is being analysed (a cycle)
    # are recomputed until nothing changes.
    for _ in range(8):
        flow.changed = False
        busy_free = dict(flow.summary)
        flow.summary = {}
        names = set()
        for m in targets:
            owner = Class.getName(m.getDeclaringClass()).replace('.', '/')
            key = (owner, m.getName(), Type.getMethodDescriptor(m))
            analysed += 1
            if flow.returns_name(key):
                names.add(m.getName())
                b = bean(m.getName(), len(m.getParameterTypes()))
                if b:
                    names.add(b)
        if flow.summary == busy_free:
            break
    return sorted(names), len(targets), len(flow.summary)


def quiet():
    # The image's classes log through logback; keep the output readable.
    from org.slf4j import LoggerFactory
    from ch.qos.logback.classic import Level
    LoggerFactory.getLogger('ROOT').setLevel(Level.OFF)


def compress(rows):
    """Replaces the patterns below a free-form key that hold an expression
    at every probed nesting (`struct.*`, `struct.*.*`, `struct.*[]`, ...)
    by one recursive pattern `struct.**`."""
    out = []
    by_type = {}
    for r in rows:
        by_type.setdefault((r[0], r[1]), []).append(r)
    for key in sorted(by_type):
        rs = by_type[key]
        plain = set((r[2], r[3]) for r in rs if r[4] is None)
        recursive = set()
        for pat, sink in plain:
            if not pat.endswith('.*'):
                continue
            prefix = pat[:-2]
            wanted = set([prefix + '.*'])
            for steps in nestings(NEST_DEPTH):
                wanted.add(prefix + '.*' + ''.join('.*' if st == 'object' else '[]'
                                                   for st in steps))
            if all((w, sink) in plain for w in wanted):
                recursive.add((prefix, sink))
        for r in rs:
            covered = any(r[3] == sink and r[4] is None and r[2].startswith(prefix + '.*')
                          for prefix, sink in recursive)
            if not covered:
                out.append(r)
        for prefix, sink in sorted(recursive):
            out.append((key[0], key[1], prefix + '.**', sink, None))
    return sorted(out)


def fnv1a64(data):
    h = 0xcbf29ce484222325
    for ch in data:
        h ^= ord(ch)
        h = (h * 0x100000001b3) & 0xffffffffffffffff
    return '%016x' % h


def rust_str(s):
    return json.dumps(s)


def emit(out_rs, rows, names, probe, others, name_stats):
    source = open(sys.argv[2], 'rb').read().replace('\r\n', '\n')
    lines = []
    lines.append('// @generated by stubs-gen/gen_perspective_fields.py; do not edit by hand.')
    lines.append('// Run `stubs-gen/perspective-fields.sh` (needs the Ignition 8.3.3 image)')
    lines.append('// to regenerate. The test `generated_fields_match_the_generator` fails')
    lines.append('// when the generator changes without regenerating.')
    lines.append('//')
    lines.append('// Binding types: %s.' % ', '.join(probe.types['binding']))
    lines.append('// Transform types: %s.' % ', '.join(probe.types['transform']))
    lines.append('// Configurations probed per type (accepted / generated):')
    for kind, type_name, total, rejected in STATS:
        lines.append('//   %s %s: %d / %d' % (kind, type_name, total - len(rejected), total))
    for kind, type_name, package, error in UNCONSTRUCTED:
        lines.append('// Not constructed without a gateway: %s %s (%s); no method of'
                     % (kind, type_name, error.split(':')[0]))
        lines.append('//   %s calls a parser.' % package)
    lines.append('// Every Perspective method that calls an expression or property')
    lines.append('// parser was reached by a probe or is classified in the generator')
    lines.append('// (NOT_CONFIGURATION). Parser callers outside Perspective (other')
    lines.append('// resource kinds), by package:')
    for pkg, n in others:
        lines.append('//   %s: %d' % (pkg, n))
    lines.append('// Name members: %d members of the component script object and the'
                 % name_stats[0])
    lines.append('// component model delegates, %d method bodies analysed.' % name_stats[1])
    lines.append('')
    lines.append('/// FNV-1a 64 of `stubs-gen/gen_perspective_fields.py` when this file')
    lines.append('/// was generated.')
    lines.append('#[cfg_attr(not(test), allow(dead_code))]')
    lines.append('pub(crate) const GENERATOR_HASH: &str = "%s";' % fnv1a64(source))
    lines.append('')
    lines.append('/// The configuration fields Perspective parses: (owner, type, path,')
    lines.append('/// sink, condition field, condition values). `owner` is `binding` or')
    lines.append('/// `transform`; `path` is relative to the binding\'s `config` (or the')
    lines.append('/// transform object), with `*` for any key of a free-form object, `[]`')
    lines.append('/// for any item of a list and a final `**` for any value at any depth')
    lines.append('/// below; `sink` is `expression` or `property` (a property path). When')
    lines.append('/// the condition field is not empty, the field is parsed only when the')
    lines.append('/// condition field (relative to the same root) holds one of the values,')
    lines.append('/// given as JSON text (`absent`: no value). Sorted.')
    lines.append('#[allow(clippy::type_complexity)]')
    lines.append('pub(crate) const CONFIG_FIELDS: &[(&str, &str, &str, &str, &str, &[&str])] = &[')
    for kind, type_name, pat, sink, cond in rows:
        field, values = cond if cond else ('', [])
        lines.append('    (%s, %s, %s, %s, %s, &[%s]),' % (
            rust_str(kind), rust_str(type_name), rust_str(pat), rust_str(sink),
            rust_str(field), ', '.join(rust_str(v) for v in values)))
    lines.append('];')
    lines.append('')
    lines.append('/// Members of the component script object (Java and bean spellings,')
    lines.append('/// including the component model delegates\' script-callable methods)')
    lines.append('/// whose value may derive from the component\'s name. Sorted.')
    lines.append('pub(crate) const COMPONENT_NAME_MEMBERS: &[&str] = &[')
    for n in names:
        lines.append('    %s,' % rust_str(n))
    lines.append('];')
    f = open(out_rs, 'wb')
    f.write(('\n'.join(lines) + '\n').encode('utf-8'))
    f.close()


def self_check(loader, classes):
    """The flow analysis must see a name through string building."""
    flow = NameFlow(loader, classes)
    key = ('com/inductiveautomation/perspective/gateway/api/PerspectiveElement',
           'getQualifiedPath', '()Ljava/lang/String;')
    if not flow.returns_name(key):
        raise Exception('the name flow analysis misses PerspectiveElement.getQualifiedPath')


def main():
    out_rs = sys.argv[1]
    quiet()
    jars = image_jars()
    classes = class_bytes(jars)
    scanned = scan(classes)
    own = [f for f in jars if '/lib/core/common/' not in f.getPath()]
    urls = jarray.array([f.toURI().toURL() for f in own], URL)
    loader = URLClassLoader(urls, ClassLoader.getSystemClassLoader())
    self_check(loader, classes)
    names, ntargets, nsummaries = component_name_members(loader, classes)
    rows, probe = measure_fields(loader, jars, scanned)
    others = check_coverage(scanned, probe)
    emit(out_rs, compress(rows), names, probe, others, (ntargets, nsummaries))


if __name__ == '__main__':
    main()
