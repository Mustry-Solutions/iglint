# Runs inside IA's Jython 2.7 (see run-oracle.sh --reads). Runs small
# sources with a module namespace that records every lookup and writes the
# names each source's code looks up in its module's globals while it runs:
# the names it reads itself (module and class bodies by name, functions
# for names they do not bind) and the ones the interpreter reads without
# the code naming them (`__name__` when a class body starts or a function
# is created, `__metaclass__` after a class body without bases, the names
# an import reads to find its package, `__debug__` and `AssertionError` for
# `assert`).
#
# usage: jython jython_implicit_reads_oracle.py LIST_FILE OUTPUT_FILE
#
# LIST_FILE holds one path per line, each a ".cases" file: many small
# sources, each introduced by a line "### <name>". Each source runs once,
# with `exec` in a namespace that holds `__name__` (`'oracle_mod'`) and the
# builtins; every exception is caught.
#
# OUTPUT_FILE gets one record per case, in input order:
#
#   ### <name>
#   reads <names>
#
# <names> is the sorted set of names looked up in the namespace, without
# `__builtins__` (read when `exec` sets up the frame). A source that does
# not compile is recorded as `compile-error`; one that raised adds
# `!<exception type>`.
import sys

sys.path.insert(0, '/oracle')
import jython_compile_oracle as base


class Recording(dict):
    """A module namespace that records the names looked up in it."""

    def __init__(self, *args):
        dict.__init__(self, *args)
        self.reads = set()

    def __getitem__(self, key):
        self.reads.add(key)
        return dict.__getitem__(self, key)

    def get(self, key, default=None):
        self.reads.add(key)
        return dict.get(self, key, default)

    def __contains__(self, key):
        self.reads.add(key)
        return dict.__contains__(self, key)

    def has_key(self, key):
        self.reads.add(key)
        return dict.has_key(self, key)


def run(source):
    try:
        code = compile(source, '<oracle>', 'exec')
    except SyntaxError:
        return 'compile-error'
    namespace = Recording({'__name__': 'oracle_mod', '__builtins__': __builtins__})
    namespace.reads = set()
    raised = ''
    try:
        exec code in namespace
    except BaseException, e:
        raised = ' !%s' % e.__class__.__name__
    names = sorted(n for n in namespace.reads if n != '__builtins__')
    return 'reads %s%s' % (' '.join(names), raised)


def main():
    list_file, output_file = sys.argv[1], sys.argv[2]
    paths = [line.rstrip('\r\n') for line in open(list_file, 'rb')]
    out = open(output_file, 'wb')
    try:
        for path in paths:
            if not path:
                continue
            for name, source in base.split_cases(base.read(path)):
                out.write('### %s\n%s\n' % (base.one_line(name), run(source.decode('utf-8'))))
    finally:
        out.close()


if __name__ == '__main__':
    main()
