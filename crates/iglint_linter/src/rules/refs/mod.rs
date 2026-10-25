//! REF: references between resources.

pub mod library_function_arity;
#[cfg(test)]
mod library_routes;
pub(crate) mod library_uses;
pub mod manifest_mismatch;
pub mod named_query_params;
pub mod popup_params;
pub(crate) mod script_calls;
pub mod unhandled_message_handler;
pub mod unknown_library_member;
pub mod unknown_message_type;
pub mod unknown_named_query;
pub mod unknown_page_url;
pub mod unknown_parent_project;
pub mod unknown_view;
pub mod unused_library_function;
pub mod unused_named_query;
pub mod unused_view;

use iglint_json::Value;
use iglint_project::kinds::ResourceKind;
use iglint_project::script::Owner;
use iglint_project::view::View;
use iglint_project::{Container, ProjectId, Workspace};

/// Whether a project sees resources whose scripts iglint does not model
/// (Vision windows and templates, reports, alarm pipelines, SFC charts,
/// event streams). Such scripts may reference anything, so "unused" rules
/// stay silent in that project.
/// Whether code outside a library module may use a top-level name of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExternalUse {
    /// Another resource references `pkg.mod.name`, or uses the module as a
    /// value (and may read any of its names).
    Used,
    /// Not referenced, and every reference in the project is known.
    Unused,
    /// Cannot be ruled out: references are computed at run time, the
    /// project is inheritable or has scripts iglint does not model, or the
    /// project is unknown.
    Unknown,
}

/// How the library module being checked has its top-level `name` used by
/// other resources, through the project index.
pub(crate) fn external_use(cx: &crate::checker::Checker<'_, '_>, name: &str) -> ExternalUse {
    let (Some((index, project)), Some(resource)) = (cx.project_index(), cx.resource()) else {
        return ExternalUse::Unknown;
    };
    if index.external_symbol_usage_count(resource.id, name) > 0 {
        return ExternalUse::Used;
    }
    // The module is used as a value somewhere (`m = core.util`, a default
    // argument, a container, `getGlobals()`, `sys.modules['core.util']`):
    // whoever holds it may read any of its names, so each counts as used.
    if std::iter::once(project)
        .chain(cx.workspace().project_descendants(project))
        .any(|p| index.module_used_as_value(p, resource.id))
    {
        return ExternalUse::Used;
    }
    // Code that is not vetted (`cPickle.loads` of a `c<module>` opcode,
    // say) may read any module's names by name.
    let unknown_code = std::iter::once(project)
        .chain(cx.workspace().project_descendants(project))
        .any(|p| index.has_unknown_code(p));
    if !unknown_code && all_library_references_known(cx.workspace(), index, project, resource.id) {
        ExternalUse::Unused
    } else {
        ExternalUse::Unknown
    }
}

/// Whether a resource of the project (or of a project inheriting from it)
/// assigns the top-level `name` of the library module being checked, or
/// deletes it, through any path ([`iglint_project::ProjectIndex::library_name_stored`]:
/// `lib.util.json = 5`, `m0.util.json = 5` with `m0` importing `lib.util`,
/// `setattr(lib.util, 'json', 5)`). A store replaces the binding, so a fix
/// that removes or changes it is not known to keep what runs (review
/// round 29). Fails closed: an unknown context counts as a store.
pub(crate) fn name_stored_elsewhere(cx: &crate::checker::Checker<'_, '_>, name: &str) -> bool {
    let (Some((index, project)), Some(resource)) = (cx.project_index(), cx.resource()) else {
        return true;
    };
    let dotted = format!("{}.{name}", resource.path.replace('/', "."));
    std::iter::once(project)
        .chain(cx.workspace().project_descendants(project))
        .any(|p| index.library_name_stored(p, &dotted))
}

/// Whether every reference to the top-level names of library `module` is
/// visible in the index (no dynamic access, no unknown users).
pub(crate) fn all_library_references_known(
    ws: &Workspace,
    index: &iglint_project::ProjectIndex<'_>,
    project: ProjectId,
    module: iglint_project::ResourceId,
) -> bool {
    !(ws.project(project).inheritable()
        || !ws.project(project).inheritance.complete
        || has_unmodelled_scripts(ws, project)
        || index.module_escapes(project, module)
        || index
            .dynamic_references(project)
            .library
            .iter()
            .any(String::is_empty))
}

pub(crate) fn has_unmodelled_scripts(ws: &Workspace, project: ProjectId) -> bool {
    ws.effective_resources(Container::Project(project))
        .into_iter()
        .chain(
            ws.resources
                .iter()
                .filter(|r| matches!(r.container, Container::Collection(_)) && !r.manifest.missing)
                .map(|r| r.id),
        )
        .any(|r| {
            matches!(
                ws.resource(r).kind,
                ResourceKind::VisionWindow
                    | ResourceKind::VisionTemplate
                    | ResourceKind::Report
                    | ResourceKind::AlarmPipeline
                    | ResourceKind::SfcChart
                    | ResourceKind::EventStream
            )
        })
}

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
            Some(i) => part.split_at(i),
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
