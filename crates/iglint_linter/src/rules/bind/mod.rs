//! BIND: bindings, transforms and expressions.

use iglint_project::model::ModelData;
use iglint_project::view::{Binding, BindingConfig};
use iglint_project::{EmbeddedExpression, ExprId, ExpressionKind, FileId};

pub mod disabled_binding;
pub mod expression_function_arity;
pub mod expression_syntax_error;
pub mod expression_type_mismatch;
pub mod indirect_tag_reference_mismatch;
pub mod map_transform_duplicate_input;
pub mod placeholder_in_direct_tag_path;
pub mod polling_interval;
pub mod transform_without_return;
pub mod unknown_expression_function;
pub mod unresolved_property_reference;

/// The bindings of a view or of session properties.
pub(crate) fn host_bindings<'d, 'a>(data: &'d ModelData<'a>) -> &'d [Binding<'a>] {
    match data {
        ModelData::View { view, .. } => &view.bindings,
        ModelData::SessionProps { props, .. } => &props.bindings,
        _ => &[],
    }
}

/// The expression arena of a view or of session properties.
pub(crate) fn host_expressions<'d, 'a>(data: &'d ModelData<'a>) -> &'d [EmbeddedExpression<'a>] {
    match data {
        ModelData::View { view, .. } => &view.expressions,
        ModelData::SessionProps { props, .. } => &props.expressions,
        _ => &[],
    }
}

/// The file the model's expressions live in, for views and session
/// properties; for tags the expression's tag file.
pub(crate) fn expression_file(data: &ModelData<'_>, e: &EmbeddedExpression<'_>) -> Option<FileId> {
    data.expressions()
        .into_iter()
        .find(|(_, x)| std::ptr::eq(*x, e))
        .map(|(f, _)| f)
}

fn is(data: &ModelData<'_>, id: ExprId, e: &EmbeddedExpression<'_>) -> bool {
    host_expressions(data)
        .get(id.index())
        .is_some_and(|x| std::ptr::eq(x, e))
}

/// The node path ign-lint uses (or would use) for the location of `e` in a
/// view: expression bindings, expression-structure leaves
/// (`{binding}.{key}`), query parameters (`{binding}.{param}`), tag
/// bindings in expression mode, transforms and other binding expressions
/// by their own ign-lint style path. `None` outside views.
pub(crate) fn ign_lint_expression_path(
    data: &ModelData<'_>,
    e: &EmbeddedExpression<'_>,
) -> Option<String> {
    if !matches!(data, ModelData::View { .. }) {
        return None;
    }
    let Some(binding) = e.binding.and_then(|b| host_bindings(data).get(b.index())) else {
        return e.ign_lint_path.clone();
    };
    match e.kind {
        ExpressionKind::Binding | ExpressionKind::TagPathExpression | ExpressionKind::Transform => {
            e.ign_lint_path.clone()
        }
        ExpressionKind::StructLeaf => match &binding.typed {
            BindingConfig::ExprStruct { leaves, .. } => leaves
                .iter()
                .find(|l| is(data, l.expression, e))
                .map(|l| format!("{}.{}", binding.ign_lint_path, l.key)),
            _ => None,
        },
        ExpressionKind::QueryParameter => match &binding.typed {
            BindingConfig::Query(q) => q
                .parameters
                .iter()
                .find(|p| p.expression.is_some_and(|id| is(data, id, e)))
                .map(|p| format!("{}.{}", binding.ign_lint_path, p.name.value())),
            _ => None,
        },
        _ => e.ign_lint_path.clone(),
    }
}
