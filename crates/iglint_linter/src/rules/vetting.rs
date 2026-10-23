//! Whether code uses only vetted facilities, for safe fixes that depend on
//! what code can read (see `iglint_python_semantic::vetted`).

use iglint_project::library::LibraryLookup;
use iglint_project::{ProjectId, ProjectIndex, ResourceId};
use iglint_python::ast::Stmt;
use iglint_python_semantic::SemanticModel;
use rustc_hash::FxHashSet;

use crate::checker::Checker;

/// Whether `body` (a script, `model` its semantic model) or a project
/// library module it imports or uses (transitively) uses something that
/// is not vetted: a module, Java class, builtin or module attribute off
/// the allowlists, an import that is not a vetted module or a project
/// library module, or a vetted module name that a project library package
/// shadows. Without the project index every import and library name is
/// unknown.
pub(crate) fn uses_unknown_code(
    cx: &Checker<'_, '_>,
    model: &SemanticModel<'_>,
    body: &[Stmt],
) -> bool {
    let unknown = iglint_python_semantic::vetted::unknown_code(model);
    if unknown.other.is_some() {
        return true;
    }
    let names = crate::script_facts::access::library_names_used(model, body);
    if unknown.imports.is_empty() && names.is_empty() && unknown.vetted_imports.is_empty() {
        return false;
    }
    let Some((index, project)) = cx.project_index() else {
        return !unknown.imports.is_empty() || !names.is_empty();
    };
    let (globals, complete) = index.script_library_globals(project);
    if !complete
        || unknown
            .vetted_imports
            .iter()
            .any(|r| globals.contains(&r.as_str()))
    {
        return true;
    }
    let mut seen = FxHashSet::default();
    unknown
        .imports
        .iter()
        .any(|(name, _)| !library_vetted(index, project, name, true, &mut seen))
        || names
            .iter()
            .any(|name| !library_vetted(index, project, name, false, &mut seen))
}

/// Whether the library name `dotted` is vetted code: a package, or a
/// module or a name of a module whose code is vetted. An import of
/// anything else is not; a used name that does not exist is (using it
/// raises).
fn library_vetted(
    index: &ProjectIndex<'_>,
    project: ProjectId,
    dotted: &str,
    import: bool,
    seen: &mut FxHashSet<ResourceId>,
) -> bool {
    if dotted.is_empty() {
        // A relative import.
        return false;
    }
    match index.resolve_library(project, dotted) {
        LibraryLookup::Package { .. } => true,
        LibraryLookup::Module { resource, .. } | LibraryLookup::Symbol { resource, .. } => {
            module_vetted(index, project, resource, seen)
        }
        LibraryLookup::NotLibrary => !import,
        LibraryLookup::Missing { .. }
        | LibraryLookup::HiddenByModule { .. }
        | LibraryLookup::MissingAttribute { .. } => !import,
        LibraryLookup::Unknown => false,
    }
}

/// Whether the code of library module `resource`, and of every library
/// module it imports or uses, is vetted.
fn module_vetted(
    index: &ProjectIndex<'_>,
    project: ProjectId,
    resource: ResourceId,
    seen: &mut FxHashSet<ResourceId>,
) -> bool {
    if !seen.insert(resource) {
        return true;
    }
    let Some(symbols) = index.module_symbols(resource) else {
        return false;
    };
    let (globals, _) = index.script_library_globals(project);
    !symbols.unknown_code
        && symbols
            .imports
            .iter()
            .all(|name| library_vetted(index, project, name, true, seen))
        && symbols
            .library_names
            .iter()
            .filter(|n| globals.contains(&n.split('.').next().unwrap_or(n)))
            .all(|name| library_vetted(index, project, name, false, seen))
}

/// Whether a script of the unit (a view's scripts, say) uses code that is
/// not vetted ([`uses_unknown_code`]). Scripts that do not parse never
/// run.
pub(crate) fn unit_uses_unknown_code(cx: &Checker<'_, '_>) -> bool {
    cx.unit()
        .data
        .scripts()
        .iter()
        .zip(cx.parsed_scripts())
        .any(|((_, script), parsed)| {
            if !parsed.errors.is_empty() {
                return false;
            }
            let model = SemanticModel::build(
                &parsed.module,
                &iglint_python_semantic::SemanticOptions {
                    injected: script.implicit_globals().collect(),
                    unknown_globals: false,
                },
            );
            uses_unknown_code(cx, &model, &parsed.module.body)
        })
}

/// Whether any script anywhere in the workspace uses code that is not
/// vetted or reads a frame or the interpreter
/// ([`iglint_project::ProjectIndex::has_any_unknown_code`]). The Ignition
/// runtime is shared, so such code in any project or gateway-scoped script
/// can observe any module's names and any frame's locals (through
/// `sys._current_frames`, reflection or a `cPickle` opcode). Safe fixes
/// that assume a name or value is not observed (`SCR102`, `VIEW015`,
/// `NAME001`) are only safe when the whole closed world is free of it.
/// With no project index (a single file linted on its own) this is
/// unknown; the per-rule context checks (incomplete inheritance, an
/// inheritable project, unmodelled scripts) already fail closed there.
pub(crate) fn workspace_has_unknown_code(cx: &Checker<'_, '_>) -> bool {
    cx.project_index()
        .is_some_and(|(index, _)| index.has_any_unknown_code())
}
