#!/usr/bin/env python3
"""Cross product of control flow and compile errors for the compile oracle.

    compile_cross.py all OUT                    every case, as a .cases file
    compile_cross.py stratify CASES EXPECTED PER OUT_CASES OUT_EXPECTED
                                                up to PER cases per prefix,
                                                construct and context, and
                                                per outcome

Each case puts a statement that ends its block (or a mix of statements
whose code Jython generates but cannot reach, or a `yield` that makes the
function a generator) before a construct that Jython's compiler, code
generator or bytecode verifier rejects, in one of several nesting
contexts. Whether the error happens depends on whether Jython generates
the construct's code (it stops after `return`, `raise`, `continue` and an
`if`/`else` that ends both ways) and whether the bytecode can reach it
(the verifier ignores unreachable code, but a generator resumes after
every `yield`). Case names are `<context>/<prefix>/<construct>`. Run the
cases with `oracle/run-oracle.sh --compile`.
"""
import sys

# Statements that end a block, make what follows unreachable, or neither.
PREFIXES = {
    'none': [],
    'pass': ['pass'],
    'return': ['return'],
    'raise': ['raise X'],
    'continue': ['continue'],
    'break': ['break'],
    'if-else-return': ['if x:', '    return', 'else:', '    return'],
    'if-else-raise': ['if x:', '    raise X', 'else:', '    raise Y'],
    'if-return': ['if x:', '    return'],
    'try-return-finally': ['try:', '    return', 'finally:', '    pass'],
    'try-pass-finally-return': ['try:', '    pass', 'finally:', '    return'],
    'try-return-except-return': ['try:', '    return', 'except:', '    return'],
    'try-return-except-pass': ['try:', '    return', 'except:', '    pass'],
    'try-raise-except-raise': ['try:', '    raise X', 'except E:', '    raise Y'],
    'try-global-except': ['try:', '    global g', 'except E:', '    pass'],
    'while-1': ['while 1:', '    pass'],
    'while-true-break': ['while True:', '    break'],
    'for-else-return': ['for i in y:', '    return', 'else:', '    return'],
    'for-return': ['for i in y:', '    return'],
    'with-return': ['with c:', '    return'],
    'yield': ['yield 1'],
    'yield-return': ['yield 1', 'return'],
    'yield-try-return-finally': ['yield 1', 'try:', '    return', 'finally:', '    pass'],
}

# Constructs that fail to compile somewhere.
CONSTRUCTS = {
    'listcomp-yield-iter': ['[x for x in (yield [])]'],
    'listcomp-yield-elt': ['[(yield x) for x in y]'],
    'listcomp-yield-if': ['[x for x in y if (yield x)]'],
    'setcomp-yield-iter': ['{x for x in (yield [])}'],
    'dictcomp-yield-iter': ['{x: 1 for x in (yield [])}'],
    'genexp-yield-iter': ['list(x for x in (yield []))'],
    'invalid-target-unary': ['-a = 1'],
    'invalid-target-call-for': ['for f() in x:', '    pass'],
    'invalid-target-with': ['with c as f():', '    pass'],
    'invalid-target-listcomp': ['[1 for f() in y]'],
    'return-value': ['return 1'],
    'yield-stmt': ['yield 1'],
    'break': ['break'],
    'continue': ['continue'],
    'default-except-not-last': ['try:', '    pass', 'except:', '    pass', 'except E:', '    pass'],
    'import-star': ['from os import *'],
    'del-cell': ['v = 1', 'def inner():', '    return v', 'del v'],
    'exec-with-free-nested': ['exec "x"', 'def inner():', '    return v', 'v = 1'],
    # A `yield` in an exception type: NegativeArraySizeException (review
    # round 20, Opus). Not in a lambda or in the clause's target.
    'except-yield-type': ['try:', '    pass', 'except (yield):', '    pass'],
    'except-yield-type-name': ['try:', '    pass', 'except (yield), e:', '    pass'],
    'except-yield-tuple': ['try:', '    pass', 'except (E, (yield)):', '    pass'],
    'except-yield-call': ['try:', '    pass', 'except E((yield)):', '    pass'],
    'except-yield-lambda': ['try:', '    pass', 'except (lambda: (yield))():', '    pass'],
    'except-yield-target': ['try:', '    pass', 'except E, a[(yield)]:', '    pass'],
    # A `yield` in an attribute or subscript target of an unpacking:
    # IllegalArgumentException (review round 20, Opus). Single targets,
    # chains of them, augmented assignments and `del` compile.
    'unpack-yield-subscript': ['b, a[(yield)] = 1, 2'],
    'unpack-yield-attribute': ['b, (yield).x = 1, 2'],
    'unpack-yield-slice': ['b, a[(yield):2] = 1, 2'],
    'unpack-yield-nested': ['(b, a[(yield)]), c = (1, 2), 3'],
    'unpack-yield-list': ['[b, a[(yield)]] = 1, 2'],
    'unpack-yield-chained': ['c = b, a[(yield)] = 1, 2'],
    'unpack-yield-for': ['for b, a[(yield)] in s:', '    pass'],
    'unpack-yield-with': ['with c as (b, a[(yield)]):', '    pass'],
    'unpack-yield-except': ['try:', '    pass', 'except E, (b, a[(yield)]):', '    pass'],
    'unpack-yield-genexp-target': ['x = list(1 for b, a[(yield)] in s)'],
    'unpack-yield-single': ['a[(yield)] = 1'],
    'unpack-yield-single-chained': ['a[(yield)] = b = 1'],
    'unpack-yield-for-single': ['for a[(yield)] in s:', '    pass'],
    'unpack-yield-aug': ['a[(yield)] += 1'],
    'unpack-yield-del': ['del b, a[(yield)]'],
    'unpack-yield-lambda': ['b, a[lambda: (yield)] = 1, 2'],
    'unpack-yield-value': ['b, a[0] = (yield), 2'],
}

# Where the prefix and the construct go: `{}` is the indented block.
CONTEXTS = {
    'module': ('', 0),
    'function': ('def f():\n', 1),
    'nested-function': ('def outer():\n    def f():\n', 2),
    'method': ('class C:\n    def f(self):\n', 2),
    'class': ('class C:\n', 1),
    'for-in-function': ('def f():\n    for i in z:\n', 2),
    'while-in-function': ('def f():\n    while z:\n', 2),
    'try-in-function': ('def f():\n    try:\n', 2),
    'except-in-function': ('def f():\n    try:\n        pass\n    except:\n', 2),
    'finally-in-function': ('def f():\n    try:\n        pass\n    finally:\n', 2),
    'with-in-function': ('def f():\n    with c:\n', 2),
    'if-in-function': ('def f():\n    if z:\n', 2),
    'for-in-module': ('for i in z:\n', 1),
    'generator': ('def f():\n    yield 0\n', 1),
}


def cases():
    for ctx, (head, depth) in CONTEXTS.items():
        pad = '    ' * depth
        # A try block needs its handler after the body.
        tail = ''
        if ctx == 'try-in-function':
            tail = '    except:\n        pass\n'
        for pname, prefix in PREFIXES.items():
            for cname, construct in CONSTRUCTS.items():
                body = ''.join(pad + line + '\n' for line in prefix + construct)
                yield '%s/%s/%s' % (ctx, pname, cname), head + body + tail


def write_all(out):
    with open(out, 'w', encoding='utf-8') as f:
        for name, source in cases():
            f.write('### %s\n%s' % (name, source))


def records(path):
    out = []
    with open(path, encoding='utf-8') as f:
        for line in f:
            if line.startswith('### '):
                out.append((line[4:].rstrip('\n'), []))
            elif out:
                out[-1][1].append(line)
    return out


def stratify(cases_path, expected_path, per, out_cases, out_expected):
    sources = records(cases_path)
    results = dict(records(expected_path))
    counts = {}
    keep = []
    for name, lines in sources:
        result = results[name]
        outcome = 'ok' if result[0] == 'ok\n' else 'error'
        keys = name.split('/') + [outcome, name.split('/')[2] + ':' + outcome]
        if any(counts.get(k, 0) < per for k in keys):
            for k in keys:
                counts[k] = counts.get(k, 0) + 1
            keep.append(name)
    by_name = dict(sources)
    with open(out_cases, 'w', encoding='utf-8') as f:
        for name in keep:
            f.write('### %s\n' % name)
            f.writelines(by_name[name])
    with open(out_expected, 'w', encoding='utf-8') as f:
        for name in keep:
            f.write('### %s\n' % name)
            f.writelines(results[name])
    print('kept %d of %d cases' % (len(keep), len(sources)))


if __name__ == '__main__':
    if sys.argv[1] == 'all':
        write_all(sys.argv[2])
    else:
        stratify(sys.argv[2], sys.argv[3], int(sys.argv[4]), sys.argv[5], sys.argv[6])
