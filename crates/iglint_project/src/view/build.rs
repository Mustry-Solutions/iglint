//! Building [`View`] and [`SessionProps`] from parsed JSON.

use std::borrow::Cow;

use iglint_expression::{ReferenceContext, parse_property_ref, parse_tag_path};
use iglint_json::{JsonString, Value, ValueKind};
use iglint_text::{TextRange, TextSize};

use super::*;
use crate::expression::{EmbeddedExpression, ExpressionKind};
use crate::ids::{BindingId, ComponentId, ExprId, PropConfigId, ScriptId};
use crate::kinds::RunScope;
use crate::node_path::{at, ign_index, ign_inject, ign_key, prop_path, strip_indices};
use crate::perspective_fields::{FieldOwner, FieldSink, fields_of};
use crate::script::{EmbeddedScript, Owner, ScriptKind, header_line, perspective_header};

/// Builds the model of a `view.json` document.
pub fn build_view<'a>(root: &'a Value<'a>) -> View<'a> {
    let mut b = Builder::default();
    let mut view = View {
        root,
        params: None,
        custom: None,
        props: None,
        prop_config: None,
        events_value: None,
        permissions: None,
        components: Vec::new(),
        properties: Vec::new(),
        prop_configs: Vec::new(),
        bindings: Vec::new(),
        events: Vec::new(),
        message_handlers: Vec::new(),
        custom_methods: Vec::new(),
        extension_functions: Vec::new(),
        scripts: Vec::new(),
        expressions: Vec::new(),
        view_refs: Vec::new(),
        style_refs: Vec::new(),
        problems: Vec::new(),
        index: Default::default(),
    };
    if root.as_object().is_none() {
        b.skipped(root, "view.json is not a JSON object", "view");
    } else {
        view.params = b.object_member(root, "params", "view");
        view.custom = b.object_member(root, "custom", "view");
        view.props = b.object_member(root, "props", "view");
        view.prop_config = b.object_member(root, "propConfig", "view");
        view.events_value = b.object_member(root, "events", "view");
        view.permissions = root.get("permissions");

        b.prop_configs_of(
            Owner::View,
            view.prop_config,
            "view",
            "",
            RunScope::PerspectiveComponent,
        );
        b.property_defs(Owner::View, PropCategory::Params, view.params, "view", "");
        b.property_defs(Owner::View, PropCategory::Custom, view.custom, "view", "");
        b.events_of(Owner::View, view.events_value, "view", "");
        match root.get("root") {
            Some(r) if r.as_object().is_some() => {
                b.component(
                    r,
                    None,
                    0,
                    0,
                    String::new(),
                    "/root".to_owned(),
                    "root".to_owned(),
                );
            }
            Some(r) => b.skipped(r, "`root` is not a component object", "view"),
            None => {}
        }
    }
    view.components = b.components;
    view.properties = b.properties;
    view.prop_configs = b.prop_configs;
    view.bindings = b.bindings;
    view.events = b.events;
    view.message_handlers = b.message_handlers;
    view.custom_methods = b.custom_methods;
    view.extension_functions = b.extension_functions;
    view.scripts = b.scripts;
    view.expressions = b.expressions;
    view.view_refs = b.view_refs;
    view.style_refs = b.style_refs;
    view.problems = b.problems;
    view.index = super::OwnerIndex::build(&view);
    view
}

/// Builds the model of a `session-props/props.json` document.
pub(crate) fn build_session_props<'a>(root: &'a Value<'a>) -> SessionProps<'a> {
    let mut b = Builder::default();
    let mut custom = None;
    let mut props = None;
    let mut prop_config = None;
    if root.as_object().is_none() {
        b.skipped(root, "props.json is not a JSON object", "session");
    } else {
        custom = b.object_member(root, "custom", "session");
        props = b.object_member(root, "props", "session");
        prop_config = b.object_member(root, "propConfig", "session");
        b.prop_configs_of(
            Owner::Session,
            prop_config,
            "session",
            "",
            RunScope::PerspectiveSession,
        );
        b.property_defs(Owner::Session, PropCategory::Custom, custom, "session", "");
    }
    SessionProps {
        root,
        custom,
        props,
        prop_config,
        properties: b.properties,
        prop_configs: b.prop_configs,
        bindings: b.bindings,
        scripts: b.scripts,
        expressions: b.expressions,
        style_refs: b.style_refs,
        problems: b.problems,
    }
}

#[derive(Default)]
struct Builder<'a> {
    components: Vec<Component<'a>>,
    properties: Vec<PropertyDef<'a>>,
    prop_configs: Vec<PropConfig<'a>>,
    /// Indices into `prop_configs` by owner, in order: the lookups per
    /// component stay linear in the size of the view.
    prop_configs_by_owner: rustc_hash::FxHashMap<Owner, Vec<usize>>,
    bindings: Vec<Binding<'a>>,
    events: Vec<EventHandler<'a>>,
    message_handlers: Vec<MessageHandler<'a>>,
    custom_methods: Vec<CustomMethod<'a>>,
    extension_functions: Vec<ExtensionFunctionDef<'a>>,
    scripts: Vec<EmbeddedScript<'a>>,
    expressions: Vec<EmbeddedExpression<'a>>,
    view_refs: Vec<ViewRef<'a>>,
    style_refs: Vec<StyleClassRef<'a>>,
    problems: Vec<ModelProblem>,
}

fn str_of<'a>(v: Option<&'a Value<'a>>) -> Option<&'a str> {
    match &v?.kind {
        ValueKind::String(s) => {
            let s: &'a JsonString<'a> = s;
            Some(s.value())
        }
        _ => None,
    }
}

fn json_str<'a>(v: Option<&'a Value<'a>>) -> Option<&'a JsonString<'a>> {
    match &v?.kind {
        ValueKind::String(s) => Some(s),
        _ => None,
    }
}

fn key_str<'a>(m: &'a iglint_json::Member<'a>) -> &'a str {
    let k: &'a JsonString<'a> = &m.key;
    k.value()
}

impl<'a> Builder<'a> {
    fn problem(&mut self, range: TextRange, message: impl Into<String>, node_path: &str) {
        self.problems.push(ModelProblem {
            range,
            message: message.into(),
            node_path: node_path.to_owned(),
            hides_code: false,
            file: None,
        });
    }

    /// A problem with `value`, a place for scripts that the model skips:
    /// any code in it is not read.
    fn skipped(&mut self, value: &Value<'_>, message: impl Into<String>, node_path: &str) {
        self.problems.push(ModelProblem {
            range: value.range,
            message: message.into(),
            node_path: node_path.to_owned(),
            hides_code: super::contains_string(value),
            file: None,
        });
    }

    /// `obj[key]` when it is an array; reports other types (`None` for a
    /// missing or null member).
    fn array_member(
        &mut self,
        obj: &'a Value<'a>,
        key: &str,
        path: &str,
    ) -> Option<&'a iglint_json::Array<'a>> {
        let v = obj.get(key)?;
        match &v.kind {
            ValueKind::Array(list) => Some(list),
            ValueKind::Null => None,
            _ => {
                self.skipped(v, format!("`{key}` is not an array"), path);
                None
            }
        }
    }

    /// `obj[key]` when it is an object; reports other types.
    fn object_member(
        &mut self,
        obj: &'a Value<'a>,
        key: &str,
        path: &str,
    ) -> Option<&'a Value<'a>> {
        let v = obj.get(key)?;
        if v.as_object().is_some() {
            Some(v)
        } else if v.is_null() {
            None
        } else if matches!(key, "propConfig" | "events" | "scripts") {
            self.skipped(v, format!("`{key}` is not an object"), path);
            None
        } else {
            self.problem(v.range, format!("`{key}` is not an object"), path);
            None
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_expr(
        &mut self,
        kind: ExpressionKind,
        source: &'a JsonString<'a>,
        context: ReferenceContext,
        owner: Owner,
        binding: Option<BindingId>,
        node_path: String,
        ign_lint_path: Option<String>,
        tentative: bool,
    ) -> Option<ExprId> {
        let e = EmbeddedExpression::new(
            kind,
            source,
            context,
            owner,
            binding,
            node_path,
            ign_lint_path,
            tentative,
        )?;
        let id = ExprId::from_index(self.expressions.len());
        self.expressions.push(e);
        Some(id)
    }

    #[allow(clippy::too_many_arguments)]
    fn add_script(
        &mut self,
        kind: ScriptKind,
        function_name: Cow<'a, str>,
        params: Vec<Cow<'a, str>>,
        header: String,
        body: &'a JsonString<'a>,
        anchor: TextRange,
        owner: Owner,
        node_path: String,
        ign_lint_path: Option<String>,
        enabled: bool,
        run_scope: RunScope,
    ) -> ScriptId {
        let id = ScriptId::from_index(self.scripts.len());
        self.scripts.push(EmbeddedScript::from_json(
            kind,
            function_name,
            params,
            header,
            body,
            anchor,
            owner,
            node_path,
            ign_lint_path,
            enabled,
            run_scope,
        ));
        id
    }

    /// A script with one of the fixed Perspective headers. `container` holds
    /// the script string under `key`.
    #[allow(clippy::too_many_arguments)]
    fn fixed_script(
        &mut self,
        kind: ScriptKind,
        container: &'a Value<'a>,
        key: &str,
        owner: Owner,
        node_path: String,
        ign_lint_path: Option<String>,
        enabled: bool,
        run_scope: RunScope,
    ) -> Option<ScriptId> {
        let member = container.as_object()?.member(key)?;
        let Some(body) = member.value.as_json_string() else {
            if !member.value.is_null() {
                // Gson reads a one-element array as its element.
                self.skipped(
                    &member.value,
                    format!("`{key}` is not a string"),
                    &node_path,
                );
            }
            return None;
        };
        let (name, params) = perspective_header(kind);
        let header = header_line(name, params, ", ");
        Some(self.add_script(
            kind,
            Cow::Borrowed(name),
            params.iter().map(|p| Cow::Borrowed(*p)).collect(),
            header,
            body,
            member.key.range(),
            owner,
            node_path,
            ign_lint_path,
            enabled,
            run_scope,
        ))
    }

    // ---- propConfig, bindings, transforms ----

    fn prop_configs_of(
        &mut self,
        owner: Owner,
        prop_config: Option<&'a Value<'a>>,
        owner_path: &str,
        owner_ign: &str,
        scope: RunScope,
    ) {
        let Some(obj) = prop_config.and_then(Value::as_object) else {
            return;
        };
        let base_ign = ign_key(owner_ign, "propConfig");
        for m in obj.iter() {
            let key: &'a JsonString<'a> = &m.key;
            let path = PropPath::parse(key.value());
            let node_path = prop_path(owner_path, key.value());
            let ign = ign_key(&base_ign, key.value());
            if m.value.as_object().is_none() {
                if !m.value.is_null() {
                    self.skipped(&m.value, "propConfig entry is not an object", &node_path);
                }
                continue;
            }
            let entry = &m.value;
            let id = PropConfigId::from_index(self.prop_configs.len());
            let param_direction_value = entry.get("paramDirection");
            let mut pc = PropConfig {
                id,
                owner,
                key,
                path,
                value: entry,
                binding: None,
                on_change: None,
                persistent: entry.get("persistent").and_then(Value::as_bool),
                param_direction: str_of(param_direction_value).and_then(ParamDirection::parse),
                param_direction_value,
                access: str_of(entry.get("access")),
                node_path: node_path.clone(),
                ign_lint_path: ign.clone(),
            };
            if let Some(binding) = entry.get("binding") {
                if binding.as_object().is_some() {
                    pc.binding =
                        Some(self.binding(owner, id, key.value(), binding, &node_path, &ign));
                } else if !binding.is_null() {
                    self.skipped(binding, "`binding` is not an object", &node_path);
                }
            }
            if let Some(on_change) = entry.get("onChange") {
                if on_change.as_object().is_some() {
                    let enabled = on_change.get("enabled").and_then(Value::as_bool);
                    let script = self.fixed_script(
                        ScriptKind::PropertyChange,
                        on_change,
                        "script",
                        owner,
                        at(&node_path, "onChange"),
                        Some(ign_key(&ign, "onChange")),
                        enabled != Some(false),
                        scope,
                    );
                    pc.on_change = Some(OnChange {
                        value: on_change,
                        enabled,
                        script,
                    });
                } else if !on_change.is_null() {
                    self.skipped(on_change, "`onChange` is not an object", &node_path);
                }
            }
            self.prop_configs_by_owner
                .entry(owner)
                .or_default()
                .push(self.prop_configs.len());
            self.prop_configs.push(pc);
        }
    }

    /// The `propConfig` entries of `owner`, in order.
    fn prop_configs_of_owner(
        &self,
        owner: Owner,
    ) -> impl DoubleEndedIterator<Item = &PropConfig<'a>> {
        self.prop_configs_by_owner
            .get(&owner)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|&i| &self.prop_configs[i])
    }

    fn binding(
        &mut self,
        owner: Owner,
        prop_config: PropConfigId,
        property: &'a str,
        value: &'a Value<'a>,
        prop_node: &str,
        ign: &str,
    ) -> BindingId {
        let id = BindingId::from_index(self.bindings.len());
        let node_path = at(prop_node, "binding");
        let type_value = json_str(value.get("type"));
        let kind = type_value.map_or(BindingKind::Unknown, |t| BindingKind::parse(t.value()));
        let config = value.get("config").filter(|c| c.as_object().is_some());
        if let Some(c) = value.get("config")
            && c.as_object().is_none()
            && !c.is_null()
        {
            self.skipped(c, "binding `config` is not an object", &node_path);
        }
        // Reserve the slot so expressions can point at it.
        self.bindings.push(Binding {
            id,
            owner,
            prop_config,
            property,
            value,
            type_value,
            kind,
            config,
            enabled: value.get("enabled").and_then(Value::as_bool),
            typed: BindingConfig::None,
            transforms: Vec::new(),
            node_path: node_path.clone(),
            ign_lint_path: ign.to_owned(),
        });
        let first_expression = self.expressions.len();
        let typed = match config {
            Some(config) => self.binding_config(kind, config, owner, id, prop_node, ign),
            None => BindingConfig::None,
        };
        if let (Some(config), Some(t)) = (config, type_value) {
            self.config_field_expressions(
                FieldOwner::Binding,
                t.value(),
                config,
                first_expression,
                owner,
                id,
                &at(prop_node, "binding"),
                ign,
            );
        }
        let transforms =
            self.transforms(value.get("transforms"), owner, id, prop_node, ign, property);
        let b = &mut self.bindings[id.index()];
        b.typed = typed;
        b.transforms = transforms;
        id
    }

    fn binding_config(
        &mut self,
        kind: BindingKind,
        config: &'a Value<'a>,
        owner: Owner,
        id: BindingId,
        prop_node: &str,
        ign: &str,
    ) -> BindingConfig<'a> {
        let ctx = ReferenceContext::PerspectiveBinding;
        let bnode = at(prop_node, "binding");
        match kind {
            BindingKind::Property => {
                let path = json_str(config.get("path"));
                BindingConfig::Property(PropertyBinding {
                    path,
                    reference: path.and_then(|p| parse_property_ref(p.value(), TextSize::ZERO)),
                    bidirectional: config.get("bidirectional").and_then(Value::as_bool)
                        == Some(true),
                })
            }
            BindingKind::Expr => {
                let expression = json_str(config.get("expression")).and_then(|s| {
                    self.add_expr(
                        ExpressionKind::Binding,
                        s,
                        ctx,
                        owner,
                        Some(id),
                        bnode.clone(),
                        Some(ign.to_owned()),
                        false,
                    )
                });
                BindingConfig::Expr { expression }
            }
            BindingKind::ExprStruct => {
                let mut leaves = Vec::new();
                if let Some(s) = config.get("struct") {
                    self.struct_leaves(s, String::new(), owner, id, prop_node, ign, &mut leaves);
                }
                BindingConfig::ExprStruct {
                    leaves,
                    wait_on_all: config.get("waitOnAll").and_then(Value::as_bool),
                }
            }
            BindingKind::Tag => {
                let mode_value = json_str(config.get("mode"));
                let mode = match mode_value.map(|m| m.value()) {
                    None | Some("direct") => TagMode::Direct,
                    Some("indirect") => TagMode::Indirect,
                    Some("expression") => TagMode::Expression,
                    Some(_) => TagMode::Other,
                };
                let tag_path = json_str(config.get("tagPath"));
                let mut binding = TagBinding {
                    mode,
                    mode_value,
                    tag_path,
                    parsed_path: None,
                    path_expression: None,
                    placeholders: Vec::new(),
                    references: Vec::new(),
                    bidirectional: config.get("bidirectional").and_then(Value::as_bool)
                        == Some(true),
                    fallback_delay: config.get("fallbackDelay").and_then(Value::as_f64),
                };
                if let Some(tp) = tag_path {
                    match mode {
                        TagMode::Direct => {
                            binding.parsed_path = Some(parse_tag_path(tp.value(), TextSize::ZERO));
                        }
                        TagMode::Expression => {
                            binding.path_expression = self.add_expr(
                                ExpressionKind::TagPathExpression,
                                tp,
                                ctx,
                                owner,
                                Some(id),
                                bnode.clone(),
                                Some(ign.to_owned()),
                                false,
                            );
                        }
                        TagMode::Indirect | TagMode::Other => {}
                    }
                    if mode != TagMode::Expression {
                        binding.placeholders = placeholders(tp);
                    }
                }
                if let Some(refs) = config.get("references").and_then(Value::as_object) {
                    for m in refs.iter() {
                        let reference = m.value.as_str().and_then(|s| {
                            let inner = s.strip_prefix('{')?.strip_suffix('}')?;
                            parse_property_ref(inner, TextSize::new(1))
                        });
                        binding.references.push(IndirectReference {
                            key: &m.key,
                            value: &m.value,
                            reference,
                        });
                    }
                }
                BindingConfig::Tag(binding)
            }
            BindingKind::Query => {
                let mut parameters = Vec::new();
                if let Some(params) = config.get("parameters").and_then(Value::as_object) {
                    for m in params.iter() {
                        let expression = m.value.as_json_string().and_then(|s| {
                            self.add_expr(
                                ExpressionKind::QueryParameter,
                                s,
                                ctx,
                                owner,
                                Some(id),
                                at(prop_node, &format!("binding.parameters.{}", m.key.value())),
                                Some(ign.to_owned()),
                                false,
                            )
                        });
                        parameters.push(QueryParameter {
                            name: &m.key,
                            value: &m.value,
                            expression,
                        });
                    }
                }
                BindingConfig::Query(QueryBinding {
                    query_path: json_str(config.get("queryPath")),
                    return_format: str_of(config.get("returnFormat")),
                    parameters,
                    polling: self.polling(config, owner, id, prop_node, ign),
                })
            }
            BindingKind::TagHistory => {
                let tags = match config.get("tags") {
                    Some(v) => match &v.kind {
                        ValueKind::String(s) => match self.add_expr(
                            ExpressionKind::TagHistoryTags,
                            s,
                            ctx,
                            owner,
                            Some(id),
                            at(prop_node, "binding.tags"),
                            Some(ign.to_owned()),
                            false,
                        ) {
                            Some(e) => TagHistoryTags::Expression(e),
                            None => TagHistoryTags::None,
                        },
                        ValueKind::Array(a) => {
                            let mut list = Vec::new();
                            for (i, t) in a.iter().enumerate() {
                                let path = json_str(t.get("path"));
                                let path_expression = path.and_then(|p| {
                                    self.add_expr(
                                        ExpressionKind::TagHistoryPath,
                                        p,
                                        ctx,
                                        owner,
                                        Some(id),
                                        at(prop_node, &format!("binding.tags[{i}].path")),
                                        Some(ign.to_owned()),
                                        true,
                                    )
                                });
                                list.push(HistoryTag {
                                    value: t,
                                    path,
                                    path_expression,
                                    alias: str_of(t.get("alias")),
                                    aggregate: str_of(t.get("aggregate")),
                                });
                            }
                            TagHistoryTags::List(list)
                        }
                        _ => TagHistoryTags::None,
                    },
                    None => TagHistoryTags::None,
                };
                BindingConfig::TagHistory(TagHistoryBinding {
                    tags,
                    date_range: config.get("dateRange"),
                    return_format: str_of(config.get("returnFormat")),
                    return_size: config.get("returnSize"),
                    aggregate: str_of(config.get("aggregate")),
                    polling: self.polling(config, owner, id, prop_node, ign),
                })
            }
            BindingKind::Http => {
                let request = config.get("request");
                let url = json_str(request.and_then(|r| r.get("url")));
                let url_expression = url.and_then(|u| {
                    self.add_expr(
                        ExpressionKind::HttpUrl,
                        u,
                        ctx,
                        owner,
                        Some(id),
                        at(prop_node, "binding.request.url"),
                        Some(ign.to_owned()),
                        // Always parsed (measured: a literal URL fails with
                        // "Error parsing url: Invalid expression").
                        false,
                    )
                });
                BindingConfig::Http(HttpBinding {
                    url,
                    url_expression,
                    method: str_of(request.and_then(|r| r.get("method"))),
                    auth_type: str_of(
                        request
                            .and_then(|r| r.get("auth"))
                            .and_then(|a| a.get("type")),
                    ),
                    polling: self.polling(config, owner, id, prop_node, ign),
                })
            }
            BindingKind::Unknown => BindingConfig::None,
        }
    }

    /// The measured expression fields of a binding `config` or a
    /// transform ([`crate::perspective_fields`]) that the typed model did
    /// not already read (expressions from `first` on belong to this
    /// binding): HTTP bodies, header and authorization values, tag history
    /// date ranges and return sizes, map transform fallbacks, list items of
    /// expression structures, ...
    #[allow(clippy::too_many_arguments)]
    fn config_field_expressions(
        &mut self,
        field_owner: FieldOwner,
        type_name: &str,
        root: &'a Value<'a>,
        first: usize,
        owner: Owner,
        id: BindingId,
        node: &str,
        ign: &str,
    ) {
        for field in fields_of(field_owner, type_name) {
            if field.sink != FieldSink::Expression || !field.applies(root) {
                continue;
            }
            for (path, s) in field.strings(root) {
                let range = s.range();
                if self.expressions[first..]
                    .iter()
                    .any(|e| e.source.range() == range)
                {
                    continue;
                }
                self.add_expr(
                    ExpressionKind::ConfigField,
                    s,
                    ReferenceContext::PerspectiveBinding,
                    owner,
                    Some(id),
                    format!("{node}.{path}"),
                    Some(format!("{ign}.{path}")),
                    false,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn struct_leaves(
        &mut self,
        value: &'a Value<'a>,
        prefix: String,
        owner: Owner,
        id: BindingId,
        prop_node: &str,
        ign: &str,
        out: &mut Vec<StructLeaf>,
    ) {
        match &value.kind {
            ValueKind::Object(o) => {
                for m in o.iter() {
                    let key = if prefix.is_empty() {
                        m.key.value().to_owned()
                    } else {
                        format!("{prefix}.{}", m.key.value())
                    };
                    self.struct_leaves(&m.value, key, owner, id, prop_node, ign, out);
                }
            }
            ValueKind::String(s) => {
                if let Some(expression) = self.add_expr(
                    ExpressionKind::StructLeaf,
                    s,
                    ReferenceContext::PerspectiveBinding,
                    owner,
                    Some(id),
                    at(prop_node, &format!("binding.struct.{prefix}")),
                    Some(ign.to_owned()),
                    false,
                ) {
                    out.push(StructLeaf {
                        key: prefix,
                        expression,
                    });
                }
            }
            _ => {}
        }
    }

    fn polling(
        &mut self,
        config: &'a Value<'a>,
        owner: Owner,
        id: BindingId,
        prop_node: &str,
        ign: &str,
    ) -> Polling<'a> {
        let Some(polling) = config.get("polling") else {
            return Polling::default();
        };
        let rate = polling.get("rate");
        let rate_expression = json_str(rate).and_then(|s| {
            self.add_expr(
                ExpressionKind::PollingRate,
                s,
                ReferenceContext::PerspectiveBinding,
                owner,
                Some(id),
                at(prop_node, "binding.polling.rate"),
                Some(ign.to_owned()),
                false,
            )
        });
        Polling {
            value: Some(polling),
            enabled: polling.get("enabled").and_then(Value::as_bool),
            rate,
            rate_expression,
        }
    }

    fn transforms(
        &mut self,
        value: Option<&'a Value<'a>>,
        owner: Owner,
        id: BindingId,
        prop_node: &str,
        ign: &str,
        property: &str,
    ) -> Vec<Transform<'a>> {
        let Some(list) = value.and_then(Value::as_array) else {
            return Vec::new();
        };
        let scope = if owner == Owner::Session {
            RunScope::PerspectiveSession
        } else {
            RunScope::PerspectiveComponent
        };
        let ctx = ReferenceContext::PerspectiveTransform;
        let mut out = Vec::with_capacity(list.len());
        for (i, t) in list.iter().enumerate() {
            let first_expression = self.expressions.len();
            let node_path = at(prop_node, &format!("binding.transforms[{i}]"));
            let ign_path = ign_index(&ign_key(ign, "binding.transforms"), i);
            let type_name = str_of(t.get("type"));
            let enabled = t.get("enabled").and_then(Value::as_bool);
            let kind = match type_name {
                Some("script") => TransformKind::Script {
                    script: self.fixed_script(
                        ScriptKind::Transform,
                        t,
                        "code",
                        owner,
                        node_path.clone(),
                        Some(ign_path.clone()),
                        enabled != Some(false),
                        scope,
                    ),
                },
                Some("expression") => TransformKind::Expression {
                    expression: json_str(t.get("expression")).and_then(|s| {
                        self.add_expr(
                            ExpressionKind::Transform,
                            s,
                            ctx,
                            owner,
                            Some(id),
                            node_path.clone(),
                            Some(ign_path.clone()),
                            false,
                        )
                    }),
                },
                Some("map") => {
                    let input_type = str_of(t.get("inputType"));
                    let output_type = str_of(t.get("outputType"));
                    let mut mappings = Vec::new();
                    if let Some(ms) = t.get("mappings").and_then(Value::as_array) {
                        for (j, m) in ms.iter().enumerate() {
                            let input = m.get("input");
                            let output = m.get("output");
                            let input_expression = if input_type == Some("expression") {
                                json_str(input).and_then(|s| {
                                    self.add_expr(
                                        ExpressionKind::MapInput,
                                        s,
                                        ctx,
                                        owner,
                                        Some(id),
                                        format!("{node_path}.mappings[{j}].input"),
                                        Some(format!("{ign_path}.mappings[{j}].input")),
                                        false,
                                    )
                                })
                            } else {
                                None
                            };
                            let output_expression = if output_type == Some("expression") {
                                json_str(output).and_then(|s| {
                                    self.add_expr(
                                        ExpressionKind::MapOutput,
                                        s,
                                        ctx,
                                        owner,
                                        Some(id),
                                        format!("{node_path}.mappings[{j}].output"),
                                        Some(format!("{ign_path}.mappings[{j}].output")),
                                        false,
                                    )
                                })
                            } else {
                                None
                            };
                            if output_type == Some("style-list")
                                && let Some(s) = json_str(output)
                            {
                                self.style_ref(
                                    owner,
                                    StyleRefOrigin::MapOutput,
                                    s,
                                    property.to_owned(),
                                    false,
                                    format!("{node_path}.mappings[{j}].output"),
                                );
                            }
                            mappings.push(Mapping {
                                value: m,
                                input,
                                output,
                                input_expression,
                                output_expression,
                            });
                        }
                    }
                    let fallback = t.get("fallback");
                    if output_type == Some("style-list")
                        && let Some(s) = json_str(fallback)
                    {
                        self.style_ref(
                            owner,
                            StyleRefOrigin::MapOutput,
                            s,
                            property.to_owned(),
                            false,
                            format!("{node_path}.fallback"),
                        );
                    }
                    TransformKind::Map(MapTransform {
                        input_type,
                        output_type,
                        mappings,
                        fallback,
                    })
                }
                Some("format") => TransformKind::Format(FormatTransform {
                    format_type: str_of(t.get("formatType")),
                    format_value: str_of(t.get("formatValue")),
                    locale: str_of(t.get("locale")),
                    timezone: str_of(t.get("timezone")),
                }),
                _ => TransformKind::Unknown,
            };
            if let Some(type_name) = type_name {
                self.config_field_expressions(
                    FieldOwner::Transform,
                    type_name,
                    t,
                    first_expression,
                    owner,
                    id,
                    &node_path,
                    &ign_path,
                );
            }
            out.push(Transform {
                index: i,
                value: t,
                type_name,
                enabled,
                kind,
                node_path,
                ign_lint_path: ign_path,
            });
        }
        out
    }

    // ---- property definitions ----

    fn property_defs(
        &mut self,
        owner: Owner,
        category: PropCategory,
        object: Option<&'a Value<'a>>,
        owner_path: &str,
        owner_ign: &str,
    ) {
        let cat = category.name();
        let mut names: Vec<(&'a str, Option<&'a JsonString<'a>>, Option<&'a Value<'a>>)> =
            Vec::new();
        // Position of each name in `names`.
        let mut seen: rustc_hash::FxHashMap<&'a str, usize> = Default::default();
        if let Some(o) = object.and_then(Value::as_object) {
            for m in o.iter() {
                let name = key_str(m);
                // Duplicate keys: the last one wins, as in Gson.
                match seen.get(name) {
                    Some(&i) => names[i] = (name, Some(&m.key), Some(&m.value)),
                    None => {
                        seen.insert(name, names.len());
                        names.push((name, Some(&m.key), Some(&m.value)));
                    }
                }
            }
        }
        // The last entry of each key.
        let mut last: rustc_hash::FxHashMap<&'a str, (PropConfigId, Option<ParamDirection>)> =
            Default::default();
        for pc in self.prop_configs_of_owner(owner) {
            if pc.path.category == cat
                && !pc.path.name.is_empty()
                && !seen.contains_key(pc.path.name)
            {
                seen.insert(pc.path.name, names.len());
                names.push((pc.path.name, None, None));
            }
            last.insert(pc.key.value(), (pc.id, pc.param_direction));
        }
        for (name, key, value) in names {
            let exact = format!("{cat}.{name}");
            let config = last.get(exact.as_str()).copied();
            self.properties.push(PropertyDef {
                owner,
                category,
                name,
                key,
                value,
                config: config.map(|(id, _)| id),
                direction: config.and_then(|(_, direction)| direction),
                node_path: prop_path(owner_path, &exact),
                ign_lint_path: strip_indices(&ign_key(owner_ign, &exact)),
            });
        }
    }

    // ---- events ----

    fn events_of(
        &mut self,
        owner: Owner,
        events: Option<&'a Value<'a>>,
        owner_path: &str,
        owner_ign: &str,
    ) {
        let Some(groups) = events.and_then(Value::as_object) else {
            return;
        };
        let events_ign = ign_key(owner_ign, "events");
        for g in groups.iter() {
            let group = key_str(g);
            let Some(handlers) = g.value.as_object() else {
                if !g.value.is_null() {
                    self.skipped(&g.value, "event group is not an object", owner_path);
                }
                continue;
            };
            for h in handlers.iter() {
                let name = key_str(h);
                let node_path = at(owner_path, &format!("events.{group}.{name}"));
                let ign = ign_key(&ign_key(&events_ign, group), name);
                let mut handler = EventHandler {
                    owner,
                    group,
                    name,
                    key: &h.key,
                    value: &h.value,
                    is_list: false,
                    actions: Vec::new(),
                    node_path: node_path.clone(),
                    ign_lint_path: ign.clone(),
                };
                match &h.value.kind {
                    ValueKind::Object(_) => {
                        let a = self.action(owner, &h.value, None, node_path.clone(), ign.clone());
                        handler.actions.push(a);
                    }
                    ValueKind::Array(list) => {
                        handler.is_list = true;
                        for (i, a) in list.iter().enumerate() {
                            if a.as_object().is_none() {
                                self.skipped(a, "event action is not an object", &node_path);
                                continue;
                            }
                            let action = self.action(
                                owner,
                                a,
                                Some(i),
                                format!("{node_path}[{i}]"),
                                ign_index(&ign, i),
                            );
                            handler.actions.push(action);
                        }
                    }
                    ValueKind::Null => {}
                    _ => self.skipped(
                        &h.value,
                        "event is not an action or a list of actions",
                        &node_path,
                    ),
                }
                self.events.push(handler);
            }
        }
    }

    fn action(
        &mut self,
        owner: Owner,
        value: &'a Value<'a>,
        index: Option<usize>,
        node_path: String,
        ign: String,
    ) -> Action<'a> {
        let type_value = json_str(value.get("type"));
        let config = value.get("config").filter(|c| c.as_object().is_some());
        if let Some(c) = value.get("config")
            && c.as_object().is_none()
            && !c.is_null()
        {
            self.skipped(c, "action `config` is not an object", &node_path);
        }
        let enabled = value.get("enabled").and_then(Value::as_bool);
        let cfg = |key: &str| config.and_then(|c| c.get(key));
        let non_empty = |v: Option<&'a Value<'a>>| json_str(v).filter(|s| !s.value().is_empty());
        let kind = match type_value.map(|t| t.value()) {
            Some("script") => ActionKind::Script {
                script: config.and_then(|c| {
                    self.fixed_script(
                        ScriptKind::EventAction,
                        c,
                        "script",
                        owner,
                        node_path.clone(),
                        Some(ign.clone()),
                        enabled != Some(false),
                        RunScope::PerspectiveComponent,
                    )
                }),
            },
            Some("nav") => {
                let target = if let Some(view) = non_empty(cfg("view")) {
                    NavTarget::View {
                        view,
                        params: cfg("params"),
                    }
                } else if let Some(page) = non_empty(cfg("page")) {
                    NavTarget::Page(page)
                } else if let Some(url) = non_empty(cfg("url")) {
                    NavTarget::Url(url)
                } else {
                    NavTarget::Other
                };
                if let NavTarget::View { view, params } = &target {
                    self.view_refs.push(ViewRef {
                        owner,
                        kind: ViewRefKind::NavigateAction,
                        path: view,
                        params: *params,
                        property: node_path.clone(),
                        bound: false,
                        params_bound: false,
                        node_path: node_path.clone(),
                    });
                }
                ActionKind::Navigate(target)
            }
            Some("popup") => {
                let popup = PopupAction {
                    operation: str_of(cfg("type")),
                    id: json_str(cfg("id")),
                    view_path: non_empty(cfg("viewPath")),
                    view_params: cfg("viewParams"),
                };
                if popup.operation != Some("close")
                    && let Some(path) = popup.view_path
                {
                    self.view_refs.push(ViewRef {
                        owner,
                        kind: ViewRefKind::PopupAction,
                        path,
                        params: popup.view_params,
                        property: node_path.clone(),
                        bound: false,
                        params_bound: false,
                        node_path: node_path.clone(),
                    });
                }
                ActionKind::Popup(popup)
            }
            Some("dock") => ActionKind::Dock(DockAction {
                operation: str_of(cfg("type")),
                id: json_str(cfg("id")),
                view_params: cfg("viewParams"),
            }),
            Some("alter-dock") => {
                let view = non_empty(cfg("view"));
                let view_params = cfg("viewParams");
                if let Some(path) = view {
                    self.view_refs.push(ViewRef {
                        owner,
                        kind: ViewRefKind::AlterDockAction,
                        path,
                        params: view_params,
                        property: node_path.clone(),
                        bound: false,
                        params_bound: false,
                        node_path: node_path.clone(),
                    });
                }
                ActionKind::AlterDock { view, view_params }
            }
            Some("login") => ActionKind::Login,
            Some("logout") => ActionKind::Logout,
            Some("auth-challenge") => ActionKind::AuthChallenge,
            Some("theme") => ActionKind::Theme {
                name: json_str(cfg("name")),
            },
            Some(t) if ACTION_TYPES.contains(&t) => ActionKind::Other,
            _ => ActionKind::Unknown,
        };
        Action {
            index,
            value,
            type_value,
            scope: str_of(value.get("scope")),
            enabled,
            config,
            kind,
            node_path,
            ign_lint_path: ign,
        }
    }

    // ---- components ----

    #[allow(clippy::too_many_arguments)]
    fn component(
        &mut self,
        value: &'a Value<'a>,
        parent: Option<ComponentId>,
        depth: u32,
        index_in_parent: usize,
        parent_path: String,
        pointer: String,
        ign_base: String,
    ) -> ComponentId {
        let id = ComponentId::from_index(self.components.len());
        let meta = value.get("meta").filter(|m| m.as_object().is_some());
        let name_value = meta.and_then(|m| m.get("name"));
        // A root container without `meta.name` is named `root` by
        // Perspective.
        let name = match (str_of(name_value), parent) {
            (Some(n), _) => n,
            (None, None) if name_value.is_none() => "root",
            (None, _) => "",
        };
        let path = if parent.is_none() {
            name.to_owned()
        } else {
            format!("{parent_path}/{name}")
        };
        if name_value.is_none() && parent.is_some() {
            self.problem(value.range, "component has no `meta.name`", &path);
        } else if name_value.is_some() && str_of(name_value).is_none() {
            self.problem(
                name_value.map_or(value.range, |v| v.range),
                "`meta.name` is not a string",
                &path,
            );
        }
        let ign_lint_path = ign_inject(ign_base, value);
        let type_value = value.get("type");
        let type_id = str_of(type_value);
        let props = self.object_member(value, "props", &path);
        let custom = self.object_member(value, "custom", &path);
        let position = self.object_member(value, "position", &path);
        let prop_config = self.object_member(value, "propConfig", &path);
        let events = self.object_member(value, "events", &path);
        let scripts = self.object_member(value, "scripts", &path);
        if value.get("meta").is_some_and(|m| m.as_object().is_none()) {
            self.problem(
                value.get("meta").map_or(value.range, |m| m.range),
                "`meta` is not an object",
                &path,
            );
        }
        self.components.push(Component {
            id,
            parent,
            children: Vec::new(),
            depth,
            index_in_parent,
            value,
            name,
            name_value,
            type_id,
            type_value,
            meta,
            props,
            custom,
            position,
            prop_config,
            events,
            scripts,
            path: path.clone(),
            pointer: pointer.clone(),
            ign_lint_path: ign_lint_path.clone(),
        });
        let owner = Owner::Component(id);
        self.prop_configs_of(
            owner,
            prop_config,
            &path,
            &ign_lint_path,
            RunScope::PerspectiveComponent,
        );
        self.property_defs(owner, PropCategory::Custom, custom, &path, &ign_lint_path);
        self.events_of(owner, events, &path, &ign_lint_path);
        self.component_scripts(owner, scripts, type_id, &path, &ign_lint_path);
        if let Some(props) = props {
            self.style_refs_in(owner, props, "props".to_owned(), false, &path);
            self.component_view_refs(owner, type_id, props, &path);
        }

        if let Some(children) = value.get("children") {
            match &children.kind {
                ValueKind::Array(list) => {
                    let children_ign = ign_key(&ign_lint_path, "children");
                    let mut ids = Vec::with_capacity(list.len());
                    for (i, child) in list.iter().enumerate() {
                        if child.as_object().is_none() {
                            self.skipped(child, "child component is not an object", &path);
                            continue;
                        }
                        let cid = self.component(
                            child,
                            Some(id),
                            depth + 1,
                            i,
                            path.clone(),
                            format!("{pointer}/children/{i}"),
                            ign_index(&children_ign, i),
                        );
                        ids.push(cid);
                    }
                    self.components[id.index()].children = ids;
                }
                ValueKind::Null => {}
                _ => self.skipped(children, "`children` is not an array", &path),
            }
        }
        id
    }

    fn component_scripts(
        &mut self,
        owner: Owner,
        scripts: Option<&'a Value<'a>>,
        type_id: Option<&str>,
        path: &str,
        ign: &str,
    ) {
        let Some(scripts) = scripts else { return };
        let scripts_ign = ign_key(ign, "scripts");
        let scope = RunScope::PerspectiveComponent;
        if let Some(list) = self.array_member(scripts, "messageHandlers", path) {
            let base = ign_key(&scripts_ign, "messageHandlers");
            for (i, h) in list.iter().enumerate() {
                if h.as_object().is_none() {
                    self.skipped(h, "message handler is not an object", path);
                    continue;
                }
                if let Some(t) = h.get("messageType")
                    && t.as_json_string().is_none()
                    && !t.is_null()
                {
                    self.problem(t.range, "`messageType` is not a string", path);
                }
                let message_type = json_str(h.get("messageType"));
                let node_path = at(
                    path,
                    &format!("messageHandlers.{}", message_type.map_or("", |t| t.value())),
                );
                let ign_path = ign_index(&base, i);
                let script = self.fixed_script(
                    ScriptKind::MessageHandler,
                    h,
                    "script",
                    owner,
                    node_path.clone(),
                    Some(ign_path.clone()),
                    true,
                    scope,
                );
                let flag = |k: &str| h.get(k).and_then(Value::as_bool) == Some(true);
                self.message_handlers.push(MessageHandler {
                    owner,
                    index: i,
                    value: h,
                    message_type,
                    page_scope: flag("pageScope"),
                    session_scope: flag("sessionScope"),
                    view_scope: flag("viewScope"),
                    script,
                    node_path,
                    ign_lint_path: ign_path,
                });
            }
        }
        if let Some(list) = self.array_member(scripts, "customMethods", path) {
            let base = ign_key(&scripts_ign, "customMethods");
            for (i, m) in list.iter().enumerate() {
                if m.as_object().is_none() {
                    self.skipped(m, "custom method is not an object", path);
                    continue;
                }
                let name = json_str(m.get("name"));
                if let Some(p) = m.get("params")
                    && p.as_array().is_none()
                    && !p.is_null()
                {
                    self.problem(p.range, "custom method `params` is not an array", path);
                }
                let params: Vec<&'a JsonString<'a>> = m
                    .get("params")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_json_string).collect())
                    .unwrap_or_default();
                let node_path = at(
                    path,
                    &format!("customMethods.{}", name.map_or("", |n| n.value())),
                );
                let ign_path = ign_index(&base, i);
                let mut script = None;
                if let (Some(n), Some(member)) =
                    (name, m.as_object().and_then(|o| o.member("script")))
                {
                    if member.value.as_json_string().is_none() && !member.value.is_null() {
                        self.skipped(
                            &member.value,
                            "custom method `script` is not a string",
                            path,
                        );
                    }
                    if let Some(body) = member.value.as_json_string() {
                        let mut all: Vec<Cow<'a, str>> = vec![Cow::Borrowed("self")];
                        all.extend(params.iter().map(|p| {
                            let p: &'a JsonString<'a> = p;
                            Cow::Borrowed(p.value())
                        }));
                        let refs: Vec<&str> = all.iter().map(|c| c.as_ref()).collect();
                        // `CustomMethodConfig.getSignature`: "self," then the
                        // params joined by ",".
                        let header = if params.is_empty() {
                            format!("def {}(self,):", n.value())
                        } else {
                            header_line(n.value(), &refs, ",")
                        };
                        script = Some(self.add_script(
                            ScriptKind::CustomMethod,
                            Cow::Borrowed(n.value()),
                            all,
                            header,
                            body,
                            member.key.range(),
                            owner,
                            node_path.clone(),
                            Some(ign_path.clone()),
                            true,
                            scope,
                        ));
                    }
                } else if name.is_none() {
                    // Its script, if any, is not read.
                    match m.get("name") {
                        Some(n) if !n.is_null() => {
                            self.skipped(m, "custom method `name` is not a string", path);
                        }
                        _ => self.skipped(m, "custom method has no `name`", path),
                    }
                }
                self.custom_methods.push(CustomMethod {
                    owner,
                    index: i,
                    value: m,
                    name,
                    params,
                    script,
                    node_path,
                    ign_lint_path: ign_path,
                });
            }
        }
        if let Some(list) = self.array_member(scripts, "extensionFunctions", path) {
            let base = ign_key(&scripts_ign, "extensionFunctions");
            let component_type = type_id.and_then(iglint_stubs::component_type);
            for (i, f) in list.iter().enumerate() {
                if f.as_object().is_none() {
                    self.skipped(f, "extension function is not an object", path);
                    continue;
                }
                let name = json_str(f.get("name"));
                let script_value = f.get("script").filter(|v| !v.is_null());
                match (f.get("name").filter(|n| !n.is_null()), name) {
                    (Some(_), None) => {
                        self.skipped(f, "extension function `name` is not a string", path);
                    }
                    (None, _) if script_value.is_some() => {
                        self.skipped(f, "extension function has no `name`", path);
                    }
                    _ => {}
                }
                if let Some(v) = script_value
                    && v.as_json_string().is_none()
                {
                    self.skipped(v, "extension function `script` is not a string", path);
                }
                let enabled = f.get("enabled").and_then(Value::as_bool);
                let node_path = at(
                    path,
                    &format!("extensionFunctions.{}", name.map_or("", |n| n.value())),
                );
                let ign_path = ign_index(&base, i);
                let declared_fn = name.and_then(|n| {
                    component_type
                        .and_then(|t| t.extension_functions.iter().find(|e| e.name == n.value()))
                });
                let declared = match (component_type, name) {
                    (Some(_), Some(_)) => Some(declared_fn.is_some()),
                    _ => None,
                };
                let mut script = None;
                if let (Some(n), Some(member)) =
                    (name, f.as_object().and_then(|o| o.member("script")))
                    && let Some(body) = member.value.as_json_string()
                {
                    let (params, header): (Vec<Cow<'a, str>>, String) = match declared_fn {
                        Some(decl) => {
                            let inner = decl
                                .signature
                                .split_once('(')
                                .and_then(|(_, r)| r.strip_suffix(')'))
                                .unwrap_or("self");
                            (
                                inner
                                    .split(',')
                                    .map(|p| Cow::Borrowed(p.trim()))
                                    .filter(|p| !p.is_empty())
                                    .collect(),
                                format!("def {}:", decl.signature),
                            )
                        }
                        None => (
                            vec![Cow::Borrowed("self")],
                            format!("def {}(self):", n.value()),
                        ),
                    };
                    script = Some(self.add_script(
                        ScriptKind::ExtensionFunction { declared },
                        Cow::Borrowed(n.value()),
                        params,
                        header,
                        body,
                        member.key.range(),
                        owner,
                        node_path.clone(),
                        Some(ign_path.clone()),
                        enabled != Some(false),
                        scope,
                    ));
                }
                self.extension_functions.push(ExtensionFunctionDef {
                    owner,
                    index: i,
                    value: f,
                    name,
                    enabled,
                    declared,
                    script,
                    node_path,
                    ign_lint_path: ign_path,
                });
            }
        }
    }

    // ---- style class and view references ----

    fn style_ref(
        &mut self,
        owner: Owner,
        origin: StyleRefOrigin,
        source: &'a JsonString<'a>,
        property: String,
        bound: bool,
        node_path: String,
    ) {
        let text: &'a str = source.value();
        let mut classes = Vec::new();
        let mut offset = 0usize;
        for part in text.split(|c: char| c.is_ascii_whitespace()) {
            if !part.is_empty() {
                let r = TextRange::at(TextSize::new(offset as u32), TextSize::of(part));
                classes.push((part, source.raw_range(r)));
            }
            offset += part.len() + 1;
        }
        self.style_refs.push(StyleClassRef {
            owner,
            origin,
            source,
            classes,
            property,
            bound,
            node_path,
        });
    }

    /// Finds `classes` strings of style objects below `value`.
    fn style_refs_in(
        &mut self,
        owner: Owner,
        value: &'a Value<'a>,
        path: String,
        in_style: bool,
        owner_path: &str,
    ) {
        match &value.kind {
            ValueKind::Object(o) => {
                for m in o.iter() {
                    let key = key_str(m);
                    let child = format!("{path}.{key}");
                    if in_style && key == "classes" {
                        if let Some(s) = m.value.as_json_string() {
                            let bound = self.bound(owner, &child);
                            let node_path = prop_path(owner_path, &child);
                            self.style_ref(
                                owner,
                                StyleRefOrigin::Property,
                                s,
                                child,
                                bound,
                                node_path,
                            );
                        }
                        continue;
                    }
                    let is_style =
                        key.len() >= 5 && key[key.len() - 5..].eq_ignore_ascii_case("style");
                    if matches!(m.value.kind, ValueKind::Object(_) | ValueKind::Array(_)) {
                        self.style_refs_in(owner, &m.value, child, is_style, owner_path);
                    }
                }
            }
            ValueKind::Array(a) => {
                for (i, e) in a.iter().enumerate() {
                    if matches!(e.kind, ValueKind::Object(_) | ValueKind::Array(_)) {
                        self.style_refs_in(owner, e, format!("{path}[{i}]"), false, owner_path);
                    }
                }
            }
            _ => {}
        }
    }

    fn bound(&self, owner: Owner, path: &str) -> bool {
        self.prop_configs_of_owner(owner)
            .any(|c| c.binding.is_some() && path_covers(c.key.value(), path))
    }

    fn bound_within(&self, owner: Owner, path: &str) -> bool {
        self.prop_configs_of_owner(owner).any(|c| {
            c.binding.is_some()
                && (path_covers(c.key.value(), path) || path_covers(path, c.key.value()))
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn push_view_ref(
        &mut self,
        owner: Owner,
        kind: ViewRefKind,
        path_value: Option<&'a Value<'a>>,
        path_prop: String,
        params: Option<&'a Value<'a>>,
        params_prop: Option<String>,
        owner_path: &str,
    ) {
        let Some(path) = json_str(path_value).filter(|s| !s.value().is_empty()) else {
            return;
        };
        let bound = self.bound(owner, &path_prop);
        let params_bound = params_prop.is_some_and(|p| self.bound_within(owner, &p));
        self.view_refs.push(ViewRef {
            owner,
            kind,
            path,
            params,
            node_path: prop_path(owner_path, &path_prop),
            property: path_prop,
            bound,
            params_bound,
        });
    }

    fn component_view_refs(
        &mut self,
        owner: Owner,
        type_id: Option<&str>,
        props: &'a Value<'a>,
        owner_path: &str,
    ) {
        let Some(type_id) = type_id else { return };
        let list = |key: &str| props.get(key).and_then(Value::as_array);
        match type_id {
            "ia.display.view" => self.push_view_ref(
                owner,
                ViewRefKind::EmbeddedView,
                props.get("path"),
                "props.path".into(),
                props.get("params"),
                Some("props.params".into()),
                owner_path,
            ),
            "ia.display.flex-repeater" => self.push_view_ref(
                owner,
                ViewRefKind::FlexRepeater,
                props.get("path"),
                "props.path".into(),
                props.get("instances"),
                Some("props.instances".into()),
                owner_path,
            ),
            "ia.display.viewcanvas" => {
                for (i, inst) in list("instances").into_iter().flatten().enumerate() {
                    self.push_view_ref(
                        owner,
                        ViewRefKind::ViewCanvas,
                        inst.get("viewPath"),
                        format!("props.instances[{i}].viewPath"),
                        inst.get("viewParams"),
                        Some(format!("props.instances[{i}].viewParams")),
                        owner_path,
                    );
                }
            }
            "ia.display.carousel" => {
                for (i, v) in list("views").into_iter().flatten().enumerate() {
                    self.push_view_ref(
                        owner,
                        ViewRefKind::Carousel,
                        v.get("viewPath"),
                        format!("props.views[{i}].viewPath"),
                        v.get("viewParams"),
                        Some(format!("props.views[{i}].viewParams")),
                        owner_path,
                    );
                }
            }
            "ia.display.accordion" => {
                for (i, item) in list("items").into_iter().flatten().enumerate() {
                    let body = item.get("body");
                    self.push_view_ref(
                        owner,
                        ViewRefKind::Accordion,
                        body.and_then(|b| b.get("viewPath")),
                        format!("props.items[{i}].body.viewPath"),
                        body.and_then(|b| b.get("viewParams")),
                        Some(format!("props.items[{i}].body.viewParams")),
                        owner_path,
                    );
                    let content = item.get("header").and_then(|h| h.get("content"));
                    self.push_view_ref(
                        owner,
                        ViewRefKind::Accordion,
                        content.and_then(|c| c.get("viewPath")),
                        format!("props.items[{i}].header.content.viewPath"),
                        content.and_then(|c| c.get("viewParams")),
                        Some(format!("props.items[{i}].header.content.viewParams")),
                        owner_path,
                    );
                }
            }
            "ia.display.dashboard" => {
                for key in ["widgets", "availableWidgets"] {
                    for (i, w) in list(key).into_iter().flatten().enumerate() {
                        self.push_view_ref(
                            owner,
                            ViewRefKind::Dashboard,
                            w.get("viewPath"),
                            format!("props.{key}[{i}].viewPath"),
                            w.get("viewParams"),
                            Some(format!("props.{key}[{i}].viewParams")),
                            owner_path,
                        );
                    }
                }
            }
            "ia.display.table" => {
                let subview = props.get("rows").and_then(|r| r.get("subview"));
                self.push_view_ref(
                    owner,
                    ViewRefKind::TableSubview,
                    subview.and_then(|s| s.get("viewPath")),
                    "props.rows.subview.viewPath".into(),
                    subview.and_then(|s| s.get("viewParams")),
                    Some("props.rows.subview.viewParams".into()),
                    owner_path,
                );
                for (i, c) in list("columns").into_iter().flatten().enumerate() {
                    self.push_view_ref(
                        owner,
                        ViewRefKind::TableColumn,
                        c.get("viewPath"),
                        format!("props.columns[{i}].viewPath"),
                        c.get("viewParams"),
                        Some(format!("props.columns[{i}].viewParams")),
                        owner_path,
                    );
                }
            }
            _ => {}
        }
    }
}

/// `{name}` placeholders in an indirect tag path, with file ranges of the
/// names.
fn placeholders(path: &JsonString<'_>) -> Vec<(String, TextRange)> {
    let text = path.value();
    let mut out = Vec::new();
    let mut rest = 0usize;
    while let Some(open) = text[rest..].find('{') {
        let start = rest + open + 1;
        let Some(close) = text[start..].find('}') else {
            break;
        };
        let name = &text[start..start + close];
        if !name.contains('{') {
            let r = TextRange::at(TextSize::new(start as u32), TextSize::of(name));
            out.push((name.to_owned(), path.raw_range(r)));
        }
        rest = start + close + 1;
    }
    out
}
