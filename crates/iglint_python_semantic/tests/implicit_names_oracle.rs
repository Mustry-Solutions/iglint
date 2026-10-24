//! Differential test of the names statements create implicitly, against
//! Ignition's own script loading.
//!
//! `tests/oracle/implicit_names.cases` holds small sources;
//! `implicit_names.expected` records, per source and per context, the
//! dunder names of the namespace after the code ran (recorded with
//! `oracle/run-oracle.sh --implicit`, which loads each source as a project
//! library module through Ignition 8.3.3's `ScriptManager`, as the body of
//! a compiled event function, and as top-level code after one). The model
//! must agree:
//!
//! * a library module's globals hold `__builtins__` and `__name__`,
//!   `__doc__` only with a docstring, never `__file__`; an event script's
//!   hold `__builtins__` and none of `__name__`, `__doc__`, `__file__`
//!   unless the code binds them;
//! * `__package__` is in the namespace exactly when the model binds it
//!   there ([`BindingKind::ImportPackage`] or the code's own binding),
//!   except where the model over-approximates on purpose (an import in a
//!   function that never runs) or the code is dynamic or not vetted (an
//!   `exec` string, `__import__` with the caller's globals);
//! * no statement creates any other name (`+name` entries come only from
//!   `from m import *` and from the event function's own name).

use iglint_python_semantic::{BindingKind, ScopeId, SemanticModel, SemanticOptions};

fn cases(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut name: Option<String> = None;
    let mut body = String::new();
    for line in text.split_inclusive('\n') {
        if let Some(n) = line.strip_prefix("### ") {
            if let Some(prev) = name.take() {
                out.push((prev, std::mem::take(&mut body)));
            }
            name = Some(n.trim_end().to_owned());
        } else {
            body.push_str(line);
        }
    }
    if let Some(n) = name {
        out.push((n, body));
    }
    out
}

/// Per case: (context, entries) for each recorded context.
fn expected(text: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut out: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("### ") {
            out.push((name.to_owned(), Vec::new()));
        } else if let Some((context, entries)) = line.split_once(':') {
            out.last_mut()
                .unwrap()
                .1
                .push((context.to_owned(), entries.trim().to_owned()));
        }
    }
    out
}

/// The dunder names of an entry list (`__package__=None` gives
/// `__package__`).
fn dunders(entries: &str) -> Vec<&str> {
    entries
        .split(' ')
        .map(|e| e.split('=').next().unwrap())
        .filter(|e| e.starts_with("__"))
        .collect()
}

/// Whether the model may leave `__package__` in the module namespace.
fn binds_package(model: &SemanticModel<'_>) -> bool {
    model
        .scope(ScopeId::MODULE)
        .bindings_of("__package__")
        .iter()
        .any(|id| model.binding(*id).kind != BindingKind::Global)
}

fn model_of(source: &str, module_has_name: bool, f: impl FnOnce(&SemanticModel<'_>)) {
    let parsed = iglint_python::parse_module(source);
    assert!(parsed.errors.is_empty(), "{source}: {:?}", parsed.errors);
    let options = SemanticOptions {
        injected: vec!["system", "fpmi", "sys", "pkg", "runAction"],
        unknown_globals: false,
        module_has_name,
    };
    f(&SemanticModel::build(&parsed.module, &options));
}

/// Cases where the model binds `__package__` although the recorded run
/// did not: the import is in a function that never runs. (The model is
/// flow-insensitive across scopes, as for `global` assignments.)
const OVER_APPROXIMATED: &[&str] = &["import-in-function-not-called"];

/// Cases where an import the model does not see runs: an `exec` string
/// (the scope is dynamic) and `__import__` with the caller's globals (not
/// vetted code). Both keep every safe fix that depends on names away.
const DYNAMIC: &[&str] = &["import-in-unqualified-exec", "dunder-import-call-globals"];

#[test]
fn implicit_names_match_ignition() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/oracle");
    let sources = cases(&std::fs::read_to_string(format!("{dir}/implicit_names.cases")).unwrap());
    let expected =
        expected(&std::fs::read_to_string(format!("{dir}/implicit_names.expected")).unwrap());
    assert_eq!(sources.len(), expected.len());
    let mut checked = 0;
    for ((name, source), (expected_name, contexts)) in sources.iter().zip(&expected) {
        assert_eq!(name, expected_name);
        for (context, entries) in contexts {
            if entries == "compile-error" {
                assert!(
                    name.contains("future") && context != "library",
                    "{name} {context}"
                );
                continue;
            }
            let (text, module_has_name) = match context.as_str() {
                "library" => (source.clone(), true),
                "function" => {
                    let body: String = source
                        .split_inclusive('\n')
                        .map(|l| format!("    {l}"))
                        .collect();
                    (format!("def runAction(self, event):\n{body}"), false)
                }
                "toplevel" => (
                    format!("def runAction(self, event):\n    pass\n{source}"),
                    false,
                ),
                // Plain modules are recorded for comparison only.
                _ => continue,
            };
            if entries.contains('!') {
                // The code raised part-way, deliberately: `del __package__`
                // where no import bound it, or a read of the `__package__`
                // that a function-local `__name__` does not make.
                assert!(
                    name.starts_with("package-deleted")
                        || (name.starts_with("name-assigned") && context == "function"),
                    "{name} {context}"
                );
            }
            if entries.starts_with('!') {
                // No namespace to compare.
                continue;
            }
            let names = dunders(entries);
            let binds_name = source.contains("__name__ =");
            let docstring = source.starts_with("\"\"\"");
            // Ignition's own names.
            assert!(names.contains(&"__builtins__"), "{name} {context}");
            assert!(!names.contains(&"__file__"), "{name} {context}");
            let has_name = module_has_name || (binds_name && context == "toplevel");
            assert_eq!(names.contains(&"__name__"), has_name, "{name} {context}");
            assert_eq!(
                names.contains(&"__doc__"),
                docstring && module_has_name,
                "{name} {context}"
            );
            // No other statement creates a name.
            for extra in entries.split(' ').filter_map(|e| e.strip_prefix('+')) {
                assert!(
                    extra == "runAction" || name == "from-import-star",
                    "{name} {context}: {extra}"
                );
            }
            if names.contains(&"__warningregistry__") {
                // An import that warned: a relative import outside a
                // library module, or any import once the code set a
                // dotted `__name__` or a `__package__`.
                assert!(
                    name.starts_with("relative-")
                        || name.starts_with("name-assigned")
                        || name.starts_with("package-assigned"),
                    "{name} {context}"
                );
            }
            if entries.contains('!') {
                continue;
            }
            let observed = names.contains(&"__package__");
            model_of(&text, module_has_name, |model| {
                let modelled = binds_package(model);
                if OVER_APPROXIMATED.contains(&name.as_str()) && !observed {
                    assert_eq!(modelled, module_has_name, "{name} {context}");
                } else if DYNAMIC.contains(&name.as_str()) && observed {
                    assert!(
                        model.scope(ScopeId::MODULE).is_dynamic()
                            || !iglint_python_semantic::vetted::unknown_code(model).is_empty(),
                        "{name} {context}"
                    );
                } else {
                    assert_eq!(modelled, observed, "{name} {context}: {entries}");
                }
            });
            checked += 1;
        }
    }
    assert!(checked > 100, "{checked}");
}

/// The reviewer's repro (round 12, K1) on Ignition: the module returns
/// `None` with the import and `'ABSENT'` without it, and keeps `None` when
/// another import is left.
#[test]
fn removing_the_only_import_unbinds_package() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/oracle");
    let expected =
        expected(&std::fs::read_to_string(format!("{dir}/implicit_names.expected")).unwrap());
    let library = |case: &str| -> String {
        expected
            .iter()
            .find(|(n, _)| n == case)
            .and_then(|(_, contexts)| contexts.iter().find(|(c, _)| c == "library"))
            .map(|(_, e)| e.clone())
            .unwrap()
    };
    assert!(library("k1-repro-before").contains("r_value=None"));
    assert!(library("k1-repro-after-safe-fix").contains("r_value='ABSENT'"));
    assert!(library("k1-repro-other-import-kept-after").contains("r_value=None"));
}
