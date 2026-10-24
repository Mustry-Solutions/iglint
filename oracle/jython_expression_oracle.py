# Evaluates one Python expression per line on IA's Jython, compiled from
# unicode source the way Ignition compiles scripts, with `x` bound to
# u'\xe9', and records "ok" or the exception class and message.
#
# usage (inside the image, see oracle/format-unicode.sh): jython THIS IN OUT
import codecs
import sys

from java.lang import Throwable


def main():
    out = codecs.open(sys.argv[2], 'w', 'utf-8')
    for line in codecs.open(sys.argv[1], 'r', 'utf-8'):
        source = line.rstrip(u'\n')
        try:
            eval(compile(source, '<script>', 'eval'), {'x': u'\xe9'})
            result = u'ok'
        except (Exception, Throwable), e:
            try:
                message = unicode(e)
            except Exception:
                message = repr(getattr(e, 'args', ''))
            message = message.replace(u'\n', u' ').encode('ascii', 'backslashreplace')
            result = u'err %s: %s' % (e.__class__.__name__, message.decode('ascii'))
        out.write(u'%s\t%s\n' % (source, result))
    out.close()


if __name__ == '__main__':
    main()
