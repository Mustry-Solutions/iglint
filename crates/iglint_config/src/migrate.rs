//! `iglint migrate`: converting an ign-lint `rule_config.json` to
//! `iglint.toml`.
//!
//! The generated configuration reproduces what ign-lint ran: the iglint
//! equivalents of every loaded ign-lint rule are selected (ign-lint runs all
//! of its rules unless disabled), kwargs become rule options, severities that
//! differ from iglint's defaults are written out and `_comment` entries
//! become TOML comments. Whatever has no iglint equivalent is listed in the
//! summary and kept as a comment.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use iglint_linter::registry::{Registry, RuleCode};
use iglint_linter::settings::OptionValue;

use crate::compat::{self, CompatMode, RuleConfig};

/// The result of a migration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migration {
    /// The `iglint.toml` text.
    pub toml: String,
    /// One line per decision, for the user.
    pub summary: Vec<String>,
}

fn comment_lines(out: &mut String, text: &str) {
    for line in text.lines() {
        if line.trim().is_empty() {
            out.push_str("#\n");
        } else {
            let _ = writeln!(out, "# {line}");
        }
    }
}

/// TOML text of an option value.
pub fn toml_value(value: &OptionValue) -> String {
    fn convert(value: &OptionValue) -> toml::Value {
        match value {
            OptionValue::Bool(b) => toml::Value::Boolean(*b),
            OptionValue::Integer(i) => toml::Value::Integer(*i),
            OptionValue::Float(f) => toml::Value::Float(*f),
            OptionValue::String(s) => toml::Value::String(s.clone()),
            OptionValue::List(items) => toml::Value::Array(items.iter().map(convert).collect()),
            OptionValue::Table(table) => {
                toml::Value::Table(table.iter().map(|(k, v)| (k.clone(), convert(v))).collect())
            }
        }
    }
    convert(value).to_string()
}

/// TOML text of a key, quoted when needed.
fn toml_key(key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if bare {
        key.to_owned()
    } else {
        toml::Value::String(key.to_owned()).to_string()
    }
}

/// Converts `config` (read from `source`) into `iglint.toml` text.
pub fn migrate(config: &RuleConfig, source: &Path, registry: &Registry) -> Migration {
    let root = source.parent().unwrap_or(Path::new(""));
    let resolved = compat::to_settings(config, registry, root, CompatMode::IgnLintOnly);
    let mut summary = Vec::new();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# iglint configuration, migrated from {} by `iglint migrate`.",
        source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| compat::RULE_CONFIG_FILE.to_owned())
    );
    if !config.comments.is_empty() {
        out.push_str("#\n");
        for comment in &config.comments {
            comment_lines(&mut out, comment);
        }
    }

    // Rules ign-lint ran that are not in iglint's default set.
    let mut extend_select: Vec<RuleCode> = Vec::new();
    for codes in resolved.rule_map.values() {
        for rule in codes {
            if !rule.meta().default_enabled && !extend_select.contains(rule) {
                extend_select.push(*rule);
            }
        }
    }
    extend_select.sort();
    if !extend_select.is_empty() {
        out.push('\n');
        out.push_str("# ign-lint runs every rule that is not disabled.\n");
        let _ = writeln!(
            out,
            "extend-select = [{}]",
            extend_select
                .iter()
                .map(|r| format!("\"{}\"", r.code()))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let unfixable: Vec<RuleCode> = resolved.settings.unfixable.iter().copied().collect();
    if !unfixable.is_empty() {
        out.push_str("# allow_fix: false in ign-lint.\n");
        let _ = writeln!(
            out,
            "unfixable = [{}]",
            unfixable
                .iter()
                .map(|r| format!("\"{}\"", r.code()))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let mut written: BTreeSet<RuleCode> = BTreeSet::new();
    for entry in &config.rules {
        let Some(info) = entry.ign_lint else {
            out.push('\n');
            let _ = writeln!(
                out,
                "# {}: unknown to ign-lint 0.6.4; not migrated.",
                entry.name
            );
            summary.push(format!("{}: unknown rule, not migrated", entry.name));
            continue;
        };
        let equivalents = compat::equivalents(info, registry);
        let targets: Vec<RuleCode> = equivalents.iter().map(|e| e.rule).collect();
        if targets.is_empty() {
            if entry.configured {
                out.push('\n');
                let _ = writeln!(
                    out,
                    "# {}: iglint has no equivalent rule yet; not migrated.",
                    entry.name
                );
                for comment in &entry.comments {
                    comment_lines(&mut out, comment);
                }
                for (kwarg, value) in &entry.kwargs {
                    let _ = writeln!(out, "# {kwarg} = {value}");
                }
            }
            summary.push(format!(
                "{}: no iglint equivalent, not migrated",
                entry.name
            ));
            continue;
        }
        let names = targets
            .iter()
            .map(|r| r.code())
            .collect::<Vec<_>>()
            .join(", ");
        if let Some(error) = &entry.error {
            out.push('\n');
            let _ = writeln!(
                out,
                "# {}: ign-lint could not create this rule ({error}); it did not run.",
                entry.name
            );
            summary.push(format!(
                "{}: invalid in ign-lint, disabled ({names})",
                entry.name
            ));
        } else if !entry.enabled {
            summary.push(format!("{}: disabled ({names})", entry.name));
        } else {
            summary.push(format!("{} -> {names}", entry.name));
        }
        for equivalent in &equivalents {
            let rule = &equivalent.rule;
            if !written.insert(*rule) {
                continue;
            }
            let meta = rule.meta();
            let mut body = String::new();
            match resolved.settings.base.rules.get(rule) {
                None => {
                    if meta.default_enabled {
                        body.push_str("enabled = false\n");
                    }
                }
                Some(settings) => {
                    if settings.severity != meta.default_severity {
                        let _ = writeln!(body, "severity = \"{}\"", settings.severity);
                    }
                    for (option, value) in &settings.options {
                        let _ = writeln!(body, "{} = {}", toml_key(option), toml_value(value));
                    }
                }
            }
            // Rules of the ign-lint rule's category take no kwargs and keep
            // their defaults; only a disabled rule needs a section.
            if !equivalent.direct {
                if !body.is_empty() {
                    out.push('\n');
                    let _ = writeln!(out, "[rules.{}]", meta.code);
                    let _ = writeln!(
                        out,
                        "# {}: {}, from ign-lint {}.",
                        meta.name, meta.summary, entry.name
                    );
                    out.push_str(&body);
                }
                continue;
            }
            for (kwarg, value) in &entry.kwargs {
                let mapped = meta.option_for_ign_lint_kwarg(kwarg).is_some() || kwarg == "severity";
                if !mapped {
                    let _ = writeln!(
                        body,
                        "# ign-lint kwarg without iglint equivalent: {kwarg} = {value}"
                    );
                }
            }
            if body.is_empty() && entry.comments.is_empty() {
                continue;
            }
            out.push('\n');
            let _ = writeln!(out, "[rules.{}]", meta.code);
            if meta.summary.is_empty() {
                let _ = writeln!(out, "# {}, from ign-lint {}.", meta.name, entry.name);
            } else {
                let _ = writeln!(
                    out,
                    "# {}: {}, from ign-lint {}.",
                    meta.name, meta.summary, entry.name
                );
            }
            for comment in &entry.comments {
                comment_lines(&mut out, comment);
            }
            out.push_str(&body);
        }
    }
    for notice in &resolved.notices {
        summary.push(notice.clone());
    }
    Migration { toml: out, summary }
}
