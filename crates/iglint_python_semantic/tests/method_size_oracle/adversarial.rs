//! Programs that maximise each term of the derivation of the upper bound
//! (`docs/internal/fix-safety.md`, "Size limits"): one family per term,
//! named `adv-<family>-<n>`, plus combinations. Every family is
//! deterministic; `count(family)` programs are generated (more with
//! `IGLINT_METHOD_SIZE_SCALE` for a calibration run).

use super::Rng;

/// The families and how many programs of each the committed data holds.
pub(crate) const FAMILIES: &[(&str, usize)] = &[
    ("closure", 40),
    ("closure-loop", 12),
    ("closure-finally", 12),
    ("finally", 40),
    ("with", 16),
    ("yield", 30),
    ("call", 20),
    ("unpack", 16),
    ("decorated", 16),
    ("compare", 16),
    ("display", 30),
    ("strings", 10),
    ("imports", 8),
    ("aug", 12),
    ("misc", 20),
    ("deep", 12),
    ("names", 30),
    ("wide", 20),
    ("combo", 30),
];

/// Programs per family: the committed count times
/// `IGLINT_METHOD_SIZE_SCALE` (default 1).
pub(crate) fn count(base: usize) -> usize {
    let scale: usize = std::env::var("IGLINT_METHOD_SIZE_SCALE")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(1);
    base * scale
}

pub(crate) fn sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for &(family, base) in FAMILIES {
        for k in 0..count(base) {
            let mut r = Rng::new(0xADD0_0000 + 1000 * family_id(family) as u64 + k as u64);
            out.push((format!("adv-{family}-{k}"), program(family, &mut r)));
        }
    }
    out
}

fn family_id(family: &str) -> usize {
    FAMILIES.iter().position(|f| f.0 == family).unwrap_or(0)
}

fn params(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("p{i}")).collect()
}

/// `k` distinct names of `pool`, as a tuple display.
fn tuple_of(r: &mut Rng, pool: &[String], k: usize) -> String {
    let mut picked: Vec<String> = Vec::new();
    let k = k.min(pool.len());
    let start = r.below(pool.len().max(1));
    for i in 0..k {
        picked.push(pool[(start + i) % pool.len()].clone());
    }
    if picked.is_empty() {
        "()".to_owned()
    } else {
        format!("({},)", picked.join(", "))
    }
}

fn indent(text: &str, pad: &str) -> String {
    text.lines()
        .map(|l| format!("{pad}{l}\n"))
        .collect::<String>()
}

/// A unit that creates a closure over `vars`, as statements at `pad`.
fn creation(r: &mut Rng, i: usize, vars: &str) -> String {
    match r.below(9) {
        0 => format!("def h{i}():\n    return {vars}\n"),
        1 => format!("l{i} = lambda: {vars}\n"),
        2 => format!("l{i} = list(x for x in {vars})\n"),
        3 => format!("l{i} = {{x for x in {vars} if x}}\n"),
        4 => format!("l{i} = {{x: {vars} for x in q}}\n"),
        5 => format!("class K{i}:\n    v = {vars}\n"),
        6 => format!("class K{i}:\n    def m(self):\n        return {vars}\n"),
        7 => format!(
            "def h{i}():\n    def h2():\n        def h3():\n            return {vars}\n        return h3\n    return h2\n"
        ),
        _ => format!("l{i} = lambda a=1, *b, **c: (lambda: {vars})\n"),
    }
}

fn program(family: &str, r: &mut Rng) -> String {
    match family {
        "closure" => {
            // Many creations times many free variables, up to three
            // levels deep, classes in between.
            let p = 1 + r.below(300);
            let n = 1 + r.below(60);
            let pool = params(p);
            let mut s = format!("def big({}):\n", pool.join(", "));
            for i in 0..n {
                let k = 1 + r.below(p);
                let vars = tuple_of(r, &pool, k);
                s.push_str(&indent(&creation(r, i, &vars), "    "));
            }
            s.push_str("    return 1\n");
            s
        }
        "closure-loop" => {
            // Locals as well as parameters, creations inside loops and
            // conditions, and a class around the function.
            let p = 1 + r.below(150);
            let pool = params(p);
            let mut s = String::from("class Outer(object):\n    def big(self, q):\n");
            for v in &pool {
                s.push_str(&format!("        {v} = q\n"));
            }
            for i in 0..1 + r.below(30) {
                let k = 1 + r.below(p);
                let vars = tuple_of(r, &pool, k);
                s.push_str("        for x in q:\n            if x:\n");
                s.push_str(&indent(&creation(r, i, &vars), "                "));
            }
            s.push_str("        return 1\n");
            s
        }
        "closure-finally" => {
            // Creations in `finally` bodies, copied at every way out: each
            // copy appends the free variables again.
            let p = 1 + r.below(80);
            let pool = params(p);
            let mut s = format!("def big({}):\n", pool.join(", "));
            let depth = 1 + r.below(3);
            let mut pad = String::from("    ");
            s.push_str("    for z in q:\n");
            pad.push_str("    ");
            for d in 0..depth {
                s.push_str(&format!("{pad}try:\n"));
                pad.push_str("    ");
                for _ in 0..r.below(3) {
                    let exit = ["return 1", "break", "continue", "x = 1"][r.below(4)];
                    s.push_str(&format!("{pad}if z:\n{pad}    {exit}\n"));
                }
                let _ = d;
            }
            s.push_str(&format!("{pad}x = 2\n"));
            for d in (0..depth).rev() {
                pad.truncate(pad.len() - 4);
                s.push_str(&format!("{pad}finally:\n"));
                let k = 1 + r.below(p);
                let vars = tuple_of(r, &pool, k);
                s.push_str(&indent(&creation(r, d, &vars), &format!("{pad}    ")));
            }
            s.push_str("    return 1\n");
            s
        }
        "finally" => {
            // Nested try/finally with many ways out, finally bodies with
            // their own ways out and statements, `with` and except around.
            let depth = 1 + r.below(5);
            let mut s = String::from("def big(a, b):\n    for z in a:\n");
            let mut pad = String::from("        ");
            for _ in 0..depth {
                s.push_str(&format!("{pad}try:\n"));
                pad.push_str("    ");
                for _ in 0..r.below(4) {
                    let exit = ["return b", "break", "continue", "return", "x = z"][r.below(5)];
                    s.push_str(&format!("{pad}if z:\n{pad}    {exit}\n"));
                }
            }
            s.push_str(&format!("{pad}x = f(a, b, z)\n"));
            for _ in 0..depth {
                pad.truncate(pad.len() - 4);
                if s.ends_with(":\n") {
                    s.push_str(&format!("{pad}    pass\n"));
                }
                if r.chance(70) {
                    s.push_str(&format!("{pad}finally:\n"));
                    for _ in 0..1 + r.below(4) {
                        let stmt = [
                            "x = (a, b, z)",
                            "f(a)",
                            "if a:\n    return 2",
                            "y = [i for i in a]",
                        ][r.below(4)];
                        s.push_str(&indent(stmt, &format!("{pad}    ")));
                    }
                } else {
                    s.push_str(&format!("{pad}except E, e:\n{pad}    return e\n"));
                }
            }
            s
        }
        "with" => {
            let depth = 1 + r.below(8);
            let mut s = String::from("def big(a, b):\n    for z in a:\n");
            let mut pad = String::from("        ");
            for i in 0..depth {
                s.push_str(&format!("{pad}with f(a, {i}) as (w{i}, v{i}):\n"));
                pad.push_str("    ");
                for _ in 0..r.below(3) {
                    let exit = ["return b", "break", "continue", "x = z"][r.below(4)];
                    s.push_str(&format!("{pad}if z:\n{pad}    {exit}\n"));
                }
            }
            s.push_str(&format!("{pad}x = (a, b)\n"));
            s
        }
        "yield" => {
            // Yields with many active temporaries (loops, `try`, compares,
            // `with`) and stack items (inside calls and operators).
            let loops = 1 + r.below(10);
            let mut s = String::from("def big(a, b):\n");
            let mut pad = String::from("    ");
            for i in 0..loops {
                match r.below(4) {
                    0 => s.push_str(&format!("{pad}try:\n")),
                    1 => s.push_str(&format!("{pad}with a as w{i}:\n")),
                    _ => s.push_str(&format!("{pad}for i{i} in a:\n")),
                }
                pad.push_str("    ");
            }
            for _ in 0..1 + r.below(6) {
                let y = match r.below(6) {
                    0 => "x = f(a, b, (yield a), (yield b))".to_owned(),
                    1 => "x = a.m(b, (yield 1), c, d, e, g)".to_owned(),
                    2 => "x = a < (yield b) < c".to_owned(),
                    3 => "x = [a, b, (yield a), {1: (yield 2)}]".to_owned(),
                    4 => format!("x = {}", "a + (".repeat(8) + "(yield a)" + &")".repeat(8)),
                    _ => "yield a; yield".to_owned(),
                };
                s.push_str(&format!("{pad}{y}\n"));
            }
            for _ in 0..loops {
                pad.truncate(pad.len() - 4);
                // Close the `try` blocks.
                let open = s
                    .lines()
                    .filter(|l| {
                        l.trim_start() == "try:" && l.len() - l.trim_start().len() == pad.len()
                    })
                    .count();
                let closed = s
                    .lines()
                    .filter(|l| {
                        l.trim_start() == "finally:" && l.len() - l.trim_start().len() == pad.len()
                    })
                    .count();
                if open > closed {
                    s.push_str(&format!("{pad}finally:\n{pad}    x = (yield a)\n"));
                }
            }
            s
        }
        "call" => {
            let n = 1 + r.below(20);
            let mut s = String::from("def big(a, b):\n");
            for _ in 0..n {
                let args = r.below(300);
                let kws = r.below(50);
                let mut parts: Vec<String> = (0..args).map(|i| format!("a{}", i % 7)).collect();
                parts.extend((0..kws).map(|i| format!("k{i}=b")));
                if r.chance(30) {
                    parts.push("*a".to_owned());
                }
                if r.chance(30) {
                    parts.push("**b".to_owned());
                }
                let func = ["f", "a.m", "f(a)(b)", "a.b.c"][r.below(4)];
                s.push_str(&format!("    x = {func}({})\n", parts.join(", ")));
            }
            s
        }
        "unpack" => {
            let n = 1 + r.below(10);
            let mut s = String::from("def big(a, b):\n");
            fn target(r: &mut Rng, depth: usize, width: usize) -> String {
                let parts: Vec<String> = (0..width)
                    .map(|i| {
                        if depth > 0 && r.chance(20) {
                            {
                                let w = 2 + r.below(5);
                                format!("({})", target(r, depth - 1, w))
                            }
                        } else {
                            [
                                format!("v{i}"),
                                format!("a.x{i}"),
                                format!("a[{i}]"),
                                format!("a[{i}:]"),
                            ][r.below(4)]
                            .clone()
                        }
                    })
                    .collect();
                parts.join(", ") + ","
            }
            for _ in 0..n {
                let width = 1 + r.below(200);
                let t = target(r, 3, width);
                match r.below(3) {
                    0 => s.push_str(&format!("    {t} = b\n")),
                    1 => s.push_str(&format!("    for {t} in b:\n        pass\n")),
                    _ => s.push_str(&format!("    {t} = {t} = b\n")),
                }
            }
            s
        }
        "decorated" => {
            let n = 1 + r.below(20);
            let mut s = String::new();
            for i in 0..n {
                for _ in 0..r.below(30) {
                    s.push_str("@d.e(1)\n");
                }
                let defaults = r.below(100);
                let ps: Vec<String> = (0..defaults).map(|j| format!("p{j}=f({j})")).collect();
                if r.chance(50) {
                    let bases: Vec<String> = (0..r.below(50)).map(|j| format!("B{j}")).collect();
                    s.push_str(&format!("class K{i}({}):\n    pass\n", bases.join(", ")));
                } else {
                    s.push_str(&format!("def g{i}({}):\n    return 1\n", ps.join(", ")));
                }
                if r.chance(30) {
                    s.push_str(&format!("x{i} = lambda {}: 1\n", ps.join(", ")));
                }
            }
            s
        }
        "compare" => {
            let n = 1 + r.below(10);
            let mut s = String::from("def big(a, b):\n");
            for _ in 0..n {
                let len = 1 + r.below(200);
                let ops = ["<", "==", "is not", "not in", ">="];
                let mut e = String::from("a");
                for i in 0..len {
                    e.push_str(&format!(" {} b{}", ops[r.below(5)], i % 5));
                }
                let bools: Vec<String> = (0..1 + r.below(200))
                    .map(|i| format!("c{}", i % 3))
                    .collect();
                let op = if r.chance(50) { " and " } else { " or " };
                s.push_str(&format!("    x = {e}\n    y = {}\n", bools.join(op)));
            }
            s
        }
        "display" => {
            let n = 1 + r.below(5);
            let mut s = String::new();
            let scope = r.below(3);
            let pad = if scope == 0 { "" } else { "    " };
            match scope {
                1 => s.push_str("def big(a):\n"),
                2 => s.push_str("def big(a):\n    yield a\n"),
                _ => {}
            }
            for _ in 0..n {
                let len = match r.below(4) {
                    0 => 90 + r.below(20),
                    1 => r.below(300),
                    2 => 4090 + r.below(20),
                    _ => r.below(9000),
                };
                let elts: Vec<String> = (0..len)
                    .map(|i| match r.below(10) {
                        0 => format!("-{i}"),
                        1 => "a".to_owned(),
                        2 => format!("'s{i}'"),
                        3 => format!("{i}.5"),
                        4 => format!("{}L", 10_000_000_000u64 + i as u64),
                        _ => i.to_string(),
                    })
                    .collect();
                let elts = if r.chance(50) {
                    // All constants: moved into `set$$N` methods.
                    (0..len).map(|i| i.to_string()).collect()
                } else {
                    elts
                };
                let text = match r.below(4) {
                    0 => format!("[{}]", elts.join(", ")),
                    1 => format!("({},)", elts.join(", ")),
                    2 if !elts.is_empty() => format!("{{{}}}", elts.join(", ")),
                    _ => {
                        let items: Vec<String> = elts.iter().map(|e| format!("{e}: {e}")).collect();
                        format!("{{{}}}", items.join(", "))
                    }
                };
                s.push_str(&format!("{pad}x = {text}\n"));
            }
            if scope == 0 && s.is_empty() {
                s.push_str("pass\n");
            }
            s
        }
        "strings" => {
            let mut s = String::from("def big(a):\n");
            for i in 0..1 + r.below(4) {
                let len = 15_990 + r.below(60_000);
                let text = match r.below(3) {
                    0 => "x".repeat(len),
                    1 => "\u{e9}".repeat(len),
                    _ => "\u{1F600}".repeat(len / 2),
                };
                let prefix = if r.chance(50) { "u" } else { "" };
                s.push_str(&format!("    x{i} = {prefix}'{text}'\n"));
                s.push_str(&format!(
                    "    y{i} = {}L\n",
                    "7".repeat(1 + r.below(40_000))
                ));
            }
            s
        }
        "imports" => {
            let mut s = String::from("def big(a):\n");
            for i in 0..1 + r.below(10) {
                let names: Vec<String> = (0..1 + r.below(300)).map(|j| format!("n{j}")).collect();
                s.push_str(&format!("    from m{i} import {}\n", names.join(", ")));
                let mods: Vec<String> = (0..1 + r.below(100))
                    .map(|j| format!("a{j}.b{j} as c{j}"))
                    .collect();
                s.push_str(&format!("    import {}\n", mods.join(", ")));
                s.push_str(&format!("    from ...m{i} import x as y, z\n"));
            }
            s
        }
        "aug" => {
            let mut s = String::from("def big(a, b):\n");
            for _ in 0..1 + r.below(100) {
                let target = [
                    "a",
                    "a.b.c",
                    "a[f(b)]",
                    "a[1:2]",
                    "a[::b]",
                    "a[1, 2:3, ...]",
                    "f(a).x[b]",
                ][r.below(7)];
                let op = ["+=", "-=", "**=", "//=", "|=", ">>="][r.below(6)];
                s.push_str(&format!("    {target} {op} f(a, b, c, d, e)\n"));
            }
            s
        }
        "misc" => {
            let mut s = String::from("def big(a, b):\n");
            for _ in 0..1 + r.below(60) {
                let stmt = [
                    "print >>a, b, a, b,",
                    "print",
                    "print >>a",
                    "exec a in b, a",
                    "assert a < b, (a, b)",
                    "raise a, b, a",
                    "del a.x, a[1], a[1:2], a[1, ...]",
                    "x = a[1:2:3, ..., ::2]",
                    "x = `a` if b else (a if a else b)",
                    "while a:\n    break\nelse:\n    pass",
                    "for i in a:\n    continue\nelse:\n    x = 1",
                    "try:\n    a()\nexcept (E, F), e:\n    pass\nexcept G:\n    pass\nexcept:\n    raise\nelse:\n    x = 2",
                    "x = [[i, j] for i in a if i if not i for j in i]",
                    "x = {i: [j for j in i] for i in a if i}",
                    "global g\ng = 1",
                ][r.below(15)];
                s.push_str(&indent(stmt, "    "));
            }
            s
        }
        "deep" => {
            // Many live temporaries: JVM locals beyond 255 need `wide`.
            let depth = 60 + r.below(150);
            let e = match r.below(4) {
                0 => {
                    let mut e = String::from("a");
                    for _ in 0..depth {
                        e = format!("b < ({e}) < c");
                    }
                    e
                }
                1 => {
                    let mut e = String::from("a");
                    for i in 0..depth {
                        e = format!("[{e} for i{i} in b]");
                    }
                    e
                }
                2 => {
                    let mut e = String::from("a");
                    for _ in 0..depth {
                        e = format!("f(a, k=({e}))");
                    }
                    e
                }
                _ => {
                    let mut e = String::from("a");
                    for _ in 0..depth {
                        e = format!("a[b:({e})]");
                    }
                    e
                }
            };
            let yield_ = if r.chance(50) { "    yield 1\n" } else { "" };
            format!("def big(a, b, c):\n    x = {e}\n    y = {e}\n{yield_}")
        }
        "names" => {
            // The random statements with a large pool of names and nested
            // units that read them (free variables passed through).
            let p = 50 + r.below(250);
            let pool = params(p);
            let mut s = format!("def big({}):\n", pool.join(", "));
            for i in 0..20 + r.below(200) {
                let a = &pool[r.below(p)];
                let c = &pool[r.below(p)];
                s.push_str(&match r.below(6) {
                    0 => format!("    {a} = f({c}, {a})\n"),
                    1 => format!("    def h{i}(x={c}):\n        return lambda: ({a}, {c}, x)\n"),
                    2 => format!("    if {a}:\n        {c} += 1\n"),
                    3 => format!("    k{i} = [({a}, {c}) for {a} in {c}]\n"),
                    4 => format!("    k{i} = list(({a}, z) for z in {c})\n"),
                    _ => format!("    class C{i}:\n        x = {a}\n        def m(self):\n            return {c}\n"),
                });
            }
            s
        }
        "wide" => {
            // Methods past 32 KB: jumps whose span needs `goto_w`.
            let n = 1_000 + r.below(1_200);
            let mut s = String::from("def big(a):\n");
            let kind = r.below(4);
            let head = ["if a:", "while a:", "for i in a:", "try:"][kind];
            s.push_str(&format!("    {head}\n"));
            for i in 0..n {
                s.push_str(&format!("        x = abs({})\n", i % 7));
                if r.chance(1) {
                    s.push_str(
                        [
                            "        if a:\n            pass\n",
                            "        y = a and b or c\n",
                        ][r.below(2)],
                    );
                }
            }
            if kind == 3 {
                s.push_str("    finally:\n        x = 1\n");
            } else if r.chance(50) {
                s.push_str("    else:\n        x = 2\n");
            }
            s.push_str("    return 1\n");
            s
        }
        _ => {
            // Combinations: closures in finally bodies of generators,
            // calls with yields, wide displays of closures.
            let p = 1 + r.below(100);
            let pool = params(p);
            let mut s = format!(
                "def big({}):\n    for z in q:\n        try:\n",
                pool.join(", ")
            );
            for i in 0..1 + r.below(20) {
                let k = 1 + r.below(p);
                let vars = tuple_of(r, &pool, k);
                match r.below(4) {
                    0 => s.push_str(&indent(&creation(r, i, &vars), "            ")),
                    1 => s.push_str(&format!("            x = f({vars}, (yield z))\n")),
                    2 => s.push_str("            if z:\n                break\n"),
                    _ => s.push_str(&format!("            x = [lambda: {vars} for w in z]\n")),
                }
            }
            s.push_str("        finally:\n");
            let k = 1 + r.below(p);
            let vars = tuple_of(r, &pool, k);
            s.push_str(&indent(&creation(r, 99, &vars), "            "));
            s
        }
    }
}
