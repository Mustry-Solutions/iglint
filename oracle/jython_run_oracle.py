# Runs inside IA's Jython 2.7 (see run-oracle.sh --run). Compiles and runs
# many sources in one JVM, each as Ignition runs a project library module
# (compiled from unicode text, executed in a fresh module namespace whose
# __name__ is the module's name), and records what each one does: the
# compile error, or what it printed, the exception it raised (with the
# line), and the values of its globals. Two sources that give the same
# record behave the same for every observation the records cover; the
# layout-fix differential test compares sources before and after iglint's
# safe fixes with it (crates/iglint_linter/tests/layout_fix_oracle.rs).
#
# usage: jython jython_run_oracle.py LIST_FILE OUTPUT_FILE
#
# LIST_FILE holds one path per line. OUTPUT_FILE gets one record per
# source, in input order:
#
#   ### <path>
#   compile-error <lineno> <message>
#
# or
#
#   ### <path>
#   ok                              (or: raised <Type> <lineno> <message>)
#   out <what it printed>
#   g <name> <value>                (one line per global, sorted by name)
#
# Values are repr() for None, bool, numbers, strings and containers of
# them, and the type name for anything else (functions, classes, modules).
# Line breaks and backslashes in text are escaped so that every record
# line is one line.
import sys

from java.lang import Throwable
from StringIO import StringIO

PLAIN = (type(None), bool, int, long, float, complex, str, unicode)


def one_line(text):
    if isinstance(text, unicode):
        text = text.encode('utf-8')
    return text.replace('\\', '\\\\').replace('\r', '\\r').replace('\n', '\\n')


def value(v, depth=0):
    if isinstance(v, PLAIN):
        return repr(v)
    if depth > 5:
        return '...'
    if isinstance(v, (list, tuple)):
        inner = ', '.join([value(x, depth + 1) for x in v])
        return '%s(%s)' % (type(v).__name__, inner)
    if isinstance(v, dict):
        items = sorted([(value(k, depth + 1), value(x, depth + 1)) for k, x in v.items()])
        return 'dict(%s)' % ', '.join(['%s: %s' % kv for kv in items])
    return '<%s>' % type(v).__name__


def run(data):
    lines = []
    try:
        source = data.decode('utf-8')
    except UnicodeDecodeError:
        return ['compile-error 0 source is not valid UTF-8']
    try:
        code = compile(source, 'm', 'exec')
    except SyntaxError, e:
        return ['compile-error %s %s' % (e.lineno, one_line(unicode(e.msg)))]
    except Exception, e:
        return ['compile-error 0 internal error: %s' % e.__class__.__name__]
    except Throwable, e:
        return ['compile-error 0 internal error: %s' % e.getClass().getName()]
    namespace = {'__name__': 'm'}
    out = StringIO()
    saved = sys.stdout
    sys.stdout = out
    try:
        try:
            exec code in namespace
            status = 'ok'
        except Exception, e:
            tb = sys.exc_info()[2]
            while tb.tb_next is not None:
                tb = tb.tb_next
            status = 'raised %s %s %s' % (
                e.__class__.__name__, tb.tb_lineno, one_line(unicode(e)))
        except Throwable, e:
            status = 'raised %s' % e.getClass().getName()
    finally:
        sys.stdout = saved
    lines.append(status)
    lines.append('out %s' % one_line(out.getvalue()))
    for name in sorted(namespace.keys()):
        if name == '__builtins__':
            continue
        lines.append('g %s %s' % (name, one_line(value(namespace[name]))))
    return lines


def read(path):
    f = open(path, 'rb')
    try:
        return f.read()
    finally:
        f.close()


def main():
    list_file, output_file = sys.argv[1], sys.argv[2]
    paths = [line.rstrip('\r\n') for line in open(list_file, 'rb')]
    out = open(output_file, 'wb')
    try:
        for path in paths:
            if not path:
                continue
            out.write('### %s\n' % one_line(path))
            for line in run(read(path)):
                out.write('%s\n' % line)
    finally:
        out.close()


if __name__ == '__main__':
    main()
