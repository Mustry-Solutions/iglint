//! Python 3 syntax is rejected with an explanation (at the position where
//! Jython reports the error).

use iglint_python::parse_module;

fn first_error(source: &str) -> String {
    let parsed = parse_module(source);
    assert!(!parsed.errors.is_empty(), "accepted: {source:?}");
    parsed.errors[0].message.clone()
}

#[test]
fn python3_syntax_is_explained() {
    let cases = [
        ("x = f'{a}'\n", "f-strings"),
        ("x = rb'a'\n", "'rb'"),
        ("def f():\n    nonlocal x\n", "nonlocal"),
        ("async def f():\n    pass\n", "async"),
        ("def f():\n    yield from g()\n", "yield from"),
        ("raise A from B\n", "raise ... from"),
        ("def f(a, *, b):\n    pass\n", "keyword-only"),
        ("def f(a: int):\n    pass\n", "annotations"),
        ("def f() -> int:\n    pass\n", "return annotations"),
        ("a, *b = c\n", "starred"),
        ("if (x := 1):\n    pass\n", ":="),
        ("x = a @ b\n", "matrix"),
        ("x = ...\n", "'...'"),
        ("print('a', end='')\n", "print_function"),
        ("class A(metaclass=M):\n    pass\n", "metaclass"),
        ("x = [*a, *b]\n", "unpacking"),
        ("x = 1_000\n", "underscores"),
    ];
    for (source, expected) in cases {
        let message = first_error(source);
        assert!(
            message.contains(expected),
            "{source:?}: {message:?} does not mention {expected:?}"
        );
    }
}

#[test]
fn print_function_future_makes_print_a_name() {
    let source = "from __future__ import print_function\nprint('a', end='')\n";
    assert!(parse_module(source).errors.is_empty());
}
