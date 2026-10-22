//! Script call sites of the reference rules: the API calls of a script
//! with their resource targets, and helpers to check the literals they
//! pass.

use iglint_project::{ProjectId, ProjectIndex};
use iglint_python::ast::Expr;
use iglint_python_semantic::{SemanticModel, UnboundNames};

use crate::checker::Checker;
use crate::rule::ScriptUnit;
use crate::rules::script::semantic;
use crate::script_facts::api::{ApiTarget, api_target};
use crate::script_facts::calls::{CallSite, call_sites, is_project_name_call, string_values};

/// The API calls of a script that name resources, with the semantic model.
/// Empty for scripts that do not parse or belong to no project.
pub(crate) fn api_targets<'u, 's>(
    cx: &Checker<'_, '_>,
    unit: &'u ScriptUnit<'s, '_>,
) -> Option<(&'u SemanticModel<'s>, Vec<(CallSite<'s>, ApiTarget<'s>)>)> {
    cx.project_index()?;
    let model = semantic(cx, unit)?;
    let sites = call_sites(model, &model.module.body, UnboundNames::Reject);
    let targets = sites
        .into_iter()
        .filter_map(|s| {
            let t = api_target(model, &s)?;
            Some((s, t))
        })
        .collect();
    Some((model, targets))
}

/// The project a `project` argument names: the script's own project for
/// `system.util.getProjectName()`, a workspace project for a literal
/// naming one. `None` when unknown.
pub(crate) fn project_argument(
    cx: &Checker<'_, '_>,
    model: &SemanticModel<'_>,
    project: &Expr,
) -> Option<ProjectId> {
    if is_project_name_call(model, project) {
        return cx.project();
    }
    let values = string_values(model, project)?;
    let [value] = values.as_slice() else {
        return None;
    };
    cx.workspace()
        .projects
        .iter()
        .find(|p| p.name == value.value)
        .map(|p| p.id)
}

/// Where a named query call looks its query up: the project and the path
/// expression. `None` when that cannot be decided.
pub(crate) fn named_query_lookup<'e>(
    cx: &Checker<'_, '_>,
    model: &SemanticModel<'_>,
    index: &ProjectIndex<'_>,
    call: &crate::script_facts::api::NamedQueryCall<'e>,
) -> Option<(ProjectId, &'e Expr)> {
    let own = cx.project()?;
    if let Some(project) = call.project {
        return Some((project_argument(cx, model, project)?, call.path));
    }
    if !call.ambiguous {
        return Some((own, call.path));
    }
    // `(first, second)` with a second argument of unknown type: the first
    // is a path when it cannot be a project name.
    let values = string_values(model, call.path)?;
    let path_form = values.iter().all(|v| {
        v.value.contains('/')
            || (!cx.workspace().projects.iter().any(|p| p.name == v.value)
                && !matches!(
                    index.named_query(own, &v.value),
                    iglint_project::Lookup::Missing
                ))
    });
    path_form.then_some((own, call.path))
}
