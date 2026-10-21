# Runs Ignition's own parsers on generated inputs. Executed by Jython 2.7
# inside the inductiveautomation/ignition image (see run.sh).
#
#   ignition_oracle.py expr|props|tags CASES OUT
#
# CASES holds one hex-encoded UTF-8 input per line. OUT gets one result line
# per case:
#   expr   OK<TAB><tree>  or  ERR<TAB><line>:<column>  (ERR<TAB>? without position)
#   props  <PropertyRef.isParseable><TAB><PropertyRef or LegacyPropertyRef>
#   tags   OK<TAB><hex source><TAB><hex components, comma separated><TAB>P|-  or  ERR
import sys

from java.io import File, FileOutputStream, StringReader
from java.lang import Double, String as JString, StringBuilder
from java.util.regex import Pattern
from java.util.zip import ZipFile
from java.lang.reflect import Array
from java.lang import Byte

from com.inductiveautomation.ignition.common.expressions import (
    ConstantExpression, ExpressionParseContext, FunctionFactory)
from com.inductiveautomation.ignition.common.expressions.functions import Function
from com.inductiveautomation.ignition.common.expressions.parsing import ELParser
from com.inductiveautomation.ignition.common.tags.paths.parser import TagPathParser

PERSPECTIVE = '/usr/local/bin/ignition/user-lib/modules/Perspective-module.modl'
POSITION = Pattern.compile(r'\(Line (\d+) , Char (\d+)\)')
MARK = u'\x01'


def unhex(line):
    data = Array.newInstance(Byte.TYPE, len(line) // 2)
    for i in range(0, len(line), 2):
        v = int(line[i:i + 2], 16)
        Array.setByte(data, i // 2, v - 256 if v > 127 else v)
    return JString(data, 'UTF-8')


def hexs(s):
    data = JString(s).getBytes('UTF-8')
    return ''.join('%02x' % (b & 0xff) for b in data)


class DummyFunction(Function):
    def initArgs(self, args):
        pass


class Factory(FunctionFactory):
    def getFunction(self, name):
        return DummyFunction()


class Context(ExpressionParseContext):
    """Marks references so the dump can tell them from string constants."""

    def __init__(self):
        self.factory = Factory()

    def createBoundExpression(self, path):
        sb = StringBuilder()
        sb.append(MARK)
        sb.append(path)
        return ConstantExpression(sb.toString())

    def getFunctionFactory(self):
        return self.factory


def dump(e):
    cls = e.getClass().getSimpleName()
    if cls == 'ConstantExpression':
        v = e.execute().getValue()
        if v is None:
            return 'Null'
        if isinstance(v, bool):
            return 'Bool(%s)' % ('true' if v else 'false')
        if isinstance(v, (int, long)):
            return 'Long(%d)' % v
        if isinstance(v, float):
            return 'Double(%016x)' % (Double.doubleToRawLongBits(v) & 0xffffffffffffffff)
        if isinstance(v, basestring):
            if v.startswith(MARK):
                return 'Bound(%s)' % hexs(v[1:])
            return 'Str(%s)' % hexs(v)
        return 'Unknown'
    kids = e.getChildren() or []
    if cls == 'FunctionExpression':
        return 'Call(%s;%s)' % (hexs(e.getFuncName()), ','.join(dump(k) for k in kids))
    return '%s[%s](%s)' % (cls, e.getOpName(), ','.join(dump(k) for k in kids))


def run_expr(s, context):
    try:
        return 'OK\t' + dump(ELParser(StringReader(s), context).parse().value)
    except:
        sb = StringBuilder()
        sb.append(sys.exc_info()[1])
        m = POSITION.matcher(sb)
        if m.find():
            return 'ERR\t%s:%s' % (m.group(1), m.group(2))
        return 'ERR\t?'


def load_perspective():
    """Puts perspective-common from the Perspective module on sys.path."""
    modl = ZipFile(PERSPECTIVE)
    for entry in list(modl.entries()):
        name = entry.getName()
        if name.endswith('.jar') and 'perspective-common' in name:
            target = File.createTempFile('perspective-common', '.jar')
            stream = modl.getInputStream(entry)
            out = FileOutputStream(target)
            buf = Array.newInstance(Byte.TYPE, 65536)
            while True:
                n = stream.read(buf)
                if n < 0:
                    break
                out.write(buf, 0, n)
            out.close()
            sys.path.append(target.getAbsolutePath())
            return
    raise Exception('perspective-common jar not found in ' + PERSPECTIVE)


def run_tags(s):
    try:
        tp = TagPathParser.parse(s)
    except:
        return 'ERR'
    comps = [hexs(tp.getPathComponent(i)) for i in range(tp.getPathLength())]
    source = tp.getSource() or ''
    return 'OK\t%s\t%s\t%s' % (hexs(source), ','.join(comps), '-' if tp.getProperty() is None else 'P')


def main():
    mode, cases_path, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
    lines = [l for l in open(cases_path).read().split('\n') if l]
    results = []
    if mode == 'expr':
        context = Context()
        for line in lines:
            results.append(run_expr(unhex(line), context))
    elif mode == 'props':
        load_perspective()
        from com.inductiveautomation.perspective.common.api import LegacyPropertyRef, PropertyRef
        for line in lines:
            s = unhex(line)
            modern = PropertyRef.isParseable(s)
            any_ = modern or LegacyPropertyRef.isParseable(s)
            results.append('%s\t%s' % ('true' if modern else 'false', 'true' if any_ else 'false'))
    elif mode == 'tags':
        for line in lines:
            results.append(run_tags(unhex(line)))
    else:
        raise Exception('unknown mode ' + mode)
    out = open(out_path, 'w')
    out.write('\n'.join(results) + '\n')
    out.close()


main()
