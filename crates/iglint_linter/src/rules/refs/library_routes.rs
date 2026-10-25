//! Every measured route from one library module to another's names
//! (review round 26): replayed from the oracle records in
//! `tests/fixtures/library-routes/` (`oracle/run-oracle.sh
//! --library-routes`, `oracle/jython_library_routes_oracle.py`).
//!
//! The table holds each member of Ignition's library module and package
//! objects that the oracle found (as `m.<member>` and `pkg.<member>`) and
//! each route of `routes.cases`: special attributes, Jython's accessors,
//! the builtins and standard functions that take an object and a name,
//! aliases, containers and package chains. For every route, the unused
//! import of the module it reaches (`import re` in `m` or `pkg.m`) is
//! never removed by a safe fix, `REF002` says nothing, and a route that may
//! reach any name of the module (`value`) keeps `REF012` silent about its
//! function `f`. A route that names `re` (`literal`) uses it.
//!
//! The attributes of `m.f` the oracle found on a function are routes too,
//! and so is each `runScript` expression of `runscript.cases` (review round
//! 27), in a tag expression and in a Perspective binding (`perspective:`
//! cases only there): the same for `value` and `literal`; a `plain` route
//! (a call of `f`) uses `f` and keeps the import's fix safe; an `unknown`
//! route (code that is not vetted, reads a frame, a file or the
//! Perspective objects) keeps every fix unsafe.
//!
//! `chains.cases` (review round 28) holds paths through the imports of
//! library modules (`m0.util.json` when `m0` does `from lib import util`,
//! aliases, chains of 12 and 70 imports, cycles, star imports, ambiguous
//! bindings, `runScript` targets), each with the modules it needs, measured
//! on Ignition's ScriptManager: what the path uses is used (`SCR102`,
//! `REF012` silent), a path that raises `AttributeError` or `TypeError`
//! is reported (`REF002`, `REF003`), and what a path may reach but cannot
//! follow counts as used as a value.

use iglint_project::library::{
    HARMLESS_ATTRIBUTES, MODULE_OBJECT_MEMBERS, OBJECT_MEMBERS, PACKAGE_OBJECT_MEMBERS,
    is_special_attribute, is_value_attribute,
};

use crate::diagnostic::Applicability;
use crate::rules::script::testing::{library, with_library};

const CASES: &str = include_str!("../../../tests/fixtures/library-routes/routes.cases");
const RUN_SCRIPT: &str = include_str!("../../../tests/fixtures/library-routes/runscript.cases");
const CHAINS: &str = include_str!("../../../tests/fixtures/library-routes/chains.cases");
const EXPECTED: &str = include_str!("../../../tests/fixtures/library-routes/routes.expected");
const MODULE: &str = "import re\ndef f():\n    return 1\n";

/// What a route reaches in its target module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    /// Any name: the module is used as a value.
    Value,
    /// The name `re` only.
    Literal,
    /// Nothing but a name or documentation ([`HARMLESS_ATTRIBUTES`]).
    Harmless,
    /// A call or read of `f`: the import's fix stays safe.
    Plain,
    /// Code that is not vetted, reads a frame or a file: no fix is safe.
    Unknown,
}

/// Where a route is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Site {
    /// The body of `def g(n):` in the library module `b`.
    Script,
    /// An expression tag of the gateway.
    Tag,
    /// An expression binding of a Perspective view.
    View,
}

struct Route {
    name: String,
    expect: Expect,
    target: &'static str,
    body: String,
    site: Site,
}

fn records() -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for line in EXPECTED.lines() {
        match line.strip_prefix("### ") {
            Some(name) => out.push((name.to_owned(), Vec::new())),
            None => out.last_mut().expect("record").1.push(line.to_owned()),
        }
    }
    out
}

/// The measured members, as `(object, member)`.
fn members() -> Vec<(String, String)> {
    let records = records();
    let (_, lines) = records
        .iter()
        .find(|(n, _)| n == "members")
        .expect("members");
    lines
        .iter()
        .map(|l| {
            let (object, member) = l.split_once(' ').expect("member line");
            (object.to_owned(), member.to_owned())
        })
        .collect()
}

/// The cases of a `.cases` file: `(name, expect, target, body)`.
fn cases(text: &'static str) -> Vec<(String, Expect, &'static str, String)> {
    let mut out = Vec::new();
    for chunk in text.split("### ").filter(|c| !c.is_empty()) {
        let (header, body) = chunk.split_once('\n').expect("case header");
        let mut words = header.split_whitespace();
        let name = words.next().expect("name").to_owned();
        let expect = match words.next() {
            Some("value") => Expect::Value,
            Some("literal") => Expect::Literal,
            Some("plain") => Expect::Plain,
            Some("unknown") => Expect::Unknown,
            other => panic!("{name}: expectation {other:?}"),
        };
        let target = match words.next() {
            Some("m") => "m",
            Some("pkg.m") => "pkg/m",
            other => panic!("{name}: target {other:?}"),
        };
        out.push((name, expect, target, body.to_owned()));
    }
    out
}

fn routes() -> Vec<Route> {
    let mut out = Vec::new();
    for (object, member) in members() {
        let (receiver, target) = match object.as_str() {
            "module" => ("m", "m"),
            "package" => ("pkg", "pkg/m"),
            // `m.f` is a function; the other objects are not in the module.
            "function" => ("m.f", "m"),
            _ => continue,
        };
        out.push(Route {
            name: format!("{object} {member}"),
            expect: if HARMLESS_ATTRIBUTES.contains(&member.as_str()) {
                Expect::Harmless
            } else {
                Expect::Value
            },
            target,
            body: format!("    return {receiver}.{member}\n"),
            site: Site::Script,
        });
    }
    for (name, expect, target, body) in cases(CASES) {
        out.push(Route {
            name,
            expect,
            target,
            body,
            site: Site::Script,
        });
    }
    for (name, expect, target, body) in cases(RUN_SCRIPT) {
        let body = body.trim();
        let (sites, body): (&[Site], &str) = match body.strip_prefix("perspective:") {
            Some(body) => (&[Site::View], body),
            None => (&[Site::Tag, Site::View], body),
        };
        for &site in sites {
            out.push(Route {
                name: format!("{name} ({site:?})"),
                expect,
                target,
                body: body.to_owned(),
                site,
            });
        }
    }
    out
}

/// The oracle measured every case of `routes.cases`, in order, and the
/// members of the module and package objects are those the linter knows.
#[test]
fn library_object_members_match_the_oracle() {
    let records = records();
    let cases: Vec<&str> = CASES
        .lines()
        .chain(RUN_SCRIPT.lines())
        .chain(CHAINS.lines())
        .filter_map(|l| l.strip_prefix("### "))
        .map(|h| h.split_whitespace().next().unwrap_or(""))
        .collect();
    let recorded: Vec<&str> = records
        .iter()
        .map(|(n, _)| n.as_str())
        .filter(|n| *n != "members")
        .collect();
    assert_eq!(
        cases, recorded,
        "re-run oracle/run-oracle.sh --library-routes"
    );
    for (name, lines) in &records {
        assert_eq!(lines.len(), if name == "members" { lines.len() } else { 1 });
    }
    let members = members();
    let of = |object: &str| -> Vec<&str> {
        let mut names: Vec<&str> = members
            .iter()
            .filter(|(o, m)| o == object && !m.starts_with("__"))
            .map(|(_, m)| m.as_str())
            .collect();
        names.sort_unstable();
        names
    };
    assert_eq!(of("module"), MODULE_OBJECT_MEMBERS);
    assert_eq!(of("package"), PACKAGE_OBJECT_MEMBERS);
    // The other objects a dotted name reaches: functions, methods,
    // classes, instances, generators.
    let mut others: Vec<&str> = members
        .iter()
        .filter(|(o, m)| o != "module" && o != "package" && !m.starts_with("__"))
        .map(|(_, m)| m.as_str())
        .collect();
    others.sort_unstable();
    others.dedup();
    assert_eq!(others, OBJECT_MEMBERS);
    for (object, m) in &members {
        assert!(
            is_value_attribute(m) || HARMLESS_ATTRIBUTES.contains(&m.as_str()),
            "{object} {m}"
        );
    }
    // Every special member is a use of the object, but names and
    // documentation.
    for (_, m) in &members {
        if m.starts_with("__") {
            assert!(
                is_special_attribute(m) || HARMLESS_ATTRIBUTES.contains(&m.as_str()),
                "{m}"
            );
        }
    }
}

/// The fixture of a route: modules `m` and `pkg.m`, and the route in the
/// function `g` of module `b`, in an expression tag or in a view's binding.
fn fixture(route: &Route) -> crate::rules::test_support::Fixture {
    let modules = |f| with_library(with_library(f, "m", MODULE), "pkg/m", MODULE);
    let expression = serde_json::to_string(&route.body).expect("json");
    match route.site {
        Site::Script => with_library(
            modules(library("m", MODULE)),
            "b",
            &format!("def g(n):\n{}", route.body),
        ),
        Site::Tag => crate::rules::tag::fixtures::tags(
            modules(crate::rules::tag::fixtures::gateway()),
            &format!(
                r#"[{{"name": "T", "tagType": "AtomicTag", "valueSource": "expr", "dataType": "String", "expression": {expression}}}]"#
            ),
            "[]",
        ),
        Site::View => modules(library("m", MODULE)).with_view(
            "Main",
            &format!(
                r#"{{"root": {{"type": "ia.display.label", "meta": {{"name": "root"}}, "props": {{"text": ""}},
                "propConfig": {{"props.text": {{"binding": {{"type": "expr", "config": {{"expression": {expression}}}}}}}}}}}}}"#
            ),
        ),
    }
}

/// Each route keeps the import it may reach and the function it may call,
/// across modules (`SCR102`, `REF012`), and names nothing that is missing
/// (`REF002`).
#[test]
fn every_route_to_a_library_module_counts() {
    let routes = routes();
    assert!(routes.len() > 250, "{}", routes.len());
    let mut failures = Vec::new();
    for route in &routes {
        let f = fixture(route);
        let found = f.lint(&["SCR102", "REF012", "REF002"]);
        let target = format!("script-python/{}/code.py", route.target);
        let in_target = |code: &str| {
            found
                .iter()
                .filter(|d| {
                    d.rule.code() == code
                        && d.file
                            .path()
                            .to_string_lossy()
                            .replace('\\', "/")
                            .ends_with(&target)
                })
                .collect::<Vec<_>>()
        };
        let imports = in_target("SCR102");
        let safe = imports.iter().any(|d| {
            d.fix
                .as_ref()
                .is_some_and(|f| f.applicability == Applicability::Safe)
        });
        let unused_f = !in_target("REF012").is_empty();
        let missing = found.iter().any(|d| d.rule.code() == "REF002");
        let ok = match route.expect {
            Expect::Value => !safe && !unused_f && !missing,
            Expect::Literal => imports.is_empty() && !missing,
            Expect::Harmless => !missing,
            Expect::Plain => safe && !unused_f && !missing,
            Expect::Unknown => !imports.is_empty() && !safe && !missing,
        };
        if !ok {
            failures.push(format!(
                "{} ({:?}): SCR102 {} (safe {safe}), REF012 f {unused_f}, REF002 {missing}",
                route.name,
                route.expect,
                imports.len()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A library object held by another module's variable is that module's
/// use of it as a value (review round 27): `h.alias.dict`, `h.alias.clear`
/// and `runScript("h.alias.dict.get", 0, "re")` reach `m` without a
/// special attribute, and the assignment `alias = m` already makes every
/// name of `m` used (and hands its shared objects to code that is not
/// followed), so no fix of the workspace is safe.
#[test]
fn library_objects_held_by_variables_are_values() {
    for (holder, user) in [
        ("alias = m\n", "def g(n):\n    return h.alias.dict[n]\n"),
        (
            "alias = m\n",
            "def g(n):\n    return h.alias.__findattr_ex__(n)\n",
        ),
        ("alias = pkg\n", "def g():\n    h.alias.clear()\n"),
        (
            "def get():\n    return m\nalias = get()\n",
            "def g(n):\n    return h.alias.getDict()[n]\n",
        ),
        (
            "alias = [m]\n",
            "def g(n):\n    return h.alias[0].dict[n]\n",
        ),
    ] {
        let f = with_library(
            with_library(
                with_library(library("m", MODULE), "pkg/m", MODULE),
                "h",
                holder,
            ),
            "b",
            user,
        );
        let found = f.lint(&["SCR102"]);
        let safe = found.iter().any(|d| {
            d.fix
                .as_ref()
                .is_some_and(|f| f.applicability == Applicability::Safe)
        });
        assert!(!safe, "{holder}{user}");
    }
}

/// What a chain of `chains.cases` does.
#[derive(Debug)]
enum ChainExpect {
    /// Reads these names (`module.name`): their imports are used.
    Uses(Vec<String>),
    /// Calls these functions (`module.name`): they are referenced.
    Calls(Vec<String>),
    /// Raises `AttributeError`: `REF002` reports it.
    Missing,
    /// Raises `TypeError`: `REF003` reports it.
    Arity,
    /// May read any name of these modules.
    Value(Vec<String>),
    /// Replaces these names (`module.name`; review round 29): `REF002`
    /// and `REF003` stay silent, and no safe fix removes their imports.
    Stores(Vec<String>),
    /// The case named, with the import it relies on removed: Jython's
    /// record differs (what the import is needed for).
    Without(String),
}

/// A case of `chains.cases`: name, expectation, modules (`dotted`, code)
/// and the `runScript` expression, if any.
type Chain = (String, ChainExpect, Vec<(String, String)>, Option<String>);

/// The cases of `chains.cases`.
fn chains() -> Vec<Chain> {
    let mut out = Vec::new();
    for chunk in CHAINS.split("### ").filter(|c| !c.is_empty()) {
        let (header, body) = chunk.split_once('\n').expect("case header");
        let mut words = header.split_whitespace();
        let name = words.next().expect("name").to_owned();
        let kind = words.next().expect("expectation");
        let targets: Vec<String> = words.map(str::to_owned).collect();
        let expect = match kind {
            "uses" => ChainExpect::Uses(targets),
            "calls" => ChainExpect::Calls(targets),
            "missing" => ChainExpect::Missing,
            "arity" => ChainExpect::Arity,
            "value" => ChainExpect::Value(targets),
            "stores" => ChainExpect::Stores(targets),
            "without" => ChainExpect::Without(targets.join(" ")),
            other => panic!("{name}: expectation {other}"),
        };
        let mut modules: Vec<(String, String)> = Vec::new();
        for line in body.split_inclusive('\n') {
            match line.strip_prefix("== ") {
                Some(module) => modules.push((module.trim().to_owned(), String::new())),
                None => modules.last_mut().expect("section").1.push_str(line),
            }
        }
        let expression = modules
            .iter()
            .position(|(m, _)| m == "runScript")
            .map(|i| modules.remove(i).1.trim().to_owned());
        out.push((name, expect, modules, expression));
    }
    out
}

/// Every chain through library imports counts what it uses, and reports
/// what Jython reports (review rounds 28 and 29: also deletes, augmented
/// assignments and stores through paths, and `__all__` changes), from a
/// script, a tag and a view.
#[test]
fn every_chain_through_library_imports_counts() {
    let records = records();
    let cases = chains();
    assert!(cases.len() >= 80, "{}", cases.len());
    let mut failures = Vec::new();
    for (name, expect, modules, expression) in &cases {
        let (_, record) = records
            .iter()
            .find(|(n, _)| n == name)
            .expect("oracle record");
        let record = record.first().map(String::as_str).unwrap_or("");
        let measured = match expect {
            ChainExpect::Missing => record == "raised AttributeError",
            ChainExpect::Arity => record == "raised TypeError",
            ChainExpect::Without(with) => {
                let (_, other) = records
                    .iter()
                    .find(|(n, _)| n == with)
                    .expect("oracle record of the case with the import");
                other.first().is_some_and(|o| o != record)
            }
            _ => record.starts_with("ok "),
        };
        assert!(measured, "{name}: oracle says {record}");
        if matches!(expect, ChainExpect::Without(_)) {
            continue;
        }
        let library = |f| {
            modules.iter().fold(f, |f, (m, code)| {
                with_library(f, &m.replace('.', "/"), code)
            })
        };
        let fixtures = match expression {
            None => vec![library(crate::rules::test_support::Fixture::new())],
            Some(e) => {
                let e = serde_json::to_string(e).expect("json");
                vec![
                    library(crate::rules::test_support::Fixture::new()).with_view(
                        "Main",
                        &format!(
                            r#"{{"root": {{"type": "ia.display.label", "meta": {{"name": "root"}}, "props": {{"text": ""}},
                            "propConfig": {{"props.text": {{"binding": {{"type": "expr", "config": {{"expression": {e}}}}}}}}}}}}}"#
                        ),
                    ),
                    crate::rules::tag::fixtures::tags(
                        library(crate::rules::tag::fixtures::gateway()),
                        &format!(
                            r#"[{{"name": "T", "tagType": "AtomicTag", "valueSource": "expr", "dataType": "String", "expression": {e}}}]"#
                        ),
                        "[]",
                    ),
                ]
            }
        };
        for f in fixtures {
            let found = f.lint(&["SCR102", "REF012", "REF002", "REF003"]);
            let file = |d: &crate::Diagnostic| d.file.path().to_string_lossy().replace('\\', "/");
            let in_module = |code: &str, module: &str| -> Vec<&crate::Diagnostic> {
                let path = format!("script-python/{}/code.py", module.replace('.', "/"));
                found
                    .iter()
                    .filter(|d| d.rule.code() == code && file(d).ends_with(&path))
                    .collect()
            };
            let split = |t: &String| -> (String, String) {
                let (m, n) = t.rsplit_once('.').expect("module.name");
                (m.to_owned(), format!("`{n}`"))
            };
            let named = |code: &str, t: &String| {
                let (m, n) = split(t);
                in_module(code, &m)
                    .into_iter()
                    .any(|d| d.message.contains(&n))
            };
            let reported = |code: &str| found.iter().any(|d| d.rule.code() == code);
            let ok = match expect {
                ChainExpect::Uses(names) => {
                    !reported("REF002") && names.iter().all(|t| !named("SCR102", t))
                }
                ChainExpect::Calls(names) => {
                    !reported("REF002") && names.iter().all(|t| !named("REF012", t))
                }
                ChainExpect::Missing => reported("REF002"),
                ChainExpect::Arity => reported("REF003"),
                ChainExpect::Stores(names) => {
                    !reported("REF002")
                        && !reported("REF003")
                        && names.iter().all(|t| {
                            let (m, n) = split(t);
                            !in_module("SCR102", &m).iter().any(|d| {
                                d.message.contains(&n)
                                    && d.fix
                                        .as_ref()
                                        .is_some_and(|f| f.applicability == Applicability::Safe)
                            })
                        })
                }
                ChainExpect::Without(_) => true,
                ChainExpect::Value(modules) => {
                    !reported("REF002")
                        && modules.iter().all(|m| {
                            in_module("REF012", m).is_empty()
                                && !in_module("SCR102", m).iter().any(|d| {
                                    d.fix
                                        .as_ref()
                                        .is_some_and(|f| f.applicability == Applicability::Safe)
                                })
                        })
                }
            };
            if !ok {
                let summary: Vec<String> = found
                    .iter()
                    .map(|d| format!("{} {} {}", d.rule.code(), file(d), d.message))
                    .collect();
                failures.push(format!("{name} ({expect:?}): {summary:#?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Generated chains through the imports of library modules (review round
/// 28), of every form `chains.cases` measured and of any length: the
/// import a path reads at the end of the chain is used, a function it
/// calls is referenced, and a missing name is reported where every import
/// of the chain binds its name once; through a name bound otherwise too
/// (`try:` / `except: a = None`) or a star import, no import the path may
/// read is removed by a safe fix and a missing name is not reported.
#[test]
fn generated_import_chains_resolve_like_jython() {
    let mut seed: u64 = 0x2028_1028_9e37_79b9;
    let mut next = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    let mut failures = Vec::new();
    for case in 0..160 {
        let hops = 1 + next(12);
        // Module `i` of the chain is `c{i}` or `pk.c{i}`.
        let names: Vec<String> = (0..=hops)
            .map(|i| {
                if i > 0 && next(2) == 0 {
                    format!("pk.c{i}")
                } else {
                    format!("c{i}")
                }
            })
            .collect();
        let mut modules: Vec<(String, String)> = Vec::new();
        let mut path = names[0].clone();
        let mut certain = true;
        for i in 0..hops {
            let target = &names[i + 1];
            let (code, segment) = match (next(6), target.split_once('.')) {
                (0, Some((package, module))) => (
                    format!("from {package} import {module}\n"),
                    module.to_owned(),
                ),
                (1, Some(_)) => (format!("import {target}\n"), target.clone()),
                (1, None) => (format!("import {target}\n"), target.clone()),
                (2, _) => (format!("from {target} import *\n"), String::new()),
                (3, _) => {
                    certain = false;
                    (
                        format!(
                            "try:\n    import {target} as a\nexcept ImportError:\n    a = None\n"
                        ),
                        "a".to_owned(),
                    )
                }
                (4, _) => (
                    format!("def init():\n    global a\n    import {target} as a\ninit()\n"),
                    "a".to_owned(),
                ),
                _ => (format!("import {target} as a\n"), "a".to_owned()),
            };
            if segment.is_empty() {
                // The star import binds the next module's names here.
                certain = false;
            } else {
                path.push('.');
                path.push_str(&segment);
            }
            modules.push((names[i].replace('.', "/"), code));
        }
        let last = names[hops].replace('.', "/");
        modules.push((
            last.clone(),
            "import json\ndef f():\n    return 1\n".to_owned(),
        ));
        // The star import of the last module needs a name to continue.
        let (body, expect) = match next(3) {
            0 => (format!("    return {path}.json.dumps(1)\n"), "uses"),
            1 => (format!("    return {path}.f()\n"), "calls"),
            _ => (format!("    return {path}.nothere\n"), "missing"),
        };
        let f = modules.iter().fold(
            crate::rules::test_support::Fixture::new(),
            |f, (m, code)| with_library(f, m, code),
        );
        let f = with_library(f, "b", &format!("def g(n):\n{body}"));
        let found = f.lint(&["SCR102", "REF012", "REF002"]);
        let in_last = |code: &str| {
            let path = format!("script-python/{last}/code.py");
            found
                .iter()
                .filter(|d| {
                    d.rule.code() == code
                        && d.file
                            .path()
                            .to_string_lossy()
                            .replace('\\', "/")
                            .ends_with(&path)
                })
                .collect::<Vec<_>>()
        };
        let missing = found.iter().any(|d| d.rule.code() == "REF002");
        let json = in_last("SCR102");
        let safe_json = json.iter().any(|d| {
            d.fix
                .as_ref()
                .is_some_and(|f| f.applicability == Applicability::Safe)
        });
        let ok = match expect {
            "uses" if certain => json.is_empty() && !missing,
            "uses" => !safe_json && !missing,
            "calls" => in_last("REF012").is_empty() && !missing,
            _ if certain => missing,
            // The name may exist.
            _ => !missing,
        };
        if !ok {
            let summary: Vec<String> = found
                .iter()
                .map(|d| format!("{} {}", d.rule.code(), d.message))
                .collect();
            failures.push(format!("case {case}: {modules:?} {body}: {summary:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
