# Runs "format<TAB>argument" cases (oracle/format_spec_cases.py) on IA's
# Jython: format.format(argument), and records "ok" or the exception.
#
# usage (inside the image, see oracle/format-spec.sh): jython THIS IN OUT
import sys


def main():
    out = open(sys.argv[2], 'w')
    for line in open(sys.argv[1]):
        fmt, arg = line.rstrip('\n').split('\t')
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
        out.write('%s\t%s\t%s\n' % (fmt, arg, result))
    out.close()


if __name__ == '__main__':
    main()
