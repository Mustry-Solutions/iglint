//! ign-lint's view of a `view.json`: the flattened path map and the typed
//! node model built from it (`docs/compat/ign-lint.md`, sections 2 and 3).
//!
//! iglint's rules do not use this model; it exists so that ign-lint's
//! statistics and debugging flags (`--stats-only`, `--debug-nodes`,
//! `--analyze-rules`, `--debug-output`) report what ign-lint reports.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::sync::LazyLock;

use iglint_json::{Object, Value as JsonValue, ValueKind};
use regex::Regex;
use serde_json::{Map, Value, json};

/// The flattened map: path to leaf value, in document order. Assigning an
/// existing path replaces its value in place, like a Python dict.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Flat {
    entries: Vec<(String, Value)>,
    index: HashMap<String, usize>,
}

impl Flat {
    fn insert(&mut self, key: String, value: Value) {
        match self.index.get(&key) {
            Some(&i) => self.entries[i].1 = value,
            None => {
                self.index.insert(key.clone(), self.entries.len());
                self.entries.push((key, value));
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.index.get(key).map(|&i| &self.entries[i].1)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The map as a JSON object with sorted keys (ign-lint's debug file).
    pub fn to_sorted_json(&self) -> Value {
        let sorted: BTreeMap<&str, &Value> = self.iter().collect();
        Value::Object(
            sorted
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.clone()))
                .collect(),
        )
    }
}

/// Members of a JSON object as Python's `dict` sees them: duplicate keys
/// keep the position of the first and the value of the last.
fn dict_members<'v, 'a>(object: &'v Object<'a>) -> Vec<(&'v str, &'v JsonValue<'a>)> {
    if object.duplicate_keys().is_empty() {
        return object.iter().map(|m| (m.key(), &m.value)).collect();
    }
    let mut members: Vec<(&str, &JsonValue<'a>)> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for m in object.iter() {
        match index.get(m.key()) {
            Some(&i) => members[i].1 = &m.value,
            None => {
                index.insert(m.key(), members.len());
                members.push((m.key(), &m.value));
            }
        }
    }
    members
}

fn is_java_date(members: &[(&str, &JsonValue<'_>)]) -> bool {
    let get = |key: &str| members.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
    members.len() == 2
        && get("$")
            .and_then(JsonValue::as_array)
            .is_some_and(|a| a.len() == 3)
        && get("$ts").is_some_and(|v| v.as_number().is_some())
}

fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python truthiness of a parsed JSON value.
pub fn json_truthy(value: &JsonValue<'_>) -> bool {
    match &value.kind {
        ValueKind::Null => false,
        ValueKind::Bool(b) => *b,
        ValueKind::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        ValueKind::String(s) => !s.value().is_empty(),
        ValueKind::Array(a) => !a.is_empty(),
        ValueKind::Object(o) => !o.is_empty(),
    }
}

/// A leaf (scalar or empty container) as a `serde_json` value, with the
/// number semantics of `serde_json` (and Python's `json`).
fn leaf(value: &JsonValue<'_>) -> Value {
    match &value.kind {
        ValueKind::Null => Value::Null,
        ValueKind::Bool(b) => Value::Bool(*b),
        ValueKind::Number(n) => serde_json::from_str(n.raw()).unwrap_or(Value::Null),
        ValueKind::String(s) => Value::String(s.value().to_owned()),
        ValueKind::Array(_) => Value::Array(Vec::new()),
        ValueKind::Object(_) => Value::Object(Map::new()),
    }
}

/// Python `str()` of a scalar JSON value.
pub fn python_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Python `str()` of a `meta.name` value. Containers print as
/// `serde_json` would print them, without recursion.
fn name_str(value: &JsonValue<'_>) -> String {
    if !matches!(value.kind, ValueKind::Array(_) | ValueKind::Object(_)) {
        return python_str(&leaf(value));
    }
    enum Step<'v, 'a> {
        Value(&'v JsonValue<'a>),
        Key(&'v str),
        Text(&'static str),
    }
    let mut out = String::new();
    let mut pending = vec![Step::Value(value)];
    while let Some(step) = pending.pop() {
        let value = match step {
            Step::Text(text) => {
                out.push_str(text);
                continue;
            }
            Step::Key(key) => {
                out.push_str(&Value::from(key).to_string());
                continue;
            }
            Step::Value(value) => value,
        };
        match &value.kind {
            ValueKind::Array(a) => {
                out.push('[');
                pending.push(Step::Text("]"));
                for (i, e) in a.iter().enumerate().rev() {
                    pending.push(Step::Value(e));
                    if i > 0 {
                        pending.push(Step::Text(","));
                    }
                }
            }
            ValueKind::Object(o) => {
                out.push('{');
                pending.push(Step::Text("}"));
                let members = dict_members(o);
                for (i, (key, value)) in members.into_iter().enumerate().rev() {
                    pending.push(Step::Value(value));
                    pending.push(Step::Text(":"));
                    pending.push(Step::Key(key));
                    if i > 0 {
                        pending.push(Step::Text(","));
                    }
                }
            }
            _ => out.push_str(&leaf(value).to_string()),
        }
    }
    out
}

/// Appends `.key` to `path` (just `key` at the top).
fn join(path: &mut String, key: &str) {
    if !path.is_empty() {
        path.push('.');
    }
    path.push_str(key);
}

/// The items of an array or object being flattened.
enum Items<'v, 'a> {
    Members(std::vec::IntoIter<(&'v str, &'v JsonValue<'a>)>),
    Elements(std::iter::Enumerate<std::slice::Iter<'v, JsonValue<'a>>>),
}

/// Flattens a parsed `view.json` (section 2). ign-lint recurses (and
/// crashes at about 500 levels, see `docs/compat/ign-lint-differences.md`);
/// this walks with an explicit stack and one path buffer, so any depth is
/// flattened in time and memory linear in the input.
pub fn flatten(data: &JsonValue<'_>) -> Flat {
    let mut flat = Flat::default();
    let mut path = String::new();
    // Open containers: their items and the length of their path.
    let mut stack: Vec<(Items<'_, '_>, usize)> = Vec::new();
    let mut next = Some(data);
    loop {
        if let Some(container) = next.take() {
            match &container.kind {
                ValueKind::Object(object) => {
                    let members = dict_members(object);
                    if is_java_date(&members) {
                        let ts = members.iter().find(|(k, _)| *k == "$ts").map(|(_, v)| *v);
                        path.push_str("._JavaDate");
                        flat.insert(path.clone(), ts.map_or(Value::Null, leaf));
                    } else {
                        // A non-object `meta` makes ign-lint crash; here it
                        // is ordinary data.
                        let name = members
                            .iter()
                            .rev()
                            .find(|(k, _)| *k == "meta")
                            .and_then(|(_, m)| m.as_object())
                            .map(dict_members)
                            .and_then(|m| m.into_iter().find(|(k, _)| *k == "name"))
                            .map(|(_, n)| n)
                            .filter(|n| json_truthy(n));
                        if let Some(name) = name {
                            join(&mut path, &name_str(name));
                        }
                        stack.push((Items::Members(members.into_iter()), path.len()));
                    }
                }
                ValueKind::Array(items) => {
                    stack.push((Items::Elements(items.iter().enumerate()), path.len()));
                }
                _ => {}
            }
        }
        let Some((items, base)) = stack.last_mut() else {
            return flat;
        };
        path.truncate(*base);
        let in_object = matches!(items, Items::Members(_));
        let item = match items {
            Items::Members(members) => members.next().map(|(key, value)| {
                join(&mut path, key);
                value
            }),
            Items::Elements(elements) => elements.next().map(|(i, value)| {
                use std::fmt::Write as _;
                let _ = write!(path, "[{i}]");
                value
            }),
        };
        let Some(value) = item else {
            stack.pop();
            continue;
        };
        match &value.kind {
            ValueKind::Object(o) if o.is_empty() => {
                if in_object {
                    flat.insert(path.clone(), leaf(value));
                }
            }
            ValueKind::Array(a) if a.is_empty() => {
                if in_object {
                    flat.insert(path.clone(), leaf(value));
                }
            }
            ValueKind::Object(_) | ValueKind::Array(_) => next = Some(value),
            _ => flat.insert(path.clone(), leaf(value)),
        }
    }
}

/// ign-lint's node types, in the order of its `NodeType` enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeType {
    Component,
    ExpressionBinding,
    ExpressionStructBinding,
    PropertyBinding,
    TagBinding,
    QueryBinding,
    MessageHandler,
    CustomMethod,
    Transform,
    EventHandler,
    PropertyChangeScript,
    Property,
}

impl NodeType {
    pub const ALL: [NodeType; 12] = [
        NodeType::Component,
        NodeType::ExpressionBinding,
        NodeType::ExpressionStructBinding,
        NodeType::PropertyBinding,
        NodeType::TagBinding,
        NodeType::QueryBinding,
        NodeType::MessageHandler,
        NodeType::CustomMethod,
        NodeType::Transform,
        NodeType::EventHandler,
        NodeType::PropertyChangeScript,
        NodeType::Property,
    ];

    pub const BINDINGS: [NodeType; 5] = [
        NodeType::ExpressionBinding,
        NodeType::ExpressionStructBinding,
        NodeType::PropertyBinding,
        NodeType::TagBinding,
        NodeType::QueryBinding,
    ];

    pub const SCRIPTS: [NodeType; 5] = [
        NodeType::MessageHandler,
        NodeType::CustomMethod,
        NodeType::Transform,
        NodeType::EventHandler,
        NodeType::PropertyChangeScript,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            NodeType::Component => "component",
            NodeType::ExpressionBinding => "expression_binding",
            NodeType::ExpressionStructBinding => "expression_struct_binding",
            NodeType::PropertyBinding => "property_binding",
            NodeType::TagBinding => "tag_binding",
            NodeType::QueryBinding => "query_binding",
            NodeType::MessageHandler => "message_handler",
            NodeType::CustomMethod => "custom_method",
            NodeType::Transform => "transform",
            NodeType::EventHandler => "event_handler",
            NodeType::PropertyChangeScript => "property_change_script",
            NodeType::Property => "property",
        }
    }

    pub fn parse(value: &str) -> Option<NodeType> {
        NodeType::ALL.into_iter().find(|t| t.as_str() == value)
    }

    /// `['component', 'expression_binding', ...]`, as ign-lint prints it.
    pub fn available_list() -> String {
        let names: Vec<String> = NodeType::ALL
            .iter()
            .map(|t| format!("'{}'", t.as_str()))
            .collect();
        format!("[{}]", names.join(", "))
    }
}

impl fmt::Display for NodeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Type-specific data of a node.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeData {
    Component {
        name: String,
        component_type: String,
    },
    Expression {
        expression: String,
    },
    ExpressionStruct {
        structure: Vec<(String, String)>,
        config: Vec<(String, Value)>,
    },
    PropertyBinding {
        target_path: String,
    },
    Tag {
        tag_path: String,
        mode: String,
        references: Vec<(String, String)>,
        config: Vec<(String, Value)>,
    },
    Query {
        query_path: String,
        parameters: Vec<(String, String)>,
    },
    MessageHandler {
        message_type: String,
        script: String,
        page_scope: bool,
        session_scope: bool,
        view_scope: bool,
    },
    CustomMethod {
        name: String,
        script: String,
        params: Vec<String>,
    },
    Transform {
        script: String,
    },
    EventHandler {
        domain: String,
        event_type: String,
        script: String,
        scope: String,
    },
    PropertyChange {
        property: String,
        script: String,
    },
    Property {
        name: String,
        value: Option<Value>,
        persistent: Option<bool>,
        private_access: Option<bool>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub path: String,
    pub node_type: NodeType,
    pub data: NodeData,
}

impl Node {
    /// Private properties (`_x`, `_JavaDate`) are skipped by ign-lint rules.
    pub fn is_private_property(&self) -> bool {
        matches!(&self.data, NodeData::Property { name, .. } if name.starts_with('_'))
    }

    pub fn script(&self) -> Option<&str> {
        match &self.data {
            NodeData::MessageHandler { script, .. }
            | NodeData::CustomMethod { script, .. }
            | NodeData::Transform { script }
            | NodeData::EventHandler { script, .. }
            | NodeData::PropertyChange { script, .. } => Some(script),
            _ => None,
        }
    }

    /// The one-line summary `--analyze-rules` prints.
    pub fn summary(&self) -> String {
        fn head(text: &str, n: usize) -> String {
            text.chars().take(n).collect()
        }
        match &self.data {
            NodeData::Component {
                name,
                component_type,
            } => format!("Component '{name}' of type '{component_type}'"),
            NodeData::Expression { expression } => {
                format!("Expression: {}...", head(expression, 50))
            }
            NodeData::Tag { tag_path, .. } => format!("Tag path: {tag_path}"),
            NodeData::PropertyBinding { target_path } => format!("Property path: {target_path}"),
            _ => match self.script() {
                Some(script) => format!("Script: {}...", head(script, 30)),
                None => format!("{} node", self.node_type),
            },
        }
    }

    fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("path".into(), json!(self.path));
        object.insert("node_type".into(), json!(self.node_type.as_str()));
        let pairs = |items: &[(String, String)]| -> Value {
            Value::Object(items.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
        };
        let config =
            |items: &[(String, Value)]| -> Value { Value::Object(items.iter().cloned().collect()) };
        let script_fields = |object: &mut Map<String, Value>, script: &str| {
            object.insert("script_length".into(), json!(script.chars().count()));
            let preview: String = script.chars().take(100).collect();
            let preview = if script.chars().count() > 100 {
                format!("{preview}...")
            } else {
                preview
            };
            object.insert("script_preview".into(), json!(preview));
        };
        match &self.data {
            NodeData::Component {
                name,
                component_type,
            } => {
                object.insert("name".into(), json!(name));
                object.insert("type".into(), json!(component_type));
            }
            NodeData::Expression { expression } => {
                object.insert("expression".into(), json!(expression));
            }
            NodeData::ExpressionStruct {
                structure,
                config: c,
            } => {
                object.insert("struct".into(), pairs(structure));
                object.insert("config".into(), config(c));
            }
            NodeData::PropertyBinding { target_path } => {
                object.insert("target_path".into(), json!(target_path));
            }
            NodeData::Tag {
                tag_path,
                mode,
                references,
                config: c,
            } => {
                object.insert("tag_path".into(), json!(tag_path));
                object.insert("mode".into(), json!(mode));
                object.insert("references".into(), pairs(references));
                object.insert("config".into(), config(c));
            }
            NodeData::Query {
                query_path,
                parameters,
            } => {
                object.insert("query_path".into(), json!(query_path));
                object.insert("parameters".into(), pairs(parameters));
            }
            NodeData::MessageHandler {
                message_type,
                script,
                page_scope,
                session_scope,
                view_scope,
            } => {
                script_fields(&mut object, script);
                object.insert("message_type".into(), json!(message_type));
                object.insert(
                    "scopes".into(),
                    json!({"page": page_scope, "session": session_scope, "view": view_scope}),
                );
            }
            NodeData::CustomMethod {
                name,
                script,
                params,
            } => {
                script_fields(&mut object, script);
                object.insert("method_name".into(), json!(name));
                object.insert("params".into(), json!(params));
            }
            NodeData::Transform { script } => script_fields(&mut object, script),
            NodeData::EventHandler {
                domain,
                event_type,
                script,
                scope,
            } => {
                script_fields(&mut object, script);
                object.insert("domain".into(), json!(domain));
                object.insert("event_type".into(), json!(event_type));
                object.insert("scope".into(), json!(scope));
            }
            NodeData::PropertyChange { property, script } => {
                script_fields(&mut object, script);
                object.insert("property_name".into(), json!(property));
            }
            NodeData::Property {
                name,
                value,
                persistent,
                private_access,
            } => {
                object.insert("name".into(), json!(name));
                let value = value.clone().unwrap_or(Value::Null);
                object.insert("value_type".into(), json!(python_type_name(&value)));
                object.insert("value".into(), value);
                if let Some(persistent) = persistent {
                    object.insert("persistent".into(), json!(persistent));
                }
                if let Some(private_access) = private_access {
                    object.insert("private_access".into(), json!(private_access));
                }
            }
        }
        Value::Object(object)
    }
}

fn python_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// The node model of one view.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewModel {
    pub components: Vec<Node>,
    pub message_handlers: Vec<Node>,
    pub custom_methods: Vec<Node>,
    pub expression_bindings: Vec<Node>,
    pub expression_struct_bindings: Vec<Node>,
    pub property_bindings: Vec<Node>,
    pub tag_bindings: Vec<Node>,
    pub query_bindings: Vec<Node>,
    pub script_transforms: Vec<Node>,
    pub event_handlers: Vec<Node>,
    pub property_change_scripts: Vec<Node>,
    pub properties: Vec<Node>,
}

static INDEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\d+\]").expect("valid regex"));
static TRAILING_INDEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\d+\]$").expect("valid regex"));
static PARENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.*?)\.children\[\d+\]\.[^.]+$").expect("valid regex"));
static CUSTOM_METHOD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.*\.scripts\.customMethods\[\d+\])").expect("valid regex"));
static EVENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\.events\.([^.]+)\.([^.]+)$").expect("valid regex"));

/// The parent component path of a component path, if it has one.
pub fn parent_path(component_path: &str) -> Option<&str> {
    PARENT
        .captures(component_path)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
}

impl ViewModel {
    /// Builds the model (section 3).
    pub fn build(flat: &Flat) -> ViewModel {
        let mut model = ViewModel::default();
        let string = |key: &str, default: &str| -> String {
            flat.get(key)
                .map(python_str)
                .unwrap_or_else(|| default.to_owned())
        };

        // Components.
        for (key, value) in flat.iter() {
            if let Some(path) = key.strip_suffix(".meta.name") {
                model.components.push(Node {
                    path: path.to_owned(),
                    node_type: NodeType::Component,
                    data: NodeData::Component {
                        name: python_str(value),
                        component_type: string(&format!("{path}.type"), "unknown"),
                    },
                });
            }
        }

        // Bindings and their transforms.
        let mut binding_paths: Vec<String> = Vec::new();
        let mut seen = BTreeSet::new();
        for (key, _) in flat.iter() {
            if let Some(at) = key.rfind(".binding.type") {
                let path = key[..at].to_owned();
                if seen.insert(path.clone()) {
                    binding_paths.push(path);
                }
            }
        }
        for bp in &binding_paths {
            let binding_type = string(&format!("{bp}.binding.type"), "");
            let config_prefix = format!("{bp}.binding.config.");
            let leaves_under = |prefix: &str| -> Vec<(String, String)> {
                flat.iter()
                    .filter_map(|(k, v)| {
                        let rest = k.strip_prefix(prefix)?;
                        v.as_str().map(|s| (rest.to_owned(), s.to_owned()))
                    })
                    .collect()
            };
            match binding_type.as_str() {
                "expr" | "expression" => model.expression_bindings.push(Node {
                    path: bp.clone(),
                    node_type: NodeType::ExpressionBinding,
                    data: NodeData::Expression {
                        expression: string(&format!("{config_prefix}expression"), "unknown"),
                    },
                }),
                "expr-struct" => {
                    let struct_prefix = format!("{config_prefix}struct.");
                    let config = flat
                        .iter()
                        .filter_map(|(k, v)| {
                            let rest = k.strip_prefix(&config_prefix)?;
                            (!k.starts_with(&struct_prefix)).then(|| (rest.to_owned(), v.clone()))
                        })
                        .collect();
                    model.expression_struct_bindings.push(Node {
                        path: bp.clone(),
                        node_type: NodeType::ExpressionStructBinding,
                        data: NodeData::ExpressionStruct {
                            structure: leaves_under(&struct_prefix),
                            config,
                        },
                    });
                }
                "property" => model.property_bindings.push(Node {
                    path: bp.clone(),
                    node_type: NodeType::PropertyBinding,
                    data: NodeData::PropertyBinding {
                        target_path: string(&format!("{config_prefix}path"), "unknown"),
                    },
                }),
                "tag" => {
                    let references_prefix = format!("{config_prefix}references.");
                    let config = flat
                        .iter()
                        .filter_map(|(k, v)| {
                            let rest = k.strip_prefix(&config_prefix)?;
                            (!k.starts_with(&references_prefix)
                                && !k.ends_with("mode")
                                && !k.ends_with("tagPath"))
                            .then(|| (rest.to_owned(), v.clone()))
                        })
                        .collect();
                    model.tag_bindings.push(Node {
                        path: bp.clone(),
                        node_type: NodeType::TagBinding,
                        data: NodeData::Tag {
                            tag_path: string(&format!("{config_prefix}tagPath"), "unknown"),
                            mode: string(&format!("{config_prefix}mode"), "direct"),
                            references: leaves_under(&references_prefix),
                            config,
                        },
                    });
                }
                "query" => model.query_bindings.push(Node {
                    path: bp.clone(),
                    node_type: NodeType::QueryBinding,
                    data: NodeData::Query {
                        query_path: string(&format!("{config_prefix}queryPath"), "unknown"),
                        parameters: leaves_under(&format!("{config_prefix}parameters.")),
                    },
                }),
                _ => {}
            }
            let transforms_prefix = format!("{bp}.binding.transforms");
            for (key, value) in flat.iter() {
                if !key.starts_with(&transforms_prefix) {
                    continue;
                }
                let Some(t) = key.strip_suffix(".type") else {
                    continue;
                };
                match value.as_str() {
                    Some("script") => {
                        if let Some(code) = flat.get(&format!("{t}.code")) {
                            model.script_transforms.push(Node {
                                path: t.to_owned(),
                                node_type: NodeType::Transform,
                                data: NodeData::Transform {
                                    script: python_str(code),
                                },
                            });
                        }
                    }
                    Some("expression") => {
                        if let Some(expression) = flat.get(&format!("{t}.expression")) {
                            model.expression_bindings.push(Node {
                                path: t.to_owned(),
                                node_type: NodeType::ExpressionBinding,
                                data: NodeData::Expression {
                                    expression: python_str(expression),
                                },
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        // Message handlers.
        for (key, value) in flat.iter() {
            if key.contains(".scripts.messageHandlers")
                && let Some(base) = key.strip_suffix(".messageType")
            {
                let flag = |name: &str| {
                    flat.get(&format!("{base}.{name}"))
                        .is_some_and(python_truthy)
                };
                model.message_handlers.push(Node {
                    path: base.to_owned(),
                    node_type: NodeType::MessageHandler,
                    data: NodeData::MessageHandler {
                        message_type: python_str(value),
                        script: string(&format!("{base}.script"), ""),
                        page_scope: flag("pageScope"),
                        session_scope: flag("sessionScope"),
                        view_scope: flag("viewScope"),
                    },
                });
            }
        }

        // Custom methods.
        let mut method_bases = Vec::new();
        let mut seen = BTreeSet::new();
        for (key, _) in flat.iter() {
            if let Some(m) = CUSTOM_METHOD.captures(key).and_then(|c| c.get(1))
                && seen.insert(m.as_str().to_owned())
            {
                method_bases.push(m.as_str().to_owned());
            }
        }
        for base in method_bases {
            let mut params = Vec::new();
            for j in 0.. {
                match flat.get(&format!("{base}.params[{j}]")) {
                    Some(param) => params.push(python_str(param)),
                    None => break,
                }
            }
            model.custom_methods.push(Node {
                node_type: NodeType::CustomMethod,
                data: NodeData::CustomMethod {
                    name: string(&format!("{base}.name"), "unknown_method"),
                    script: string(&format!("{base}.script"), ""),
                    params,
                },
                path: base,
            });
        }

        // Event handlers.
        let mut seen = BTreeSet::new();
        for (key, value) in flat.iter() {
            if !key.contains(".events.") {
                continue;
            }
            let event_path = if let Some(at) = key.find(".config.script") {
                &key[..at]
            } else if let Some(path) = key.strip_suffix(".script") {
                path
            } else {
                continue;
            };
            let Some(captures) = EVENT.captures(event_path) else {
                continue;
            };
            if !seen.insert(event_path.to_owned()) {
                continue;
            }
            model.event_handlers.push(Node {
                path: event_path.to_owned(),
                node_type: NodeType::EventHandler,
                data: NodeData::EventHandler {
                    domain: captures[1].to_owned(),
                    event_type: captures[2].to_owned(),
                    script: python_str(value),
                    scope: string(&format!("{event_path}.scope"), "L"),
                },
            });
        }

        // Property change scripts.
        for (key, value) in flat.iter() {
            let Some(path) = key.strip_suffix(".script") else {
                continue;
            };
            if !path.ends_with(".onChange")
                || !(key.starts_with("propConfig.") || key.contains(".propConfig."))
            {
                continue;
            }
            let after = match key.rfind("propConfig.") {
                Some(at) => &key[at + "propConfig.".len()..],
                None => continue,
            };
            let property = after
                .rfind(".onChange")
                .map(|at| &after[..at])
                .unwrap_or(after);
            model.property_change_scripts.push(Node {
                path: path.to_owned(),
                node_type: NodeType::PropertyChangeScript,
                data: NodeData::PropertyChange {
                    property: property.to_owned(),
                    script: python_str(value),
                },
            });
        }

        model.properties = build_properties(flat, &model.components);
        model
    }

    pub fn collections(&self) -> [(&'static str, &[Node]); 12] {
        [
            ("components", &self.components),
            ("message_handlers", &self.message_handlers),
            ("custom_methods", &self.custom_methods),
            ("expression_bindings", &self.expression_bindings),
            (
                "expression_struct_bindings",
                &self.expression_struct_bindings,
            ),
            ("property_bindings", &self.property_bindings),
            ("tag_bindings", &self.tag_bindings),
            ("query_bindings", &self.query_bindings),
            ("script_transforms", &self.script_transforms),
            ("event_handlers", &self.event_handlers),
            ("property_change_scripts", &self.property_change_scripts),
            ("properties", &self.properties),
        ]
    }

    /// Every node, in the order rules see them (section 3.7).
    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.collections()
            .into_iter()
            .flat_map(|(_, nodes)| nodes.iter())
    }

    pub fn node_count(&self) -> usize {
        self.collections().iter().map(|(_, n)| n.len()).sum()
    }

    pub fn to_json(&self) -> Value {
        Value::Object(
            self.collections()
                .into_iter()
                .map(|(name, nodes)| {
                    (
                        name.to_owned(),
                        json!({
                            "count": nodes.len(),
                            "nodes": nodes.iter().map(Node::to_json).collect::<Vec<_>>(),
                        }),
                    )
                })
                .collect(),
        )
    }

    /// Node counts per type, in `NodeType` order, zero counts omitted.
    pub fn type_counts(&self) -> Vec<(NodeType, usize)> {
        let mut counts: BTreeMap<NodeType, usize> = BTreeMap::new();
        for node in self.nodes() {
            *counts.entry(node.node_type).or_default() += 1;
        }
        counts.into_iter().collect()
    }

    /// Component counts per component type, in first appearance order.
    pub fn components_by_type(&self) -> Vec<(String, usize)> {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for node in &self.components {
            if let NodeData::Component { component_type, .. } = &node.data {
                match counts.iter_mut().find(|(t, _)| t == component_type) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((component_type.clone(), 1)),
                }
            }
        }
        counts
    }
}

fn build_properties(flat: &Flat, components: &[Node]) -> Vec<Node> {
    const SKIP: [&str; 4] = [".meta.", ".binding.", ".scripts.", ".events."];
    let component_paths: Vec<&str> = components.iter().map(|c| c.path.as_str()).collect();
    let mut properties: Vec<Node> = Vec::new();
    let mut created: BTreeSet<String> = BTreeSet::new();
    let flag = |key: String| flat.get(&key).and_then(Value::as_bool);
    for (key, value) in flat.iter() {
        if SKIP.iter().any(|s| key.contains(s)) || key.ends_with(".type") {
            continue;
        }
        if key.starts_with("propConfig.") {
            continue;
        }
        let view_level = key.starts_with("custom.") || key.starts_with("params.");
        if view_level {
            if flag(format!("propConfig.{key}.persistent")) == Some(false) {
                continue;
            }
        } else if !component_paths.iter().any(|c| key.starts_with(c)) {
            continue;
        }
        let path = INDEX.replace_all(key, "").into_owned();
        if !created.insert(path.clone()) {
            continue;
        }
        let last = key.rsplit('.').next().unwrap_or(key);
        let name = TRAILING_INDEX.replace(last, "").into_owned();
        let (persistent, private_access) = if view_level {
            (
                flag(format!("propConfig.{key}.persistent")),
                flat.get(&format!("propConfig.{key}.access"))
                    .and_then(Value::as_str)
                    .map(|a| a == "PRIVATE"),
            )
        } else {
            (None, None)
        };
        properties.push(Node {
            path,
            node_type: NodeType::Property,
            data: NodeData::Property {
                name,
                value: Some(value.clone()),
                persistent,
                private_access,
            },
        });
    }
    // Properties that exist only as view-level propConfig entries.
    let mut extra: BTreeSet<String> = BTreeSet::new();
    for (key, _) in flat.iter() {
        let Some(rest) = key.strip_prefix("propConfig.") else {
            continue;
        };
        let segments: Vec<&str> = rest.split('.').collect();
        let mut kept = vec![segments[0]];
        for segment in &segments[1..] {
            if matches!(
                *segment,
                "binding" | "persistent" | "paramDirection" | "access"
            ) {
                break;
            }
            kept.push(segment);
        }
        let path = kept.join(".");
        if (path.starts_with("custom.") || path.starts_with("params.")) && !created.contains(&path)
        {
            extra.insert(path);
        }
    }
    for path in extra {
        let last = path.rsplit('.').next().unwrap_or(&path);
        let name = TRAILING_INDEX.replace(last, "").into_owned();
        properties.push(Node {
            node_type: NodeType::Property,
            data: NodeData::Property {
                name,
                value: None,
                persistent: flag(format!("propConfig.{path}.persistent")),
                private_access: flat
                    .get(&format!("propConfig.{path}.access"))
                    .and_then(Value::as_str)
                    .map(|a| a == "PRIVATE"),
            },
            path,
        });
    }
    properties
}

/// The node types an ign-lint rule targets; empty means every node.
pub fn rule_targets(rule: &str, kwargs: &[(String, Value)]) -> BTreeSet<NodeType> {
    let all_scripts = NodeType::SCRIPTS.into_iter();
    match rule {
        "NamePatternRule" => {
            let kwarg = |name: &str| kwargs.iter().find(|(k, _)| k == name).map(|(_, v)| v);
            let explicit: BTreeSet<NodeType> = match kwarg("target_node_types") {
                Some(Value::String(s)) => NodeType::parse(s).into_iter().collect(),
                Some(Value::Array(items)) => items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(NodeType::parse)
                    .collect(),
                _ => BTreeSet::new(),
            };
            if !explicit.is_empty() {
                return explicit;
            }
            let from_specific: BTreeSet<NodeType> = kwarg("node_type_specific_rules")
                .and_then(Value::as_object)
                .map(|o| o.keys().filter_map(|k| NodeType::parse(k)).collect())
                .unwrap_or_default();
            if !from_specific.is_empty() {
                return from_specific;
            }
            BTreeSet::from([NodeType::Component])
        }
        "PollingIntervalRule" => NodeType::BINDINGS.into_iter().collect(),
        "PylintScriptRule" | "PerspectiveScriptPylintRule" => all_scripts.collect(),
        "BadComponentReferenceRule" | "ComponentReferenceValidationRule" => [
            NodeType::ExpressionBinding,
            NodeType::ExpressionStructBinding,
            NodeType::PropertyBinding,
        ]
        .into_iter()
        .chain(all_scripts)
        .collect(),
        "UnusedCustomPropertiesRule" => BTreeSet::from([
            NodeType::Property,
            NodeType::ExpressionBinding,
            NodeType::PropertyBinding,
            NodeType::TagBinding,
        ]),
        _ => BTreeSet::new(),
    }
}

/// `component, property` or `all`.
pub fn targets_label(targets: &BTreeSet<NodeType>) -> String {
    if targets.is_empty() {
        return "all".to_owned();
    }
    let mut names: Vec<&str> = targets.iter().map(|t| t.as_str()).collect();
    names.sort_unstable();
    names.join(", ")
}

/// Nodes a rule with these targets processes (private properties excluded).
pub fn nodes_for<'a>(model: &'a ViewModel, targets: &'a BTreeSet<NodeType>) -> Vec<&'a Node> {
    model
        .nodes()
        .filter(|n| targets.is_empty() || targets.contains(&n.node_type))
        .filter(|n| !n.is_private_property())
        .collect()
}

/// Returns `value` with every object's keys sorted (Python `sort_keys=True`).
pub fn sort_keys(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let sorted: BTreeMap<&String, Value> =
                object.iter().map(|(k, v)| (k, sort_keys(v))).collect();
            Value::Object(sorted.into_iter().map(|(k, v)| (k.clone(), v)).collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(sort_keys).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(flat: &Flat) -> Vec<&str> {
        flat.iter().map(|(k, _)| k).collect()
    }

    #[test]
    fn flatten_leaves() {
        let data = iglint_json::parse(
            r#"{"a": 1, "b": {"c": "x", "d": null, "e": true}, "empty": {}, "list": [],
                "arr": [1, {}, [], {"k": 2}, [3]],
                "date": {"$": ["ts", 192, 5], "$ts": 7},
                "ds": {"$": ["ds", 1], "$columns": [{"name": "c"}], "rows": [[1]]}}"#,
        )
        .unwrap();
        let flat = flatten(&data);
        assert_eq!(
            keys(&flat),
            vec![
                "a",
                "b.c",
                "b.d",
                "b.e",
                "empty",
                "list",
                "arr[0]",
                "arr[3].k",
                "arr[4][0]",
                "date._JavaDate",
                "ds.$[0]",
                "ds.$[1]",
                "ds.$columns[0].name",
                "ds.rows[0][0]",
            ]
        );
        assert_eq!(flat.get("empty"), Some(&json!({})));
        assert_eq!(flat.get("list"), Some(&json!([])));
        assert_eq!(flat.get("date._JavaDate"), Some(&json!(7)));
    }

    #[test]
    fn flatten_injects_component_names() {
        let data = iglint_json::parse(
            r#"{"root": {"meta": {"name": "root"}, "type": "ia.container.flex",
                "children": [{"meta": {"name": "Button"}, "props": {"text": "Go"}}],
                "custom": {"ghost": {"meta": {"name": "Ghost"}, "x": 1}}}}"#,
        )
        .unwrap();
        let flat = flatten(&data);
        assert_eq!(
            keys(&flat),
            vec![
                "root.root.meta.name",
                "root.root.type",
                "root.root.children[0].Button.meta.name",
                "root.root.children[0].Button.props.text",
                "root.root.custom.ghost.Ghost.meta.name",
                "root.root.custom.ghost.Ghost.x",
            ]
        );
        let top = iglint_json::parse(r#"[1, {"a": 2}]"#).unwrap();
        assert_eq!(keys(&flatten(&top)), vec!["[0]", "[1].a"]);
        // A non-object meta is ordinary data here (ign-lint crashes).
        let bad = iglint_json::parse(r#"{"meta": "x", "b": 1}"#).unwrap();
        assert_eq!(keys(&flatten(&bad)), vec!["meta", "b"]);
    }

    #[test]
    fn parents() {
        assert_eq!(
            parent_path("root.root.children[0].A.children[2].B"),
            Some("root.root.children[0].A")
        );
        assert_eq!(parent_path("root.root"), None);
    }

    #[test]
    fn model_of_a_small_view() {
        let data = iglint_json::parse(
            r#"{
              "custom": {"a": 1, "hidden": 2, "_priv": 3},
              "params": {"p": [1, 2]},
              "propConfig": {
                "custom.hidden": {"persistent": false},
                "custom.bound": {"binding": {"type": "expr", "config": {"expression": "now(100)"},
                    "transforms": [{"type": "script", "code": "\treturn value"},
                                   {"type": "expression", "expression": "{value}"},
                                   {"type": "map"}]}},
                "custom.a": {"onChange": {"script": "\tpass", "enabled": null}}
              },
              "events": {"system": {"onStartup": {"config": {"script": "\tpass"}}}},
              "root": {
                "meta": {"name": "root"}, "type": "ia.container.flex",
                "children": [{
                  "meta": {"name": "Btn"}, "type": "ia.input.button",
                  "props": {"text": "Go"},
                  "custom": {"x": 1},
                  "propConfig": {"props.text": {"binding": {"type": "tag", "config": {"tagPath": "[default]T", "mode": "direct", "fallbackDelay": 2.5}}}},
                  "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\tself.getSibling('x')"}}}},
                  "scripts": {
                    "customMethods": [{"name": "doIt", "params": ["a", "b"], "script": "\treturn a"}],
                    "messageHandlers": [{"messageType": "refresh", "script": "\tpass", "pageScope": true, "sessionScope": false, "viewScope": false}]
                  }
                }]
              }
            }"#,
        )
        .unwrap();
        let flat = flatten(&data);
        let model = ViewModel::build(&flat);
        let paths = |nodes: &[Node]| nodes.iter().map(|n| n.path.clone()).collect::<Vec<_>>();
        assert_eq!(
            paths(&model.components),
            vec!["root.root", "root.root.children[0].Btn"]
        );
        assert_eq!(
            paths(&model.expression_bindings),
            vec![
                "propConfig.custom.bound",
                "propConfig.custom.bound.binding.transforms[1]"
            ]
        );
        assert_eq!(
            paths(&model.script_transforms),
            vec!["propConfig.custom.bound.binding.transforms[0]"]
        );
        assert_eq!(
            paths(&model.tag_bindings),
            vec!["root.root.children[0].Btn.propConfig.props.text"]
        );
        assert_eq!(
            paths(&model.custom_methods),
            vec!["root.root.children[0].Btn.scripts.customMethods[0]"]
        );
        assert_eq!(
            paths(&model.message_handlers),
            vec!["root.root.children[0].Btn.scripts.messageHandlers[0]"]
        );
        // View-level events have no leading dot and are not modelled.
        assert_eq!(
            paths(&model.event_handlers),
            vec!["root.root.children[0].Btn.events.component.onActionPerformed"]
        );
        assert_eq!(
            paths(&model.property_change_scripts),
            vec!["propConfig.custom.a.onChange"]
        );
        let props = paths(&model.properties);
        assert!(props.contains(&"custom.a".to_owned()));
        // Non-persistent view properties are not created from their value,
        // only from their propConfig entry, without a value.
        let hidden = model
            .properties
            .iter()
            .find(|p| p.path == "custom.hidden")
            .unwrap();
        assert!(matches!(
            &hidden.data,
            NodeData::Property { value: None, .. }
        ));
        assert!(props.contains(&"custom._priv".to_owned()));
        assert!(props.contains(&"params.p".to_owned()));
        assert!(props.contains(&"root.root.children.Btn.props.text".to_owned()));
        assert!(props.contains(&"root.root.children.Btn.custom.x".to_owned()));
        // propConfig-only view properties come last, sorted.
        assert!(props.contains(&"custom.bound".to_owned()));
        let NodeData::CustomMethod { params, name, .. } = &model.custom_methods[0].data else {
            panic!()
        };
        assert_eq!((name.as_str(), params.len()), ("doIt", 2));
        let NodeData::Tag { config, mode, .. } = &model.tag_bindings[0].data else {
            panic!()
        };
        assert_eq!(mode, "direct");
        assert_eq!(config.len(), 1);
        assert_eq!(
            model.type_counts()[0],
            (NodeType::Component, 2),
            "counts follow NodeType order"
        );
    }

    #[test]
    fn name_pattern_targets() {
        let kwargs = |v: Value| -> Vec<(String, Value)> {
            v.as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        assert_eq!(
            targets_label(&rule_targets("NamePatternRule", &[])),
            "component"
        );
        assert_eq!(
            targets_label(&rule_targets(
                "NamePatternRule",
                &kwargs(json!({"target_node_types": "property"}))
            )),
            "property"
        );
        assert_eq!(
            targets_label(&rule_targets(
                "NamePatternRule",
                &kwargs(json!({"node_type_specific_rules": {"custom_method": {}, "bogus": {}}}))
            )),
            "custom_method"
        );
        assert_eq!(
            targets_label(&rule_targets("ExcessiveContextDataRule", &[])),
            "all"
        );
    }
}
