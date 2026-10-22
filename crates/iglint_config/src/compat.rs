//! Reading ign-lint configuration: `rule_config.json` and whitelist files.
//!
//! `rule_config.json` is read exactly as ign-lint 0.6.4 reads it (see
//! `docs/compat/ign-lint.md`, section 7). Rule class names map to iglint
//! rules through [`RuleMeta::ign_lint_names`] and kwargs to options through
//! [`RuleMeta::ign_lint_option_map`]. An ign-lint rule can also stand for a
//! whole category ([`IgnLintRule::category`]): `PylintScriptRule` maps to
//! `SCR900` (real pylint) and to every default rule of the `SCR` category,
//! iglint's native script checks, which take no kwargs and keep their own
//! severities ([`equivalents`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iglint_linter::Severity;
use iglint_linter::registry::{Category, OptionType, Registry, RuleCode, RuleMeta};
use iglint_linter::settings::{
    LinterSettings, OptionValue, Override, ProjectSettings, RuleSettings,
};
use serde_json::Value;

/// Default name of the ign-lint configuration file.
pub const RULE_CONFIG_FILE: &str = "rule_config.json";

/// Whitelist file names picked up next to a detected `rule_config.json`.
pub const WHITELIST_FILES: [&str; 2] = [".ignition-lint-whitelist.txt", ".whitelist.txt"];

/// What ign-lint 0.6.4 knows about one of its rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IgnLintRule {
    pub name: &'static str,
    /// Other names accepted for the rule (renames on ign-lint's `dev`).
    pub aliases: &'static [&'static str],
    pub default_severity: Severity,
    pub supports_fix: bool,
    /// Accepted kwargs, without `severity`.
    pub kwargs: &'static [&'static str],
    /// Accepts any kwarg (`**kwargs`), silently ignoring unknown ones.
    pub accepts_any_kwarg: bool,
    /// `severity` values other than `warning`/`error` are a creation error
    /// instead of silently meaning `error`.
    pub strict_severity: bool,
    /// The rule also stands for every default rule of this iglint category
    /// (besides the rules naming it in [`RuleMeta::ign_lint_names`]).
    pub category: Option<Category>,
}

/// The rules registered in ign-lint 0.6.4, in the order the spec lists them.
pub const IGN_LINT_RULES: &[IgnLintRule] = &[
    IgnLintRule {
        name: "NamePatternRule",
        aliases: &[],
        default_severity: Severity::Warning,
        supports_fix: true,
        kwargs: &[
            "convention",
            "target_node_types",
            "custom_pattern",
            "suggestion_convention",
            "node_type_specific_rules",
            "allow_numbers",
            "min_length",
            "max_length",
            "forbidden_names",
            "skip_names",
            "allowed_abbreviations",
            "auto_detect_abbreviations",
        ],
        accepts_any_kwarg: true,
        strict_severity: true,
        category: None,
    },
    IgnLintRule {
        name: "PollingIntervalRule",
        aliases: &[],
        default_severity: Severity::Error,
        supports_fix: false,
        kwargs: &["minimum_interval"],
        accepts_any_kwarg: false,
        strict_severity: false,
        category: None,
    },
    IgnLintRule {
        name: "PylintScriptRule",
        aliases: &["PerspectiveScriptPylintRule"],
        default_severity: Severity::Error,
        supports_fix: true,
        kwargs: &[
            "pylintrc",
            "debug",
            "batch_mode",
            "debug_dir",
            "category_mapping",
        ],
        accepts_any_kwarg: false,
        strict_severity: false,
        category: Some(Category::Script),
    },
    IgnLintRule {
        name: "BadComponentReferenceRule",
        aliases: &[],
        default_severity: Severity::Error,
        supports_fix: false,
        kwargs: &["forbidden_patterns", "case_sensitive"],
        accepts_any_kwarg: false,
        strict_severity: false,
        category: None,
    },
    IgnLintRule {
        name: "ComponentReferenceValidationRule",
        aliases: &[],
        default_severity: Severity::Error,
        supports_fix: false,
        kwargs: &[
            "validate_expressions",
            "validate_property_bindings",
            "validate_scripts",
        ],
        accepts_any_kwarg: false,
        strict_severity: false,
        category: None,
    },
    IgnLintRule {
        name: "UnusedCustomPropertiesRule",
        aliases: &[],
        default_severity: Severity::Error,
        supports_fix: true,
        kwargs: &[],
        accepts_any_kwarg: false,
        strict_severity: false,
        category: None,
    },
    IgnLintRule {
        name: "ExcessiveContextDataRule",
        aliases: &[],
        default_severity: Severity::Error,
        supports_fix: false,
        kwargs: &[
            "max_array_size",
            "max_sibling_properties",
            "max_nesting_depth",
            "max_data_points",
            "max_value_length",
        ],
        accepts_any_kwarg: false,
        strict_severity: false,
        category: None,
    },
    IgnLintRule {
        name: "PropertyAccessRule",
        aliases: &[],
        default_severity: Severity::Error,
        supports_fix: true,
        kwargs: &["expected_access", "exempt_props"],
        accepts_any_kwarg: false,
        strict_severity: false,
        category: None,
    },
    IgnLintRule {
        name: "PropertyPersistenceRule",
        aliases: &[],
        default_severity: Severity::Error,
        supports_fix: true,
        kwargs: &["expected_persistent", "exempt_binding_types"],
        accepts_any_kwarg: false,
        strict_severity: false,
        category: None,
    },
];

/// Finds an ign-lint rule by class name or alias (case-sensitive, like
/// ign-lint).
pub fn ign_lint_rule(name: &str) -> Option<&'static IgnLintRule> {
    IGN_LINT_RULES
        .iter()
        .find(|r| r.name == name || r.aliases.contains(&name))
}

/// One iglint rule an ign-lint rule maps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Equivalent {
    pub rule: RuleCode,
    /// Named by the rule's [`RuleMeta::ign_lint_names`]: it takes the
    /// ign-lint rule's kwargs and severity. `false` for the rules of
    /// [`IgnLintRule::category`], which keep their own defaults.
    pub direct: bool,
}

/// The iglint rules that implement `info`, in code order: the rules naming
/// it or one of its aliases, and the default rules of its category that no
/// ign-lint rule names.
pub fn equivalents(info: &IgnLintRule, registry: &Registry) -> Vec<Equivalent> {
    let mut out: Vec<Equivalent> = Vec::new();
    for name in std::iter::once(&info.name).chain(info.aliases) {
        for rule in registry.find_ign_lint(name) {
            if !out.iter().any(|e| e.rule == rule) {
                out.push(Equivalent { rule, direct: true });
            }
        }
    }
    if let Some(category) = info.category {
        for rule in registry.iter() {
            let meta = rule.meta();
            // A rule another ign-lint rule names belongs to that rule.
            if meta.category == category
                && meta.default_enabled
                && meta.ign_lint_names.is_empty()
                && !out.iter().any(|e| e.rule == rule)
            {
                out.push(Equivalent {
                    rule,
                    direct: false,
                });
            }
        }
    }
    out.sort_by_key(|e| e.rule);
    out
}

/// A `rule_config.json` that could not be used at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleConfigError {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for RuleConfigError {
    /// ign-lint's wording: `Error loading config file {path}: {error}`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Error loading config file {}: {}",
            self.path.display(),
            self.message
        )
    }
}

impl std::error::Error for RuleConfigError {}

/// One rule entry of `rule_config.json`, after ign-lint's checks.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleEntry {
    /// The key as written in the file.
    pub name: String,
    /// The ign-lint rule it names, if ign-lint knows it.
    pub ign_lint: Option<&'static IgnLintRule>,
    /// Present in the file (as opposed to running with defaults).
    pub configured: bool,
    pub enabled: bool,
    pub allow_fix: Option<bool>,
    /// kwargs without `_`-prefixed keys, in file order.
    pub kwargs: Vec<(String, Value)>,
    /// `_comment`-style strings of the entry and of its kwargs, in order.
    pub comments: Vec<String>,
    /// Creation error, as ign-lint prints it; the rule does not run.
    pub error: Option<String>,
}

impl RuleEntry {
    /// Runs: enabled and created without error.
    pub fn loaded(&self) -> bool {
        self.enabled && self.error.is_none()
    }

    pub fn kwarg(&self, name: &str) -> Option<&Value> {
        self.kwargs.iter().find(|(k, _)| k == name).map(|(_, v)| v)
    }
}

/// A parsed `rule_config.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleConfig {
    pub path: PathBuf,
    /// Top-level `_comment`-style strings, in order.
    pub comments: Vec<String>,
    /// Every ign-lint rule (configured or running with defaults) followed by
    /// configured names ign-lint does not know, in ign-lint's rule order.
    pub rules: Vec<RuleEntry>,
    /// Lines ign-lint prints while loading (`Unknown rule in config: X`,
    /// `Error creating rule X: ...`, `Note: allow_fix has no effect ...`).
    pub messages: Vec<String>,
}

impl RuleConfig {
    pub fn rule(&self, name: &str) -> Option<&RuleEntry> {
        self.rules.iter().find(|r| {
            r.name == name
                || r.ign_lint
                    .is_some_and(|i| i.name == name || i.aliases.contains(&name))
        })
    }

    /// Names of the rules that run.
    pub fn loaded_rules(&self) -> impl Iterator<Item = &RuleEntry> {
        self.rules
            .iter()
            .filter(|r| r.loaded() && r.ign_lint.is_some())
    }
}

fn comment_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) => out.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                comment_strings(item, out);
            }
        }
        other => out.push(other.to_string()),
    }
}

fn python_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::String(text) => format!("'{text}'"),
        other => other.to_string(),
    }
}

/// Python truthiness of a JSON value, which is what ign-lint's
/// `if not config.get("enabled", True)` tests.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Parses `rule_config.json` text.
pub fn parse_rule_config(path: &Path, text: &str) -> Result<RuleConfig, RuleConfigError> {
    let error = |message: String| RuleConfigError {
        path: path.to_path_buf(),
        message,
    };
    let value: Value = serde_json::from_str(text).map_err(|e| error(e.to_string()))?;
    let Value::Object(top) = value else {
        return Err(error(
            "the configuration must be a JSON object of rule names".to_owned(),
        ));
    };
    let mut config = RuleConfig {
        path: path.to_path_buf(),
        comments: Vec::new(),
        rules: Vec::new(),
        messages: Vec::new(),
    };
    let mut configured: BTreeMap<&'static str, RuleEntry> = BTreeMap::new();
    let mut unknown = Vec::new();
    for (name, entry) in &top {
        if name.starts_with('_') {
            comment_strings(entry, &mut config.comments);
            continue;
        }
        let Value::Object(entry) = entry else {
            continue;
        };
        let ign_lint = ign_lint_rule(name);
        let mut rule = RuleEntry {
            name: name.clone(),
            ign_lint,
            configured: true,
            enabled: entry.get("enabled").is_none_or(truthy),
            allow_fix: None,
            kwargs: Vec::new(),
            comments: Vec::new(),
            error: None,
        };
        for (key, value) in entry {
            if key.starts_with('_') {
                comment_strings(value, &mut rule.comments);
            }
        }
        let Some(info) = ign_lint else {
            config
                .messages
                .push(format!("Unknown rule in config: {name}"));
            rule.error = Some(format!("Unknown rule in config: {name}"));
            unknown.push(rule);
            continue;
        };
        if !rule.enabled {
            configured.insert(info.name, rule);
            continue;
        }
        let mut creation_error = None;
        match entry.get("kwargs") {
            None | Some(Value::Null) => {}
            Some(Value::Object(kwargs)) => {
                for (key, value) in kwargs {
                    if key.starts_with('_') {
                        comment_strings(value, &mut rule.comments);
                        continue;
                    }
                    if key == "severity" {
                        let valid = matches!(value.as_str(), Some("warning" | "error"));
                        if info.strict_severity && !valid {
                            creation_error.get_or_insert(format!(
                                "severity must be 'warning' or 'error'; got {}",
                                python_repr(value)
                            ));
                        }
                    } else if !info.accepts_any_kwarg && !info.kwargs.contains(&key.as_str()) {
                        creation_error.get_or_insert(format!(
                            "{}.__init__() got an unexpected keyword argument '{key}'",
                            info.name
                        ));
                    }
                    rule.kwargs.push((key.clone(), value.clone()));
                }
            }
            Some(other) => {
                creation_error = Some(format!(
                    "kwargs must be a JSON object; got {}",
                    python_repr(other)
                ));
            }
        }
        match entry.get("allow_fix") {
            None => {}
            Some(Value::Bool(allow)) => {
                rule.allow_fix = Some(*allow);
                if !info.supports_fix && creation_error.is_none() {
                    config.messages.push(format!(
                        "Note: allow_fix has no effect on {name} (rule does not support auto-fix)"
                    ));
                }
            }
            Some(other) => {
                creation_error.get_or_insert(format!(
                    "allow_fix must be true or false; got {}",
                    python_repr(other)
                ));
            }
        }
        if let Some(message) = creation_error {
            config
                .messages
                .push(format!("Error creating rule {name}: {message}"));
            rule.error = Some(message);
        }
        configured.insert(info.name, rule);
    }
    for info in IGN_LINT_RULES {
        let entry = configured.remove(info.name).unwrap_or_else(|| RuleEntry {
            name: info.name.to_owned(),
            ign_lint: Some(info),
            configured: false,
            enabled: true,
            allow_fix: None,
            kwargs: Vec::new(),
            comments: Vec::new(),
            error: None,
        });
        config.rules.push(entry);
    }
    config.rules.extend(unknown);
    Ok(config)
}

/// Reads `rule_config.json` from disk.
pub fn read_rule_config(path: &Path) -> Result<RuleConfig, RuleConfigError> {
    let text = std::fs::read_to_string(path).map_err(|e| RuleConfigError {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    parse_rule_config(path, &text)
}

/// Reads an ign-lint whitelist: one path per line, relative to `base` (ign-lint
/// uses the current directory), blank lines and `#` lines ignored. A missing
/// file is an empty whitelist.
pub fn read_whitelist(path: &Path, base: &Path) -> std::io::Result<Vec<PathBuf>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    Ok(parse_whitelist(&text, base))
}

pub fn parse_whitelist(text: &str, base: &Path) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| iglint_linter::discovery::absolute(&base.join(line)))
        .collect()
}

/// Which rules run when settings come from an ign-lint configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompatMode {
    /// Only the iglint equivalents of loaded ign-lint rules (`iglint ign-lint`).
    IgnLintOnly,
    /// iglint's default rule set plus the equivalents of loaded ign-lint
    /// rules, minus the equivalents of disabled ones (`iglint check` with a
    /// legacy configuration).
    ExtendDefaults,
}

/// ign-lint settings resolved against the iglint rule registry.
#[derive(Clone, Debug)]
pub struct CompatSettings {
    pub settings: ProjectSettings,
    /// What could not be carried over, for the user.
    pub notices: Vec<String>,
    /// Loaded ign-lint rule name -> the iglint rules implementing it.
    pub rule_map: BTreeMap<String, Vec<RuleCode>>,
}

/// Converts a JSON kwarg to an option value checked against `ty`.
pub fn convert_option(value: &Value, ty: OptionType) -> Result<OptionValue, String> {
    let mut converted =
        json_to_option(value).ok_or_else(|| "null is not a valid value".to_owned())?;
    // ign-lint accepts a single string where it expects a list of strings
    // (`target_node_types: "component"`).
    if ty == OptionType::StringList && matches!(converted, OptionValue::String(_)) {
        converted = OptionValue::List(vec![converted]);
    }
    check_option_type(&converted, ty)?;
    Ok(converted)
}

pub(crate) fn check_option_type(value: &OptionValue, ty: OptionType) -> Result<(), String> {
    let ok = match ty {
        OptionType::Bool => matches!(value, OptionValue::Bool(_)),
        OptionType::Integer => matches!(value, OptionValue::Integer(_)),
        OptionType::Float => matches!(value, OptionValue::Integer(_) | OptionValue::Float(_)),
        OptionType::String => matches!(value, OptionValue::String(_)),
        OptionType::StringList => value.as_string_list().is_some(),
        OptionType::Table => matches!(value, OptionValue::Table(_)),
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "expected {}, found {}",
            option_type_name(ty),
            value.type_name()
        ))
    }
}

pub fn option_type_name(ty: OptionType) -> &'static str {
    match ty {
        OptionType::Bool => "a boolean",
        OptionType::Integer => "an integer",
        OptionType::Float => "a number",
        OptionType::String => "a string",
        OptionType::StringList => "an array of strings",
        OptionType::Table => "a table",
    }
}

/// JSON to option value; `null` (ign-lint's "use the default") gives `None`
/// at the top level and is dropped inside arrays and objects.
pub fn json_to_option(value: &Value) -> Option<OptionValue> {
    Some(match value {
        Value::Null => return None,
        Value::Bool(b) => OptionValue::Bool(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => OptionValue::Integer(i),
            None => OptionValue::Float(n.as_f64().unwrap_or_default()),
        },
        Value::String(s) => OptionValue::String(s.clone()),
        Value::Array(items) => OptionValue::List(items.iter().filter_map(json_to_option).collect()),
        Value::Object(map) => OptionValue::Table(
            map.iter()
                .filter(|(k, _)| !k.starts_with('_'))
                .filter_map(|(k, v)| Some((k.clone(), json_to_option(v)?)))
                .collect(),
        ),
    })
}

/// Rule settings for one iglint rule from an ign-lint entry.
fn rule_settings(
    meta: &'static RuleMeta,
    entry: &RuleEntry,
    mode: CompatMode,
    notices: &mut Vec<String>,
    unmapped: &mut BTreeSet<String>,
) -> RuleSettings {
    let mut severity = match (mode, entry.ign_lint) {
        (CompatMode::IgnLintOnly, Some(info)) => info.default_severity,
        _ => meta.default_severity,
    };
    let mut options = BTreeMap::new();
    for (kwarg, value) in &entry.kwargs {
        if let Some(option) = meta.option_for_ign_lint_kwarg(kwarg) {
            let Some(option_meta) = meta.option(option) else {
                notices.push(format!(
                    "{}: kwarg `{kwarg}` maps to unknown option `{option}` of {}",
                    entry.name, meta.code
                ));
                continue;
            };
            if value.is_null() {
                continue;
            }
            match convert_option(value, option_meta.ty) {
                Ok(converted) => {
                    options.insert(option.to_owned(), converted);
                }
                Err(message) => notices.push(format!(
                    "{}: kwarg `{kwarg}` ignored for {} ({message})",
                    entry.name, meta.code
                )),
            }
        } else if kwarg == "severity" {
            if let Some(parsed) = value.as_str().and_then(Severity::parse) {
                severity = parsed;
            } else {
                severity = Severity::Error;
            }
        } else {
            unmapped.insert(kwarg.clone());
        }
    }
    for (option, values) in meta.ign_lint_implied_options {
        options.entry((*option).to_owned()).or_insert_with(|| {
            OptionValue::List(
                values
                    .iter()
                    .map(|v| OptionValue::String((*v).to_owned()))
                    .collect(),
            )
        });
    }
    RuleSettings { severity, options }
}

/// The severity of a native rule standing in for an ign-lint rule with a
/// `category_mapping` (PylintScriptRule): an error-level rule takes the
/// severity mapped for pylint's `E` category, a warning-level rule the one
/// for `W`. Info-level rules and unmapped categories keep the rule's
/// default. A project that maps `E` to `warning` because pylint
/// misreports Ignition's names keeps its errors-only CI passing.
fn native_severity(meta: &RuleMeta, entry: &RuleEntry) -> Severity {
    let letter = match meta.default_severity {
        Severity::Error => "E",
        Severity::Warning => "W",
        Severity::Info => return meta.default_severity,
    };
    entry
        .kwarg("category_mapping")
        .and_then(|m| m.get(letter))
        .and_then(Value::as_str)
        .and_then(Severity::parse)
        .unwrap_or(meta.default_severity)
}

/// Resolves an ign-lint configuration into iglint settings rooted at `root`.
pub fn to_settings(
    config: &RuleConfig,
    registry: &Registry,
    root: &Path,
    mode: CompatMode,
) -> CompatSettings {
    let mut notices = Vec::new();
    let mut rules: BTreeMap<RuleCode, RuleSettings> = BTreeMap::new();
    if mode == CompatMode::ExtendDefaults {
        for rule in registry.iter().filter(|r| r.meta().default_enabled) {
            rules.insert(
                rule,
                RuleSettings {
                    severity: rule.meta().default_severity,
                    options: BTreeMap::new(),
                },
            );
        }
    }
    let mut unfixable = BTreeSet::new();
    let mut rule_map = BTreeMap::new();
    for entry in &config.rules {
        let Some(info) = entry.ign_lint else {
            continue;
        };
        let equivalents = equivalents(info, registry);
        let targets: Vec<RuleCode> = equivalents.iter().map(|e| e.rule).collect();
        if !entry.loaded() {
            for rule in &targets {
                rules.remove(rule);
            }
            continue;
        }
        if targets.is_empty() {
            if entry.configured {
                notices.push(format!(
                    "{}: iglint has no equivalent of this ign-lint rule yet; it is not run",
                    entry.name
                ));
            }
            rule_map.insert(entry.name.clone(), Vec::new());
            continue;
        }
        let mut unmapped = BTreeSet::new();
        let mut mapped_somewhere = BTreeSet::new();
        for equivalent in &equivalents {
            let rule = &equivalent.rule;
            if !equivalent.direct {
                // Native checks of the category: their own defaults, with
                // the severity the entry's `category_mapping` gives their
                // pylint counterparts.
                rules.insert(
                    *rule,
                    RuleSettings {
                        severity: native_severity(rule.meta(), entry),
                        options: BTreeMap::new(),
                    },
                );
                if entry.allow_fix == Some(false) {
                    unfixable.insert(*rule);
                }
                continue;
            }
            let mut unmapped_here = BTreeSet::new();
            let settings =
                rule_settings(rule.meta(), entry, mode, &mut notices, &mut unmapped_here);
            for (kwarg, _) in &entry.kwargs {
                if !unmapped_here.contains(kwarg) {
                    mapped_somewhere.insert(kwarg.clone());
                }
            }
            unmapped.extend(unmapped_here);
            rules.insert(*rule, settings);
            if entry.allow_fix == Some(false) {
                unfixable.insert(*rule);
            }
        }
        if !equivalents.iter().any(|e| e.direct) {
            unmapped.extend(
                entry
                    .kwargs
                    .iter()
                    .map(|(k, _)| k.clone())
                    .filter(|k| k != "severity"),
            );
        }
        for kwarg in unmapped.difference(&mapped_somewhere) {
            notices.push(format!(
                "{}: kwarg `{kwarg}` has no iglint equivalent and is ignored",
                entry.name
            ));
        }
        rule_map.insert(entry.name.clone(), targets);
    }
    // ign-lint lints `view.json` files only. On top of iglint's defaults,
    // the rules it adds (SCR900 above all) keep that scope.
    let mut overrides = Vec::new();
    if mode == CompatMode::ExtendDefaults {
        let added: BTreeMap<RuleCode, RuleSettings> = rules
            .iter()
            .filter(|(rule, _)| !rule.meta().default_enabled)
            .map(|(rule, settings)| (*rule, settings.clone()))
            .collect();
        if !added.is_empty() {
            rules.retain(|rule, _| rule.meta().default_enabled);
            overrides.push(Override {
                files: vec![crate::migrate::IGN_LINT_SCOPE.to_owned()],
                enable: added,
                ..Override::default()
            });
        }
    }
    let settings = ProjectSettings {
        root: root.to_path_buf(),
        base: Arc::new(LinterSettings {
            rules,
            strict: false,
        }),
        overrides,
        unfixable,
        ..ProjectSettings::default()
    };
    CompatSettings {
        settings,
        notices,
        rule_map,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> RuleConfig {
        parse_rule_config(Path::new("rule_config.json"), text).unwrap()
    }

    /// Configurations written for ign-lint 0.6.3 (the kwargs it accepts,
    /// no `allow_fix`, no `max_value_length`, no property rules) and for
    /// 0.6.4 both load without a message or a creation error.
    #[test]
    fn configurations_of_0_6_3_and_0_6_4_are_accepted() {
        let v063 = parse(
            r#"{
              "NamePatternRule": {"kwargs": {"convention": "PascalCase", "target_node_types": ["component"], "allow_numbers": true, "min_length": 2, "max_length": 40, "forbidden_names": ["x"], "skip_names": ["root"], "allowed_abbreviations": ["ID"], "auto_detect_abbreviations": true, "severity": "warning"}},
              "PollingIntervalRule": {"kwargs": {"minimum_interval": 10000}},
              "PylintScriptRule": {"kwargs": {"pylintrc": ".pylintrc", "debug": false, "batch_mode": false, "debug_dir": "d", "category_mapping": {"E": "error"}}},
              "BadComponentReferenceRule": {"kwargs": {"forbidden_patterns": [".getSibling("], "case_sensitive": true}},
              "ComponentReferenceValidationRule": {"kwargs": {"validate_expressions": true, "validate_property_bindings": true, "validate_scripts": true}},
              "UnusedCustomPropertiesRule": {"enabled": true},
              "ExcessiveContextDataRule": {"kwargs": {"max_array_size": 50, "max_sibling_properties": 50, "max_nesting_depth": 5, "max_data_points": 1000}}
            }"#,
        );
        let v064 = parse(
            r#"{
              "UnusedCustomPropertiesRule": {"allow_fix": false},
              "ExcessiveContextDataRule": {"kwargs": {"max_value_length": 20000}},
              "PropertyAccessRule": {"kwargs": {"expected_access": "PRIVATE", "exempt_props": []}},
              "PropertyPersistenceRule": {"allow_fix": true, "kwargs": {"expected_persistent": false, "exempt_binding_types": []}}
            }"#,
        );
        for config in [&v063, &v064] {
            assert!(config.messages.is_empty(), "{:?}", config.messages);
            assert!(config.rules.iter().all(|r| r.error.is_none()));
            assert_eq!(config.loaded_rules().count(), IGN_LINT_RULES.len());
        }
    }

    #[test]
    fn empty_config_runs_every_rule() {
        let config = parse("{}");
        assert_eq!(config.loaded_rules().count(), IGN_LINT_RULES.len());
        assert!(config.messages.is_empty());
    }

    #[test]
    fn comments_unknown_rules_and_disabled_rules() {
        let config = parse(
            r#"{
              "_comment": "top",
              "Bogus": {"enabled": true},
              "notARule": 5,
              "PollingIntervalRule": {"enabled": false, "_comment": "too noisy"},
              "NamePatternRule": {"kwargs": {"_comment": "inner", "convetion": "x", "severity": "warning"}}
            }"#,
        );
        assert_eq!(config.comments, vec!["top"]);
        assert_eq!(config.messages, vec!["Unknown rule in config: Bogus"]);
        let polling = config.rule("PollingIntervalRule").unwrap();
        assert!(!polling.loaded());
        assert_eq!(polling.comments, vec!["too noisy"]);
        let naming = config.rule("NamePatternRule").unwrap();
        // NamePatternRule accepts any kwarg.
        assert!(naming.loaded());
        assert_eq!(naming.comments, vec!["inner"]);
        assert_eq!(naming.kwargs.len(), 2);
        assert_eq!(config.loaded_rules().count(), IGN_LINT_RULES.len() - 1);
    }

    #[test]
    fn creation_errors_match_ign_lint() {
        let config = parse(
            r#"{
              "PollingIntervalRule": {"kwargs": {"minimum": 5}},
              "UnusedCustomPropertiesRule": {"allow_fix": "yes"},
              "PollingIntervalRule2": {},
              "ExcessiveContextDataRule": {"allow_fix": false},
              "NamePatternRule": {"kwargs": {"severity": "info"}},
              "BadComponentReferenceRule": {"kwargs": [1]}
            }"#,
        );
        assert_eq!(
            config.messages,
            vec![
                "Error creating rule PollingIntervalRule: PollingIntervalRule.__init__() got an unexpected keyword argument 'minimum'",
                "Error creating rule UnusedCustomPropertiesRule: allow_fix must be true or false; got 'yes'",
                "Unknown rule in config: PollingIntervalRule2",
                "Note: allow_fix has no effect on ExcessiveContextDataRule (rule does not support auto-fix)",
                "Error creating rule NamePatternRule: severity must be 'warning' or 'error'; got 'info'",
                "Error creating rule BadComponentReferenceRule: kwargs must be a JSON object; got [1]",
            ]
        );
        assert_eq!(config.loaded_rules().count(), IGN_LINT_RULES.len() - 4);
    }

    #[test]
    fn enabled_uses_python_truthiness() {
        let config = parse(
            r#"{"PollingIntervalRule": {"enabled": 0}, "NamePatternRule": {"enabled": "yes"}}"#,
        );
        assert!(!config.rule("PollingIntervalRule").unwrap().loaded());
        assert!(config.rule("NamePatternRule").unwrap().loaded());
    }

    #[test]
    fn invalid_files() {
        let error = parse_rule_config(Path::new("c.json"), "[1]").unwrap_err();
        assert_eq!(
            error.to_string(),
            "Error loading config file c.json: the configuration must be a JSON object of rule names"
        );
        let error = parse_rule_config(Path::new("c.json"), "{").unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("Error loading config file c.json: EOF")
        );
    }

    #[test]
    fn whitelist_lines() {
        let list = parse_whitelist(
            "# comment\n\n  a/view.json  \nb/../c/view.json\n",
            Path::new("/repo"),
        );
        assert_eq!(
            list,
            vec![
                PathBuf::from("/repo/a/view.json"),
                PathBuf::from("/repo/c/view.json")
            ]
        );
    }
}
