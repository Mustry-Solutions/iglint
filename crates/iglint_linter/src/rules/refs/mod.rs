//! REF: references between resources.

pub mod manifest_mismatch;
pub mod named_query_params;
pub mod popup_params;
pub mod unknown_named_query;
pub mod unknown_page_url;
pub mod unknown_parent_project;
pub mod unknown_view;
pub mod unused_named_query;
pub mod unused_view;

use iglint_json::Value;
use iglint_project::kinds::ResourceKind;
use iglint_project::script::Owner;
use iglint_project::view::View;
use iglint_project::{Container, ProjectId, Workspace};

/// The effective resource of `kind` whose path equals `path` ignoring
/// case, for "did you mean" hints when a lookup fails.
pub(crate) fn case_insensitive_match(
    ws: &Workspace,
    project: ProjectId,
    kind: ResourceKind,
    path: &str,
) -> Option<String> {
    let path = path.trim_start_matches('/');
    ws.effective_resources(Container::Project(project))
        .into_iter()
        .map(|r| ws.resource(r))
        .find(|r| r.kind == kind && r.path.eq_ignore_ascii_case(path) && &*r.path != path)
        .map(|r| r.path.to_string())
}

/// The JSON object that owns properties of `owner` in a view: the component
/// object, or the view document.
pub(crate) fn owner_value<'a>(view: &View<'a>, owner: Owner) -> Option<&'a Value<'a>> {
    match owner {
        Owner::Component(id) => Some(view.component(id).value),
        Owner::View => Some(view.root),
        _ => None,
    }
}

/// The value at a property path such as `props.columns[3].render` below
/// `root`.
pub(crate) fn value_at<'v, 'a>(root: &'v Value<'a>, path: &str) -> Option<&'v Value<'a>> {
    let mut at = root;
    for part in path.split('.') {
        let (key, indexes) = match part.find('[') {
            Some(i) => (&part[..i], &part[i..]),
            None => (part, ""),
        };
        if !key.is_empty() {
            at = at.get(key)?;
        }
        for index in indexes.split('[').filter(|s| !s.is_empty()) {
            let i: usize = index.trim_end_matches(']').parse().ok()?;
            at = at.index(i)?;
        }
    }
    Some(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_at_paths() {
        let root = iglint_json::parse(
            r#"{"props": {"columns": [{"render": "auto"}, {"render": "view"}]}}"#,
        )
        .unwrap();
        assert_eq!(
            value_at(&root, "props.columns[1].render").and_then(Value::as_str),
            Some("view")
        );
        assert!(value_at(&root, "props.columns[2].render").is_none());
    }
}
