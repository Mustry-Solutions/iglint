# Runs inside IA's Jython 2.7 (see oracle/scope/run.sh). Compiles each source
# with Jython's own compiler and records, per code object, how the generated
# code accesses every name: which PyFrame accessor (by name, global, fast
# local, cell or free variable) the compiler chose for each read, store and
# delete, with the spelling it used (after private-name mangling), and every
# attribute name (also mangled).
#
# usage: jython jython_scope_oracle.py CASES_FILE OUTPUT_FILE
#
# CASES_FILE holds sources introduced by "### <name>" lines (see
# jython_compile_oracle.py). OUTPUT_FILE gets, per case:
#
#   ### <name>
#   ok
#   scope <kind> <co_name> <co_firstlineno>
#   <op> <access> <name>
#   ...
#
# or "error <lineno> <offset>" and the message when compile() rejects the
# source. <kind> is module, class or function (functions, lambdas and
# generator expressions). <op> is load, store or del for names and
# attr-load, attr-store or attr-del for attributes (with access "-").
# <access> is name (PyFrame.getname, setlocal(String), dellocal(String): the
# namespace lookups of module and class bodies), global (getglobal,
# setglobal, delglobal) or global! when the scope declares the name global,
# fast (getlocal(int) and friends, named through co_varnames), cell or free
# (getderef/setderef, named through co_cellvars and co_freevars). Each access
# is listed once however often it occurs (Jython emits a `finally` clause
# once per way out of its `try`). Scopes with the same kind, name and line
# are merged; lines are sorted. Code the bytecode cannot reach has no
# accesses: the class writer replaces it with nop/athrow. The compiler's
# own temporaries (list comprehension accumulators "_[...]" and generator
# arguments "_(...)") are left out.
import sys

sys.path.insert(0, '/oracle')
import jython_compile_oracle as base

from java.io import ByteArrayOutputStream
from java.lang import Class, Integer, String as JString, Throwable
from org.python.core import BytecodeLoader, CompileMode, CompilerFlags, ParserFacade, PyTableCode
from org.python.compiler import Module
from org.python.objectweb.asm import ClassReader, ClassVisitor, MethodVisitor, Opcodes, Type

import ast

DEBUG = bool(__import__('os').environ.get('IGLINT_SCOPE_DEBUG'))
API = Opcodes.ASM7 if hasattr(Opcodes, 'ASM7') else Opcodes.ASM5
FRAME = 'org/python/core/PyFrame'
PYOBJECT = 'org/python/core/PyObject'
NAME_OPS = {
    ('getname', '(Ljava/lang/String;)Lorg/python/core/PyObject;'): ('load', 'name'),
    ('setlocal', '(Ljava/lang/String;Lorg/python/core/PyObject;)V'): ('store', 'name'),
    ('dellocal', '(Ljava/lang/String;)V'): ('del', 'name'),
    ('getglobal', '(Ljava/lang/String;)Lorg/python/core/PyObject;'): ('load', 'global'),
    ('setglobal', '(Ljava/lang/String;Lorg/python/core/PyObject;)V'): ('store', 'global'),
    ('delglobal', '(Ljava/lang/String;)V'): ('del', 'global'),
}
INDEX_OPS = {
    ('getlocal', '(I)Lorg/python/core/PyObject;'): ('load', 'fast'),
    ('setlocal', '(ILorg/python/core/PyObject;)V'): ('store', 'fast'),
    ('dellocal', '(I)V'): ('del', 'fast'),
    ('getderef', '(I)Lorg/python/core/PyObject;'): ('load', 'deref'),
    ('setderef', '(ILorg/python/core/PyObject;)V'): ('store', 'deref'),
}
ATTR_OPS = {
    '__getattr__': 'attr-load',
    '__setattr__': 'attr-store',
    '__delattr__': 'attr-del',
    'invoke': 'attr-load',
}
SCOPE_DESC = '(Lorg/python/core/PyFrame;Lorg/python/core/ThreadState;)Lorg/python/core/PyObject;'


class Method(MethodVisitor):
    """Collects the name accesses of one generated method. Strings pushed by
    LDC wait on a stack until a call with String parameters (or an AASTORE
    into a String array) consumes them, so a name is attributed to the call
    that uses it even when other code runs in between."""

    def __init__(self, record):
        MethodVisitor.__init__(self, API)
        self.record = record
        self.strings = []
        self.string_locals = {}
        self.last_int = None
        self.last_was_string = False
        self.last_code_field = None
        self.ops = []


    def visitLdcInsn(self, value):
        if isinstance(value, (str, unicode)) or isinstance(value, JString):
            self.strings.append(unicode(value))
            self.last_was_string = True
            return
        if isinstance(value, (int, long)) or isinstance(value, Integer):
            self.last_int = int(value)
        self.last_was_string = False

    def visitInsn(self, opcode):
        if Opcodes.ICONST_M1 <= opcode <= Opcodes.ICONST_5:
            self.last_int = opcode - Opcodes.ICONST_0
        elif opcode == Opcodes.AASTORE and self.last_was_string and self.strings:
            self.strings.pop()
        self.last_was_string = False

    def visitIntInsn(self, opcode, operand):
        if opcode in (Opcodes.BIPUSH, Opcodes.SIPUSH):
            self.last_int = operand
        self.last_was_string = False

    def visitVarInsn(self, opcode, var):
        # The compiler keeps a name in a temporary for augmented attribute
        # assignment: LDC, ASTORE, then ALOAD before each use.
        if opcode == Opcodes.ASTORE:
            if self.last_was_string and self.strings:
                self.string_locals[var] = self.strings.pop()
            else:
                self.string_locals.pop(var, None)
        elif opcode == Opcodes.ALOAD and var in self.string_locals:
            self.strings.append(self.string_locals[var])
        self.last_was_string = False

    def visitFrame(self, type_, nlocal, local, nstack, stack):
        # Control flow may join here from code laid out later (a loop's
        # test after its body): keep only temporaries the frame still
        # types as strings.
        types = list(local or [])[:nlocal]
        for var in list(self.string_locals):
            if var >= len(types) or types[var] != 'java/lang/String':
                del self.string_locals[var]

    def visitFieldInsn(self, opcode, owner, name, desc):
        if desc == 'Lorg/python/core/PyCode;':
            self.last_code_field = name
        self.last_was_string = False

    def visitMethodInsn(self, opcode, owner, name, desc, itf=False):
        self.last_was_string = False
        count = sum(1 for t in Type.getArgumentTypes(desc) if t.getDescriptor() == 'Ljava/lang/String;')
        taken = []
        for _ in range(count):
            if self.strings:
                taken.insert(0, self.strings.pop())
        if owner == FRAME and (name, desc) in NAME_OPS and taken:
            op, access = NAME_OPS[(name, desc)]
            self.ops.append((op, access, taken[0]))
        elif owner == FRAME and (name, desc) in INDEX_OPS:
            op, access = INDEX_OPS[(name, desc)]
            self.ops.append((op, access, self.last_int))
        elif owner == PYOBJECT and name in ATTR_OPS and taken:
            self.ops.append((ATTR_OPS[name], '-', taken[0]))


class Collector(ClassVisitor):
    def __init__(self):
        ClassVisitor.__init__(self, API)
        self.methods = {}

    def visitMethod(self, access, name, desc, signature, exceptions):
        if desc != SCOPE_DESC or name in ('call_function',):
            return None
        m = Method(None)
        self.methods[name] = m
        return m


def jfield(obj, name):
    """A public Java field of a PyTableCode (Jython's exposed types hide
    them from attribute access)."""
    return Class.getField(PyTableCode, name).get(obj)


def code_objects(data):
    """The PyTableCode objects of a compiled module, by field name."""
    cls = BytecodeLoader.makeClass('oracle$py', data)
    funcs = cls('<oracle>')
    out = {}
    for field in Class.getDeclaredFields(cls):
        field.setAccessible(True)
        try:
            value = field.get(funcs)
        except Exception:
            continue
        if isinstance(value, PyTableCode):
            out[field.getName()] = value
    return out


def mangle(class_name, name):
    if class_name is None or not name.startswith('__') or name.endswith('__'):
        return name
    return '_' + class_name.lstrip('_') + name


def declared_globals(tree):
    """{(co_name, lineno): set of names declared global}, from the AST."""
    out = {}

    def walk_body(nodes, key, class_name):
        for node in nodes:
            walk(node, key, class_name)

    def walk(node, key, class_name):
        if not isinstance(node, ast.AST):
            return
        if isinstance(node, ast.Global):
            out.setdefault(key, set()).update(node.names)
            return
        if isinstance(node, ast.FunctionDef):
            for d in list(node.decorator_list) + list(node.args.defaults):
                walk(d, key, class_name)
            inner = (mangle(class_name, node.name), node.lineno)
            out.setdefault(inner, set())
            walk_body(node.body, inner, class_name)
            return
        if isinstance(node, ast.ClassDef):
            for d in list(node.decorator_list) + list(node.bases):
                walk(d, key, class_name)
            inner = (mangle(class_name, node.name), node.lineno)
            out.setdefault(inner, set())
            walk_body(node.body, inner, mangle(class_name, node.name))
            return
        for field in node._fields:
            value = getattr(node, field, None)
            if isinstance(value, ast.AST):
                walk(value, key, class_name)
            elif value is not None and not isinstance(value, (str, unicode, int, long, float)):
                try:
                    items = list(value)
                except TypeError:
                    continue
                for item in items:
                    if isinstance(item, ast.AST):
                        walk(item, key, class_name)

    walk_body(tree.body, ('<module>', 1), None)
    return out


def scope_ops(source):
    flags = CompilerFlags()
    node = ParserFacade.parse(source, CompileMode.exec, '<oracle>', flags)
    out = ByteArrayOutputStream()
    Module.compile(node, out, 'oracle$py', '<oracle>', True, False, flags)
    data = out.toByteArray()
    collector = Collector()
    ClassReader(data).accept(collector, ClassReader.EXPAND_FRAMES)
    codes = code_objects(data)
    explicit = declared_globals(ast.parse(source))
    scopes = {}
    for field, code in codes.items():
        method = collector.methods.get(field)
        if method is None:
            continue
        name = unicode(jfield(code, 'co_name'))
        line = jfield(code, 'co_firstlineno')
        if name == '<module>':
            kind = 'module'
            line = 1
        elif method.ops[:2] == [('load', 'name', u'__name__'), ('store', 'name', u'__module__')]:
            # A class body starts with `__module__ = __name__` by name.
            kind = 'class'
        else:
            kind = 'function'
        varnames = list(jfield(code, 'co_varnames') or [])
        cellvars = list(jfield(code, 'co_cellvars') or [])
        derefs = cellvars + list(jfield(code, 'co_freevars') or [])
        ncell = len(cellvars)
        declared = explicit.get((name, line), set())
        lines = scopes.setdefault((line, kind, name), {})
        for op, access, arg in method.ops:
            if access == 'fast':
                arg = varnames[arg] if arg is not None and 0 <= arg < len(varnames) else '?%s' % arg
            elif access == 'deref':
                if arg is not None and 0 <= arg < len(derefs):
                    access = 'cell' if arg < ncell else 'free'
                    arg = derefs[arg]
                else:
                    arg = '?%s' % arg
            if arg.startswith('_[') or arg.startswith('_('):
                continue
            if access == 'global' and arg in declared:
                access = 'global!'
            key = (op, access, arg)
            lines[key] = lines.get(key, 0) + 1
    result = []
    for (line, kind, name) in sorted(scopes):
        result.append('scope %s %s %d' % (kind, name, line))
        ops = scopes[(line, kind, name)]
        for key in sorted(ops):
            result.append('%s %s %s' % key)
    return result


def run(name, data):
    if data.startswith(base.codecs.BOM_UTF8):
        data = data[len(base.codecs.BOM_UTF8):]
    result = base.parse(data)
    lines = ['### %s' % base.one_line(name)]
    if result[0] != 'ok':
        lines.append('error %s %s' % (result[1], result[2]))
        lines.append(base.one_line(result[3]))
        return lines
    try:
        source = data.decode('utf-8')
        ops = scope_ops(source)
    except (Exception, Throwable), e:
        if DEBUG:
            raise
        lines.append('error 0 0')
        lines.append('internal error: %s' % e.__class__.__name__)
        return lines
    lines.append('ok')
    lines.extend(base.one_line(l) for l in ops)
    return lines


def main():
    cases_file, output_file = sys.argv[1], sys.argv[2]
    data = base.read(cases_file)
    out = open(output_file, 'wb')
    try:
        for name, source in base.split_cases(data):
            if isinstance(source, unicode):
                source = source.encode('utf-8')
            for line in run(name, source):
                if isinstance(line, unicode):
                    line = line.encode('utf-8')
                out.write(line + '\n')
    finally:
        out.close()


if __name__ == '__main__':
    main()
