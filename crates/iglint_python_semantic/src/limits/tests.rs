use super::*;

fn mark(source: &str) -> Option<(String, usize)> {
    mark_limit_error(source).map(|e| {
        let text = e.range.get(source).unwrap();
        (text.chars().take(3).collect(), text.len())
    })
}

#[test]
fn first_two_lines() {
    let n = MARK_LIMIT;
    // Line 2 ends at index 100,000 or later: fails.
    let comment = |len: usize| format!("#{}\nx=1\n", "a".repeat(len));
    // "#" + len + "\n" + "x=1" ends line 2 at len + 5.
    assert!(mark(&comment(n - 6)).is_none());
    assert_eq!(mark(&comment(n - 5)), Some(("#aa".to_owned(), n - 4)));
    // A single line: the appended newlines make an empty line 2.
    let single = |len: usize| "#".repeat(len);
    assert!(mark(&single(n - 2)).is_none());
    assert!(mark(&single(n - 1)).is_some());
    assert!(mark(&(single(n - 2) + "\n")).is_none());
    assert!(mark(&(single(n - 1) + "\n")).is_some());
    // Line 2 is the long one.
    let header = "def runAction(self, event):\n";
    let body = |len: usize| format!("{header}\t#{}\n\tpass\n", "b".repeat(len));
    let fits = n - header.len() - 2;
    assert!(mark(&body(fits - 1)).is_none());
    assert_eq!(mark(&body(fits)).unwrap().0, "\t#b");
    // UTF-16 units: an astral character counts twice, é once.
    assert!(mark(&format!("#{}\n", "é".repeat(n - 3))).is_none());
    assert!(mark(&format!("#{}\n", "😀".repeat(n / 2))).is_some());
    // `\r\n` after line 1 counts twice; after line 2 only the `\r`.
    let crlf = |len: usize| format!("#{}\r\nx=1\r\n", "a".repeat(len));
    assert!(mark(&crlf(n - 7)).is_none());
    assert!(mark(&crlf(n - 6)).is_some());
    // Later lines are not limited.
    assert!(mark(&format!("x=1\ny=2\n#{}\n", "a".repeat(10 * n))).is_none());
}

fn sizes(source: &str) -> Vec<CompileErrorKind> {
    let parsed = iglint_python::parse_module(source);
    assert!(parsed.errors.is_empty());
    size_errors(&parsed.module, source)
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

#[test]
fn method_size() {
    let lines = |n: usize| "pass\n".repeat(n);
    // 5 bytes for lines 1 to 6, 6 to line 128, 7 beyond: 9,381 lines
    // stay within 65,535 bytes (Jython fails from 9,380).
    assert!(sizes(&lines(9_381)).is_empty());
    assert_eq!(sizes(&lines(9_382)), [CompileErrorKind::MethodTooLarge]);
    // Every statement updates the line number, also on one line.
    assert!(sizes(&"pass; ".repeat(13_107)).is_empty());
    assert_eq!(
        sizes(&"pass; ".repeat(13_108)),
        [CompileErrorKind::MethodTooLarge]
    );
    // `global` generates nothing.
    assert!(sizes(&format!("def f():\n{}", "    global x\n".repeat(30_000))).is_empty());
    // Function bodies are their own methods; nested blocks are not.
    let function = format!("def f():\n{}", "    pass\n".repeat(9_400));
    assert_eq!(
        sizes(&format!("{function}{function}")),
        [
            CompileErrorKind::MethodTooLarge,
            CompileErrorKind::MethodTooLarge
        ]
    );
    let half = "    pass\n".repeat(4_700);
    assert_eq!(
        sizes(&format!("if x:\n{half}else:\n{half}")),
        [CompileErrorKind::MethodTooLarge]
    );
    // Expressions count: 681 statements of this shape fail on Jython
    // (97 bytes each); 742 are certainly too large.
    let items = |n: usize| -> String {
        (0..n)
            .map(|i| format!("    d[\"k{i}\"] = {{\"a\": {i}, \"b\": [1, 2, 3]}}\n"))
            .collect()
    };
    assert!(sizes(&format!("def f():\n{}", items(679))).is_empty());
    assert_eq!(
        sizes(&format!("def f():\n{}", items(900))),
        [CompileErrorKind::MethodTooLarge]
    );
}

#[test]
fn dead_code_is_not_generated() {
    let passes = |indent: &str| format!("{indent}pass\n").repeat(9_500);
    for dead in [
        format!("def f():\n    return\n{}", passes("    ")),
        format!("def f():\n    raise X\n{}", passes("    ")),
        format!(
            "def f():\n    for i in x:\n        continue\n{}",
            passes("        ")
        ),
        format!(
            "def f():\n    if x:\n        return\n    elif y:\n        raise X\n    else:\n        return\n{}",
            passes("    ")
        ),
        format!("class C:\n    raise X\n{}", passes("    ")),
        format!("if x:\n    raise X\n{}", passes("    ")),
        format!("def f():\n    return\n    def g():\n{}", passes("        ")),
        // Dead code inside a `try` body: the `try` itself goes on.
        format!(
            "def f():\n    try:\n        return\n{}    finally:\n        pass\n",
            passes("        ")
        ),
    ] {
        assert!(sizes(&dead).is_empty(), "{}", dead.get(..60).unwrap());
    }
    for live in [
        format!("raise X\n{}", passes("")),
        format!("def f():\n    if x:\n        return\n{}", passes("    ")),
        format!(
            "def f():\n    while x:\n        break\n{}",
            passes("        ")
        ),
        format!(
            "def f():\n    try:\n        return\n    except:\n        return\n{}",
            passes("    ")
        ),
        format!(
            "def f():\n    if x:\n        return\n    else:\n        pass\n{}",
            passes("    ")
        ),
        format!(
            "def f():\n    for i in x:\n        break\n{}",
            passes("        ")
        ),
        format!(
            "def f():\n    try:\n        return\n    finally:\n        pass\n{}",
            passes("    ")
        ),
        format!("def f():\n    with x:\n        return\n{}", passes("    ")),
        format!(
            "def f():\n    for i in x:\n        pass\n    else:\n        return\n{}",
            passes("    ")
        ),
    ] {
        assert_eq!(
            sizes(&live),
            [CompileErrorKind::MethodTooLarge],
            "{}",
            live.get(..60).unwrap()
        );
    }
    let strings: String = (0..=CONSTANT_POOL_LIMIT as usize / 2)
        .map(|i| format!("'{i}', "))
        .collect();
    assert!(sizes(&format!("def f():\n    return\n    x = [{strings}]\n")).is_empty());
    // String statements after `return`, one per line.
    let lines: String = (0..=CONSTANT_POOL_LIMIT as usize / 2)
        .map(|i| format!("    's{i}'\n"))
        .collect();
    assert!(sizes(&format!("def f():\n    return\n{lines}")).is_empty());
    assert_eq!(
        sizes(&format!("def f():\n    x = [{strings}]\n    return\n")),
        [CompileErrorKind::ClassTooLarge]
    );
}

/// The review's repro (round 18, B2): 2,176 lines of `a = abs(i % 7)`
/// after an import, which Jython rejects, and accepts without the
/// import line. Neither is certainly too large for the lower bound, so
/// the verdict is open; many code units leave `<init>` open; small
/// scripts are decided.
pub(crate) fn repro(import: bool) -> String {
    let mut s = String::from("def g():\n");
    s.push_str(if import { "    import math\n" } else { "\n" });
    for i in 0..2_176 {
        s.push_str(&format!("    a = abs({})\n", i % 7));
    }
    s.push_str("    c = 1\n    c = 1\n    return a + c\n");
    s
}

#[test]
fn open_verdicts() {
    let open = |source: &str| {
        let parsed = iglint_python::parse_module(source);
        assert!(parsed.errors.is_empty());
        (
            size_unknown(&parsed.module, source),
            size_errors(&parsed.module, source).is_empty(),
        )
    };
    for import in [true, false] {
        let (verdict, fits) = open(&repro(import));
        assert!(fits);
        let verdict = verdict.expect("open");
        assert!(
            verdict.starts_with("the function `g` needs between"),
            "{verdict}"
        );
    }
    // Functions of eight parameters: Jython compiles 760 of them and
    // fails on `<init>` from about 765. The mirror of `<init>` decides
    // 700 (fits); from 765 on the verdict is open or `SCR002` reports it.
    let units = |n: usize| -> String {
        (0..n)
            .map(|i| format!("def g{i}(a, b, c, d, e, f1, f2, f3):\n    return a\n"))
            .collect()
    };
    assert_eq!(open(&units(700)), (None, true));
    for n in [765, 800, 900] {
        let (verdict, fits) = open(&units(n));
        assert!(verdict.is_some() || !fits, "{n}");
        let parsed = iglint_python::parse_module(&units(n));
        assert!(upper::class(&parsed.module, &units(n)).init > METHOD_LIMIT as usize);
    }
    assert_eq!(open("import math\nx = 1\n").0, None);
    // Certainly too large: decided (SCR002 reports it).
    let (verdict, fits) = open(&"pass\n".repeat(40_000));
    assert_eq!((verdict, fits), (None, false));
}

#[test]
fn constant_pool() {
    // Few lines, so that the module is not too large a method first.
    let packed = |n: usize| -> String {
        (0..n)
            .map(|i| format!("'{i}'"))
            .collect::<Vec<_>>()
            .chunks(1000)
            .map(|c| format!("x = [{}]\n", c.join(", ")))
            .collect()
    };
    assert!(sizes(&packed(CONSTANT_POOL_LIMIT as usize / 2)).is_empty());
    assert_eq!(
        sizes(&packed(CONSTANT_POOL_LIMIT as usize / 2 + 1)),
        [CompileErrorKind::ClassTooLarge]
    );
    // Equal values count once; `str` and `unicode` of the same text too.
    let same: String = (0..CONSTANT_POOL_LIMIT as usize)
        .map(|_| "'a', u'a', ")
        .collect();
    assert!(sizes(&format!("x = [{same}]\n")).is_empty());
}

/// `def f():` with `depth` nested `try: ... finally: return 2`, the
/// innermost body `return 1` (review round 19, Codex: the bounds
/// overflowed).
pub(crate) fn nested_finally(depth: usize) -> String {
    let mut s = String::from("def f():\n");
    for d in 0..depth {
        s.push_str(&format!("{}try:\n", "    ".repeat(d + 1)));
    }
    s.push_str(&format!("{}return 1\n", "    ".repeat(depth + 1)));
    for d in (0..depth).rev() {
        let pad = "    ".repeat(d + 1);
        s.push_str(&format!("{pad}finally:\n{pad}    return 2\n"));
    }
    s.push_str("x = 1  \n");
    s
}

/// Sizes that multiply (nested `finally` bodies, closures copied with
/// them) never wrap: the verdict is open or `SCR002` reports the script.
#[test]
fn multiplied_sizes_saturate() {
    let mut sources: Vec<String> = [10, 50, 64, 200].into_iter().map(nested_finally).collect();
    // Closures over many variables in nested `finally` bodies with ways
    // out at every level.
    let params: Vec<String> = (0..200).map(|i| format!("p{i}")).collect();
    let mut s = format!("def f({}):\n    for z in q:\n", params.join(", "));
    for d in 0..30 {
        s.push_str(&format!(
            "{}try:\n{}    if z: break\n",
            "    ".repeat(d + 2),
            "    ".repeat(d + 2)
        ));
    }
    s.push_str(&format!("{}x = 1\n", "    ".repeat(32)));
    for d in (0..30).rev() {
        let pad = "    ".repeat(d + 2);
        s.push_str(&format!(
            "{pad}finally:\n{pad}    g = lambda: ({},)\n",
            params.join(", ")
        ));
    }
    sources.push(s);
    // Wide `with` nesting with `continue` at every level.
    let mut s = String::from("def f(a):\n    for z in a:\n");
    for d in 0..60 {
        s.push_str(&format!(
            "{}with a as w{d}:\n{}    if z: continue\n",
            "    ".repeat(d + 2),
            "    ".repeat(d + 2)
        ));
    }
    s.push_str(&format!("{}x = 1\n", "    ".repeat(62)));
    sources.push(s);
    for (i, source) in sources.iter().enumerate() {
        let parsed = iglint_python::parse_module(source);
        if i == 3 {
            // Two hundred levels are deeper than the parser accepts (a
            // syntax error, `SCR001`).
            assert!(!parsed.errors.is_empty());
            continue;
        }
        assert!(parsed.errors.is_empty(), "{i}");
        let errors = size_errors(&parsed.module, source);
        let open = size_unknown(&parsed.module, source);
        if i == 0 {
            // Ten levels are mirrored exactly and fit.
            assert!(errors.is_empty() && open.is_none(), "{errors:?} {open:?}");
            continue;
        }
        if i < 3 {
            assert!(
                open.is_some() || !errors.is_empty(),
                "{i}: decided and fits"
            );
        }
        for unit in code_units(&parsed.module, source) {
            assert!(unit.upper >= unit.bytes, "{i}: {unit:?}");
        }
        let _ = upper::class(&parsed.module, source);
    }
    // Fifty levels and more: every `return` copies the `finally` bodies
    // around it, each with its own `return` (2^50 copies). The mirror
    // gives up and the verdict is open.
    for depth in [50, 64] {
        let source = nested_finally(depth);
        let parsed = iglint_python::parse_module(&source);
        let verdict = size_unknown(&parsed.module, &source).expect("open");
        assert!(verdict.contains("too large to bound"), "{verdict}");
        let f = code_units(&parsed.module, &source)
            .into_iter()
            .find(|u| u.name == "f" && u.owner.is_some())
            .unwrap();
        assert_eq!(f.upper, usize::MAX);
    }
}

/// Review round 19 (Opus, blocker): closures created many times over many
/// free variables. The end-to-end repro (Jython rejects it with the
/// import and compiles it without, 65,517 bytes) has an open verdict
/// both ways.
#[test]
fn closures_count_per_creation_and_free_variable() {
    let p: Vec<String> = (0..150).map(|i| format!("p{i}")).collect();
    let p = p.join(", ");
    for import in [true, false] {
        let mut s = format!("def big({p}):\n");
        s.push_str(if import { "    import math\n" } else { "\n" });
        for i in 0..25 {
            s.push_str(&format!("    l{i} = lambda: ({p})\n"));
        }
        for _ in 0..84 {
            s.push_str("    f(g(a, k=h(b, k=i(c, k=d))), k=j(a, b, c, d, e))\n");
        }
        for i in 0..8 {
            s.push_str(&format!("    x = {i}\n"));
        }
        s.push_str("    return 1\n");
        let parsed = iglint_python::parse_module(&s);
        assert!(size_errors(&parsed.module, &s).is_empty());
        let verdict = size_unknown(&parsed.module, &s).expect("open");
        assert!(
            verdict.starts_with("the function `big` needs between"),
            "{verdict}"
        );
        let big = code_units(&parsed.module, &s)
            .into_iter()
            .find(|u| u.name == "big")
            .unwrap();
        // Jython: 65,517 bytes without the import.
        assert!(big.upper >= 65_517, "{}", big.upper);
    }
}

/// Review round 20 (Codex, blocker): Jython loads a name of more than
/// 16,000 characters in chunks (`Code.ldc`), and the bound charged one
/// `ldc` per name. Its repro (1,683 reads of an attribute of 16,001
/// characters) fails with the import and compiles with the import's line
/// blanked (65,528 bytes): the bounds now enclose it, and the identifier
/// alone makes the verdict open.
#[test]
fn long_names_are_charged_by_length() {
    for import in [true, false] {
        let mut s = String::from("def work(obj):\n");
        s.push_str(if import { "    import math\n" } else { "\n" });
        for _ in 0..1_683 {
            s.push_str(&format!("    obj.{}\n", "n".repeat(16_001)));
        }
        s.push_str("    return 1\nprint \"loaded\"\n");
        let parsed = iglint_python::parse_module(&s);
        assert!(size_errors(&parsed.module, &s).is_empty());
        let work = code_units(&parsed.module, &s)
            .into_iter()
            .find(|u| u.name == "work")
            .unwrap();
        assert!(work.upper >= 65_528, "{}", work.upper);
        assert!(work.bytes <= 65_528, "{}", work.bytes);
        assert_eq!(
            size_unknown(&parsed.module, &s).as_deref(),
            Some("identifier of 16,001 characters: Jython's code size cannot be bounded precisely")
        );
    }
}

/// Identifiers in every position count for [`IDENTIFIER_LIMIT`], also in
/// code Jython does not generate.
#[test]
fn long_identifiers_make_the_size_verdict_open() {
    let at = |n: usize| "n".repeat(n);
    let positions = [
        "x = {N}",
        "o.{N} = 1",
        "o({N}=1)",
        "import {N}",
        "import m as {N}",
        "from {N} import x",
        "from m import {N}",
        "from m import x as {N}",
        "def f():\n    global {N}",
        "def {N}(): pass",
        "class {N}: pass",
        "def f({N}): pass",
        "def f(*{N}): pass",
        "def f(**{N}): pass",
        "l = lambda *{N}: 1",
        "def f():\n    return\n    x = {N}",
    ];
    for position in positions {
        for (n, open) in [(IDENTIFIER_LIMIT, false), (IDENTIFIER_LIMIT + 1, true)] {
            let s = format!("{}\n", position.replace("{N}", &at(n)));
            let parsed = iglint_python::parse_module(&s);
            assert!(parsed.errors.is_empty(), "{position}");
            assert_eq!(longest_identifier(&parsed.module), n, "{position}");
            assert_eq!(
                size_unknown(&parsed.module, &s).is_some(),
                open,
                "{position}"
            );
        }
    }
    // Strings are modelled exactly: no limit.
    let s = format!("x = '{}'\n", at(100_000));
    let parsed = iglint_python::parse_module(&s);
    assert_eq!(size_unknown(&parsed.module, &s), None);
}

/// Review round 20 (Codex): the Java method of a `def` or `class` body is
/// named `<name>$<id>` (mangled in a class), at most 65,535 bytes.
#[test]
fn names_too_long_for_a_java_method() {
    let n = |len: usize| "n".repeat(len);
    let cases = [
        (format!("\n\ndef {}():\n    return 1\n", n(65_533)), false),
        (format!("\n\ndef {}():\n    return 1\n", n(65_534)), true),
        (format!("\n\nclass {}:\n    pass\n", n(65_534)), true),
        (
            format!(
                "\n\nclass K:\n    def __{}(self):\n        return 1\n",
                n(65_529)
            ),
            false,
        ),
        (
            format!(
                "\n\nclass K:\n    def __{}(self):\n        return 1\n",
                n(65_530)
            ),
            true,
        ),
        // Not generated: no method.
        (
            format!(
                "\n\ndef f():\n    return\n    def {}():\n        pass\n",
                n(70_000)
            ),
            false,
        ),
        // Names elsewhere are loaded in chunks.
        (format!("\n\n{} = 1\n", n(70_000)), false),
    ];
    for (source, fails) in cases {
        let kinds = sizes(&source);
        assert_eq!(
            kinds.contains(&CompileErrorKind::NameTooLong),
            fails,
            "{}",
            source.len()
        );
    }
}

/// Every certain size error has a key without its counts, the same for
/// two sizes of the same failure; other messages have none.
#[test]
fn size_errors_have_keys_without_counts() {
    let module = |n: usize, extra: &str| {
        let mut s: String = (0..n).map(|i| format!("x{i} = {i}\n")).collect();
        s.push_str(extra);
        s
    };
    let strings: String = (0..40_000).map(|i| format!("s = 's{i}'\n")).collect();
    let keys = |source: &str| -> Vec<String> {
        let parsed = iglint_python::parse_module(source);
        size_errors(&parsed.module, source)
            .into_iter()
            .map(|e| {
                let key = size_error_key(&e.message).expect("size errors have keys");
                assert!(!key.chars().any(|c| c.is_ascii_digit()), "{key}");
                key
            })
            .collect()
    };
    let a = keys(&module(9000, "import math\n"));
    assert!(!a.is_empty());
    assert_eq!(a, keys(&module(9500, "")));
    assert!(
        keys(&strings)
            .iter()
            .any(|k| k.ends_with("(Class too large)"))
    );
    assert_eq!(size_error_key("'return' outside function"), None);
}
