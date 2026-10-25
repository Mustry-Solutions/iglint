//! Generated star-import graphs between library modules, replayed from
//! Jython's records (review round 30, B1).
//!
//! Each case is a library: `core.leaf` (`import json`), `lib.exports`
//! (`from core import leaf`), and two to six modules `s<i>` / `pk.s<i>`
//! that star-import each other, `lib.exports`, the packages `core` and
//! `lib`, or a module named through another module's import (`from s1.e
//! import *` where `s1` does `import lib.exports as e`); that import
//! `lib.exports` or another module under a name; and that sometimes set a
//! literal `__all__`. The star imports form chains, diamonds and cycles.
//! Module `b` reads `json` of `core.leaf` through one path (`s2.leaf.json`,
//! `s0.h.exports.leaf.json`, a star import in `b` itself). Each case comes
//! with the same library with `import os` instead of `import json`.
//!
//! The oracle (`oracle/run-oracle.sh --library-routes`, a `chains` file)
//! runs both on Ignition's ScriptManager; wherever the two records differ,
//! `import json` is needed, and `SCR102` must not offer its removal as a
//! safe fix. The committed records (`tests/fixtures/library-routes/
//! chains-stars.expected`) are replayed without Docker; the cases are
//! regenerated here. Recording a new batch (needs Docker and the 8.3.3
//! image):
//!
//! ```sh
//! IGLINT_STAR_GRAPHS_OUT=$HOME/.cache/stars cargo test -p iglint_linter --lib \
//!     star_graphs::write_cases -- --ignored
//! oracle/run-oracle.sh --library-routes $HOME/.cache/stars \
//!     $HOME/.cache/stars/list.txt $HOME/.cache/stars/out.txt
//! sed -n '/^### gen/,$p' $HOME/.cache/stars/out.txt \
//!     > crates/iglint_linter/tests/fixtures/library-routes/chains-stars.expected
//! ```

use crate::diagnostic::Applicability;
use crate::rules::script::testing::with_library;

/// The committed records, one per case, in order.
const EXPECTED: &str = include_str!("../../../tests/fixtures/library-routes/chains-stars.expected");
/// The batch the records are of.
const SEED: u64 = 30;
const COUNT: usize = 600;

const LEAF: &str = "import json\ndef f():\n    return 1\n";
const LEAF_REMOVED: &str = "import os\ndef f():\n    return 1\n";

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).unwrap_or(0)
    }

    /// True with probability `percent` / 100.
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

/// A generated case: its modules (`dotted`, code) without `core.leaf`.
struct Case {
    name: String,
    modules: Vec<(String, String)>,
}

fn generate(rng: &mut Rng, name: String) -> Case {
    let n = 2 + rng.below(5);
    let names: Vec<String> = (0..n)
        .map(|i| {
            if rng.chance(30) {
                format!("pk.s{i}")
            } else {
                format!("s{i}")
            }
        })
        .collect();
    // What each module binds by name (for paths through it): name, target.
    let mut binds: Vec<Vec<(&'static str, String)>> = vec![Vec::new(); n];
    let mut code: Vec<Vec<String>> = vec![Vec::new(); n];
    // Modules outside the chain, as written.
    let mut extra: Vec<(String, String)> = Vec::new();
    let chain = rng.chance(40);
    if chain {
        // A chain of star imports from `s0` to the last module, one of
        // them naming its module through a name another, later module
        // binds by an import (`from s2.h import *`, `s2` doing `import s1
        // as h`; `from s3.e import *`, `s3` doing `import lib.exports as
        // e`), the last ending in `lib.exports` directly or through such a
        // name: the shape of review round 30, B1, also with more hops.
        // Hops through a name: of a module outside the chain (`hx<i>`; one
        // in the chain would load while the chain loads, a cycle), or, for
        // the last, of the module itself (bound before the star import).
        for i in 0..n {
            let next = if i + 1 < n {
                names[i + 1].clone()
            } else {
                "lib.exports".to_owned()
            };
            let through = i + 1 == n || rng.chance(25);
            let star = if i + 1 == n && rng.chance(12) {
                // The package: binds its module `leaf`.
                "core".to_owned()
            } else if !through {
                next
            } else {
                let (line, b) = match (i + 1 == n, rng.chance(50)) {
                    (true, true) => ("from lib import exports".to_owned(), "exports"),
                    (true, false) => ("import lib.exports as e".to_owned(), "e"),
                    (false, _) => (format!("import {next} as h"), "h"),
                };
                if i + 1 == n && rng.chance(40) {
                    code[i].push(line);
                    format!("{}.{b}", names[i])
                } else {
                    let helper = if rng.chance(30) {
                        format!("pk.hx{i}")
                    } else {
                        format!("hx{i}")
                    };
                    // Sometimes bound twice (a variable: the star import
                    // cannot be resolved to one module).
                    let code = if rng.chance(25) {
                        let indented = line.replace('\n', "\n    ");
                        format!("try:\n    {indented}\nexcept ImportError:\n    {b} = None\n")
                    } else {
                        format!("{line}\n")
                    };
                    extra.push((helper.clone(), code));
                    format!("{helper}.{b}")
                }
            };
            code[i].push(format!("from {star} import *"));
        }
    } else {
        // Mostly a module later in the list (a cycle raises `ImportError` while
        // the library loads), sometimes an earlier one (a cycle), never the
        // module itself; `None` for the last module, mostly.
        let later = |rng: &mut Rng, i: usize| {
            if i + 1 < n && !rng.chance(8) {
                Some(i + 1 + rng.below(n - i - 1))
            } else if i + 1 == n && !rng.chance(8) {
                None
            } else {
                Some((i + 1 + rng.below(n - 1)) % n)
            }
        };
        // First what modules bind by name, then their star imports (which may
        // name a module through another module's import).
        for i in 0..n {
            for _ in 0..rng.below(3) {
                let k = rng.below(100);
                let other = later(rng, i);
                let (line, bound) = if k < 35 || (other.is_none() && k < 85) {
                    (
                        "from lib import exports".to_owned(),
                        Some(("exports", "lib.exports".to_owned())),
                    )
                } else if k < 60 {
                    (
                        "import lib.exports as e".to_owned(),
                        Some(("e", "lib.exports".to_owned())),
                    )
                } else if let Some(other) = other.filter(|_| k < 85) {
                    (
                        format!("import {} as h", names[other]),
                        Some(("h", names[other].clone())),
                    )
                } else {
                    let all = ["['leaf']", "['f']", "['leaf', 'exports']", "[]"][rng.below(4)];
                    (format!("__all__ = {all}"), None)
                };
                code[i].push(line);
                if let Some((name, target)) = bound {
                    match binds[i].iter_mut().find(|(n, _)| *n == name) {
                        Some(b) => b.1 = target,
                        None => binds[i].push((name, target)),
                    }
                }
            }
        }
        for i in 0..n {
            for _ in 0..1 + rng.below(2) {
                let Some(other) = later(rng, i) else {
                    let star =
                        ["lib.exports", "lib.exports", "lib.exports", "core", "lib"][rng.below(5)];
                    let at = rng.below(code[i].len() + 1);
                    code[i].insert(at, format!("from {star} import *"));
                    continue;
                };
                let star = match rng.below(100) {
                    k if k < 35 && !binds[other].is_empty() => {
                        let (b, _) = &binds[other][rng.below(binds[other].len())];
                        format!("{}.{b}", names[other])
                    }
                    k if k < 75 => names[other].clone(),
                    k if k < 88 => "lib.exports".to_owned(),
                    k if k < 94 => "core".to_owned(),
                    _ => "lib".to_owned(),
                };
                let line = format!("from {star} import *");
                // Before or after the module's own bindings.
                let at = rng.below(code[i].len() + 1);
                code[i].insert(at, line);
            }
        }
    }
    // The user's path to `json` of `core.leaf`: through names a module
    // binds, or straight to `leaf` (which star imports may bring).
    let m = if chain { 0 } else { rng.below(n) };
    let user = if rng.chance(20) {
        format!(
            "from {} import *\ndef g(n):\n    return leaf.json.dumps(1)\n",
            names[m]
        )
    } else {
        let mut segs = vec![names[m].clone()];
        let mut at = Some(m);
        for _ in 0..3 {
            let Some(a) = at.filter(|a| !binds[*a].is_empty()) else {
                break;
            };
            if rng.chance(35) {
                break;
            }
            let (b, target) = binds[a][rng.below(binds[a].len())].clone();
            segs.push(b.to_owned());
            at = names.iter().position(|x| *x == target);
        }
        segs.push("leaf".to_owned());
        format!("def g(n):\n    return {}.json.dumps(1)\n", segs.join("."))
    };
    let mut modules = vec![(
        "lib.exports".to_owned(),
        "from core import leaf\n".to_owned(),
    )];
    for (m, lines) in names.iter().zip(code) {
        let mut text = if lines.is_empty() {
            "pass".to_owned()
        } else {
            lines.join("\n")
        };
        text.push('\n');
        modules.push((m.clone(), text));
    }
    modules.extend(extra);
    modules.push(("b".to_owned(), user));
    Case { name, modules }
}

fn cases() -> Vec<Case> {
    let mut rng = Rng(SEED);
    (0..COUNT)
        .map(|i| generate(&mut rng, format!("gen{SEED}-{i}")))
        .collect()
}

/// The chain case text of `case` with `leaf` as `core.leaf`.
fn render(case: &Case, name: &str, leaf: &str) -> String {
    let mut out = format!("### {name} gen\n== core.leaf\n{leaf}");
    for (m, code) in &case.modules {
        out.push_str(&format!("== {m}\n{code}"));
    }
    out
}

/// Writes the cases and a list for the oracle into `IGLINT_STAR_GRAPHS_OUT`.
#[test]
#[ignore = "writes oracle input"]
fn write_cases() {
    let dir =
        std::path::PathBuf::from(std::env::var("IGLINT_STAR_GRAPHS_OUT").expect("output dir"));
    std::fs::create_dir_all(&dir).expect("dir");
    let mut text = String::new();
    for case in cases() {
        text.push_str(&render(&case, &case.name, LEAF));
        text.push_str(&render(
            &case,
            &format!("{}-removed", case.name),
            LEAF_REMOVED,
        ));
    }
    std::fs::write(dir.join("chains-stars.cases"), text).expect("cases");
    std::fs::write(dir.join("list.txt"), "chains-stars.cases\n").expect("list");
}

/// Wherever Jython needs `import json` of `core.leaf` (the records with and
/// without it differ), `SCR102` offers no safe removal of it.
#[test]
fn generated_star_graphs_keep_read_imports() {
    let mut records: Vec<(&str, &str)> = Vec::new();
    let mut lines = EXPECTED.lines();
    while let Some(header) = lines.next() {
        let name = header.strip_prefix("### ").expect("record header");
        records.push((name, lines.next().expect("record")));
    }
    let cases = cases();
    assert_eq!(records.len(), 2 * cases.len(), "records of another batch");
    let mut needed = 0;
    let mut failures = Vec::new();
    for (case, pair) in cases.iter().zip(records.chunks(2)) {
        assert_eq!(pair[0].0, case.name);
        assert_eq!(pair[1].0, format!("{}-removed", case.name));
        if pair[0].1 == pair[1].1 {
            continue;
        }
        needed += 1;
        let f = case.modules.iter().fold(
            with_library(
                crate::rules::test_support::Fixture::new(),
                "core/leaf",
                LEAF,
            ),
            |f, (m, code)| with_library(f, &m.replace('.', "/"), code),
        );
        let found = f.lint(&["SCR102"]);
        let removed = found.iter().any(|d| {
            d.file
                .path()
                .to_string_lossy()
                .replace('\\', "/")
                .ends_with("script-python/core/leaf/code.py")
                && d.message.contains("`json`")
                && d.fix
                    .as_ref()
                    .is_some_and(|f| f.applicability == Applicability::Safe)
        });
        if removed {
            failures.push(render(case, &case.name, LEAF));
        }
    }
    assert!(needed >= 100, "{needed}");
    eprintln!("NEEDED {needed}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
