# Runs inside IA's Jython 2.7 (see generate-unicode-names.sh). Dumps the
# character name table that Jython uses for `\N{...}` escapes in unicode
# literals, in the form iglint embeds.
#
# usage: jython jython_unicode_names.py OUT_DIR
#
# Jython resolves `\N{name}` with org.python.modules.ucnhash (ucnhash.dat in
# the Jython jar): a name is split into words, every word of the word list is
# replaced by its index and the resulting bytes are looked up in a sorted
# table. iglint repeats the encoding, so this script dumps
#
#   * the word list with the index Jython gives each word,
#   * every encoded name that Jython's lookup finds, with its code point,
#   * the characters that java.lang.Character.digit accepts in radix 16
#     (Jython parses `CJK COMPATIBILITY IDEOGRAPH-<hex>` with
#     Integer.parseInt instead of looking it up).
#
# OUT_DIR gets table.bin and table.rs (see
# crates/iglint_python/src/unicode_names.rs for the format) and names.tsv,
# every name Jython accepts with its code point, for checking.
import struct
import sys

from java.lang import Character
from org.python.modules import ucnhash

CHARMAP = " ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-()"
BLOCK = 16


def field(name):
    f = ucnhash.getDeclaredField(name)
    f.setAccessible(True)
    return f.get(None)


def method(name, arity):
    for m in ucnhash.getDeclaredMethods():
        if m.getName() == name and len(m.getParameterTypes()) == arity:
            m.setAccessible(True)
            return m
    raise LookupError(name)


def unsigned(data):
    return [b & 0xFF for b in data]


def load():
    if ucnhash().getCchMax() < 0:
        raise IOError('ucnhash.dat not loaded')
    t = {}
    for name in ('wordoffs', 'worddata', 'rawdata', 'rawindex', 'codepoint'):
        t[name] = list(field(name))
    for name in ('wordstart', 'wordcutoff', 'maxklen'):
        t[name] = int(field(name))
    return t


def words_of(t):
    offs, data = t['wordoffs'], t['worddata']
    words = []
    for i in range(len(offs)):
        end = offs[i + 1] if i < len(offs) - 1 else len(data)
        words.append(''.join(CHARMAP[c] for c in data[offs[i]:end]))
    return words


def entries_of(t):
    """The encoded name of every entry of the table, in table order."""
    raw, index = unsigned(t['rawdata']), t['rawindex']
    entries = []
    for i in range(len(t['codepoint'])):
        block = i // 12
        off = index[block * 3]
        lengths = ((index[block * 3 + 1] & 0xFFFFFFFF) << 32) | (index[block * 3 + 2] & 0xFFFFFFFF)
        for k in range(i % 12):
            off += (lengths >> (k * 5)) & 0x1F
        length = (lengths >> ((i % 12) * 5)) & 0x1F
        entries.append(raw[off:off + length])
    return entries


def separator(token):
    # A word ends at a space, or after a hyphen that does not start it.
    return '' if len(token) > 1 and token.endswith('-') else ' '


def spell(raw, words, t, k=0):
    """Every name that encodes to raw[k:]. A byte equal to wordcutoff is
    either a one-byte word index or the first byte of a two-byte one."""
    if k == len(raw):
        return ['']
    b = raw[k]
    if b == 0:
        return [None]  # handled by the caller of a spelled-out run
    if b < t['wordstart']:
        j = k
        while j < len(raw) and 0 < raw[j] < t['wordstart']:
            j += 1
        token = ''.join(CHARMAP[c] for c in raw[k:j])
        if j < len(raw) and raw[j] == 0:
            j += 1
        if j == len(raw):
            return [token]
        return [token + separator(token) + rest for rest in spell(raw, words, t, j) if rest]
    options = []
    if b <= t['wordcutoff']:
        options.append((b, k + 1))
    if b >= t['wordcutoff'] and k + 1 < len(raw):
        options.append((((b - t['wordcutoff']) << 8) | raw[k + 1], k + 2))
    names = []
    for h, j in options:
        if not 0 <= h - t['wordstart'] < len(words):
            continue
        token = words[h - t['wordstart']]
        if j == len(raw):
            names.append(token)
        else:
            names.extend(token + separator(token) + rest for rest in spell(raw, words, t, j) if rest)
    return names


def varint(value):
    out = []
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return out


def zigzag(value):
    return (value << 1) if value >= 0 else ((-value << 1) - 1)


def hex_digits():
    """(first, last, value of first) runs of UTF-16 units that
    Character.digit accepts in radix 16, with consecutive values."""
    runs = []
    for c in range(0x10000):
        d = Character.digit(c, 16)  # the int (code point) overload
        if d < 0:
            continue
        if runs and runs[-1][1] == c - 1 and runs[-1][2] + (c - runs[-1][0]) == d:
            runs[-1][1] = c
        else:
            runs.append([c, c, d])
    return runs


def main():
    out_dir = sys.argv[1]
    t = load()
    words = words_of(t)
    hash_ = method('hash', 1)
    for i, w in enumerate(words):
        if hash_.invoke(None, [w]) != i:
            raise ValueError('word hash is not perfect for %r' % w)
    search = method('binarysearch', 3)
    from jarray import array
    found = {}
    for raw in entries_of(t):
        key = tuple(raw)
        if key in found:
            continue
        idx = search.invoke(None, [array([b - 256 if b > 127 else b for b in raw], 'b'), 0, len(raw)])
        if idx >= 0:
            found[key] = t['codepoint'][idx]
    # Check the dump against Jython's own lookup of the spelled-out names.
    names = []
    for key in sorted(found.keys()):
        spellings = [n for n in spell(list(key), words, t) if ucnhash.lookup(n) == found[key]]
        if not spellings:
            # Unreachable: the stored encoding uses a word that starts with
            # a space (" SHOG"), which no input word can match.
            sys.stderr.write('unreachable: %r\n' % (key,))
            del found[key]
            continue
        names.extend((n, found[key]) for n in spellings)
    names.sort()

    # Words, sorted, for binary search: per word its end offset (u16) and
    # Jython index (u16), then the text.
    by_name = sorted(range(len(words)), key=lambda i: words[i])
    text = []
    ends = []
    for i in by_name:
        text.extend(ord(c) for c in words[i])
        ends.append(len(text))
    word_section = []
    for n, i in zip(ends, by_name):
        word_section.extend(struct.unpack('4B', struct.pack('<HH', n, i)))
    word_section.extend(text)

    # Names, sorted by their encoding, front coded in blocks of BLOCK:
    # shared prefix length, suffix length, suffix, code point (zigzag varint
    # delta from the previous entry of the block; absolute for the first).
    keys = sorted(found)
    blocks = []
    entries = []
    for i, key in enumerate(keys):
        if i % BLOCK == 0:
            blocks.append(len(entries))
            shared = 0
            delta = found[key]
        else:
            prev = keys[i - 1]
            shared = 0
            while shared < min(len(prev), len(key)) and prev[shared] == key[shared]:
                shared += 1
            delta = zigzag(found[key] - found[prev])
        entries.append(shared)
        entries.append(len(key) - shared)
        entries.extend(key[shared:])
        entries.extend(varint(delta))
    block_section = []
    for b in blocks:
        block_section.extend(struct.unpack('4B', struct.pack('<I', b)))

    data = word_section + block_section + entries
    f = open(out_dir + '/table.bin', 'wb')
    f.write(''.join(chr(b) for b in data))
    f.close()

    runs = hex_digits()
    rs = open(out_dir + '/table.rs', 'w')
    rs.write('// @generated by oracle/generate-unicode-names.sh from the ucnhash table\n')
    rs.write('// of IA\'s Jython 2.7.3; do not edit.\n\n')
    rs.write('pub(super) static DATA: &[u8] = include_bytes!("table.bin");\n')
    rs.write('pub(super) const WORD_COUNT: usize = %d;\n' % len(words))
    rs.write('pub(super) const WORDS_TEXT: usize = %d;\n' % (len(words) * 4))
    rs.write('pub(super) const BLOCKS: usize = %d;\n' % len(word_section))
    rs.write('pub(super) const BLOCK_COUNT: usize = %d;\n' % len(blocks))
    rs.write('pub(super) const ENTRIES: usize = %d;\n' % (len(word_section) + len(block_section)))
    rs.write('pub(super) const ENTRY_COUNT: usize = %d;\n' % len(keys))
    rs.write('pub(super) const BLOCK_LEN: usize = %d;\n' % BLOCK)
    rs.write('pub(super) const WORD_START: u16 = %d;\n' % t['wordstart'])
    rs.write('pub(super) const WORD_CUTOFF: u16 = %d;\n' % t['wordcutoff'])
    rs.write('pub(super) const MAX_NAME_LEN: usize = %d;\n' % t['maxklen'])
    rs.write('/// `(first, last, digit of first)`: UTF-16 units with a hex digit value\n')
    rs.write('/// for `java.lang.Character.digit`.\n')
    rs.write('#[rustfmt::skip]\n')
    rs.write('pub(super) static HEX_DIGITS: &[(u16, u16, u8)] = &[\n')
    for first, last, value in runs:
        rs.write('    (0x%04X, 0x%04X, %d),\n' % (first, last, value))
    rs.write('];\n')
    rs.close()

    tsv = open(out_dir + '/names.tsv', 'w')
    for name, cp in names:
        tsv.write('%s\t%X\n' % (name, cp))
    tsv.close()
    sys.stderr.write('%d words, %d names (%d spellings), %d bytes, %d hex digit runs\n'
                     % (len(words), len(keys), len(names), len(data), len(runs)))


if __name__ == '__main__':
    main()
