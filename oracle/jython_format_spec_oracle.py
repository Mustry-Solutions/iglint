# Runs "format<TAB>argument" cases (oracle/format_spec_cases.py) on IA's
# Jython: format.format(argument), and records "ok" or the exception. A
# format prefixed with `u` is a unicode format string, one prefixed with
# `b` a byte string of its Latin-1 encoding (lines are UTF-8); others are
# ASCII byte strings.
#
# usage (inside the image, see oracle/format-spec.sh): jython THIS IN OUT
import sys


def main():
    out = open(sys.argv[2], 'w')
    for line in open(sys.argv[1]):
        column, arg = line.rstrip('\n').split('\t')
        fmt = column
        if column[:1] == 'u':
            fmt = column[1:].decode('utf-8')
        elif column[:1] == 'b':
            fmt = column[1:].decode('utf-8').encode('latin-1')
        try:
            fmt.format(eval(arg))
            result = 'ok'
        except Exception, e:
            try:
                message = unicode(e)
            except Exception:
                message = repr(e.args)
            message = message.replace(u'\n', u' ').encode('ascii', 'backslashreplace')
            result = 'err %s: %s' % (e.__class__.__name__, message)
        out.write('%s\t%s\t%s\n' % (column, arg, result))
    out.close()


if __name__ == '__main__':
    main()
