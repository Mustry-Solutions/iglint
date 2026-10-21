//! TAG001 unknown-udt-type.

use iglint_project::Existence;
use iglint_project::model::ModelData;
use iglint_project::tags::TagType;

use super::definition_path;
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "TAG001",
    name: "unknown-udt-type",
    category: Category::Tag,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "`typeId` does not name a UDT definition, or UDT inheritance is cyclic",
    explanation: r#"## What it does

Checks the `typeId` of every UDT instance (and the parent `typeId` of
every UDT definition) against the UDT definitions of the tag provider, and
reports UDT definitions whose parent chain leads back to themselves.
Providers whose definitions are not in the workspace are not checked.

## Why is this bad?

An instance of a missing UDT has no members: the gateway shows it with a
configuration error and every binding to its members fails. An
inheritance cycle makes every definition in the cycle unusable.

## Example

```json
{"name": "Motor1", "tagType": "UdtInstance", "typeId": "Motors/Motr"}
```

Use instead:

```json
{"name": "Motor1", "tagType": "UdtInstance", "typeId": "Motors/Motor"}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Resource(&UnknownUdtType)],
    options: None,
};

struct UnknownUdtType;

impl ResourceRule for UnknownUdtType {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let ModelData::Tags(folder) = data else {
            return;
        };
        let Some(index) = cx.index() else {
            return;
        };
        for tag in &folder.tags {
            if !matches!(tag.tag_type, TagType::UdtInstance | TagType::UdtType) {
                continue;
            }
            let Some(type_id) = tag.type_id else {
                continue;
            };
            let raw = type_id.value();
            if raw.trim().is_empty() || raw.contains('{') {
                continue;
            }
            let (provider, path) = definition_path(raw);
            let provider = provider.unwrap_or(folder.provider);
            match index.udt_existence(provider, path) {
                Existence::Missing => {
                    cx.report_in(
                        tag.file,
                        type_id.range(),
                        format!("UDT definition `{path}` does not exist in provider `{provider}`"),
                    )
                    .node_path(&tag.full_path);
                }
                Existence::Exists
                    if tag.tag_type == TagType::UdtType
                        && folder.definitions
                        && in_cycle(index, folder.provider, &tag.path) =>
                {
                    cx.report_in(
                        tag.file,
                        type_id.range(),
                        format!("UDT definition `{}` inherits from itself", tag.path),
                    )
                    .node_path(&tag.full_path);
                }
                _ => {}
            }
        }
    }
}

/// Whether following parent definitions from `path` comes back to it.
fn in_cycle(index: &iglint_project::ProjectIndex<'_>, provider: &str, path: &str) -> bool {
    let start = path.to_lowercase();
    let mut at = path.to_owned();
    for _ in 0..64 {
        let Some(udt) = index.udt(provider, &at) else {
            return false;
        };
        let Some(parent) = &udt.parent else {
            return false;
        };
        let (_, parent) = definition_path(parent);
        if parent.is_empty() {
            return false;
        }
        if parent.to_lowercase() == start {
            return true;
        }
        at = parent.to_owned();
    }
    false
}

#[cfg(test)]
mod tests {
    use crate::rules::tag::fixtures::*;
    use crate::rules::test_support::*;

    const MOTOR: &str =
        r#"[{"name": "Motor", "tagType": "UdtType", "parameters": {}, "tags": []}]"#;

    #[test]
    fn unknown_and_known_types() {
        let f = tags(
            udts(gateway(), MOTOR),
            "[]",
            r#"[{"name": "M1", "tagType": "UdtInstance", "typeId": "Motor"},
                {"name": "M2", "tagType": "UdtInstance", "typeId": "Motr"},
                {"name": "M3", "tagType": "UdtInstance", "typeId": "[default]_types_/Motor"}]"#,
        );
        let d = f.lint(&["TAG001"]);
        assert_eq!(flagged(&d), vec!["\"Motr\""]);
        assert_eq!(d[0].node_path.as_deref(), Some("[default]M2"));
    }

    #[test]
    fn providers_without_definitions_are_unknown() {
        let f = tags(
            gateway(),
            "[]",
            r#"[{"name": "M", "tagType": "UdtInstance", "typeId": "X"}]"#,
        );
        assert!(f.lint(&["TAG001"]).is_empty());
    }

    #[test]
    fn inheritance_cycles() {
        let f = udts(
            gateway(),
            r#"[{"name": "A", "tagType": "UdtType", "typeId": "B", "tags": []},
                {"name": "B", "tagType": "UdtType", "typeId": "A", "tags": []},
                {"name": "C", "tagType": "UdtType", "typeId": "A", "tags": []}]"#,
        );
        let d = f.lint(&["TAG001"]);
        assert_eq!(
            messages(&d).into_iter().map(|m| m.1).collect::<Vec<_>>(),
            vec![
                "UDT definition `A` inherits from itself".to_owned(),
                "UDT definition `B` inherits from itself".to_owned(),
            ]
        );
    }
}
