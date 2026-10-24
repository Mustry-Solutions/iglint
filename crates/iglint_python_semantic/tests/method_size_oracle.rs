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

use iglint_python_semantic::{CompileErrorKind, code_units, size_errors};

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

fn sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
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
        let compiles = lines.next().expect("verdict") == "ok";
        out.push((name, Measured { units, compiles }));
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
        let sizes: Vec<&(String, usize)> = sizes.iter().collect();
        // Jython may generate a unit more often than the bound lists it
        // (a `def` in a `finally` body is compiled once per way out of
        // the `try`): each bound is matched with the next unit of its name.
        let mut next = 0;
        for bound in &bounds {
            let Some(at) = sizes[next..].iter().position(|(n, _)| n == bound.name) else {
                panic!("{name}: no unit `{}` where the bound has one", bound.name);
            };
            let (unit, size) = sizes[next + at];
            next += at + 1;
            units += 1;
            assert!(
                bound.bytes <= *size,
                "{name}: unit `{unit}` has {size} bytes, the bound says {}",
                bound.bytes
            );
            if *size >= 1000 {
                tightness.push(bound.bytes as f64 / *size as f64);
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
    assert!(programs > 2_000, "{programs}");
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
