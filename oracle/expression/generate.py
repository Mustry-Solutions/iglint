#!/usr/bin/env python3
"""Random inputs for the expression oracle, and merging of oracle output.

    generate.py expr|props|tags SEED COUNT OUT   one hex-encoded UTF-8 case per line
    generate.py merge CASES RESULTS OUT         "<hex>\t<result>" expectation lines

Everything is generated from the seed; no project data is involved.
"""
import random
import sys

BIN = ['+', '-', '*', '/', '%', '^', '=', '!=', '<', '<=', '>', '>=', '&&', '||', '&', '|', 'xor', 'XOR', 'like',
       'Like', '<<', '>>']
UN = ['-', '!', '~']
LIT = ['0', '1', '42', '007', '00', '0x1F', '0XfF', '1.5', '.5', '5.', '1e3', '1E-3', '1.e5', '2.5e+2',
       '9223372036854775807', '9223372036854775808', '0x7FFFFFFFFFFFFFFF', '0x8000000000000000', 'true', 'FALSE',
       'null', 'None', 'NONE', 'True', '"abc"', "'abc'", '""', "''", '"a\\tb"', '"a\\Tb"', '"a\\Nb"', '"q\\"q"',
       "'q\\'q'", '"b\\\\s"', '"\\r"', '"\u00e9\u20ac\U0001F600"', "'it''s'", '"x\\\'y"', "'x\\\"y'", '"\\u0041"',
       '"a//b"', '"{x}"']
REFS = ['{this.props.text}', '{view.params.x}', '{[default]A/B}', '{../X.props.y}', '{ a }', '{}', '{value}',
        '{a//b}', '{x[0]}', '{\u00e9}', '{a"b}', "{a'b}"]
NAMES = ['if', 'IF', 'len', 'toStr', 'a1', '_x', '$f', '\u00e9t\u00e9', 'f']
SOUP = BIN + UN + LIT + REFS + NAMES + ['(', ')', ',', '[', ']', '{', '}', ' ', '\n', '\t', '\r', '\f', '\v',
                                        '//c\n', '//', '.', '#', '?', ':', ';', '@', '`', '"', "'", '\\', 'abc',
                                        'x', '\u00a0', '\U0001F600', '==', '<>', 'and', 'not']

PROP_PIECES = ['this', 'view', 'parent', 'session', 'page', 'THIS', 'thi', '.', '.', '.', 'props', 'custom',
               'params', 'meta', 'position', 'propsx', 'x', 'Name_1', 'a-b', 'a b', '1a', '_', '[0]', '[12]', '[]',
               "['k']", '["k"]', '["k\']', "['a b']", '[x]', '/', '/', '../', '../', '.../', './', '..', 'root',
               'Label', ' ', '$', '\u00e9', '[', ']', '"']
TAG_PIECES = ['[default]', '[~]', '[.]', '[..]', '[]', '[System]', '[a b]', '[x/y]', 'Folder', 'Tag', 'Tag 1', '/',
              '/', '\\', '.', '.', 'Quality', 'value', '../', '..\\', '[0]', '[1,2]', '["k"]', "['k']", '[]',
              '[1, 2]', '\t', '\n', ' ', '*', '(', ')', "'", ':', '-', '_', '#', '@', ',', '!', '=', '~', '$',
              '\u00e9', '\u20ac', '{', '}', '"', '[', ']']
NAMES_FOR_PROPS = ['a', 'Label', 'x_1', 'a-b', 'a b', 'Tank']


def expression(rnd, depth):
    r = rnd.random()
    if depth > 4 or r < 0.3:
        return rnd.choice(LIT + REFS)
    if r < 0.55:
        return (expression(rnd, depth + 1) + rnd.choice([' ', '', '  ']) + rnd.choice(BIN)
                + rnd.choice([' ', '']) + expression(rnd, depth + 1))
    if r < 0.65:
        return rnd.choice(UN) + rnd.choice(['', ' ']) + expression(rnd, depth + 1)
    if r < 0.75:
        return '(' + expression(rnd, depth + 1) + ')'
    if r < 0.88:
        n = rnd.choice([0, 1, 2, 3])
        args = ', '.join(expression(rnd, depth + 1) for _ in range(n))
        return rnd.choice(NAMES) + rnd.choice(['', ' ']) + '(' + args + ')'
    if rnd.random() < 0.7:
        return expression(rnd, depth + 1) + '[' + expression(rnd, depth + 1) + ']'
    return expression(rnd, depth + 1) + '[' + expression(rnd, depth + 1) + ', ' + expression(rnd, depth + 1) + ']'


def soup(rnd, pieces, low, high):
    return ''.join(rnd.choice(pieces) for _ in range(rnd.randint(low, high)))


def mutate(rnd, s):
    if not s:
        return s
    i = rnd.randrange(len(s))
    op = rnd.random()
    if op < 0.4:
        return s[:i] + s[i + 1:]
    if op < 0.8:
        return s[:i] + rnd.choice(SOUP) + s[i:]
    return s[:i] + rnd.choice(SOUP) + s[i + 1:]


def expr_case(rnd):
    r = rnd.random()
    if r < 0.5:
        s = expression(rnd, 0)
    elif r < 0.8:
        s = mutate(rnd, expression(rnd, 0))
    else:
        s = soup(rnd, SOUP, 1, 7)
    if rnd.random() < 0.1:
        s += rnd.choice([' // trailing', '\n', ' "open', ' {open', '"', '{'])
    return s


def prop_case(rnd):
    if rnd.random() >= 0.5:
        return soup(rnd, PROP_PIECES, 1, 8)
    typ = rnd.choice(['props', 'custom', 'params', 'meta', 'position'])
    tail = ''.join(rnd.choice(['.' + rnd.choice(NAMES_FOR_PROPS), '[0]', "['k']", '[]', '.'])
                   for _ in range(rnd.randint(0, 3)))
    if rnd.random() < 0.3:
        scope = rnd.choice(['this', 'view', 'parent', 'session', 'page'])
        return scope + rnd.choice(['', '[0]']) + '.' + typ + tail
    comps = '/'.join(rnd.choice(NAMES_FOR_PROPS) for _ in range(rnd.randint(1, 3)))
    prefix = rnd.choice(['/', '../', '../../', './', '.../', '..../', './/', '..'])
    return prefix + comps + '.' + typ + rnd.choice(['', '.']) + tail


def tag_case(rnd):
    return soup(rnd, TAG_PIECES, 1, 8)


def main(argv):
    if argv[1] == 'merge':
        cases = [l for l in open(argv[2]).read().split('\n') if l]
        results = open(argv[3], encoding='utf-8').read().split('\n')[:len(cases)]
        assert len(results) == len(cases), (len(results), len(cases))
        with open(argv[4], 'w', encoding='utf-8', newline='\n') as out:
            for c, r in zip(cases, results):
                out.write(c + '\t' + r + '\n')
        return
    kind, seed, count, path = argv[1], int(argv[2]), int(argv[3]), argv[4]
    make = {'expr': expr_case, 'props': prop_case, 'tags': tag_case}[kind]
    rnd = random.Random(seed)
    lines = []
    while len(lines) < count:
        s = make(rnd)
        if s:
            lines.append(s.encode('utf-8').hex())
    with open(path, 'w', newline='\n') as out:
        out.write('\n'.join(lines) + '\n')


if __name__ == '__main__':
    main(sys.argv)
