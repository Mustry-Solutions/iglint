//! Tags and UDTs (`ignition/tag-definition`, `ignition/tag-type-definition`,
//! format spec 9).

use std::borrow::Cow;

use iglint_expression::ReferenceContext;
use iglint_json::{JsonString, Value, ValueKind};
use iglint_text::{TextRange, TextSize};

use crate::expression::{EmbeddedExpression, ExpressionKind};
use crate::ids::{ExprId, FileId, ScriptId, TagId};
use crate::kinds::RunScope;
use crate::node_path::at;
use crate::script::{EmbeddedScript, Owner, ScriptKind, TagEventKind};
use crate::view::ModelProblem;

/// `tagType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TagType {
    AtomicTag,
    Folder,
    UdtInstance,
    UdtType,
    /// Missing or another value.
    Other,
}

impl TagType {
    /// Parses `tagType`.
    pub fn parse(s: Option<&str>) -> TagType {
        match s {
            Some("AtomicTag") => TagType::AtomicTag,
            Some("Folder") => TagType::Folder,
            Some("UdtInstance") => TagType::UdtInstance,
            Some("UdtType") => TagType::UdtType,
            _ => TagType::Other,
        }
    }
}

/// A tag folder resource: the folder's own config and the tags of its
/// listed data files.
#[derive(Debug)]
pub struct TagFolder<'a> {
    /// Tag provider name (first resource path component).
    pub provider: &'a str,
    /// Folder path inside the provider (or inside `_types_`), `/`-separated,
    /// empty for the provider root.
    pub folder: &'a str,
    /// True for `tag-type-definition` (UDT definitions).
    pub definitions: bool,
    /// `attributes.config` of the folder manifest.
    pub config: Option<&'a Value<'a>>,
    /// Data files that were modelled, with whether the manifest lists them.
    pub files: Vec<(FileId, bool)>,
    /// All tags, depth first in file order.
    pub tags: Vec<Tag<'a>>,
    pub scripts: Vec<EmbeddedScript<'a>>,
    pub expressions: Vec<EmbeddedExpression<'a>>,
    pub problems: Vec<ModelProblem>,
}

impl<'a> TagFolder<'a> {
    /// A tag by id.
    pub fn tag(&self, id: TagId) -> &Tag<'a> {
        &self.tags[id.index()]
    }

    /// The `[provider]folder` prefix of tag paths in this folder.
    pub fn base_path(&self) -> String {
        let types = if self.definitions { "_types_" } else { "" };
        let mut s = format!("[{}]{types}", self.provider);
        if !self.folder.is_empty() {
            if self.definitions {
                s.push('/');
            }
            s.push_str(self.folder);
        }
        s
    }
}

/// One tag object (atomic tag, folder, UDT instance, UDT definition or
/// member).
#[derive(Debug)]
pub struct Tag<'a> {
    pub id: TagId,
    pub parent: Option<TagId>,
    pub children: Vec<TagId>,
    pub file: FileId,
    pub value: &'a Value<'a>,
    pub name: &'a str,
    pub name_value: Option<&'a Value<'a>>,
    pub tag_type: TagType,
    /// Path inside the provider (or inside `_types_` for definitions), e.g.
    /// `Plant/Line1/Motor1/Speed` or `Motors/Motor`.
    pub path: String,
    /// `[provider]path`, or `[provider]_types_/path` for definitions.
    pub full_path: String,
    /// `typeId`: the UDT of an instance, or the parent of a UDT definition.
    pub type_id: Option<&'a JsonString<'a>>,
    pub value_source: Option<&'a str>,
    pub data_type: Option<&'a str>,
    pub enabled: Option<bool>,
    /// True for tags inside a UDT definition (members).
    pub in_definition: bool,
    pub parameters: Vec<UdtParameter<'a>>,
    pub event_scripts: Vec<TagEventScript<'a>>,
    pub alarms: Vec<Alarm<'a>>,
    /// Properties bound with `{bindType, binding}` objects.
    pub bound_properties: Vec<BoundProperty<'a>>,
    pub expression: Option<ExprId>,
    pub derive_getter: Option<ExprId>,
    pub derive_setter: Option<ExprId>,
    pub source_tag_path: Option<&'a JsonString<'a>>,
    pub datasource: Option<&'a JsonString<'a>>,
    pub history_enabled: Option<bool>,
    pub history_provider: Option<&'a JsonString<'a>>,
    pub tag_group: Option<&'a JsonString<'a>>,
}

impl Tag<'_> {
    /// A literal string property.
    pub fn string(&self, key: &str) -> Option<&str> {
        self.value.get(key)?.as_str()
    }
}

/// A UDT parameter (`parameters.<name>`).
#[derive(Debug)]
pub struct UdtParameter<'a> {
    pub name: &'a JsonString<'a>,
    pub value: &'a Value<'a>,
    pub data_type: Option<&'a str>,
    /// The parameter value (`value` of the object form, or the bare value).
    pub default: Option<&'a Value<'a>>,
}

/// One `eventScripts` entry.
#[derive(Debug)]
pub struct TagEventScript<'a> {
    pub value: &'a Value<'a>,
    pub event_id: Option<&'a JsonString<'a>>,
    /// `None` for an unknown event id.
    pub kind: Option<TagEventKind>,
    pub enabled: Option<bool>,
    pub script: Option<ScriptId>,
}

/// An alarm.
#[derive(Debug)]
pub struct Alarm<'a> {
    pub value: &'a Value<'a>,
    pub name: Option<&'a JsonString<'a>>,
    pub mode: Option<&'a str>,
    pub priority: Option<&'a str>,
    pub enabled: Option<bool>,
    /// Properties bound with `{bindType, value}` objects.
    pub bindings: Vec<AlarmBinding<'a>>,
    /// `activePipeline`, `clearPipeline`, `ackPipeline` with literal values.
    pub pipelines: Vec<(&'a str, &'a JsonString<'a>)>,
    pub node_path: String,
}

/// An alarm property bound to a tag, expression or UDT parameter.
#[derive(Debug)]
pub struct AlarmBinding<'a> {
    pub property: &'a JsonString<'a>,
    pub value: &'a Value<'a>,
    /// `Tag`, `Expression`, `UDTParameter` (anything else is rejected).
    pub bind_type: Option<&'a JsonString<'a>>,
    pub bound_value: Option<&'a Value<'a>>,
    pub expression: Option<ExprId>,
}

/// A tag property bound with `{bindType, binding}` (UDT parameters).
#[derive(Debug)]
pub struct BoundProperty<'a> {
    pub property: &'a JsonString<'a>,
    pub value: &'a Value<'a>,
    /// `parameter` (text substitution) or `Expression`.
    pub bind_type: Option<&'a str>,
    pub binding: Option<&'a JsonString<'a>>,
    /// `{Name}` placeholders in a `parameter` binding with the file range of
    /// each name.
    pub placeholders: Vec<(&'a str, TextRange)>,
    pub expression: Option<ExprId>,
}

/// Built-in UDT parameters available to `{...}` placeholders.
pub const BUILTIN_UDT_PARAMETERS: &[&str] = &[
    "InstanceName",
    "PathToParentFolder",
    "PathToTag",
    "TagName",
    "ParentInstanceName",
    "RootInstanceName",
];

pub(crate) struct TagBuilder<'a> {
    pub folder: TagFolder<'a>,
}

const RESERVED: &[&str] = &["parameters", "tags", "alarms", "eventScripts"];

impl<'a> TagBuilder<'a> {
    pub(crate) fn new(
        provider: &'a str,
        folder: &'a str,
        definitions: bool,
        config: Option<&'a Value<'a>>,
    ) -> Self {
        TagBuilder {
            folder: TagFolder {
                provider,
                folder,
                definitions,
                config,
                files: Vec::new(),
                tags: Vec::new(),
                scripts: Vec::new(),
                expressions: Vec::new(),
                problems: Vec::new(),
            },
        }
    }

    pub(crate) fn add_file(&mut self, file: FileId, root: &'a Value<'a>, listed: bool) {
        self.folder.files.push((file, listed));
        let Some(list) = root.as_array() else {
            let base = self.folder.base_path();
            self.skipped(root, "tag file is not a JSON array", &base);
            return;
        };
        let base = self.folder.folder;
        for t in list.iter() {
            self.tag(file, t, None, base, self.folder.definitions);
        }
    }

    fn problem(&mut self, range: TextRange, message: &str, node_path: &str) {
        self.folder.problems.push(ModelProblem {
            range,
            message: message.to_owned(),
            node_path: node_path.to_owned(),
            hides_code: false,
            file: self.folder.files.last().map(|(f, _)| *f),
        });
    }

    /// A problem with `value`, which the model skips with any tag event
    /// scripts or expressions in it.
    fn skipped(&mut self, value: &Value<'_>, message: &str, node_path: &str) {
        self.folder.problems.push(ModelProblem {
            range: value.range,
            message: message.to_owned(),
            node_path: node_path.to_owned(),
            hides_code: crate::view::contains_string(value),
            file: self.folder.files.last().map(|(f, _)| *f),
        });
    }

    fn expr(
        &mut self,
        kind: ExpressionKind,
        s: &'a JsonString<'a>,
        tag: TagId,
        node_path: String,
    ) -> Option<ExprId> {
        let e = EmbeddedExpression::new(
            kind,
            s,
            ReferenceContext::Gateway,
            Owner::Tag(tag),
            None,
            node_path,
            None,
            false,
        )?;
        let id = ExprId::from_index(self.folder.expressions.len());
        self.folder.expressions.push(e);
        Some(id)
    }

    fn tag(
        &mut self,
        file: FileId,
        value: &'a Value<'a>,
        parent: Option<TagId>,
        parent_path: &str,
        in_definition: bool,
    ) {
        if value.as_object().is_none() {
            self.skipped(value, "tag is not a JSON object", parent_path);
            return;
        }
        let id = TagId::from_index(self.folder.tags.len());
        let name_value = value.get("name");
        let name: &'a str = match name_value.map(|v| &v.kind) {
            Some(ValueKind::String(s)) => {
                let s: &'a JsonString<'a> = s;
                s.value()
            }
            _ => "",
        };
        let path = if parent_path.is_empty() {
            name.to_owned()
        } else {
            format!("{parent_path}/{name}")
        };
        let full_path = if self.folder.definitions {
            format!("[{}]_types_/{path}", self.folder.provider)
        } else {
            format!("[{}]{path}", self.folder.provider)
        };
        match name_value {
            None => self.problem(value.range, "tag has no `name`", &full_path),
            Some(v) if v.as_str().is_none() => {
                self.problem(v.range, "tag `name` is not a string", &full_path)
            }
            Some(v) if name.is_empty() => {
                self.problem(v.range, "tag has an empty `name`", &full_path)
            }
            Some(_) => {}
        }
        let tag_type_str = value.get("tagType").and_then(Value::as_str);
        let tag_type = TagType::parse(tag_type_str);
        let s = |k: &str| value.get(k).and_then(Value::as_json_string);
        self.folder.tags.push(Tag {
            id,
            parent,
            children: Vec::new(),
            file,
            value,
            name,
            name_value,
            tag_type,
            path: path.clone(),
            full_path: full_path.clone(),
            type_id: s("typeId"),
            value_source: value.get("valueSource").and_then(Value::as_str),
            data_type: value.get("dataType").and_then(Value::as_str),
            enabled: value.get("enabled").and_then(Value::as_bool),
            in_definition,
            parameters: Vec::new(),
            event_scripts: Vec::new(),
            alarms: Vec::new(),
            bound_properties: Vec::new(),
            expression: None,
            derive_getter: None,
            derive_setter: None,
            source_tag_path: s("sourceTagPath"),
            datasource: s("datasource"),
            history_enabled: value.get("historyEnabled").and_then(Value::as_bool),
            history_provider: s("historyProvider"),
            tag_group: s("tagGroup"),
        });
        if let Some(p) = parent {
            self.folder.tags[p.index()].children.push(id);
        }

        // Expressions.
        let expression = s("expression").and_then(|e| {
            self.expr(
                ExpressionKind::TagExpression,
                e,
                id,
                format!("{full_path}.expression"),
            )
        });
        let getter = s("deriveExpressionGetter").and_then(|e| {
            self.expr(
                ExpressionKind::DeriveGetter,
                e,
                id,
                format!("{full_path}.deriveExpressionGetter"),
            )
        });
        let setter = s("deriveExpressionSetter").and_then(|e| {
            self.expr(
                ExpressionKind::DeriveSetter,
                e,
                id,
                format!("{full_path}.deriveExpressionSetter"),
            )
        });

        // Parameters.
        let mut parameters = Vec::new();
        if let Some(params) = value.get("parameters").and_then(Value::as_object) {
            for m in params.iter() {
                let (data_type, default) = match &m.value.kind {
                    ValueKind::Object(_) => (
                        m.value.get("dataType").and_then(Value::as_str),
                        m.value.get("value"),
                    ),
                    _ => (None, Some(&m.value)),
                };
                parameters.push(UdtParameter {
                    name: &m.key,
                    value: &m.value,
                    data_type,
                    default,
                });
            }
        }

        // Bound properties.
        let mut bound_properties = Vec::new();
        if let Some(obj) = value.as_object() {
            for m in obj.iter() {
                if RESERVED.contains(&m.key.value()) || m.value.get("bindType").is_none() {
                    continue;
                }
                let bind_type = m.value.get("bindType").and_then(Value::as_str);
                let binding = m.value.get("binding").and_then(Value::as_json_string);
                let mut placeholders = Vec::new();
                let mut expression = None;
                if let Some(b) = binding {
                    if bind_type.is_some_and(|t| t.eq_ignore_ascii_case("expression")) {
                        expression = self.expr(
                            ExpressionKind::TagProperty,
                            b,
                            id,
                            format!("{full_path}.{}", m.key.value()),
                        );
                    } else {
                        placeholders = udt_placeholders(b);
                    }
                }
                bound_properties.push(BoundProperty {
                    property: &m.key,
                    value: &m.value,
                    bind_type,
                    binding,
                    placeholders,
                    expression,
                });
            }
        }

        // Event scripts.
        let mut event_scripts = Vec::new();
        if let Some(list) = value.get("eventScripts").and_then(Value::as_array) {
            for e in list.iter() {
                let event_id = e.get("eventid").and_then(Value::as_json_string);
                let kind = event_id.and_then(|i| TagEventKind::from_event_id(i.value()));
                let enabled = e.get("enabled").and_then(Value::as_bool);
                let mut script = None;
                if let (Some(kind), Some(member)) =
                    (kind, e.as_object().and_then(|o| o.member("script")))
                    && let Some(body) = member.value.as_json_string()
                {
                    let sid = ScriptId::from_index(self.folder.scripts.len());
                    self.folder.scripts.push(EmbeddedScript::from_json(
                        ScriptKind::TagEvent(kind),
                        Cow::Borrowed(kind.event_id()),
                        kind.params().iter().map(|p| Cow::Borrowed(*p)).collect(),
                        kind.header(),
                        body,
                        member.key.range(),
                        Owner::Tag(id),
                        at(&full_path, &format!("eventScripts.{}", kind.event_id())),
                        None,
                        enabled != Some(false),
                        RunScope::Tag,
                    ));
                    script = Some(sid);
                }
                event_scripts.push(TagEventScript {
                    value: e,
                    event_id,
                    kind,
                    enabled,
                    script,
                });
            }
        }

        // Alarms.
        let mut alarms = Vec::new();
        if let Some(list) = value.get("alarms").and_then(Value::as_array) {
            for a in list.iter() {
                let alarm_name = a.get("name").and_then(Value::as_json_string);
                let node_path = at(
                    &full_path,
                    &format!("alarms.{}", alarm_name.map_or("", |n| n.value())),
                );
                let mut bindings = Vec::new();
                let mut pipelines = Vec::new();
                if let Some(obj) = a.as_object() {
                    for m in obj.iter() {
                        let key: &'a JsonString<'a> = &m.key;
                        if m.value.get("bindType").is_some() {
                            let bind_type = m.value.get("bindType").and_then(Value::as_json_string);
                            let bound_value = m.value.get("value");
                            let expression = if bind_type.is_some_and(|t| t.value() == "Expression")
                            {
                                bound_value.and_then(Value::as_json_string).and_then(|e| {
                                    self.expr(
                                        ExpressionKind::AlarmProperty,
                                        e,
                                        id,
                                        format!("{node_path}.{}", key.value()),
                                    )
                                })
                            } else {
                                None
                            };
                            bindings.push(AlarmBinding {
                                property: key,
                                value: &m.value,
                                bind_type,
                                bound_value,
                                expression,
                            });
                        } else if matches!(
                            key.value(),
                            "activePipeline" | "clearPipeline" | "ackPipeline"
                        ) && let Some(p) = m.value.as_json_string()
                        {
                            pipelines.push((key.value(), p));
                        }
                    }
                }
                alarms.push(Alarm {
                    value: a,
                    name: alarm_name,
                    mode: a.get("mode").and_then(Value::as_str),
                    priority: a.get("priority").and_then(Value::as_str),
                    enabled: a.get("enabled").and_then(Value::as_bool),
                    bindings,
                    pipelines,
                    node_path,
                });
            }
        }

        {
            let t = &mut self.folder.tags[id.index()];
            t.expression = expression;
            t.derive_getter = getter;
            t.derive_setter = setter;
            t.parameters = parameters;
            t.bound_properties = bound_properties;
            t.event_scripts = event_scripts;
            t.alarms = alarms;
        }

        // Children.
        if let Some(children) = value.get("tags") {
            match children.as_array() {
                Some(list) => {
                    let child_in_def = in_definition || tag_type == TagType::UdtType;
                    for c in list.iter() {
                        self.tag(file, c, Some(id), &path, child_in_def);
                    }
                }
                None => {
                    if !children.is_null() {
                        self.skipped(children, "`tags` is not an array", &full_path);
                    }
                }
            }
        }
    }
}

/// `{Name}` placeholders in a parameter binding, with file ranges of the
/// names.
fn udt_placeholders<'a>(s: &'a JsonString<'a>) -> Vec<(&'a str, TextRange)> {
    let text: &'a str = s.value();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some((before, after)) = text.get(from..).and_then(|t| t.split_once('{')) {
        let start = from + before.len() + 1;
        let Some(close) = after.find(['}', '{']) else {
            break;
        };
        if after.as_bytes()[close] == b'{' {
            from = start + close;
            continue;
        }
        let name = after.get(..close).unwrap_or_default();
        if !name.is_empty() {
            let r = TextRange::at(TextSize::new(start as u32), TextSize::of(name));
            out.push((name, s.raw_range(r)));
        }
        from = start + close + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders() {
        let text = r#"["ns\u003d1;s\u003d[{DeviceName}]{BaseAddress}.Speed"]"#;
        let root = iglint_json::parse(text).unwrap();
        let s = root.index(0).unwrap().as_json_string().unwrap();
        let p = udt_placeholders(s);
        let names: Vec<_> = p.iter().map(|(n, r)| (*n, r.slice(text))).collect();
        assert_eq!(
            names,
            vec![("DeviceName", "DeviceName"), ("BaseAddress", "BaseAddress")]
        );
    }
}
