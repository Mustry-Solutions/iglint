# Runs inside IA's Jython 2.7 (see run-oracle.sh --warnings). Compiles many
# inputs in one JVM, the way Ignition compiles scripts, and records the
# SyntaxWarnings Jython's compiler issues for each.
#
# usage: jython jython_warning_oracle.py LIST_FILE OUTPUT_FILE
#
# LIST_FILE and the case files are as for jython_compile_oracle.py.
# OUTPUT_FILE gets one record per source, in input order:
#
#   ### <name>
#   warning <lineno> <message>     (one line per warning, in order)
#   ok                             (or `error`, when compile() fails)
import sys
import warnings

from jython_compile_oracle import has_coding, one_line, read, split_cases

import codecs


def compile_warnings(data):
    if data.startswith(codecs.BOM_UTF8):
        data = data[len(codecs.BOM_UTF8):]
    source = data if has_coding(data) else data.decode('utf-8')
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter('always')
        try:
            compile(source, '<oracle>', 'exec')
            status = 'ok'
        except Exception:
            status = 'error'
        except:
            status = 'error'
    found = [(w.lineno, unicode(w.message)) for w in caught
             if issubclass(w.category, SyntaxWarning)]
    return found, status


def main():
    list_file, output_file = sys.argv[1], sys.argv[2]
    paths = [line.rstrip('\r\n') for line in open(list_file, 'rb')]
    out = open(output_file, 'wb')
    try:
        for path in paths:
            if not path:
                continue
            data = read(path)
            sources = split_cases(data) if path.endswith('.cases') else [(path, data)]
            for name, source in sources:
                try:
                    found, status = compile_warnings(source)
                except UnicodeDecodeError:
                    found, status = [], 'error'
                out.write('### %s\n' % one_line(name))
                for lineno, message in found:
                    out.write('warning %s %s\n' % (lineno, one_line(message)))
                out.write('%s\n' % status)
    finally:
        out.close()


if __name__ == '__main__':
    main()
