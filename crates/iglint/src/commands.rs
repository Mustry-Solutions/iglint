//! `iglint rule`, `iglint rules` and `iglint migrate`.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iglint_config::compat;
use iglint_config::{LoadOptions, absolutize};
use iglint_linter::baseline::{Baseline, BaselineOrigin, DEFAULT_BASELINE_FILE};
use iglint_linter::engine::{self, CheckOptions};
use iglint_linter::registry::{Category, FixAvailability, OptionType, RuleCode};

use crate::args::{ListFormat, MigrateArgs, RuleArgs, RulesArgs};
use crate::output::display_path;
use crate::{Environment, ExitStatus, Io};

pub(crate) fn fix_text(fix: FixAvailability) -> &'static str {
    match fix {
        FixAvailability::None => "no",
        FixAvailability::Sometimes => "sometimes",
        FixAvailability::Always => "always",
    }
}

pub(crate) fn option_type(ty: OptionType) -> &'static str {
    match ty {
        OptionType::Bool => "boolean",
        OptionType::Integer => "integer",
        OptionType::Float => "number",
        OptionType::String => "string",
        OptionType::StringList => "array of strings",
        OptionType::Table => "table",
    }
}

/// The Markdown documentation of a rule.
pub fn rule_markdown(rule: RuleCode) -> String {
    let meta = rule.meta();
    let mut out = format!("# {} ({})\n\n", meta.name, meta.code);
    if !meta.summary.is_empty() {
        out.push_str(&format!("{}\n\n", meta.summary));
    }
    out.push_str(&format!(
        "- Category: {} (`{}`)\n- Default severity: {}\n- Enabled by default: {}\n- Fix: {}\n",
        meta.category.title(),
        meta.category.prefix(),
        meta.default_severity,
        if meta.default_enabled { "yes" } else { "no" },
        fix_text(meta.fix)
    ));
    if !meta.ign_lint_names.is_empty() {
        out.push_str(&format!("- ign-lint: {}\n", meta.ign_lint_names.join(", ")));
    }
    if !meta.explanation.is_empty() {
        out.push('\n');
        out.push_str(meta.explanation.trim_end());
        out.push('\n');
    }
    if !meta.options.is_empty() {
        out.push_str(&format!(
            "\n## Options\n\nSet in `[rules.{}]`.\n\n| Option | Type | Default | Description |\n|---|---|---|---|\n",
            meta.code
        ));
        for option in meta.options {
            out.push_str(&format!(
                "| `{}` | {} | `{}` | {} |\n",
                option.name,
                option_type(option.ty),
                option.default,
                option.description
            ));
        }
        if !meta.ign_lint_option_map.is_empty() {
            out.push_str("\nign-lint kwargs: ");
            let pairs: Vec<String> = meta
                .ign_lint_option_map
                .iter()
                .map(|(from, to)| format!("`{from}` -> `{to}`"))
                .collect();
            out.push_str(&pairs.join(", "));
            out.push('\n');
        }
    }
    out
}

fn rule_json(rule: RuleCode) -> serde_json::Value {
    let meta = rule.meta();
    serde_json::json!({
        "code": meta.code,
        "name": meta.name,
        "category": meta.category.prefix(),
        "summary": meta.summary,
        "default_severity": meta.default_severity.as_str(),
        "default_enabled": meta.default_enabled,
        "fix": fix_text(meta.fix),
        "options": meta.options.iter().map(|o| serde_json::json!({
            "name": o.name,
            "type": option_type(o.ty),
            "default": o.default,
            "description": o.description,
        })).collect::<Vec<_>>(),
        "ign_lint_names": meta.ign_lint_names,
        "explanation": meta.explanation,
    })
}

pub fn rule(args: &RuleArgs, environment: &Environment, io: &mut Io<'_>) -> ExitStatus {
    let registry = environment.registry;
    let rules: Vec<RuleCode> = if args.all {
        registry.iter().collect()
    } else {
        let key = args.rule.as_deref().unwrap_or_default();
        match registry.find(key) {
            Some(rule) => vec![rule],
            None => {
                let mut message = format!("error: unknown rule `{key}`");
                if let Some(suggestion) = registry.suggest(key) {
                    message.push_str(&format!("; did you mean `{suggestion}`?"));
                }
                let _ = writeln!(io.stderr, "{message}");
                return ExitStatus::Error;
            }
        }
    };
    let result = match args.output_format {
        ListFormat::Text => {
            let text: Vec<String> = rules.iter().map(|r| rule_markdown(*r)).collect();
            io.stdout.write_all(text.join("\n").as_bytes())
        }
        ListFormat::Json => {
            let value = if args.all {
                serde_json::Value::Array(rules.iter().map(|r| rule_json(*r)).collect())
            } else {
                rule_json(rules[0])
            };
            writeln!(
                io.stdout,
                "{}",
                serde_json::to_string_pretty(&value).unwrap_or_default()
            )
        }
    };
    match result {
        Ok(()) => ExitStatus::Success,
        Err(_) => ExitStatus::Error,
    }
}

pub fn rules(args: &RulesArgs, environment: &Environment, io: &mut Io<'_>) -> ExitStatus {
    let category = match &args.category {
        None => None,
        Some(key) => {
            let found = Category::from_prefix(key).or_else(|| {
                Category::ALL.into_iter().find(|c| {
                    c.title().eq_ignore_ascii_case(key)
                        || format!("{c:?}").eq_ignore_ascii_case(key)
                })
            });
            match found {
                Some(category) => Some(category),
                None => {
                    let names: Vec<_> = Category::ALL.iter().map(|c| c.prefix()).collect();
                    let _ = writeln!(
                        io.stderr,
                        "error: unknown category `{key}`; expected one of {}",
                        names.join(", ")
                    );
                    return ExitStatus::Error;
                }
            }
        }
    };
    let rules: Vec<RuleCode> = environment
        .registry
        .iter()
        .filter(|r| category.is_none_or(|c| r.meta().category == c))
        .collect();
    let result = match args.format {
        ListFormat::Json => writeln!(
            io.stdout,
            "{}",
            serde_json::to_string_pretty(&rules.iter().map(|r| rule_json(*r)).collect::<Vec<_>>())
                .unwrap_or_default()
        ),
        ListFormat::Text => write_rule_table(&rules, io.stdout),
    };
    match result {
        Ok(()) => ExitStatus::Success,
        Err(_) => ExitStatus::Error,
    }
}

fn write_rule_table(rules: &[RuleCode], out: &mut dyn Write) -> io::Result<()> {
    if rules.is_empty() {
        return writeln!(out, "No rules.");
    }
    let code_width = rules
        .iter()
        .map(|r| r.code().len())
        .max()
        .unwrap_or(4)
        .max(4);
    let name_width = rules
        .iter()
        .map(|r| r.name().len())
        .max()
        .unwrap_or(4)
        .max(4);
    writeln!(
        out,
        "{:code_width$}  {:name_width$}  {:8}  {:3}  {:7}  SUMMARY",
        "CODE", "NAME", "SEVERITY", "FIX", "DEFAULT"
    )?;
    for rule in rules {
        let meta = rule.meta();
        writeln!(
            out,
            "{:code_width$}  {:name_width$}  {:8}  {:3}  {:7}  {}",
            meta.code,
            meta.name,
            meta.default_severity.as_str(),
            if meta.fix == FixAvailability::None {
                ""
            } else {
                "[*]"
            },
            if meta.default_enabled { "yes" } else { "no" },
            meta.summary
        )?;
    }
    Ok(())
}

fn find_whitelist(args: &MigrateArgs, cwd: &Path, config_dir: &Path) -> Option<PathBuf> {
    if let Some(path) = &args.whitelist {
        return Some(absolutize(cwd, path));
    }
    compat::WHITELIST_FILES
        .iter()
        .map(|name| config_dir.join(name))
        .find(|p| p.is_file())
}

pub fn migrate(args: &MigrateArgs, environment: &Environment, io: &mut Io<'_>) -> ExitStatus {
    let cwd = &environment.cwd;
    let shown = |p: &Path| display_path(p, cwd);
    let source = absolutize(cwd, &args.config);
    let config = match compat::read_rule_config(&source) {
        Ok(config) => config,
        Err(error) => {
            let _ = writeln!(io.stderr, "error: {error}");
            return ExitStatus::Error;
        }
    };
    let config_dir = source
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cwd.clone());
    let output = args
        .output
        .as_ref()
        .map(|p| absolutize(cwd, p))
        .unwrap_or_else(|| config_dir.join("iglint.toml"));
    let output_dir = output
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cwd.clone());
    let baseline_path = args
        .baseline
        .as_ref()
        .map(|p| absolutize(cwd, p))
        .unwrap_or_else(|| output_dir.join(DEFAULT_BASELINE_FILE));

    let migration = iglint_config::migrate::migrate(&config, &source, environment.registry);
    let mut toml = migration.toml.clone();
    let whitelist_path = find_whitelist(args, cwd, &config_dir);
    let whitelist = match &whitelist_path {
        Some(path) => match compat::read_whitelist(path, &config_dir) {
            Ok(list) => list,
            Err(error) => {
                let _ = writeln!(io.stderr, "error: cannot read {}: {error}", shown(path));
                return ExitStatus::Error;
            }
        },
        None => Vec::new(),
    };
    let default_baseline = output_dir.join(DEFAULT_BASELINE_FILE);
    if !whitelist.is_empty() && baseline_path != default_baseline {
        let relative = baseline_path
            .strip_prefix(&output_dir)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| baseline_path.to_string_lossy().into_owned());
        let insert_at = toml.find('\n').map_or(toml.len(), |i| i + 1);
        toml.insert_str(
            insert_at,
            &format!("baseline = {}\n", toml::Value::String(relative)),
        );
    }

    if args.dry_run {
        let _ = io.stdout.write_all(toml.as_bytes());
        for line in &migration.summary {
            let _ = writeln!(io.stderr, "  {line}");
        }
        if !whitelist.is_empty() {
            let _ = writeln!(
                io.stderr,
                "  {} whitelisted file(s) would be linted, with their findings written to {}",
                whitelist.len(),
                shown(&baseline_path)
            );
        }
        return ExitStatus::Success;
    }
    for path in std::iter::once(&output).chain((!whitelist.is_empty()).then_some(&baseline_path)) {
        if path.exists() && !args.force {
            let _ = writeln!(
                io.stderr,
                "error: {} already exists; use --force to overwrite it",
                shown(path)
            );
            return ExitStatus::Error;
        }
    }
    if let Err(error) = std::fs::write(&output, &toml) {
        let _ = writeln!(io.stderr, "error: cannot write {}: {error}", shown(&output));
        return ExitStatus::Error;
    }
    let _ = writeln!(
        io.stdout,
        "Wrote {} from {}.",
        shown(&output),
        shown(&source)
    );
    for line in &migration.summary {
        let _ = writeln!(io.stdout, "  {line}");
    }

    if !whitelist.is_empty() {
        // The whitelisted files are linted from now on; their current
        // findings go into the baseline so that only new ones fail.
        let options = LoadOptions {
            cwd: cwd.clone(),
            config_file: Some(output.clone()),
            ..LoadOptions::default()
        };
        let configuration = match iglint_config::load(&options, environment.registry) {
            Ok(configuration) => configuration,
            Err(error) => {
                let _ = writeln!(
                    io.stderr,
                    "error: the migrated configuration is invalid: {error}"
                );
                return ExitStatus::Error;
            }
        };
        let files: Vec<PathBuf> = whitelist.iter().filter(|p| p.is_file()).cloned().collect();
        let missing = whitelist.len() - files.len();
        let mut check_options = CheckOptions::default();
        if let Some(analyzer) = &environment.analyzer {
            check_options.analyzer = Some(Arc::clone(analyzer));
        }
        let result = engine::check(&configuration.settings, &files, &check_options);
        let baseline = Baseline::from_diagnostics(
            &BaselineOrigin::new(&configuration.settings, &files),
            &result.diagnostics,
        );
        if let Err(error) = baseline.save(&baseline_path) {
            let _ = writeln!(
                io.stderr,
                "error: cannot write {}: {error}",
                shown(&baseline_path)
            );
            return ExitStatus::Error;
        }
        let _ = writeln!(
            io.stdout,
            "Wrote {} with {} finding(s) in the {} file(s) of the whitelist {}.",
            shown(&baseline_path),
            baseline.len(),
            files.len(),
            whitelist_path.as_deref().map(shown).unwrap_or_default()
        );
        if missing > 0 {
            let _ = writeln!(
                io.stdout,
                "  {missing} whitelisted file(s) no longer exist and were dropped."
            );
        }
        let _ = writeln!(
            io.stdout,
            "The whitelist is no longer needed: whitelisted files are linted, and only new findings fail."
        );
    }
    ExitStatus::Success
}
