//! Size limits against IA's Jython.
//!
//! The sources are generated (they are up to megabytes long); `CASES`
//! records what Jython's full `compile()` made of each, measured with
//! `oracle/run-oracle.sh --compile`. To measure again, write the sources
//! with
//!
//! ```sh
//! IGLINT_LIMITS_OUT=$HOME/.cache/limits cargo test -p iglint_python_semantic \
//!     --test limits_oracle -- --ignored write_sources
//! (cd $HOME/.cache/limits && ls *.py > list.txt)
//! oracle/run-oracle.sh --compile $HOME/.cache/limits $HOME/.cache/limits/list.txt out.txt
//! ```
//!
//! iglint must report exactly Jython's `Mark invalid` failures, and a size
//! error only where Jython fails too.

use iglint_python_semantic::{CompileErrorKind, MARK_LIMIT, mark_limit_error, size_errors};

const HEADER: &str = "def runAction(self, event):\n";

fn source(kind: &str, n: usize) -> String {
    match kind {
        "comment" => format!("#{}\nx=1\n", "a".repeat(n)),
        "single" => "#".repeat(n),
        "single-nl" => format!("{}\n", "#".repeat(n)),
        "crlf" => format!("#{}\r\nx=1\r\n", "a".repeat(n)),
        "cr" => format!("#{}\rx=1\r", "a".repeat(n)),
        "eacute" => format!("#{}\n", "é".repeat(n)),
        "astral" => format!("#{}\n", "\u{1F600}".repeat(n)),
        "list" => format!("x = [{}1]\n", "1,".repeat(n)),
        "line-2" => format!("x = 1\ny = '{}'\n", "a".repeat(n)),
        "view" => format!("{HEADER}\t#{}\n\tpass\n", "b".repeat(n)),
        "line-3" => format!("x = 1\ny = 2\nz = u'{}'\n", "\u{e9}".repeat(n)),
        "pass" => "pass\n".repeat(n),
        "assign" => "x = 1\n".repeat(n),
        "function" => format!("def f():\n{}", "    pass\n".repeat(n)),
        "if" => format!("if x:\n{}", "    pass\n".repeat(n)),
        "view-body" => format!("{HEADER}{}", "\tpass\n".repeat(n)),
        "global" => format!("def f():\n{}    pass\n", "    global x\n".repeat(n)),
        // Code after a statement that ends a suite: Jython generates none.
        "dead-return" => format!("def f():\n    return\n{}", "    pass\n".repeat(n)),
        "dead-raise" => format!("def f():\n    raise X\n{}", "    pass\n".repeat(n)),
        "dead-continue" => format!(
            "def f():\n    for i in x:\n        continue\n{}",
            "        pass\n".repeat(n)
        ),
        "dead-if-else" => format!(
            "def f():\n    if x:\n        return\n    elif y:\n        raise X\n    else:\n        return\n{}",
            "    pass\n".repeat(n)
        ),
        "dead-class" => format!("class C:\n    raise X\n{}", "    pass\n".repeat(n)),
        "dead-nested" => format!("if x:\n    raise X\n{}", "    pass\n".repeat(n)),
        "dead-view" => format!("{HEADER}\treturn\n{}", "\tpass\n".repeat(n)),
        "dead-def" => format!(
            "def f():\n    return\n    def g():\n{}",
            "        pass\n".repeat(n)
        ),
        "dead-strings" => format!(
            "def f():\n    return\n{}",
            (0..n)
                .map(|i| format!("'s{i}'"))
                .collect::<Vec<_>>()
                .chunks(1000)
                .map(|c| format!("    x = [{}]\n", c.join(", ")))
                .collect::<String>()
        ),
        "dead-string-lines" => format!(
            "def f():\n    return\n{}",
            (0..n).map(|i| format!("    's{i}'\n")).collect::<String>()
        ),
        "dead-try-body" => format!(
            "def f():\n    try:\n        return\n{}    finally:\n        pass\n",
            "        pass\n".repeat(n)
        ),
        // Code Jython does generate.
        "live-for-break" => format!(
            "def f():\n    for i in x:\n        break\n{}",
            "        pass\n".repeat(n)
        ),
        "live-try-finally" => format!(
            "def f():\n    try:\n        return\n    finally:\n        pass\n{}",
            "    pass\n".repeat(n)
        ),
        "live-with" => format!(
            "def f():\n    with x:\n        return\n{}",
            "    pass\n".repeat(n)
        ),
        "live-if" => format!(
            "def f():\n    if x:\n        return\n{}",
            "    pass\n".repeat(n)
        ),
        "live-break" => format!(
            "def f():\n    while x:\n        break\n{}",
            "        pass\n".repeat(n)
        ),
        "live-try" => format!(
            "def f():\n    try:\n        return\n    except:\n        return\n{}",
            "    pass\n".repeat(n)
        ),
        "live-module" => format!("raise X\n{}", "pass\n".repeat(n)),
        "live-same-line" => "pass; ".repeat(n * 10) + "\n",
        // Names: only the Java method of a `def` or `class` body, named
        // `<name>$<id>`, is limited (65,535 bytes of a class file
        // constant); every other position is loaded in chunks. The first
        // two lines stay short.
        "def-name" => format!("\n\ndef {}():\n    return 1\n", "n".repeat(n)),
        "class-name" => format!("\n\nclass {}:\n    pass\n", "n".repeat(n)),
        "method-name" => format!(
            "\n\nclass K:\n    def {}(self):\n        return 1\n",
            "n".repeat(n)
        ),
        "private-method" => format!(
            "\n\nclass K:\n    def __{}(self):\n        return 1\n",
            "n".repeat(n)
        ),
        "nested-def" => format!(
            "\n\ndef f():\n    def {}():\n        return 1\n    return 1\n",
            "n".repeat(n)
        ),
        "dead-def-name" => format!(
            "\n\ndef f():\n    return\n    def {}():\n        pass\n",
            "n".repeat(n)
        ),
        "name-positions" => {
            let v = "v".repeat(n);
            format!(
                "\n\n{v} = 1\nprint {v}\ndef g({v}, o, *a{v}, **k{v}):\n    '{v}'\n    \
                 global g{v}\n    import {v}.{v} as i{v}\n    from {v} import {v} as f{v}\n    \
                 o.{v} = o.{v}\n    o.{v}(1, {v}=2)\n    l = lambda {v}: {v}\n    \
                 x = u'{v}' + '{v}'\n    def h():\n        return {v}\n    return h\n"
            )
        }
        "strings" => (0..n)
            .map(|i| format!("'s{i}'"))
            .collect::<Vec<_>>()
            .chunks(1000)
            .map(|c| format!("x = [{}]\n", c.join(", ")))
            .collect(),
        _ => unreachable!("{kind}"),
    }
}

/// Kind, size, whether Jython compiles it.
const CASES: &[(&str, usize, bool)] = &[
    ("assign", 10921, false),
    ("assign", 10922, false),
    ("assign", 10923, false),
    ("assign", 10924, false),
    ("astral", 49998, true),
    ("astral", 49999, false),
    ("astral", 50000, false),
    ("astral", 50001, false),
    ("comment", 99993, true),
    ("comment", 99994, true),
    ("comment", 99995, false),
    ("comment", 99996, false),
    ("cr", 99993, true),
    ("cr", 99994, true),
    ("cr", 99995, false),
    ("cr", 99996, false),
    ("crlf", 99992, true),
    ("crlf", 99993, true),
    ("crlf", 99994, false),
    ("crlf", 99995, false),
    ("eacute", 99996, true),
    ("eacute", 99997, true),
    ("eacute", 99998, false),
    ("eacute", 99999, false),
    ("function", 10921, false),
    ("function", 10922, false),
    ("function", 10923, false),
    ("function", 10924, false),
    ("global", 30000, true),
    ("if", 10921, false),
    ("if", 10922, false),
    ("if", 10923, false),
    ("if", 10924, false),
    ("line-2", 99986, true),
    ("line-2", 99987, true),
    ("line-2", 99988, false),
    ("line-2", 99989, false),
    ("line-3", 300000, true),
    ("list", 49993, true),
    ("list", 49994, true),
    ("list", 49995, true),
    ("list", 49996, false),
    ("pass", 10921, false),
    ("pass", 10922, false),
    ("pass", 10923, false),
    ("pass", 10924, false),
    ("single", 99997, true),
    ("single", 99998, true),
    ("single", 99999, false),
    ("single", 100000, false),
    ("single-nl", 99997, true),
    ("single-nl", 99998, true),
    ("single-nl", 99999, false),
    ("single-nl", 100000, false),
    ("strings", 32766, false),
    ("strings", 32767, false),
    ("strings", 32768, false),
    ("strings", 32769, false),
    ("view", 99968, true),
    ("view", 99969, true),
    ("view", 99970, false),
    ("view", 99971, false),
    ("view-body", 10921, false),
    ("view-body", 10922, false),
    ("view-body", 10923, false),
    ("view-body", 10924, false),
    ("dead-class", 10921, true),
    ("dead-class", 10922, true),
    ("dead-class", 10923, true),
    ("dead-class", 10924, true),
    ("dead-continue", 10921, true),
    ("dead-continue", 10922, true),
    ("dead-continue", 10923, true),
    ("dead-continue", 10924, true),
    ("dead-def", 10921, true),
    ("dead-def", 10922, true),
    ("dead-def", 10923, true),
    ("dead-def", 10924, true),
    ("dead-if-else", 10921, true),
    ("dead-if-else", 10922, true),
    ("dead-if-else", 10923, true),
    ("dead-if-else", 10924, true),
    ("dead-nested", 10921, true),
    ("dead-nested", 10922, true),
    ("dead-nested", 10923, true),
    ("dead-nested", 10924, true),
    ("dead-raise", 10921, true),
    ("dead-raise", 10922, true),
    ("dead-raise", 10923, true),
    ("dead-raise", 10924, true),
    ("dead-return", 10921, true),
    ("dead-return", 10922, true),
    ("dead-return", 10923, true),
    ("dead-return", 10924, true),
    ("dead-strings", 32766, true),
    ("dead-strings", 32767, true),
    ("dead-strings", 32768, true),
    ("dead-strings", 32769, true),
    ("dead-view", 10921, true),
    ("dead-view", 10922, true),
    ("dead-view", 10923, true),
    ("dead-view", 10924, true),
    ("live-break", 10921, false),
    ("live-break", 10922, false),
    ("live-break", 10923, false),
    ("live-break", 10924, false),
    ("live-if", 10921, false),
    ("live-if", 10922, false),
    ("live-if", 10923, false),
    ("live-if", 10924, false),
    ("live-module", 10921, false),
    ("live-module", 10922, false),
    ("live-module", 10923, false),
    ("live-module", 10924, false),
    ("live-same-line", 2000, false),
    ("live-try", 10921, false),
    ("live-try", 10922, false),
    ("live-try", 10923, false),
    ("live-try", 10924, false),
    ("dead-string-lines", 32766, true),
    ("dead-string-lines", 32767, true),
    ("dead-string-lines", 32768, true),
    ("dead-string-lines", 32769, true),
    ("dead-try-body", 10921, true),
    ("dead-try-body", 10922, true),
    ("dead-try-body", 10923, true),
    ("dead-try-body", 10924, true),
    ("live-for-break", 10921, false),
    ("live-for-break", 10922, false),
    ("live-for-break", 10923, false),
    ("live-for-break", 10924, false),
    ("live-try-finally", 10921, false),
    ("live-try-finally", 10922, false),
    ("live-try-finally", 10923, false),
    ("live-try-finally", 10924, false),
    ("live-with", 10921, false),
    ("live-with", 10922, false),
    ("live-with", 10923, false),
    ("live-with", 10924, false),
    // `java.lang.IllegalArgumentException: UTF8 string too large`.
    ("def-name", 65532, true),
    ("def-name", 65533, true),
    ("def-name", 65534, false),
    ("def-name", 65535, false),
    ("class-name", 65532, true),
    ("class-name", 65533, true),
    ("class-name", 65534, false),
    ("class-name", 65535, false),
    ("method-name", 65532, true),
    ("method-name", 65533, true),
    ("method-name", 65534, false),
    ("method-name", 65535, false),
    ("nested-def", 65532, true),
    ("nested-def", 65533, true),
    ("nested-def", 65534, false),
    ("nested-def", 65535, false),
    ("private-method", 65528, true),
    ("private-method", 65529, true),
    ("private-method", 65530, false),
    ("private-method", 65531, false),
    ("dead-def-name", 70000, true),
    ("name-positions", 70000, true),
    ("name-positions", 1000000, true),
];

/// The sources whose only possible failure is a name too long for a Java
/// method name: `SCR002` reports exactly those Jython rejects.
const NAME_KINDS: &[&str] = &[
    "def-name",
    "class-name",
    "method-name",
    "private-method",
    "nested-def",
    "dead-def-name",
    "name-positions",
];

/// The sources whose only possible failure is the first-two-lines limit.
const MARK_KINDS: &[&str] = &[
    "comment",
    "single",
    "single-nl",
    "crlf",
    "cr",
    "eacute",
    "astral",
    "list",
    "line-2",
    "view",
    "line-3",
];

fn name(kind: &str, n: usize) -> String {
    format!("{kind}-{n}.py")
}

/// The sizes measured: around every boundary.
fn sizes() -> Vec<(&'static str, usize)> {
    let m = MARK_LIMIT;
    let mut out = Vec::new();
    let around = |kind: &'static str, at: usize, out: &mut Vec<(&'static str, usize)>| {
        for n in [at - 2, at - 1, at, at + 1] {
            out.push((kind, n));
        }
    };
    around("comment", m - 5, &mut out);
    around("single", m - 1, &mut out);
    around("single-nl", m - 1, &mut out);
    around("crlf", m - 6, &mut out);
    around("cr", m - 5, &mut out);
    around("eacute", m - 2, &mut out);
    around("astral", m / 2, &mut out);
    around("list", (m - 9) / 2, &mut out);
    around("line-2", m - 12, &mut out);
    around("view", m - 30, &mut out);
    out.push(("line-3", 3 * m));
    for kind in ["pass", "assign", "function", "if", "view-body"] {
        around(kind, 10_923, &mut out);
    }
    out.push(("global", 30_000));
    around("strings", 32_768, &mut out);
    for kind in [
        "dead-return",
        "dead-raise",
        "dead-continue",
        "dead-if-else",
        "dead-class",
        "dead-nested",
        "dead-view",
        "dead-def",
        "live-if",
        "live-break",
        "live-try",
        "live-module",
        "dead-try-body",
        "live-for-break",
        "live-try-finally",
        "live-with",
    ] {
        around(kind, 10_923, &mut out);
    }
    around("dead-strings", 32_768, &mut out);
    around("dead-string-lines", 32_768, &mut out);
    out.push(("live-same-line", 2_000));
    for kind in ["def-name", "class-name", "method-name", "nested-def"] {
        around(kind, 65_534, &mut out);
    }
    around("private-method", 65_530, &mut out);
    out.push(("dead-def-name", 70_000));
    out.push(("name-positions", 70_000));
    out.push(("name-positions", 1_000_000));
    out
}

fn verdicts(source: &str) -> (bool, bool) {
    let mark = mark_limit_error(source).is_some();
    let parsed = iglint_python::parse_module(source);
    let size = parsed.errors.is_empty()
        && size_errors(&parsed.module, source).iter().any(|e| {
            matches!(
                e.kind,
                CompileErrorKind::MethodTooLarge
                    | CompileErrorKind::ClassTooLarge
                    | CompileErrorKind::NameTooLong
            )
        });
    (mark, size)
}

#[test]
#[ignore = "writes the sources for the oracle"]
fn write_sources() {
    let dir = std::env::var("IGLINT_LIMITS_OUT").expect("IGLINT_LIMITS_OUT");
    std::fs::create_dir_all(&dir).unwrap();
    for (kind, n) in sizes() {
        std::fs::write(format!("{dir}/{}", name(kind, n)), source(kind, n)).unwrap();
    }
}

#[test]
fn limits_match_jython() {
    let mut measured: Vec<(&str, usize)> = CASES.iter().map(|&(k, n, _)| (k, n)).collect();
    let mut generated = sizes();
    measured.sort_unstable();
    generated.sort_unstable();
    assert_eq!(measured, generated);
    for &(kind, n, compiles) in CASES {
        let source = source(kind, n);
        let (mark, size) = verdicts(&source);
        let which = name(kind, n);
        if MARK_KINDS.contains(&kind) {
            // The first-two-lines limit is exact.
            assert_eq!(mark, !compiles, "{which}");
        }
        if NAME_KINDS.contains(&kind) {
            assert_eq!(size, !compiles, "{which}");
        }
        if mark || size {
            assert!(!compiles, "{which}: iglint reports what Jython compiles");
        }
    }
}
