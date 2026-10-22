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
use crate::options::OptionReader;
use crate::registry::{Category, OptionMeta, OptionType, RuleMeta};
use crate::rule::{Hook, ParsedOptions, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{file_line, injected_names};

pub(crate) mod run;
#[cfg(test)]
mod tests;

pub static META: RuleMeta = RuleMeta {
    code: "SCR900",
    name: "pylint",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: false,
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

The rule severity applies to message categories that `category-mapping`
does not map.

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

* `pylint`: the command that runs pylint, split on whitespace (for
  example `"python3 -m pylint"`). A relative path with a directory part
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
    ..RuleMeta::DEFAULT
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

fn parse_options(r: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    let command: Vec<String> = r
        .string("pylint")?
        .split_whitespace()
        .map(str::to_owned)
        .collect();
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
        for (m, header) in reports {
            let severity = options.severity_of(&m.id).unwrap_or(cx.severity());
            let text = format!("{} ({}, {})", m.text, m.symbol, m.id);
            let (range, ign_lint) = if header {
                (
                    script.anchor(),
                    format!("{} ({}) ({})", m.text, m.symbol, m.id),
                )
            } else {
                let line = file_line(cx, unit, m.range.start()).unwrap_or(1);
                (
                    script.file_range(m.range),
                    format!("Line {line}: {} ({}) ({})", m.text, m.symbol, m.id),
                )
            };
            let _ = cx
                .report_in(unit.file, range, text)
                .node_path(script.node_path.clone())
                .ign_lint_path(script.ign_lint_path.clone())
                .ign_lint_message(ign_lint)
                .severity(severity);
        }
    }
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
