# Generates str.format cases for SCR112's format spec check: every
# combination of fill and alignment, sign, '#', '0', width, ',', precision
# and presentation type (valid and invalid ones), with and without !s / !r,
# applied to literal arguments of every kind; then, on a smaller grid,
# the same with characters after the type (`dd`, `%Y`, `.2.2`, `s.`). Prints "format<TAB>argument"
# lines: all of them, or a seeded random sample. `stratify` picks, from a
# result file of oracle/format-spec.sh, COUNT cases half of which Jython
# accepts (the cases that a false positive would come from).
#
# `nonascii` generates, on a smaller grid, specs whose fill character is
# not ASCII (Latin-1, beyond Latin-1, astral), for unicode format strings
# (format column prefixed with `u`) and, for fills up to U+00FF, byte
# format strings (prefixed with `b`, the fill encoded as Latin-1). Lines
# are UTF-8.
#
# usage: python3 oracle/format_spec_cases.py all
#        python3 oracle/format_spec_cases.py nonascii
#        python3 oracle/format_spec_cases.py sample SEED COUNT
#        python3 oracle/format_spec_cases.py stratify RESULTS SEED COUNT
import itertools
import random
import sys

ALIGNS = ['', '<', '>', '^', '=', 'x<', '*=', '0>', '0=']
SIGNS = ['', '+', '-', ' ']
ALTS = ['', '#']
ZEROS = ['', '0']
WIDTHS = ['', '5', '10']
COMMAS = ['', ',']
PRECISIONS = ['', '.2', '.0']
TYPES = [''] + list('bcdoxXneEfFgG%s') + list('zaiurjZSB')
CONVERSIONS = ['', '!s', '!r']
# Characters after the type, on a smaller grid of the other parts.
TRAILERS = ['d', 's', '.', '#', '2', '.2', 'Y', '%', ',', '<']
SMALL_ALIGNS = ['', '<', '0=']
SMALL_SIGNS = ['', '+']
SMALL_WIDTHS = ['', '5']
SMALL_PRECISIONS = ['', '.2']
ARGS = ["'ab'", "u'ab'", "''", '5', '-5', '0', '5L', '65', '1.5', '-1.5',
        '1e300', '1j', '[1]', '(1,)', '{}', 'None']


def specs():
    for parts in itertools.product(ALIGNS, SIGNS, ALTS, ZEROS, WIDTHS, COMMAS,
                                   PRECISIONS, TYPES):
        yield ''.join(parts)
    for parts in itertools.product(SMALL_ALIGNS, SMALL_SIGNS, ALTS, ZEROS,
                                   SMALL_WIDTHS, COMMAS, SMALL_PRECISIONS,
                                   TYPES, TRAILERS):
        yield ''.join(parts)


FILLS = [u'\xe9', u'\xa0', u'\x80', u'\xff', u'\u0100', u'\u20ac', u'\U0001f980']
FILL_ALIGNS = ['<', '>', '^', '=']
FILL_ARGS = ["'ab'", "u'ab'", '5', '1.5', 'None', '[1]']
FILL_CONVERSIONS = ['', '!s']


def nonascii_cases():
    for fill in FILLS:
        kinds = ['u', 'b'] if ord(fill[0]) <= 0xff else ['u']
        for parts in itertools.product(FILL_ALIGNS, SMALL_SIGNS, ALTS, ZEROS,
                                       SMALL_WIDTHS, COMMAS, SMALL_PRECISIONS,
                                       TYPES):
            spec = fill + ''.join(parts)
            for kind in kinds:
                for conversion in FILL_CONVERSIONS:
                    for arg in FILL_ARGS:
                        yield '%s{0%s:%s}' % (kind, conversion, spec), arg


def cases():
    for spec in specs():
        for conversion in CONVERSIONS:
            for arg in ARGS:
                yield '{0%s:%s}' % (conversion, spec), arg


def stratify(results, seed, count):
    ok, err = [], []
    for line in open(results):
        (ok if line.rstrip('\n').split('\t')[2] == 'ok' else err).append(line)
    random.seed(seed)
    picked = random.sample(ok, count // 2) + random.sample(err, count - count // 2)
    sys.stdout.write(''.join(sorted(picked)))


def main():
    if sys.argv[1] == 'stratify':
        stratify(sys.argv[2], int(sys.argv[3]), int(sys.argv[4]))
        return
    if sys.argv[1] == 'nonascii':
        out = list(nonascii_cases())
    else:
        out = list(cases())
    if sys.argv[1] == 'sample':
        random.seed(int(sys.argv[2]))
        out = random.sample(out, int(sys.argv[3]))
    write = getattr(sys.stdout, 'buffer', sys.stdout).write
    for fmt, arg in out:
        write(('%s\t%s\n' % (fmt, arg)).encode('utf-8'))


if __name__ == '__main__':
    main()
