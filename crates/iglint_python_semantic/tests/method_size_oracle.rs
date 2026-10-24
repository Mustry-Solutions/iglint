//! The bytecode lower bound of `SCR002` ("Module or method too large")
//! against IA's Jython.
//!
//! The sources are generated here: random programs that mix every
//! statement and expression form, and repetitions of single statements
//! up to past the point where Jython fails. `tests/oracle/method_size.expected`
//! records, per source, the bytecode length of every code unit as Jython's
//! code generator emits it and whether full `compile()` succeeds
//! (`oracle/run-oracle.sh --method-size`). To measure again:
//!
//! ```sh
//! IGLINT_METHOD_SIZE_OUT=$HOME/.cache/method-size cargo test -p iglint_python_semantic \
//!     --test method_size_oracle -- --ignored write_sources
//! echo method_size.cases > $HOME/.cache/method-size/list.txt
//! oracle/run-oracle.sh --method-size $HOME/.cache/method-size \
//!     $HOME/.cache/method-size/list.txt crates/iglint_python_semantic/tests/oracle/method_size.expected
//! ```
//!
//! The bound must never exceed the measured size of any unit, so that
//! `SCR002` never reports a script Jython compiles; and it must report
//! the shapes Jython rejects close to where Jython starts to reject them.
//!
//! The file also records every method of the class and its constant pool
//! count, for the upper bounds (`iglint_python_semantic::upper`, review
//! round 18): they must never be below what Jython generates, also for
//! the programs close to each limit ([`near_limit_sources`]), and a
//! program whose verdict both bounds decide must get Jython's verdict.
//! A larger calibration run: `IGLINT_METHOD_SIZE_NEAR=3000` for writing
//! the sources and for the check, with the output of the oracle in
//! `IGLINT_METHOD_SIZE_EXPECTED`.

use iglint_python_semantic::{
    CompileErrorKind, code_units, init_lower, size_errors, size_unknown, upper,
};

/// Runs `f` on a thread with a stack deep enough for the longest `elif`
/// chains (as the linter does).
fn big_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(256 * 1024 * 1024)
            .spawn_scoped(scope, f)
            .expect("thread")
            .join()
            .expect("no panic")
    })
}

/// Random programs: (first seed, count, statements per program).
const RANDOM: &[(u64, u64, usize)] = &[(0, 2000, 40), (100_000, 300, 300)];

/// A small deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x2545_F491_4F6C_DD1D | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

const NAMES: &[&str] = &["a", "b", "c", "x", "y", "self", "None", "True", "q"];

fn atom(r: &mut Rng) -> String {
    match r.below(10) {
        0..=2 => r.pick(NAMES).to_owned(),
        3 => r.below(300).to_string(),
        4 => (1_000_000_000 + r.below(1_000_000_000)).to_string(),
        5 => "1.5".to_owned(),
        6 => "2j".to_owned(),
        7 => format!("'s{}'", r.below(10)),
        8 => "u'x'".to_owned(),
        _ => "'\\xe9'".to_owned(),
    }
}

fn expr(r: &mut Rng, depth: usize) -> String {
    if depth == 0 || r.chance(25) {
        return atom(r);
    }
    let d = depth - 1;
    match r.below(25) {
        0 => format!("({}).{}", expr(r, d), r.pick(&["a", "b", "append"])),
        1 => format!("{}[{}]", expr(r, d), expr(r, d)),
        2 => format!("{}[{}:{}]", expr(r, d), expr(r, d), expr(r, d)),
        3 => format!("{}[:]", expr(r, d)),
        4 => {
            let mut args: Vec<String> = (0..r.below(8)).map(|_| expr(r, d)).collect();
            if r.chance(30) {
                args.push(format!("k={}", expr(r, d)));
            }
            if r.chance(15) {
                args.push(format!("*{}", expr(r, d)));
            }
            if r.chance(10) {
                args.push(format!("**{}", expr(r, d)));
            }
            format!("{}({})", expr(r, d), args.join(", "))
        }
        5 => {
            let op = r.pick(&["+", "-", "*", "/", "%", "**", "<<", "&", "|", "^", "//"]);
            format!("({} {op} {})", expr(r, d), expr(r, d))
        }
        6 => format!("({} {})", r.pick(&["-", "+", "~", "not"]), expr(r, d)),
        7 => {
            let op = format!(" {} ", r.pick(&["and", "or"]));
            let n = 2 + r.below(3);
            let values: Vec<String> = (0..n).map(|_| expr(r, d)).collect();
            format!("({})", values.join(&op))
        }
        8 => {
            let mut parts = vec![expr(r, d)];
            for _ in 0..1 + r.below(3) {
                parts.push(
                    r.pick(&["<", "==", "!=", "is", "is not", "in", "not in", ">="])
                        .to_owned(),
                );
                parts.push(expr(r, d));
            }
            format!("({})", parts.join(" "))
        }
        9 => {
            let elts: Vec<String> = (0..r.below(7)).map(|_| expr(r, d)).collect();
            format!("[{}]", elts.join(", "))
        }
        10 => {
            let n = r.below(7);
            let elts: Vec<String> = (0..n).map(|_| expr(r, d)).collect();
            format!("({}{})", elts.join(", "), if n == 1 { "," } else { "" })
        }
        11 => {
            let items: Vec<String> = (0..r.below(6))
                .map(|_| format!("{}: {}", expr(r, d), expr(r, d)))
                .collect();
            format!("{{{}}}", items.join(", "))
        }
        12 => {
            let elts: Vec<String> = (0..1 + r.below(4)).map(|_| expr(r, d)).collect();
            format!("{{{}}}", elts.join(", "))
        }
        13 => {
            let gens: Vec<String> = (0..1 + r.below(2))
                .map(|_| {
                    let target = r.pick(&["i", "j", "(i, j)"]);
                    let iter = expr(r, d);
                    let cond = if r.chance(40) {
                        format!(" if {}", expr(r, d))
                    } else {
                        String::new()
                    };
                    format!("for {target} in {iter}{cond}")
                })
                .collect();
            format!("[{} {}]", expr(r, d), gens.join(" "))
        }
        14 => format!("({} for i in {})", expr(r, d), expr(r, d)),
        15 => format!("{{{} for i in {}}}", expr(r, d), expr(r, d)),
        16 => format!("{{{}: {} for i in {}}}", expr(r, d), expr(r, d), expr(r, d)),
        17 => {
            let params = match r.below(5) {
                0 => String::new(),
                1 => "p".to_owned(),
                2 => format!("p={}", expr(r, d)),
                3 => "p, *r".to_owned(),
                _ => "p, **kw".to_owned(),
            };
            format!("(lambda {params}: {})", expr(r, d))
        }
        18 => format!("({} if {} else {})", expr(r, d), expr(r, d), expr(r, d)),
        19 => format!("`{}`", expr(r, d)),
        20 => format!("('%s' % {})", expr(r, d)),
        21 => format!("{}[...]", expr(r, d)),
        22 => format!("{}[{}, {}]", expr(r, d), expr(r, d), expr(r, d)),
        23 => format!(
            "{}[{}:{}, {}]",
            expr(r, d),
            expr(r, d),
            expr(r, d),
            expr(r, d)
        ),
        _ => expr(r, d),
    }
}

fn target(r: &mut Rng, depth: usize) -> String {
    match r.below(7) {
        0..=2 => r.pick(&["a", "b", "c", "x", "y"]).to_owned(),
        3 => format!("({}).a", expr(r, depth.saturating_sub(1))),
        4 => format!(
            "{}[{}]",
            expr(r, depth.saturating_sub(1)),
            expr(r, depth.saturating_sub(1))
        ),
        5 if depth > 0 => format!("{}, {}", target(r, depth - 1), target(r, depth - 1)),
        5 => "a, b".to_owned(),
        _ => "[a, b]".to_owned(),
    }
}

#[derive(Clone, Copy)]
struct Context {
    function: bool,
    looping: bool,
    /// Inside a `def` or `class` (no `import *` or bare `exec` there).
    nested: bool,
}

fn simple(r: &mut Rng, c: &Context) -> String {
    let e = |r: &mut Rng| expr(r, 3);
    match r.below(21) {
        0 => "pass".to_owned(),
        1 => format!("{} = {}", target(r, 2), e(r)),
        2 => format!("{} = {} = {}", target(r, 1), target(r, 1), e(r)),
        3 => format!(
            "{} {}= {}",
            r.pick(&["a", "x.a", "x[0]"]),
            r.pick(&["+", "-", "*", "|"]),
            e(r)
        ),
        4 => e(r),
        5 => {
            let values: Vec<String> = (0..r.below(4)).map(|_| e(r)).collect();
            let comma = if !values.is_empty() && r.chance(30) {
                ","
            } else {
                ""
            };
            format!("print {}{comma}", values.join(", "))
        }
        6 => format!("print >>{}, {}", e(r), e(r)),
        // Names may be free in nested functions, where `del` does not compile.
        7 => format!("del {}", r.pick(&["x.a", "x[0]", "x.a, x[0]", "x[1:2]"])),
        8 => {
            let msg = if r.chance(50) {
                format!(", {}", e(r))
            } else {
                String::new()
            };
            format!("assert {}{msg}", e(r))
        }
        9 => r
            .pick(&[
                "import os",
                "import os.path as p",
                "import os, sys",
                "from os import path",
                "from os import path as q, sep",
            ])
            .to_owned(),
        10 if !c.nested => "from os import *".to_owned(),
        11 if !c.nested => r
            .pick(&["exec '1'", "exec '1' in d", "exec '1' in d, e"])
            .to_owned(),
        12 => format!("global g{}", r.below(4)),
        13 if c.looping => "break".to_owned(),
        14 if c.looping && r.chance(20) => "continue".to_owned(),
        // A value is not allowed in a generator.
        15 if c.function && r.chance(20) => "return".to_owned(),
        16 if r.chance(10) => match r.below(3) {
            0 => "raise".to_owned(),
            1 => format!("raise {}", e(r)),
            _ => format!("raise {}, {}", e(r), e(r)),
        },
        17 if c.function && r.chance(30) => match r.below(3) {
            0 => "yield".to_owned(),
            1 => format!("yield {}", e(r)),
            _ => format!("a = yield {}", e(r)),
        },
        18 => "'''doc'''".to_owned(),
        _ => format!("{} = {}", target(r, 1), e(r)),
    }
}

fn block(r: &mut Rng, indent: usize, depth: usize, n: usize, c: &Context, lines: &mut Vec<String>) {
    let pad = "    ".repeat(indent);
    let inner = |r: &mut Rng, n: usize, c: &Context, lines: &mut Vec<String>| {
        block(r, indent + 1, depth - 1, n, c, lines)
    };
    for _ in 0..n {
        if depth == 0 || !r.chance(25) {
            lines.push(format!("{pad}{}", simple(r, c)));
            continue;
        }
        let looping = Context {
            looping: true,
            ..*c
        };
        match r.below(9) {
            0 => {
                lines.push(format!("{pad}if {}:", expr(r, 2)));
                let n = 1 + r.below(4);
                inner(r, n, c, lines);
                if r.chance(30) {
                    lines.push(format!("{pad}elif {}:", expr(r, 2)));
                    let n = 1 + r.below(3);
                    inner(r, n, c, lines);
                }
                if r.chance(50) {
                    lines.push(format!("{pad}else:"));
                    let n = 1 + r.below(3);
                    inner(r, n, c, lines);
                }
            }
            1 => {
                lines.push(format!("{pad}while {}:", expr(r, 2)));
                let n = 1 + r.below(4);
                inner(r, n, &looping, lines);
                if r.chance(30) {
                    lines.push(format!("{pad}else:"));
                    inner(r, 1, c, lines);
                }
            }
            2 => {
                let t = r.pick(&["i", "i, j", "(i, j)", "x.a", "x[0]"]);
                lines.push(format!("{pad}for {t} in {}:", expr(r, 2)));
                let n = 1 + r.below(4);
                inner(r, n, &looping, lines);
                if r.chance(30) {
                    lines.push(format!("{pad}else:"));
                    inner(r, 1, c, lines);
                }
            }
            3 => {
                lines.push(format!("{pad}try:"));
                let n = 1 + r.below(3);
                inner(r, n, c, lines);
                for _ in 0..r.below(3) {
                    let h = r.pick(&[
                        "except E:",
                        "except (E, F), e:",
                        "except E, e:",
                        "except E as e:",
                    ]);
                    lines.push(format!("{pad}{h}"));
                    let n = 1 + r.below(2);
                    inner(r, n, c, lines);
                }
                lines.push(format!("{pad}except:"));
                inner(r, 1, c, lines);
                if r.chance(30) {
                    lines.push(format!("{pad}else:"));
                    inner(r, 1, c, lines);
                }
            }
            4 => {
                lines.push(format!("{pad}try:"));
                let n = 1 + r.below(3);
                inner(r, n, c, lines);
                lines.push(format!("{pad}finally:"));
                // `continue` is not allowed in a finally clause.
                let n = 1 + r.below(3);
                inner(
                    r,
                    n,
                    &Context {
                        looping: false,
                        ..*c
                    },
                    lines,
                );
            }
            5 => {
                let alias = if r.chance(50) { " as w" } else { "" };
                lines.push(format!("{pad}with {}{alias}:", expr(r, 2)));
                let n = 1 + r.below(3);
                inner(r, n, c, lines);
            }
            6 => {
                for _ in 0..r.below(3) {
                    lines.push(format!(
                        "{pad}@{}",
                        r.pick(&["dec", "x.dec", "staticmethod"])
                    ));
                }
                let params = match r.below(6) {
                    0 => String::new(),
                    1 => "a".to_owned(),
                    2 => "a, b=1".to_owned(),
                    3 => "a, (b, c)".to_owned(),
                    4 => "*a, **k".to_owned(),
                    _ => format!("a={}", expr(r, 1)),
                };
                lines.push(format!("{pad}def g{}({params}):", r.below(10)));
                let n = 1 + r.below(5);
                inner(
                    r,
                    n,
                    &Context {
                        function: true,
                        looping: false,
                        nested: true,
                    },
                    lines,
                );
            }
            7 => {
                let bases = match r.below(3) {
                    0 => String::new(),
                    1 => "(object)".to_owned(),
                    _ => format!("({})", expr(r, 1)),
                };
                lines.push(format!("{pad}class K{}{bases}:", r.below(10)));
                let n = 1 + r.below(4);
                inner(
                    r,
                    n,
                    &Context {
                        function: false,
                        looping: false,
                        nested: true,
                    },
                    lines,
                );
            }
            _ => {
                let parts: Vec<String> = (0..2 + r.below(3)).map(|_| simple(r, c)).collect();
                lines.push(format!("{pad}{}", parts.join("; ")));
            }
        }
    }
}

fn program(seed: u64, size: usize) -> String {
    let mut r = Rng::new(seed);
    let mut lines = Vec::new();
    if r.chance(50) {
        lines.push("def big(a, b):".to_owned());
        block(
            &mut r,
            1,
            3,
            size,
            &Context {
                function: true,
                looping: false,
                nested: true,
            },
            &mut lines,
        );
    } else {
        block(
            &mut r,
            0,
            3,
            size,
            &Context {
                function: false,
                looping: false,
                nested: false,
            },
            &mut lines,
        );
    }
    lines.join("\n") + "\n"
}

/// Repetitions of one statement (`{i}` is replaced by the line's index),
/// in a function or at module level.
const SHAPES: &[(&str, &str, bool)] = &[
    // Review round 14: the shapes Jython rejected far below the old
    // bound of 10,922 lines.
    (
        "dict-item",
        "d[\"k{i}\"] = {{\"a\": {i}, \"b\": [1, 2, 3]}}",
        true,
    ),
    ("call", "f({i}, \"a\", b)", true),
    ("assign-string", "x{i} = \"v{i}\"", false),
    ("pass", "pass", true),
    ("pass-module", "pass", false),
    ("name", "x", true),
    ("assign", "x = 1", true),
    ("attribute", "x.a = y.b", true),
    ("compare", "if x < {i}: pass", true),
    ("print", "print x, y", false),
    ("import", "import os", true),
    ("tuple", "a, b = c", true),
    ("aug", "x += {i}", true),
    ("listcomp", "x = [i for i in y]", true),
];

fn shape(kind: &str, n: usize) -> String {
    if kind == "elif" {
        // An if/elif chain with `n` branches.
        let mut s = String::from("def shape(x):\n    if x == 0:\n        pass\n");
        for i in 1..n {
            s.push_str(&format!("    elif x == {i}:\n        pass\n"));
        }
        return s;
    }
    if let Some(element) = match kind {
        "constant-list" => Some("{i}"),
        "name-list" => Some("y"),
        _ => None,
    } {
        // A display of 100 elements per statement: Jython builds the
        // constant one in methods of its own.
        let mut s = String::from("def shape():\n");
        for i in 0..n {
            let elts: Vec<String> = (0..100)
                .map(|k| element.replace("{i}", &(i + k).to_string()))
                .collect();
            s.push_str(&format!("    x = [{}]\n", elts.join(", ")));
        }
        return s;
    }
    let &(_, template, function) = SHAPES.iter().find(|s| s.0 == kind).expect("shape");
    let mut s = String::new();
    if function {
        s.push_str("def shape():\n");
    }
    let pad = if function { "    " } else { "" };
    for i in 0..n {
        s.push_str(pad);
        s.push_str(
            &template
                .replace("{i}", &i.to_string())
                .replace("{{", "{")
                .replace("}}", "}"),
        );
        s.push('\n');
    }
    s
}

/// Where the bytes per statement of a shape are measured: at 200 and 201
/// statements, or below half of its limit.
fn probe(limit: usize) -> usize {
    200.min(limit / 2)
}

/// About where Jython starts to fail for each shape (statements).
const SHAPE_LIMITS: &[(&str, usize)] = &[
    ("dict-item", 700),
    ("call", 2_000),
    ("assign-string", 3_100),
    ("pass", 9_400),
    ("pass-module", 9_400),
    ("name", 4_800),
    ("assign", 3_500),
    ("attribute", 2_100),
    ("compare", 1_560),
    ("print", 2_720),
    ("import", 3_000),
    ("tuple", 1_220),
    ("aug", 2_300),
    ("listcomp", 550),
    ("elif", 1_560),
    ("constant-list", 3_000),
    ("name-list", 60),
];

/// Where each shape is measured: from 70% to 150% of about where Jython
/// starts to fail, in steps of 3% (10% for the long display lines), and
/// where [`probe`] measures the bytes per statement.
fn shape_sizes() -> Vec<(&'static str, usize)> {
    let mut out = Vec::new();
    for &(kind, limit) in SHAPE_LIMITS {
        let base = probe(limit);
        out.push((kind, base));
        out.push((kind, base + 1));
        // The displays are long lines: fewer sizes.
        let step = if kind.ends_with("-list") { 10 } else { 3 };
        for percent in (70..=150).step_by(step) {
            out.push((kind, limit * percent / 100));
        }
    }
    out
}

/// Programs close to the limits, for the upper bounds and the margins
/// (reviewer round 18, B2): random function bodies whose lower bound lies
/// around the method limit, many code units (each a code object in the
/// class's `<init>`), many distinct constants and names around the
/// constant pool limit, and the review's repro shape.
fn near_limit_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    // Random statements in one function: 220 to 320 statements of this
    // generator lie around 64 KB.
    for seed in 300_000..300_000 + near_random() {
        let size = 220 + (seed % 100) as usize;
        out.push((format!("near-{seed}-{size}"), program(seed, size)));
    }
    // The review's repro: 2,176 `a = abs(i % 7)` lines, an import first.
    for n in [2_100, 2_150, 2_170, 2_176, 2_180, 2_200] {
        for import in [true, false] {
            let mut s = String::from("def g():\n");
            s.push_str(if import { "    import math\n" } else { "\n" });
            for i in 0..n {
                s.push_str(&format!("    a = abs({})\n", i % 7));
            }
            s.push_str("    c = 1\n    c = 1\n    return a + c\n");
            out.push((format!("repro-{n}-{import}"), s));
        }
    }
    // Code units: `<init>` builds a code object per unit with its names.
    for (n, params) in [
        (500, 8),
        (700, 8),
        (760, 8),
        (800, 8),
        (150, 40),
        (200, 40),
        (220, 40),
    ] {
        let mut s = String::new();
        for i in 0..n {
            let p: Vec<String> = (0..params).map(|j| format!("p{j}")).collect();
            s.push_str(&format!("def g{i}({}):\n    return p0\n", p.join(", ")));
        }
        out.push((format!("units-{n}-{params}"), s));
    }
    // Code objects around the limit of the class initializer: functions
    // with and without parameters, decorators and defaults, classes,
    // nested functions, methods, lambdas, generator expressions and
    // constants, 1,000 to 2,400 units (Jython fails from about 1,300).
    let mut r = Rng::new(7);
    for k in 0..NEAR_INIT {
        let n = 1_000 + r.below(1_400);
        let mut s = String::new();
        for i in 0..n {
            let p = r.below(7);
            let args: Vec<String> = (0..p).map(|j| format!("a{j}")).collect();
            let args = args.join(", ");
            let comma = if args.is_empty() { "" } else { ", " };
            s.push_str(&match r.below(9) {
                0 => format!("def g{i}({args}):\n    return 1\n"),
                1 => format!("@d\ndef g{i}({args}{comma}z=1):\n    return z\n"),
                2 => format!("class K{i}(object):\n    v = 1\n"),
                3 => format!("def g{i}({args}):\n    def h():\n        return 1\n    return h\n"),
                4 => format!("x{i} = lambda {args}: 1\n"),
                5 => format!("x{i} = (q for q in y)\n"),
                6 => format!("c{i} = 's{i}'\n"),
                7 => format!("class M{i}:\n    def m(self{comma}{args}):\n        return self\n"),
                _ => format!("def g{i}(*a, **k):\n    v = a\n    return v\n"),
            });
        }
        out.push((format!("init-{k}-{n}"), s));
    }
    // Distinct constants and names around the constant pool limit.
    for n in [28_000, 31_000, 32_000, 32_400, 32_600, 33_000] {
        for kind in ["strings", "names", "mixed"] {
            let mut s = String::from("def g():\n");
            for chunk in (0..n).collect::<Vec<_>>().chunks(500) {
                let items: Vec<String> = chunk
                    .iter()
                    .map(|i| match (kind, i % 2) {
                        ("strings", _) | ("mixed", 0) => format!("'s{i}'"),
                        _ => format!("n{i}"),
                    })
                    .collect();
                s.push_str(&format!("    x = [{}]\n", items.join(", ")));
            }
            out.push((format!("pool-{kind}-{n}"), s));
        }
    }
    out
}

/// Programs near the limit of the class initializer in
/// [`near_limit_sources`].
const NEAR_INIT: usize = 300;

/// Random programs near the method limit in [`near_limit_sources`]: 400,
/// or `IGLINT_METHOD_SIZE_NEAR` for a larger calibration run (set it for
/// writing the sources and for checking them).
fn near_random() -> u64 {
    std::env::var("IGLINT_METHOD_SIZE_NEAR")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(400)
}

fn sources() -> Vec<(String, String)> {
    let mut out = near_limit_sources();
    for &(start, count, size) in RANDOM {
        for seed in start..start + count {
            out.push((format!("random-{seed}-{size}"), program(seed, size)));
        }
    }
    for (kind, n) in shape_sizes() {
        out.push((format!("shape-{kind}-{n}"), shape(kind, n)));
    }
    out
}

#[test]
#[ignore = "writes the sources for the oracle"]
fn write_sources() {
    big_stack(write_sources_here);
}

fn write_sources_here() {
    let dir = std::env::var("IGLINT_METHOD_SIZE_OUT").expect("IGLINT_METHOD_SIZE_OUT");
    std::fs::create_dir_all(&dir).unwrap();
    let mut cases = String::new();
    for (name, source) in sources() {
        cases.push_str(&format!("### {name}\n{source}"));
    }
    std::fs::write(format!("{dir}/method_size.cases"), cases).unwrap();
}

/// A recorded source: the units Jython generated (method name, bytes), or
/// `None` when the code generator failed, and whether `compile()` passed.
struct Measured {
    units: Option<Vec<(String, usize)>>,
    /// Every method of the class (code units, lambdas and generator
    /// expressions included, and the support methods), in class order.
    methods: Option<Vec<(String, usize)>>,
    /// The constant pool count of the class.
    pool: Option<usize>,
    compiles: bool,
}

fn measured() -> Vec<(String, Measured)> {
    let path = std::env::var("IGLINT_METHOD_SIZE_EXPECTED").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/oracle/method_size.expected"
        )
        .to_owned()
    });
    let text = std::fs::read_to_string(&path).expect("method_size.expected");
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(header) = lines.next() {
        let name = header.strip_prefix("### ").expect("header").to_owned();
        let units = lines
            .next()
            .expect("units")
            .strip_prefix("units ")
            .expect("units");
        let units = (units != "error").then(|| {
            let parts: Vec<&str> = units.split(' ').collect();
            parts
                .chunks(2)
                .map(|p| (p[0].to_owned(), p[1].parse().expect("size")))
                .collect()
        });
        let mut methods = None;
        let mut pool = None;
        let verdict = loop {
            let line = lines.next().expect("verdict");
            if let Some(m) = line.strip_prefix("methods ") {
                let parts: Vec<&str> = m.split(' ').collect();
                methods = Some(
                    parts
                        .chunks(2)
                        .map(|p| (p[0].to_owned(), p[1].parse().expect("size")))
                        .collect(),
                );
            } else if let Some(n) = line.strip_prefix("pool ") {
                pool = Some(n.parse().expect("pool"));
            } else {
                break line;
            }
        };
        let compiles = verdict == "ok";
        out.push((
            name,
            Measured {
                units,
                methods,
                pool,
                compiles,
            },
        ));
    }
    out
}

#[test]
fn bound_never_exceeds_what_jython_generates() {
    big_stack(bound_never_exceeds_here);
}

fn bound_never_exceeds_here() {
    let sources: std::collections::HashMap<String, String> = sources().into_iter().collect();
    let (mut units, mut programs, mut skipped, mut reported) = (0, 0, 0, 0);
    let mut margins = Margins::default();
    let mut violations: Vec<String> = Vec::new();
    let mut tightness: Vec<f64> = Vec::new();
    for (name, m) in measured() {
        let source = &sources[&name];
        let parsed = iglint_python::parse_module(source);
        if !parsed.errors.is_empty() {
            assert!(
                !m.compiles,
                "{name}: Jython compiles what iglint cannot parse"
            );
            skipped += 1;
            continue;
        }
        let errors = size_errors(&parsed.module, source);
        if errors
            .iter()
            .any(|e| e.kind == CompileErrorKind::MethodTooLarge)
        {
            reported += 1;
            assert!(!m.compiles, "{name}: reported, but Jython compiles it");
        }
        violations.extend(check_upper(&name, &parsed.module, source, &m, &mut margins));
        let Some(sizes) = &m.units else {
            continue;
        };
        programs += 1;
        // The module and the `def` and `class` bodies, matched in order
        // (the generated names are not `f`). Lambdas and generator
        // expressions are not modelled (their bound is 0), and Jython may
        // number them in another order than the bound lists them.
        let bounds: Vec<_> = code_units(&parsed.module, source)
            .into_iter()
            .enumerate()
            .filter(|(i, u)| *i == 0 || u.owner.is_some())
            .map(|(_, u)| u)
            .collect();
        // Jython may generate a unit more often than the bound lists it
        // (a `def` in a `finally` body is compiled once per way out of
        // the `try`), and the copies come in an order of their own: per
        // name, the bounds sorted must be at most the sizes sorted, which
        // holds when every bound is at most the size of its own unit.
        let mut real: std::collections::BTreeMap<&str, Vec<usize>> = Default::default();
        for (n, size) in sizes {
            real.entry(n.as_str()).or_default().push(*size);
        }
        let mut lower: std::collections::BTreeMap<&str, Vec<usize>> = Default::default();
        for bound in &bounds {
            lower.entry(bound.name).or_default().push(bound.bytes);
        }
        for (unit, bounds) in &mut lower {
            let Some(sizes) = real.get_mut(unit) else {
                panic!("{name}: no unit `{unit}` where the bound has one");
            };
            assert!(
                sizes.len() >= bounds.len(),
                "{name}: fewer units `{unit}` than bounds"
            );
            bounds.sort_unstable_by(|x, y| y.cmp(x));
            sizes.sort_unstable_by(|x, y| y.cmp(x));
            for (bound, size) in bounds.iter().zip(sizes.iter()) {
                units += 1;
                assert!(
                    bound <= size,
                    "{name}: unit `{unit}` has {size} bytes, the bound says {bound}"
                );
                if *size >= 1000 {
                    tightness.push(*bound as f64 / *size as f64);
                }
            }
        }
    }
    tightness.sort_by(f64::total_cmp);
    let at = |q: f64| tightness[((tightness.len() - 1) as f64 * q) as usize];
    eprintln!(
        "{programs} programs, {units} code units, {skipped} not parsed, {reported} reported; \
         bound / size of units of 1,000 bytes or more: min {:.2}, median {:.2}, max {:.2}",
        at(0.0),
        at(0.5),
        at(1.0)
    );
    margins.report();
    assert!(
        violations.is_empty(),
        "{} upper bounds below Jython's size, the first:\n{}",
        violations.len(),
        {
            let mut v = violations.clone();
            v.sort_by_key(|l| l.split(':').nth(1).map(str::to_owned));
            v.dedup_by_key(|l| {
                l.split(':')
                    .nth(1)
                    .map(|t| t.split(" has").next().unwrap_or("").to_owned())
            });
            v[..v.len().min(60)].join("\n")
        }
    );
    assert!(programs > 2_000, "{programs}");
}

/// How close the upper bounds come to what Jython generates.
#[derive(Default)]
struct Margins {
    /// upper / size of methods of 1,000 bytes or more.
    methods: Vec<f64>,
    /// upper / count of constant pools.
    pools: Vec<f64>,
    /// lower bound / size of `<init>` of 30,000 bytes or more.
    init: Vec<f64>,
    /// Programs Jython rejects for their size, and those `SCR002` reports.
    rejected: usize,
    rejected_reported: usize,
    /// The same for the programs near the class initializer's limit.
    init_rejected: (usize, usize),
    /// Programs whose verdict the bounds leave open, and programs whose
    /// bounds decide it.
    unknown: usize,
    known: usize,
    /// The smallest distance in bytes from a measured unit of more than
    /// 60,000 bytes to its upper bound.
    closest: Option<usize>,
}

impl Margins {
    fn report(&self) {
        let q = |v: &[f64], f: f64| {
            let mut v = v.to_vec();
            v.sort_by(f64::total_cmp);
            v.get(((v.len().max(1) - 1) as f64 * f) as usize)
                .copied()
                .unwrap_or(0.0)
        };
        eprintln!(
            "lower / size of <init> of 30,000 bytes or more: min {:.2}, median {:.2}, max {:.2}; upper / size of methods of 1,000 bytes or more: min {:.2}, median {:.2}, max {:.2} ({} methods); upper / count of constant pools: min {:.2}, median {:.2}, max {:.2}; verdict open for {} programs, decided for {}; Jython rejects {} for their size, SCR002 reports {} (near the class initializer's limit: {} and {}); closest upper bound above a unit of more than 60,000 bytes: {:?} bytes",
            q(&self.init, 0.0),
            q(&self.init, 0.5),
            q(&self.init, 1.0),
            q(&self.methods, 0.0),
            q(&self.methods, 0.5),
            q(&self.methods, 1.0),
            self.methods.len(),
            q(&self.pools, 0.0),
            q(&self.pools, 0.5),
            q(&self.pools, 1.0),
            self.unknown,
            self.known,
            self.rejected,
            self.rejected_reported,
            self.init_rejected.0,
            self.init_rejected.1,
            self.closest
        );
    }
}

/// The upper bounds of every Java method and of the constant pool are at
/// least what Jython generates (reviewer round 18, B2), and a program
/// whose verdict the bounds decide gets that verdict.
fn check_upper(
    name: &str,
    module: &iglint_python::ast::Module,
    source: &str,
    m: &Measured,
    margins: &mut Margins,
) -> Vec<String> {
    let mut out = Vec::new();
    let open = size_unknown(module, source).is_some();
    let reported = !size_errors(module, source).is_empty();
    if !m.compiles && m.units.is_none() {
        margins.rejected += 1;
        margins.rejected_reported += usize::from(reported);
        if name.starts_with("init-") {
            margins.init_rejected.0 += 1;
            margins.init_rejected.1 += usize::from(reported);
        }
    }
    if open {
        margins.unknown += 1;
    } else {
        margins.known += 1;
        // Decided: SCR002 reports exactly the programs Jython rejects for
        // their size (other compile errors aside).
        if m.compiles {
            assert!(!reported, "{name}: reported, but Jython compiles it");
        } else if m.units.is_none() {
            // The code generator failed: the class is too large.
            assert!(
                reported,
                "{name}: Jython rejects it, the bounds say it fits"
            );
        }
    }
    let (Some(methods), Some(pool)) = (&m.methods, m.pool) else {
        return out;
    };
    let class = upper::class(module, source);
    if class.pool < pool {
        out.push(format!("{name}: pool {pool}, upper {}", class.pool));
    }
    margins.pools.push(class.pool as f64 / pool as f64);
    let init = methods
        .iter()
        .find(|(n, _)| n == "<init>")
        .map_or(0, |m| m.1);
    let (lower, _) = init_lower(module, source);
    if lower > init {
        out.push(format!(
            "{name}: `<init>` has {init} bytes, the lower bound says {lower}"
        ));
    }
    if init >= 30_000 {
        margins.init.push(lower as f64 / init as f64);
    }
    for (method, bound) in [
        ("<init>", class.init),
        ("call_function", class.call_function),
    ] {
        let size = methods.iter().find(|(n, _)| n == method).map_or(0, |m| m.1);
        if bound < size {
            out.push(format!(
                "{name}: `{method}` has {size} bytes, upper {bound}"
            ));
        }
    }
    // Every code unit, lambdas and generator expressions included, by
    // name: Jython numbers lambdas and nested units in an order of its own
    // and compiles a unit in a `finally` body once per way out, so each
    // measured method must be at most the largest bound of its name, and
    // where Jython compiled each unit once, the sizes sorted must be at
    // most the bounds sorted (a necessary condition for every unit's
    // bound to hold).
    let mut real: std::collections::BTreeMap<&str, Vec<usize>> = Default::default();
    for (n, size) in methods.iter().filter(|(n, _)| n.starts_with("set$$")) {
        if class.displays < *size {
            out.push(format!(
                "{name}: `{n}` has {size} bytes, upper {}",
                class.displays
            ));
        }
    }
    for (n, size) in methods
        .iter()
        .filter(|(n, _)| n.contains('$') && !n.starts_with("set$$"))
    {
        real.entry(n.rsplit_once('$').map_or(n.as_str(), |p| p.0))
            .or_default()
            .push(*size);
    }
    let units = code_units(module, source);
    let mut bounds: std::collections::BTreeMap<&str, Vec<usize>> = Default::default();
    for unit in &units {
        bounds.entry(unit.name).or_default().push(unit.upper);
    }
    for (unit, sizes) in &mut real {
        let Some(b) = bounds.get_mut(unit) else {
            out.push(format!("{name}: method `{unit}` without a bound"));
            continue;
        };
        sizes.sort_unstable_by(|x, y| y.cmp(x));
        b.sort_unstable_by(|x, y| y.cmp(x));
        let pairs: Vec<(usize, usize)> = if sizes.len() == b.len() {
            sizes.iter().copied().zip(b.iter().copied()).collect()
        } else {
            vec![(sizes[0], b[0])]
        };
        for (size, bound) in pairs {
            if bound < size {
                out.push(format!(
                    "{name}: unit `{unit}` has {size} bytes, the upper bound says {bound}"
                ));
            }
            if size >= 1_000 {
                margins.methods.push(bound as f64 / size as f64);
            }
            if size > 60_000 {
                let gap = bound.saturating_sub(size);
                margins.closest = Some(margins.closest.map_or(gap, |c| c.min(gap)));
            }
        }
    }
    out
}

/// The shapes are reported close to where Jython starts to reject them.
#[test]
fn shapes_are_reported_near_jythons_limit() {
    big_stack(shapes_are_reported_here);
}

fn shapes_are_reported_here() {
    let measured: std::collections::HashMap<String, Measured> = measured().into_iter().collect();
    for &(kind, _) in SHAPE_LIMITS {
        let sizes = shape_sizes();
        let sizes: Vec<usize> = sizes
            .iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, n)| *n)
            .collect();
        let verdict = |n: usize| measured[&format!("shape-{kind}-{n}")].compiles;
        let reports = |n: usize| {
            let source = shape(kind, n);
            let parsed = iglint_python::parse_module(&source);
            size_errors(&parsed.module, &source)
                .iter()
                .any(|e| e.kind == CompileErrorKind::MethodTooLarge)
        };
        // Jython's limit from the bytes per statement: the shape's unit
        // grows linearly with the number of statements.
        let unit = |n: usize| -> usize {
            let units = measured[&format!("shape-{kind}-{n}")]
                .units
                .as_ref()
                .expect("units");
            units
                .iter()
                .find(|(name, _)| name == "shape")
                .or(units.first())
                .expect("unit")
                .1
        };
        let approximate = SHAPE_LIMITS.iter().find(|s| s.0 == kind).expect("shape").1;
        let base = probe(approximate);
        let (at_base, per) = (unit(base), unit(base + 1) - unit(base));
        let first_fail = sizes
            .iter()
            .copied()
            .filter(|&n| !verdict(n))
            .min()
            .expect("fails");
        let last_ok = sizes
            .iter()
            .copied()
            .filter(|&n| verdict(n))
            .max()
            .expect("compiles");
        // An `elif` nests one `if` deeper per branch, which costs more
        // than the branch before it: not linear, the sample decides.
        let limit = if kind == "elif" {
            first_fail
        } else {
            base + (65_535 - at_base) / per + 1
        };
        assert!(
            last_ok < limit && limit <= first_fail,
            "{kind}: limit {limit}, {last_ok} compiles, {first_fail} fails"
        );
        let first_report = sizes.iter().copied().filter(|&n| reports(n)).min();
        eprintln!(
            "{kind}: {per} bytes per statement, Jython fails from {limit} statements, iglint reports from {} (sampled)",
            first_report.map_or("none".to_owned(), |n| n.to_string())
        );
        let first_report = first_report.unwrap_or_else(|| panic!("{kind}: never reported"));
        assert!(
            first_report * 100 <= limit * 150,
            "{kind}: Jython fails from {limit} statements, iglint reports only from {first_report}"
        );
    }
}

/// How many `.py` files below `IGLINT_SIZE_SCAN` (a private corpus, read
/// only) have a size verdict the bounds leave open: aggregate counts only.
#[test]
#[ignore = "needs IGLINT_SIZE_SCAN"]
fn open_size_verdicts_in_a_corpus() {
    let Some(root) = std::env::var_os("IGLINT_SIZE_SCAN") else {
        return;
    };
    let mut stack = vec![std::path::PathBuf::from(root)];
    let (mut files, mut open, mut largest) = (0, 0, 0);
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                if !path.ends_with(".git") {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "py") {
                let Ok(source) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let parsed = iglint_python::parse_module(&source);
                if !parsed.errors.is_empty() {
                    continue;
                }
                files += 1;
                open += usize::from(size_unknown(&parsed.module, &source).is_some());
                for unit in code_units(&parsed.module, &source) {
                    largest = largest.max(unit.upper);
                }
            }
        }
    }
    eprintln!("{files} files, {open} with an open size verdict, largest upper bound {largest}");
}
