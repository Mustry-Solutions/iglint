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

use iglint_project::library::{
    HARMLESS_ATTRIBUTES, MODULE_OBJECT_MEMBERS, OBJECT_MEMBERS, PACKAGE_OBJECT_MEMBERS,
    is_special_attribute, is_value_attribute,
};

use crate::diagnostic::Applicability;
use crate::rules::script::testing::{library, with_library};

const CASES: &str = include_str!("../../../tests/fixtures/library-routes/routes.cases");
const RUN_SCRIPT: &str = include_str!("../../../tests/fixtures/library-routes/runscript.cases");
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
