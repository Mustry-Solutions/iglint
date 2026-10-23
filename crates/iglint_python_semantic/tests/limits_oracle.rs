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
    out
}

fn verdicts(source: &str) -> (bool, bool) {
    let mark = mark_limit_error(source).is_some();
    let parsed = iglint_python::parse_module(source);
    let size = parsed.errors.is_empty()
        && size_errors(&parsed.module, source).iter().any(|e| {
            matches!(
                e.kind,
                CompileErrorKind::MethodTooLarge | CompileErrorKind::ClassTooLarge
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
        if mark || size {
            assert!(!compiles, "{which}: iglint reports what Jython compiles");
        }
    }
}
