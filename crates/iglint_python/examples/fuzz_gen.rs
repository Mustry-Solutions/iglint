//! Generates fuzz cases for the differential test against Jython (see
//! `oracle/fuzz.sh`): mutations of snippets of real Python files, and
//! programs from a small random grammar.
//!
//! usage: fuzz_gen SEED COUNT SOURCE_DIR OUT.cases

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use iglint_python::TokenKind;

/// xorshift64*
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next() % bound as u64) as usize
        }
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [seed, count, dir, out] = <[String; 4]>::try_from(args).unwrap_or_else(|_| {
        eprintln!("usage: fuzz_gen SEED COUNT SOURCE_DIR OUT.cases");
        std::process::exit(2);
    });
    let mut rng = Rng(seed.parse::<u64>().expect("seed").wrapping_mul(0x9E37_79B9) | 1);
    let count: usize = count.parse().expect("count");
    let mut files = Vec::new();
    collect_python_files(Path::new(&dir), &mut files);
    files.sort();
    let sources: Vec<String> = files
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter(|source| !source.is_empty())
        .collect();
    assert!(!sources.is_empty(), "no Python sources in {dir}");

    let mut output = String::new();
    for index in 0..count {
        let case = if rng.chance(70) {
            let source = rng_pick_source(&mut rng, &sources);
            mutate_snippet(&mut rng, source)
        } else {
            let mut generator = Grammar {
                rng: &mut rng,
                out: String::new(),
                depth: 0,
            };
            generator.module();
            let mut source = generator.out;
            if rng.chance(30) {
                source = mutate(&mut rng, &source);
            }
            source
        };
        let case = sanitize(&case);
        if case.ends_with('\n') {
            let _ = writeln!(output, "### fuzz {index}");
            output.push_str(&case);
        } else {
            // Cases end with a line end in the file; the flag removes it.
            let _ = writeln!(output, "### fuzz {index} [nonl]");
            output.push_str(&case);
            output.push('\n');
        }
    }
    std::fs::write(out, output).expect("write cases");
}

fn rng_pick_source<'a>(rng: &mut Rng, sources: &'a [String]) -> &'a str {
    &sources[rng.below(sources.len())]
}

fn collect_python_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_python_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "py") {
            out.push(path);
        }
    }
}

/// Lines starting with `### ` would split the case.
fn sanitize(source: &str) -> String {
    source
        .split_inclusive('\n')
        .map(|line| {
            if line.starts_with("###") {
                format!("#x{}", &line[1..])
            } else {
                line.to_owned()
            }
        })
        .collect()
}

/// A window of lines from a source, optionally dedented, then mutated.
fn mutate_snippet(rng: &mut Rng, source: &str) -> String {
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let start = rng.below(lines.len());
    let len = 1 + rng.below(15);
    let mut snippet: String = lines[start..(start + len).min(lines.len())].concat();
    if rng.chance(60) {
        snippet = dedent(&snippet);
    }
    let rounds = rng.below(4);
    for _ in 0..rounds {
        snippet = mutate(rng, &snippet);
    }
    snippet
}

fn dedent(text: &str) -> String {
    let indent = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start_matches([' ', '\t']).len())
        .min()
        .unwrap_or(0);
    text.split_inclusive('\n')
        .map(|line| {
            let strip = indent.min(line.len() - line.trim_start_matches([' ', '\t']).len());
            &line[strip..]
        })
        .collect()
}

const INSERTIONS: &[&str] = &[
    "(",
    ")",
    "[",
    "]",
    "{",
    "}",
    ":",
    ",",
    ";",
    "=",
    "+",
    "-",
    "*",
    "**",
    "/",
    "//",
    "%",
    "<<",
    ">>",
    "&",
    "|",
    "^",
    "~",
    "<",
    ">",
    "==",
    "!=",
    "<>",
    "<=",
    ">=",
    "+=",
    ".",
    "...",
    "`",
    "@",
    "\n",
    "\n    ",
    "\n\t",
    "    ",
    "\\\n",
    "#",
    "'",
    "\"",
    "'''",
    "\"\"\"",
    "if ",
    "else ",
    "elif ",
    "for ",
    " in ",
    "not ",
    " is ",
    "and ",
    "or ",
    "lambda ",
    "lambda x: ",
    "yield ",
    "print ",
    "print >>f, ",
    "exec ",
    "del ",
    "return ",
    "def ",
    "class ",
    "try:",
    "except:",
    "finally:",
    "with ",
    " as ",
    "import ",
    "from ",
    "global ",
    "assert ",
    "raise ",
    "pass",
    "None",
    "x",
    "self",
    "0",
    "1L",
    "0x1f",
    "1.5",
    "2j",
    "'s'",
    "u'u'",
    "r'\\n'",
    "*args",
    "**kw",
    "x=1",
    "\r\n",
    "\x0c",
    "\t",
    "!",
    "$",
];

/// One random token-level or character-level mutation.
fn mutate(rng: &mut Rng, source: &str) -> String {
    let (tokens, _) = iglint_python::tokenize(source);
    let real: Vec<_> = tokens
        .iter()
        .filter(|token| {
            !token.range.is_empty()
                && !matches!(
                    token.kind,
                    TokenKind::Indent | TokenKind::Dedent | TokenKind::EndOfFile
                )
        })
        .collect();
    let boundary = |rng: &mut Rng| -> usize {
        if real.is_empty() || rng.chance(20) {
            let mut at = rng.below(source.len() + 1);
            while !source.is_char_boundary(at) {
                at -= 1;
            }
            at
        } else {
            let token = rng.pick(&real);
            if rng.chance(50) {
                token.range.start().to_usize()
            } else {
                token.range.end().to_usize()
            }
        }
    };
    match rng.below(6) {
        // Delete a token.
        0 if !real.is_empty() => {
            let token = rng.pick(&real);
            let range = token.range.to_std();
            format!("{}{}", &source[..range.start], &source[range.end..])
        }
        // Duplicate a token somewhere else.
        1 if !real.is_empty() => {
            let token = rng.pick(&real);
            let text = &source[token.range.to_std()];
            let at = boundary(rng);
            format!("{}{}{}", &source[..at], text, &source[at..])
        }
        // Swap two adjacent tokens.
        2 if real.len() > 1 => {
            let index = rng.below(real.len() - 1);
            let (a, b) = (real[index].range.to_std(), real[index + 1].range.to_std());
            if a.end > b.start {
                return source.to_owned();
            }
            format!(
                "{}{}{}{}{}",
                &source[..a.start],
                &source[b.clone()],
                &source[a.end..b.start],
                &source[a.clone()],
                &source[b.end..]
            )
        }
        // Truncate.
        3 => {
            let at = boundary(rng);
            source[..at].to_owned()
        }
        // Delete a character.
        4 if !source.is_empty() => {
            let mut at = rng.below(source.len());
            while !source.is_char_boundary(at) {
                at -= 1;
            }
            let next = source[at..].chars().next().map_or(0, char::len_utf8);
            format!("{}{}", &source[..at], &source[at + next..])
        }
        // Insert a token.
        _ => {
            let at = boundary(rng);
            let text = rng.pick(INSERTIONS);
            format!("{}{}{}", &source[..at], text, &source[at..])
        }
    }
}

/// Random programs from a small Python 2 grammar.
struct Grammar<'a> {
    rng: &'a mut Rng,
    out: String,
    depth: usize,
}

impl Grammar<'_> {
    fn module(&mut self) {
        let count = 1 + self.rng.below(5);
        for _ in 0..count {
            self.statement(0);
        }
    }

    fn indent(&mut self, level: usize) {
        for _ in 0..level {
            self.out
                .push_str(if self.rng.chance(5) { "\t" } else { "    " });
        }
    }

    fn statement(&mut self, level: usize) {
        self.indent(level);
        if level < 3 && self.rng.chance(30) {
            self.compound(level);
        } else {
            self.simple();
            self.out.push('\n');
        }
    }

    fn block(&mut self, level: usize) {
        if self.rng.chance(20) {
            self.out.push(' ');
            self.simple();
            self.out.push('\n');
            return;
        }
        self.out.push('\n');
        let count = 1 + self.rng.below(3);
        for _ in 0..count {
            self.statement(level + 1);
        }
    }

    fn compound(&mut self, level: usize) {
        match self.rng.below(8) {
            0 => {
                self.out.push_str("if ");
                self.test();
                self.out.push(':');
                self.block(level);
                if self.rng.chance(40) {
                    self.indent(level);
                    self.out.push_str("elif ");
                    self.test();
                    self.out.push(':');
                    self.block(level);
                }
                if self.rng.chance(40) {
                    self.indent(level);
                    self.out.push_str("else:");
                    self.block(level);
                }
            }
            1 => {
                self.out.push_str("while ");
                self.test();
                self.out.push(':');
                self.block(level);
            }
            2 => {
                self.out.push_str("for ");
                self.target();
                self.out.push_str(" in ");
                self.testlist();
                self.out.push(':');
                self.block(level);
            }
            3 => {
                self.out.push_str("try:");
                self.block(level);
                let handlers = self.rng.below(3);
                for _ in 0..handlers {
                    self.indent(level);
                    self.out.push_str("except");
                    if self.rng.chance(60) {
                        self.out.push(' ');
                        self.expr();
                        if self.rng.chance(50) {
                            self.out
                                .push_str(if self.rng.chance(50) { ", " } else { " as " });
                            self.target();
                        }
                    }
                    self.out.push(':');
                    self.block(level);
                }
                if handlers == 0 || self.rng.chance(30) {
                    self.indent(level);
                    self.out.push_str("finally:");
                    self.block(level);
                }
            }
            4 => {
                self.out.push_str("with ");
                self.test();
                if self.rng.chance(50) {
                    self.out.push_str(" as ");
                    self.target();
                }
                self.out.push(':');
                self.block(level);
            }
            5 | 6 => {
                if self.rng.chance(30) {
                    self.out.push('@');
                    self.name();
                    if self.rng.chance(50) {
                        self.out.push_str("(x)");
                    }
                    self.out.push('\n');
                    self.indent(level);
                }
                self.out.push_str("def ");
                self.name();
                self.out.push('(');
                self.parameters();
                self.out.push_str("):");
                self.block(level);
            }
            _ => {
                self.out.push_str("class ");
                self.name();
                if self.rng.chance(50) {
                    self.out.push('(');
                    self.expr();
                    self.out.push(')');
                }
                self.out.push(':');
                self.block(level);
            }
        }
    }

    fn parameters(&mut self) {
        let count = self.rng.below(4);
        let mut defaults = false;
        for index in 0..count {
            if index > 0 {
                self.out.push_str(", ");
            }
            if self.rng.chance(10) {
                self.out.push_str("(a, b)");
            } else {
                self.name();
            }
            if defaults || self.rng.chance(30) {
                defaults = true;
                self.out.push('=');
                self.atom();
            }
        }
        if self.rng.chance(20) {
            if count > 0 {
                self.out.push_str(", ");
            }
            self.out.push_str("*args");
            if self.rng.chance(50) {
                self.out.push_str(", **kw");
            }
        }
    }

    fn simple(&mut self) {
        match self.rng.below(16) {
            0..=4 => {
                let targets = 1 + self.rng.below(2);
                for _ in 0..targets {
                    self.target();
                    self.out.push_str(" = ");
                }
                self.testlist();
            }
            5 => {
                self.target();
                let op = *self.rng.pick(&[
                    "+=", "-=", "*=", "/=", "//=", "%=", "**=", ">>=", "<<=", "&=", "^=", "|=",
                ]);
                self.out.push(' ');
                self.out.push_str(op);
                self.out.push(' ');
                self.testlist();
            }
            6 => {
                self.out.push_str("print");
                if self.rng.chance(30) {
                    self.out.push_str(" >>f,");
                }
                if self.rng.chance(80) {
                    self.out.push(' ');
                    self.testlist();
                }
            }
            7 => {
                self.out.push_str("return");
                if self.rng.chance(70) {
                    self.out.push(' ');
                    self.testlist();
                }
            }
            8 => {
                self.out.push_str("raise ");
                self.test();
                if self.rng.chance(30) {
                    self.out.push_str(", ");
                    self.test();
                }
            }
            9 => {
                let word: &&str = self.rng.pick(&["pass", "break", "continue"]);
                self.out.push_str(word);
            }
            10 => {
                self.out.push_str("import ");
                self.name();
                if self.rng.chance(30) {
                    self.out.push_str(".path as p");
                }
            }
            11 => {
                self.out.push_str("from ");
                if self.rng.chance(30) {
                    self.out.push('.');
                }
                self.name();
                self.out.push_str(" import ");
                if self.rng.chance(20) {
                    self.out.push('*');
                } else {
                    self.name();
                    if self.rng.chance(30) {
                        self.out.push_str(", y as z");
                    }
                }
            }
            12 => {
                self.out.push_str("del ");
                self.target();
            }
            13 => {
                self.out.push_str("assert ");
                self.test();
            }
            14 => {
                self.out.push_str("exec ");
                self.expr();
                if self.rng.chance(50) {
                    self.out.push_str(" in g");
                }
            }
            _ => self.testlist(),
        }
    }

    fn target(&mut self) {
        match self.rng.below(6) {
            0 => {
                self.out.push('(');
                self.name();
                self.out.push_str(", ");
                self.name();
                self.out.push(')');
            }
            1 => {
                self.name();
                self.out.push_str(", ");
                self.name();
            }
            2 => {
                self.atom();
                self.out.push('.');
                self.name();
            }
            3 => {
                self.name();
                self.out.push('[');
                self.test();
                self.out.push(']');
            }
            _ => self.name(),
        }
    }

    fn name(&mut self) {
        let name = *self.rng.pick(&[
            "x", "y", "foo", "self", "None", "True", "a1", "_b", "print", "nonlocal",
        ]);
        self.out.push_str(name);
    }

    fn testlist(&mut self) {
        self.test();
        if self.rng.chance(20) {
            self.out.push_str(", ");
            self.test();
            if self.rng.chance(30) {
                self.out.push(',');
            }
        }
    }

    fn test(&mut self) {
        if self.depth > 4 {
            self.atom();
            return;
        }
        self.depth += 1;
        match self.rng.below(12) {
            0 => {
                self.out.push_str("lambda ");
                self.parameters();
                self.out.push_str(": ");
                self.test();
            }
            1 => {
                self.expr();
                self.out.push_str(" if ");
                self.expr();
                self.out.push_str(" else ");
                self.test();
            }
            2 => {
                self.out.push_str("not ");
                self.expr();
            }
            3 => {
                self.expr();
                let op = *self.rng.pick(&[" and ", " or "]);
                self.out.push_str(op);
                self.expr();
            }
            4 => {
                self.expr();
                let op = *self.rng.pick(&[
                    " < ", " > ", " == ", " != ", " <> ", " in ", " not in ", " is ", " is not ",
                    " <= ",
                ]);
                self.out.push_str(op);
                self.expr();
            }
            _ => self.expr(),
        }
        self.depth -= 1;
    }

    fn expr(&mut self) {
        if self.depth > 5 {
            self.atom();
            return;
        }
        self.depth += 1;
        match self.rng.below(8) {
            0..=2 => {
                self.factor();
                let count = 1 + self.rng.below(3);
                for _ in 0..count {
                    let op = *self.rng.pick(&[
                        " + ", " - ", " * ", " / ", " // ", " % ", " | ", " ^ ", " & ", " << ",
                        " >> ", " ** ",
                    ]);
                    self.out.push_str(op);
                    self.factor();
                }
            }
            _ => self.factor(),
        }
        self.depth -= 1;
    }

    fn factor(&mut self) {
        if self.rng.chance(15) {
            let op = *self.rng.pick(&["-", "+", "~", "- "]);
            self.out.push_str(op);
        }
        self.atom();
        while self.depth < 6 && self.rng.chance(25) {
            match self.rng.below(3) {
                0 => {
                    self.out.push('.');
                    let name = *self.rng.pick(&["x", "y", "print", "from", "if"]);
                    self.out.push_str(name);
                }
                1 => {
                    self.out.push('[');
                    match self.rng.below(4) {
                        0 => self.out.push_str("1:2"),
                        1 => self.out.push_str("::x, ..."),
                        2 => self.out.push_str("a, b"),
                        _ => self.test(),
                    }
                    self.out.push(']');
                }
                _ => {
                    self.out.push('(');
                    match self.rng.below(5) {
                        0 => {}
                        1 => self.out.push_str("a, b=1"),
                        2 => self.out.push_str("*a, **k"),
                        3 => self.out.push_str("x for x in y"),
                        _ => self.test(),
                    }
                    self.out.push(')');
                }
            }
        }
    }

    fn atom(&mut self) {
        let choice = if self.depth > 5 {
            self.rng.below(4)
        } else {
            self.rng.below(12)
        };
        match choice {
            0 => self.name(),
            1 => {
                let number = *self.rng.pick(&[
                    "0",
                    "1",
                    "42",
                    "0777",
                    "0x1F",
                    "10L",
                    "1.5",
                    "1e16",
                    ".5",
                    "2j",
                    "2147483648",
                    "0b101",
                    "1e-5",
                ]);
                self.out.push_str(number);
            }
            2 => {
                let string = *self.rng.pick(&[
                    "'a'",
                    "\"b\"",
                    "u'c'",
                    "r'\\d'",
                    "'''e\nf'''",
                    "'g' 'h'",
                    "b'i'",
                    "'\\x41\\n'",
                ]);
                self.out.push_str(string);
            }
            3 => self.name(),
            4 => {
                self.out.push('(');
                self.test();
                if self.rng.chance(40) {
                    self.out.push(',');
                }
                self.out.push(')');
            }
            5 => {
                self.out.push('[');
                if self.rng.chance(40) {
                    self.test();
                    self.out.push_str(" for ");
                    self.target();
                    self.out.push_str(" in ");
                    self.expr();
                    if self.rng.chance(40) {
                        self.out.push_str(" if ");
                        self.expr();
                    }
                } else if self.rng.chance(70) {
                    self.test();
                    self.out.push_str(", ");
                    self.test();
                }
                self.out.push(']');
            }
            6 => {
                self.out.push('{');
                match self.rng.below(4) {
                    0 => {}
                    1 => {
                        self.test();
                        self.out.push_str(": ");
                        self.test();
                    }
                    2 => {
                        self.test();
                        self.out.push_str(": ");
                        self.test();
                        self.out.push_str(" for x in y");
                    }
                    _ => {
                        self.test();
                        self.out.push_str(", ");
                        self.test();
                    }
                }
                self.out.push('}');
            }
            7 => {
                self.out.push('`');
                self.test();
                self.out.push('`');
            }
            8 => {
                self.out.push('(');
                self.test();
                self.out.push_str(" for x in y)");
            }
            9 => self.out.push_str("(yield x)"),
            10 => {
                self.out.push_str("(\n    ");
                self.test();
                self.out.push_str(",\n)");
            }
            _ => self.out.push_str("()"),
        }
    }
}
