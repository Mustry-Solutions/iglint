# Runs inside IA's Jython 2.7 (see run-oracle.sh). Records the token stream
# that Jython's lexer and indentation filter hand to its parser.
#
# usage: jython jython_token_oracle.py LIST_FILE OUTPUT_FILE
#
# Inputs are listed and split exactly like jython_ast_oracle.py does. The
# source gets the two newlines that compile() appends. For every source one
# record is written:
#
#   ### <name>
#   <TYPE>@<line>:<col> ...                (one line, all tokens)
#   error <line> <col> <message>           (only if lexing failed)
#
# Only tokens on the default channel are listed, which is what the parser
# sees. Columns count UTF-16 code units.
import codecs
import sys

from java.lang import Throwable
from org.python.antlr import FailFastHandler, ParseException
from org.python.antlr import PythonLexer, PythonParser, PythonTokenSource
from org.python.antlr.runtime import ANTLRStringStream, CommonTokenStream, Token

import jython_ast_oracle as cases

NAMES = list(PythonParser.tokenNames)


def message(e):
    try:
        return unicode(e.getMessage())
    except ValueError:
        # Messages may quote half of a surrogate pair.
        return u'<message with unpaired surrogate>'


def lex(source):
    lexer = PythonLexer(ANTLRStringStream(source + u'\n\n'))
    lexer.setErrorHandler(FailFastHandler())
    stream = CommonTokenStream(lexer)
    indented = PythonTokenSource(stream, '<oracle>', False)
    out = []
    error = None
    try:
        while True:
            t = indented.nextToken()
            if t.getChannel() == Token.DEFAULT_CHANNEL:
                ttype = t.getType()
                name = 'EOF' if ttype == Token.EOF else NAMES[ttype]
                out.append('%s@%d:%d' % (name, t.getLine(), t.getCharPositionInLine()))
            if t.getType() == Token.EOF:
                break
    except ParseException, e:
        error = 'error %d %d %s' % (e.line, e.charPositionInLine, message(e))
    except Throwable, e:
        error = 'error 0 0 internal error: %s' % e.getClass().getName()
    return out, error


def main():
    list_file, output_file = sys.argv[1], sys.argv[2]
    paths = [line.rstrip('\r\n') for line in open(list_file, 'rb')]
    out = open(output_file, 'wb')
    try:
        for path in paths:
            if not path:
                continue
            data = cases.read(path)
            if path.endswith('.cases'):
                items = cases.split_cases(data)
            else:
                items = [(path, data)]
            for name, data in items:
                if data.startswith(codecs.BOM_UTF8):
                    data = data[len(codecs.BOM_UTF8):]
                tokens, error = lex(data.decode('utf-8', 'replace'))
                out.write('### %s\n' % cases.one_line(name))
                out.write(' '.join(tokens) + '\n')
                if error is not None:
                    out.write(cases.one_line(error) + '\n')
    finally:
        out.close()


if __name__ == '__main__':
    main()
