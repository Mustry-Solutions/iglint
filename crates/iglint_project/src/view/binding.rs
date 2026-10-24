//! Bindings and transforms (format spec 6.6 and 6.7).

use iglint_expression::{PropertyRef, TagPath};
use iglint_json::{JsonString, Value};
use iglint_text::TextRange;

use crate::ids::{BindingId, ExprId, PropConfigId, ScriptId};
use crate::script::Owner;

/// The registered binding types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BindingKind {
    Tag,
    Query,
    Expr,
    ExprStruct,
    Property,
    TagHistory,
    Http,
    /// A type the gateway does not register (becomes `BrokenBinding`), or a
    /// missing type.
    Unknown,
}

impl BindingKind {
    /// Parses a binding `type`.
    pub fn parse(s: &str) -> BindingKind {
        match s {
            "tag" => BindingKind::Tag,
            "query" => BindingKind::Query,
            "expr" => BindingKind::Expr,
            "expr-struct" => BindingKind::ExprStruct,
            "property" => BindingKind::Property,
            "tag-history" => BindingKind::TagHistory,
            "http" => BindingKind::Http,
            _ => BindingKind::Unknown,
        }
    }

    /// The type as written (`unknown` for unknown types).
    pub fn name(self) -> &'static str {
        match self {
            BindingKind::Tag => "tag",
            BindingKind::Query => "query",
            BindingKind::Expr => "expr",
            BindingKind::ExprStruct => "expr-struct",
            BindingKind::Property => "property",
            BindingKind::TagHistory => "tag-history",
            BindingKind::Http => "http",
            BindingKind::Unknown => "unknown",
        }
    }
}

/// A binding (`propConfig.<path>.binding`).
#[derive(Debug)]
pub struct Binding<'a> {
    pub id: BindingId,
    pub owner: Owner,
    pub prop_config: PropConfigId,
    /// The bound property path, e.g. `props.text`.
    pub property: &'a str,
    /// The binding object.
    pub value: &'a Value<'a>,
    pub type_value: Option<&'a JsonString<'a>>,
    pub kind: BindingKind,
    pub config: Option<&'a Value<'a>>,
    /// `enabled`; absent means enabled.
    pub enabled: Option<bool>,
    pub typed: BindingConfig<'a>,
    pub transforms: Vec<Transform<'a>>,
    /// e.g. `root/Title.props.text@binding`.
    pub node_path: String,
    /// The binding's property path, e.g. `root.root.children[0].Title.propConfig.props.text`.
    pub ign_lint_path: String,
}

impl Binding<'_> {
    /// False when `enabled: false`.
    pub fn is_enabled(&self) -> bool {
        self.enabled != Some(false)
    }

    /// Every expression of the binding and its transforms.
    pub fn expression_ids(&self) -> Vec<ExprId> {
        let mut out = Vec::new();
        match &self.typed {
            BindingConfig::Expr { expression } => out.extend(*expression),
            BindingConfig::ExprStruct { leaves, .. } => {
                out.extend(leaves.iter().map(|l| l.expression))
            }
            BindingConfig::Tag(t) => {
                out.extend(t.path_expression);
            }
            BindingConfig::Query(q) => {
                out.extend(q.parameters.iter().filter_map(|p| p.expression));
                out.extend(q.polling.rate_expression);
            }
            BindingConfig::TagHistory(h) => {
                match &h.tags {
                    TagHistoryTags::Expression(e) => out.push(*e),
                    TagHistoryTags::List(list) => {
                        out.extend(list.iter().filter_map(|t| t.path_expression));
                    }
                    TagHistoryTags::None => {}
                }
                out.extend(h.polling.rate_expression);
            }
            BindingConfig::Http(h) => {
                out.extend(h.url_expression);
                out.extend(h.polling.rate_expression);
            }
            BindingConfig::Property(_) | BindingConfig::None => {}
        }
        for t in &self.transforms {
            match &t.kind {
                TransformKind::Expression { expression } => out.extend(*expression),
                TransformKind::Map(m) => {
                    for mapping in &m.mappings {
                        out.extend(mapping.input_expression);
                        out.extend(mapping.output_expression);
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// Typed binding configs.
#[derive(Debug)]
pub enum BindingConfig<'a> {
    Property(PropertyBinding<'a>),
    Expr {
        expression: Option<ExprId>,
    },
    ExprStruct {
        leaves: Vec<StructLeaf>,
        wait_on_all: Option<bool>,
    },
    Tag(TagBinding<'a>),
    Query(QueryBinding<'a>),
    TagHistory(TagHistoryBinding<'a>),
    Http(HttpBinding<'a>),
    /// Unknown binding type or missing config.
    None,
}

/// `property` binding config.
#[derive(Debug)]
pub struct PropertyBinding<'a> {
    pub path: Option<&'a JsonString<'a>>,
    /// The parsed path; `None` when it is not a valid property reference.
    pub reference: Option<PropertyRef>,
    pub bidirectional: bool,
}

/// One leaf of an `expr-struct` binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructLeaf {
    /// Dotted key path inside `struct`, e.g. `count` or `a.b`.
    pub key: String,
    pub expression: ExprId,
}

/// `tag` binding mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TagMode {
    Direct,
    Indirect,
    Expression,
    /// A mode string Ignition does not know.
    Other,
}

/// `tag` binding config.
#[derive(Debug)]
pub struct TagBinding<'a> {
    /// `mode`; absent means `direct`.
    pub mode: TagMode,
    pub mode_value: Option<&'a JsonString<'a>>,
    pub tag_path: Option<&'a JsonString<'a>>,
    /// The parsed `tagPath` in `direct` mode.
    pub parsed_path: Option<TagPath>,
    /// `tagPath` in `expression` mode.
    pub path_expression: Option<ExprId>,
    /// `{name}` placeholders in an `indirect` tag path, with file ranges
    /// (of the name inside the braces).
    pub placeholders: Vec<(String, TextRange)>,
    /// `references` of an `indirect` binding.
    pub references: Vec<IndirectReference<'a>>,
    pub bidirectional: bool,
    pub fallback_delay: Option<f64>,
}

/// One entry of an indirect tag binding's `references`.
#[derive(Debug)]
pub struct IndirectReference<'a> {
    pub key: &'a JsonString<'a>,
    pub value: &'a Value<'a>,
    /// The property reference when the value is exactly `{<property path>}`.
    pub reference: Option<PropertyRef>,
}

/// `query` binding config.
#[derive(Debug)]
pub struct QueryBinding<'a> {
    pub query_path: Option<&'a JsonString<'a>>,
    pub return_format: Option<&'a str>,
    pub parameters: Vec<QueryParameter<'a>>,
    pub polling: Polling<'a>,
}

/// One query binding parameter. Values are expressions.
#[derive(Debug)]
pub struct QueryParameter<'a> {
    pub name: &'a JsonString<'a>,
    pub value: &'a Value<'a>,
    pub expression: Option<ExprId>,
}

/// `polling` of query, tag history and HTTP bindings.
#[derive(Debug, Default)]
pub struct Polling<'a> {
    pub value: Option<&'a Value<'a>>,
    pub enabled: Option<bool>,
    pub rate: Option<&'a Value<'a>>,
    /// `rate` when it is a string (an expression).
    pub rate_expression: Option<ExprId>,
}

impl Polling<'_> {
    /// The rate when it is a literal number, or a string holding only a
    /// number.
    pub fn literal_rate(&self) -> Option<f64> {
        let rate = self.rate?;
        rate.as_f64()
            .or_else(|| rate.as_str().and_then(|s| s.trim().parse().ok()))
    }
}

/// `tag-history` binding config.
#[derive(Debug)]
pub struct TagHistoryBinding<'a> {
    pub tags: TagHistoryTags<'a>,
    pub date_range: Option<&'a Value<'a>>,
    pub return_format: Option<&'a str>,
    pub return_size: Option<&'a Value<'a>>,
    pub aggregate: Option<&'a str>,
    pub polling: Polling<'a>,
}

/// `tags` of a tag history binding.
#[derive(Debug)]
pub enum TagHistoryTags<'a> {
    List(Vec<HistoryTag<'a>>),
    Expression(ExprId),
    None,
}

/// One entry of a tag history binding's `tags` list.
#[derive(Debug)]
pub struct HistoryTag<'a> {
    pub value: &'a Value<'a>,
    pub path: Option<&'a JsonString<'a>>,
    /// The path when it parses as an expression.
    pub path_expression: Option<ExprId>,
    pub alias: Option<&'a str>,
    pub aggregate: Option<&'a str>,
}

/// `http` binding config.
#[derive(Debug)]
pub struct HttpBinding<'a> {
    pub url: Option<&'a JsonString<'a>>,
    /// The URL expression.
    pub url_expression: Option<ExprId>,
    pub method: Option<&'a str>,
    pub auth_type: Option<&'a str>,
    pub polling: Polling<'a>,
}

/// One transform of a binding.
#[derive(Debug)]
pub struct Transform<'a> {
    pub index: usize,
    pub value: &'a Value<'a>,
    pub type_name: Option<&'a str>,
    /// `enabled`; absent means enabled.
    pub enabled: Option<bool>,
    pub kind: TransformKind<'a>,
    pub node_path: String,
    pub ign_lint_path: String,
}

/// Transform types (format spec 6.7).
#[derive(Debug)]
pub enum TransformKind<'a> {
    Script {
        script: Option<ScriptId>,
    },
    Expression {
        expression: Option<ExprId>,
    },
    Map(MapTransform<'a>),
    Format(FormatTransform<'a>),
    /// A type the gateway does not register.
    Unknown,
}

/// `map` transform.
#[derive(Debug)]
pub struct MapTransform<'a> {
    pub input_type: Option<&'a str>,
    pub output_type: Option<&'a str>,
    pub mappings: Vec<Mapping<'a>>,
    pub fallback: Option<&'a Value<'a>>,
}

/// One `{input, output}` pair of a map transform.
#[derive(Debug)]
pub struct Mapping<'a> {
    pub value: &'a Value<'a>,
    pub input: Option<&'a Value<'a>>,
    pub output: Option<&'a Value<'a>>,
    pub input_expression: Option<ExprId>,
    pub output_expression: Option<ExprId>,
}

/// `format` transform.
#[derive(Debug)]
pub struct FormatTransform<'a> {
    pub format_type: Option<&'a str>,
    pub format_value: Option<&'a str>,
    pub locale: Option<&'a str>,
    pub timezone: Option<&'a str>,
}
