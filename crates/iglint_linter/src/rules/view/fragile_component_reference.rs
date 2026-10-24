//! VIEW018 fragile-component-reference.

use iglint_expression::{PropertyTarget, Reference, parse_property_ref};
use iglint_project::view::{BindingConfig, BindingKind, View};
use iglint_project::{ExpressionKind, FileId, ModelData};
use iglint_python::ast::Expr;
use iglint_python::visitor::{Visitor, walk_expr};
use iglint_text::{TextRange, TextSize};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::options::OptionReader;
use crate::registry::{Category, OptionMeta, OptionType, RuleMeta};
use crate::rule::{Hook, ParsedOptions, Phase, RuleImpl, ViewRule};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW018",
    name: "fragile-component-reference",
    category: Category::View,
    default_severity: Severity::Warning,
    default_enabled: false,
    summary: "Script or binding navigates the component tree",
    explanation: r#"## What it does

Reports scripts and bindings that reach other components by walking the
component tree:

* in scripts: calls of `getSibling`, `getParent`, `getChild` and
  `getChildren`, and `self.parent` / `self.children`;
* in expression bindings, expression transforms, expression-structure
  bindings and property bindings: relative component paths `../Sibling...`
  and `./Child...`.

Scripts are parsed with the Jython parser and expressions with the
expression parser, so text in comments and string literals never counts,
and `../` is not also reported as `./`. One finding per script or binding
lists every pattern it uses.

## Why is this bad?

Such references break silently when components are renamed, moved or
wrapped in a new container. View custom properties, view parameters and
message handlers keep components decoupled. This is a style rule and is not
in the default set.

## Example

```python
self.getSibling('Total').props.text = str(value)
```

Use instead: bind `Total.props.text` to `{view.custom.total}` and write
`self.view.custom.total = value`.

## Options

* `forbidden-patterns`: the patterns to report, in ign-lint's notation:
  `.getSibling(`, `.getParent(`, `.getChild(`, `.getChildren(`,
  `self.parent.` (and the other `self.parent` forms), `self.children.`
  (and its other forms), `../` (paths going up) and `./` (paths relative to
  the component itself). Other strings are matched as plain text in the
  script or expression. An empty list means the default list (all of
  these).
* `case-sensitive` (default `true`): when false, method and attribute names
  and text patterns match case-insensitively.

## ign-lint

Equivalent of `BadComponentReferenceRule` (`forbidden_patterns`,
`case_sensitive`) with the same messages in `iglint ign-lint`. ign-lint
matches substrings: it also reports text in comments, strings and invalid
expressions, and every `../` also counts as `./` (bug 18); iglint does not.
"#,
    options: &[
        OptionMeta {
            name: "forbidden-patterns",
            ty: OptionType::StringList,
            default: r#"[".getSibling(", ".getParent(", ".getChild(", ".getChildren(", "self.parent.", "self.children.", "self.parent)", "self.children)", "self.parent,", "self.children,", "self.parent\n", "self.children\n", "self.parent\r", "self.children\r", "../", "./"]"#,
            description: "Patterns to report.",
        },
        OptionMeta {
            name: "case-sensitive",
            ty: OptionType::Bool,
            default: "true",
            description: "Match names case-sensitively.",
        },
    ],
    ign_lint_names: &["BadComponentReferenceRule"],
    ign_lint_option_map: &[
        ("forbidden_patterns", "forbidden-patterns"),
        ("case_sensitive", "case-sensitive"),
    ],
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::View(&FragileReference)],
    options: Some(parse_options),
};

const DEFAULT_PATTERNS: [&str; 16] = [
    ".getSibling(",
    ".getParent(",
    ".getChild(",
    ".getChildren(",
    "self.parent.",
    "self.children.",
    "self.parent)",
    "self.children)",
    "self.parent,",
    "self.children,",
    "self.parent\n",
    "self.children\n",
    "self.parent\r",
    "self.children\r",
    "../",
    "./",
];

struct Options {
    patterns: Vec<String>,
    case_sensitive: bool,
}

fn parse_options(o: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    let mut patterns = o.string_list("forbidden-patterns")?;
    if patterns.is_empty() {
        patterns = DEFAULT_PATTERNS.iter().map(|p| (*p).to_owned()).collect();
    }
    Ok(Box::new(Options {
        patterns,
        case_sensitive: o.bool("case-sensitive")?,
    }))
}

/// What a pattern detects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Detector<'p> {
    /// `.name(`: a call of a method `name`.
    Method(&'p str),
    /// `self.parent` / `self.children` followed by `next` (`None`: any).
    SelfAttribute(&'p str, Option<char>),
    /// `../`: a relative path going up.
    Up,
    /// `./`: a path relative to the component itself.
    Here,
    /// Any other text.
    Text(&'p str),
}

fn detector(pattern: &str) -> Detector<'_> {
    if let Some(name) = pattern
        .strip_prefix('.')
        .and_then(|p| p.strip_suffix('('))
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_'))
    {
        return Detector::Method(name);
    }
    for attr in ["parent", "children"] {
        if let Some(rest) = pattern
            .strip_prefix("self.")
            .and_then(|p| p.strip_prefix(attr))
        {
            let mut chars = rest.chars();
            match (chars.next(), chars.next()) {
                (None, _) => return Detector::SelfAttribute(attr, None),
                (Some(c), None) if matches!(c, '.' | ')' | ',' | '\n' | '\r') => {
                    return Detector::SelfAttribute(attr, Some(c));
                }
                _ => {}
            }
        }
    }
    match pattern {
        "../" => Detector::Up,
        "./" => Detector::Here,
        other => Detector::Text(other),
    }
}

/// One use of a traversal construct, before it is matched to patterns.
enum Hit {
    Method(String),
    SelfAttribute(String, Option<char>),
    Up,
    Here,
}

fn eq(a: &str, b: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        a == b
    } else {
        a.eq_ignore_ascii_case(b)
    }
}

impl Hit {
    fn matches(&self, d: Detector<'_>, cs: bool) -> bool {
        match (self, d) {
            (Hit::Method(n), Detector::Method(m)) => eq(n, m, cs),
            (Hit::SelfAttribute(a, next), Detector::SelfAttribute(b, want)) => {
                eq(a, b, cs) && (want.is_none() || *next == want)
            }
            (Hit::Up, Detector::Up) | (Hit::Here, Detector::Here) => true,
            _ => false,
        }
    }
}

/// Matches hits (with file ranges) and plain-text patterns against the
/// configured patterns. Returns the matched patterns in configuration
/// order and the range of the first hit.
fn matched<'o>(
    options: &'o Options,
    hits: &[(Hit, TextRange)],
    text: &str,
) -> (Vec<&'o str>, Option<TextRange>) {
    let mut found = Vec::new();
    let mut first: Option<TextRange> = None;
    for p in &options.patterns {
        let d = detector(p);
        let hit = match d {
            Detector::Text(t) => {
                let present = if options.case_sensitive {
                    text.contains(t)
                } else {
                    text.to_lowercase().contains(&t.to_lowercase())
                };
                present.then_some(None)
            }
            _ => hits
                .iter()
                .find(|(h, _)| h.matches(d, options.case_sensitive))
                .map(|(_, r)| Some(*r)),
        };
        if let Some(range) = hit {
            if !found.contains(&p.as_str()) {
                found.push(p.as_str());
            }
            if let Some(r) = range
                && first.is_none_or(|f| r.start() < f.start())
            {
                first = Some(r);
            }
        }
    }
    (found, first)
}

struct ScriptHits<'s> {
    source: &'s str,
    hits: Vec<(Hit, TextRange)>,
}

impl<'a> Visitor<'a> for ScriptHits<'_> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Call(call) => {
                // Every method call; the configured patterns decide which
                // names count.
                if let Expr::Attribute(a) = &*call.func {
                    self.hits
                        .push((Hit::Method(a.attr.as_str().to_owned()), a.attr.range));
                }
            }
            Expr::Attribute(a) => {
                if let Expr::Name(n) = &*a.value
                    && &*n.id == "self"
                    && matches!(
                        a.attr.as_str().to_ascii_lowercase().as_str(),
                        "parent" | "children"
                    )
                {
                    let next = self
                        .source
                        .get(a.range.end().to_usize()..)
                        .and_then(|rest| rest.chars().next());
                    self.hits.push((
                        Hit::SelfAttribute(a.attr.as_str().to_owned(), next),
                        a.range,
                    ));
                }
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

fn path_hit(target: &PropertyTarget, range: TextRange) -> Option<(Hit, TextRange)> {
    match target {
        PropertyTarget::Relative { up, .. } if *up > 0 => Some((Hit::Up, range)),
        PropertyTarget::Relative { .. } => Some((Hit::Here, range)),
        _ => None,
    }
}

struct FragileReference;

/// Expressions of one ign-lint node: node path, ign-lint path, hits, text,
/// anchor.
type ExpressionGroup = (
    String,
    Option<String>,
    Vec<(Hit, TextRange)>,
    String,
    TextRange,
);

impl ViewRule for FragileReference {
    fn check_view(&self, cx: &mut Checker<'_, '_>, view: &View<'_>) {
        let ModelData::View { file, .. } = cx.data() else {
            return;
        };
        let file = *file;
        let parsed = cx.parsed_scripts();
        // Scripts.
        for (script, parsed) in view.scripts.iter().zip(parsed) {
            let mut v = ScriptHits {
                source: &script.source,
                hits: Vec::new(),
            };
            v.visit_module(&parsed.module);
            let hits: Vec<(Hit, TextRange)> = v
                .hits
                .into_iter()
                .map(|(h, r)| (h, script.file_range(r)))
                .collect();
            let anchor = script.anchor();
            report(
                cx,
                file,
                "Script",
                &hits,
                script.body_text(),
                anchor,
                &script.node_path,
                script.ign_lint_path.clone(),
            );
        }
        // Expressions, grouped by their ign-lint node (a struct binding is
        // one node).
        let mut groups: Vec<ExpressionGroup> = Vec::new();
        // Position of each node's group.
        let mut group_of: rustc_hash::FxHashMap<String, usize> = Default::default();
        for e in &view.expressions {
            if !matches!(
                e.kind,
                ExpressionKind::Binding | ExpressionKind::StructLeaf | ExpressionKind::Transform
            ) {
                continue;
            }
            let mut hits = Vec::new();
            for (bound, reference) in e.references() {
                if let Reference::Property(p) = reference
                    && let Some(hit) = path_hit(&p.target, e.file_range(bound.path_range))
                {
                    hits.push(hit);
                }
            }
            let node = match e.kind {
                ExpressionKind::StructLeaf => e
                    .binding
                    .map(|b| view.binding(b).node_path.clone())
                    .unwrap_or_else(|| e.node_path.clone()),
                _ => e.node_path.clone(),
            };
            match group_of.get(&node) {
                Some(&i) => {
                    let g = &mut groups[i];
                    g.2.extend(hits);
                    g.3.push('\n');
                    g.3.push_str(e.text());
                }
                None => {
                    group_of.insert(node.clone(), groups.len());
                    groups.push((
                        node,
                        e.ign_lint_path.clone(),
                        hits,
                        e.text().to_owned(),
                        e.source.range(),
                    ));
                }
            }
        }
        for (node, ign, hits, text, anchor) in groups {
            report(cx, file, "Expression", &hits, &text, anchor, &node, ign);
        }
        // Property bindings.
        for b in &view.bindings {
            if b.kind != BindingKind::Property {
                continue;
            }
            let BindingConfig::Property(p) = &b.typed else {
                continue;
            };
            let Some(path) = p.path else {
                continue;
            };
            let hits: Vec<(Hit, TextRange)> = parse_property_ref(path.value(), TextSize::ZERO)
                .and_then(|r| path_hit(&r.target, path.raw_range(r.range)))
                .into_iter()
                .collect();
            report(
                cx,
                file,
                "Property Binding",
                &hits,
                path.value(),
                path.range(),
                &b.node_path,
                Some(b.ign_lint_path.clone()),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn report(
    cx: &mut Checker<'_, '_>,
    file: FileId,
    kind: &str,
    hits: &[(Hit, TextRange)],
    text: &str,
    anchor: TextRange,
    node_path: &str,
    ign_lint_path: Option<String>,
) {
    let options = cx.options::<Options>();
    let (patterns, first) = matched(options, hits, text);
    let Some(head) = patterns.first() else {
        return;
    };
    let shown: Vec<String> = patterns
        .iter()
        .map(|p| format!("`{}`", p.replace('\n', "\\n").replace('\r', "\\r")))
        .collect();
    let what = match kind {
        "Script" => "Script",
        "Expression" => "Expression",
        _ => "Property binding",
    };
    let message = format!(
        "{what} navigates the component tree ({}); use view custom properties, parameters or messages instead",
        shown.join(", ")
    );
    let more = if patterns.len() > 1 {
        format!(
            " and {} other object traversal pattern(s)",
            patterns.len() - 1
        )
    } else {
        String::new()
    };
    let ign = format!(
        "{kind} contains '{head}'{more} which creates brittle view structure dependencies. Consider using view.custom properties or message handling for component communication instead."
    );
    let range = first.unwrap_or(anchor);
    cx.report_in(file, range, message)
        .node_path(node_path.to_owned())
        .ign_lint_path(ign_lint_path)
        .ign_lint_message(ign);
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn view(script: &str, expression: &str, path: &str) -> String {
        format!(
            r#"{{"root": {{"meta": {{"name": "root"}}, "type": "ia.container.flex", "children": [
  {{"meta": {{"name": "Button"}}, "type": "ia.input.button",
    "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": {script}}}}}}}}},
    "propConfig": {{
      "props.text": {{"binding": {{"type": "expr", "config": {{"expression": {expression}}}}}}},
      "props.enabled": {{"binding": {{"type": "property", "config": {{"path": {path}}}}}}}
    }}
  }}]}}}}"#
        )
    }

    fn lint(view: &str, options: &str) -> Vec<(String, String)> {
        Fixture::view(view)
            .lint_with(&[("VIEW018", options)])
            .iter()
            .map(|d| {
                (
                    d.message.clone(),
                    flagged(std::slice::from_ref(d))[0].clone(),
                )
            })
            .collect()
    }

    #[test]
    fn reports_scripts_expressions_and_paths() {
        let found = lint(
            &view(
                r#""\tx = self.getSibling('Label').props.text\n\tself.parent.custom.y = 1""#,
                r#""{../Label.props.text} + 'a'""#,
                r#""./Child.props.enabled""#,
            ),
            "",
        );
        assert_eq!(
            found,
            vec![
                (
                    "Script navigates the component tree (`.getSibling(`, `self.parent.`); use view custom properties, parameters or messages instead".into(),
                    "getSibling".into()
                ),
                (
                    "Expression navigates the component tree (`../`); use view custom properties, parameters or messages instead".into(),
                    "../Label.props.text".into()
                ),
                (
                    "Property binding navigates the component tree (`./`); use view custom properties, parameters or messages instead".into(),
                    "./Child.props.enabled".into()
                ),
            ]
        );
    }

    #[test]
    fn comments_strings_and_clean_code_are_ignored() {
        let found = lint(
            &view(
                r#""\t# self.getSibling('X')\n\tlabel = 'self.parent.x ../y'\n\tself.view.custom.x = 1""#,
                r#""'../' + {view.custom.x} + {/root/Label.props.text}""#,
                r#""view.params.enabled""#,
            ),
            "",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn options_select_patterns_and_case() {
        let v = view(
            r#""\tx = self.getSibling('L')\n\ty = self.GetChild('M')""#,
            r#""{../Label.props.text}""#,
            r#""view.params.x""#,
        );
        let only_sibling = lint(&v, "forbidden-patterns = [\".getSibling(\"]");
        assert_eq!(only_sibling.len(), 1);
        assert!(only_sibling[0].0.contains("`.getSibling(`"));
        assert_eq!(lint(&v, "forbidden-patterns = [\".getChild(\"]").len(), 0);
        let insensitive = lint(
            &v,
            "forbidden-patterns = [\".getChild(\"]\ncase-sensitive = false",
        );
        assert_eq!(insensitive.len(), 1);
        let text = lint(&v, "forbidden-patterns = [\"Label\"]");
        assert_eq!(text.len(), 1, "{text:?}");
    }

    #[test]
    fn struct_bindings_are_one_finding() {
        let v = r#"{"root": {"meta": {"name": "root"}, "type": "ia.container.flex", "children": [
  {"meta": {"name": "A"}, "type": "ia.display.label", "propConfig": {"props.text": {"binding": {"type": "expr-struct", "config": {"struct": {"a": "{../B.props.text}", "b": "{./C.props.text}"}}}}}}]}}"#;
        let found = lint(v, "");
        assert_eq!(found.len(), 1);
        assert!(found[0].0.contains("`../`, `./`"));
    }
}
