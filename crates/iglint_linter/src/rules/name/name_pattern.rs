//! NAME001 name-pattern.

use std::collections::BTreeMap;

use iglint_json::{JsonString, Value, ValueKind};
use iglint_project::node_path::strip_indices;
use iglint_project::script::ScriptBody;
use iglint_project::view::{BindingConfig, Component, PropCategory, View};
use iglint_project::{FileId, Owner};
use iglint_text::{TextEdit, TextRange, TextSize};
use regex::Regex;
use rustc_hash::FxHashSet;

use super::convention::{
    BUILTIN_ABBREVIATIONS, Convention, compile, normalize_abbreviations, suggest,
};
use super::rename::{References, escaped};
use crate::checker::Checker;
use crate::diagnostic::{Fix, ReasonSite, Severity, UnsafeReason};
use crate::options::OptionReader;
use crate::registry::{Category, FixAvailability, OptionMeta, OptionType, RuleMeta};
use crate::rule::{Hook, ParsedOptions, Phase, RuleImpl, ViewRule};
use crate::script_facts::AccessTarget;
use crate::settings::OptionValue;

pub static META: RuleMeta = RuleMeta {
    code: "NAME001",
    name: "name-pattern",
    category: Category::Name,
    default_severity: Severity::Warning,
    default_enabled: false,
    fix: FixAvailability::Sometimes,
    fix_safety: crate::registry::FixSafety::Depends,
    summary: "Name does not follow the configured naming convention",
    explanation: r#"## What it does

Checks the names of components (`meta.name`), properties (custom
properties, view parameters and the free-form keys inside them), message
handlers (their `messageType`) and custom methods against a naming
convention or a regular expression, with per-node-type settings. By default
only component names are checked, against PascalCase, and `root` is skipped.

A failing name gets a suggestion in the configured convention. For
components the suggestion is offered as a fix that renames the component
and updates every reference to it in the view: property references in
every binding and transform field Perspective parses as an expression
(`{../Label.props.text}`, `{/root/Panel/Label.custom.x}`; the fields are
measured in the Ignition image: expression bindings and structures, query
parameters, tag paths and indirect references, polling rates, tag history
date ranges and return sizes, HTTP URLs, bodies, header and authorization
values, expression and map transforms), property binding paths, and
literal `getChild('...')` / `getSibling('...')` calls in scripts whose
receiver resolves (`self`, `self.parent`, `self.view.rootContainer`,
earlier lookups). References are found by resolution, so a different
component with the same name elsewhere in the view is left alone. The
rename is unsafe when a script could find the component another way: it
reads `meta.name`, or the component's name through the members measured
to return it (`self.name`, `c.getName()`, also for a component reached
through `children`, `getChildren()`, a loop or an index), looks children
up with a computed name or a matching string, hands a component to code
outside the view, reaches `eval`, `locals()` or a frame in any way (also
under another name, `__builtin__.eval`), calls an unnamed value
(`factory()()`), or calls a library name that is not a plain function,
class or literal constant (it may be `eval` under another name; outside a
project any such name counts).

The fix is offered only when every reference can be updated: a literal
lookup on an unresolvable receiver naming the component, a suggestion that
is taken by a sibling or that would itself break the convention, means no
fix. It is *safe* when nothing could refer to the component in a way
iglint cannot follow: no references at all, no script reads the
component's name (`meta.name`, `name`, `getName()`), no script looks up
children with computed names or has a string literal equal to the name,
every script parses, no script hands a component to code outside the view
(passing `self`, `self.parent`, `event.source` or another component to a
library function, in a message payload, to `getGlobals()` or to any call
iglint does not model): a library function that receives a component may
call `getChild('SaveButton')` itself; no string of the view that iglint
does not read as an expression or script holds a `{...}` reference (an
action's settings, a field of an unknown binding type); no reference is
written with whitespace inside its braces (`{ ../Label.props.text }`,
which Perspective reads as a tag path); no string of the view holds
the name, as a word, anywhere the fix does not update it; and the new name
occurs nowhere in the view: not in any string of the view (a broken
`{../NewName.props.text}` or binding path `../NewName.props.text` would
start to resolve) and not in any string literal of a script
(`self.getSibling('NewName')`, which finds nothing now, would find the
renamed component), in any spelling that differs only in case.
Perspective compares names exactly, but case variants count too, to stay
on the safe side. It is *unsafe* otherwise, and names the place that
keeps it from being safe. The safety net checks the result as well: a
safe fix that changes what any reference of the view resolves to (see
`docs/internal/fix-safety.md`, "Threat model") is reported as unsafe.

When a character of the name changes length under case mapping (the
ligature `ﬁ` upper-cases to `FI`, `ß` to `SS`), no suggestion and no fix
are offered and the message says why. Like every
fix, it is also unsafe while code in the workspace may read the files a
fix edits (any file access, read, write, rename or delete, whose path
is not a literal outside the workspace, or
code that is not vetted); the project setting `[fix]
assume-scripts-do-not-read-workspace-files = true` declares that no script
does (see `docs/internal/fix-safety.md`, "File reads").

Keys of `props` that the component's schema declares (`text`, `style`,
`pipes`, ...) are Ignition's names, not yours, and are never checked; only
free-form keys (for example the `params` passed to an embedded view) are.
Style objects, `position`, private names (starting with `_`), Java dates
and dataset internals are skipped.

## Why is this bad?

Consistent names make views readable and make references such as
`{../SaveButton.props.enabled}` predictable. Names with spaces or
punctuation are easy to mistype in property paths and scripts.

## Example

With `convention = "PascalCase"`:

```json
{"meta": {"name": "save button"}, "type": "ia.input.button"}
```

Use instead:

```json
{"meta": {"name": "SaveButton"}, "type": "ia.input.button"}
```

## Options

* `convention`: one of `PascalCase`, `camelCase`, `snake_case`,
  `kebab-case`, `SCREAMING_SNAKE_CASE`, `Title Case`, `lower case`. Unset
  means PascalCase without suggestions.
* `custom-pattern`: a regular expression (matched at the start of the name,
  like Python's `re.match`) that replaces the convention's pattern.
* `suggestion-convention`: the convention suggestions are written in, when
  it differs from `convention` (for example with a `custom-pattern`).
* `target-node-types`: node types to check: `component`, `property`,
  `message_handler`, `custom_method`. Defaults to the keys of `per-type`, or
  `["component"]`.
* `per-type`: a table from node type to overrides of `convention`,
  `pattern`, `pattern_description`, `suggestion_convention`, `min_length`,
  `max_length`, `forbidden_names`, `skip_names`, `severity` and
  `allow_numbers`.
* `allow-numbers`: whether convention patterns accept digits.
* `min-length`, `max-length`: length limits (characters).
* `forbidden-names`: names that are always reported.
* `skip-names`: names never checked; giving a list replaces the default
  `["root"]`.
* `allowed-abbreviations`: extra abbreviations; an abbreviation written in
  capitals as a whole word is accepted where the convention writes words
  differently (`read_API_data` in snake_case, `API Key` in Title Case).
* `auto-detect-abbreviations`: also use the built-in list (`API`, `HTTP`,
  `ID`, `URL`, ...).

## ign-lint

Equivalent of `NamePatternRule`; every kwarg maps to the option of the same
name (`node_type_specific_rules` is `per-type`) and `iglint ign-lint`
prints the same messages. Intended differences
(`docs/compat/ign-lint-differences.md`): abbreviations only ever widen what
is accepted (ign-lint's substring replacement rejects `idleTimer`,
`restartButton` or `urlParams` in camelCase and accepts any SCREAMING name
containing an abbreviation); suggestions keep digits and split words
consistently (`badEmbeddedPotato_0` becomes `BadEmbeddedPotato0`, not
`Badembeddedpotato0`); a component is never reported as colliding with its
own name; schema-defined `props` keys and propConfig metadata are not
properties; object-valued properties are checked too, not only leaves;
renames update references by resolution and are unsafe when scripts could
refer to the component dynamically.
"#,
    options: &[
        OptionMeta {
            name: "convention",
            ty: OptionType::String,
            default: "none",
            description: "Naming convention of all node types.",
        },
        OptionMeta {
            name: "custom-pattern",
            ty: OptionType::String,
            default: "none",
            description: "Regular expression replacing the convention's pattern.",
        },
        OptionMeta {
            name: "suggestion-convention",
            ty: OptionType::String,
            default: "none",
            description: "Convention used for suggestions.",
        },
        OptionMeta {
            name: "target-node-types",
            ty: OptionType::StringList,
            default: "none",
            description: "Node types to check.",
        },
        OptionMeta {
            name: "per-type",
            ty: OptionType::Table,
            default: "{}",
            description: "Per node type overrides.",
        },
        OptionMeta {
            name: "allow-numbers",
            ty: OptionType::Bool,
            default: "true",
            description: "Whether convention patterns accept digits.",
        },
        OptionMeta {
            name: "min-length",
            ty: OptionType::Integer,
            default: "1",
            description: "Minimum name length.",
        },
        OptionMeta {
            name: "max-length",
            ty: OptionType::Integer,
            default: "none",
            description: "Maximum name length.",
        },
        OptionMeta {
            name: "forbidden-names",
            ty: OptionType::StringList,
            default: "[]",
            description: "Names that are always reported.",
        },
        OptionMeta {
            name: "skip-names",
            ty: OptionType::StringList,
            default: "[\"root\"]",
            description: "Names never checked.",
        },
        OptionMeta {
            name: "allowed-abbreviations",
            ty: OptionType::StringList,
            default: "[]",
            description: "Extra abbreviations.",
        },
        OptionMeta {
            name: "auto-detect-abbreviations",
            ty: OptionType::Bool,
            default: "true",
            description: "Use the built-in abbreviation list.",
        },
    ],
    ign_lint_names: &["NamePatternRule"],
    ign_lint_option_map: &[
        ("convention", "convention"),
        ("custom_pattern", "custom-pattern"),
        ("suggestion_convention", "suggestion-convention"),
        ("target_node_types", "target-node-types"),
        ("node_type_specific_rules", "per-type"),
        ("allow_numbers", "allow-numbers"),
        ("min_length", "min-length"),
        ("max_length", "max-length"),
        ("forbidden_names", "forbidden-names"),
        ("skip_names", "skip-names"),
        ("allowed_abbreviations", "allowed-abbreviations"),
        ("auto_detect_abbreviations", "auto-detect-abbreviations"),
    ],
    ign_lint_implied_options: &[],
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    // The rename fix checks the names scripts call through the project
    // index (they may be frame builtins that look components up).
    phase: Phase::Project,
    hooks: &[Hook::View(&NamePattern)],
    options: Some(parse_options),
};

/// The kinds of names the rule checks (ign-lint's node type names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeType {
    Component,
    Property,
    MessageHandler,
    CustomMethod,
}

impl NodeType {
    const ALL: [NodeType; 4] = [
        NodeType::Component,
        NodeType::Property,
        NodeType::MessageHandler,
        NodeType::CustomMethod,
    ];

    fn name(self) -> &'static str {
        match self {
            NodeType::Component => "component",
            NodeType::Property => "property",
            NodeType::MessageHandler => "message_handler",
            NodeType::CustomMethod => "custom_method",
        }
    }

    fn parse(s: &str) -> Option<NodeType> {
        NodeType::ALL.into_iter().find(|t| t.name() == s)
    }
}

/// How suggestions are made for a node type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Suggestion {
    /// No suggestion is shown.
    None,
    /// A suggestion convention that is not known: ign-lint shows
    /// `No suggestion available`.
    Unknown,
    In(Convention),
}

/// The resolved settings of one node type.
#[derive(Debug)]
struct TypeRules {
    regex: Regex,
    description: String,
    /// Convention used to accept whole-word abbreviations.
    normalize: Option<Convention>,
    suggestion: Suggestion,
    allow_numbers: bool,
    min_length: usize,
    max_length: Option<usize>,
    forbidden: Vec<String>,
    skip: Vec<String>,
    severity: Option<Severity>,
}

/// Parsed options.
#[derive(Debug)]
pub struct Options {
    types: Vec<(NodeType, TypeRules)>,
    abbreviations: Vec<String>,
}

impl Options {
    fn rules(&self, t: NodeType) -> Option<&TypeRules> {
        self.types.iter().find(|(n, _)| *n == t).map(|(_, r)| r)
    }
}

fn table_get<'t>(table: &'t BTreeMap<String, OptionValue>, key: &str) -> Option<&'t OptionValue> {
    table.get(key).or_else(|| table.get(&key.replace('_', "-")))
}

fn string_list(value: &OptionValue, what: &str) -> Result<Vec<String>, String> {
    value
        .as_string_list()
        .map(|l| l.into_iter().map(str::to_owned).collect())
        .ok_or_else(|| format!("`{what}` must be an array of strings"))
}

fn length(value: &OptionValue, what: &str) -> Result<usize, String> {
    value
        .as_i64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| format!("`{what}` must be a non-negative integer"))
}

fn parse_options(o: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    let convention = o.opt_string("convention")?;
    let custom_pattern = o.opt_string("custom-pattern")?;
    let suggestion_convention = o.opt_string("suggestion-convention")?;
    let allow_numbers = o.bool("allow-numbers")?;
    let min_length = usize::try_from(o.int("min-length")?).unwrap_or(0);
    let max_length = o
        .opt_int("max-length")?
        .and_then(|n| usize::try_from(n).ok());
    let forbidden = o.string_list("forbidden-names")?;
    let skip = o.string_list("skip-names")?;
    let per_type = o.table("per-type")?;
    let mut abbreviations = o.string_list("allowed-abbreviations")?;
    if o.bool("auto-detect-abbreviations")? {
        abbreviations.extend(BUILTIN_ABBREVIATIONS.iter().map(|s| (*s).to_owned()));
    }

    let known = convention.as_deref().and_then(Convention::parse);
    let (global_pattern, global_description) = match (&custom_pattern, known) {
        (Some(p), _) => (p.clone(), format!("custom pattern: {p}")),
        (None, Some(c)) => (c.regex_source(allow_numbers), c.description().to_owned()),
        (None, None) => (
            Convention::Pascal.regex_source(allow_numbers),
            Convention::Pascal.description().to_owned(),
        ),
    };

    let mut overrides: BTreeMap<String, BTreeMap<String, OptionValue>> = BTreeMap::new();
    for (key, value) in &per_type {
        if NodeType::parse(key).is_none() {
            return Err(format!("unknown node type `{key}` in `per-type`"));
        }
        let Some(table) = value.as_table() else {
            return Err(format!("`per-type.{key}` must be a table"));
        };
        overrides.insert(key.clone(), table.clone());
    }

    let targets: Vec<NodeType> = match o.opt_string_list("target-node-types")? {
        Some(list) => {
            let mut out = Vec::new();
            for t in &list {
                out.push(NodeType::parse(t).ok_or_else(|| {
                    format!(
                        "unknown node type `{t}`; expected one of component, property, message_handler, custom_method"
                    )
                })?);
            }
            out
        }
        None => Vec::new(),
    };
    let targets = if !targets.is_empty() {
        targets
    } else if !overrides.is_empty() {
        overrides
            .keys()
            .filter_map(|k| NodeType::parse(k))
            .collect()
    } else {
        vec![NodeType::Component]
    };

    let mut types = Vec::new();
    for t in targets {
        let empty = BTreeMap::new();
        let ov = overrides.get(t.name()).unwrap_or(&empty);
        let get_str = |k: &str| -> Result<Option<String>, String> {
            match table_get(ov, k) {
                None => Ok(None),
                Some(v) => v
                    .as_str()
                    .map(|s| Some(s.to_owned()))
                    .ok_or_else(|| format!("`per-type.{}.{k}` must be a string", t.name())),
            }
        };
        let ov_convention = get_str("convention")?;
        let ov_pattern = get_str("pattern")?;
        let ov_description = get_str("pattern_description")?;
        let ov_suggestion = get_str("suggestion_convention")?;
        let ov_allow_numbers = match table_get(ov, "allow_numbers") {
            None => allow_numbers,
            Some(v) => v.as_bool().ok_or_else(|| {
                format!("`per-type.{}.allow_numbers` must be a boolean", t.name())
            })?,
        };
        let conv_name = ov_convention.clone().or_else(|| convention.clone());
        let conv = conv_name.as_deref().and_then(Convention::parse);
        let (pattern, description) = if let Some(p) = &ov_pattern {
            (
                p.clone(),
                ov_description
                    .clone()
                    .unwrap_or_else(|| format!("custom pattern: {p}")),
            )
        } else if let Some(c) = ov_convention.as_deref().and_then(Convention::parse) {
            (c.regex_source(ov_allow_numbers), c.description().to_owned())
        } else if ov_convention.is_some() {
            (
                Convention::Pascal.regex_source(ov_allow_numbers),
                Convention::Pascal.description().to_owned(),
            )
        } else {
            (global_pattern.clone(), global_description.clone())
        };
        let suggestion_name = ov_suggestion.or_else(|| suggestion_convention.clone());
        let suggestion = match (&suggestion_name, conv) {
            (Some(s), _) => Convention::parse(s).map_or(Suggestion::Unknown, Suggestion::In),
            (None, Some(c)) => Suggestion::In(c),
            (None, None) => Suggestion::None,
        };
        let normalize = match suggestion {
            Suggestion::In(c) if ov_pattern.is_some() || custom_pattern.is_some() => {
                conv.or(Some(c))
            }
            _ => conv,
        };
        let severity = match table_get(ov, "severity") {
            None => None,
            Some(v) => Some(
                v.as_str()
                    .and_then(|s| matches!(s, "warning" | "error").then(|| Severity::parse(s)))
                    .flatten()
                    .ok_or_else(|| {
                        format!(
                            "`per-type.{}.severity` must be \"warning\" or \"error\"",
                            t.name()
                        )
                    })?,
            ),
        };
        types.push((
            t,
            TypeRules {
                regex: compile(&pattern)?,
                description,
                normalize,
                suggestion,
                allow_numbers: ov_allow_numbers,
                min_length: match table_get(ov, "min_length") {
                    Some(v) => length(v, "min_length")?,
                    None => min_length,
                },
                max_length: match table_get(ov, "max_length") {
                    Some(v) => Some(length(v, "max_length")?),
                    None => max_length,
                },
                forbidden: match table_get(ov, "forbidden_names") {
                    Some(v) => string_list(v, "forbidden_names")?,
                    None => forbidden.clone(),
                },
                skip: match table_get(ov, "skip_names") {
                    Some(v) => string_list(v, "skip_names")?,
                    None => skip.clone(),
                },
                severity,
            },
        ));
    }
    Ok(Box::new(Options {
        types,
        abbreviations,
    }))
}

/// Why a name fails.
enum Failure {
    Forbidden,
    TooShort(usize),
    TooLong(usize),
    Pattern,
}

fn check_name(name: &str, rules: &TypeRules, abbreviations: &[String]) -> Option<Failure> {
    if rules.skip.iter().any(|s| s == name) {
        return None;
    }
    if rules.forbidden.iter().any(|s| s == name) {
        return Some(Failure::Forbidden);
    }
    let len = name.chars().count();
    if len < rules.min_length {
        return Some(Failure::TooShort(rules.min_length));
    }
    if let Some(max) = rules.max_length
        && max > 0
        && len > max
    {
        return Some(Failure::TooLong(max));
    }
    if matches_pattern(name, rules, abbreviations) {
        None
    } else {
        Some(Failure::Pattern)
    }
}

fn matches_pattern(name: &str, rules: &TypeRules, abbreviations: &[String]) -> bool {
    rules.regex.is_match(name)
        || rules
            .normalize
            .and_then(|c| normalize_abbreviations(name, c, abbreviations))
            .is_some_and(|n| rules.regex.is_match(&n))
}

/// One name to check.
struct Node<'a> {
    name: &'a str,
    file_range: TextRange,
    node_path: String,
    ign_lint_path: String,
}

struct NamePattern;

impl ViewRule for NamePattern {
    fn check_view(&self, cx: &mut Checker<'_, '_>, view: &View<'_>) {
        let options: &Options = cx.options();
        let file = cx.primary_file();
        if let Some(rules) = options.rules(NodeType::Component) {
            check_components(cx, view, file, rules, options);
        }
        if let Some(rules) = options.rules(NodeType::Property) {
            for node in properties(view) {
                report(cx, NodeType::Property, &node, rules, options, None);
            }
        }
        if let Some(rules) = options.rules(NodeType::MessageHandler) {
            for h in &view.message_handlers {
                let Some(t) = h.message_type else { continue };
                let node = Node {
                    name: t.value(),
                    file_range: t.range(),
                    node_path: h.node_path.clone(),
                    ign_lint_path: h.ign_lint_path.clone(),
                };
                report(cx, NodeType::MessageHandler, &node, rules, options, None);
            }
        }
        if let Some(rules) = options.rules(NodeType::CustomMethod) {
            for m in &view.custom_methods {
                let Some(n) = m.name else { continue };
                let node = Node {
                    name: n.value(),
                    file_range: n.range(),
                    node_path: m.node_path.clone(),
                    ign_lint_path: m.ign_lint_path.clone(),
                };
                report(cx, NodeType::CustomMethod, &node, rules, options, None);
            }
        }
    }
}

fn message(t: NodeType, node: &Node<'_>, failure: &Failure, rules: &TypeRules) -> String {
    let name = node.name;
    let ty = t.name();
    match failure {
        Failure::Forbidden => format!("Name '{name}' is forbidden for {ty}"),
        Failure::TooShort(n) => {
            format!("Name '{name}' is too short (minimum {n} characters) for {ty}")
        }
        Failure::TooLong(n) => {
            format!("Name '{name}' is too long (maximum {n} characters) for {ty}")
        }
        Failure::Pattern => format!(
            "Name '{name}' doesn't follow {} for {ty}",
            rules.description
        ),
    }
}

/// The suggestion for a failing name, if suggestions are configured and the
/// suggestion differs from the name.
fn suggestion_for(name: &str, rules: &TypeRules, options: &Options) -> Option<String> {
    match rules.suggestion {
        Suggestion::None => None,
        Suggestion::Unknown => Some("No suggestion available".to_owned()),
        Suggestion::In(_) if length_changing_char(name).is_some() => None,
        Suggestion::In(c) => {
            let s = suggest(name, c, &options.abbreviations, rules.allow_numbers);
            (!s.is_empty() && s != name).then_some(s)
        }
    }
}

/// The first character of `name` whose upper or lower case is more than
/// one character (the ligature `ﬁ` upper-cases to `FI`, `ß` to `SS`).
/// Converting such a name gives a suggestion that is not the name re-cased
/// (`ﬁle_name` would become `FIleName`), so none is offered (review round
/// 21, N3).
fn length_changing_char(name: &str) -> Option<char> {
    name.chars()
        .find(|c| c.to_uppercase().count() != 1 || c.to_lowercase().count() != 1)
}

/// Why no suggestion is offered for a name that fails the pattern, when
/// suggestions are configured.
fn no_suggestion_note(name: &str, rules: &TypeRules) -> Option<String> {
    if !matches!(rules.suggestion, Suggestion::In(_)) {
        return None;
    }
    length_changing_char(name).map(|c| {
        format!(" (no suggestion: changing the case of '{c}' changes its length; rename manually)")
    })
}

fn report(
    cx: &mut Checker<'_, '_>,
    t: NodeType,
    node: &Node<'_>,
    rules: &TypeRules,
    options: &Options,
    fix: Option<Fix>,
) {
    if node.name.is_empty() {
        return;
    }
    let Some(failure) = check_name(node.name, rules, &options.abbreviations) else {
        return;
    };
    let mut text = message(t, node, &failure, rules);
    if matches!(failure, Failure::Pattern)
        && let Some(s) = suggestion_for(node.name, rules, options)
    {
        text.push_str(&format!(" (suggestion: '{s}')"));
    }
    if matches!(failure, Failure::Pattern)
        && let Some(note) = no_suggestion_note(node.name, rules)
    {
        text.push_str(&note);
    }
    emit(cx, node, text, rules, fix);
}

fn emit(
    cx: &mut Checker<'_, '_>,
    node: &Node<'_>,
    text: String,
    rules: &TypeRules,
    fix: Option<Fix>,
) {
    let file = cx.primary_file();
    let severity = rules.severity.unwrap_or(cx.severity());
    cx.report_in(file, node.file_range, text)
        .node_path(node.node_path.clone())
        .ign_lint_path(Some(node.ign_lint_path.clone()))
        .severity(severity)
        .fix_opt(fix);
}

fn check_components<'v>(
    cx: &mut Checker<'_, '_>,
    view: &'v View<'v>,
    file: FileId,
    rules: &TypeRules,
    options: &Options,
) {
    let mut rename: Option<RenameContext> = None;
    let mut guards: Option<Guards<'v>> = None;
    // The names of the children of each parent, with how often each
    // occurs, computed once per parent.
    let mut sibling_names: rustc_hash::FxHashMap<u32, rustc_hash::FxHashMap<&str, u32>> =
        rustc_hash::FxHashMap::default();
    // Names claimed by earlier renames, per parent.
    let mut claimed: FxHashSet<(Option<u32>, String)> = FxHashSet::default();
    for c in &view.components {
        if c.name.is_empty() {
            continue;
        }
        let Some(failure) = check_name(c.name, rules, &options.abbreviations) else {
            continue;
        };
        let node = Node {
            name: c.name,
            file_range: c.name_range(),
            node_path: c.path.clone(),
            ign_lint_path: c.ign_lint_path.clone(),
        };
        let mut text = message(NodeType::Component, &node, &failure, rules);
        let suggestion = suggestion_for(c.name, rules, options);
        let names = c.parent.map(|p| {
            &*sibling_names.entry(p.0).or_insert_with(|| {
                let mut names = rustc_hash::FxHashMap::default();
                for child in view.children(p) {
                    *names.entry(child.name).or_insert(0u32) += 1;
                }
                names
            })
        });
        // Whether another child of the parent has the name `s`.
        let sibling_named = |s: &str| {
            names.is_some_and(|n| n.get(s).copied().unwrap_or(0) > u32::from(s == c.name))
        };
        let usable = suggestion.as_deref().filter(|s| {
            matches!(rules.suggestion, Suggestion::In(_))
                && !sibling_named(s)
                && check_name(s, rules, &options.abbreviations).is_none()
        });
        if let Some(s) = &suggestion
            && matches!(failure, Failure::Pattern)
        {
            if sibling_named(s) {
                text.push_str(&format!(
                    " (suggestion '{s}' already in use by a sibling; rename manually)"
                ));
            } else {
                text.push_str(&format!(" (suggestion: '{s}')"));
            }
        }
        if matches!(failure, Failure::Pattern)
            && let Some(note) = no_suggestion_note(c.name, rules)
        {
            text.push_str(&note);
        }
        let fix = usable.and_then(|new| {
            let key = (c.parent.map(|p| p.0), new.to_owned());
            if claimed.contains(&key) {
                return None;
            }
            let rename = rename.get_or_insert_with(|| RenameContext::new(cx, view));
            let fix = rename_fix(cx, view, c, file, new, rename, &mut guards)?;
            claimed.insert(key);
            Some(fix)
        });
        emit(cx, &node, text, rules, fix);
    }
}

/// What every rename of one view needs, computed once per view.
struct RenameContext {
    refs: References,
    /// The ranges of `refs.sites` of each component, in order.
    sites: rustc_hash::FxHashMap<iglint_project::ComponentId, Vec<TextRange>>,
    /// The first `refs.fragile` reference of each component.
    fragile: rustc_hash::FxHashMap<iglint_project::ComponentId, TextRange>,
    /// Why code that is not vetted keeps renames from being safe.
    vetting: Option<UnsafeReason>,
    /// A component of the view may be handed to code outside it.
    escape: bool,
    /// The words of the scripts' string literals by their lower-case form:
    /// each spelling with the literal's file range, in order.
    script_words: rustc_hash::FxHashMap<String, Vec<(String, TextRange)>>,
}

impl RenameContext {
    fn new(cx: &Checker<'_, '_>, view: &View<'_>) -> RenameContext {
        let refs = References::collect(view, cx.parsed_scripts());
        let mut sites: rustc_hash::FxHashMap<_, Vec<TextRange>> = Default::default();
        for site in &refs.sites {
            sites.entry(site.component).or_default().push(site.range);
        }
        let mut fragile = rustc_hash::FxHashMap::default();
        for (id, range) in &refs.fragile {
            fragile.entry(*id).or_insert(*range);
        }
        // Code that is not vetted may look the component up in ways
        // iglint does not know (`cPickle.loads` of an `eval` opcode).
        // Closed world: such code anywhere in the shared runtime counts.
        let vetting = crate::rules::vetting::unit_reason(cx);
        let mut script_words: rustc_hash::FxHashMap<String, Vec<(String, TextRange)>> =
            Default::default();
        for (text, range) in &refs.script_strings {
            for word in text
                .split(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
                .filter(|w| !w.is_empty())
            {
                script_words
                    .entry(word.to_lowercase())
                    .or_default()
                    .push((word.to_owned(), *range));
            }
        }
        RenameContext {
            script_words,
            sites,
            fragile,
            vetting,
            escape: components_may_escape(cx, view),
            refs,
        }
    }
}

fn rename_fix<'v>(
    cx: &Checker<'_, '_>,
    view: &'v View<'v>,
    c: &Component<'_>,
    file: FileId,
    new: &str,
    rename: &RenameContext,
    guards: &mut Option<Guards<'v>>,
) -> Option<Fix> {
    let refs = &rename.refs;
    if refs.blocked.contains(c.name) {
        return None;
    }
    let name_value = c.name_value?;
    let editor = cx.json_editor(file, view.root)?;
    let style = editor.escape_style();
    let mut edits = vec![editor.replace_with_string(name_value, new)];
    let mut count = 0;
    let mut sites = FxHashSet::default();
    for &range in rename.sites.get(&c.id).map_or(&[][..], Vec::as_slice) {
        edits.push(TextEdit::replace(range, escaped(new, style)));
        sites.insert(range);
        count += 1;
    }
    let vetting = rename.vetting.clone();
    let guards = guards.get_or_insert_with(|| Guards::collect(cx, view));
    let observed = guards.observed(cx, file, c, rename, &sites);
    let message = format!("Rename component '{}' to '{new}'", c.name);
    if let Some(reason) = guards.new_name_used(cx, file, c, new, rename) {
        let edits = cx.file_edits(file, edits);
        return Some(
            Fix::unsafe_edit(message, edits, reason.message.clone()).because(Some(reason)),
        );
    }
    let risk = if count > 0 {
        Some(
            "the rename also rewrites references to the component in scripts and bindings, \
             which code iglint does not see may name too",
        )
    } else if rename.escape {
        Some("a component of the view is handed to code outside it, which may look it up by name")
    } else if refs.meta_name_read.contains(&c.id) || refs.dynamic_meta_name {
        Some("a script reads the component's `meta.name`")
    } else if refs.dynamic_lookup {
        Some("a script looks up components by a computed name")
    } else if refs.literals.contains(c.name) {
        Some("the name appears in a string literal, which may be used to look the component up")
    } else if vetting.is_some() || observed.is_some() {
        Some("code may observe the component's name")
    } else {
        None
    };
    let edits = cx.file_edits(file, edits);
    Some(match risk {
        Some(reason) => Fix::unsafe_edit(message, edits, reason).because(vetting.or(observed)),
        None => Fix::safe(message, edits),
    })
}

/// What can read a component's name, or name a component, in ways the
/// rename does not update, for every rename of one view: name reads of the
/// scripts, and the strings of the view iglint does not read as code.
struct Guards<'v> {
    /// Reads of a component's name (`self.name`, `c.getName()` for a
    /// child `c`, measured members of the component script object): the
    /// component read (`AnyComponent` when it cannot be determined) and
    /// the file range.
    name_reads: Vec<(AccessTarget, TextRange)>,
    /// Every JSON string (values and keys) of the view except the script
    /// bodies (analysed as scripts) and the components' own names.
    strings: Vec<&'v JsonString<'v>>,
    /// The first `{...}` reference-like token in a string that is neither
    /// a modelled expression nor a property binding path: Perspective may
    /// read it as a reference iglint does not resolve.
    token: Option<(TextRange, String)>,
    /// Position in `name_reads` of the first read of any component.
    first_any_read: Option<usize>,
    /// Position in `name_reads` of the first read of each component.
    first_read: rustc_hash::FxHashMap<iglint_project::ComponentId, usize>,
    /// Every identifier of `strings` (maximal runs of letters, digits and
    /// `_`) with the string and offset of each occurrence, in order: the
    /// occurrences of a name made of such characters, built on first use.
    words: std::cell::OnceCell<rustc_hash::FxHashMap<&'v str, Vec<(u32, u32)>>>,
    /// The distinct words of `words` by their lower-case form, built on
    /// first use.
    folded: std::cell::OnceCell<rustc_hash::FxHashMap<String, Vec<&'v str>>>,
}

impl<'v> Guards<'v> {
    fn collect(cx: &Checker<'_, '_>, view: &'v View<'v>) -> Guards<'v> {
        let (accesses, _) = cx.script_property_accesses();
        let name_reads: Vec<(AccessTarget, TextRange)> = accesses
            .iter()
            .filter(|a| {
                a.category == PropCategory::Meta
                    && a.read
                    && a.name.as_deref().is_none_or(|n| n == "name")
            })
            .map(|a| (a.target, a.range))
            .collect();
        let mut skip: FxHashSet<TextRange> = FxHashSet::default();
        for s in &view.scripts {
            if let ScriptBody::Json(body) = &s.body {
                skip.insert(body.range());
            }
        }
        for comp in &view.components {
            if let Some(ValueKind::String(n)) = comp.name_value.map(|v| &v.kind) {
                skip.insert(n.range());
            }
        }
        let mut parsed: FxHashSet<TextRange> =
            view.expressions.iter().map(|e| e.source.range()).collect();
        for b in &view.bindings {
            if let BindingConfig::Property(p) = &b.typed
                && let Some(path) = p.path
            {
                parsed.insert(path.range());
            }
        }
        let mut strings = Vec::new();
        collect_strings(view.root, &mut strings);
        strings.retain(|s| !skip.contains(&s.range()));
        let token = strings
            .iter()
            .filter(|s| !parsed.contains(&s.range()))
            .find_map(|s| {
                reference_like_token(s.value()).map(|r| (s.raw_range(r), s.value().to_owned()))
            });
        let mut first_any_read = None;
        let mut first_read = rustc_hash::FxHashMap::default();
        for (i, (target, _)) in name_reads.iter().enumerate() {
            match *target {
                AccessTarget::AnyComponent => {
                    first_any_read.get_or_insert(i);
                }
                AccessTarget::Component(id) => {
                    first_read.entry(id).or_insert(i);
                }
                _ => {}
            }
        }
        Guards {
            name_reads,
            strings,
            token,
            first_any_read,
            first_read,
            words: std::cell::OnceCell::new(),
            folded: std::cell::OnceCell::new(),
        }
    }

    /// [`Guards::words`].
    fn words(&self) -> &rustc_hash::FxHashMap<&'v str, Vec<(u32, u32)>> {
        self.words.get_or_init(|| {
            let mut words: rustc_hash::FxHashMap<&'v str, Vec<(u32, u32)>> = Default::default();
            for (i, s) in self.strings.iter().enumerate() {
                let text: &'v str = s.value();
                let mut start = None;
                for (at, ch) in text
                    .char_indices()
                    .chain(std::iter::once((text.len(), ' ')))
                {
                    let ident = at < text.len() && (ch.is_alphanumeric() || ch == '_');
                    match (ident, start) {
                        (true, None) => start = Some(at),
                        (false, Some(from)) => {
                            // Both ends come from `char_indices` (or the length).
                            #[allow(clippy::string_slice)]
                            let word = &text[from..at];
                            words.entry(word).or_default().push((i as u32, from as u32));
                            start = None;
                        }
                        _ => {}
                    }
                }
            }
            words
        })
    }

    /// Why renaming `c` to `new` may make something of the view find the
    /// component that did not find it before (review round 21, B1): the
    /// guards of [`Guards::observed`] look for the old name, this looks for
    /// the new one. Perspective compares names with `String.equals`, but
    /// any spelling of `new` that differs only in case counts too: a
    /// string of the view (expressions, binding paths and every other
    /// value or key) or a string literal of a script (lookup arguments
    /// included, resolved or not) holding it as a word. Occurrences of the
    /// old name itself are left to the old name's guards (a case-only
    /// rename rewrites them).
    fn new_name_used(
        &self,
        cx: &Checker<'_, '_>,
        file: FileId,
        c: &Component<'_>,
        new: &str,
        rename: &RenameContext,
    ) -> Option<UnsafeReason> {
        const WHY: &str = "the new name already occurs in the view, where the rename could \
                           make a reference or lookup that finds nothing now find the component";
        let target = new.to_lowercase();
        let is_ident =
            |s: &str| !s.is_empty() && s.chars().all(|ch| ch.is_alphanumeric() || ch == '_');
        if is_ident(new) {
            let folded = self.folded.get_or_init(|| {
                let mut folded: rustc_hash::FxHashMap<String, Vec<&'v str>> = Default::default();
                for word in self.words().keys() {
                    folded.entry(word.to_lowercase()).or_default().push(word);
                }
                folded
            });
            let first = folded
                .get(&target)
                .into_iter()
                .flatten()
                .filter(|w| **w != c.name)
                .flat_map(|w| self.words()[w].iter().map(move |o| (*o, w.len())))
                .min();
            if let Some(((i, at), len)) = first {
                let s = self.strings[i as usize];
                let range = s.raw_range(TextRange::at(
                    TextSize::from(at),
                    TextSize::from(len as u32),
                ));
                return Some(reason_at(cx, file, range, WHY));
            }
            if let Some(range) = rename.script_words.get(&target).and_then(|hits| {
                hits.iter()
                    .find(|(word, _)| word != c.name)
                    .map(|(_, range)| *range)
            }) {
                return Some(reason_at(cx, file, range, WHY));
            }
            return None;
        }
        // A name with other characters (`Title Case`): any string holding
        // it in any case, beyond the occurrences of the old name.
        let old = (c.name.to_lowercase() == target).then_some(c.name);
        let holds = |text: &str| {
            text.to_lowercase().matches(&target).count()
                > old.map_or(0, |o| text.matches(o).count())
        };
        if let Some(s) = self.strings.iter().find(|s| holds(s.value())) {
            return Some(reason_at(cx, file, s.range(), WHY));
        }
        if let Some((_, range)) = rename.refs.script_strings.iter().find(|(t, _)| holds(t)) {
            return Some(reason_at(cx, file, *range, WHY));
        }
        None
    }

    /// Why renaming `c` may change what the view does although the edits
    /// update every reference iglint resolves (`sites`): a script reads
    /// its name, a reference Perspective reads as a tag path names it, a
    /// string iglint does not read may hold a reference, or a string holds
    /// its name where the fix does not update it.
    fn observed(
        &self,
        cx: &Checker<'_, '_>,
        file: FileId,
        c: &Component<'_>,
        rename: &RenameContext,
        sites: &FxHashSet<TextRange>,
    ) -> Option<UnsafeReason> {
        let first = [self.first_any_read, self.first_read.get(&c.id).copied()]
            .into_iter()
            .flatten()
            .min();
        if let Some((_, range)) = first.map(|i| &self.name_reads[i]) {
            return Some(reason_at(
                cx,
                file,
                *range,
                "a script reads the component's name, which the rename changes",
            ));
        }
        if let Some(range) = rename.fragile.get(&c.id) {
            return Some(reason_at(
                cx,
                file,
                *range,
                "a reference with whitespace inside its braces names the component; \
                 Perspective reads it as a tag path",
            ));
        }
        if let Some((range, _)) = &self.token {
            return Some(reason_at(
                cx,
                file,
                *range,
                "a string iglint does not read as an expression holds a `{...}` reference",
            ));
        }
        // A name made of identifier characters occurs as a word only as a
        // whole identifier: look it up instead of searching every string.
        if !c.name.is_empty() && c.name.chars().all(|ch| ch.is_alphanumeric() || ch == '_') {
            for &(i, at) in self.words().get(c.name).map_or(&[][..], Vec::as_slice) {
                let s = self.strings[i as usize];
                let range = s.raw_range(TextRange::at(TextSize::from(at), TextSize::of(c.name)));
                if !sites.contains(&range) {
                    return Some(reason_at(
                        cx,
                        file,
                        range,
                        "a string of the view holds the component's name where the rename \
                         does not update it",
                    ));
                }
            }
            return None;
        }
        for s in &self.strings {
            for (i, _) in s.value().match_indices(c.name) {
                if !is_word(s.value(), i, c.name.len()) {
                    continue;
                }
                let range = s.raw_range(TextRange::at(
                    TextSize::from(i as u32),
                    TextSize::of(c.name),
                ));
                if !sites.contains(&range) {
                    return Some(reason_at(
                        cx,
                        file,
                        range,
                        "a string of the view holds the component's name where the rename \
                         does not update it",
                    ));
                }
            }
        }
        None
    }
}

/// Every JSON string of `v`: member keys and string values.
fn collect_strings<'v>(v: &'v Value<'v>, out: &mut Vec<&'v JsonString<'v>>) {
    match &v.kind {
        ValueKind::String(s) => out.push(s),
        ValueKind::Object(o) => {
            for m in o.iter() {
                out.push(&m.key);
                collect_strings(&m.value, out);
            }
        }
        ValueKind::Array(a) => {
            for item in a.iter() {
                collect_strings(item, out);
            }
        }
        _ => {}
    }
}

/// Whether `text[at..at + len]` is not part of a longer identifier.
fn is_word(text: &str, at: usize, len: usize) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let before = text.get(..at).unwrap_or_default();
    let after = text.get(at + len..).unwrap_or_default();
    !before.chars().next_back().is_some_and(ident) && !after.chars().next().is_some_and(ident)
}

/// The first `{...}` in `text` whose content, without surrounding
/// whitespace, starts like a Perspective property reference: `this`,
/// `view`, `parent`, `page` or `session` followed by `.`, `[` or nothing;
/// a relative (`.`) or absolute (`/`) path; or `Name.props`, `.custom`,
/// `.meta`, `.position`, `.params`. The range of the braces and content.
fn reference_like_token(text: &str) -> Option<TextRange> {
    let mut from = 0;
    // `from` and `open + 1` are just past an ASCII brace found in `text`.
    while let Some(open) = text.get(from..)?.find('{').map(|i| i + from) {
        let rest = text.get(open + 1..)?;
        let close = rest.find(['{', '}'])?;
        if rest.as_bytes()[close] == b'}' && looks_like_reference(rest.get(..close)?) {
            return Some(TextRange::new(
                TextSize::from(open as u32),
                TextSize::from((open + close + 2) as u32),
            ));
        }
        from = open + 1 + close;
    }
    None
}

fn looks_like_reference(content: &str) -> bool {
    let t = content.trim();
    if t.starts_with('.') || t.starts_with('/') {
        return true;
    }
    for head in ["this", "view", "parent", "page", "session"] {
        if let Some(rest) = t.strip_prefix(head)
            && (rest.is_empty() || rest.starts_with(['.', '[']))
        {
            return true;
        }
    }
    let Some((first, rest)) = t.split_once('.') else {
        return false;
    };
    !first.is_empty()
        && ["props", "custom", "meta", "position", "params"]
            .iter()
            .any(|ty| {
                rest == *ty
                    || rest.starts_with(&format!("{ty}."))
                    || rest.starts_with(&format!("{ty}["))
            })
}

/// An [`UnsafeReason`] naming `range` of the view's file.
fn reason_at(cx: &Checker<'_, '_>, file: FileId, range: TextRange, message: &str) -> UnsafeReason {
    let site = cx.file(file).map(|source| {
        let at = source
            .line_index()
            .line_column(range.start(), source.text());
        ReasonSite {
            path: source.path().to_path_buf(),
            line: at.line,
            column: at.column,
            what: crate::script_facts::access::snippet(source.text(), range),
        }
    });
    UnsafeReason {
        message: message.to_owned(),
        site,
        workspace: false,
    }
}

/// Whether a script of the view may hand a component to code outside the
/// view (`util.initialise(self)`, a message payload, `getGlobals()`),
/// which can then look up children by name; or the scripts were not all
/// followed.
fn components_may_escape(cx: &Checker<'_, '_>, view: &View<'_>) -> bool {
    let (accesses, complete) = cx.script_property_accesses();
    // Names the scripts call that may be frame builtins reach every
    // component.
    let accesses =
        crate::rules::view::unused_custom_property::resolve_callee_checks(cx, view, accesses);
    !complete
        || accesses.iter().any(|a| {
            a.target == crate::script_facts::AccessTarget::AnyComponent
                && a.name.is_none()
                && a.read
                && a.write
        })
}

// ---- properties ----

/// Path substrings ign-lint never treats as property names.
const SKIPPED: &[&str] = &[
    ".style.",
    ".elementStyle.",
    ".textStyle.",
    ".instanceStyle.",
    ".position.",
    ".meta.",
    ".binding.",
    ".scripts.",
    ".events.",
];

fn properties<'v>(view: &'v View<'v>) -> Vec<Node<'v>> {
    let mut out = Vec::new();
    let mut seen: FxHashSet<String> = FxHashSet::default();
    let mut walker = PropertyWalker {
        out: &mut out,
        seen: &mut seen,
    };
    for (category, value) in [("custom", view.custom), ("params", view.params)] {
        if let Some(value) = value {
            walker.walk(value, category, &format!("view.{category}"), None);
        }
    }
    for c in &view.components {
        let ign = strip_indices(&c.ign_lint_path);
        if let Some(custom) = c.custom {
            walker.walk(
                custom,
                &format!("{ign}.custom"),
                &format!("{}.custom", c.path),
                None,
            );
        }
        if let (Some(props), Some(ct)) = (c.props, c.component_type()) {
            walker.walk(
                props,
                &format!("{ign}.props"),
                &format!("{}.props", c.path),
                Some(SchemaContext {
                    ty: ct,
                    path: String::new(),
                }),
            );
        }
    }
    // Properties that exist only through their propConfig entry.
    for pc in &view.prop_configs {
        if !matches!(pc.path.category, "custom" | "params") {
            continue;
        }
        if pc.path.category == "params" && pc.owner != Owner::View {
            continue;
        }
        let key = pc.key.value();
        let ign = match pc.owner {
            Owner::View => strip_indices(key),
            Owner::Component(c) => {
                format!(
                    "{}.{}",
                    strip_indices(&view.component(c).ign_lint_path),
                    strip_indices(key)
                )
            }
            _ => continue,
        };
        let last = key.rsplit('.').next().unwrap_or(key);
        let name = last.split('[').next().unwrap_or(last);
        if name.is_empty() || name.starts_with('_') || !seen.insert(ign.clone()) {
            continue;
        }
        if SKIPPED.iter().any(|s| format!("{ign}.").contains(s)) {
            continue;
        }
        let start = key.len() - last.len();
        let name_range = TextRange::at(
            iglint_text::TextSize::from(start),
            iglint_text::TextSize::of(name),
        );
        out.push(Node {
            name,
            file_range: pc.key.raw_range(name_range),
            node_path: format!("{}.{key}", view.owner_path(pc.owner)),
            ign_lint_path: ign,
        });
    }
    out
}

#[derive(Clone)]
struct SchemaContext {
    ty: &'static iglint_stubs::ComponentType,
    /// Path below `props`, e.g. `params` or `columns[0]`.
    path: String,
}

struct PropertyWalker<'o, 'v> {
    out: &'o mut Vec<Node<'v>>,
    seen: &'o mut FxHashSet<String>,
}

impl<'v> PropertyWalker<'_, 'v> {
    fn walk(&mut self, value: &'v Value<'v>, ign: &str, node: &str, schema: Option<SchemaContext>) {
        match &value.kind {
            ValueKind::Object(object) => {
                if is_java_date(value) || is_dataset(value) {
                    return;
                }
                for member in &object.members {
                    let key = member.key();
                    let child_ign = format!("{ign}.{key}");
                    let probe = format!("{child_ign}.");
                    if SKIPPED.iter().any(|s| probe.contains(s))
                        || child_ign.contains(".props.aspectRatio")
                        || (key == "d" && child_ign.contains(".props.elements"))
                    {
                        continue;
                    }
                    let child_node = format!("{node}.{key}");
                    let (declared, child_schema) = match &schema {
                        Some(s) => {
                            let path = if s.path.is_empty() {
                                key.to_owned()
                            } else {
                                format!("{}.{key}", s.path)
                            };
                            let declared = s.ty.prop(&path).is_some();
                            (
                                declared,
                                declared.then_some(SchemaContext { ty: s.ty, path }),
                            )
                        }
                        None => (false, None),
                    };
                    if !declared
                        && !key.is_empty()
                        && !key.starts_with('_')
                        && self.seen.insert(child_ign.clone())
                    {
                        self.out.push(Node {
                            name: key,
                            file_range: member.key.range(),
                            node_path: child_node.clone(),
                            ign_lint_path: child_ign.clone(),
                        });
                    }
                    self.walk(&member.value, &child_ign, &child_node, child_schema);
                }
            }
            ValueKind::Array(array) => {
                for (i, element) in array.iter().enumerate() {
                    let child_schema = schema.as_ref().map(|s| SchemaContext {
                        ty: s.ty,
                        path: format!("{}[{i}]", s.path),
                    });
                    self.walk(element, ign, &format!("{node}[{i}]"), child_schema);
                }
            }
            _ => {}
        }
    }
}

/// `{"$": ["ts", ...], "$ts": n}`: a Java date as Ignition stores it.
fn is_java_date(value: &Value<'_>) -> bool {
    let Some(o) = value.as_object() else {
        return false;
    };
    o.len() == 2
        && o.get("$ts").is_some()
        && o.get("$")
            .and_then(Value::as_array)
            .and_then(|a| a.get(0))
            .and_then(Value::as_str)
            == Some("ts")
}

/// `{"$": ["ds", ...], "$columns": [...], "rows": [...]}`: a dataset.
fn is_dataset(value: &Value<'_>) -> bool {
    value
        .get("$")
        .and_then(Value::as_array)
        .and_then(|a| a.get(0))
        .and_then(Value::as_str)
        == Some("ds")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::test_support::*;

    fn component(name: &str, ty: &str, extra: &str) -> String {
        format!(r#"{{"meta": {{"name": "{name}"}}, "type": "{ty}"{extra}}}"#)
    }

    fn root(children: &[String]) -> String {
        format!(
            r#"{{"custom": {{}}, "params": {{}}, "root": {{"meta": {{"name": "root"}}, "type": "ia.container.flex", "children": [{}]}}}}"#,
            children.join(", ")
        )
    }

    #[test]
    fn default_checks_components_against_pascal_case() {
        let f = Fixture::view(&root(&[
            component("saveButton", "ia.input.button", ""),
            component("Title", "ia.display.label", ""),
            component("Data Table", "ia.display.table", ""),
        ]));
        let d = f.lint(&["NAME001"]);
        assert_eq!(
            messages(&d),
            vec![
                (
                    "NAME001".into(),
                    "Name 'saveButton' doesn't follow PascalCase for component".into()
                ),
                (
                    "NAME001".into(),
                    "Name 'Data Table' doesn't follow PascalCase for component".into()
                ),
            ]
        );
        assert_eq!(flagged(&d), vec!["\"saveButton\"", "\"Data Table\""]);
        assert_eq!(d[0].node_path.as_deref(), Some("root/saveButton"));
        assert_eq!(
            d[0].ign_lint_path.as_deref(),
            Some("root.root.children[0].saveButton")
        );
        assert!(
            d.iter().all(|d| d.fix.is_none()),
            "no convention, no suggestion"
        );
    }

    #[test]
    fn camel_case_with_safe_rename() {
        let view = root(&[component("SaveButton", "ia.input.button", "")]);
        let f = Fixture::view(&view);
        let rules = [("NAME001", "convention = \"camelCase\"")];
        let d = f.lint_with(&rules);
        assert_eq!(
            d[0].message,
            "Name 'SaveButton' doesn't follow camelCase for component (suggestion: 'saveButton')"
        );
        assert_eq!(
            d[0].fix.as_ref().unwrap().applicability,
            crate::Applicability::Safe
        );
        let fixed = f.fixed(&rules, false, &f.view_path("Main"));
        assert_eq!(fixed, view.replace("\"SaveButton\"", "\"saveButton\""));
    }

    #[test]
    fn abbreviations_never_cause_findings() {
        let f = Fixture::view(&root(&[
            component("idleTimer", "ia.display.label", ""),
            component("restartButton", "ia.input.button", ""),
            component("urlParams", "ia.display.label", ""),
            component("myAPIButton", "ia.input.button", ""),
        ]));
        assert!(
            f.lint_with(&[("NAME001", "convention = \"camelCase\"")])
                .is_empty()
        );
        // ...and do not hide real violations.
        let d = f.lint_with(&[("NAME001", "convention = \"SCREAMING_SNAKE_CASE\"")]);
        assert_eq!(d.len(), 4);
        let d = f.lint_with(&[("NAME001", "convention = \"PascalCase\"")]);
        assert_eq!(d.len(), 4, "idleTimer is not PascalCase");
    }

    #[test]
    fn whole_word_abbreviations_widen_snake_and_title_case() {
        let f = Fixture::view(&root(&[
            component("read_API_data", "ia.display.label", ""),
            component("read_Api_data", "ia.display.label", ""),
        ]));
        let d = f.lint_with(&[("NAME001", "convention = \"snake_case\"")]);
        assert_eq!(flagged(&d), vec!["\"read_Api_data\""]);
        let d = f.lint_with(&[(
            "NAME001",
            "convention = \"snake_case\"\nauto-detect-abbreviations = false",
        )]);
        assert_eq!(d.len(), 2);
        let f = Fixture::view(&root(&[component("API Key", "ia.display.label", "")]));
        assert!(
            f.lint_with(&[("NAME001", "convention = \"Title Case\"")])
                .is_empty()
        );
    }

    #[test]
    fn suggestions_keep_digits_and_split_consistently() {
        let f = Fixture::view(&root(&[
            component("pump_12", "ia.display.label", ""),
            component("HTTPServer", "ia.display.label", ""),
        ]));
        let d = f.lint_with(&[("NAME001", "convention = \"camelCase\"")]);
        assert_eq!(
            messages(&d).into_iter().map(|m| m.1).collect::<Vec<_>>(),
            vec![
                "Name 'pump_12' doesn't follow camelCase for component (suggestion: 'pump12')",
                "Name 'HTTPServer' doesn't follow camelCase for component (suggestion: 'httpServer')",
            ]
        );
    }

    /// Review round 21, N3: a name with a character whose case mapping
    /// changes length gets no suggestion and no fix, and the message says
    /// why.
    #[test]
    fn no_suggestion_when_case_mapping_changes_length() {
        let f = Fixture::view(&root(&[
            component("\u{fb01}le_name", "ia.display.label", ""),
            component("stra\u{df}e", "ia.display.label", ""),
        ]));
        let d = f.lint_with(&[(
            "NAME001",
            "convention = \"PascalCase\"\ntarget-node-types = [\"component\"]\nskip-names = [\"root\"]",
        )]);
        assert_eq!(
            d.iter().map(|d| d.message.as_str()).collect::<Vec<_>>(),
            [
                "Name '\u{fb01}le_name' doesn't follow PascalCase for component (no suggestion: changing the case of '\u{fb01}' changes its length; rename manually)",
                "Name 'stra\u{df}e' doesn't follow PascalCase for component (no suggestion: changing the case of '\u{df}' changes its length; rename manually)",
            ]
        );
        assert!(d.iter().all(|d| d.fix.is_none()));
    }

    #[test]
    fn sibling_collision_but_never_with_itself() {
        let f = Fixture::view(&root(&[
            component("save_button", "ia.input.button", ""),
            component("SaveButton", "ia.input.button", ""),
        ]));
        let d = f.lint_with(&[("NAME001", "convention = \"PascalCase\"")]);
        assert_eq!(
            d[0].message,
            "Name 'save_button' doesn't follow PascalCase for component (suggestion 'SaveButton' already in use by a sibling; rename manually)"
        );
        assert!(d[0].fix.is_none());
        assert_eq!(d.len(), 1);
    }

    /// Review round 21, B1: renaming to a name that broken references
    /// already use would make them resolve. The new name gets the guards of
    /// the old one: any occurrence of it (in any case) in the view's
    /// strings or the scripts' literals makes the rename unsafe, naming
    /// the site; `--fix` without `--unsafe-fixes` leaves the view as it is.
    #[test]
    fn rename_to_a_name_that_broken_references_use_is_unsafe() {
        const LOOKUP: &str = r#", "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\tlbl = self.getSibling('MyLabel')\n\tif lbl is None:\n\t\tprint 'missing'\n\telse:\n\t\tprint 'found'"}}}}"#;
        const BINDING: &str = r#", "propConfig": {"props.text": {"binding": {"type": "property", "config": {"path": "../MyLabel.props.text"}}}}"#;
        const EXPRESSION: &str = r#", "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "coalesce({/root/MyLabel.props.text}, 'none')"}}}}"#;
        let rules = [(
            "NAME001",
            "convention = \"PascalCase\"\ntarget-node-types = [\"component\"]",
        )];
        let cases = [
            (
                "my_label",
                vec![
                    component("Btn", "ia.input.button", LOOKUP),
                    component("Other", "ia.display.label", BINDING),
                ],
                "`MyLabel` at",
            ),
            (
                "my_label",
                vec![component("Other", "ia.display.label", EXPRESSION)],
                "`MyLabel` at",
            ),
            (
                "myLabel",
                vec![component("Btn", "ia.input.button", LOOKUP)],
                "`'MyLabel'` at",
            ),
            // Case variants count too.
            (
                "my_label",
                vec![component(
                    "Other",
                    "ia.display.label",
                    &BINDING.replace("MyLabel", "MYLABEL"),
                )],
                "`MYLABEL` at",
            ),
        ];
        for (old, others, site) in cases {
            let mut children = vec![component(old, "ia.display.label", "")];
            children.extend(others);
            let view = root(&children);
            let f = Fixture::view(&view);
            let d = f.lint_with(&rules);
            let d = d
                .iter()
                .find(|d| d.message.contains(&format!("'{old}'")))
                .unwrap_or_else(|| panic!("{old}: {d:?}"));
            let fix = d.fix.as_ref().expect("a fix");
            assert_eq!(fix.applicability, crate::Applicability::Unsafe, "{view}");
            let reason = fix
                .unsafe_reason
                .as_ref()
                .unwrap()
                .describe(|p| p.display().to_string());
            assert!(
                reason.contains("the new name already occurs in the view") && reason.contains(site),
                "{reason}"
            );
            let result = f.run(
                &rules,
                crate::FixMode::DiffOnly {
                    unsafe_fixes: false,
                },
            );
            assert!(result.fixed.iter().all(|s| !s.changed()), "{view}");
        }
    }

    /// Review round 21, B1: on generated views with random names and
    /// references (expressions, property bindings, script lookups, broken
    /// ones included), every fix reported safe leaves the resolution map
    /// of the view as it was (renamed components keep every reference),
    /// alone and all together; and some fixes are safe.
    #[test]
    fn safe_fixes_keep_every_resolution() {
        const NAMES: [&str; 8] = ["a_b", "AB", "ab", "Ab", "x_y", "XY", "Xy", "c_d"];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |n: usize| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((seed >> 33) as usize) % n
        };
        let rules = [(
            "NAME001",
            "convention = \"PascalCase\"\ntarget-node-types = [\"component\"]\nskip-names = [\"root\"]",
        )];
        let mut safe = 0;
        for _ in 0..60 {
            let mut pick = || NAMES[next(NAMES.len())];
            let (p, q) = (pick(), pick());
            let (r, t) = (pick(), pick());
            let (u, v, w) = (pick(), pick(), pick());
            let inner = component(
                q,
                "ia.display.label",
                &format!(
                    r#", "propConfig": {{"props.text": {{"binding": {{"type": "expr", "config": {{"expression": "{{../{r}.props.text}} + {{/root/{p}/{t}.props.text}}"}}}}}}}}"#
                ),
            );
            let other = component(r, "ia.display.label", "");
            let panel = format!(
                r#"{{"meta": {{"name": "{p}"}}, "type": "ia.container.flex", "children": [{inner}, {other}]}}"#
            );
            let script = format!(
                r#"\tx = self.getSibling('{u}')\n\ty = self.parent.getChild('{p}/{v}')\n\tprint x, y"#
            );
            let button = component(
                w,
                "ia.input.button",
                &format!(
                    r#", "propConfig": {{"props.enabled": {{"binding": {{"type": "property", "config": {{"path": "../{p}/{q}.props.visible"}}}}}}}}, "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": "{script}"}}}}}}}}"#
                ),
            );
            let view = root(&[panel, button, component(pick(), "ia.display.label", "")]);
            let f = Fixture::view(&view);
            let path = f.view_path("Main");
            let text = std::fs::read_to_string(&path).unwrap();
            let before = crate::safety::Resolutions::of(&path, &text).unwrap();
            let d = f.lint_with(&rules);
            let mut all = Vec::new();
            for d in &d {
                let Some(fix) = d.fix.as_ref() else { continue };
                if fix.applicability != crate::Applicability::Safe {
                    continue;
                }
                safe += 1;
                let edits: Vec<TextEdit> = fix.edits.iter().map(|e| e.edit.clone()).collect();
                all.extend(edits.iter().cloned());
                let after = iglint_text::apply_edits(&text, &edits).unwrap();
                let after = crate::safety::Resolutions::of(&path, &after).unwrap();
                assert_eq!(before.difference(&after), None, "{}\n{view}", fix.message);
            }
            let result = f.run(
                &rules,
                crate::FixMode::DiffOnly {
                    unsafe_fixes: false,
                },
            );
            if let Some(fixed) = result.fixed.iter().find(|s| s.path == path) {
                let after = crate::safety::Resolutions::of(&path, &fixed.fixed).unwrap();
                assert_eq!(before.difference(&after), None, "{view}");
            }
        }
        assert!(safe > 0);
    }

    #[test]
    fn rename_updates_every_resolved_reference() {
        let label = component(
            "TheLabel",
            "ia.display.label",
            r#", "props": {"text": "x"}"#,
        );
        let other_label = component("TheLabel", "ia.display.label", "");
        let panel = format!(
            r#"{{"meta": {{"name": "Panel"}}, "type": "ia.container.flex", "children": [{other_label}]}}"#
        );
        let button = component(
            "Btn",
            "ia.input.button",
            r#", "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "{../TheLabel.props.text} + {/root/Panel/TheLabel.props.text}"}}}, "props.enabled": {"binding": {"type": "property", "config": {"path": "../TheLabel.props.visible"}}}}, "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\tself.getSibling(\"TheLabel\").props.text = 'a'\n\tself.parent.getChild(\"Panel/TheLabel\").props.text = 'b'"}}}}"#,
        );
        let view = root(&[label, panel, button]);
        let f = Fixture::view(&view);
        let rules = [(
            "NAME001",
            "convention = \"camelCase\"\nskip-names = [\"root\", \"Panel\", \"Btn\"]",
        )];
        let d = f.lint_with(&rules);
        assert_eq!(d.len(), 2);
        assert!(
            d.iter()
                .all(|d| d.fix.as_ref().unwrap().applicability == crate::Applicability::Unsafe)
        );
        let fixed = f.fixed(&rules, true, &f.view_path("Main"));
        let expected = view
            .replace(
                r#""meta": {"name": "TheLabel"}"#,
                r#""meta": {"name": "theLabel"}"#,
            )
            .replace("{../TheLabel.", "{../theLabel.")
            .replace("{/root/Panel/TheLabel.", "{/root/Panel/theLabel.")
            .replace(
                "\"../TheLabel.props.visible\"",
                "\"../theLabel.props.visible\"",
            )
            .replace(r#"getSibling(\"TheLabel\")"#, r#"getSibling(\"theLabel\")"#)
            .replace(
                r#"getChild(\"Panel/TheLabel\")"#,
                r#"getChild(\"Panel/theLabel\")"#,
            );
        assert_eq!(fixed, expected);
        // Safe fixes alone change nothing.
        assert!(
            f.run(
                &rules,
                crate::FixMode::DiffOnly {
                    unsafe_fixes: false
                }
            )
            .fixed
            .iter()
            .all(|s| !s.changed())
        );
    }

    #[test]
    fn unresolvable_lookups_block_the_rename() {
        let button = component(
            "Btn",
            "ia.input.button",
            r#", "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\tthing = system.util.getGlobals()['x']\n\tthing.getChild('the_label')"}}}}"#,
        );
        let f = Fixture::view(&root(&[
            component("the_label", "ia.display.label", ""),
            button,
        ]));
        let d = f.lint_with(&[("NAME001", "convention = \"PascalCase\"")]);
        assert_eq!(flagged(&d), vec!["\"the_label\""]);
        assert!(d[0].fix.is_none());
    }

    /// Dynamic code that can look a component up by a name it builds:
    /// `eval` under a library name, the builtins module, a library function
    /// whose code is replaced (it receives `self`), `getattr` with a
    /// computed name, `exec`. Each makes the rename unsafe; a call of a
    /// plain library function does not.
    #[test]
    fn dynamic_code_makes_the_rename_unsafe() {
        let util = "ev = eval\ndef handle(p):\n    pass\ndef _real(p):\n    p.parent.getChild('the_label')\nhandle.func_code = _real.func_code\n";
        let applicability = |script: &str| {
            let button = component(
                "Btn",
                "ia.input.button",
                &format!(
                    r#", "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": "{script}"}}}}}}}}"#
                ),
            );
            let f = Fixture::view(&root(&[
                component("the_label", "ia.display.label", ""),
                button,
            ]))
            .resource(
                "projects/p/ignition/script-python/lib/util",
                &[("code.py", util)],
            )
            .resource(
                "projects/p/ignition/script-python/lib/plain",
                &[("code.py", "def log(m):\n    pass\n")],
            )
            .resource(
                "projects/p/ignition/script-python/lib/unvetted",
                &[("code.py", "import warnings\ndef log(m):\n    pass\n")],
            );
            let d = f.lint_with(&[("NAME001", "convention = \"PascalCase\"")]);
            let d = d
                .iter()
                .find(|d| d.message.contains("the_label"))
                .unwrap_or_else(|| panic!("{script}: {d:?}"));
            d.fix.as_ref().map(|f| f.applicability)
        };
        for script in [
            "\\tlib.util.ev('self.parent.getChild(n)')",
            "\\timport __builtin__\\n\\t__builtin__.eval(s)",
            "\\tlib.util.handle(self)",
            "\\tgetattr(self.parent, n)('x')",
            "\\texec s",
            "\\tfrom __builtin__ import locals as l\\n\\tl()",
            // Code that is not vetted (reviewer repro R6-B): `cPickle`
            // runs `eval` in the caller's frame; a library module that
            // imports something unvetted.
            "\\timport cPickle\\n\\tc = cPickle.loads(\\\"c__builtin__\\\\neval\\\\n(S'self.parent.getChild(\\\\\\\"the_\\\\\\\"+\\\\\\\"label\\\\\\\")'\\\\ntR.\\\")",
            "\\tlib.unvetted.log('x')",
        ] {
            assert_eq!(
                applicability(script),
                Some(crate::Applicability::Unsafe),
                "{script}"
            );
        }
        // `lib/util` and `lib/unvetted` give the workspace a frame reader,
        // so the closed-world check makes even a clean-callee rename unsafe
        // (reviewer round 7, H3): a global reader could grab the component
        // through `sys._current_frames`.
        assert_eq!(
            applicability("\\tlib.plain.log('x')"),
            Some(crate::Applicability::Unsafe)
        );
        // With no unknown code in the workspace the rename is safe.
        let clean = Fixture::view(&root(&[
            component("the_label", "ia.display.label", ""),
            component(
                "Btn",
                "ia.input.button",
                r#", "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\tlib.plain.log('x')"}}}}"#,
            ),
        ]))
        .resource(
            "projects/p/ignition/script-python/lib/plain",
            &[("code.py", "def log(m):\n    pass\n")],
        );
        let d = clean.lint_with(&[("NAME001", "convention = \"PascalCase\"")]);
        let d = d.iter().find(|d| d.message.contains("the_label")).unwrap();
        assert_eq!(
            d.fix.as_ref().map(|f| f.applicability),
            Some(crate::Applicability::Safe)
        );
    }

    /// A library function that reaches the caller's frame can get `self`
    /// and look a component up by name, even when the script hands it no
    /// component. It is not "plain", so the rename is unsafe. (Reviewer
    /// repro R5-C.)
    #[test]
    fn frame_reading_library_function_blocks_rename() {
        let applicability = |logger: &str| {
            let button = component(
                "Btn",
                "ia.input.button",
                r#", "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\tlib.log.capture('x')"}}}}"#,
            );
            let f = Fixture::view(&root(&[
                component("the_label", "ia.display.label", ""),
                button,
            ]))
            .resource(
                "projects/p/ignition/script-python/lib/log",
                &[("code.py", logger)],
            );
            let d = f.lint_with(&[("NAME001", "convention = \"PascalCase\"")]);
            let d = d
                .iter()
                .find(|d| d.message.contains("the_label"))
                .unwrap_or_else(|| panic!("{logger}: {d:?}"));
            d.fix.as_ref().map(|f| f.applicability)
        };
        for logger in [
            "import sys\ndef capture(m):\n    try:\n        raise ValueError(m)\n    except ValueError:\n        tb = sys.exc_info()[2]\n    fr = getattr(getattr(tb, 'tb_frame'), 'f_back')\n    me = getattr(fr, 'f_locals')['self']\n    me.getSibling('the_label')\n",
            "import sys\ndef capture(m):\n    tb = sys.exc_info()[2]\n    return getattr(tb, 'tb_' + 'frame')\n",
            "from org.python.core import Py\ndef capture(m):\n    return getattr(Py.getFrame(), 'f_back')\n",
        ] {
            assert_eq!(
                applicability(logger),
                Some(crate::Applicability::Unsafe),
                "{logger}"
            );
        }
    }

    /// The component name is observable through `meta.name` reached any
    /// way, not only as a direct `self.meta.name` chain. (Reviewer repro
    /// R5-G2.)
    #[test]
    fn meta_read_through_alias_or_subscript_is_unsafe() {
        for script in [
            // Alias of `.meta`, then `.name`.
            "\\tm = self.meta\\n\\tself.props.text = m.name",
            // Subscript of `.meta`.
            "\\tself.props.text = self.meta[\\u0027name\\u0027]",
            // getattr on `.meta`.
            "\\tself.props.text = getattr(self.meta, \\u0027name\\u0027)",
            // Another member of `.meta` reaches meta data too.
            "\\tx = self.meta.tooltip",
            // `.meta` passed to code.
            "\\tfoo(self.meta)",
        ] {
            let button = component(
                "bad name",
                "ia.input.button",
                &format!(
                    r#", "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": "{script}"}}}}}}}}"#
                ),
            );
            let f = Fixture::view(&root(&[button]));
            let d = f.lint_with(&[("NAME001", "convention = \"PascalCase\"")]);
            let d = d
                .iter()
                .find(|d| d.message.contains("bad name"))
                .unwrap_or_else(|| panic!("{script}: {d:?}"));
            assert_eq!(
                d.fix.as_ref().unwrap().applicability,
                crate::Applicability::Unsafe,
                "{script}"
            );
        }
        // A plain `self.meta.name` read still blocks only the read
        // component, and an unrelated script does not block a rename.
        let safe = component("bad name", "ia.input.button", "");
        let f = Fixture::view(&root(&[safe]));
        let d = f.lint_with(&[("NAME001", "convention = \"PascalCase\"")]);
        assert_eq!(
            d[0].fix.as_ref().unwrap().applicability,
            crate::Applicability::Safe
        );
    }

    #[test]
    fn literal_name_in_script_makes_the_rename_unsafe() {
        let button = component(
            "Btn",
            "ia.input.button",
            r#", "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\tfor c in self.parent.children:\n\t\tif c.meta.name == 'the_label':\n\t\t\tpass"}}}}"#,
        );
        let f = Fixture::view(&root(&[
            component("the_label", "ia.display.label", ""),
            button,
        ]));
        let d = f.lint_with(&[("NAME001", "convention = \"PascalCase\"")]);
        assert_eq!(
            d[0].fix.as_ref().unwrap().applicability,
            crate::Applicability::Unsafe
        );
    }

    #[test]
    fn properties_are_user_chosen_keys_only() {
        let embedded = component(
            "Embed",
            "ia.display.view",
            r#", "props": {"path": "Other", "params": {"Bad_Param": 1, "goodParam": {"Inner_Key": 2}}, "style": {"Bad_Style": 1}}, "custom": {"Bad_Custom": 1, "_private": 2}, "propConfig": {"custom.Bound_Only": {"binding": {"type": "expr", "config": {"expression": "1"}}}}"#,
        );
        let view = format!(
            r#"{{"custom": {{"Top_Level": {{"Nested_Key": 1}}, "rows": [{{"Row_Key": 1}}, {{"Row_Key": 2}}]}}, "params": {{"Param_One": 1}}, "propConfig": {{"custom.Config_Only": {{"persistent": false}}}}, "root": {{"meta": {{"name": "root"}}, "type": "ia.container.flex", "children": [{embedded}]}}}}"#
        );
        let f = Fixture::view(&view);
        let d = f.lint_with(&[(
            "NAME001",
            "target-node-types = [\"property\"]\nconvention = \"camelCase\"",
        )]);
        let mut names: Vec<String> = flagged(&d)
            .into_iter()
            .map(|n| n.trim_matches('"').to_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "Bad_Custom",
                "Bad_Param",
                "Bound_Only",
                "Config_Only",
                "Inner_Key",
                "Nested_Key",
                "Param_One",
                "Row_Key",
                "Top_Level",
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
        );
        let paths: Vec<&str> = d
            .iter()
            .filter_map(|d| d.ign_lint_path.as_deref())
            .collect();
        assert!(paths.contains(&"custom.rows.Row_Key"));
        assert!(paths.contains(&"root.root.children.Embed.props.params.Bad_Param"));
        assert!(
            d.iter().all(|d| d.fix.is_none()),
            "only components are renamed"
        );
    }

    #[test]
    fn handlers_methods_and_per_type_settings() {
        let comp = component(
            "Comp",
            "ia.display.label",
            r#", "scripts": {"customMethods": [{"name": "do_thing", "params": [], "script": "\tpass"}, {"name": "go", "params": [], "script": "\tpass"}], "messageHandlers": [{"messageType": "refreshAll", "script": "\tpass", "pageScope": true, "sessionScope": false, "viewScope": false}]}"#,
        );
        let f = Fixture::view(&root(&[comp]));
        let d = f.lint_with(&[(
            "NAME001",
            r#"[per-type.custom_method]
convention = "camelCase"
min_length = 3
severity = "error"
[per-type.message_handler]
convention = "kebab-case"
"#,
        )]);
        assert_eq!(
            d.iter()
                .map(|d| (d.severity, d.message.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (
                    Severity::Error,
                    "Name 'do_thing' doesn't follow camelCase for custom_method (suggestion: 'doThing')"
                ),
                (
                    Severity::Error,
                    "Name 'go' is too short (minimum 3 characters) for custom_method"
                ),
                (
                    Severity::Warning,
                    "Name 'refreshAll' doesn't follow kebab-case for message_handler (suggestion: 'refresh-all')"
                ),
            ]
        );
    }

    #[test]
    fn lengths_forbidden_skip_and_custom_patterns() {
        let f = Fixture::view(&root(&[
            component("Button", "ia.input.button", ""),
            component("Ab", "ia.display.label", ""),
            component("VeryLongComponentName", "ia.display.label", ""),
            component("btnSave", "ia.input.button", ""),
            component("Legacy_Name", "ia.display.label", ""),
        ]));
        let d = f.lint_with(&[(
            "NAME001",
            r#"custom-pattern = "^(btn|lbl)[A-Z][a-zA-Z0-9]*$"
suggestion-convention = "PascalCase"
min-length = 3
max-length = 12
forbidden-names = ["Button"]
skip-names = ["root", "Legacy_Name"]"#,
        )]);
        assert_eq!(
            messages(&d).into_iter().map(|m| m.1).collect::<Vec<_>>(),
            vec![
                "Name 'Button' is forbidden for component",
                "Name 'Ab' is too short (minimum 3 characters) for component",
                "Name 'VeryLongComponentName' is too long (maximum 12 characters) for component",
            ]
        );
        // A suggestion that the custom pattern would reject is never applied.
        let f = Fixture::view(&root(&[component("Save", "ia.input.button", "")]));
        let d = f.lint_with(&[(
            "NAME001",
            "custom-pattern = \"^btn\"\nsuggestion-convention = \"PascalCase\"",
        )]);
        assert_eq!(
            d[0].message,
            "Name 'Save' doesn't follow custom pattern: ^btn for component"
        );
        assert!(d[0].fix.is_none());
    }

    #[test]
    fn allow_numbers() {
        let f = Fixture::view(&root(&[component("Pump12", "ia.display.label", "")]));
        assert!(
            f.lint_with(&[("NAME001", "convention = \"PascalCase\"")])
                .is_empty()
        );
        let d = f.lint_with(&[(
            "NAME001",
            "convention = \"PascalCase\"\nallow-numbers = false",
        )]);
        assert_eq!(
            d[0].message,
            "Name 'Pump12' doesn't follow PascalCase for component (suggestion: 'Pump')"
        );
    }

    #[test]
    fn option_errors() {
        let meta = &META;
        let parse = |toml: &str| {
            let values = options_from_toml(toml);
            parse_options(&OptionReader::new(meta, &values)).map(|_| ())
        };
        assert!(parse("").is_ok());
        assert!(
            parse("target-node-types = [\"widget\"]")
                .unwrap_err()
                .contains("widget")
        );
        assert!(parse("[per-type.widget]\nconvention = \"camelCase\"").is_err());
        assert!(parse("custom-pattern = \"(\"").is_err());
        assert!(parse("[per-type.component]\nseverity = \"info\"").is_err());
    }

    /// A component handed to a library function can be looked up there by
    /// name: the rename is unsafe (the library is not updated).
    #[test]
    fn components_passed_out_of_the_view_make_the_rename_unsafe() {
        let rules = [("NAME001", "convention = \"camelCase\"")];
        for script in [
            r#"\tutil.initialise(self)"#,
            r#"\tsystem.util.getGlobals()['c'] = self.rootContainer"#,
            r#"\tsystem.perspective.sendMessage('m', payload={'c': self})"#,
        ] {
            let root_json = format!(
                r#"{{"custom": {{}}, "params": {{}}, "events": {{"system": {{"onStartup": {{"type": "script", "scope": "G", "config": {{"script": "{script}"}}}}}}}}, "root": {{"meta": {{"name": "root"}}, "type": "ia.container.flex", "children": [{}]}}}}"#,
                component("SaveButton", "ia.input.button", "")
            );
            let f = crate::rules::script::testing::with_library(
                Fixture::view(&root_json),
                "util",
                "def initialise(component):\n    component.getChild(\"SaveButton\").props.text = \"Ready\"\n",
            );
            let d = f.lint_with(&rules);
            assert_eq!(
                d[0].fix.as_ref().map(|f| f.applicability),
                Some(crate::Applicability::Unsafe),
                "{script}"
            );
        }
    }

    /// Reviewer round 9 (Opus) blocker: another view reaches this view's
    /// components through the page (`v.getChild('root/submitButton')`,
    /// with a literal or computed path): the rename in view `A` is unsafe,
    /// a script of `B` that stays on its own page and session is not.
    #[test]
    fn other_views_through_the_page_make_the_rename_unsafe() {
        let rules = [("NAME001", "convention = \"PascalCase\"")];
        let applicability = |script: &str| {
            let a = root(&[component("submitButton", "ia.input.button", "")]);
            let b = root(&[component(
                "Btn",
                "ia.input.button",
                &format!(
                    r#", "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": "{script}"}}}}}}}}"#
                ),
            )]);
            let f = Fixture::new().with_view("A", &a).with_view("B", &b);
            let d = f.lint_with(&rules);
            let d: Vec<_> = d
                .into_iter()
                .filter(|d| d.message.contains("submitButton"))
                .collect();
            assert_eq!(d.len(), 1, "{d:?}");
            d[0].fix.as_ref().map(|f| f.applicability)
        };
        for script in [
            r"\tfor v in self.page.views:\n\t\tv.getChild('root/submitButton').props.text = 'x'",
            r"\tn = 'submit' + 'Button'\n\tfor v in self.page.getViews():\n\t\tv.getChild('root/' + n)",
            r"\tself.page.view.rootContainer.getChild('submitButton')",
        ] {
            assert_eq!(
                applicability(script),
                Some(crate::Applicability::Unsafe),
                "{script}"
            );
        }
        assert_eq!(
            applicability(r"\tself.page.props.path\n\tself.getSibling('Btn').props.text = 'x'"),
            Some(crate::Applicability::Safe)
        );
    }

    /// Reviewer round 18 (Opus) blocker B1: a third-party
    /// `system.perspective.runJavaScriptAsync` call, an expression function
    /// the gateway does not register and a component that runs JavaScript
    /// in the browser make the rename unsafe; JavaScript in the page can
    /// look the component up by its name.
    #[test]
    fn unknown_functions_and_browser_code_make_the_rename_unsafe() {
        let rules = [("NAME001", "convention = \"PascalCase\"")];
        let applicability = |b: &str| {
            let a = root(&[component("submitButton", "ia.input.button", "")]);
            let f = Fixture::new().with_view("A", &a).with_view("B", b);
            let d = f.lint_with(&rules);
            let d: Vec<_> = d
                .into_iter()
                .filter(|d| d.message.contains("submitButton"))
                .collect();
            assert_eq!(d.len(), 1, "{d:?}");
            d[0].fix.as_ref().map(|f| f.applicability)
        };
        let button = |script: &str| {
            root(&[component(
                "Btn",
                "ia.input.button",
                &format!(
                    r#", "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": "{script}"}}}}}}}}"#
                ),
            )])
        };
        for b in [
            button(r"\tsystem.perspective.runJavaScriptAsync('x')"),
            button(r"\tsystem.perspective.navigate(url=self.props.text)"),
            root(&[component("Frame", "ia.display.iframe", "")]),
            root(&[component(
                "Draw",
                "ia.shapes.svg",
                r#", "props": {"elements": [{"type": "script", "text": "x"}]}"#,
            )]),
            root(&[component("J", "embr.periscope.embedding.javascript", "")]),
            root(&[component(
                "Lbl",
                "ia.display.label",
                r#", "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "objectScript('x')"}}}}"#,
            )]),
        ] {
            assert_eq!(applicability(&b), Some(crate::Applicability::Unsafe), "{b}");
        }
        assert_eq!(
            applicability(&button(r"\tsystem.perspective.navigate('/page')")),
            Some(crate::Applicability::Safe)
        );
    }

    /// The NAME001 finding for component `name` in `view`, and its fix.
    fn rename_of(view: &str, name: &str) -> (Option<crate::Applicability>, Option<String>) {
        let f = Fixture::view(view);
        let d = f.lint_with(&[("NAME001", "convention = \"PascalCase\"")]);
        let d = d
            .iter()
            .find(|d| d.message.contains(&format!("'{name}'")))
            .unwrap_or_else(|| panic!("{name}: {d:?}"));
        let fix = d.fix.as_ref();
        (
            fix.map(|f| f.applicability),
            fix.and_then(|f| f.unsafe_reason.as_ref())
                .map(|r| r.describe(|p| p.display().to_string())),
        )
    }

    fn with_bindings(bindings: &str) -> String {
        root(&[
            component("start_date", "ia.input.date-time-input", ""),
            component("api_token", "ia.input.text-field", ""),
            component(
                "Chart",
                "ia.display.label",
                &format!(r#", "propConfig": {{{bindings}}}"#),
            ),
        ])
    }

    const HISTORY: &str = r#""props.text": {"binding": {"type": "tag-history", "config": {"dateRange": {"startDate": "{../start_date.props.value}", "endDate": "now()"}, "tags": [{"path": "[default]T"}], "returnSize": {"type": "raw"}}}}"#;
    const HTTP: &str = r#""props.style": {"binding": {"type": "http", "config": {"request": {"url": "'https://example.com'", "method": "GET", "header": [{"key": "Authorization", "value": "'Bearer ' + {../api_token.props.text}"}]}}}}"#;

    /// Reviewer round 10, O1: references in binding fields iglint did not
    /// model (tag history date ranges, HTTP header values) are updated by
    /// the rename, which is therefore unsafe, never a safe rename that
    /// leaves them dangling.
    #[test]
    fn references_in_measured_binding_fields_are_renamed() {
        let view = with_bindings(&format!("{HISTORY}, {HTTP}"));
        assert_eq!(
            rename_of(&view, "start_date").0,
            Some(crate::Applicability::Unsafe)
        );
        assert_eq!(
            rename_of(&view, "api_token").0,
            Some(crate::Applicability::Unsafe)
        );
        let f = Fixture::view(&view);
        let rules = [("NAME001", "convention = \"PascalCase\"")];
        let fixed = f.fixed(&rules, true, &f.view_path("Main"));
        assert!(fixed.contains("{../StartDate.props.value}"), "{fixed}");
        assert!(fixed.contains("{../APIToken.props.text}"), "{fixed}");
        assert!(!fixed.contains("start_date") && !fixed.contains("api_token"));
    }

    /// Reviewer round 10, O1 (c): a `{...}` reference in a string iglint
    /// does not read as an expression (an action's settings), or the
    /// component's name in any string the fix does not update, makes the
    /// rename unsafe and names the place.
    #[test]
    fn unread_strings_make_the_rename_unsafe() {
        let action = |config: &str| {
            root(&[
                component("start_date", "ia.input.date-time-input", ""),
                component(
                    "Btn",
                    "ia.input.button",
                    &format!(
                        r#", "events": {{"component": {{"onActionPerformed": {{"type": "navigate", "scope": "C", "config": {config}}}}}}}"#
                    ),
                ),
            ])
        };
        let (applicability, reason) = rename_of(
            &action(r#"{"url": "/x?d={../start_date.props.value}"}"#),
            "start_date",
        );
        assert_eq!(applicability, Some(crate::Applicability::Unsafe));
        let reason = reason.unwrap();
        assert!(
            reason.contains("does not read as an expression"),
            "{reason}"
        );
        assert!(reason.contains("{../start_date.props.value}"), "{reason}");
        let (applicability, reason) =
            rename_of(&action(r#"{"page": "/start_date"}"#), "start_date");
        assert_eq!(applicability, Some(crate::Applicability::Unsafe));
        assert!(reason.unwrap().contains("holds the component's name"));
        // Braces that are no reference, and another word containing the
        // name, keep the rename safe.
        let (applicability, _) = rename_of(
            &action(r#"{"page": "/x/{id}/my_start_dates", "text": "{not a reference}"}"#),
            "start_date",
        );
        assert_eq!(applicability, Some(crate::Applicability::Safe));
    }

    /// Reviewer round 10, O2: reading a component's name through the
    /// script object's `name` / `getName()` (measured members), directly,
    /// through `children`, `getChildren()`, a loop or an index, blocks the
    /// safe rename.
    #[test]
    fn name_reads_through_the_script_object_make_the_rename_unsafe() {
        let view = |script: &str| {
            format!(
                r#"{{"custom": {{"selected": "", "target": "x"}}, "params": {{}}, "root": {{"meta": {{"name": "root"}}, "type": "ia.container.flex", "children": [{}, {}, {}]}}}}"#,
                component("pump_a", "ia.input.button", ""),
                component(
                    "PumpB",
                    "ia.input.button",
                    &format!(
                        r#", "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": "{script}"}}}}}}}}"#
                    ),
                ),
                component("my_label", "ia.display.label", ""),
            )
        };
        let (applicability, reason) = rename_of(
            &view(r"\tself.view.custom.selected = self.parent.getChild('pump_a').name"),
            "pump_a",
        );
        assert_eq!(applicability, Some(crate::Applicability::Unsafe));
        assert!(reason.unwrap().contains("reads the component's name"));
        for script in [
            r"\tfor c in self.parent.getChildren():\n\t\tif c.name == self.view.custom.target:\n\t\t\tc.props.text = 'x'",
            r"\tself.view.custom.selected = self.parent.getChildren()[0].name",
            r"\tfor c in self.parent.children:\n\t\tself.view.custom.selected = c.name",
            r"\tself.view.custom.selected = self.parent.getChildren()[2].getName()",
        ] {
            let (applicability, reason) = rename_of(&view(script), "my_label");
            assert_eq!(
                applicability,
                Some(crate::Applicability::Unsafe),
                "{script}"
            );
            assert!(
                reason
                    .as_deref()
                    .is_some_and(|r| r.contains("reads the component's name")),
                "{script}: {reason:?}"
            );
        }
        // `self.name` in a script of `pump_a` itself.
        let own = root(&[component(
            "pump_a",
            "ia.input.button",
            r#", "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\tself.view.custom.selected = self.name"}}}}"#,
        )]);
        let (applicability, reason) = rename_of(&own, "pump_a");
        assert_eq!(applicability, Some(crate::Applicability::Unsafe));
        assert!(reason.unwrap().contains("`name`"));
        let own = own.replace("self.name", "self.getName()");
        assert_eq!(
            rename_of(&own, "pump_a").0,
            Some(crate::Applicability::Unsafe)
        );
        // Reading another known component's name does not block this one.
        let other = view(r"\tself.view.custom.selected = self.name");
        assert_eq!(
            rename_of(&other, "my_label").0,
            Some(crate::Applicability::Safe)
        );
    }

    /// Reviewer round 10, O3: `{ ../pump_a.props.text }` is a tag path to
    /// Perspective; renaming `pump_a` cannot be proven not to change it.
    #[test]
    fn references_with_spaces_inside_the_braces_make_the_rename_unsafe() {
        let view = root(&[
            component("pump_a", "ia.input.button", ""),
            component(
                "Label",
                "ia.display.label",
                r#", "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "{ ../pump_a.props.text }"}}}}"#,
            ),
        ]);
        let (applicability, reason) = rename_of(&view, "pump_a");
        assert_eq!(applicability, Some(crate::Applicability::Unsafe));
        assert!(reason.unwrap().contains("whitespace inside its braces"));
    }
}
