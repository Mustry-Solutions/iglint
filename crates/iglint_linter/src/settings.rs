//! Resolved linter settings.
//!
//! `iglint_config` turns configuration files into a [`ProjectSettings`]; the
//! linter only ever sees the resolved form. Settings for a file are the base
//! settings with every matching override applied in order.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::diagnostic::Severity;
use crate::registry::RuleCode;

/// A rule option value. Mirrors TOML, so JSON from ign-lint configs converts
/// losslessly too.
#[derive(Clone, Debug, PartialEq)]
pub enum OptionValue {
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    List(Vec<OptionValue>),
    Table(BTreeMap<String, OptionValue>),
}

impl OptionValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            OptionValue::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            OptionValue::Integer(value) => Some(*value),
            OptionValue::Float(value) if value.fract() == 0.0 => Some(*value as i64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            OptionValue::Integer(value) => Some(*value as f64),
            OptionValue::Float(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            OptionValue::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[OptionValue]> {
        match self {
            OptionValue::List(values) => Some(values),
            _ => None,
        }
    }

    pub fn as_table(&self) -> Option<&BTreeMap<String, OptionValue>> {
        match self {
            OptionValue::Table(table) => Some(table),
            _ => None,
        }
    }

    pub fn as_string_list(&self) -> Option<Vec<&str>> {
        self.as_list()?.iter().map(OptionValue::as_str).collect()
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            OptionValue::Bool(_) => "boolean",
            OptionValue::Integer(_) => "integer",
            OptionValue::Float(_) => "float",
            OptionValue::String(_) => "string",
            OptionValue::List(_) => "array",
            OptionValue::Table(_) => "table",
        }
    }
}

/// Settings of one enabled rule.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleSettings {
    pub severity: Severity,
    /// Options as configured; absent keys use the rule's defaults.
    pub options: BTreeMap<String, OptionValue>,
}

/// The fully resolved settings that apply to one file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinterSettings {
    /// Enabled rules. A rule not in this map does not run.
    pub rules: BTreeMap<RuleCode, RuleSettings>,
    /// Report dynamic references that cannot be resolved statically.
    pub strict: bool,
}

impl LinterSettings {
    pub fn rule(&self, rule: RuleCode) -> Option<&RuleSettings> {
        self.rules.get(&rule)
    }

    pub fn is_enabled(&self, rule: RuleCode) -> bool {
        self.rules.contains_key(&rule)
    }
}

/// A glob-matched adjustment of the base settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Override {
    /// Glob patterns relative to the project root, `/`-separated.
    pub files: Vec<String>,
    /// Rules to switch off for matching files.
    pub disable: Vec<RuleCode>,
    /// Rules to switch on (or reconfigure) for matching files.
    pub enable: BTreeMap<RuleCode, RuleSettings>,
    pub strict: Option<bool>,
    /// New severities for rules that are enabled for matching files. Unlike
    /// `enable`, this never switches a rule on.
    pub severity: BTreeMap<RuleCode, Severity>,
}

/// Settings for a whole lint run.
#[derive(Clone, Debug)]
pub struct ProjectSettings {
    /// Directory that relative paths in settings are resolved against.
    pub root: PathBuf,
    pub base: Arc<LinterSettings>,
    /// Applied in order to files they match.
    pub overrides: Vec<Override>,
    /// Glob patterns of files not linted at all.
    pub exclude: Vec<String>,
    /// Per-rule glob patterns of files the rule skips.
    pub rule_exclude: BTreeMap<RuleCode, Vec<String>>,
    /// Files skipped entirely, as listed in an ign-lint whitelist.
    pub whitelist: Vec<PathBuf>,
    /// Paths of Ignition projects to lint; empty means discover.
    pub projects: Vec<PathBuf>,
    /// Paths of gateway configuration resource trees; empty means discover.
    pub gateways: Vec<PathBuf>,
    /// Skip files ignored by `.gitignore`, `.ignore` and git excludes.
    pub respect_gitignore: bool,
    /// Rules whose fixes are never applied.
    pub unfixable: BTreeSet<RuleCode>,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        ProjectSettings {
            root: PathBuf::new(),
            base: Arc::default(),
            overrides: Vec::new(),
            exclude: Vec::new(),
            rule_exclude: BTreeMap::new(),
            whitelist: Vec::new(),
            projects: Vec::new(),
            gateways: Vec::new(),
            respect_gitignore: true,
            unfixable: BTreeSet::new(),
        }
    }
}

/// Matches `/`-separated relative paths against glob patterns.
///
/// Supports `*` (within a segment), `**` (any number of segments), `?`, and
/// character classes `[abc]`. A pattern without `/` matches a file or
/// directory name at any depth (and so everything below that directory),
/// like `.gitignore`.
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    if !pattern.contains('/') {
        return path
            .split('/')
            .any(|segment| segment_matches(pattern.as_bytes(), segment.as_bytes()))
            || matches_segments(pattern, path);
    }
    let pattern = pattern.strip_prefix('/').unwrap_or(pattern);
    matches_segments(pattern, path)
        // A pattern naming a directory matches everything below it.
        || matches_segments(&format!("{}/**", pattern.trim_end_matches('/')), path)
}

fn matches_segments(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let path: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match_parts(&pattern, &path)
}

fn match_parts(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| match_parts(rest, &path[skip..])),
        Some((first, rest)) => match path.split_first() {
            Some((segment, path_rest)) => {
                segment_matches(first.as_bytes(), segment.as_bytes())
                    && match_parts(rest, path_rest)
            }
            None => false,
        },
    }
}

fn segment_matches(pattern: &[u8], text: &[u8]) -> bool {
    // Iterative wildcard matching with backtracking on the last `*`.
    let (mut p, mut t) = (0, 0);
    let (mut star, mut star_t) = (None, 0);
    while t < text.len() {
        if p < pattern.len() {
            match pattern[p] {
                b'*' => {
                    star = Some(p);
                    star_t = t;
                    p += 1;
                    continue;
                }
                b'?' => {
                    p += 1;
                    t += 1;
                    continue;
                }
                b'[' => {
                    if let Some((matched, next)) = class_matches(&pattern[p..], text[t]) {
                        if matched {
                            p += next;
                            t += 1;
                            continue;
                        }
                    } else if text[t] == b'[' {
                        p += 1;
                        t += 1;
                        continue;
                    }
                }
                c if c == text[t] => {
                    p += 1;
                    t += 1;
                    continue;
                }
                _ => {}
            }
        }
        match star {
            Some(s) => {
                p = s + 1;
                star_t += 1;
                t = star_t;
            }
            None => return false,
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

/// Matches a `[...]` class at the start of `pattern`. Returns whether `c`
/// matched and the length of the class, or `None` if the class is unclosed.
fn class_matches(pattern: &[u8], c: u8) -> Option<(bool, usize)> {
    let mut i = 1;
    let negate = matches!(pattern.get(1), Some(b'!' | b'^'));
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < pattern.len() {
        if pattern[i] == b']' && !first {
            return Some((matched != negate, i + 1));
        }
        first = false;
        if i + 2 < pattern.len() && pattern[i + 1] == b'-' && pattern[i + 2] != b']' {
            if pattern[i] <= c && c <= pattern[i + 2] {
                matched = true;
            }
            i += 3;
        } else {
            if pattern[i] == c {
                matched = true;
            }
            i += 1;
        }
    }
    None
}

impl ProjectSettings {
    /// Path of `file` relative to the settings root, `/`-separated.
    pub fn relative_path(&self, file: &Path) -> String {
        let relative = file.strip_prefix(&self.root).unwrap_or(file);
        relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Whether `file` is excluded from linting altogether.
    pub fn is_excluded(&self, file: &Path) -> bool {
        let relative = self.relative_path(file);
        self.exclude.iter().any(|p| glob_matches(p, &relative))
            || self
                .whitelist
                .iter()
                .any(|w| w == file || self.relative_path(w) == relative)
    }

    /// The settings that apply to `file`.
    pub fn for_file(&self, file: &Path) -> Arc<LinterSettings> {
        let relative = self.relative_path(file);
        let rule_excluded: Vec<RuleCode> = self
            .rule_exclude
            .iter()
            .filter(|(_, globs)| globs.iter().any(|g| glob_matches(g, &relative)))
            .map(|(rule, _)| *rule)
            .collect();
        let matching: Vec<&Override> = self
            .overrides
            .iter()
            .filter(|o| o.files.iter().any(|g| glob_matches(g, &relative)))
            .collect();
        if matching.is_empty() && rule_excluded.is_empty() {
            return Arc::clone(&self.base);
        }
        let mut settings = (*self.base).clone();
        for o in matching {
            for rule in &o.disable {
                settings.rules.remove(rule);
            }
            for (rule, rule_settings) in &o.enable {
                settings.rules.insert(*rule, rule_settings.clone());
            }
            if let Some(strict) = o.strict {
                settings.strict = strict;
            }
            for (rule, severity) in &o.severity {
                if let Some(rule_settings) = settings.rules.get_mut(rule) {
                    rule_settings.severity = *severity;
                }
            }
        }
        for rule in rule_excluded {
            settings.rules.remove(&rule);
        }
        Arc::new(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::glob_matches;

    #[test]
    fn globs() {
        assert!(glob_matches("**/view.json", "projects/a/views/x/view.json"));
        assert!(glob_matches("view.json", "projects/a/views/x/view.json"));
        assert!(glob_matches(
            "projects/*/views/Legacy/**",
            "projects/a/views/Legacy/x/view.json"
        ));
        assert!(!glob_matches(
            "projects/*/views/Legacy/**",
            "projects/a/views/New/x/view.json"
        ));
        assert!(glob_matches(
            "projects/a/views/Legacy",
            "projects/a/views/Legacy/x/view.json"
        ));
        assert!(glob_matches("*.py", "a/b/code.py"));
        assert!(glob_matches("build", "build/x.py"));
        assert!(glob_matches(
            "Legacy",
            "projects/a/views/Legacy/x/view.json"
        ));
        assert!(!glob_matches(
            "Legacy",
            "projects/a/views/LegacyNot/x/view.json"
        ));
        assert!(glob_matches("a/[bc]/*.py", "a/c/code.py"));
        assert!(!glob_matches("a/[!bc]/*.py", "a/c/code.py"));
        assert!(glob_matches("a/?/x", "a/b/x"));
        assert!(!glob_matches("a/*/x", "a/b/c/x"));
        assert!(glob_matches("a/**/x", "a/x"));
    }
}
