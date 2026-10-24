//! BIND005 unresolved-property-reference.

use iglint_expression::{
    PropertyRef, PropertyTarget, Reference, ReferenceContext, misspaced_property_ref,
};
use iglint_project::view::{Binding, BindingConfig, RefResolution, Unresolved, View};
use iglint_project::{EmbeddedExpression, Existence, ExpressionKind, FileId};
use iglint_text::TextRange;

use super::{expression_file, ign_lint_expression_path};
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::options::OptionReader;
use crate::registry::{Category, OptionMeta, OptionType, RuleMeta};
use crate::rule::{BindingRule, ExpressionRule, Hook, ParsedOptions, Phase, RuleImpl};
use crate::script_facts::AccessTarget;
use iglint_project::view::RefTarget;

pub static META: RuleMeta = RuleMeta {
    code: "BIND005",
    name: "unresolved-property-reference",
    category: Category::Bind,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Property reference does not resolve in the view",
    explanation: r#"## What it does

Resolves every property reference of a view the way Perspective does and
reports those that point nowhere:

* `{../Sibling.props.text}`, `{./Child.custom.x}`, `{.../Uncle.props.y}`,
  `{/root/Panel/Label.props.text}` and property binding paths with a
  component name that does not exist at that point of the tree;
* relative references that climb above the root container (or start from a
  view-level property, which has no component);
* `{ view.custom.x }`: whitespace inside the braces makes a property
  reference unparseable for Perspective's `PropertyRef` and
  `LegacyPropertyRef`, so it is read as a tag path (checked against
  Ignition 8.3.3's parsers);
* `{this.custom.x}`, `{view.params.y}`, `{view.custom.z}`, `{parent.custom.w}`
  where the custom property or view parameter is not defined;
* references with nothing after the scope or the property type (`{view}`,
  `{this}`, `{session}`, `{view.custom}`, `{this.props}`,
  `{../Label.props}`): they match Perspective's reference grammar, so the
  expression reads them as property references, but Perspective's gateway
  rejects them ("Invalid property identifier ..., no scope specified" or
  "no property specified", checked on Ignition 8.3.3) and the binding
  fails;
* with `strict`, `{session.custom.x}` where the project's session
  properties do not define `x` (only when the project and its parents are
  all linted; session startup scripts often create session properties at
  run time, so this is not checked by default).

Assigning to an undefined custom property or view parameter in a script
creates it, so a property that a script of the view may write is not
reported.

`props`, `meta` and `position` are not checked beyond the component: their
keys depend on the component type and may be added at run time.

## Why is this bad?

A reference that does not resolve makes the binding evaluate to `null`
with a bad quality, usually after a component was renamed or moved. The
Designer does not warn.

## Example

```json
{"binding": {"type": "expr", "config": {"expression": "{../Titel.props.text}"}}}
```

Use instead:

```json
{"binding": {"type": "expr", "config": {"expression": "{../Title.props.text}"}}}
```

## Options

* `validate-expressions`: check references in expressions.
* `validate-property-bindings`: check property binding paths.

## ign-lint

Equivalent of the expression and property binding part of
`ComponentReferenceValidationRule` (kwargs `validate_expressions`,
`validate_property_bindings`), with the same messages in `iglint ign-lint`
for the references ign-lint checks (relative and absolute component
paths). The script part is `SCR305`. iglint also checks custom properties,
view parameters and session properties, which ign-lint does not, and it
parses references instead of matching text.
"#,
    options: &[
        OptionMeta {
            name: "validate-expressions",
            ty: OptionType::Bool,
            default: "true",
            description: "Check references in expressions.",
        },
        OptionMeta {
            name: "validate-property-bindings",
            ty: OptionType::Bool,
            default: "true",
            description: "Check property binding paths.",
        },
    ],
    ign_lint_names: &["ComponentReferenceValidationRule"],
    ign_lint_option_map: &[
        ("validate_expressions", "validate-expressions"),
        ("validate_property_bindings", "validate-property-bindings"),
    ],
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[
        Hook::Expression(&UnresolvedPropertyReference),
        Hook::Binding(&UnresolvedPropertyReference),
    ],
    options: Some(parse_options),
};

struct Options {
    expressions: bool,
    property_bindings: bool,
}

fn parse_options(r: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    Ok(Box::new(Options {
        expressions: r.bool("validate-expressions")?,
        property_bindings: r.bool("validate-property-bindings")?,
    }))
}

struct UnresolvedPropertyReference;

/// Where a reference was found.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Site {
    Expression,
    PropertyBinding,
}

impl Site {
    fn kind(self) -> &'static str {
        match self {
            Site::Expression => "expression",
            Site::PropertyBinding => "property binding",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Site::Expression => "Expression",
            Site::PropertyBinding => "Property Binding",
        }
    }
}

struct Finding {
    range: TextRange,
    message: String,
    ign_lint_message: Option<String>,
}

/// Component path segments of a relative or absolute reference, as ign-lint
/// prints them (`a/b`).
fn component_segments(reference: &PropertyRef) -> Option<(u32, String)> {
    match &reference.target {
        PropertyTarget::Relative { up, components, .. } => Some((
            *up,
            components
                .iter()
                .filter_map(|c| c.name())
                .collect::<Vec<_>>()
                .join("/"),
        )),
        PropertyTarget::Absolute { components } => Some((
            0,
            components
                .iter()
                .filter_map(|c| c.name())
                .collect::<Vec<_>>()
                .join("/"),
        )),
        _ => None,
    }
}

/// Checks one reference. `path` is its text, `full` the whole expression or
/// binding path (ign-lint quotes it).
fn check(
    cx: &Checker<'_, '_>,
    view: &View<'_>,
    owner: iglint_project::Owner,
    reference: &PropertyRef,
    path: &str,
    full: &str,
    site: Site,
) -> Option<Finding> {
    let ign = component_segments(reference);
    // Perspective's gateway parser (`PropertyReference`, 8.3.3) needs a
    // property type and a property after the scope: `{view}` fails with
    // "no scope specified" and `{view.custom}` with "no property
    // specified", although both match `PropertyRef`, so the expression
    // reads them as property references, not tag paths.
    if reference.segments.len() < 2 {
        let (after, needs, error) = if reference.segments.is_empty() {
            (
                "scope",
                "a property type and a property",
                "no scope specified",
            )
        } else {
            ("property type", "a property", "no property specified")
        };
        return Some(Finding {
            range: reference.range,
            message: format!(
                "`{path}` stops after the {after}: Perspective needs {needs} (`view.custom.x`) \
                 and cannot bind it (\"{error}\")"
            ),
            ign_lint_message: None,
        });
    }
    match view.resolve_ref(owner, reference) {
        RefResolution::Resolved(_) | RefResolution::Dynamic => None,
        RefResolution::Scope(scope) => {
            // Session custom properties are often created at run time by
            // session startup scripts; only report them in strict mode.
            if scope.scope != "session" || !cx.strict() {
                return None;
            }
            let (index, project) = cx.project_index()?;
            let (category, name) = (scope.category?, scope.name?);
            if category != "custom" {
                return None;
            }
            (index.session_property(project, &category, &name) == Existence::Missing).then(|| {
                Finding {
                    range: reference
                        .segments
                        .get(1)
                        .map_or(reference.range, |s| s.range),
                    message: format!(
                        "`{path}`: session property `custom.{name}` is not defined in the project's session properties"
                    ),
                    ign_lint_message: None,
                }
            })
        }
        RefResolution::Unresolved(u) => Some(match u {
            Unresolved::AboveRoot { up } => Finding {
                range: reference.range,
                message: format!(
                    "`{path}` goes {up} level{} up, above the root container",
                    if up == 1 { "" } else { "s" }
                ),
                ign_lint_message: ign.map(|_| {
                    format!(
                        "Relative {} '{full}' navigates above root component",
                        site.kind()
                    )
                }),
            },
            Unresolved::NoComponent => Finding {
                range: reference.range,
                message: format!(
                    "`{path}` is relative to a component, but it is used outside of any component"
                ),
                ign_lint_message: ign.map(|(up, segments)| {
                    if up >= 1 {
                        format!(
                            "Relative {} '{full}' navigates above root component",
                            site.kind()
                        )
                    } else {
                        format!(
                            "{} references non-existent component '{segments}' in: {full}",
                            site.title()
                        )
                    }
                }),
            },
            Unresolved::MissingComponent { name, range } => Finding {
                range,
                message: format!(
                    "`{path}`: there is no component `{name}` at this point of the view"
                ),
                ign_lint_message: ign.map(|(_, segments)| {
                    format!(
                        "{} references non-existent component '{segments}' in: {full}",
                        site.title()
                    )
                }),
            },
            Unresolved::MissingProperty {
                category,
                name,
                range,
            } => {
                if may_be_created_by_scripts(cx, view, owner, reference, &category, &name) {
                    return None;
                }
                Finding {
                    range,
                    message: format!(
                        "`{path}`: {} `{name}` is not defined",
                        if category == "params" {
                            "view parameter"
                        } else {
                            "custom property"
                        }
                    ),
                    ign_lint_message: None,
                }
            }
        }),
    }
}

/// Whether a script of the view may create `category.name` at run time
/// (assigning to an undefined custom property or parameter creates it).
/// With complete script facts this is an exact write check; otherwise any
/// script that mentions the name as an identifier counts.
fn may_be_created_by_scripts(
    cx: &Checker<'_, '_>,
    view: &View<'_>,
    owner: iglint_project::Owner,
    reference: &PropertyRef,
    category: &str,
    name: &str,
) -> bool {
    let (accesses, complete) = cx.script_property_accesses();
    // A name the scripts call that may be a frame builtin may create
    // anything.
    let accesses =
        crate::rules::view::unused_custom_property::resolve_callee_checks(cx, view, accesses);
    if complete {
        // The object the reference points at: resolve it with a category
        // that always exists.
        let mut probe = reference.clone();
        probe.segments.truncate(1);
        if let Some(first) = probe.segments.first_mut() {
            first.kind = iglint_expression::SegmentKind::Name("props".to_owned());
        }
        let target = match view.resolve_ref(owner, &probe) {
            RefResolution::Resolved(r) => r.target,
            _ => return true,
        };
        return accesses.iter().any(|a| {
            let same_target = match (a.target, target) {
                (AccessTarget::View, RefTarget::View) => true,
                (AccessTarget::Component(c), RefTarget::Component(t)) => c == t,
                (AccessTarget::AnyComponent, RefTarget::Component(_)) => true,
                _ => false,
            };
            same_target
                && a.write
                && a.category.name() == category
                && a.name.as_deref().is_none_or(|n| n == name)
        });
    }
    cx.data()
        .scripts()
        .iter()
        .any(|(_, script)| mentions_identifier(script.body_text(), name))
}

fn mentions_identifier(text: &str, name: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(name).any(|(i, _)| {
        // `match_indices` yields character boundaries.
        let before = text.get(..i).unwrap_or_default().chars().next_back();
        let after = text
            .get(i + name.len()..)
            .unwrap_or_default()
            .chars()
            .next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

impl ExpressionRule for UnresolvedPropertyReference {
    fn check_expression(&self, cx: &mut Checker<'_, '_>, e: &EmbeddedExpression<'_>) {
        if !cx.options::<Options>().expressions {
            return;
        }
        let Some(view) = cx.view() else {
            return;
        };
        let mut findings = Vec::new();
        for (bound, reference) in e.references() {
            if let Reference::Tag(_) = reference
                && e.context != ReferenceContext::Gateway
                && misspaced_property_ref(&bound.path, bound.path_range.start()).is_some()
            {
                findings.push(Finding {
                    range: bound.path_range,
                    message: format!(
                        "`{{{}}}` is not a valid property reference: Perspective reads it as a \
                         tag path because of the whitespace inside the braces",
                        bound.path
                    ),
                    ign_lint_message: None,
                });
                continue;
            }
            let Reference::Property(p) = reference else {
                continue;
            };
            if let Some(f) = check(
                cx,
                view,
                e.owner,
                &p,
                &bound.path,
                e.text(),
                Site::Expression,
            ) {
                findings.push(f);
            }
        }
        if findings.is_empty() {
            return;
        }
        let Some(file) = expression_file(cx.data(), e) else {
            return;
        };
        // ign-lint names a structure leaf by its binding; every other
        // expression by its own path (`{binding}.{parameter}` for query
        // parameters), also where ign-lint itself does not look.
        let ign_path = match e.kind {
            ExpressionKind::StructLeaf => e.ign_lint_path.clone(),
            _ => ign_lint_expression_path(cx.data(), e),
        };
        for f in findings {
            let report = cx
                .report_expression(file, e, f.range, f.message)
                .node_path(e.node_path.clone())
                .ign_lint_path(ign_path.clone());
            if let Some(m) = f.ign_lint_message {
                report.ign_lint_message(m);
            }
        }
    }
}

impl BindingRule for UnresolvedPropertyReference {
    fn check_binding(&self, cx: &mut Checker<'_, '_>, binding: &Binding<'_>) {
        if !cx.options::<Options>().property_bindings {
            return;
        }
        let Some(view) = cx.view() else {
            return;
        };
        let BindingConfig::Property(p) = &binding.typed else {
            return;
        };
        let (Some(path), Some(reference)) = (p.path, &p.reference) else {
            return;
        };
        let text = path.value();
        let Some(f) = check(
            cx,
            view,
            binding.owner,
            reference,
            text,
            text,
            Site::PropertyBinding,
        ) else {
            return;
        };
        let file: FileId = cx.primary_file();
        let range = path.raw_range(f.range);
        let report = cx
            .report_in(file, range, f.message)
            .node_path(binding.node_path.clone())
            .ign_lint_path(Some(binding.ign_lint_path.clone()));
        if let Some(m) = f.ign_lint_message {
            report.ign_lint_message(m);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    const TREE: &str = r#"{"params": {"p": 1}, "custom": {"c": 1}, "propConfig": {PC}, "root": {"type": "ia.container.flex", "meta": {"name": "root"}, "custom": {"rc": 1}, "children": [
        {"type": "ia.display.label", "meta": {"name": "Left"}, "custom": {"lc": 1}, "propConfig": {LEFT}},
        {"type": "ia.container.flex", "meta": {"name": "Panel"}, "children": [
            {"type": "ia.display.label", "meta": {"name": "Inner"}, "propConfig": {INNER}}]}]}}"#;

    fn tree(pc: &str, left: &str, inner: &str) -> Fixture {
        Fixture::view(
            &TREE
                .replace("{PC}", &format!("{{{pc}}}"))
                .replace("{LEFT}", &format!("{{{left}}}"))
                .replace("{INNER}", &format!("{{{inner}}}")),
        )
    }

    fn expr(e: &str) -> String {
        format!(
            r#""props.text": {{"binding": {{"type": "expr", "config": {{"expression": {}}}}}}}"#,
            serde_json::to_string(e).unwrap()
        )
    }

    fn prop(p: &str) -> String {
        format!(
            r#""props.text": {{"binding": {{"type": "property", "config": {{"path": "{p}"}}}}}}"#
        )
    }

    #[test]
    fn resolvable_references_are_clean() {
        let f = tree(
            &expr("{this.custom.c} + {view.params.p}").replace("props.text", "custom.x"),
            &expr(
                "{../Panel/Inner.props.text} + {this.custom.lc} + {parent.custom.rc} + {/root/Left.meta.name} + {view.custom.c} + {[default]Tag}",
            ),
            &prop("../../Left.custom.lc"),
        );
        assert_eq!(messages(&f.lint(&["BIND005"])), vec![]);
    }

    #[test]
    fn missing_components_with_ign_lint_wording() {
        let f = tree(
            "",
            &expr("{../Right.props.text} + {./Nope.props.text}"),
            &prop("/root/Ghost.props.text"),
        );
        let d = f.lint(&["BIND005"]);
        assert_eq!(flagged(&d), vec!["Right", "Nope", "Ghost"]);
        assert_eq!(
            d[1].ign_lint_message.as_deref(),
            Some(
                "Expression references non-existent component 'Nope' in: {../Right.props.text} + {./Nope.props.text}"
            )
        );
        assert_eq!(
            d[2].ign_lint_message.as_deref(),
            Some(
                "Property Binding references non-existent component 'root/Ghost' in: /root/Ghost.props.text"
            )
        );
        assert_eq!(
            d[2].ign_lint_path.as_deref(),
            Some("root.root.children[1].Panel.children[0].Inner.propConfig.props.text")
        );
    }

    #[test]
    fn above_root_and_view_level_relative_references() {
        let f = tree(
            r#""custom.v": {"binding": {"type": "expr", "config": {"expression": "{../root.props.text}"}}}"#,
            &expr("{.../Left.props.text}"),
            "",
        );
        let d = f.lint(&["BIND005"]);
        let ign: Vec<_> = d
            .iter()
            .map(|d| d.ign_lint_message.clone().unwrap())
            .collect();
        assert_eq!(
            ign,
            vec![
                "Relative expression '{../root.props.text}' navigates above root component",
                "Relative expression '{.../Left.props.text}' navigates above root component",
            ]
        );
    }

    /// Review round 22 (Opus) N1: Perspective 8.3.3's `PropertyReference`
    /// rejects a scope alone ("no scope specified") and a property type
    /// alone ("no property specified"); `{..}` and `{../Left}` do not match
    /// `PropertyRef` and are tag paths, not property references.
    #[test]
    fn references_without_a_property() {
        let f = tree(
            "",
            &expr(
                "{view} + {this} + {session} + {page} + {parent} + {view.custom} + {this.props} + {../Left.props} + {/root.props} + {view.params}",
            ),
            &prop("view.custom"),
        );
        let d = f.lint(&["BIND005"]);
        assert_eq!(
            flagged(&d),
            vec![
                "view",
                "this",
                "session",
                "page",
                "parent",
                "view.custom",
                "this.props",
                "../Left.props",
                "/root.props",
                "view.params",
                "view.custom",
            ]
        );
        assert_eq!(
            d[0].message,
            "`view` stops after the scope: Perspective needs a property type and a property \
             (`view.custom.x`) and cannot bind it (\"no scope specified\")"
        );
        assert_eq!(
            d[5].message,
            "`view.custom` stops after the property type: Perspective needs a property \
             (`view.custom.x`) and cannot bind it (\"no property specified\")"
        );
        let f = tree(
            "",
            &expr("{..} + {../Left} + {view.custom.c} + {this.meta.name}"),
            "",
        );
        assert_eq!(messages(&f.lint(&["BIND005"])), vec![]);
    }

    #[test]
    fn undefined_custom_properties_and_params() {
        let f = tree(
            "",
            &expr("{this.custom.nope} + {view.params.q} + {view.custom.c}"),
            "",
        );
        let d = f.lint(&["BIND005"]);
        assert_eq!(flagged(&d), vec!["nope", "q"]);
        assert!(d.iter().all(|d| d.ign_lint_message.is_none()));
    }

    #[test]
    fn properties_a_script_may_create_are_not_reported() {
        let left = format!(
            "{}, \"custom.y\": {{\"onChange\": {{\"enabled\": true, \"script\": \"\\tself.custom.nope = 1\"}}}}",
            expr("{this.custom.nope} + {this.custom.other}")
        );
        let d = tree("", &left, "").lint(&["BIND005"]);
        assert_eq!(flagged(&d), vec!["other"]);
    }

    #[test]
    fn session_properties_only_in_strict_mode() {
        let f = Fixture::view(&format!(
            r#"{{"propConfig": {{{}}}}}"#,
            expr("{session.custom.unknown}").replace("props.text", "custom.x")
        ))
        .resource(
            &format!("projects/p/{PERSPECTIVE}/session-props"),
            &[("props.json", r#"{"custom": {"known": 1}}"#)],
        );
        assert!(f.lint(&["BIND005"]).is_empty());
    }

    #[test]
    fn options_disable_each_part() {
        let f = tree(
            "",
            &expr("{../Right.props.text}"),
            &prop("../Nope.props.text"),
        );
        assert_eq!(
            f.lint_with(&[("BIND005", "validate-expressions = false")])
                .len(),
            1
        );
        assert_eq!(
            f.lint_with(&[("BIND005", "validate-property-bindings = false")])
                .len(),
            1
        );
    }

    /// Reviewer round 10, O1: references in HTTP header values and tag
    /// history date ranges are resolved like any other.
    #[test]
    fn references_in_measured_binding_fields() {
        let f = tree(
            "",
            r#""props.text": {"binding": {"type": "http", "config": {"request": {"url": "'u'", "method": "GET",
                "header": [{"key": "A", "value": "{../Ghost.props.text}"}], "body": "{this.custom.nope}"}}}},
               "props.style": {"binding": {"type": "tag-history", "config": {"dateRange": {"startDate": "{../Left.custom.lc}", "endDate": "{../Gone.props.value}"},
                "tags": [{"path": "[default]A"}]}}}"#,
            "",
        );
        assert_eq!(
            flagged(&f.lint(&["BIND005"])),
            vec!["Ghost", "nope", "Gone"]
        );
    }

    /// Reviewer round 10, O3: whitespace inside the braces makes
    /// Perspective read a property reference as a tag path (checked with
    /// the expression oracle on 8.3.3).
    #[test]
    fn whitespace_inside_the_braces_is_not_a_property_reference() {
        let f = tree(
            &expr("{ view.custom.c } + {view.custom.c }").replace("props.text", "custom.x"),
            &expr("{ ../Panel/Inner.props.text}"),
            "",
        );
        let d = f.lint(&["BIND005"]);
        let texts: Vec<String> = d.iter().map(|d| d.message.clone()).collect();
        assert_eq!(
            texts,
            vec![
                "`{ view.custom.c }` is not a valid property reference: Perspective reads it as a tag path because of the whitespace inside the braces",
                "`view.custom.c `: custom property `c ` is not defined",
                "`{ ../Panel/Inner.props.text}` is not a valid property reference: Perspective reads it as a tag path because of the whitespace inside the braces",
            ]
        );
        // Tag paths with whitespace stay tag paths.
        let f = tree(&expr("{ [default]A/B } + { Folder/Tag }"), "", "");
        assert!(f.lint(&["BIND005"]).is_empty());
    }
}
