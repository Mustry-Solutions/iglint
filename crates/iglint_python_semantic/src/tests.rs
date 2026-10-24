use iglint_python::ast::{Expr, Stmt};
use iglint_text::TextRange;

use crate::*;

fn parse(src: &str) -> iglint_python::Parsed {
    let parsed = iglint_python::parse_module(src);
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    parsed
}

fn with_model(src: &str, injected: &[&str], f: impl FnOnce(&SemanticModel<'_>)) {
    let parsed = parse(src);
    let options = SemanticOptions {
        injected: injected.to_vec(),
        ..SemanticOptions::default()
    };
    let model = SemanticModel::build(&parsed.module, &options);
    f(&model);
}

/// Names of references with the given resolution, in source order.
fn refs_with(src: &str, injected: &[&str], resolution: Resolution) -> Vec<String> {
    let mut out = Vec::new();
    with_model(src, injected, |m| {
        let mut refs: Vec<_> = m
            .references()
            .filter(|(_, r)| r.resolution == resolution)
            .map(|(_, r)| (r.range.start(), r.name.to_owned()))
            .collect();
        refs.sort();
        out = refs.into_iter().map(|(_, n)| n).collect();
    });
    out
}

fn undefined(src: &str) -> Vec<String> {
    refs_with(src, &[], Resolution::Unresolved)
}

/// Names of reachable references that no binding reaches.
fn unbound(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    with_model(src, &[], |m| {
        let mut refs: Vec<_> = m
            .references()
            .filter(|(_, r)| r.unbound && r.reachable)
            .map(|(_, r)| (r.range.start(), r.name.to_owned()))
            .collect();
        refs.sort();
        out = refs.into_iter().map(|(_, n)| n).collect();
    });
    out
}

fn unused_bindings(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    with_model(src, &[], |m| {
        let mut b: Vec<_> = m
            .bindings()
            .filter(|(_, b)| !b.is_implicit() && b.kind != BindingKind::Global && !b.is_used())
            .map(|(_, b)| (b.range.start(), b.name.to_owned()))
            .collect();
        b.sort();
        out = b.into_iter().map(|(_, n)| n).collect();
    });
    out
}

fn unreachable(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    with_model(src, &[], |m| {
        out = m
            .unreachable_statements()
            .iter()
            .map(|(s, _)| {
                let r: TextRange = iglint_text::Ranged::range(*s);
                src[r.to_std()]
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_owned()
            })
            .collect();
    });
    out
}

fn falls_through(src: &str) -> bool {
    let parsed = parse(src);
    let model = SemanticModel::build(&parsed.module, &SemanticOptions::default());
    let Some(Stmt::FunctionDef(f)) = parsed.module.body.first() else {
        panic!("first statement is not a def");
    };
    model.function_can_fall_through(f)
}

#[test]
fn builtins_and_injected_names() {
    let src = "x = len(system.tag) + undefined_thing + core.util.f()\n";
    assert_eq!(undefined(src), ["system", "undefined_thing", "core"]);
    assert_eq!(
        refs_with(src, &["system", "core"], Resolution::Unresolved),
        ["undefined_thing"]
    );
    assert_eq!(
        refs_with(src, &["system", "core"], Resolution::Injected),
        ["system", "core"]
    );
    assert_eq!(refs_with(src, &[], Resolution::Builtin), ["len"]);
    assert!(undefined("print __builtins__, __name__, __doc__, None, True\n").is_empty());
    assert_eq!(undefined("print __file__\n"), ["__file__"]);
}

#[test]
fn list_comprehension_variables_leak() {
    assert!(undefined("[x for x in range(3)]\nprint x\n").is_empty());
    assert!(undefined("def f():\n    [x for x in range(3)]\n    return x\n").is_empty());
}

#[test]
fn generator_and_set_comprehensions_have_their_own_scope() {
    assert_eq!(undefined("list(x for x in range(3))\nprint x\n"), ["x"]);
    assert_eq!(undefined("{x for x in range(3)}\nprint x\n"), ["x"]);
    assert_eq!(undefined("{x: 1 for x in range(3)}\nprint x\n"), ["x"]);
    // The first iterable is evaluated outside, later ones inside.
    assert_eq!(undefined("list(a for a in b for c in a)\n"), ["b"]);
}

#[test]
fn class_scope_is_not_visible_to_methods() {
    let src = "class C:\n    X = 1\n    Y = X\n    def m(self):\n        return X\n";
    assert_eq!(undefined(src), ["X"]);
    // Class bodies see enclosing function variables.
    assert!(
        undefined("def f():\n    v = 1\n    class C:\n        w = v\n    return C\n").is_empty()
    );
    // `__module__` is bound in class bodies.
    assert!(undefined("class C:\n    m = __module__\n").is_empty());
}

#[test]
fn decorators_and_defaults_are_evaluated_outside() {
    let src = "def outer():\n    def f(a=a):\n        a = 1\n    return f\n";
    assert_eq!(undefined(src), ["a"]);
    let src = "def deco(f): return f\nclass C:\n    @deco\n    def m(self): pass\n";
    assert!(undefined(src).is_empty());
}

#[test]
fn globals_declared_in_functions() {
    let src = "def init():\n    global CONFIG\n    CONFIG = 1\ndef use():\n    return CONFIG\n";
    assert!(undefined(src).is_empty());
    with_model(src, &[], |m| {
        let config: Vec<_> = m.scope(ScopeId::MODULE).bindings_of("CONFIG").to_vec();
        assert_eq!(config.len(), 1);
        assert!(m.binding(config[0]).is_used());
    });
    // Module code reading it before (in source order) is not "unbound".
    assert!(unbound("def init():\n    global X\n    X = 1\ninit()\nprint X\n").is_empty());
}

#[test]
fn dynamic_scopes() {
    assert!(undefined("from os.path import *\nprint join\n").is_empty());
    assert_eq!(
        refs_with(
            "from os.path import *\nprint join\n",
            &[],
            Resolution::Dynamic
        ),
        ["join"]
    );
    assert!(undefined("def f():\n    exec 'y = 1'\n    return y\n").is_empty());
    assert_eq!(
        undefined("def f():\n    exec 'y = 1' in {}\n    return y\n"),
        ["y"]
    );
    assert!(undefined("globals()['z'] = 1\nprint z\n").is_empty());
}

/// Each case checked on Ignition's Jython: the name exists at run time.
#[test]
fn qualified_exec_and_module_namespace_writes() {
    for src in [
        "def f():\n    exec 'x = 1' in globals()\n    return x\n",
        "def f():\n    exec 'x = 1' in globals(), locals()\n    return x\n",
        "def f():\n    exec 'x = 1' in globals(), globals()\n    return x\n",
        "def f():\n    exec 'x = 1' in {}, globals()\n    return x\n",
        "def f():\n    exec 'x = 1' in locals()\n    return x\n",
        "def f():\n    exec 'x = 1' in vars()\n    return x\n",
        "def g():\n    exec 'x = 1' in globals()\ndef h():\n    return x\n",
        "def f():\n    def g():\n        exec 'x = 1' in globals()\n    g()\n    return x\n",
        "exec 'x = 1' in globals()\nprint x\n",
        "exec 'x = 1' in vars()\nprint x\n",
        "exec 'x = 1' in locals()\nprint x\n",
        "exec 'x = 1' in {}, globals()\nprint x\n",
        "def f():\n    globals().update(x=1)\n    return x\n",
        "def f():\n    globals()['x'] = 1\n    return x\n",
        "import sys\nsetattr(sys.modules[__name__], 'x', 1)\nprint x\n",
        "import sys\nsys.modules[__name__].x = 1\nprint x\n",
        "import sys\nsys.modules[__name__].__dict__['x'] = 1\nprint x\n",
        "import sys\nvars(sys.modules[__name__]).update(x=1)\nprint x\n",
        "__builtins__['x'] = 1\nprint x\n",
        "import __builtin__\nsetattr(__builtin__, 'x', 1)\nprint x\n",
        "import __builtin__\n__builtin__.x = 1\nprint x\n",
    ] {
        assert!(undefined(src).is_empty(), "{src}");
    }
    // Names written elsewhere stay undefined (also NameError on Jython).
    for src in [
        "def f():\n    exec 'x = 1' in {}\n    return x\n",
        "def f():\n    exec 'x = 1' in globals(), {}\n    return x\n",
        "def f(path):\n    execfile(path)\n    return x\n",
        "def f(path):\n    execfile(path, globals())\n    return x\n",
        "def f():\n    exec 'x = 1' in locals()\ndef g():\n    return x\n",
    ] {
        assert_eq!(undefined(src), ["x"], "{src}");
    }
}

#[test]
fn tuple_parameters_and_star_parameters() {
    let src = "def f((a, b), c=1, *args, **kw):\n    return a + b + c + len(args) + len(kw)\n";
    assert!(undefined(src).is_empty());
    assert!(undefined("f = lambda (x, y): x + y\n").is_empty());
}

#[test]
fn import_forms() {
    let src = "import a.b.c\nimport d.e as f\nfrom g import h as i, j\nprint a, f, i, j\n";
    assert!(undefined(src).is_empty());
    assert_eq!(undefined("import a.b\nprint b\n"), ["b"]);
    with_model(src, &[], |m| {
        let q: Vec<_> = m
            .bindings()
            .filter_map(|(_, b)| b.qualified.clone().filter(|_| !b.is_implicit()))
            .collect();
        assert_eq!(q, ["a", "d.e", "g.h", "g.j"]);
    });
}

#[test]
fn except_with_and_for_targets() {
    let src = "try:\n    pass\nexcept ValueError, e:\n    pass\nprint e\nwith open('f') as fh:\n    pass\nfor k, v in []:\n    pass\nprint fh, k, v\n";
    assert!(undefined(src).is_empty());
}

#[test]
fn free_variables_resolve_to_enclosing_functions() {
    let src = "def f():\n    x = 1\n    def g():\n        return x\n    return g\n";
    with_model(src, &[], |m| {
        let (_, r) = m.references().find(|(_, r)| r.name == "x").unwrap();
        assert!(matches!(r.resolution, Resolution::Free(_)));
    });
    assert_eq!(unused_bindings(src), ["f"]);
}

#[test]
fn used_before_assignment_is_flow_sensitive() {
    assert_eq!(unbound("def f():\n    print x\n    x = 1\n"), ["x"]);
    assert_eq!(unbound("def f():\n    x = x + 1\n"), ["x"]);
    assert_eq!(unbound("def f():\n    x += 1\n"), ["x"]);
    // Bound in every branch, or in some: never reported.
    assert!(
        unbound("def f(c):\n    if c:\n        x = 1\n    else:\n        x = 2\n    return x\n")
            .is_empty()
    );
    assert!(unbound("def f(c):\n    if c:\n        x = 1\n    return x\n").is_empty());
    // Loops: a later iteration sees earlier bindings.
    assert!(unbound("def f(xs):\n    for i in xs:\n        if i:\n            print prev\n        prev = i\n").is_empty());
    assert!(unbound("def f():\n    while True:\n        try:\n            x = g()\n            break\n        except:\n            continue\n    return x\n").is_empty());
    // try/except/else/finally
    assert!(unbound("def f():\n    try:\n        x = g()\n    except ValueError:\n        return\n    return x\n").is_empty());
    assert!(unbound("def f():\n    try:\n        g()\n    except ValueError:\n        x = 1\n    else:\n        x = 2\n    return x\n").is_empty());
    assert!(
        unbound("def f():\n    try:\n        x = g()\n    finally:\n        print x\n").is_empty()
    );
    assert!(unbound("def f():\n    while True:\n        try:\n            break\n        finally:\n            x = 1\n    return x\n").is_empty());
    // del unbinds
    assert_eq!(
        unbound("def f():\n    x = 1\n    del x\n    return x\n"),
        ["x"]
    );
    // Module level: builtins are the fallback.
    assert!(unbound("print len\nlen = 5\n").is_empty());
    assert_eq!(unbound("print y\ny = 5\n"), ["y"]);
    // Dead branches do not bind.
    assert_eq!(
        unbound("def f():\n    if False:\n        x = 1\n    return x\n"),
        ["x"]
    );
    // Unreachable code is not reported.
    assert!(unbound("def f():\n    return 1\n    print x\n    x = 2\n").is_empty());
    // Comprehension variables may be bound.
    assert!(unbound("def f(y):\n    [x for x in y]\n    return x\n").is_empty());
    // Except variables persist after the handler.
    assert!(unbound("def f():\n    try:\n        g()\n    except Exception, e:\n        pass\n    return e\n").is_empty());
}

#[test]
fn reaching_definitions_mark_usage() {
    assert_eq!(unused_bindings("import os\nos = 1\nprint os\n"), ["os"]);
    assert!(unused_bindings("import os\nif c:\n    os = 1\nprint os\n").is_empty());
    assert!(unused_bindings("def f(): pass\ndef g(): return f()\n").contains(&"g".to_owned()));
    // Free and global reads mark every binding of the name.
    assert_eq!(
        unused_bindings("x = 1\nx = 2\ndef f():\n    return x\n"),
        ["f"]
    );
}

#[test]
fn unreachable_statements() {
    assert_eq!(
        unreachable("def f():\n    return 1\n    x = 2\n    y = 3\n"),
        ["x = 2"]
    );
    assert_eq!(
        unreachable(
            "def f(c):\n    if c:\n        return 1\n    else:\n        raise ValueError()\n    print c\n"
        ),
        ["print c"]
    );
    assert_eq!(unreachable("while True:\n    pass\nprint 1\n"), ["print 1"]);
    assert!(unreachable("while True:\n    if x:\n        break\nprint 1\n").is_empty());
    assert_eq!(
        unreachable("for x in y:\n    continue\n    print x\n"),
        ["print x"]
    );
    assert!(unreachable("try:\n    return_value = 1\nexcept:\n    pass\nprint 1\n").is_empty());
    // Dead `if False:` bodies are not reported.
    assert!(unreachable("if False:\n    x = 1\n").is_empty());
    // A `finally` clause runs after `return`.
    assert!(
        unreachable("def f():\n    try:\n        return 1\n    finally:\n        print 2\n")
            .is_empty()
    );
    assert_eq!(
        unreachable(
            "def f():\n    try:\n        return 1\n    finally:\n        print 2\n    print 3\n"
        ),
        ["print 3"]
    );
}

#[test]
fn function_fall_through() {
    assert!(falls_through("def f(x):\n    if x:\n        return 1\n"));
    assert!(!falls_through(
        "def f(x):\n    if x:\n        return 1\n    else:\n        return 2\n"
    ));
    assert!(!falls_through(
        "def f(x):\n    if x:\n        return 1\n    raise ValueError(x)\n"
    ));
    assert!(!falls_through("def f(x):\n    while True:\n        pass\n"));
    assert!(falls_through(
        "def f(x):\n    while True:\n        if x:\n            break\n"
    ));
    assert!(!falls_through(
        "def f(x):\n    try:\n        return g(x)\n    except Exception:\n        return None\n"
    ));
    assert!(falls_through(
        "def f(x):\n    try:\n        return g(x)\n    except Exception:\n        log(x)\n"
    ));
    assert!(!falls_through(
        "def f(x):\n    try:\n        pass\n    finally:\n        return x\n"
    ));
    assert!(!falls_through(
        "def f(x):\n    for i in x:\n        return i\n    else:\n        return None\n"
    ));
    assert!(falls_through(
        "def f(x):\n    for i in x:\n        return i\n"
    ));
    assert!(!falls_through("def f(x):\n    with x:\n        return 1\n"));
    assert!(falls_through("def f(x):\n    pass\n"));
}

#[test]
fn qualified_names_follow_imports_and_aliases() {
    let src = "import time\nfrom java.lang import Thread\nimport os.path as osp\ndb = system.db\nfrom time import sleep as nap\ntime.sleep(1)\nThread.sleep(1)\ndb.runQuery('q')\nnap(1)\nosp.join('a')\nlen([])\n";
    let parsed = parse(src);
    let model = SemanticModel::build(
        &parsed.module,
        &SemanticOptions {
            injected: vec!["system"],
            ..SemanticOptions::default()
        },
    );
    let calls: Vec<String> = parsed
        .module
        .body
        .iter()
        .filter_map(|s| match s {
            Stmt::Expr(e) => match &*e.value {
                Expr::Call(c) => model.callee(c),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        calls,
        [
            "time.sleep",
            "java.lang.Thread.sleep",
            "system.db.runQuery",
            "time.sleep",
            "os.path.join",
            "len"
        ]
    );
}

#[test]
fn qualified_names_give_up_on_ambiguity() {
    let src = "import time\nif c:\n    time = other\ntime.sleep(1)\n";
    let parsed = parse(src);
    let model = SemanticModel::build(&parsed.module, &SemanticOptions::default());
    let Stmt::Expr(e) = parsed.module.body.last().unwrap() else {
        panic!()
    };
    let Expr::Call(c) = &*e.value else { panic!() };
    assert_eq!(model.callee(c), None);
}

#[test]
fn dunder_all_and_locals() {
    with_model(
        "__all__ = ['a', 'b']\n__all__ += ['c']\n__all__.append('d')\n",
        &[],
        |m| {
            assert_eq!(m.dunder_all(), ["a", "b", "c", "d"]);
            assert!(!m.dunder_all_is_dynamic());
        },
    );
    with_model(
        "def f():\n    x = 1\n    return '%(x)s' % locals()\n",
        &[],
        |m| {
            let f = m.scopes().find(|(_, s)| s.name == "f").unwrap().1;
            assert!(f.reads_locals);
        },
    );
}

#[test]
fn compile_errors_basic() {
    let cases = [
        ("return 1\n", "'return' outside function"),
        (
            "def f():\n    yield 1\n    return 2\n",
            "'return' with argument inside generator",
        ),
        (
            "while x:\n    pass\nelse:\n    continue\n",
            "'continue' not properly in loop",
        ),
        ("def f(a, a): pass\n", "duplicate argument name found: a"),
        (
            "try:\n    pass\nexcept:\n    pass\nexcept ValueError:\n    pass\n",
            "default 'except:' must be last",
        ),
    ];
    for (src, message) in cases {
        let parsed = parse(src);
        let model = SemanticModel::build(&parsed.module, &SemanticOptions::default());
        let errors = compile_errors(&model);
        assert_eq!(errors.len(), 1, "{src}: {errors:?}");
        assert_eq!(errors[0].message, message);
    }
}

#[test]
fn deep_and_wide_code_is_linear() {
    // Many sequential loops and branches must not blow up.
    let mut src = String::from("def f(xs):\n");
    for i in 0..400 {
        src.push_str(&format!("    for a{i} in xs:\n        if a{i}:\n            b{i} = a{i}\n        else:\n            b{i} = 0\n"));
    }
    let start = std::time::Instant::now();
    let parsed = parse(&src);
    let _ = SemanticModel::build(&parsed.module, &SemanticOptions::default());
    assert!(start.elapsed().as_secs() < 5);
    // Nested loops converge quickly.
    let mut src = String::from("def f(xs):\n");
    for i in 0..12 {
        src.push_str(&format!("{}for a{i} in xs:\n", "    ".repeat(i + 1)));
    }
    src.push_str(&format!("{}x = a0\n", "    ".repeat(13)));
    let parsed = parse(&src);
    let start = std::time::Instant::now();
    let _ = SemanticModel::build(&parsed.module, &SemanticOptions::default());
    assert!(start.elapsed().as_secs() < 5);
}

#[test]
fn qualified_names_of_unbound_globals() {
    use crate::UnboundNames;
    let src =
        "from x import *\ndef f():\n    core.util.go()\ncore.util.go()\nlocal = 1\nlocal.attr\n";
    let parsed = parse(src);
    let model = SemanticModel::build(&parsed.module, &SemanticOptions::default());
    let exprs: Vec<&Expr> = parsed
        .module
        .body
        .iter()
        .filter_map(|s| match s {
            Stmt::Expr(e) => Some(&*e.value),
            _ => None,
        })
        .collect();
    // The star import makes `core` dynamic.
    let Expr::Call(c) = exprs[0] else { panic!() };
    assert_eq!(model.callee(c), None);
    assert_eq!(
        model.qualified_name_with(&c.func, UnboundNames::AsGlobal),
        None
    );
    assert_eq!(
        model
            .qualified_name_with(&c.func, UnboundNames::AsGlobalEvenIfDynamic)
            .as_deref(),
        Some("core.util.go")
    );
    // Bound names keep their normal resolution.
    assert_eq!(
        model.qualified_name_with(exprs[1], UnboundNames::AsGlobal),
        None
    );
    let src = "def f():\n    core.util.go()\n";
    let parsed = parse(src);
    let model = SemanticModel::build(&parsed.module, &SemanticOptions::default());
    let Stmt::FunctionDef(f) = &parsed.module.body[0] else {
        panic!()
    };
    let Stmt::Expr(e) = &f.body[0] else { panic!() };
    let Expr::Call(c) = &*e.value else { panic!() };
    assert_eq!(model.callee(c), None);
    assert_eq!(
        model
            .qualified_name_with(&c.func, UnboundNames::AsGlobal)
            .as_deref(),
        Some("core.util.go")
    );
}

/// How each reference to `__package__` resolves in a library module
/// (`module_has_name`) or an event script: (resolution, unbound).
fn package_refs(src: &str, module_has_name: bool) -> Vec<(Resolution, bool)> {
    let parsed = parse(src);
    let options = SemanticOptions {
        module_has_name,
        ..SemanticOptions::default()
    };
    let model = SemanticModel::build(&parsed.module, &options);
    let mut refs: Vec<_> = model
        .references()
        .filter(|(_, r)| r.name == "__package__")
        .map(|(_, r)| (r.range.start(), (r.resolution, r.unbound)))
        .collect();
    refs.sort_by_key(|(at, _)| *at);
    refs.into_iter().map(|(_, r)| r).collect()
}

/// Every import that runs binds `__package__` in a library module's
/// globals, wherever it is (measured, `tests/oracle/implicit_names.cases`):
/// the reviewer's repro (round 12, K1) reads it in a function.
#[test]
fn imports_bind_package_in_library_modules() {
    use Resolution::*;
    let repro = "import math\n\ndef get_package():\n    try:\n        return __package__\n    except NameError:\n        return 'ABSENT'\n";
    assert_eq!(package_refs(repro, true), [(Global, false)]);
    let without = repro.replacen("import math", "pass", 1);
    assert_eq!(package_refs(&without, true), [(Unresolved, false)]);
    // Event scripts run without `__name__`: their imports bind nothing.
    assert_eq!(package_refs(repro, false), [(Unresolved, false)]);
    // Module level: unbound before the first import, bound after.
    let src = "x = __package__\nimport os\ny = __package__\n";
    assert_eq!(package_refs(src, true), [(Local, true), (Local, false)]);
    // Every kind of import statement, failing ones included.
    for import in [
        "import os.path",
        "import os as o",
        "from os import sep",
        "from math import *",
        "from __future__ import division",
        "from java.lang import String",
        "import pkg.other",
        "try:\n    import no_such_module\nexcept ImportError:\n    pass",
    ] {
        let src = format!("{import}\ny = __package__\n");
        assert_eq!(package_refs(&src, true), [(Local, false)], "{import}");
    }
    // A relative import raises `ValueError` first (the module name has no
    // dot), unless the code sets `__name__` itself.
    assert_eq!(
        package_refs("from . import x\ny = __package__\n", true),
        [(Unresolved, false)]
    );
    assert_eq!(
        package_refs(
            "__name__ = 'a.b'\nfrom . import x\ny = __package__\n",
            false
        ),
        [(Local, false)]
    );
    // Imports in functions and class bodies write the module's globals.
    let src = "def f():\n    import os\nclass C:\n    import json\nprint __package__\ndef g():\n    return __package__\n";
    assert_eq!(package_refs(src, true), [(Local, false), (Global, false)]);
    with_model_options(src, true, |m| {
        let sites: Vec<ScopeKind> = m
            .bindings()
            .filter(|(_, b)| b.kind == BindingKind::ImportPackage)
            .map(|(_, b)| {
                assert_eq!(b.scope, ScopeId::MODULE);
                m.scope(b.site).kind
            })
            .collect();
        assert_eq!(sites, [ScopeKind::Function, ScopeKind::Class]);
    });
    // A function's own `__package__` does not hide the global one.
    let src = "def f():\n    __package__ = 1\n    import os\ndef g():\n    return __package__\n";
    assert_eq!(package_refs(src, true), [(Global, false)]);
}

fn with_model_options(src: &str, module_has_name: bool, f: impl FnOnce(&SemanticModel<'_>)) {
    let parsed = parse(src);
    let options = SemanticOptions {
        module_has_name,
        ..SemanticOptions::default()
    };
    f(&SemanticModel::build(&parsed.module, &options));
}
