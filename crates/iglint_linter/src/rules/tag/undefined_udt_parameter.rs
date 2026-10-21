//! TAG002 undefined-udt-parameter.

use iglint_project::ProjectIndex;
use iglint_project::kinds::ResourceKind;
use iglint_project::model::ModelData;
use iglint_project::summary::Facts;
use iglint_project::tags::BUILTIN_UDT_PARAMETERS;
use rustc_hash::FxHashSet;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "TAG002",
    name: "undefined-udt-parameter",
    category: Category::Tag,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "`{Param}` in a UDT member names no parameter at all",
    explanation: r#"## What it does

Checks the `{Name}` placeholders of parameter-bound properties
(`{"bindType": "parameter", "binding": "..."}`) of UDT definition
members. A name is reported when it is neither a built-in parameter
(`InstanceName`, `PathToParentFolder`, `PathToTag`, `TagName`,
`ParentInstanceName`, `RootInstanceName`) nor a parameter of any UDT
definition or UDT instance of the tag provider. Names are compared
case-insensitively.

The check is deliberately provider-wide: Ignition resolves a placeholder
through the enclosing instances, so a member of a UDT that is nested in
another UDT may use the outer UDT's parameters, and instances may add
parameters of their own. A name that nothing in the provider defines can
never be substituted. Placeholders that are not plain names (offsets,
formats) are not checked, nor are providers whose tags are not in the
workspace.

## Why is this bad?

An undefined parameter is not substituted: the OPC item path, tooltip or
other property keeps the literal `{Name}` text, so the member reads from
the wrong address or shows the placeholder to operators.

## Example

The UDT declares `DeviceName`:

```json
{"opcItemPath": {"bindType": "parameter", "binding": "ns=1;s=[{Device}]Speed"}}
```

Use instead:

```json
{"opcItemPath": {"bindType": "parameter", "binding": "ns=1;s=[{DeviceName}]Speed"}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Resource(&UndefinedParameter)],
    options: None,
};

struct UndefinedParameter;

impl ResourceRule for UndefinedParameter {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let ModelData::Tags(folder) = data else {
            return;
        };
        if !folder.definitions {
            return;
        }
        let Some(index) = cx.index() else {
            return;
        };
        let mut known: Option<FxHashSet<String>> = None;
        for tag in &folder.tags {
            for p in &tag.bound_properties {
                if !p
                    .bind_type
                    .is_some_and(|t| t.eq_ignore_ascii_case("parameter"))
                {
                    continue;
                }
                for (name, range) in &p.placeholders {
                    if !is_plain_name(name)
                        || BUILTIN_UDT_PARAMETERS
                            .iter()
                            .any(|b| b.eq_ignore_ascii_case(name))
                    {
                        continue;
                    }
                    let known =
                        known.get_or_insert_with(|| provider_parameters(index, folder.provider));
                    if !known.contains(&name.to_lowercase()) {
                        cx.report_in(
                            tag.file,
                            *range,
                            format!(
                                "`{name}` is not a parameter of any UDT in provider `{}`",
                                folder.provider
                            ),
                        )
                        .node_path(&tag.full_path);
                    }
                }
            }
        }
    }
}

fn is_plain_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Lowercase names of every parameter of every definition and instance of
/// `provider`.
fn provider_parameters(index: &ProjectIndex<'_>, provider: &str) -> FxHashSet<String> {
    let ws = index.workspace();
    let mut out = FxHashSet::default();
    for r in &ws.resources {
        if !matches!(
            r.kind,
            ResourceKind::TagDefinition | ResourceKind::TagTypeDefinition
        ) {
            continue;
        }
        let Some(Facts::Tags(t)) = index.summary(r.id).map(|s| &s.facts) else {
            continue;
        };
        if !t.provider.eq_ignore_ascii_case(provider) {
            continue;
        }
        for tag in &t.tags {
            out.extend(tag.parameters.iter().map(|p| p.to_lowercase()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::rules::tag::fixtures::*;
    use crate::rules::test_support::*;

    #[test]
    fn names_nobody_defines() {
        let f = udts(
            gateway(),
            r#"[{"name": "Outer", "tagType": "UdtType", "parameters": {"Device": {"dataType": "String", "value": ""}}, "tags": []},
                {"name": "Motor", "tagType": "UdtType",
                 "parameters": {"Address": {"dataType": "String", "value": ""}},
                 "tags": [{"name": "Speed", "tagType": "AtomicTag", "valueSource": "opc",
                    "opcItemPath": {"bindType": "parameter", "binding": "[{device}]{Address}.{Adress} {InstanceName} {Offset+1} {Extra}"}}]}]"#,
        );
        let f = tags(
            f,
            "[]",
            r#"[{"name": "M1", "tagType": "UdtInstance", "typeId": "Motor", "parameters": {"Extra": {"dataType": "String", "value": "x"}}}]"#,
        );
        let d = f.lint(&["TAG002"]);
        assert_eq!(flagged(&d), vec!["Adress"]);
        assert_eq!(
            d[0].message,
            "`Adress` is not a parameter of any UDT in provider `default`"
        );
    }
}
