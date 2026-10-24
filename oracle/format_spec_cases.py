# Generates str.format cases for SCR112's format spec check: every
# combination of fill and alignment, sign, '#', '0', width, ',', precision
# and presentation type (valid and invalid ones), with and without !s / !r,
# applied to literal arguments of every kind. Prints "format<TAB>argument"
# lines: all of them, or a seeded random sample. `stratify` picks, from a
# result file of oracle/format-spec.sh, COUNT cases half of which Jython
# accepts (the cases that a false positive would come from).
#
# usage: python3 oracle/format_spec_cases.py all
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
ARGS = ["'ab'", "u'ab'", "''", '5', '-5', '0', '5L', '65', '1.5', '-1.5',
        '1e300', '1j', '[1]', '(1,)', '{}', 'None']


def cases():
    for parts in itertools.product(ALIGNS, SIGNS, ALTS, ZEROS, WIDTHS, COMMAS,
                                   PRECISIONS, TYPES):
        spec = ''.join(parts)
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
    out = list(cases())
    if sys.argv[1] == 'sample':
        random.seed(int(sys.argv[2]))
        out = random.sample(out, int(sys.argv[3]))
    for fmt, arg in out:
        print('%s\t%s' % (fmt, arg))


if __name__ == '__main__':
    main()
