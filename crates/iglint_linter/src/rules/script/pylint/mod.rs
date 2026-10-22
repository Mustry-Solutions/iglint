//! SCR900 pylint: real pylint over every script, in one process per run.
//!
//! The work is split in two:
//!
//! * [`run`]: after the project index is built and before the project
//!   phase, the driver (`analyze`) collects the scripts of every linted unit
//!   that enables SCR900, writes each script's `source` (the generated
//!   header and the body, as Ignition compiles it) to its own `.py` file in
//!   one temporary directory, runs pylint once over that directory and maps
//!   every message back to a range of its script's `source`. The results are
//!   stored in the unit's cache ([`UnitMessages`]).
//! * The rule itself ([`Pylint`]) runs as a script rule in the project phase
//!   and reports the stored messages of each script, so the script's
//!   `# iglint: ignore[...]` comments apply to them and `SCR123` knows
//!   which comments they used.

use std::collections::BTreeMap;

use iglint_project::ScriptKind;
use iglint_text::TextRange;
use rustc_hash::FxHashMap;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::diagnostic::{Applicability, Diagnostic, Fix};
use crate::options::OptionReader;
use crate::registry::{Category, FixAvailability, OptionMeta, OptionType, RuleMeta};
use crate::rule::{Hook, ParsedOptions, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{
    file_line, fix, in_header, injected_names, is_python_space, string_ranges, trailing_whitespace,
};

pub(crate) mod run;
#[cfg(test)]
mod tests;

pub static META: RuleMeta = RuleMeta {
    code: "SCR900",
    name: "pylint",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: false,
    fix: FixAvailability::Sometimes,
    summary: "Message reported by pylint",
    explanation: r#"## What it does

Runs the real [pylint](https://pylint.readthedocs.io/) over every script
and reports its messages at their exact place in the file: library
modules, gateway, session and WebDev event files, and every script
embedded in a view, session properties or tags (with the `def` header
Ignition compiles it under). This is the equivalent of ign-lint's
`PylintScriptRule`.

pylint runs once per lint run, in one process, over all scripts. Each
script is linted as its own module, so module-wide checks (such as
`inconsistent-quotes`) never mix scripts. pylint must be installed
(`pip install pylint`); when it cannot be started, the run prints one
warning and every other rule still runs.

The rule severity (`severity` in `[rules.SCR900]`) applies only to
message categories that `category-mapping` does not map: for a mapped
category the mapping wins, for every kind of script (library modules,
event scripts, WebDev, views, tags). With `severity = "error"` and
`category-mapping = { C = "warning" }`, `C0103` is a warning everywhere.

Messages that pylint gets wrong for Ignition are not reported:

* `undefined-variable` and `import-error` for names every script sees
  (`system`, `fpmi`, `sys`, the project's top-level library packages)
  and for Jython 2.7 builtins (`xrange`, `basestring`, `unicode`, ...).
  When the project's library is not known (scripts outside a project),
  `undefined-variable` is not reported at all, like `SCR101`.
* `syntax-error` for scripts that are valid Jython 2.7 (pylint parses
  Python 3, so `print "x"` or `except E, e:` fail there). Real syntax
  errors are `SCR001`'s; `syntax-error` is reported only when the script
  does not parse and `SCR001` is disabled.
* Naming, docstring, argument and signature messages about the generated
  header (`def runAction(self, event):`), which Ignition writes and the
  user cannot change. Other messages located in the header (such as
  `too-many-branches`) are reported at the script's property. Custom
  method headers are the user's, so all of their messages are reported.

## Why

pylint has hundreds of checks beyond iglint's native `SCR` rules
(conventions, refactoring hints, many more error classes). Teams that
used ign-lint's pylint integration keep the same checks, without its
combined-file artefacts (wrong line numbers, one Python 2 statement
hiding every finding of a view, messages leaking between scripts).

## Example

```python
def runAction(self, event):
	if self.props.text == None:
		self.props.text = "empty"
```

```text
Comparison 'self.props.text == None' should be 'self.props.text is None' (singleton-comparison, C0121)
```

## Options

* `pylint`: the command that runs pylint, with arguments, split on
  whitespace (for example `"python3 -m pylint --enable=C0303"`). Single
  and double quotes group a word that contains spaces (`"'/opt/my
  tools/pylint' --jobs=1"`); inside double quotes `\"` and `\\` escape,
  other backslashes are literal (Windows paths). Every argument is passed
  to pylint, before iglint's own. A relative path with a directory part
  is relative to the project root.
* `pylintrc`: pylint configuration file, relative to the project root.
  By default iglint uses its bundled configuration: ign-lint's
  `.ignition-pylintrc` (MIT License), which also disables
  `unused-argument`, `missing-docstring`, `redefined-outer-name` and
  `function-redefined` as ign-lint's combined file did. A missing file
  falls back to the bundled configuration with a warning.
* `jobs`: number of pylint processes that run at the same time, each on
  a share of the scripts (with pylint's `--jobs=1`); `0` (the default)
  starts one per CPU, `1` lints in a single process. Fewer processes are
  started when there are few scripts (at least 8 per process). Scripts
  are independent modules, so the findings do not depend on `jobs`.
  Where possible (POSIX, and a `pylint` script or `python -m pylint` as
  the command), one Python process imports pylint once and forks the
  others.
* `disable`: pylint messages to disable in addition to the configuration
  file (passed as `--disable`). Empty by default. ign-lint's combined file
  starts with `#pylint: disable=unused-argument,missing-docstring,redefined-outer-name,function-redefined`,
  which applies whatever rcfile is used; a configuration read from an
  ign-lint `rule_config.json` (and `iglint ign-lint`) therefore sets
  `disable` to those four messages, so equal configurations give equal
  findings. The bundled configuration disables them already; with your
  own `pylintrc` in `iglint.toml` they are reported unless the rcfile or
  `disable` turns them off.
* `category-mapping`: severity per pylint message category letter (`F`,
  `E`, `W`, `C`, `R`, `I`). It replaces the default
  (`F` and `E` errors, `W`, `C` and `R` warnings); categories it leaves
  out get the rule severity.

## Fix

When pylint reports `trailing-whitespace` (C0303, disabled by the bundled
configuration), the script's first such finding carries ign-lint's fix:
every line of the script is `rstrip()`ed. Safe, except when whitespace
inside a multi-line string literal would be removed or the script does
not parse: then unsafe. `SCR124` is the native equivalent.

## Duplicates

A message that a native rule also reports at the same place (and about
the same name), for example `undefined-variable` and `SCR101`, is shown
only as the native finding.

## Cache

Results are cached with the rest of the run. The cache key includes the
command, `jobs` and the content of the pylint configuration, but not
pylint's version: run with `--no-cache` after upgrading pylint.
"#,
    options: &[
        OptionMeta {
            name: "pylint",
            ty: OptionType::String,
            default: "\"pylint\"",
            description: "Command that runs pylint, split on whitespace.",
        },
        OptionMeta {
            name: "pylintrc",
            ty: OptionType::String,
            default: "none",
            description: "pylint configuration file relative to the project root; the bundled one by default.",
        },
        OptionMeta {
            name: "jobs",
            ty: OptionType::Integer,
            default: "0",
            description: "Number of parallel pylint processes: 0 is one per CPU.",
        },
        OptionMeta {
            name: "disable",
            ty: OptionType::StringList,
            default: "[]",
            description: "pylint messages disabled in addition to the configuration file.",
        },
        OptionMeta {
            name: "category-mapping",
            ty: OptionType::Table,
            default: "{ F = \"error\", E = \"error\", W = \"warning\", C = \"warning\", R = \"warning\" }",
            description: "Severity per pylint category letter; replaces the default.",
        },
    ],
    ign_lint_names: &["PylintScriptRule", "PerspectiveScriptPylintRule"],
    ign_lint_option_map: &[
        ("pylintrc", "pylintrc"),
        ("category_mapping", "category-mapping"),
    ],
    // The header of ign-lint's combined file.
    ign_lint_implied_options: &[(
        "disable",
        &[
            "unused-argument",
            "missing-docstring",
            "redefined-outer-name",
            "function-redefined",
        ],
    )],
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&Pylint)],
    options: Some(parse_options),
};

/// The parsed options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PylintOptions {
    /// Program and leading arguments.
    pub command: Vec<String>,
    pub pylintrc: Option<String>,
    pub jobs: u32,
    /// Messages disabled in addition to the configuration file.
    pub disable: Vec<String>,
    /// Category letter (upper case) to severity.
    pub category_mapping: BTreeMap<char, Severity>,
}

impl PylintOptions {
    /// The severity of a message id such as `C0103`, when its category is
    /// mapped.
    pub fn severity_of(&self, message_id: &str) -> Option<Severity> {
        let letter = message_id.chars().next()?.to_ascii_uppercase();
        self.category_mapping.get(&letter).copied()
    }
}

/// Splits the `pylint` option into words: whitespace separates them,
/// `'...'` and `"..."` quote (inside double quotes `\"` and `\\` escape);
/// other backslashes are kept, for Windows paths.
pub(crate) fn split_command(text: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if let Some(w) = word.take() {
                    words.push(w);
                }
            }
            '\'' | '"' => {
                let w = word.get_or_insert_with(String::new);
                loop {
                    match chars.next() {
                        None => {
                            return Err(format!(
                                "option `pylint` of SCR900 has an unterminated {c} quote"
                            ));
                        }
                        Some(q) if q == c => break,
                        Some('\\') if c == '"' && matches!(chars.peek(), Some('"' | '\\')) => {
                            w.extend(chars.next());
                        }
                        Some(other) => w.push(other),
                    }
                }
            }
            c => word.get_or_insert_with(String::new).push(c),
        }
    }
    words.extend(word);
    Ok(words)
}

fn parse_options(r: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    let command = split_command(&r.string("pylint")?)?;
    if command.is_empty() {
        return Err("option `pylint` of SCR900 must name a command".to_owned());
    }
    let jobs = r.int("jobs")?;
    let jobs = u32::try_from(jobs)
        .map_err(|_| format!("option `jobs` of SCR900 must be 0 or more, found {jobs}"))?;
    let mut category_mapping = BTreeMap::new();
    for (key, value) in r.table("category-mapping")? {
        let mut letters = key.chars();
        let letter = match (letters.next(), letters.next()) {
            (Some(c), None) if "FEWCRI".contains(c.to_ascii_uppercase()) => c.to_ascii_uppercase(),
            _ => {
                return Err(format!(
                    "`category-mapping` of SCR900: `{key}` is not a pylint category letter (F, E, W, C, R, I)"
                ));
            }
        };
        let severity = value.as_str().and_then(Severity::parse).ok_or_else(|| {
            format!(
                "`category-mapping` of SCR900: the severity of `{key}` must be \"error\", \"warning\" or \"info\""
            )
        })?;
        category_mapping.insert(letter, severity);
    }
    Ok(Box::new(PylintOptions {
        command,
        pylintrc: r.opt_string("pylintrc")?,
        jobs,
        disable: r.string_list("disable")?,
        category_mapping,
    }))
}

/// One pylint message, mapped to its script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Message {
    /// Range in the script's `source`.
    pub range: TextRange,
    /// `C0103`.
    pub id: String,
    /// `invalid-name`.
    pub symbol: String,
    pub text: String,
}

/// The pylint messages of one unit, by script position.
#[derive(Debug, Default)]
pub(crate) struct UnitMessages {
    pub by_script: FxHashMap<usize, Vec<Message>>,
    /// Whether `SCR001 syntax-error` runs on the unit.
    pub syntax_rule_enabled: bool,
}

/// Messages about the generated function's name, parameters and
/// docstrings: Ignition writes the header, so they are not actionable.
const SIGNATURE_MESSAGES: &[&str] = &[
    "invalid-name",
    "disallowed-name",
    "non-ascii-name",
    "unused-argument",
    "missing-function-docstring",
    "missing-module-docstring",
    "missing-docstring",
    "too-many-arguments",
    "too-many-positional-arguments",
    "redefined-outer-name",
    "redefined-builtin",
];

struct Pylint;

impl ScriptRule for Pylint {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(found) = cx.unit().cache.pylint.get() else {
            return;
        };
        let Some(messages) = found.by_script.get(&unit.index) else {
            return;
        };
        let options: &PylintOptions = cx.options();
        let script = unit.script;
        let mut reports = Vec::new();
        {
            let (injected, unknown_globals) = injected_names(cx, script);
            for m in messages {
                if !keep(
                    m,
                    unit,
                    &injected,
                    unknown_globals,
                    found.syntax_rule_enabled,
                ) {
                    continue;
                }
                let header = script.source_map.is_synthetic(m.range.start());
                if header
                    && script.kind != ScriptKind::CustomMethod
                    && SIGNATURE_MESSAGES.contains(&m.symbol.as_str())
                {
                    continue;
                }
                reports.push((m, header));
            }
        }
        let mut whitespace_fix = None;
        for (m, header) in reports {
            let severity = options.severity_of(&m.id).unwrap_or(cx.severity());
            let text = format!("{} ({}, {})", m.text, m.symbol, m.id);
            let mut fix = None;
            let (range, ign_lint) = if header {
                (
                    script.anchor(),
                    format!("{} ({}) ({})", m.text, m.symbol, m.id),
                )
            } else {
                let line = file_line(cx, unit, m.range.start()).unwrap_or(1);
                let mut range = m.range;
                if m.id == TRAILING_WHITESPACE {
                    // The whitespace itself, so that removing it resolves
                    // the finding; the first finding of the script carries
                    // ign-lint's fix for the whole script.
                    range = whitespace_on_line(&script.source, range).unwrap_or(range);
                    if whitespace_fix.is_none() {
                        whitespace_fix = Some(strip_trailing_whitespace(cx, unit));
                        fix = whitespace_fix.clone().flatten();
                    }
                }
                (
                    script.file_range(range),
                    format!("Line {line}: {} ({}) ({})", m.text, m.symbol, m.id),
                )
            };
            let _ = cx
                .report_in(unit.file, range, text)
                .node_path(script.node_path.clone())
                .ign_lint_path(script.ign_lint_path.clone())
                .ign_lint_message(ign_lint)
                .severity(severity)
                .fix_opt(fix);
        }
    }
}

/// pylint messages that native rules report as well: message id and the
/// native rule codes.
const NATIVE_EQUIVALENTS: &[(&str, &[&str])] = &[
    ("E0001", &["SCR001"]),
    ("E0602", &["SCR101"]),
    ("W0611", &["SCR102"]),
    ("W0612", &["SCR103"]),
    ("W0404", &["SCR104"]),
    ("E0102", &["SCR104"]),
    ("W0101", &["SCR105"]),
    ("W0702", &["SCR106"]),
    ("W0102", &["SCR107"]),
    ("W0109", &["SCR108"]),
    ("W0127", &["SCR109"]),
    ("R0123", &["SCR110"]),
    ("W0199", &["SCR111"]),
    ("E1305", &["SCR112"]),
    ("E1306", &["SCR112"]),
    ("E0701", &["SCR113"]),
    ("W0104", &["SCR114"]),
    ("W0105", &["SCR114"]),
    ("W0622", &["SCR117"]),
    ("E0601", &["SCR118"]),
    ("W0150", &["SCR119"]),
    ("W0640", &["SCR120"]),
    ("W0604", &["SCR121"]),
    ("C0303", &["SCR124"]),
];

/// The pylint message id (`E0602`) and text of an SCR900 finding.
pub fn message_id(diagnostic: &Diagnostic) -> Option<(&str, &str)> {
    if diagnostic.rule.code() != META.code {
        return None;
    }
    let message = diagnostic.message.strip_suffix(')')?;
    let open = message.rfind(" (")?;
    let (_, id) = message[open + 2..].rsplit_once(", ")?;
    Some((id, &message[..open]))
}

/// The name a pylint message is about: the first `'...'`, or the module
/// of `Unused import x`.
fn subject(id: &str, text: &str) -> Option<String> {
    if let Some(name) = quoted(text) {
        return Some(name.to_owned());
    }
    if id == "W0611" {
        let rest = text.strip_prefix("Unused ")?;
        let rest = rest.strip_prefix("import ").unwrap_or(rest);
        return rest.split_whitespace().next().map(str::to_owned);
    }
    None
}

/// Drops pylint findings that a native rule also reports: same file,
/// overlapping ranges, an equivalent rule (and the same name, when the
/// message names one).
pub fn drop_duplicates(diagnostics: &mut Vec<Diagnostic>) {
    if !diagnostics.iter().any(|d| d.rule.code() == META.code) {
        return;
    }
    let mut native: FxHashMap<&std::path::Path, Vec<usize>> = FxHashMap::default();
    for (i, d) in diagnostics.iter().enumerate() {
        if NATIVE_EQUIVALENTS
            .iter()
            .any(|(_, codes)| codes.contains(&d.rule.code()))
        {
            native.entry(d.file.path()).or_default().push(i);
        }
    }
    if native.is_empty() {
        return;
    }
    let duplicate: Vec<bool> = diagnostics
        .iter()
        .map(|d| {
            let Some((id, text)) = message_id(d) else {
                return false;
            };
            let Some((_, codes)) = NATIVE_EQUIVALENTS.iter().find(|(i, _)| *i == id) else {
                return false;
            };
            let Some(candidates) = native.get(d.file.path()) else {
                return false;
            };
            let name = subject(id, text).map(|n| format!("`{n}`"));
            candidates.iter().any(|i| {
                let other = &diagnostics[*i];
                codes.contains(&other.rule.code())
                    && other.range.start() <= d.range.end()
                    && d.range.start() <= other.range.end()
                    && name
                        .as_ref()
                        .is_none_or(|n| other.message.contains(n.as_str()))
            })
        })
        .collect();
    let mut index = 0;
    diagnostics.retain(|_| {
        index += 1;
        !duplicate[index - 1]
    });
}

/// pylint's `trailing-whitespace`.
const TRAILING_WHITESPACE: &str = "C0303";

/// The trailing whitespace (as Python's `rstrip` sees it) of the line that
/// `at` is on.
fn whitespace_on_line(source: &str, at: TextRange) -> Option<TextRange> {
    let start = at.start().to_usize().min(source.len());
    let line_start = source[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = source[line_start..]
        .find('\n')
        .map_or(source.len(), |i| line_start + i);
    let line = &source[line_start..line_end];
    let kept = line.trim_end_matches(is_python_space).len();
    (kept < line.len()).then(|| TextRange::from(line_start + kept..line_end))
}

/// ign-lint's fix for `trailing-whitespace`: every line of the script body
/// `rstrip()`ed (also the last one, and a `\r` before a line break). Safe
/// like ign-lint's, except when whitespace inside a string literal would go
/// (or the script does not parse, so string literals are unknown): unsafe.
fn strip_trailing_whitespace(cx: &Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) -> Option<Fix> {
    let script = unit.script;
    let source: &str = &script.source;
    let ranges: Vec<TextRange> = trailing_whitespace(
        source,
        script.source_map.body_start(),
        false,
        is_python_space,
    )
    .into_iter()
    .filter(|r| !in_header(unit, *r))
    .collect();
    if ranges.is_empty() {
        return None;
    }
    let strings = string_ranges(unit.parsed);
    let in_string = |r: &TextRange| {
        strings
            .iter()
            .any(|s| s.start() < r.start() && r.start() < s.end())
    };
    let applicability = if !unit.parsed.errors.is_empty() || ranges.iter().any(in_string) {
        Applicability::Unsafe
    } else {
        Applicability::Safe
    };
    let path = script.ign_lint_path.as_deref().unwrap_or(&script.node_path);
    fix(
        cx,
        unit,
        format!("Remove trailing whitespace from script at {path}"),
        applicability,
        ranges.into_iter().map(|r| (r, String::new())).collect(),
    )
}

/// Whether a message is right for Ignition (see the rule documentation).
fn keep(
    m: &Message,
    unit: &ScriptUnit<'_, '_>,
    injected: &[&str],
    unknown_globals: bool,
    syntax_rule_enabled: bool,
) -> bool {
    let known = |name: &str| injected.contains(&name) || iglint_stubs::is_jython_builtin(name);
    match m.symbol.as_str() {
        "syntax-error" => !syntax_rule_enabled && !unit.parsed.errors.is_empty(),
        "undefined-variable" => !unknown_globals && quoted(&m.text).is_none_or(|name| !known(name)),
        "import-error" => quoted(&m.text).is_none_or(|module| {
            let top = module.split('.').next().unwrap_or(module);
            !known(top)
        }),
        _ => true,
    }
}

/// The first `'...'` of a pylint message (`Undefined variable 'x'`).
fn quoted(text: &str) -> Option<&str> {
    let start = text.find('\'')? + 1;
    let end = start + text[start..].find('\'')?;
    Some(&text[start..end])
}

/// The options of SCR900 in a rule set, when it is enabled.
pub(crate) fn options_in(set: &crate::rule::RuleSet) -> Option<&PylintOptions> {
    set.active
        .iter()
        .find(|a| a.rule.code() == META.code)?
        .options
        .as_ref()?
        .downcast_ref()
}
