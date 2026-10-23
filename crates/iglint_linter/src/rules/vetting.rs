//! Whether code uses only vetted facilities, for safe fixes that depend on
//! what code can read (see `iglint_python_semantic::vetted`).

use iglint_project::library::LibraryLookup;
use iglint_project::script::EmbeddedScript;
use iglint_project::{FileId, ProjectId, ProjectIndex, ResourceId};
use iglint_python::ast::Stmt;
use iglint_python_semantic::SemanticModel;
use iglint_text::TextRange;
use rustc_hash::FxHashSet;

use crate::checker::{Checker, FileTable};
use crate::diagnostic::{Diagnostic, ReasonSite, UnsafeReason};

/// Where code is not vetted ([`unknown_code_at`]).
#[derive(Clone, Copy, Debug)]
pub(crate) enum UnknownAt {
    /// In the script itself, at this range of its source.
    Here(TextRange),
    /// In a project library module the script imports or calls, or a
    /// vetted module name that a library package shadows.
    Library,
}

/// Where `body` (a script, `model` its semantic model) or a project
/// library module it imports or uses (transitively) uses something that
/// is not vetted: a module, Java class, builtin or module attribute off
/// the allowlists, an import that is not a vetted module or a project
/// library module, or a vetted module name that a project library package
/// shadows. Without the project index every import and library name is
/// unknown. The first use in the script itself, else the library code.
pub(crate) fn unknown_code_at(
    cx: &Checker<'_, '_>,
    model: &SemanticModel<'_>,
    body: &[Stmt],
) -> Option<UnknownAt> {
    let unknown = iglint_python_semantic::vetted::unknown_code(model);
    if let Some(range) = unknown.other {
        return Some(UnknownAt::Here(range));
    }
    let names = crate::script_facts::access::library_names_used(model, body);
    if unknown.imports.is_empty() && names.is_empty() && unknown.vetted_imports.is_empty() {
        return None;
    }
    let Some((index, project)) = cx.project_index() else {
        return match unknown.imports.first() {
            Some((_, range)) => Some(UnknownAt::Here(*range)),
            None => (!names.is_empty()).then_some(UnknownAt::Library),
        };
    };
    let (globals, complete) = index.script_library_globals(project);
    if !complete
        || unknown
            .vetted_imports
            .iter()
            .any(|r| globals.contains(&r.as_str()))
    {
        return Some(UnknownAt::Library);
    }
    let mut seen = FxHashSet::default();
    for (name, range) in &unknown.imports {
        if !library_vetted(index, project, name, true, &mut seen) {
            // A module that is not a library module is unknown here; a
            // library module whose code is not vetted, there.
            return Some(match index.resolve_library(project, name) {
                LibraryLookup::Module { .. } | LibraryLookup::Symbol { .. } => UnknownAt::Library,
                _ => UnknownAt::Here(*range),
            });
        }
    }
    names
        .iter()
        .any(|name| !library_vetted(index, project, name, false, &mut seen))
        .then_some(UnknownAt::Library)
}

/// Why a fix is unsafe when the script `script` (in `file`) uses code that
/// is not vetted ([`unknown_code_at`]).
pub(crate) fn script_reason(
    cx: &Checker<'_, '_>,
    file: FileId,
    script: &EmbeddedScript<'_>,
    at: UnknownAt,
) -> UnsafeReason {
    match at {
        UnknownAt::Here(range) => {
            let site = cx.file(file).map(|source| {
                let start = script.file_range(range).start();
                let at = source.line_index().line_column(start, source.text());
                ReasonSite {
                    path: source.path().to_path_buf(),
                    line: at.line,
                    column: at.column,
                    what: crate::script_facts::access::snippet(&script.source, range),
                }
            });
            UnsafeReason {
                message: "the script uses code that is not vetted".to_owned(),
                site,
                workspace: false,
            }
        }
        UnknownAt::Library => UnsafeReason {
            message: "the script uses a library module that is not vetted; first such code in \
                      the workspace"
                .to_owned(),
            site: None,
            workspace: true,
        },
    }
}

/// Why a fix is unsafe under the closed-world rule
/// ([`workspace_has_unknown_code`]): the site, the first use in the
/// workspace, is filled in after the cache from the project index.
pub(crate) fn workspace_reason() -> UnsafeReason {
    UnsafeReason {
        message: "closed world: code in the workspace is not vetted or reads frames, first"
            .to_owned(),
        site: None,
        workspace: true,
    }
}

/// Why a fix of the unit is unsafe because of code that is not vetted:
/// the unit's own first script that uses some, else the closed-world
/// rule. `None` when neither applies.
pub(crate) fn unit_reason(cx: &Checker<'_, '_>) -> Option<UnsafeReason> {
    for ((file, script), parsed) in cx.unit().data.scripts().iter().zip(cx.parsed_scripts()) {
        if !parsed.errors.is_empty() {
            continue;
        }
        let model = SemanticModel::build(
            &parsed.module,
            &iglint_python_semantic::SemanticOptions {
                injected: script.implicit_globals().collect(),
                unknown_globals: false,
            },
        );
        if let Some(at) = unknown_code_at(cx, &model, &parsed.module.body) {
            return Some(script_reason(cx, *file, script, at));
        }
    }
    workspace_has_unknown_code(cx).then(workspace_reason)
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

/// The first use of code that is not vetted, or that reads a frame, in the
/// workspace ([`ProjectIndex::first_unknown_code`]), placed in its file.
pub(crate) fn first_unknown_site(
    index: &ProjectIndex<'_>,
    files: &FileTable,
) -> Option<ReasonSite> {
    let site = index.first_unknown_code()?;
    let source = files.get(site.file)?;
    let at = source
        .line_index()
        .line_column(site.range.start(), source.text());
    Some(ReasonSite {
        path: source.path().to_path_buf(),
        line: at.line,
        column: at.column,
        what: site.what.clone(),
    })
}

/// Fills in the site of closed-world reasons ([`UnsafeReason::workspace`])
/// that have none: findings computed now, and findings restored from the
/// cache, which never keeps a site of another file.
pub(crate) fn complete_reasons(diagnostics: &mut [Diagnostic], site: Option<&ReasonSite>) {
    let Some(site) = site else { return };
    for d in diagnostics {
        if let Some(reason) = d.fix.as_mut().and_then(|f| f.unsafe_reason.as_mut())
            && reason.workspace
            && reason.site.is_none()
        {
            reason.site = Some(site.clone());
        }
    }
}
