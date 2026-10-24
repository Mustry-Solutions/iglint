# Runs inside IA's Jython 2.7 (see run-oracle.sh --method-size). For each
# input, generates the Java class of the module with Jython's own code
# generator (org.python.core.imp.compileSource, the generator compile()
# uses) and records the bytecode length of every Java method that holds a
# code unit (the module, then each function and class body in the order
# Jython numbers them; lambdas and generator expressions, whose methods
# are named f, are left out, so inputs must not name a function f), then
# whether
# full compile() of the unicode source succeeds, the way Ignition compiles
# scripts.
#
# usage: jython jython_method_size_oracle.py LIST_FILE OUTPUT_FILE
#
# LIST_FILE holds one path per line; a ".cases" file holds many sources,
# each introduced by "### <name>". OUTPUT_FILE gets, per source:
#
#   ### <name>
#   units <method> <bytes> <method> <bytes> ...   (or: units error)
#   ok | error <first line of the message>
import struct
import sys

import jarray
from java.io import ByteArrayInputStream
from java.lang import Throwable
from org.python.core import imp

# Methods of the generated class that are not code units.
SUPPORT = ('<init>', '<clinit>', 'getMain', 'main', 'getCodeBootstrap', 'call_function')


def method_sizes(data):
    """(name, code length) of every method of a class file."""
    p = 10
    n = struct.unpack('>H', data[8:10])[0]
    pool = [None] * n
    i = 1
    while i < n:
        tag = ord(data[p])
        p += 1
        if tag == 1:
            length = struct.unpack('>H', data[p:p + 2])[0]
            pool[i] = data[p + 2:p + 2 + length]
            p += 2 + length
        elif tag in (3, 4):
            p += 4
        elif tag in (5, 6):
            p += 8
            i += 1
        elif tag in (7, 8, 16, 19, 20):
            p += 2
        elif tag in (9, 10, 11, 12, 17, 18):
            p += 4
        elif tag == 15:
            p += 3
        else:
            raise ValueError('constant pool tag %d' % tag)
        i += 1
    p += 6
    interfaces = struct.unpack('>H', data[p:p + 2])[0]
    p += 2 + 2 * interfaces
    out = []
    for members in ('fields', 'methods'):
        count = struct.unpack('>H', data[p:p + 2])[0]
        p += 2
        for _ in range(count):
            _, name, _, attributes = struct.unpack('>HHHH', data[p:p + 8])
            p += 8
            code = None
            for _ in range(attributes):
                attribute, length = struct.unpack('>HI', data[p:p + 6])
                if pool[attribute] == 'Code':
                    code = struct.unpack('>I', data[p + 10:p + 14])[0]
                p += 6 + length
            if members == 'methods':
                out.append((pool[name], code))
    return out


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


def first_line(text):
    text = '%s' % text
    return text.split('\n')[0][:200]


def record(out, name, data):
    out.write('### %s\n' % name)
    try:
        stream = ByteArrayInputStream(jarray.array(data, 'b'))
        generated = imp.compileSource('m', stream, 'm.py').tostring()
        units = [(m.rsplit('$', 1)[0], c) for m, c in method_sizes(generated) if m not in SUPPORT]
        # The module, then the def and class bodies: lambdas and generator
        # expressions (methods named f, so inputs must not name a function
        # f) are left out.
        units = units[:1] + [(m, c) for m, c in units[1:] if m != 'f']
        out.write('units %s\n' % ' '.join('%s %d' % u for u in units))
    except (Exception, Throwable):
        out.write('units error\n')
    try:
        compile(data.decode('utf-8'), '<script>', 'exec')
        out.write('ok\n')
    except (Exception, Throwable), e:
        out.write('error %s\n' % first_line(e).encode('utf-8'))


def main():
    list_file, output_file = sys.argv[1], sys.argv[2]
    out = open(output_file, 'wb')
    try:
        for path in [l.rstrip('\r\n') for l in open(list_file, 'rb')]:
            if not path:
                continue
            data = open(path, 'rb').read()
            if path.endswith('.cases'):
                for name, source in split_cases(data):
                    record(out, name, source)
            else:
                record(out, path, data)
    finally:
        out.close()


if __name__ == '__main__':
    main()
