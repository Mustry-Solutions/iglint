//! Embedded Ignition expressions.

use iglint_expression::{BoundRef, Parsed, Reference, ReferenceContext};
use iglint_json::JsonString;
use iglint_text::{TextRange, TextSize};

use crate::ids::BindingId;
use crate::script::Owner;
use crate::source_map::SourceMap;

/// Where an expression is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExpressionKind {
    /// `expr` binding `config.expression`.
    Binding,
    /// A leaf of an `expr-struct` binding's `config.struct`.
    StructLeaf,
    /// Tag binding in `expression` mode: `config.tagPath`.
    TagPathExpression,
    /// Query binding parameter value.
    QueryParameter,
    /// Polling rate given as a string (query, tag history, HTTP bindings).
    PollingRate,
    /// Tag history binding: `config.tags` given as an expression string.
    TagHistoryTags,
    /// Tag history binding: one tag's `path`, when it parses as an
    /// expression (tentative).
    TagHistoryPath,
    /// HTTP binding `request.url`: always an expression (a literal URL is
    /// a configuration error in Perspective).
    HttpUrl,
    /// Expression transform.
    Transform,
    /// Map transform input with `inputType: expression`.
    MapInput,
    /// Map transform output with `outputType: expression`.
    MapOutput,
    /// Another field of a binding or transform configuration that
    /// Perspective parses as an expression (measured,
    /// [`crate::perspective_fields`]): HTTP `request.body`, header and
    /// authorization values, tag history `dateRange` and `returnSize`
    /// values, a map transform's `fallback`, list items of an expression
    /// structure, ...
    ConfigField,
    /// Tag `expression` (`valueSource: expr`).
    TagExpression,
    /// Derived tag `deriveExpressionGetter`.
    DeriveGetter,
    /// Derived tag `deriveExpressionSetter`.
    DeriveSetter,
    /// Alarm property bound with `bindType: Expression`.
    AlarmProperty,
    /// Tag property bound with `bindType: Expression`.
    TagProperty,
}

impl ExpressionKind {
    /// Short stable name for dumps.
    pub fn name(self) -> &'static str {
        match self {
            ExpressionKind::Binding => "binding",
            ExpressionKind::StructLeaf => "struct-leaf",
            ExpressionKind::TagPathExpression => "tag-path",
            ExpressionKind::QueryParameter => "query-parameter",
            ExpressionKind::PollingRate => "polling-rate",
            ExpressionKind::TagHistoryTags => "tag-history-tags",
            ExpressionKind::TagHistoryPath => "tag-history-path",
            ExpressionKind::HttpUrl => "http-url",
            ExpressionKind::Transform => "transform",
            ExpressionKind::MapInput => "map-input",
            ExpressionKind::MapOutput => "map-output",
            ExpressionKind::ConfigField => "config-field",
            ExpressionKind::TagExpression => "tag-expression",
            ExpressionKind::DeriveGetter => "derive-getter",
            ExpressionKind::DeriveSetter => "derive-setter",
            ExpressionKind::AlarmProperty => "alarm-property",
            ExpressionKind::TagProperty => "tag-property",
        }
    }
}

/// One expression location, parsed.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddedExpression<'a> {
    pub kind: ExpressionKind,
    /// The JSON string holding the expression.
    pub source: &'a JsonString<'a>,
    pub parsed: Parsed,
    /// How `{...}` references resolve here.
    pub context: ReferenceContext,
    /// Maps offsets of the expression text to the file.
    pub source_map: SourceMap,
    pub owner: Owner,
    /// The binding the expression belongs to, if any.
    pub binding: Option<BindingId>,
    pub node_path: String,
    pub ign_lint_path: Option<String>,
    /// True for locations that may hold either an expression or a literal
    /// (tag history paths). Such locations are only modelled as
    /// expressions when they parse, so a literal never reports a syntax
    /// error.
    pub tentative: bool,
}

impl<'a> EmbeddedExpression<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        kind: ExpressionKind,
        source: &'a JsonString<'a>,
        context: ReferenceContext,
        owner: Owner,
        binding: Option<BindingId>,
        node_path: String,
        ign_lint_path: Option<String>,
        tentative: bool,
    ) -> Option<Self> {
        let parsed = iglint_expression::parse(source.value());
        if tentative && !parsed.is_valid() {
            return None;
        }
        Some(EmbeddedExpression {
            kind,
            source,
            parsed,
            context,
            source_map: SourceMap::json(TextSize::ZERO, source.range(), source),
            owner,
            binding,
            node_path,
            ign_lint_path,
            tentative,
        })
    }

    /// The expression text (decoded).
    pub fn text(&self) -> &'a str {
        let source: &'a JsonString<'a> = self.source;
        source.value()
    }

    /// Maps a range in [`EmbeddedExpression::text`] to the file.
    pub fn file_range(&self, range: TextRange) -> TextRange {
        self.source_map.file_range(range)
    }

    /// Every `{...}` reference with its classification for this location.
    pub fn references(&self) -> Vec<(&BoundRef, Reference)> {
        self.parsed
            .expr
            .references()
            .into_iter()
            .map(|r| (r, r.resolve(self.context)))
            .collect()
    }
}
