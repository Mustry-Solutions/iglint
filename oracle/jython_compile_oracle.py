# Runs inside IA's Jython 2.7 (see run-oracle.sh --compile). Compiles many
# inputs in one JVM, the way Ignition compiles scripts (full compilation, not
# only parsing), and records whether Jython accepts each one.
#
# usage: jython jython_compile_oracle.py LIST_FILE OUTPUT_FILE
#
# LIST_FILE holds one path per line. A path ending in ".cases" is a case file:
# it holds many small sources, each introduced by a line "### <name>", and
# every case is parsed on its own. A case name may contain the flags
# "[nonl]" (drop the final newline), "[crlf]" or "[cr]" (rewrite line ends).
# Any other path is parsed as one source.
#
# OUTPUT_FILE gets one record per source, in input order:
#
#   ### <name>
#   ok
#
# or, when parsing fails:
#
#   ### <name>
#   error <lineno> <offset>
#   <message>
#
# <name> is the case name for case files and the path otherwise. Sources are
# decoded as UTF-8 (a UTF-8 BOM is dropped) and compiled as unicode, which is
# how Ignition hands script text to Jython. Sources that declare an encoding
# in their first two lines are compiled from their bytes instead, because
# Jython rejects encoding declarations in unicode source.
import codecs
import re
import sys

from java.lang import Throwable

CODING = re.compile(r'#.*coding[:=]\s*([-\w.]+)')


def apply_flags(name, source):
    if '[nonl]' in name and source.endswith('\n'):
        source = source[:-1]
    if '[crlf]' in name:
        source = source.replace('\n', '\r\n')
    elif '[cr]' in name:
        source = source.replace('\n', '\r')
    return source


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
    return [(n, apply_flags(n, s)) for n, s in cases]


def has_coding(data):
    for line in data.split('\n')[:2]:
        if CODING.search(line):
            return True
    return False


def parse(data):
    """Returns ('ok',) or ('error', lineno, offset, msg)."""
    if data.startswith(codecs.BOM_UTF8):
        data = data[len(codecs.BOM_UTF8):]
    if has_coding(data):
        source = data
    else:
        try:
            source = data.decode('utf-8')
        except UnicodeDecodeError:
            return ('error', 0, 0, 'source is not valid UTF-8')
    try:
        compile(source, '<oracle>', 'exec')
    except SyntaxError, e:
        return ('error', e.lineno, e.offset, unicode(e.msg))
    except Exception, e:
        return ('error', 0, 0, 'internal error: %s' % e.__class__.__name__)
    except Throwable, e:
        return ('error', 0, 0, 'internal error: %s' % e.getClass().getName())
    return ('ok',)


def one_line(text):
    if isinstance(text, unicode):
        text = text.encode('utf-8')
    return text.replace('\\', '\\\\').replace('\r', '\\r').replace('\n', '\\n')


def write_record(out, name, result):
    out.write('### %s\n' % one_line(name))
    if result[0] == 'ok':
        out.write('ok\n')
    else:
        out.write('error %s %s\n%s\n' % (result[1], result[2], one_line(result[3])))


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
            data = read(path)
            if path.endswith('.cases'):
                for name, source in split_cases(data):
                    write_record(out, name, parse(source))
            else:
                write_record(out, path, parse(data))
    finally:
        out.close()


if __name__ == '__main__':
    main()
