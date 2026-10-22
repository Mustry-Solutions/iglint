//! Parity with ign-lint 0.6.4 on the fixtures in
//! `crates/iglint_linter/tests/fixtures/ign-lint/`.
//!
//! For every view and every variant of `variants.json`, the configuration is
//! loaded exactly as `iglint ign-lint` loads a `rule_config.json`, the view
//! is linted by the real engine, and each finding is rendered the way
//! `iglint ign-lint` prints it (`{ign-lint node path}: {ign-lint message}`).
//! Every expected ign-lint finding must be produced with the same severity,
//! and nothing else may be produced, except for the intended differences
//! listed in `differences/*.json` next to the fixtures (each one documented in
//! `docs/compat/ign-lint-differences.md`). Unused entries of that list fail
//! the test too, so it stays exact.
//!
//! PylintScriptRule is not compared: its iglint equivalents (`SCR900`, which
//! runs the real pylint per script, and the native `SCR` rules) report on
//! each script as Ignition compiles it, so lines, wrappers and the set of
//! messages differ from ign-lint's combined file by design (bugs 11 to 14,
//! see `docs/compat/ign-lint-differences.md`), and pylint is an external
//! program the test cannot assume. Their findings are left out on both
//! sides and `SCR900` is not run. The script part of
//! ComponentReferenceValidationRule is `SCR305`.
//!
//! `PARITY_FILTER=<substring>` limits the run to views, variants or rules
//! containing the substring and prints every mismatch.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use iglint_config::compat::{CompatMode, parse_rule_config, to_settings};
use iglint_linter::engine::{CheckOptions, check};
use iglint_linter::rules::REGISTRY;
use iglint_linter::{Applicability, Severity};
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../iglint_linter/tests/fixtures/ign-lint")
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Finding {
    rule: String,
    severity: String,
    message: String,
    /// `Some(safe)` when a fix is offered.
    fix: Option<bool>,
}

#[derive(Debug)]
struct Difference {
    view: String,
    variant: String,
    rule: String,
    message: String,
    /// `missing` (ign-lint reports it, iglint does not), `extra` (iglint
    /// reports it, ign-lint does not), `severity`, or `fix`.
    kind: String,
    used: std::cell::Cell<bool>,
}

/// Reads every `differences/*.json` file: `{"differences": [{"view",
/// "variant", "rule", "message", "kind", "reason"}]}`, `*` matching
/// anything.
fn load_differences() -> Vec<Difference> {
    let mut entries = Vec::new();
    let Ok(dir) = std::fs::read_dir(fixtures().join("differences")) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = dir.map(|e| e.unwrap().path()).collect();
    files.sort();
    for file in files {
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        entries.extend(
            value["differences"]
                .as_array()
                .expect("differences array")
                .clone(),
        );
    }
    entries
        .iter()
        .map(|d| {
            let s = |k: &str| d[k].as_str().unwrap_or("*").to_owned();
            assert!(
                d["reason"].as_str().is_some_and(|r| !r.is_empty()),
                "difference without reason: {d}"
            );
            Difference {
                view: s("view"),
                variant: s("variant"),
                rule: s("rule"),
                message: s("message"),
                kind: s("kind"),
                used: std::cell::Cell::new(false),
            }
        })
        .collect()
}

fn matches(p: &str, v: &str) -> bool {
    p == "*" || p == v
}

/// Whether iglint is expected to reproduce this ign-lint finding with the
/// rules registered now.
fn in_scope(rule: &str, message: &str) -> bool {
    match rule {
        "PylintScriptRule" => false,
        "ComponentReferenceValidationRule" => {
            let body = message.split_once(": ").map_or(message, |(_, m)| m);
            let script = body.starts_with("Script ")
                || body.starts_with("Component '")
                || body.starts_with("Navigation chain");
            !script || REGISTRY.find("SCR305").is_some()
        }
        _ => true,
    }
}

fn expected_findings(entry: &Value, fixes: Option<&Value>) -> Vec<Finding> {
    let mut safe_by_message: BTreeMap<String, bool> = BTreeMap::new();
    if let Some(fixes) = fixes.and_then(Value::as_array) {
        for f in fixes {
            if let (Some(m), Some(s)) = (f["violation_message"].as_str(), f["is_safe"].as_bool()) {
                safe_by_message.insert(m.to_owned(), s);
            }
        }
    }
    entry
        .as_array()
        .expect("findings")
        .iter()
        .filter(|f| in_scope(f["rule"].as_str().unwrap(), f["message"].as_str().unwrap()))
        .map(|f| {
            let message = f["message"].as_str().unwrap().to_owned();
            Finding {
                rule: f["rule"].as_str().unwrap().to_owned(),
                severity: f["severity"].as_str().unwrap().to_owned(),
                fix: safe_by_message.get(&message).copied(),
                message,
            }
        })
        .collect()
}

fn actual_findings(view: &Path, config: &Value, variant: &str) -> Vec<Finding> {
    // Like the generator: every variant but `default` disables the rules it
    // does not configure.
    let mut config = config.clone();
    if variant != "default" {
        let object = config.as_object_mut().unwrap();
        for rule in iglint_config::compat::IGN_LINT_RULES {
            object
                .entry(rule.name)
                .or_insert_with(|| serde_json::json!({"enabled": false}));
        }
    }
    let text = serde_json::to_string(&config).unwrap();
    let rule_config =
        parse_rule_config(Path::new("rule_config.json"), &text).expect("valid rule config");
    let dir = view.parent().unwrap();
    let mut compat = to_settings(&rule_config, &REGISTRY, dir, CompatMode::IgnLintOnly);
    let mut rule_names: BTreeMap<&str, String> = BTreeMap::new();
    for (name, codes) in &compat.rule_map {
        if name == "PylintScriptRule" {
            continue;
        }
        for code in codes {
            rule_names.insert(code.code(), name.clone());
        }
    }
    // PylintScriptRule is not compared (see the module documentation).
    let mut base = (*compat.settings.base).clone();
    base.rules.retain(|rule, _| rule.code() != "SCR900");
    compat.settings.base = std::sync::Arc::new(base);
    let options = CheckOptions::default();
    let result = check(&compat.settings, &[view.to_path_buf()], &options);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    result
        .diagnostics
        .iter()
        .filter_map(|d| {
            let rule = rule_names.get(d.rule.code())?.clone();
            let path = d.ign_lint_path.as_deref().or(d.node_path.as_deref());
            let message = d.ign_lint_message.as_deref().unwrap_or(&d.message);
            Some(Finding {
                rule,
                severity: match d.severity {
                    Severity::Error => "error",
                    _ => "warning",
                }
                .to_owned(),
                message: match path {
                    Some(p) => format!("{p}: {message}"),
                    None => message.to_owned(),
                },
                fix: d.fix.as_ref().and_then(|f| match f.applicability {
                    Applicability::Safe => Some(true),
                    Applicability::Unsafe => Some(false),
                    Applicability::DisplayOnly => None,
                }),
            })
        })
        .collect()
}

#[test]
fn ign_lint_parity() {
    let root = fixtures();
    let variants: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("variants.json")).unwrap())
            .unwrap();
    let excluded = &variants["_exclude"];
    let filter = std::env::var("PARITY_FILTER").ok();
    let differences = load_differences();
    let mut views: Vec<PathBuf> = Vec::new();
    for group in ["cases", "probes"] {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(root.join(group))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        dirs.sort();
        views.extend(dirs);
    }
    let mut problems = Vec::new();
    let mut compared = 0;
    for dir in &views {
        let name = format!(
            "{}/{}",
            dir.parent().unwrap().file_name().unwrap().to_string_lossy(),
            dir.file_name().unwrap().to_string_lossy()
        );
        let expected_variants: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("expected-variants.json")).unwrap(),
        )
        .unwrap();
        for (variant, config) in variants.as_object().unwrap() {
            if variant.starts_with('_') {
                continue;
            }
            if excluded[variant]
                .as_array()
                .is_some_and(|l| l.iter().any(|v| v.as_str() == Some(&name)))
            {
                continue;
            }
            let Some(entry) = expected_variants.get(variant) else {
                continue;
            };
            let mut expected = expected_findings(&entry["findings"], entry.get("fixes"));
            let mut actual = actual_findings(&dir.join("view.json"), config, variant);
            expected.sort();
            actual.sort();
            compared += expected.len();
            let selected = |rule: &str| {
                filter
                    .as_deref()
                    .is_none_or(|f| name.contains(f) || variant.contains(f) || rule.contains(f))
            };
            let allowed = |rule: &str, message: &str, kind: &str| {
                differences.iter().any(|d| {
                    let hit = matches(&d.view, &name)
                        && matches(&d.variant, variant)
                        && matches(&d.rule, rule)
                        && matches(&d.message, message)
                        && d.kind == kind;
                    if hit {
                        d.used.set(true);
                    }
                    hit
                })
            };
            // Multiset difference by message.
            let mut unmatched_actual: Vec<Option<&Finding>> = actual.iter().map(Some).collect();
            for e in &expected {
                let found = unmatched_actual
                    .iter_mut()
                    .find(|a| a.is_some_and(|a| a.rule == e.rule && a.message == e.message));
                match found {
                    Some(slot) => {
                        let a = slot.take().unwrap();
                        if a.severity != e.severity
                            && !allowed(&e.rule, &e.message, "severity")
                            && selected(&e.rule)
                        {
                            problems.push(format!(
                                "{name} [{variant}] severity {} != {}: {}",
                                a.severity, e.severity, e.message
                            ));
                        }
                        if e.fix.is_some()
                            && a.fix != e.fix
                            && !allowed(&e.rule, &e.message, "fix")
                            && selected(&e.rule)
                        {
                            problems.push(format!(
                                "{name} [{variant}] fix {:?} != {:?}: {}",
                                a.fix, e.fix, e.message
                            ));
                        }
                    }
                    None => {
                        if !allowed(&e.rule, &e.message, "missing") && selected(&e.rule) {
                            problems.push(format!(
                                "{name} [{variant}] missing {}: {}",
                                e.rule, e.message
                            ));
                        }
                    }
                }
            }
            for a in unmatched_actual.into_iter().flatten() {
                if !allowed(&a.rule, &a.message, "extra") && selected(&a.rule) {
                    problems.push(format!(
                        "{name} [{variant}] extra {}: {}",
                        a.rule, a.message
                    ));
                }
            }
        }
    }
    if filter.is_none() {
        for d in &differences {
            if !d.used.get() {
                problems.push(format!("unused difference entry: {d:?}"));
            }
        }
    }
    assert!(compared > 0);
    assert!(
        problems.is_empty(),
        "{} parity problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}
