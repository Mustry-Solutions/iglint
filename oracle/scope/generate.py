#!/usr/bin/env python3
"""Random programs for the scope oracle (oracle/scope/run.sh).

    generate.py SEED COUNT OUT          COUNT random programs as a .cases file
    generate.py stratify CASES EXPECTED PER_FEATURE OUT_CASES OUT_EXPECTED
                                        a sample that keeps, for every
                                        feature tag, up to PER_FEATURE cases

Each program mixes the constructs whose name resolution differs between
scopes: class bodies (at module level, in functions, in classes), private
names (`__x`) as names, attributes, import aliases, `def`/`class` names,
`global` names, comprehension, generator and lambda variables, `except`
and `with` targets, `for` targets, `del`, augmented assignment and tuple
unpacking; `global` in classes and nested functions; `exec` and
`import *`; names read before they are bound. Case names carry feature
tags (`[class]`, `[mangle]`, ...) for stratified sampling. Everything comes
from the seed; no project data is involved.
"""
import random
import sys

PRIVATE = ['__x', '__y', '__m', '__p', '___w', '__x_']
MANGLED = ['_C__x', '_D__y', '_C__m', '_E__x', '_F__m', '_D__m']
PLAIN = ['a', 'b', 'x', 'y', 'm', 'f', 'g', 'C', 'D', 'len', 'object', '__name__', '__doc__',
         '__module__', '__metaclass__', '__dunder__', '__']
CLASS_NAMES = ['C', 'D', '_E', '__F', '_C', 'D_']
FUNC_NAMES = ['f', 'g', 'h', '__f', '_C__f', 'm']
MODULES = ['math', 'os', 'os.path', 'java.lang']
ATTRS = ['a', 'b', '__x', '__y', '__m', '_C__x', '__z__', '__']


class Gen:
    def __init__(self, rnd):
        self.r = rnd
        self.tags = set()

    def chance(self, p):
        return self.r.random() < p

    def name(self):
        r = self.r.random()
        if r < 0.35:
            self.tags.add('private')
            return self.r.choice(PRIVATE)
        if r < 0.5:
            self.tags.add('mangled-spelling')
            return self.r.choice(MANGLED)
        return self.r.choice(PLAIN[:12] if self.chance(0.8) else PLAIN)

    def attr(self):
        a = self.r.choice(ATTRS)
        if a.startswith('__') and not a.endswith('__'):
            self.tags.add('private-attr')
        return a

    # ---- expressions ----

    def expr(self, depth=0):
        r = self.r.random()
        if depth > 2 or r < 0.35:
            return self.name() if self.chance(0.85) else str(self.r.randint(0, 9))
        if r < 0.5:
            return '%s.%s' % (self.primary(depth + 1), self.attr())
        if r < 0.6:
            args = [self.expr(depth + 1) for _ in range(self.r.randint(0, 2))]
            if self.chance(0.2):
                args.append('k=%s' % self.expr(depth + 1))
            return '%s(%s)' % (self.expr(depth + 1), ', '.join(args))
        if r < 0.67:
            return '(%s + %s)' % (self.expr(depth + 1), self.expr(depth + 1))
        if r < 0.75:
            self.tags.add('lambda')
            params = self.params(lambda_=True)
            return '(lambda %s: %s)' % (params, self.expr(depth + 1))
        if r < 0.83:
            self.tags.add('listcomp')
            return '[%s for %s in %s%s]' % (self.expr(depth + 1), self.target(), self.expr(depth + 1),
                                           ' if %s' % self.expr(depth + 1) if self.chance(0.3) else '')
        if r < 0.9:
            self.tags.add('genexp')
            return '(%s for %s in %s%s)' % (self.expr(depth + 1), self.target(), self.expr(depth + 1),
                                           ' for %s in %s' % (self.target(), self.expr(depth + 1))
                                           if self.chance(0.3) else '')
        if r < 0.95:
            self.tags.add('setcomp')
            return '{%s for %s in %s}' % (self.expr(depth + 1), self.target(), self.expr(depth + 1))
        self.tags.add('dictcomp')
        return '{%s: %s for %s in %s}' % (self.expr(depth + 1), self.expr(depth + 1), self.target(),
                                          self.expr(depth + 1))

    def primary(self, depth):
        e = self.expr(depth)
        return '(%s)' % e if e.isdigit() else e

    def target_base(self):
        """The object of an attribute or item target: a name or an attribute
        chain. A list comprehension there makes Jython reuse its assignment
        temporary for the comprehension's own, which the bytecode verifier
        rejects depending on unrelated code (`java.lang.VerifyError`); that
        code generator bug is not name resolution, so it is left out."""
        e = self.name()
        while self.chance(0.3):
            e = '%s.%s' % (e, self.attr())
        return e

    def target(self):
        if self.chance(0.15):
            self.tags.add('unpack')
            return '(%s, %s)' % (self.name(), self.name())
        return self.name()

    def store_target(self):
        r = self.r.random()
        if r < 0.6:
            return self.name()
        if r < 0.75:
            self.tags.add('unpack')
            return '%s, (%s, %s)' % (self.name(), self.name(), self.name())
        if r < 0.9:
            return '%s.%s' % (self.target_base(), self.attr())
        return '%s[%s]' % (self.target_base(), self.expr(2))

    def params(self, lambda_=False):
        names = []
        for _ in range(self.r.randint(0, 3)):
            n = self.r.choice(PRIVATE + ['a', 'b', 'self', 'x'])
            if n not in names:
                names.append(n)
        out = []
        for i, n in enumerate(names):
            if i == len(names) - 1 and self.chance(0.3):
                out.append('%s=%s' % (n, self.expr(2)))
                self.tags.add('default')
            else:
                out.append(n)
        if self.chance(0.1) and not out:
            out.append('*args')
        return ', '.join(out)

    # ---- statements ----

    def block(self, ctx, depth, indent):
        n = self.r.randint(1, 4 if depth < 2 else 2)
        lines = []
        for _ in range(n):
            lines.extend(self.stmt(ctx, depth, indent))
        return lines

    def stmt(self, ctx, depth, indent):
        pad = '    ' * indent
        r = self.r.random()
        nested = depth < 3
        if nested and r < 0.12:
            return self.class_def(ctx, depth, indent)
        if nested and r < 0.22:
            return self.func_def(ctx, depth, indent)
        if r < 0.32:
            return [pad + '%s = %s' % (self.store_target(), self.expr())]
        if r < 0.38:
            self.tags.add('augassign')
            t = self.name() if self.chance(0.7) else '%s.%s' % (self.target_base(), self.attr())
            return [pad + '%s += %s' % (t, self.expr())]
        if r < 0.43:
            self.tags.add('del')
            t = self.name() if self.chance(0.7) else '%s.%s' % (self.target_base(), self.attr())
            return [pad + 'del %s' % t]
        if r < 0.51:
            return [pad + self.import_stmt(ctx)]
        if r < 0.56:
            self.tags.add('global-' + ctx)
            names = sorted(set(self.name() for _ in range(self.r.randint(1, 2))))
            return [pad + 'global %s' % ', '.join(names)]
        if nested and r < 0.6:
            self.tags.add('for')
            return [pad + 'for %s in %s:' % (self.target(), self.expr())] + self.block(ctx, depth + 1, indent + 1)
        if nested and r < 0.64:
            self.tags.add('with')
            return [pad + 'with %s as %s:' % (self.expr(), self.target())] + self.block(ctx, depth + 1,
                                                                                        indent + 1)
        if nested and r < 0.68:
            self.tags.add('except')
            return ([pad + 'try:'] + self.block(ctx, depth + 1, indent + 1)
                    + [pad + 'except %s, %s:' % (self.expr(2), self.name())]
                    + self.block(ctx, depth + 1, indent + 1))
        if nested and r < 0.71:
            lines = [pad + 'if %s:' % self.expr()] + self.block(ctx, depth + 1, indent + 1)
            if self.chance(0.4):
                lines += [pad + 'else:'] + self.block(ctx, depth + 1, indent + 1)
            return lines
        if nested and self.chance(0.08):
            self.tags.add('finally')
            return ([pad + 'try:'] + self.block(ctx, depth + 1, indent + 1)
                    + [pad + 'finally:'] + self.block(ctx, depth + 1, indent + 1))
        if nested and self.chance(0.06):
            self.tags.add('while')
            body = self.block(ctx, depth + 1, indent + 1)
            if self.chance(0.5):
                body.append(pad + '    ' + self.r.choice(['break', 'continue']))
            return [pad + 'while %s:' % self.r.choice(['1', 'True', self.expr()])] + body
        if self.chance(0.03):
            return [pad + 'pass']
        if r < 0.74:
            self.tags.add('exec-' + ctx)
            if self.chance(0.5):
                return [pad + "exec 'pass'"]
            return [pad + "exec 'pass' in %s" % self.expr(2)]
        if r < 0.77:
            self.tags.add('assert')
            return [pad + 'assert %s, %s' % (self.expr(), self.expr())]
        if r < 0.8:
            return [pad + 'print %s' % self.expr()]
        if ctx == 'function' and r < 0.86:
            if self.chance(0.2):
                self.tags.add('yield')
                return [pad + 'yield %s' % self.expr()]
            return [pad + 'return %s' % self.expr()]
        if r < 0.9:
            return [pad + 'raise %s' % self.expr()]
        return [pad + self.expr()]

    def import_stmt(self, ctx):
        self.tags.add('import')
        r = self.r.random()
        module = self.r.choice(MODULES)
        if r < 0.3:
            return 'import %s' % module
        if r < 0.6:
            self.tags.add('import-as')
            return 'import %s as %s' % (module, self.name())
        if r < 0.9 or (ctx == 'function' and self.chance(0.7)):
            return 'from %s import %s as %s' % (module, self.r.choice(['pi', 'sep', 'floor']), self.name())
        self.tags.add('star-' + ctx)
        return 'from %s import *' % module

    def class_def(self, ctx, depth, indent):
        self.tags.add('class-in-' + ctx)
        pad = '    ' * indent
        name = self.r.choice(CLASS_NAMES)
        if name.startswith('__'):
            self.tags.add('private-class-name')
        bases = ''
        if self.chance(0.6):
            bases = '(%s)' % ', '.join(self.expr(2) for _ in range(self.r.randint(1, 2)))
        lines = []
        if self.chance(0.1):
            lines.append(pad + '@%s' % self.name())
        lines.append(pad + 'class %s%s:' % (name, bases))
        if self.chance(0.15):
            lines.append(pad + '    """doc"""')
        lines.extend(self.block('class', depth + 1, indent + 1))
        return lines

    def func_def(self, ctx, depth, indent):
        self.tags.add('def-in-' + ctx)
        pad = '    ' * indent
        name = self.r.choice(FUNC_NAMES)
        lines = []
        if self.chance(0.1):
            lines.append(pad + '@%s' % self.name())
        lines.append(pad + 'def %s(%s):' % (name, self.params()))
        lines.extend(self.block('function', depth + 1, indent + 1))
        return lines


def program(rnd):
    g = Gen(rnd)
    lines = []
    for _ in range(rnd.randint(1, 5)):
        lines.extend(g.stmt('module', 0, 0))
    return sorted(g.tags), '\n'.join(lines) + '\n'


def generate(seed, count, out):
    rnd = random.Random(seed)
    with open(out, 'w', encoding='utf-8') as f:
        for i in range(count):
            tags, source = program(rnd)
            f.write('### s%d-%d %s\n' % (seed, i, ''.join('[%s]' % t for t in tags)))
            f.write(source)


def read_records(path, header):
    """Splits a .cases or .expected file into (name, lines) records."""
    records = []
    with open(path, encoding='utf-8') as f:
        for line in f:
            if line.startswith(header):
                records.append((line[len(header):].rstrip('\n'), []))
            elif records:
                records[-1][1].append(line)
    return records


def stratify(cases, expected, per_feature, out_cases, out_expected):
    sources = read_records(cases, '### ')
    results = dict(read_records(expected, '### '))
    counts = {}
    keep = []
    for name, lines in sources:
        if name not in results:
            continue
        tags = [t.rstrip(']') for t in name.split(' ', 1)[-1].split('[') if t]
        ok = results[name][:1] == ['ok\n']
        tags.append('ok' if ok else 'error')
        if any(counts.get(t, 0) < per_feature for t in tags):
            for t in tags:
                counts[t] = counts.get(t, 0) + 1
            keep.append((name, lines))
    with open(out_cases, 'w', encoding='utf-8') as f:
        for name, lines in keep:
            f.write('### %s\n' % name)
            f.writelines(lines)
    with open(out_expected, 'w', encoding='utf-8') as f:
        for name, _ in keep:
            f.write('### %s\n' % name)
            f.writelines(results[name])
    print('kept %d of %d cases' % (len(keep), len(sources)))


def main():
    if sys.argv[1] == 'stratify':
        stratify(sys.argv[2], sys.argv[3], int(sys.argv[4]), sys.argv[5], sys.argv[6])
    else:
        generate(int(sys.argv[1]), int(sys.argv[2]), sys.argv[3])


if __name__ == '__main__':
    main()
