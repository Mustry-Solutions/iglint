#!/usr/bin/env python3
"""String formatting cases that mix byte strings and unicode, for SCR112.

    format_unicode_cases.py all                   every case, one Python 2
                                                  expression per line
    format_unicode_cases.py sample SEED COUNT     a random sample
    format_unicode_cases.py pick RESULTS SEED COUNT
                                                  COUNT recorded results,
                                                  chosen at random (the
                                                  committed replay sample)

Each line is an expression: a `%` or `str.format` call on a literal format
string (byte string or unicode, with or without non-ASCII text, fill
characters and specs) with literal arguments (ASCII and non-ASCII byte
strings, written with escapes and as UTF-8 text, unicode strings up to
astral characters, numbers around 255/256, None, containers) and a name
(`x`, defined by the oracle as a unicode string) for values iglint cannot
know. `oracle/format-unicode.sh` records Jython's verdict on each.
"""
import itertools
import random
import sys

STRS = [r"'a'", r"'\xff'", r"'\x80b'", "'\u00e9'", r"''"]
UNIS = [r"u'a'", r"u'\xe9'", r"u'\u0100'", r"u'\U0001f980'", r"u''", "u'\u00e9'"]
OTHER = ['1', '255', '256', '300', '1114112', '1.5', 'None', '[u\'\\xe9\']', "['\\xff']", 'x']
VALUES = STRS + UNIS + OTHER

PERCENT_FORMATS = ["'%s'", "u'%s'", "'\\xe9%s'", "u'\\xe9%s'", "'%r'", "u'%r'", "'%c'",
                   "u'%c'", "'%5s'", "u'%-3.1s'", "'%d'", "u'%%%s'"]
PERCENT_PAIRS = ["'%s%s'", "u'%s%s'", "'%r%s'", "'%c%s'", "'%s%c'", "u'%c%c'", "'%*s'",
                 "u'%*s'", "'%s %d'"]


def percent_cases():
    for f, v in itertools.product(PERCENT_FORMATS, VALUES):
        yield '%s %% %s' % (f, v)
        yield '%s %% (%s,)' % (f, v)
    for f, (a, b) in itertools.product(PERCENT_PAIRS, itertools.product(VALUES, VALUES)):
        if f.endswith("*s'") and a not in ('1', '255', '256'):
            continue
        yield '%s %% (%s, %s)' % (f, a, b)
    for a, b in itertools.product(VALUES, VALUES):
        yield "'%%(a)s%%(b)s' %% {'a': %s, 'b': %s}" % (a, b)
        yield "u'%%(b)s%%(a)c' %% {'a': %s, 'b': %s}" % (a, b)


FILLS = ['', '*', '\\xe9', '\\u0100', '\\U0001f980', '\u00e9', '\u20ac']
SPECS = ['', ':{f}^5', ':{f}<3', ':{f}', ':5', ':s', ':d', ':4{f}']
FIELDS = ['{{0{c}{s}}}', '{{a{c}{s}}}', '{{0[0]{c}{s}}}', '{{}}{{{c}{s}}}']
CONVERSIONS = ['', '!r', '!s']


def format_cases():
    for prefix, field, conversion, spec, fill in itertools.product(
            ['', 'u'], FIELDS, CONVERSIONS, SPECS, FILLS):
        if '{f}' not in spec and fill:
            continue
        if prefix == '' and fill and not fill.startswith('\\x') and fill != '*':
            # Non-byte fills in a byte string are bytes: not this check's.
            continue
        text = field.format(c=conversion, s=spec.format(f=fill))
        for v in VALUES:
            if text.startswith('{a'):
                call = 'format(a=%s)' % v
            elif text.startswith('{}'):
                call = 'format(1, %s)' % v
            else:
                call = 'format(%s)' % v
            yield "%s'%s'.%s" % (prefix, text, call)
    for prefix in ['', 'u']:
        for v, w in itertools.product(VALUES, ['3', "u'\\u0100'", "'5'"]):
            yield "%s'{0:{1}}'.format(%s, %s)" % (prefix, v, w)
            yield "%s'{0:{1}\\u0100}'.format(%s, %s)" % (prefix, v, w)


def all_cases():
    seen = set()
    for c in itertools.chain(percent_cases(), format_cases()):
        if c not in seen:
            seen.add(c)
            yield c


def main():
    if sys.argv[1] == 'pick':
        with open(sys.argv[2], encoding='utf-8') as f:
            lines = f.readlines()
        random.Random(int(sys.argv[3])).shuffle(lines)
        sys.stdout.buffer.write(''.join(sorted(lines[:int(sys.argv[4])])).encode('utf-8'))
        return
    if sys.argv[1] == 'all':
        cases = list(all_cases())
    else:
        cases = list(all_cases())
        random.Random(int(sys.argv[2])).shuffle(cases)
        cases = cases[:int(sys.argv[3])]
    out = sys.stdout.buffer
    for c in cases:
        out.write(c.encode('utf-8') + b'\n')


if __name__ == '__main__':
    main()
